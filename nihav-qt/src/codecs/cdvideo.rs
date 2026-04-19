use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_codec_support::codecs::HAMShuffler;

#[derive(Debug,PartialEq)]
enum Mode {
    None,
    V4(u8),
    V1(u8),
    Skip(u8),
}

struct CDVideoDecoder {
    info:       NACodecInfoRef,
    hams:       HAMShuffler<u8>,
    width:      usize,
    height:     usize,
    cb:         [[[u8; 3]; 4]; 256],
}

impl CDVideoDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            hams:       HAMShuffler::default(),
            width:      0,
            height:     0,
            cb:         [[[0; 3]; 4]; 256],
        }
    }
}

fn yuv2rgb(rgb: &mut [u8; 3], y: u8, u: u8, v: u8) {
    let y = i16::from(y);
    let red = y + i16::from(u as i8) * 2;
    let blue = y + i16::from(v as i8) * 2;
    let green = y * 2 - (red >> 1) - (blue >> 1);
    *rgb = [red.clamp(0, 255) as u8, green.clamp(0, 255) as u8, blue.clamp(0, 255) as u8];
}

impl NADecoder for CDVideoDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = (vinfo.get_width()  + 3) & !3;
            self.height = (vinfo.get_height() + 3) & !3;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, RGB24_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    #[allow(clippy::cognitive_complexity)]
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 0x630);
        let mut br = MemoryReader::new_read(src.as_slice());

        let hdr             = br.read_u32be()?;
        if hdr == 22222222 {
                              br.read_u16be()?;
            let to_skip     = br.read_u16be()?;
            validate!(to_skip >= 8);
                              br.read_skip(usize::from(to_skip - 4))?;
        }
        let _size           = br.read_u32be()?;
                              br.read_u32be()?;
                              br.read_u32be()?;
        let is_yuv          = br.read_u32be()? > 0;
        let cb_size         = br.read_u32be()? as usize;
        if is_yuv {
            validate!((cb_size % 6) == 0 && (6..=0x600).contains(&cb_size));
            let mut yuv = [0; 6];
            for cb in self.cb.iter_mut().take(cb_size / 6) {
                              br.read_buf(&mut yuv)?;
                for (clr, &y) in cb.iter_mut().zip(yuv[2..].iter()) {
                    yuv2rgb(clr, y, yuv[0], yuv[1]);
                }
            }
        } else {
            validate!((cb_size % 12) == 0 && (3..=0xC00).contains(&cb_size));
            for cb in self.cb.iter_mut().take(cb_size / 12) {
                for clr in cb.iter_mut() {
                              br.read_buf(clr)?;
                }
            }
        }
        if (cb_size & 3) != 0 {
                              br.read_skip(4 - (cb_size & 3))?;
        }
                              br.read_u32be()?; // always 3?
                              br.read_u32be()?; // always 0?
        let width           = br.read_u32be()? as usize;
        let height          = br.read_u32be()? as usize;
        validate!(width  > 0 && width  <= self.width  && (width  & 3) == 0);
        validate!(height > 0 && height <= self.height && (height & 3) == 0);
                              br.read_u32be()?;
                              br.read_u32be()?;

        let mut is_intra = width == self.width && height == self.height;
        let bufret = self.hams.clone_ref();
        let mut buf;
        if let Some(bbuf) = bufret {
            buf = bbuf;
        } else {
            let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 2)?;
            buf = bufinfo.get_vbuf().unwrap();
            self.hams.add_frame(buf);
            buf = self.hams.get_output_frame().unwrap();
        }
        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();
        let stride = frm.stride[0];

        let mut mode = Mode::None;
        for strip in frm.data[frm.offset[0]..].chunks_exact_mut(stride * 4).take(height / 4) {
            for x in (0..width).step_by(4) {
                if mode == Mode::None {
                    let op  = br.read_byte()?;
                    let len = (op & 0x1F) + 1;
                    mode = match op >> 5 {
                            1 => Mode::V4(len),
                            6 => Mode::V1(len),
                            4 => Mode::Skip(len),
                            _ => return Err(DecoderError::InvalidData),
                        };
                }
                mode = match mode {
                    Mode::V4(len) => {
                        for sblk in 0..4 {
                            let idx = usize::from(br.read_byte()?);
                            let entry = &self.cb[idx];
                            let dst = &mut strip[(x + (sblk & 1) * 2) * 3 + (sblk >> 1) * 2 * stride..];
                            dst[..3].copy_from_slice(&entry[0]);
                            dst[3..][..3].copy_from_slice(&entry[1]);
                            dst[stride..][..3].copy_from_slice(&entry[0]);
                            dst[stride + 3..][..3].copy_from_slice(&entry[1]);
                        }
                        if len > 1 {
                            Mode::V4(len - 1)
                        } else {
                            Mode::None
                        }
                    },
                    Mode::V1(len) => {
                        let idx = usize::from(br.read_byte()?);
                        let entry = &self.cb[idx];
                        //xxx: the original code actually averages data here
                        for sblk in 0..4 {
                            let dst = &mut strip[(x + (sblk & 1) * 2) * 3 + (sblk >> 1) * 2 * stride..];
                            dst[..3].copy_from_slice(&entry[0]);
                            dst[3..][..3].copy_from_slice(&entry[1]);
                            dst[stride..][..3].copy_from_slice(&entry[0]);
                            dst[stride + 3..][..3].copy_from_slice(&entry[1]);
                        }
                        if len > 1 {
                            Mode::V1(len - 1)
                        } else {
                            Mode::None
                        }
                    },
                    Mode::Skip(len) => {
                        is_intra = false;
                        if len > 1 {
                            Mode::Skip(len - 1)
                        } else {
                            Mode::None
                        }
                    },
                    Mode::None => unreachable!(),
                };
            }
        }
        let buftype = NABufferType::VideoPacked(buf);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buftype);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        self.hams.clear();
    }
}

impl NAOptionHandler for CDVideoDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(CDVideoDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::qt_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    // samples is one of the navigable movies from the original QuickTime distribution
    #[test]
    fn test_cdvideo() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        test_decoding("mov-macbin", "qt-cdvideo", "assets/QT/Golden Gate", Some(6), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0xad94ad4f, 0x1b8148e5, 0x699e35ce, 0x562f1106],
                            [0xdbb07011, 0x605ef892, 0x166ac124, 0xe05dccea],
                            [0xc010996c, 0x6339c885, 0x64c6383b, 0xe1e4b7fa],
                            [0x27efd57d, 0xe2d95363, 0xad6b05f2, 0x812666e7],
                            [0x00ec0cee, 0xd9ca3b54, 0xaafa5501, 0x07dc49ef],
                            [0xfe3d2e1e, 0x2e0e60c5, 0x2eca8456, 0xa81b66b8],
                            [0x1c8dfa16, 0x86b8f0be, 0x245e994c, 0x9002edfa]]));
    }
}
