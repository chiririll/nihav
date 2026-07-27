use nihav_core::demuxers::*;
use nihav_core::demuxers::DemuxerError::*;

const INVALID_STREAM: usize = usize::MAX;

struct PalInfo {
    pal:        Arc<[u8; 1024]>,
    changed:    bool,
}

#[derive(Default)]
struct AVIStream {
    strm_no:        u8,
    got_strf:       bool,
    strm_type:      Option<StreamType>,
    pal:            Option<PalInfo>,
    cur_frame:      u64,
    handler:        [u8; 4],
    strm_duration:  u32,
    tb_num:         u32,
    tb_den:         u32,
    seek_idx:       Vec<(u32, u32, u32)>,
    timestamps:     Vec<u32>,

    print_chunks:   bool,
    depth:          u8,
}

impl AVIStream {
    fn parse_strh(&mut self, src: &mut dyn ByteIO, _strmgr: &mut StreamManager, _seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        if size < 0x30 { return Err(InvalidData); }
        validate!(!self.got_strf);
        let tag                 = src.read_tag()?; //stream type
        self.handler            = src.read_tag()?; //handler(fourcc)
                                  src.read_u32be()?; //flags
                                  src.read_skip(2)?; //priority
                                  src.read_skip(2)?; //language
                                  src.read_skip(4)?; //initial frames
        self.tb_num             = src.read_u32be()?; //scale
        self.tb_den             = src.read_u32be()?; //rate
        if self.tb_den == 0xFFFFFFFF {
            self.tb_den = 1000;
        }
                                  src.read_skip(4)?; //start
        self.strm_duration      = src.read_u32be()?;
                                  src.read_skip(4)?; //buf size
                                  src.read_skip(4)?; //quality
                                  src.read_skip(4)?; //sample size
        /*let _a                  = src.read_u16le()?;
        let _b                  = src.read_u16le()?;
        let _c                  = src.read_u16le()?;
        let _d                  = src.read_u16le()?;*/

                                  src.read_skip(size - 0x30)?;

        // padding streams should be ignored
        if &tag == b"pads" {
            self.got_strf = true;
            return Ok(());
        }

        self.strm_type = match &tag {
                b"vids" => Some(StreamType::Video),
                b"auds" => Some(StreamType::Audio),
                _       => Some(StreamType::Data),
            };

        Ok(())
    }

    fn parse_strf(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(self.strm_type.is_some() && !self.got_strf);
        match self.strm_type {
            Some(StreamType::Video) => self.parse_strf_vids(src, strmgr, size)?,
            Some(StreamType::Audio) => self.parse_strf_auds(src, strmgr, size)?,
            Some(_)                 => self.parse_strf_xxxx(src, strmgr, size)?,
            _ => unreachable!(),
        }
        self.got_strf = true;
        Ok(())
    }
    fn parse_strf_vids(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, size: usize) -> DemuxerResult<()> {
        if size < 40 { return Err(InvalidData); }
        let strf_end = src.tell() + (size as u64);

        let bi_size         = src.read_u32be()? as usize;
        validate!(bi_size >= 40 && bi_size <= size);
        let width           = src.read_u32be()? as usize;
        let height          = src.read_u32be()? as i32;
        validate!((1..=8192).contains(&width) && (1..=8192).contains(&height));
        let flipped = height < 0;
        let height = height.unsigned_abs() as usize;
        let planes          = src.read_u16be()?;
        let bitcount        = src.read_u16be()?;
        validate!(planes > 0 && planes <= bitcount);
        let compression     = src.read_tag()?;
        let _image_size     = src.read_u32be()?;
        let _x_dpi          = src.read_u32be()?;
        let _y_dpi          = src.read_u32be()?;
        let colors          = src.read_u32be()? as usize;
        let _clr_important  = src.read_u32be()? as usize;
        if bitcount <= 8 {
            validate!(colors <= (1 << bitcount));
        }
        let format = if bitcount > 8 { RGB24_FORMAT } else { PAL8_FORMAT };
        let mut vhdr = NAVideoInfo::new(width, height, flipped, format);
        vhdr.bits = (planes as u8) * (bitcount as u8);

        let cname = match &compression {
                b"RLE8" => "msrle",
                _ => "unknown",
            };
        let vci = NACodecTypeInfo::Video(vhdr);
        let mut edata: Option<Vec<u8>> = None;
        if src.tell() < strf_end {
            let edata_size = (strf_end - src.tell()) as usize;
            edata = Self::read_extradata(src, edata_size)?;
            if colors > 0 {
                if let Some(ref buf) = edata {
                    let mut pal = [0u8; 1024];
                    for (dpal, spal) in pal.chunks_mut(4).take(colors).zip(buf.chunks(4)) {
                        dpal[0] = spal[2];
                        dpal[1] = spal[1];
                        dpal[2] = spal[0];
                        dpal[3] = 0;
                    }
                    self.pal = Some(PalInfo { pal: Arc::new(pal), changed: true });
                }
            }
        } else {
            let pal = std::array::from_fn(|i| if (i & 3) == 3 { 0 } else { (i / 4) as u8 });
            self.pal = Some(PalInfo { pal: Arc::new(pal), changed: true });
        }
        validate!(src.tell() <= strf_end);
        src.seek(SeekFrom::Start(strf_end))?;
        let vinfo = NACodecInfo::new(cname, vci, edata);
        let res = strmgr.add_stream(NAStream::new(StreamType::Video, u32::from(self.strm_no), vinfo, self.tb_num, self.tb_den, u64::from(self.strm_duration)));
        if res.is_none() { return Err(MemoryError); }
        Ok(())
    }
    fn parse_rvdi(&mut self, src: &mut dyn ByteIO, _strmgr: &mut StreamManager, seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        let end = src.tell() + (size as u64);
        validate!(size >= 64);
        src.read_skip(4)?;
        let _width = src.read_u32be()? as usize;
        let _height = src.read_u32be()? as usize;
        let _bpp = src.read_u32be()?;
        let nframes = src.read_u32be()? as usize;
        validate!((1..=1048576).contains(&nframes));
        src.read_u32be()?;
        src.read_u32be()?;
        let ngroups = src.read_u32be()? as usize;
        validate!(ngroups > 0 && ngroups <= nframes);
        src.read_u32be()?;

        let mut last_ts = 0u32;
        for _ in 0..ngroups {
            let grp_frames = src.read_u32be()? as usize;
            let base_off = src.read_u32be()?;
            let ctab_off = src.read_u32be()?;
            let _num_smth = src.read_u32be()? as usize;
            validate!(grp_frames <= nframes);

            let ts = src.read_u32be()?;
            if last_ts > 0 {
                validate!(ts > last_ts);
            }
            last_ts = ts;
            self.timestamps.push(ts);
            let pts = u64::from(ts);
            let time = NATimeInfo::rescale_ts(pts, self.tb_num, self.tb_den, 1, 1000);
            seek_index.add_entry(u32::from(self.strm_no), SeekEntry { time, pts, pos: u64::from(base_off) });
            self.seek_idx.push((ts, base_off, ctab_off));

            src.read_skip(grp_frames * 4)?; // other timestamps
            src.read_skip((grp_frames + 1) * 12)?;
            validate!(src.tell() <= end);
        }

        Ok(())
    }
    fn parse_strf_auds(&mut self, _src: &mut dyn ByteIO, _strmgr: &mut StreamManager, _size: usize) -> DemuxerResult<()> {
        Ok(())
    }
    fn parse_radi(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(size >= 32);
        let arate = src.read_u32be()?;
        let channels = src.read_u32be()?;
        let compression = src.read_u32be()?;
        src.read_u32be()?;
        let nframes = src.read_u32be()? as usize;
        let block_size = src.read_u32be()? as usize;
        validate!((1..=1048576).contains(&nframes));
        validate!((1..=1048576).contains(&block_size));
        validate!(size == nframes * 8 + 0x18);

        let start = src.tell() + (size as u64);
        self.seek_idx.reserve(nframes);
        let mut ts = 0;
        for _i in 0..nframes {
            let size     = src.read_u32be()?;
            let offset   = src.read_u32be()?;
            validate!(u64::from(offset) > start);
            ts += size;
            self.seek_idx.push((ts, offset, 0));
        }

        validate!((4000..=48000).contains(&arate));
        validate!(channels == 1 || channels == 2);
        validate!(compression == 0);

        let ahdr = NAAudioInfo::new(arate, channels as u8, SND_S16_FORMAT, block_size);
        let ainfo = NACodecInfo::new("ulaw", NACodecTypeInfo::Audio(ahdr), None);
        let res = strmgr.add_stream(NAStream::new(StreamType::Audio, u32::from(self.strm_no), ainfo, self.tb_num, self.tb_den, u64::from(self.strm_duration)));
        if res.is_none() { return Err(MemoryError); }
        Ok(())
    }
    fn parse_strf_xxxx(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, size: usize) -> DemuxerResult<()> {
        let edata = Self::read_extradata(src, size)?;
        let info = NACodecInfo::new("unknown", NACodecTypeInfo::None, edata);
        let res = strmgr.add_stream(NAStream::new(StreamType::Data, u32::from(self.strm_no), info, self.tb_num, self.tb_den, u64::from(self.strm_duration)));
        if res.is_none() { return Err(MemoryError); }
        Ok(())
    }
    fn read_extradata(src: &mut dyn ByteIO, size: usize) -> DemuxerResult<Option<Vec<u8>>> {
        if size == 0 { return Ok(None); }
        let mut edvec: Vec<u8> = vec![0; size];
        src.read_buf(&mut edvec)?;
        Ok(Some(edvec))
    }
}

#[derive(Default)]
struct AVIState {
    size:           usize,
    movi_size:      usize,
    movi_pos:       u64,
    movi_orig:      usize,

    streams:        Vec<AVIStream>,
    nom_streams:    usize,
    vstream:        usize,
    astream:        usize,

    print_chunks:   bool,
    depth:          u8,
}

impl AVIState {
    fn new() -> Self {
        Self { vstream: INVALID_STREAM, astream: INVALID_STREAM, ..Default::default() }
    }

    fn parse_hdrl(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(size > 20);
        let end = src.tell() + (size as u64) - 4;
        parse_chunks(self, src, strmgr, seek_index, HDRL_CHUNKS, end)
    }
    fn parse_movi(&mut self, src: &mut dyn ByteIO, _strmgr: &mut StreamManager, _seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(size > 10);
        self.movi_size = size - 4;
        self.movi_orig = self.movi_size;
        self.movi_pos  = src.tell();
        Ok(())
    }

    fn parse_avih(&mut self, src: &mut dyn ByteIO, _strmgr: &mut StreamManager, _seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(size >= 0x38);
        let _timebase = src.read_u32le()?; //microsec per frame
        src.read_skip(4)?; //max frame size
        src.read_skip(4)?; //padding
        src.read_u32be()?; //flags
        let _frames = src.read_u32be()?; //frames
        src.read_skip(4)?; //initial frames
        let streams = src.read_u32be()?; //streams
        if streams > 100 { return Err(InvalidData); }
        self.nom_streams = streams as usize;

        src.read_skip(4)?; //buf size
        let _width = src.read_u32be()?; //width
        let _height = src.read_u32be()? as i32; //height
        src.read_skip(16)?; //reserved

        Ok(())
    }
    fn parse_strl(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(size >= 20);
        let stream_no = self.streams.len();
        validate!(stream_no < 100);
        let mut stream = AVIStream { strm_no: stream_no as u8, ..Default::default() };
        let end = src.tell() + size as u64 - 4;
        stream.print_chunks = self.print_chunks;
        stream.depth = self.depth;
        parse_chunks(&mut stream, src, strmgr, seek_index, STRL_CHUNKS, end)?;
        validate!(stream.got_strf);
        self.streams.push(stream);
        Ok(())
    }
}

struct AVIDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    state:          AVIState,
    print_chunks:   bool,
}

#[derive(Debug,Clone,Copy,PartialEq)]
enum RIFFTag {
    Chunk([u8; 4]),
    List([u8; 4]),
}

struct ChunkHandler<T> {
    tag:   RIFFTag,
    parse: fn(obj: &mut T, src: &mut dyn ByteIO, strmgr: &mut StreamManager, seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()>,
}

trait DebugChunks {
    fn inc_depth(&mut self);
    fn dec_depth(&mut self);
    fn get_depth(&self) -> u8;
    fn is_debug(&self) -> bool;
}

impl DebugChunks for AVIState {
    fn inc_depth(&mut self) { self.depth += 1; }
    fn dec_depth(&mut self) { self.depth -= 1; }
    fn get_depth(&self) -> u8 { self.depth }
    fn is_debug(&self) -> bool { self.print_chunks }
}

impl DebugChunks for AVIStream {
    fn inc_depth(&mut self) { self.depth += 1; }
    fn dec_depth(&mut self) { self.depth -= 1; }
    fn get_depth(&self) -> u8 { self.depth }
    fn is_debug(&self) -> bool { self.print_chunks }
}

fn parse_chunks<T: DebugChunks>(obj: &mut T, src: &mut dyn ByteIO, strmgr: &mut StreamManager, seek_index: &mut SeekIndex, handlers: &[ChunkHandler<T>], parse_end: u64) -> DemuxerResult<()> {
    obj.inc_depth();
    while src.tell() < parse_end {
        let tag                     = src.read_tag()?;
        let size                    = src.read_u32be()?;
        if obj.is_debug() {
            for _ in 0..obj.get_depth() {
                print!("    ");
            }
            print!("{}{}{}{}", tag[0] as char, tag[1] as char, tag[2] as char, tag[3] as char);
            if &tag == b"LIST" {
                if let Ok(tag2) = src.peek_tag() {
                    print!(" type {}{}{}{}", tag2[0] as char, tag2[1] as char, tag2[2] as char, tag2[3] as char);
                } else {
                    print!(" type ????");
                }
            }
            println!(" size {size:X} @ {:X}", src.tell() - 8);
        }
        let chunk_end = src.tell() + u64::from(size);
        validate!(chunk_end <= parse_end);
        if &tag == b"JUNK" {
                                      src.seek(SeekFrom::Start(chunk_end + (chunk_end & 1)))?;
            continue;
        }
        let ref_tag = if &tag == b"LIST" {
                validate!(size >= 4);
                RIFFTag::List(src.read_tag()?)
            } else { RIFFTag::Chunk(tag) };

        if let Some(handler) = handlers.iter().find(|hdl| hdl.tag == ref_tag) {
            (handler.parse)(obj, &mut *src, strmgr, seek_index, size as usize)?;
            validate!(src.tell() <= chunk_end);
        }
                                      src.seek(SeekFrom::Start(chunk_end))?;
        if (chunk_end & 1) != 0 {
                                      src.read_byte()?;
        }
    }
    obj.dec_depth();
    Ok(())
}

const AVI_ROOT_CHUNKS: &[ChunkHandler<AVIState>] = &[
    ChunkHandler{ tag: RIFFTag::List( *b"hdrl"), parse: AVIState::parse_hdrl },
    ChunkHandler{ tag: RIFFTag::List( *b"movi"), parse: AVIState::parse_movi },
];

const HDRL_CHUNKS: &[ChunkHandler<AVIState>] = &[
    ChunkHandler{ tag: RIFFTag::Chunk(*b"avih"), parse: AVIState::parse_avih },
    ChunkHandler{ tag: RIFFTag::List( *b"strl"), parse: AVIState::parse_strl },
];

const STRL_CHUNKS: &[ChunkHandler<AVIStream>] = &[
    ChunkHandler{ tag: RIFFTag::Chunk(*b"strh"), parse: AVIStream::parse_strh },
    ChunkHandler{ tag: RIFFTag::Chunk(*b"strf"), parse: AVIStream::parse_strf },
    ChunkHandler{ tag: RIFFTag::Chunk(*b"rvdi"), parse: AVIStream::parse_rvdi },
    ChunkHandler{ tag: RIFFTag::Chunk(*b"radi"), parse: AVIStream::parse_radi },
];

impl<'a> DemuxCore<'a> for AVIDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let rifx_tag = self.src.read_tag()?;
        let size     = self.src.read_u32be()? as usize;
        if self.print_chunks {
            println!("{}{}{}{} size {size:X} @ {:X}", rifx_tag[0] as char, rifx_tag[1] as char, rifx_tag[2] as char, rifx_tag[3] as char, self.src.tell() - 8);
        }
        let avi_tag  = self.src.read_tag()?;
        if !AVI_HEADER_TAGS.contains(&(&rifx_tag, &avi_tag)) {
            return Err(InvalidData);
        }
        self.state.print_chunks = self.print_chunks;
        self.state.size = size;

        match parse_chunks(&mut self.state, &mut *self.src, strmgr, seek_index, AVI_ROOT_CHUNKS, size as u64 + 8) {
            Ok(()) => {},
            Err(DemuxerError::EOF) | Err(DemuxerError::IOError) => {},
            Err(err) => return Err(err),
        }

        for (stream_no, stream) in strmgr.iter().enumerate() {
            match stream.get_media_type() {
                StreamType::Video => {
                    validate!(self.state.vstream == INVALID_STREAM);
                    self.state.vstream = stream_no;
                },
                StreamType::Audio => {
                    validate!(self.state.astream == INVALID_STREAM);
                    self.state.astream = stream_no;
                },
                _ => {},
            }
        }

        validate!(self.state.movi_pos != 0);
        validate!(!self.state.streams.is_empty());

        self.src.seek(SeekFrom::Start(self.state.movi_pos))?;

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        if self.state.movi_size == 0 {
            return Err(EOF);
        }
        let mut vdata_end = u64::MAX;
        let mut pkt = None;
        loop {
            if self.src.tell() >= vdata_end {
                vdata_end = u64::MAX;
                if let Some(ppkt) = pkt {
                    return Ok(ppkt);
                }
            }
            let tag = self.src.read_tag().map_err(|_| DemuxerError::EOF)?;
            let size = self.src.read_u32be()? as usize;
            if self.print_chunks {
                println!("   frame tag {}{}{}{} size {size:X} @ {:X}", tag[0] as char, tag[1] as char, tag[2] as char, tag[3] as char, self.src.tell() - 8);
            }
            self.state.movi_size = self.state.movi_size.saturating_sub(8);
            if size > self.state.movi_size {
                self.state.movi_size = 0;
                if let Some(ppkt) = pkt {
                    return Ok(ppkt);
                }
                return Err(InvalidData);
            }
            match &tag {
                b"JUNK" => {
                    self.state.movi_size -= size;
                    self.src.read_skip(size)?;
                    if self.state.movi_size == 0 {
                        return Err(EOF);
                    }
                },
                b"LIST" => {
                    self.state.movi_size -= 4;
                    self.src.read_skip(4)?;
                    if self.state.movi_size == 0 {
                        return Err(EOF);
                    }
                },
                b"rvdf" => { // video data container
                    vdata_end = self.src.tell() + (size as u64);
                },
                b"snau" => {
                    if let Some(stream) = strmgr.get_stream(self.state.astream) {
                        let cur_stream = &mut self.state.streams[self.state.astream];
                        let ts = stream.make_ts(Some(cur_stream.cur_frame), None, None);
                        let pkt = self.src.read_packet(stream, ts, true, size)?;
                        cur_stream.cur_frame += size as u64;
                        self.state.movi_size -= size;
                        return Ok(pkt);
                    }
                    self.state.movi_size -= size;
                    self.src.read_skip(size)?;
                    if self.state.movi_size == 0 {
                        return Err(EOF);
                    }
                },
                b"rxct" => {
                    validate!((12..=12 * 256).contains(&size) && (size % 12) == 0);
                    if self.state.vstream < self.state.streams.len() {
                        if let Some(ref mut pe) = self.state.streams[self.state.vstream].pal {
                            let mut newpal = *pe.pal;
                            for _ in 0..size / 12 {
                                let idx = self.src.read_u32be()? as usize;
                                validate!(idx < 256);
                                newpal[idx * 4]     = (self.src.read_u16be()? >> 8) as u8;
                                newpal[idx * 4 + 1] = (self.src.read_u16be()? >> 8) as u8;
                                newpal[idx * 4 + 2] = (self.src.read_u16be()? >> 8) as u8;
                                newpal[idx * 4 + 3] = 0;
                                                       self.src.read_byte()?; // flags
                                                       self.src.read_byte()?; // padding
                            }
                            pe.pal = Arc::new(newpal);
                            pe.changed = true;

                            if let Some(ref mut ppkt) = pkt {
                                ppkt.add_side_data(NASideData::Palette(pe.changed, pe.pal.clone()));
                                pe.changed = false;
                            }
                        } else {
                            self.src.read_skip(size)?;
                        }
                    } else {
                        self.src.read_skip(size)?;
                    }
                    if let Some(ppkt) = pkt {
                        return Ok(ppkt);
                    }
                    self.state.movi_size -= size;
                    if self.state.movi_size == 0 {
                        return Err(EOF);
                    }
                },
                _ if tag[0].is_ascii_digit() && tag[1].is_ascii_digit() && &tag[2..] == b"dc" => {
                    let stream_no = usize::from(tag[0] - b'0') * 10 + usize::from(tag[1] - b'0');
                    validate!(stream_no == self.state.vstream);
                    let stream = strmgr.get_stream(stream_no);
                    if stream.is_none() || stream_no >= self.state.streams.len() {
                        self.src.read_skip(size)?;
                        self.state.movi_size -= size;
                        continue;
                    }
                    let cur_stream = &mut self.state.streams[stream_no];
                    let stream = stream.unwrap();
                    if size == 0 {
                        if self.state.movi_size == 0 {
                            return Err(EOF);
                        }
                        continue;
                    }
                    let is_keyframe = cur_stream.cur_frame == 0;

                    let pts = if (cur_stream.cur_frame as usize) < cur_stream.timestamps.len() {
                            Some(u64::from(cur_stream.timestamps[cur_stream.cur_frame as usize]))
                        } else { None };

                    let ts = stream.make_ts(pts, None, None);
                    let mut lpkt = self.src.read_packet(stream, ts, is_keyframe, size)?;
                    if let Some(ref mut pe) = cur_stream.pal {
                        lpkt.add_side_data(NASideData::Palette(pe.changed, pe.pal.clone()));
                        pe.changed = false;
                    }
                    cur_stream.cur_frame += 1;
                    self.state.movi_size -= size;

                    pkt = Some(lpkt);
                },
                _ => {
                    return Err(InvalidData);
                },
            }
        }
    }

    fn seek(&mut self, time: NATimePoint, seek_index: &SeekIndex) -> DemuxerResult<()> {
        let seek_info = seek_index.find_pos(time).ok_or(DemuxerError::SeekError)?;

        let pos = seek_info.pos - 4;
        if pos < self.state.movi_pos { return Err(DemuxerError::SeekError); }
        let skip_size = (pos - self.state.movi_pos) as usize;
        if skip_size > self.state.movi_orig { return Err(DemuxerError::SeekError); }
        self.state.movi_size = self.state.movi_orig - skip_size;

        let mut vstream = &mut self.state.streams[self.state.vstream];
        let frameno = vstream.seek_idx.iter().position(|&(_, off, _)| u64::from(off) == seek_info.pos).unwrap();
        vstream.cur_frame = frameno as u64;

        if self.state.astream != INVALID_STREAM {
            let mut astream = &mut self.state.streams[self.state.astream];
            astream.cur_frame = u64::MAX;
            for &(ts, off, _) in astream.seek_idx.iter() {
                astream.cur_frame = u64::from(ts);
                if u64::from(off) > pos {
                    break;
                }
            }
        }
        self.src.seek(SeekFrom::Start(pos - 8))?; // to 'rvdf' chunk

        Ok(())
    }
    fn get_duration(&self) -> u64 { 0 }
}

const PRINT_CHUNKS: &str = "print_chunks";

const DEMUXER_OPTIONS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name:           PRINT_CHUNKS,
        description:    "Print parsed file structure",
        opt_type:       NAOptionDefinitionType::Bool },
];

#[allow(clippy::single_match)]
impl<'a> NAOptionHandler for AVIDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { DEMUXER_OPTIONS }
    fn set_options(&mut self, options: &[NAOption]) {
        for option in options.iter() {
            for opt_def in DEMUXER_OPTIONS.iter() {
                if opt_def.check(option).is_ok() {
                    match (option.name, &option.value) {
                        (PRINT_CHUNKS, NAValue::Bool(val)) => {
                            self.print_chunks = *val;
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
            _ => None,
        }
    }
}

impl<'a> AVIDemuxer<'a> {
    fn new(io: &'a mut dyn ByteIO) -> Self {
        AVIDemuxer {
            src: io,
            state: AVIState::new(),
            print_chunks: false,
        }
    }
}

const AVI_HEADER_TAGS: &[(&[u8; 4], &[u8; 4])] = &[
    ( b"RIFX", b"AVI " ),
];

pub struct AviXDemuxerCreator { }

impl DemuxerCreator for AviXDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(AVIDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "avi-x" }
}

#[cfg(test)]
mod test {
    use super::*;
    use nihav_codec_support::test::dmx_test::*;

    #[test]
    fn test_avix_demux() {
        //sample from SunSoft Solaris DiskSuite
        test_file_demux("assets/Misc/SOLSTICE.AVX", &AviXDemuxerCreator{}, 0, DemuxerTestMode::Demux);
    }
}
