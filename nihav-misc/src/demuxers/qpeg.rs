use nihav_core::demuxers::*;

struct QPEGDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    frameno:        u32,
}

impl<'a> QPEGDemuxer<'a> {
    fn new(src: &'a mut dyn ByteIO) -> Self {
        QPEGDemuxer {
            src,
            frameno:    0,
        }
    }
}

impl<'a> DemuxCore<'a> for QPEGDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let mut magic = [0; 6];
                                          self.src.read_buf(&mut magic)?;
        validate!(&magic == b"IDVCd\0");
        let height                      = self.src.read_u16le()? as usize;
        let width                       = self.src.read_u16le()? as usize;
        validate!((1..=640).contains(&width) && (1..=480).contains(&height));
        self.src.seek(SeekFrom::Start(0x60))?;

        let mut pal = vec![0; 768];
                                          self.src.read_buf(&mut pal[60..])?;

        let vci = NACodecTypeInfo::Video(NAVideoInfo::new(width, height, false, PAL8_FORMAT));
        let vinfo = NACodecInfo::new("qpeg-dvc", vci, Some(pal));
        if strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, 1, 24, 0)).is_none() {
            return Err(DemuxerError::MemoryError);
        }

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        let tag                         = self.src.read_u16le()?;
        if tag == 0 {
            return Err(DemuxerError::EOF);
        }
        let fsize                       = self.src.read_u32le()? as usize;
        validate!(fsize > 4);
        self.frameno += 1;

        if let Some(stream) = strmgr.get_stream(0) {
            let ts = stream.make_ts(Some(u64::from(self.frameno - 1)), None, None);
            self.src.read_packet(stream, ts, self.frameno == 1, fsize - 4)
        } else {
            Err(DemuxerError::InvalidData)
        }
    }

    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for QPEGDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub struct QPEGDemuxerCreator { }

impl DemuxerCreator for QPEGDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(QPEGDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "qpeg" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_qpeg_demux() {
        // sample from Inside Multimedia 1994 October
        let mut file = File::open("assets/Misc/club01.dvc").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = QPEGDemuxer::new(&mut br);
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
