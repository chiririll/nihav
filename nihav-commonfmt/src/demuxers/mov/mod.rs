use nihav_core::demuxers::*;
use nihav_registry::register::*;
use nihav_core::compr::deflate::*;

macro_rules! mktag {
    ($a:expr, $b:expr, $c:expr, $d:expr) => ({
        (($a as u32) << 24) | (($b as u32) << 16) | (($c as u32) << 8) | ($d as u32)
    });
    ($arr:expr) => ({
        (($arr[0] as u32) << 24) | (($arr[1] as u32) << 16) | (($arr[2] as u32) << 8) | ($arr[3] as u32)
    });
}

#[derive(Clone,Copy,Debug,PartialEq)]
enum DemuxMode {
    Normal,
    MacBin,
    ResFork,
}

trait Skip64 {
    fn skip64(&mut self, size: u64) -> ByteIOResult<()>;
}

impl<T: ?Sized + ByteIO> Skip64 for T {
    fn skip64(&mut self, size: u64) -> ByteIOResult<()> {
        if (size as usize as u64) != size {
            self.seek(SeekFrom::Current(size as i64))?;
        } else {
            self.read_skip(size as usize)?;
        }
        Ok(())
    }
}

fn read_chunk_header(br: &mut dyn ByteIO) -> DemuxerResult<(u32, u64)> {
    let size            = br.read_u32be()?;
    let ctype           = br.read_u32be()?;
    if size == 0 {
        Ok((ctype, br.left() as u64))
    } else if size == 1 {
        let size64      = br.read_u64be()?;
        validate!(size64 >= 16);
        Ok((ctype, size64 - 16))
    } else {
        validate!(size >= 8);
        Ok((ctype, (size as u64) - 8))
    }
}

fn read_palette(br: &mut dyn ByteIO, size: u64, pal: &mut [u8; 1024]) -> DemuxerResult<u64> {
    let _seed           = br.read_u32be()?;
    let flags           = br.read_u16be()?;
    let palsize         = (br.read_u16be()? as usize) + 1;
    validate!(palsize <= 256);
    validate!(flags == 0 || flags == 0x4000 || flags == 0x8000);
    validate!((palsize as u64) * 8 + 8 <= size);
    for i in 0..palsize {
        let a           = br.read_u16be()?;
        let r           = br.read_u16be()?;
        let g           = br.read_u16be()?;
        let b           = br.read_u16be()?;
        pal[i * 4]     = (r >> 8) as u8;
        pal[i * 4 + 1] = (g >> 8) as u8;
        pal[i * 4 + 2] = (b >> 8) as u8;
        if flags == 0x8000 {
            pal[i * 4 + 3] = (a >> 8) as u8;
        }
    }
    if flags == 0x4000 {
        br.read_skip(8)?;
    }
    Ok(size)
}

struct RootChunkHandler {
    ctype:  u32,
    parse:  fn(dmx: &mut MOVDemuxer, strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64>,
}

const IGNORED_CHUNKS: &[u32] = &[
    mktag!(b"free"), mktag!(b"skip"), mktag!(b"udta"), mktag!(b"wide")
];

const ROOT_CHUNK_HANDLERS: &[RootChunkHandler] = &[
    RootChunkHandler { ctype: mktag!(b"ftyp"), parse: read_ftyp },
    RootChunkHandler { ctype: mktag!(b"mdat"), parse: read_mdat },
    RootChunkHandler { ctype: mktag!(b"moov"), parse: read_moov },
    RootChunkHandler { ctype: mktag!(b"moof"), parse: read_moof },
    RootChunkHandler { ctype: mktag!(b"sidx"), parse: read_sidx },
];

fn print_cname(ctype: u32, size: u64, off: u64, depth: u8) {
    for _ in 0..depth { print!("    "); }
    let tag = [(ctype >> 24) as u8, (ctype >> 16) as u8, (ctype >> 8) as u8, ctype as u8];
    let mut printable = true;
    for ch in tag.iter() {
        if !(0x20..=0x7F).contains(ch) {
            printable = false;
            break;
        }
    }
    if printable {
        print!(" '{}{}{}{}'", tag[0] as char, tag[1] as char, tag[2] as char, tag[3] as char);
    } else {
        print!(" {:08X}", ctype);
    }
    println!(" size {} @ {:X}", size, off);
}

macro_rules! read_chunk_list {
    (root; $name: expr, $fname: ident, $handlers: ident) => {
        fn $fname(&mut self, strmgr: &mut StreamManager, size: u64) -> DemuxerResult<()> {
            self.depth += 1;
            validate!(self.depth < 32);
            let list_end = self.src.tell() + size;
            while self.src.tell() < list_end {
                let ret = read_chunk_header(self.src);
                if ret.is_err() { break; }
                let (ctype, size) = ret.unwrap();
                if self.print_chunks {
                    print_cname(ctype, size, self.src.tell(), self.depth as u8);
                }
                if self.src.tell() + size > list_end {
                    break;
                }
                if IGNORED_CHUNKS.contains(&ctype) {
                    self.src.skip64(size)?;
                    continue;
                }
                let handler = $handlers.iter().find(|x| x.ctype == ctype);
                let read_size;
                if let Some(ref handler) = handler {
                    read_size = (handler.parse)(self, strmgr, size)?;
                } else {
                    println!("skipping unknown chunk {:08X} size {}", ctype, size);
                    read_size = 0;
                }
                validate!(read_size <= size);
                self.src.skip64(size - read_size)?;
            }
            self.depth -= 1;
            validate!(self.src.tell() == list_end);
            Ok(())
        }
    };
    (track; $name: expr, $fname: ident, $handlers: ident) => {
        fn $fname(&mut self, br: &mut dyn ByteIO, size: u64) -> DemuxerResult<()> {
            self.depth += 1;
            validate!(self.depth < 32);
            let list_end = br.tell() + size;
            while br.tell() < list_end {
                let ret = read_chunk_header(br);
                if ret.is_err() { break; }
                let (ctype, size) = ret.unwrap();
                if self.print_chunks {
                    print_cname(ctype, size, br.tell(), self.depth + 1);
                }
                if br.tell() + size > list_end {
                    break;
                }
                if IGNORED_CHUNKS.contains(&ctype) {
                    br.skip64(size)?;
                    continue;
                }
                let handler = $handlers.iter().find(|x| x.ctype == ctype);
                let read_size;
                if let Some(ref handler) = handler {
                    read_size = (handler.parse)(self, br, size)?;
                } else {
                    read_size = 0;
                }
                validate!(read_size <= size);
                br.skip64(size - read_size)?;
            }
            self.depth -= 1;
            validate!(br.tell() == list_end);
            Ok(())
        }
    }
}

mod track;
use track::*;
mod pktread;

fn read_ftyp(dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    dmx.src.skip64(size)?;
    Ok(size)
}

fn read_mdat(dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    dmx.mdat_pos  = dmx.src.tell();
    dmx.mdat_size = size;
    dmx.src.skip64(size)?;
    Ok(size)
}

fn read_sidx(_dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, _size: u64) -> DemuxerResult<u64> {
    Ok(0)
}

fn read_moov(dmx: &mut MOVDemuxer, strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    dmx.read_moov(strmgr, size)?;
    Ok(size)
}

const MOOV_CHUNK_HANDLERS: &[RootChunkHandler] = &[
    RootChunkHandler { ctype: mktag!(b"mvhd"), parse: read_mvhd },
    RootChunkHandler { ctype: mktag!(b"cmov"), parse: read_cmov },
    RootChunkHandler { ctype: mktag!(b"ctab"), parse: read_ctab },
    RootChunkHandler { ctype: mktag!(b"trak"), parse: read_trak },
    RootChunkHandler { ctype: mktag!(b"meta"), parse: read_meta },
    RootChunkHandler { ctype: mktag!(b"mvex"), parse: read_mvex },
    RootChunkHandler { ctype: mktag!(b"iods"), parse: skip_chunk_mov },
];

fn read_mvhd(dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    const KNOWN_MVHD_SIZE: u64 = 100;
    let br = &mut dmx.src;
    validate!(size >= KNOWN_MVHD_SIZE);
    let version             = br.read_byte()?;
    validate!(version == 0 || version == 0xFF);
    dmx.ver_m1 = version == 0xFF;
    let _flags              = br.read_u24be()?;
    let _ctime              = br.read_u32be()?;
    let _mtime              = br.read_u32be()?;
    let tscale              = br.read_u32be()?;
    let duration            = br.read_u32be()?;
    let _pref_rate          = br.read_u32be()?;
    let _pref_volume        = br.read_u16be()?;
                              br.read_skip(10)?;
                              br.read_skip(36)?; // matrix
    let _preview_time       = br.read_u32be()?;
    let _preview_duration   = br.read_u32be()?;
    let _poster_time        = br.read_u32be()?;
    let _sel_time           = br.read_u32be()?;
    let _sel_duration       = br.read_u32be()?;
    let _cur_time           = br.read_u32be()?;
    let _next_track_id      = br.read_u32be()?;
    dmx.duration = duration;
    dmx.tb_den = tscale;

    Ok(KNOWN_MVHD_SIZE)
}

fn read_cmov(dmx: &mut MOVDemuxer, strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    let br = &mut dmx.src;
    validate!(size > 24);
    let dcom_size           = br.read_u32be()?;
    let dcom_tag            = br.read_tag()?;
    let compr_type          = br.read_tag()?;
    validate!(&dcom_tag == b"dcom" && dcom_size == 12);
    if &compr_type != b"zlib" {
        return Err(DemuxerError::NotImplemented);
    }
    let cmvd_size           = u64::from(br.read_u32be()?);
    let cmvd_tag            = br.read_tag()?;
    validate!(&cmvd_tag == b"cmvd" && cmvd_size > 14 && cmvd_size == size - 12);
    let comp_size = (cmvd_size - 12) as usize;
    let uncomp_size         = br.read_u32be()? as usize;
    validate!(uncomp_size > 8);
    let mut sbuf = vec![0; comp_size];
    let mut dbuf = vec![0; uncomp_size];
                              br.read_buf(sbuf.as_mut_slice())?;
    let ret = Inflate::uncompress(sbuf.as_slice(), dbuf.as_mut_slice());
    if ret.is_err() {
        return Err(DemuxerError::InvalidData);
    }
    let len = ret.unwrap();
    validate!(len == uncomp_size);
    let mut br = MemoryReader::new_read(dbuf.as_slice());
    let (ctype, csize) = read_chunk_header(&mut br)?;
    validate!(ctype == mktag!(b"moov"));
    let mut ddmx = MOVDemuxer::new(&mut br);
    ddmx.print_chunks = dmx.print_chunks;
    ddmx.read_moov(strmgr, csize)?;
    std::mem::swap(&mut dmx.tracks, &mut ddmx.tracks);
    dmx.duration = ddmx.duration;
    dmx.tb_den = ddmx.tb_den;
    std::mem::swap(&mut dmx.pal, &mut ddmx.pal);

    Ok(size)
}

fn read_ctab(dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    let mut pal = [0; 1024];
    let size = read_palette(dmx.src, size, &mut pal)?;
    dmx.pal = Some(Arc::new(pal));
    Ok(size)
}

fn read_meta(dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    dmx.src.skip64(size)?;
    Ok(size)
}

fn read_mvex(_dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, _size: u64) -> DemuxerResult<u64> {
    Ok(0)
}

fn skip_chunk_mov(_dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, _size: u64) -> DemuxerResult<u64> {
    Ok(0)
}

fn read_trak(dmx: &mut MOVDemuxer, strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    let mut track = Track::new(dmx.cur_track as u32, dmx.tb_den);
    track.print_chunks = dmx.print_chunks;
    track.ver_m1 = dmx.ver_m1;
    track.call_read_trak(dmx.src, size)?;
    validate!(track.tkhd_found);
    if !track.stsd_found {
        track.stream = Some(NAStream::new(StreamType::Data, track.track_no, DUMMY_CODEC_INFO, track.tb_num, track.tb_den, 0));
    }
    // invent keyframes for video stream if none were reported
    if !track.stss_found && track.stream_type == StreamType::Video {
        let mut intraonly = false;
        if let Some(ref stream) = track.stream {
            if let Some(desc) = get_codec_description(stream.get_info().get_name()) {
                intraonly = (desc.caps & CODEC_CAP_INTRAONLY) != 0;
            }
        }

        track.pkt_demux.invent_keyframes(intraonly);
    }
    validate!(strmgr.get_stream_by_id(track.track_id).is_none());
    dmx.cur_track += 1;
    dmx.tracks.push(track);
    Ok(size)
}

fn read_moof(dmx: &mut MOVDemuxer, strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    dmx.moof_off = dmx.src.tell() - 8;
    dmx.read_moof(strmgr, size)?;
    Ok(size)
}

const MOOF_CHUNK_HANDLERS: &[RootChunkHandler] = &[
    RootChunkHandler { ctype: mktag!(b"mfhd"), parse: read_mfhd },
    RootChunkHandler { ctype: mktag!(b"traf"), parse: read_traf },
    RootChunkHandler { ctype: mktag!(b"meta"), parse: read_meta },
];

fn read_mfhd(dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    const KNOWN_MFHD_SIZE: u64 = 8;
    validate!(size >= KNOWN_MFHD_SIZE);
    let version             = dmx.src.read_byte()?;
    validate!(version == 0);
    let flags               = dmx.src.read_u24be()?;
    validate!(flags == 0);
    let _seq_no             = dmx.src.read_u32be()?;

    Ok(KNOWN_MFHD_SIZE)
}

fn read_traf(dmx: &mut MOVDemuxer, _strmgr: &mut StreamManager, size: u64) -> DemuxerResult<u64> {
    let mut buf = [0u8; 16];
                              dmx.src.peek_buf(&mut buf)?;
    validate!(&buf[4..8] == b"tfhd");
    let track_id = read_u32be(&buf[12..16])?;
    let mut track = None;
    for trk in dmx.tracks.iter_mut() {
        if trk.track_id == track_id {
            track = Some(trk);
            break;
        }
    }
    if let Some(track) = track {
        track.moof_off = dmx.moof_off;
        track.call_read_traf(dmx.src, size)?;
        Ok(size)
    } else {
        Ok(0)
    }
}

struct MOVDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    depth:          usize,
    mdat_pos:       u64,
    mdat_size:      u64,
    tracks:         Vec<Track>,
    cur_track:      usize,
    tb_den:         u32,
    duration:       u32,
    pal:            Option<Arc<[u8; 1024]>>,
    ver_m1:         bool,

    moof_off:       u64,

    print_chunks:   bool,

    demux_mode:     DemuxMode,

    force_seek:     bool,
}

impl<'a> DemuxCore<'a> for MOVDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let mut data_start = 0;
        match self.demux_mode {
            DemuxMode::Normal => self.read_root(strmgr)?,
            DemuxMode::MacBin => {
                let ver                     = self.src.read_byte()?;
                validate!(ver == 0);
                                              self.src.read_skip(64)?;
                let tag                     = self.src.read_tag()?;
                validate!(&tag == b"MooV");
                                              self.src.read_skip(14)?;
                let data_length             = self.src.read_u32be()?;
                validate!(data_length > 8);
                let rsrc_length             = self.src.read_u32be()?;
                validate!(rsrc_length > 0);
                                              self.src.read_skip(31)?;
                let ver                     = self.src.read_byte()?;
                validate!(ver == 0x81);
                let ver                     = self.src.read_byte()?;
                validate!(ver == 0x81);
                //xxx: maybe check header CRC

                let rsrc_start = 0x80 + ((data_length + 0x7F) & !0x7F);
                                              self.src.seek(SeekFrom::Start(rsrc_start.into()))?;
                let rsrc_off                = self.src.read_u32be()?;
                let rsrc_map_off            = self.src.read_u32be()?;
                let rsrc_size               = self.src.read_u32be()?;
                let _rsrc_map_size          = self.src.read_u32be()?;
                validate!(rsrc_off >= 0x10);
                validate!(rsrc_map_off >= rsrc_off + rsrc_size);
                                              self.src.seek(SeekFrom::Current(i64::from(rsrc_off - 16)))?;
                // I'm too lazy to parse resource map, so let's just iterate over resources for movie header
                let end_pos = u64::from(rsrc_start + rsrc_off + rsrc_size);
                let mut peek_buf = [0u8; 8];
                while self.src.tell() < end_pos {
                    let cur_size            = self.src.read_u32be()?;
                    validate!(self.src.tell() + u64::from(cur_size) <= end_pos);
                    if cur_size > 8 {
                        let rsize           = self.src.peek_u32be()?;
                        if rsize == cur_size {
                                              self.src.peek_buf(&mut peek_buf)?;
                            if &peek_buf[4..] == b"moov" {
                                              self.src.read_skip(8)?;
                                              self.src.peek_buf(&mut peek_buf)?;
                                if peek_buf == [0; 8] {
                                    println!("This looks like an alpha version of the format, try warhol demuxer instead");
                                    return Err(DemuxerError::InvalidData);
                                }
                                self.read_moov(strmgr, u64::from(rsize - 8))?;
                                self.mdat_pos = 8;
                                break;
                            }
                        }
                    }
                                              self.src.read_skip(cur_size as usize)?;
                }
                // if moov/mdat is not found, it may be in data part
                if self.tracks.is_empty() || self.mdat_pos == 0 {
                                              self.src.seek(SeekFrom::Start(0x80))?;
                    let end_pos = u64::from(data_length) + 0x80;
                    while self.src.tell() < end_pos && (self.mdat_pos == 0 || self.tracks.is_empty()) {
                        let size            = self.src.read_u32be()?;
                        let tag             = self.src.read_tag()?;
                        validate!(size >= 8);
                        match &tag {
                            b"moov" => {
                                              self.src.peek_buf(&mut peek_buf)?;
                                if peek_buf == [0; 8] {
                                    println!("This looks like an alpha version of the format, try warhol demuxer instead");
                                    return Err(DemuxerError::InvalidData);
                                }
                                self.read_moov(strmgr, u64::from(size - 8))?;
                            },
                            b"mdat" => {
                                self.mdat_pos = self.src.tell() - 0x80;
                                              self.src.read_skip(size as usize - 8)?;
                            },
                            _ => {
                                              self.src.read_skip(size as usize - 8)?;
                            },
                        }
                    }
                }
            },
            DemuxMode::ResFork => {
                let res_data_offset         = self.src.read_u32be()?;
                let res_map_offset          = self.src.read_u32be()?;
                let res_map_data_len        = self.src.read_u32be()?;
                let res_map_length          = self.src.read_u32be()?;
                validate!(res_data_offset == 0x100 && res_map_length > 16);
                                              self.src.seek(SeekFrom::Start(u64::from(res_map_offset)))?;
                let res_data_offset2        = self.src.read_u32be()?;
                let res_map_offset2         = self.src.read_u32be()?;
                let res_map_data_len2       = self.src.read_u32be()?;
                let res_map_length2         = self.src.read_u32be()?;
                validate!(res_data_offset == res_data_offset2
                    && res_map_offset == res_map_offset2
                    && res_map_data_len == res_map_data_len2
                    && res_map_length == res_map_length2);
                                              self.src.read_skip(res_map_length as usize - 16)?;
                data_start = self.src.tell();
                self.read_root(strmgr)?;
            },
        };
        validate!(self.mdat_pos > 0);
        validate!(!self.tracks.is_empty());
        for track in self.tracks.iter_mut() {
            let mut strm = None;
            std::mem::swap(&mut track.stream, &mut strm);
            if let Some(stream) = strm {
                let str_id = strmgr.add_stream(stream).unwrap();
                track.track_str_id = str_id;
            }
        }
        match self.demux_mode {
            DemuxMode::MacBin => {
                // patch data offsets
                for track in self.tracks.iter_mut() {
                    for offset in track.pkt_demux.chunk_offsets.iter_mut() {
                        *offset += 0x80;
                    }
                }
            },
            DemuxMode::ResFork => {
                // patch data offsets
                for track in self.tracks.iter_mut() {
                    for offset in track.pkt_demux.chunk_offsets.iter_mut() {
                        *offset += data_start;
                    }
                }
            },
            _ => {},
        }
        for track in self.tracks.iter() {
            track.pkt_demux.fill_seek_index(track.track_no, seek_index);
        }
        self.src.seek(SeekFrom::Start(self.mdat_pos))?;
        self.cur_track = 0;
        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        if self.tracks.is_empty() {
            return Err(DemuxerError::EOF);
        }
        let mut has_all_time = true;
        let mut min_ts = u64::MAX;
        for (trk_no, trk) in self.tracks.iter().enumerate() {
            if strmgr.is_ignored(trk_no) {
                continue;
            }
            if let Some(ts) = trk.pkt_demux.cur_ts {
                min_ts = min_ts.min(ts);
            } else {
                has_all_time = false;
                break;
            }
        }
        if has_all_time {
            for (trk_no, track) in self.tracks.iter_mut().enumerate() {
                if let Some(ts) = track.pkt_demux.cur_ts {
                    if ts == min_ts {
                        let first = track.pkt_demux.is_first();
                        if let Some((cts, offset, size, is_kf)) = track.pkt_demux.get_next_chunk() {
                            self.cur_track = trk_no + 1;
                            return process_packet(self.src, strmgr, track, cts, offset, size, first, is_kf);
                        }
                    }
                }
            }
        }

        for _ in 0..self.tracks.len() {
            if self.cur_track >= self.tracks.len() {
                self.cur_track = 0;
            }
            let track = &mut self.tracks[self.cur_track];
            self.cur_track += 1;
            if strmgr.is_ignored(self.cur_track - 1) {
                continue;
            }
            let first = track.pkt_demux.is_first();
            if let Some((cts, offset, size, is_kf)) = track.pkt_demux.get_next_chunk() {
                return process_packet(self.src, strmgr, track, cts, offset, size, first, is_kf);
            }
        }
        Err(DemuxerError::EOF)
    }

    fn seek(&mut self, time: NATimePoint, seek_index: &SeekIndex) -> DemuxerResult<()> {
        let ret = seek_index.find_pos(time);
        if ret.is_none() {
            if let NATimePoint::Milliseconds(_) = time {
                let mut aonly = true;
                for track in self.tracks.iter() {
                    if track.stream_type != StreamType::Audio {
                        aonly = false;
                        break;
                    }
                }
                if aonly {
                    for track in self.tracks.iter_mut() {
                        track.pkt_demux.seek(0, time, false)?;
                    }
                    return Ok(());
                }
            }
            if self.force_seek {
                let mut ret = Ok(());
                for track in self.tracks.iter_mut() {
                    if let Err(err) = track.pkt_demux.seek(0, time, true) {
                        ret = Err(err);
                        break;
                    }
                }
                if ret.is_err() {
                    for track in self.tracks.iter_mut() {
                        track.pkt_demux.reset();
                    }
                }
                return ret;
            }
            return Err(DemuxerError::SeekError);
        }
        let seek_info = ret.unwrap();
        let tbn = self.tracks[seek_info.str_id as usize].tb_num;
        let tbd = self.tracks[seek_info.str_id as usize].tb_den;
        let mut vpts = None;
        let mut apts = None;
        for track in self.tracks.iter_mut() {
            let cur_pts = if track.track_id == seek_info.str_id {
                    seek_info.pts
                } else {
                    seek_info.pts * u64::from(tbn) * u64::from(track.tb_den) / (u64::from(tbd) * u64::from(track.tb_num))
                };
            let actual_time = track.pkt_demux.seek(cur_pts, time, false)?;
            match track.stream_type {
                StreamType::Video => vpts = Some(actual_time),
                StreamType::Audio => apts = Some(actual_time),
                _ => {},
            };
        }
        /* For audio+video stream case when the post-seek actual times differ
           by more than half a second try to seek audio to a closer position
           to video.
        */
        if let (Some(vtime), Some(atime)) = (vpts, apts) {
            let acount = self.tracks.iter().filter(|trk| trk.stream_type == StreamType::Audio).count();
            let vcount = self.tracks.iter().filter(|trk| trk.stream_type == StreamType::Video).count();
            if (vcount == 1 && acount == 1) && (vtime.max(atime) - vtime.min(atime) > 500) && (atime != 0) {
                for track in self.tracks.iter_mut() {
                    if track.stream_type == StreamType::Audio {
                        let new_pts = NATimeInfo::rescale_ts(vtime, 1, 1000, track.tb_num, track.tb_den);
                        track.pkt_demux.seek(new_pts, NATimePoint::Milliseconds(vtime), false)?;
                    }
                }
            }
        }

        Ok(())
    }
    fn get_duration(&self) -> u64 {
        if self.tb_den != 0 {
            u64::from(self.duration) * 1000 / u64::from(self.tb_den)
        } else {
            0
        }
    }
}

const PRINT_CHUNKS: &str = "print_chunks";

const DEMUXER_OPTIONS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name:           PRINT_CHUNKS,
        description:    "Print parsed file structure",
        opt_type:       NAOptionDefinitionType::Bool },
    NAOptionDefinition {
        name:           FORCE_SEEK_OPTION,
        description:    FORCE_SEEK_OPTION_DESC,
        opt_type:       NAOptionDefinitionType::Bool },
];

impl<'a> NAOptionHandler for MOVDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { DEMUXER_OPTIONS }
    #[allow(clippy::single_match)]
    fn set_options(&mut self, options: &[NAOption]) {
        for option in options.iter() {
            for opt_def in DEMUXER_OPTIONS.iter() {
                if opt_def.check(option).is_ok() {
                    match (option.name, &option.value) {
                        (PRINT_CHUNKS, NAValue::Bool(val)) => {
                            self.print_chunks = *val;
                        },
                        (FORCE_SEEK_OPTION, NAValue::Bool(val)) => {
                            self.force_seek = *val;
                        },
                        _ => {},
                    }
                }
            }
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match name {
            PRINT_CHUNKS    => Some(NAValue::Bool(self.print_chunks)),
            FORCE_SEEK_OPTION => Some(NAValue::Bool(self.force_seek)),
            _ => None,
        }
    }
}

impl<'a> MOVDemuxer<'a> {
    fn new(io: &'a mut dyn ByteIO) -> Self {
        Self::new_common(io, DemuxMode::Normal)
    }
    fn new_macbinary(io: &'a mut dyn ByteIO) -> Self {
        Self::new_common(io, DemuxMode::MacBin)
    }
    fn new_resfork(io: &'a mut dyn ByteIO) -> Self {
        Self::new_common(io, DemuxMode::ResFork)
    }
    fn new_common(io: &'a mut dyn ByteIO, demux_mode: DemuxMode,) -> Self {
        MOVDemuxer {
            src:            io,
            depth:          0,
            mdat_pos:       0,
            mdat_size:      0,
            tracks:         Vec::with_capacity(2),
            cur_track:      0,
            tb_den:         0,
            duration:       0,
            pal:            None,
            ver_m1:         false,

            moof_off:       0,

            print_chunks:   false,

            demux_mode,
            force_seek:     false,
        }
    }
    fn read_root(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<()> {
        self.depth = 0;
        while self.src.left() != 0 {
            let ret = read_chunk_header(self.src);
            if ret.is_err() { break; }
            let (ctype, size) = ret.unwrap();
            if self.print_chunks {
                print_cname(ctype, size, self.src.tell(), 0);
            }
            if IGNORED_CHUNKS.contains(&ctype) {
                self.src.skip64(size)?;
                continue;
            }
            let handler = ROOT_CHUNK_HANDLERS.iter().find(|x| x.ctype == ctype);
            let read_size;
            if let Some(handler) = handler {
                read_size = (handler.parse)(self, strmgr, size)?;
            } else {
                println!("skipping unknown chunk {:08X} size {}", ctype, size);
                read_size = 0;
            }
            validate!(read_size <= size);
            self.src.skip64(size - read_size)?;
        }
//todo check if all needed chunks are found
        Ok(())
    }
    read_chunk_list!(root; "moov", read_moov, MOOV_CHUNK_HANDLERS);
    read_chunk_list!(root; "moof", read_moof, MOOF_CHUNK_HANDLERS);
}

pub struct MOVDemuxerCreator { }

impl DemuxerCreator for MOVDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(MOVDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "mov" }
}

pub struct MacBinaryMOVDemuxerCreator { }

impl DemuxerCreator for MacBinaryMOVDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(MOVDemuxer::new_macbinary(br))
    }
    fn get_name(&self) -> &'static str { "mov-macbin" }
}

pub struct MacResForkMOVDemuxerCreator { }

impl DemuxerCreator for MacResForkMOVDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(MOVDemuxer::new_resfork(br))
    }
    fn get_name(&self) -> &'static str { "mov-resfork" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_mov_demux() {
        // sample: https://samples.mplayerhq.hu/V-codecs/IV32/cubes.mov
        let mut file = File::open("assets/Indeo/cubes.mov").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = MOVDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }

    #[test]
    fn test_dash_demux() {
        // sample: a stream downloaded with youtube-dl
        let mut file = File::open("assets/ITU/dash.m4a").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = MOVDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }

    #[test]
    fn test_macbinary_demux() {
        // sample from King's Quest VI Macintosh edition
        let mut file = File::open("assets/QT/Halfdome.bin").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = MOVDemuxer::new_macbinary(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }


    #[test]
    fn test_beta_qt() {
        // sample from Apple Reference & Presentation Library 8
        let mut file = File::open("assets/QT/Smallwhale2").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = MOVDemuxer::new_macbinary(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }

    #[test]
    fn test_resfork_demux() {
        // sample from The Wonders of Electricity: An Adventure in Safety
        let mut file = File::open("assets/QT/car.mov").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = MOVDemuxer::new_resfork(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }
}
