use nihav_core::demuxers::*;

struct MVIDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    is_large:       bool,
    vpacket:        Option<NAPacket>,
    aframe:         usize,
    asize:          usize,
    apos:           usize,
    nframes:        usize,
    frm_no:         usize,
    apts:           u64,
}

impl<'a> MVIDemuxer<'a> {
    fn new(src: &'a mut dyn ByteIO) -> Self {
        MVIDemuxer {
            src,
            is_large:   false,
            vpacket:    None,
            aframe:     0,
            asize:      0,
            apos:       0,
            nframes:    0,
            frm_no:     0,
            apts:       0,
        }
    }
}

impl<'a> DemuxCore<'a> for MVIDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let version                     = self.src.read_byte()?;
        let mut flags = vec![0; 2];
                                          self.src.read_buf(&mut flags)?;
        let nframes                     = self.src.read_u32le()? as usize;
        validate!((1..=50000).contains(&nframes));
        let delay                       = self.src.read_u32le()?;
        validate!((10000..=1000000).contains(&delay));
        let width                       = self.src.read_u16le()? as usize;
        let height                      = self.src.read_u16le()? as usize;
        validate!((1..=640).contains(&width) && (1..=480).contains(&height));
                                          self.src.read_byte()?;
        let arate                       = u32::from(self.src.read_u16le()?);
        let asize                       = self.src.read_u32le()?;
                                          self.src.read_byte()?;
        let player_ver                  = self.src.read_u32le()?;
        if version > 7 || player_ver > 0xD5 {
            return Err(DemuxerError::NotImplemented);
        }
                                          self.src.read_u16le()?;
                                          self.src.read_byte()?;

        let vci = NACodecTypeInfo::Video(NAVideoInfo::new(width, height, false, PAL8_FORMAT));
        let vinfo = NACodecInfo::new("mvi0", vci, Some(flags));
        if strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, delay, 1000000, nframes as u64)).is_none() {
            return Err(DemuxerError::MemoryError);
        }
        if arate > 0 {
            validate!((1..(1 << 24)).contains(&asize));
            let aci = NACodecTypeInfo::Audio(NAAudioInfo::new(arate, 1, SND_U8_FORMAT, 2));
            let ainfo = NACodecInfo::new("pcm", aci, None);
            if strmgr.add_stream(NAStream::new(StreamType::Audio, 1, ainfo, 1, arate, u64::from(asize))).is_none() {
                return Err(DemuxerError::MemoryError);
            }
            self.asize = asize as usize;
            self.aframe = (((self.asize as u64) << 10) / (nframes as u64)) as usize;
            self.apos = ((arate as usize) * 830 / self.aframe - 1) * self.aframe + 512;
        }

        self.is_large = (width * height * 2) >= 0x10000;
        self.frm_no = 0;
        self.nframes = nframes;

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        let mut npkt = None;
        std::mem::swap(&mut npkt, &mut self.vpacket);
        if let Some(pkt) = npkt {
            return Ok(pkt);
        }
        if self.frm_no >= self.nframes {
            return Err(DemuxerError::EOF);
        }
        self.frm_no += 1;

        let vsize = if !self.is_large {
                                          usize::from(self.src.read_u16le()?)
            } else {
                                          self.src.read_u24le()? as usize
            };

        if self.asize > 0 {
            let asize = ((self.apos + self.aframe) >> 10).min(self.asize);
            validate!(self.asize > 0);
            self.asize -= asize;
            self.apos += self.aframe;
            self.apos -= asize << 10;

            if let (Some(vstream), Some(astream)) = (strmgr.get_stream(0), strmgr.get_stream(1)) {
                let ts = astream.make_ts(Some(self.apts), None, None);
                self.apts += asize as u64;
                let apkt = self.src.read_packet(astream, ts, true, asize)?;

                let ts = vstream.make_ts(Some(self.frm_no as u64 - 1), None, None);
                let vpkt = self.src.read_packet(vstream, ts, self.frm_no == 1, vsize)?;

                self.vpacket = Some(vpkt);
                Ok(apkt)
            } else {
                Err(DemuxerError::InvalidData)
            }
        } else if let Some(vstream) = strmgr.get_stream(0) {
            let ts = vstream.make_ts(Some(self.frm_no as u64 - 1), None, None);
            self.src.read_packet(vstream, ts, self.frm_no == 1, vsize)
        } else {
            Err(DemuxerError::InvalidData)
        }
    }
    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for MVIDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub struct MVIDemuxerCreator { }

impl DemuxerCreator for MVIDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(MVIDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "mvi" }
}

#[cfg(test)]
mod test {
    use super::*;
    use nihav_codec_support::test::dmx_test::*;

    #[test]
    fn test_mvi_demux() {
        // sample from AD&D Core Rules CD
        test_file_demux("assets/Misc/MP/avtest.mvi", &MVIDemuxerCreator{}, 0, DemuxerTestMode::Demux);
    }
}
