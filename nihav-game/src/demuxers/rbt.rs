use nihav_core::frame::*;
use nihav_core::demuxers::*;

const AFRAME_HDR_SIZE: usize = 16;

#[allow(dead_code)]
struct RobotDemuxer<'a> {
    src:        &'a mut ByteReader<'a>,
    version:    u16,
    vpts:       u64,
    apts:       u64,
    pkt_no:     usize,
    audio:      bool,
    has_audio:  bool,
    initial:    Vec<u8>,
    a_id:       Option<usize>,
    v_id:       Option<usize>,
    nframes:    usize,
    vframe_len: Vec<u32>,
    pkt_len:    Vec<u32>,
}

impl<'a> DemuxCore<'a> for RobotDemuxer<'a> {
    #[allow(unused_variables)]
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let mut hdr = [0; 60];
                                          self.src.read_buf(&mut hdr)?;
        validate!(hdr[0] == 0x16 || hdr[0] == 0x3D);
        validate!(&hdr[2..6] == b"SOL\0");
        self.version = read_u16le(&hdr[6..])?;
        let aframe_size = read_u16le(&hdr[8..])? as usize;
        validate!(self.version >= 4 && self.version <= 6);

        self.nframes = read_u16le(&hdr[14..])? as usize;
        validate!(self.nframes > 0);
        let pal_size = read_u16le(&hdr[16..])? as usize;
        let audio_pre_size = read_u16le(&hdr[18..])? as usize;

        let mut width = read_u16le(&hdr[20..])? as usize;
        if width == 0 { width = 640; }
        let mut height = read_u16le(&hdr[22..])? as usize;
        if height == 0 { height = 480; }
        let has_pal = hdr[24] != 0;
        self.has_audio = hdr[25] != 0;
        let fps = read_u16le(&hdr[28..])?;
        if !self.has_audio || audio_pre_size == 0 {
                                          self.src.read_skip(audio_pre_size)?;
        } else {
            let end_pos = self.src.tell() + (audio_pre_size as u64);
            validate!(audio_pre_size >= 12);
            validate!(aframe_size > AFRAME_HDR_SIZE);
            let pre_size                = self.src.read_u32le()? as usize;
            validate!(pre_size <= audio_pre_size - 14);
            let method                  = self.src.read_u16le()?;
            validate!(method == 0);
            let size1                   = self.src.read_u32le()? as usize;
            let size2                   = self.src.read_u32le()? as usize;
            validate!(size1 + size2 <= pre_size);
            let to_skip = (aframe_size - AFRAME_HDR_SIZE) / 2;
            if size1 + size2 > to_skip {
                self.initial.resize(size1 + size2 + 1 - to_skip, 0);
                self.initial[0] = 0;
                                          self.src.read_buf(&mut self.initial[1..])?;
            }
                                          self.src.seek(SeekFrom::Start(end_pos))?;
        }
        let mut pal = vec![0; pal_size + 2];
        pal[0] = self.version as u8;
        pal[1] = has_pal as u8;
                                          self.src.read_buf(&mut pal[2..])?;
        self.vframe_len.clear();
        self.vframe_len.reserve(self.nframes);
        if self.version < 6 {
            for _ in 0..self.nframes {
                let size                = self.src.read_u16le()?;
                self.vframe_len.push(u32::from(size));
            }
        } else {
            for _ in 0..self.nframes {
                let size                = self.src.read_u32le()?;
                self.vframe_len.push(size);
            }
        }

        self.pkt_len.clear();
        self.pkt_len.reserve(self.nframes);
        if self.version < 6 {
            for _ in 0..self.nframes {
                let size                = self.src.read_u16le()?;
                self.pkt_len.push(u32::from(size));
            }
        } else {
            for _ in 0..self.nframes {
                let size                = self.src.read_u32le()?;
                self.pkt_len.push(size);
            }
        }
                                          self.src.read_skip(256 * 4)?; // cues
                                          self.src.read_skip(256 * 2)?; // smth
        let pos = (self.src.tell() & 0x7FF) as usize;
        if pos != 0 {
                                          self.src.read_skip(0x800 - pos)?;
        }

        let vhdr = NAVideoInfo::new(width, height, false, PAL8_FORMAT);
        let vci = NACodecTypeInfo::Video(vhdr);
        let vinfo = NACodecInfo::new("rbt-video", vci, Some(pal));
        self.v_id = strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, 1, fps.into(), self.nframes as u64));
        if self.has_audio {
            let ahdr = NAAudioInfo::new(11025, 2, SND_S16_FORMAT, 1);
            let ainfo = NACodecInfo::new("rbt-audio", NACodecTypeInfo::Audio(ahdr), None);
            self.a_id = strmgr.add_stream(NAStream::new(StreamType::Audio, 1, ainfo, 1, 22050, 2));
        }
        self.apts = 0;
        self.vpts = 0;
        self.pkt_no = 0;
        self.audio = false;

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        if self.has_audio && !self.initial.is_empty() {
            let mut buf = Vec::new();
            std::mem::swap(&mut self.initial, &mut buf);
            if let Some(a_id) = self.a_id {
                let stream = strmgr.get_stream(a_id).unwrap();
                let ts = stream.make_ts(Some(0), None, None);
                self.apts += (buf.len() - 1) as u64;
                return Ok(NAPacket::new(stream, ts, true, buf));
            }
        }
        loop {
            if self.pkt_no >= self.nframes {
                return Err(DemuxerError::EOF);
            }

            if !self.audio {
                let stream = strmgr.get_stream(self.v_id.unwrap_or(0)).unwrap();
                let ts = stream.make_ts(Some(self.vpts), None, None);
                self.vpts += 1;

                let mut buf = vec![0; self.vframe_len[self.pkt_no] as usize];
                                      self.src.read_buf(&mut buf)?;

                if self.has_audio {
                    self.audio = true;
                } else {
                                      self.src.read_skip((self.pkt_len[self.pkt_no] - self.vframe_len[self.pkt_no]) as usize)?;
                    self.pkt_no += 1;
                }

                return Ok(NAPacket::new(stream, ts, self.vpts == 1, buf));
            } else {
                let asize = (self.pkt_len[self.pkt_no] - self.vframe_len[self.pkt_no]) as usize;
                validate!(asize >= AFRAME_HDR_SIZE);
                self.audio = false;
                self.pkt_no += 1;

                let _ref_apts           = u64::from(self.src.read_u32le()?);
                let ref_asize           = self.src.read_u32le()? as usize;
                validate!(asize == ref_asize + 8);

                if let Some(a_id) = self.a_id {
                    let stream = strmgr.get_stream(a_id).unwrap();
                    let ts = stream.make_ts(Some(self.apts), None, None);
                    self.apts += (ref_asize - 8) as u64;
                    let mut buf = vec![0; ref_asize + 1];
                    buf[0] = 1;
                                          self.src.read_buf(&mut buf[1..])?;
                    return Ok(NAPacket::new(stream, ts, true, buf));
                } else {
                                          self.src.read_skip(ref_asize)?;
                }
            }
        }
    }

    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}
impl<'a> NAOptionHandler for RobotDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}
impl<'a> RobotDemuxer<'a> {
    fn new(io: &'a mut ByteReader<'a>) -> Self {
        RobotDemuxer {
            src:        io,
            vpts:       0,
            apts:       0,
            a_id:       None,
            v_id:       None,
            nframes:    0,
            pkt_no:     0,
            audio:      false,
            has_audio:  false,
            initial:    Vec::new(),
            version:    0,
            vframe_len: Vec::new(),
            pkt_len:    Vec::new(),
        }
    }
}

pub struct RobotDemuxerCreator { }

impl DemuxerCreator for RobotDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut ByteReader<'a>) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(RobotDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "sierra-rbt" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_rbt_v4() {
        // sample from SWAT demo
        let mut file = File::open("assets/Game/sierra/12.rbt").unwrap();
        let mut fr = FileReader::new_read(&mut file);
        let mut br = ByteReader::new(&mut fr);
        let mut dmx = RobotDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();
        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if (e as i32) == (DemuxerError::EOF as i32) { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }
    #[test]
    fn test_rbt_v5() {
        // sample from Space Quest 6
        let mut file = File::open("assets/Game/sierra/410.rbt").unwrap();
        let mut fr = FileReader::new_read(&mut file);
        let mut br = ByteReader::new(&mut fr);
        let mut dmx = RobotDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();
        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if (e as i32) == (DemuxerError::EOF as i32) { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }
    #[test]
    fn test_rbt_v6() {
        // sample from Rama
        let mut file = File::open("assets/Game/sierra/7531.RBT").unwrap();
        let mut fr = FileReader::new_read(&mut file);
        let mut br = ByteReader::new(&mut fr);
        let mut dmx = RobotDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();
        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if (e as i32) == (DemuxerError::EOF as i32) { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }
}
