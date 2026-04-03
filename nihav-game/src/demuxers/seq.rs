use nihav_core::frame::*;
use nihav_core::demuxers::*;

struct SequenceDemuxer<'a> {
    src:        &'a mut dyn ByteIO,
    nframes:    usize,
    frame_no:   usize,
}

impl<'a> DemuxCore<'a> for SequenceDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let src = &mut self.src;

        self.nframes                    = src.read_u16le()? as usize;
        validate!(self.nframes > 0);
        let pal_size                    = src.read_u32le()? as usize;
        validate!(pal_size > 37 && pal_size <= 1024 + 37);
        let mut pal_chunk = vec![0; pal_size];
                                          src.read_buf(&mut pal_chunk)?;

        let vhdr = NAVideoInfo::new(320, 200, false, PAL8_FORMAT);
        let vci = NACodecTypeInfo::Video(vhdr);
        let vinfo = NACodecInfo::new("seq-video", vci, Some(pal_chunk));
        if strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, 1, 10, self.nframes as u64)).is_none() {
            return Err(DemuxerError::MemoryError);
        }

        self.frame_no = 0;
        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        const FRAME_HEADER: usize = 24;

        if self.frame_no == self.nframes {
            return Err(DemuxerError::EOF);
        }

        let stream = strmgr.get_stream(0).unwrap();
        let ts = stream.make_ts(Some(self.frame_no as u64), None, None);

        let mut buf = vec![0; FRAME_HEADER];
                                  self.src.read_buf(&mut buf)?;
        let frm_size = read_u16le(&buf[12..])? as usize;
        let offset              = u64::from(self.src.read_u32le()?);
        validate!(offset >= self.src.tell());
                                  self.src.seek(SeekFrom::Start(offset))?;
        buf.resize(FRAME_HEADER + frm_size, 0);
                                  self.src.read_buf(&mut buf[FRAME_HEADER..])?;

        self.frame_no += 1;

        Ok(NAPacket::new(stream, ts, self.frame_no == 1, buf))
    }

    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}
impl<'a> NAOptionHandler for SequenceDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}
impl<'a> SequenceDemuxer<'a> {
    fn new(io: &'a mut dyn ByteIO) -> Self {
        SequenceDemuxer {
            src:        io,
            frame_no:   0,
            nframes:    0,
        }
    }
}

pub struct SequenceDemuxerCreator { }

impl DemuxerCreator for SequenceDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(SequenceDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "sierra-seq" }
}

#[cfg(test)]
mod test {
    use super::*;
    use nihav_codec_support::test::dmx_test::*;

    // sample from King's Quest VI
    #[test]
    fn test_seq() {
        test_file_demux("assets/Game/sierra/FS1.SEQ", &SequenceDemuxerCreator{}, 0, DemuxerTestMode::Demux);
    }
}
