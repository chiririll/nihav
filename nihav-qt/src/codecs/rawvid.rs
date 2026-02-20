use nihav_core::codecs::*;
use std::str::FromStr;

#[derive(Clone,Copy,Debug,PartialEq)]
enum RawType {
    YUV2,
    YUV4,
}

struct RawDecoder {
    info:       NACodecInfoRef,
    width:      usize,
    height:     usize,
    rtype:      RawType,
}

impl RawDecoder {
    fn new(rtype: RawType) -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            width:      0,
            height:     0,
            rtype,
        }
    }
}

impl NADecoder for RawDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.get_width();
            self.height = vinfo.get_height();
            let fmt = match self.rtype {
                    RawType::YUV2 => {
                        validate!((self.width & 1) == 0);
                        NAPixelFormaton::from_str("yuv422p").unwrap()
                    }
                    RawType::YUV4 => {
                        validate!((self.width & 1) == 0 && (self.height & 1) == 0);
                        YUV420_FORMAT
                    }
                };
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, fmt));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        let buf = match self.rtype {
            RawType::YUV2 => {
                validate!(src.len() == self.width * 2 * self.height);
                let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 1)?;
                let mut buf = bufinfo.get_vbuf().unwrap();
                let mut yoffset = buf.get_offset(0);
                let mut uoffset = buf.get_offset(1);
                let mut voffset = buf.get_offset(2);
                let ystride = buf.get_stride(0);
                let ustride = buf.get_stride(1);
                let vstride = buf.get_stride(2);
                let data = buf.get_data_mut().unwrap();
                for line in src.chunks_exact(self.width * 2) {
                    for (x, yuv2) in line.chunks_exact(4).enumerate() {
                        data[yoffset + x * 2]     = yuv2[0];
                        data[yoffset + x * 2 + 1] = yuv2[2];
                        data[uoffset + x]         = yuv2[1] ^ 0x80;
                        data[voffset + x]         = yuv2[3] ^ 0x80;
                    }
                    yoffset += ystride;
                    uoffset += ustride;
                    voffset += vstride;
                }
                bufinfo
            },
            RawType::YUV4 => {
                validate!(src.len() == self.width * self.height * 3 / 2);
                let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 1)?;
                let mut buf = bufinfo.get_vbuf().unwrap();
                let mut yoffset = buf.get_offset(0);
                let mut uoffset = buf.get_offset(1);
                let mut voffset = buf.get_offset(2);
                let ystride = buf.get_stride(0);
                let ustride = buf.get_stride(1);
                let vstride = buf.get_stride(2);
                let data = buf.get_data_mut().unwrap();
                for line in src.chunks_exact(self.width / 2 * 6) {
                    for (x, yuv4) in line.chunks_exact(6).enumerate() {
                        data[yoffset + x * 2]               = yuv4[2];
                        data[yoffset + x * 2 + 1]           = yuv4[3];
                        data[yoffset + x * 2     + ystride] = yuv4[4];
                        data[yoffset + x * 2 + 1 + ystride] = yuv4[5];
                        data[uoffset + x]                   = yuv4[0] ^ 0x80;
                        data[voffset + x]                   = yuv4[1] ^ 0x80;
                    }
                    yoffset += ystride * 2;
                    uoffset += ustride;
                    voffset += vstride;
                }
                bufinfo
            },
        };

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buf);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for RawDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_yuv2() -> Box<dyn NADecoder + Send> {
    Box::new(RawDecoder::new(RawType::YUV2))
}
pub fn get_decoder_yuv4() -> Box<dyn NADecoder + Send> {
    Box::new(RawDecoder::new(RawType::YUV4))
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::qt_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_yuv2() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        // sample from https://samples.mplayerhq.hu/V-codecs/yuv2.mov
        test_decoding("mov", "qt-yuv2", "assets/QT/yuv2.mov", Some(1), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0x330b401e, 0x399ff70b, 0xf078973d, 0x4e6609b4]));
    }
    #[test]
    fn test_yuv4() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        // sample from https://samples.mplayerhq.hu/V-codecs/yuv4/lenayuv4.mov
        test_decoding("mov", "qt-yuv2", "assets/QT/lenayuv4.mov", None, &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0xd41d8cd9, 0x8f00b204, 0xe9800998, 0xecf8427e]));
    }
}
