use nihav_core::demuxers::*;
use nihav_core::demuxers::DemuxerError::*;
use nihav_codec_support::codecs::msstructs::*;
use std::str::FromStr;

struct PalInfo {
    pal:        Arc<[u8; 1024]>,
    changed:    bool,
}

#[derive(Default)]
struct AVIState {
    key_offs:       Vec<u64>,
    size:           usize,
    movi_size:      usize,
    movi_pos:       u64,
    movi_orig:      usize,

    got_vstream:    bool,
    pal:            Option<PalInfo>,
    tb_num:         u32,
    nframes:        u32,
    cur_frame:      u64,
}

impl AVIState {
    fn new() -> Self {
        Self::default()
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
    fn parse_idx1(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        if seek_index.skip_index { return Ok(()); }
println!("index @ {:X}", src.tell());
        let _res = parse_idx1(src, strmgr, seek_index, size, self.movi_pos, &mut self.key_offs);
        Ok(())
    }

    fn parse_hdra(&mut self, src: &mut dyn ByteIO, _strmgr: &mut StreamManager, _seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(size >= 0x20);
        self.tb_num     = src.read_u32le()?;
        validate!(self.tb_num > 0);
        let _unk1       = src.read_u32le()?;
        let _unk2       = src.read_u32le()?;
        let _unk3       = src.read_u32le()?;
        self.nframes    = src.read_u32le()?;

        Ok(())
    }
    fn parse_dibh(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex, size: usize) -> DemuxerResult<()> {
        validate!(!self.got_vstream);
        if size < 40 { return Err(InvalidData); }
        validate!(self.tb_num > 0);
        let bm = MSBitmapInfo::read(src)?;
        validate!(bm.validate().is_ok());
        let mut vhdr = bm.get_video_info();
        let cname = if find_raw_rgb_fmt(&bm, &mut vhdr) {
                "rawvideo-ms"
            } else if bm.compression == [1, 0, 0, 0] {
                "msrle"
            } else {
                "unknown"
            };
        let vci = NACodecTypeInfo::Video(vhdr);
        if bm.colors > 0 {
            let mut pal = [0u8; 1024];
            let mut clr = [0; 4];
            for dpal in pal.chunks_mut(4).take(bm.colors) {
                src.read_buf(&mut clr)?;
                dpal[0] = clr[2];
                dpal[1] = clr[1];
                dpal[2] = clr[0];
                dpal[3] = 0;
            }
            self.pal = Some(PalInfo { pal: Arc::new(pal), changed: true });
        }
        let vinfo = NACodecInfo::new(cname, vci, None);
        let res = strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, self.tb_num, 1000000, u64::from(self.nframes)));
        if res.is_none() { return Err(MemoryError); }

        self.got_vstream = true;
        Ok(())
    }
}

struct AVIDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    state:          AVIState,
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

fn parse_chunks<T>(obj: &mut T, src: &mut dyn ByteIO, strmgr: &mut StreamManager, seek_index: &mut SeekIndex, handlers: &[ChunkHandler<T>], parse_end: u64) -> DemuxerResult<()> {
    while src.tell() < parse_end {
        let tag                     = src.read_tag()?;
        let size                    = src.read_u32le()?;
        let chunk_end = src.tell() + u64::from(size) + u64::from(size & 1);
        validate!(chunk_end <= parse_end);
        if &tag == b"JUNK" {
                                      src.seek(SeekFrom::Start(chunk_end))?;
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
    }
    Ok(())
}

const AVI_ROOT_CHUNKS: &[ChunkHandler<AVIState>] = &[
    ChunkHandler{ tag: RIFFTag::List( *b"hdrl"), parse: AVIState::parse_hdrl },
    ChunkHandler{ tag: RIFFTag::List( *b"movi"), parse: AVIState::parse_movi },
    ChunkHandler{ tag: RIFFTag::Chunk(*b"idx1"), parse: AVIState::parse_idx1 },
];

const HDRL_CHUNKS: &[ChunkHandler<AVIState>] = &[
    ChunkHandler{ tag: RIFFTag::Chunk(*b"hdra"), parse: AVIState::parse_hdra },
    ChunkHandler{ tag: RIFFTag::Chunk(*b"dibh"), parse: AVIState::parse_dibh },
];

impl<'a> DemuxCore<'a> for AVIDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let riff_tag = self.src.read_tag()?;
        let size     = self.src.read_u32le()? as usize;
        let avi_tag  = self.src.read_tag()?;
        validate!(&riff_tag == b"RIFF" && &avi_tag == b"AVI ");
        self.state.size = size;

        match parse_chunks(&mut self.state, &mut *self.src, strmgr, seek_index, AVI_ROOT_CHUNKS, size as u64 + 8) {
            Ok(()) => {},
            Err(DemuxerError::EOF) | Err(DemuxerError::IOError) => {},
            Err(err) => return Err(err),
        }

        validate!(self.state.movi_pos != 0);
        validate!(self.state.got_vstream);

        self.src.seek(SeekFrom::Start(self.state.movi_pos))?;

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        if self.state.movi_size == 0 {
            return Err(EOF);
        }
        loop {
            if (self.src.tell() & 1) == 1 {
                self.src.read_skip(1)?;
                self.state.movi_size -= 1;
                if self.state.movi_size == 0 {
                    return Err(EOF);
                }
            }
            let is_keyframe = self.state.key_offs.binary_search(&self.src.tell()).is_ok();
            let tag = self.src.read_tag()?;
            let size = self.src.read_u32le()? as usize;
            match &tag {
                b"JUNK" => {
                    self.state.movi_size -= size + 8;
                    self.src.read_skip(size)?;
                    if self.state.movi_size == 0 {
                        return Err(EOF);
                    }
                },
                b"LIST" => {
                    self.state.movi_size -= 12;
                    self.src.read_skip(4)?;
                    if self.state.movi_size == 0 {
                        return Err(EOF);
                    }
                },
                b"idx1" | &[b'i', b'x', _, _] => {
                    return Err(EOF);
                },
                b"dibc" => {
                    let stream = strmgr.get_stream(0);
                    if stream.is_none() {
                        self.src.read_skip(size)?;
                        self.state.movi_size -= size + 8;
                        continue;
                    }
                    let stream = stream.unwrap();
                    if size == 0 {
                        self.state.movi_size -= 8;
                        if self.state.movi_size == 0 {
                            return Err(EOF);
                        }
                        continue;
                    }
                    let ts = stream.make_ts(Some(self.state.cur_frame), None, None);
                    let mut pkt = self.src.read_packet(stream, ts, is_keyframe, size)?;
                    if let Some(ref mut pe) = self.state.pal {
                        pkt.add_side_data(NASideData::Palette(pe.changed, pe.pal.clone()));
                        pe.changed = false;
                    }
                    self.state.cur_frame += 1;
                    self.state.movi_size -= size + 8;
                    return Ok(pkt);
                },
                _ => {
                    self.src.read_skip(4)?;
                    if self.state.movi_size == 0 {
                        return Err(EOF);
                    }
                }
            }
        }
    }

    fn seek(&mut self, time: NATimePoint, seek_index: &SeekIndex) -> DemuxerResult<()> {
        let ret = seek_index.find_pos(time);
        if ret.is_none() {
            return Err(DemuxerError::SeekError);
        }
        let seek_info = ret.unwrap();
        if seek_info.pos < self.state.movi_pos { return Err(DemuxerError::SeekError); }
        let skip_size = (seek_info.pos - self.state.movi_pos) as usize;
        if skip_size > self.state.movi_orig { return Err(DemuxerError::SeekError); }
        self.state.movi_size = self.state.movi_orig - skip_size;

        self.state.cur_frame = seek_info.pts;
        self.src.seek(SeekFrom::Start(seek_info.pos))?;

        Ok(())
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for AVIDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

impl<'a> AVIDemuxer<'a> {
    fn new(io: &'a mut dyn ByteIO) -> Self {
        AVIDemuxer {
            src: io,
            state: AVIState::new(),
        }
    }
}

fn find_raw_rgb_fmt(bm: &MSBitmapInfo, vhdr: &mut NAVideoInfo) -> bool {
    match &bm.compression {
        &[0, 0, 0, 0] | b"DIB " => {
            if bm.planes != 1 {
                return false;
            }
            let fmt_name = match bm.bitcount {
                     8 => "pal8",
                    16 => "bgr555",
                    24 => "bgr24",
                    32 => "bgra24",
                    _ => return false,
                };
            if let Ok(fmt) = NAPixelFormaton::from_str(fmt_name) {
                vhdr.format = fmt;
                vhdr.flipped = !bm.flipped;
                true
            } else {
                false
            }
        },
        _ => false,
    }
}

fn parse_idx1(src: &mut dyn ByteIO, strmgr: &mut StreamManager, seek_idx: &mut SeekIndex, size: usize, movi_pos: u64, key_offs: &mut Vec<u64>) -> DemuxerResult<usize> {
    validate!((size & 15) == 0);
    let num_entries = size >> 4;
    let mut counter = 0u64;
    let mut add_offset = 0;
    let mut set_offset = false;
    if let Some(stream) = strmgr.get_stream(0) {
        for _ in 0..num_entries {
            let tag             = src.read_tag()?;
            let _flags          = src.read_u32le()?;
            let mut offset      = src.read_u32le()? as u64;
            let _length         = src.read_u32le()?;

            if !set_offset && offset < movi_pos {
                add_offset = movi_pos - offset;
            }
            set_offset = true;

            offset += add_offset;

            if &tag == b"dibc" {
                let (tb_num, tb_den) = stream.get_timebase();
                let pts = counter;
                let time = NATimeInfo::rescale_ts(pts, tb_num, tb_den, 1, 1000);
                validate!(offset >= movi_pos);
                seek_idx.add_entry(0, SeekEntry { time, pts, pos: offset });
                key_offs.push(offset);
                counter += 1;
            }
        }
        key_offs.sort_unstable();
    }
    Ok(size)
}

pub struct AVIDemuxerCreator { }

impl DemuxerCreator for AVIDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(AVIDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "avi-dib" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_avi_dib_demux() {
        //test sample from the beta AVI SDK
        let mut file = File::open("assets/MS/CAR_CD.AVI").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = AVIDemuxer::new(&mut br);
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
