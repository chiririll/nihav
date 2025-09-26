use nihav_core::io::byteio::{ByteIO,MemoryReader};
use nihav_core::codecs::*;

struct ARBCDecoder {
    info:       NACodecInfoRef,
    frame:      Vec<u8>,
    width:      usize,
    height:     usize,
    is_qt:      bool,
}

impl ARBCDecoder {
    fn new(is_qt: bool) -> Self {
        Self {
            info:       NACodecInfo::new_dummy(),
            frame:      Vec::new(),
            width:      0,
            height:     0,
            is_qt,
        }
    }
    fn fill_tile(&mut self, br: &mut MemoryReader, tile_size: usize, rgb: [u8; 3]) -> DecoderResult<()> {
        let num_tiles = if !self.is_qt { usize::from(br.read_u16le()?) } else {  usize::from(br.read_u16be()?) };

        let stride = self.width * 3;
        for _ in 0..num_tiles {
            let ypos = usize::from(br.read_byte()?);
            let xpos = usize::from(br.read_byte()?);
            validate!(xpos * tile_size < self.width && ypos * tile_size < self.height);
            let mut mask = if !self.is_qt { usize::from(br.read_u16le()?) } else {  usize::from(br.read_u16be()?) };

            for strip in self.frame[xpos * tile_size * 3 + ypos * tile_size * stride..]
                    .chunks_mut(stride * (tile_size / 4)).take(4) {
                let mut mmask = mask;
                for x in (0..tile_size).step_by(tile_size / 4) {
                    if (mmask & 0x8000) != 0 {
                        for line in strip[x * 3..].chunks_mut(stride) {
                            for el in line.chunks_exact_mut(3).take(tile_size / 4) {
                                el.copy_from_slice(&rgb);
                            }
                        }
                    }
                    mmask <<= 1;
                }
                mask <<= 4;
            }
        }

        Ok(())
    }
}

impl NADecoder for ARBCDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            self.width = w;
            self.height = h;
            self.frame = vec![0; self.width * self.height * 3];
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, !self.is_qt, RGB24_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        let mut br = MemoryReader::new_read(&src);

        br.read_u16be()?; // always 1?
        for _ in 0..3 {
            br.read_u16be()?; // some masks?
        }
        let nsegs = if !self.is_qt { usize::from(br.read_u16le()?) } else {  usize::from(br.read_u16be()?) };

        for _ in 0..nsegs {
            let r     = br.read_byte()?;
                        br.read_byte()?;
            let g     = br.read_byte()?;
                        br.read_byte()?;
            let b     = br.read_byte()?;
                        br.read_byte()?;
            let flags = br.read_byte()?;

            let mut tile_size = 1024;
            let mut mask = 0x10;
            while mask > 0 {
                if (flags & mask) != 0 {
                    self.fill_tile(&mut br, tile_size, [r, g, b])?;
                }
                mask >>= 1;
                tile_size >>= 2;
            }
        }

        let vinfo = NAVideoInfo::new(self.width, self.height, !self.is_qt, RGB24_FORMAT);
        let bufinfo = alloc_video_buffer(vinfo, 0)?;

        if let Some(mut buf) = bufinfo.get_vbuf() {
            let stride = buf.get_stride(0);
            let data = buf.get_data_mut().unwrap();

            for (dline, sline) in data.chunks_mut(stride).zip(self.frame.chunks_exact(self.width * 3)) {
                dline[..self.width * 3].copy_from_slice(sline);
            }
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        if pkt.get_pts() == Some(0) {
            frm.set_keyframe(true);
            frm.set_frame_type(FrameType::I);
        }
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.frame.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for ARBCDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_vfw() -> Box<dyn NADecoder + Send> {
    Box::new(ARBCDecoder::new(false))
}

pub fn get_decoder_qt() -> Box<dyn NADecoder + Send> {
    Box::new(ARBCDecoder::new(true))
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_arbc_vfw() {
        let mut dmx_reg = RegisteredDemuxers::new();
        misc_register_all_demuxers(&mut dmx_reg);
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Batman Activity Center
        test_decoding("avi", "gryphon-arbc-vfw", "assets/Misc/ivycrop.avi", Some(3), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xb4c2e4e7, 0x567116eb, 0xbe1a57b6, 0xa1962fb0],
                            [0x20b6d8d3, 0x4f888a4a, 0x71a59496, 0x9373f33d],
                            [0x8fffa195, 0x90df69f9, 0x36ce119f, 0xab3d05ac],
                            [0x6c7cab71, 0xd17a566a, 0x83339172, 0x763cfebf]]));
    }
    #[test]
    fn test_arbc_qt() {
        let mut dmx_reg = RegisteredDemuxers::new();
        misc_register_all_demuxers(&mut dmx_reg);
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Lion King Activity Center
        test_decoding("mov-macbin", "gryphon-arbc-qt", "assets/Misc/Hangman Movie", Some(200), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xd16838a7, 0x2dcff5a4, 0x0752e6c3, 0xbf7a6509],
                            [0xa0c3200d, 0x8a66a140, 0x8f3eb885, 0x8c00ec63],
                            [0x9069680d, 0x23ef2126, 0x859f834f, 0xc74b9f5a],
                            [0xcc18b439, 0x32f9b628, 0x2630d79b, 0x5d11edbc]]));
    }
}
