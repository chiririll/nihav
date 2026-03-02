// Somewhat simplified JPEG decoder

use nihav_core::io::byteio::{ByteIO,MemoryReader};
use nihav_core::codecs::*;
use nihav_codec_support::codecs::jpeg::*;

struct MoviePakDecoder {
    info:       NACodecInfoRef,
    jpeg:       JPEGDecoder,
}

impl MoviePakDecoder {
    fn new() -> Self {
        let mut jpeg = JPEGDecoder::new();
        jpeg.cb = JCodebooks::default_jpeg_codebooks();
        jpeg.subsamp = [0x21, 0x11, 0x11, 0, 0];
        jpeg.max_h = 2;
        jpeg.max_v = 1;

        Self {
            info:       NACodecInfo::new_dummy(),
            jpeg,
        }
    }
}

impl NADecoder for MoviePakDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            self.jpeg.width  = w;
            self.jpeg.height = h;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, YUV420_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        let mut br = MemoryReader::new_read(&src);
        let width               = usize::from(br.read_u16be()?);
        let height              = usize::from(br.read_u16be()?);
        validate!(width > 0 && height > 0);
        self.jpeg.width  = width;
        self.jpeg.height = height;
        let _field1_size        = br.read_u32be()? as usize;
        let field2_size         = br.read_u32be()? as usize;
        if field2_size != 0 {
            return Err(DecoderError::NotImplemented);
        }
        for el in self.jpeg.quant[0].iter_mut() {
            *el                 = i16::from(br.read_byte()?);
        }
        for el in self.jpeg.quant[1].iter_mut() {
            *el                 = i16::from(br.read_byte()?);
        }

        let formaton = NAPixelFormaton {
                model:      ColorModel::YUV(YUVSubmodel::YUVJ),
                components: 3,
                comp_info:  [
                    Some(NAPixelChromaton {
                        h_ss: 0, v_ss: 0,
                        packed:     false,
                        depth:      8,
                        shift:      0,
                        comp_offs:  0,
                        next_elem:  1,
                    }),
                    Some(NAPixelChromaton {
                        h_ss: 1, v_ss: 0,
                        packed:     false,
                        depth:      8,
                        shift:      0,
                        comp_offs:  1,
                        next_elem:  1,
                    }),
                    Some(NAPixelChromaton {
                        h_ss: 1, v_ss: 0,
                        packed:     false,
                        depth:      8,
                        shift:      0,
                        comp_offs:  2,
                        next_elem:  1,
                    }),
                    None,
                    None
                ],
                elem_size:  0,
                be:         false,
                alpha:      false,
                palette:    false,
            };
        let vinfo = NAVideoInfo::new(self.jpeg.width, self.jpeg.height, false, formaton);
        let bufinfo = alloc_video_buffer(vinfo, 4)?;

        if let Some(buf) = bufinfo.get_vbuf() {
            let ci = [
                ComponentInfo { component_id: 0, dc_table_id: 0, ac_table_id: 0 },
                ComponentInfo { component_id: 1, dc_table_id: 1, ac_table_id: 1 },
                ComponentInfo { component_id: 2, dc_table_id: 1, ac_table_id: 1 },
            ];
            self.jpeg.decode_image(JPEGType::Baseline, &src[br.tell() as usize..], None, buf, &ci)?;
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for MoviePakDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(MoviePakDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::misc_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_moviepak() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);
        // sample from Mac Magazin and MacEasy Magazine CD Issue 34
        test_decoding("mov-macbin", "moviepak", "assets/Misc/The Toaster Part 1.movie", Some(20), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x35ed5acb, 0xfef01d75, 0x0a6af750, 0x8c22ca1d],
                            [0x0f66a11b, 0x0b63d153, 0xa2fd5898, 0x7a8057d3],
                            [0xe0354509, 0xe394c475, 0x1833fda5, 0x8f0e3568]]));
    }
}
