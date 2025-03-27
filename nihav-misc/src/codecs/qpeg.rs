use nihav_core::io::byteio::{ByteReader,MemoryReader};
use nihav_core::codecs::*;

struct DVCDecoder {
    info:       NACodecInfoRef,
    pal:        [u8; 768],
    frame:      Vec<u8>,
    width:      usize,
    height:     usize,
}

impl DVCDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfo::new_dummy(),
            pal:        [0; 768],
            frame:      Vec::new(),
            width:      0,
            height:     0,
        }
    }
}

impl NADecoder for DVCDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            self.width = w;
            self.height = h;
            self.frame = vec![0; self.width * self.height];
            if let Some(pal) = info.get_extradata() {
                validate!(pal.len() == 768);
                self.pal.copy_from_slice(&pal);
            }
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        let mut mr = MemoryReader::new_read(&src);
        let mut br = ByteReader::new(&mut mr);

        let mut pos = 0;
        let mut is_intra = true;
        br.read_skip(128)?;
        let remap = &src[..128];
        loop {
            let op = br.read_byte()?;
            match op {
                0x00 => {
                    validate!(pos < self.frame.len());
                    pos += 1;
                    is_intra = false;
                },
                0x01..=0x7F => {
                    validate!(pos < self.frame.len());
                    self.frame[pos] = remap[usize::from(op)];
                    pos += 1;
                },
                0x80..=0xBF => {
                    let skip = usize::from(op & 0x3F);
                    let skip = match skip {
                            0x00 => usize::from(br.read_byte()?) + 0x40,
                            0x01 => usize::from(br.read_byte()?) + 0x140,
                            _ => skip,
                        };
                    validate!(pos + skip <= self.frame.len());
                    pos += skip;
                    is_intra = false;
                },
                0xC0..=0xDF => {
                    let len = usize::from(op & 0x1F) + 1;
                    validate!(pos + len <= self.frame.len());
                    br.read_buf(&mut self.frame[pos..][..len])?;
                    pos += len;
                },
                0xE0 => {
                    if pos != self.frame.len() {
                        is_intra = false;
                    }
                    break;
                },
                0xE1..=0xFF => {
                    let run = usize::from(op & 0x1F) + 1;
                    validate!(pos + run <= self.frame.len());
                    let clr = br.read_byte()?;
                    for el in self.frame[pos..][..run].iter_mut() {
                        *el = clr;
                    }
                    pos += run;
                },
            }
        }

        let vinfo = NAVideoInfo::new(self.width, self.height, false, PAL8_FORMAT);
        let bufinfo = alloc_video_buffer(vinfo, 0)?;

        if let Some(mut buf) = bufinfo.get_vbuf() {
            let stride = buf.get_stride(0);
            let pal_off = buf.get_offset(1);
            let data = buf.get_data_mut().unwrap();

            for (dline, sline) in data.chunks_mut(stride).zip(self.frame.chunks_exact(self.width)) {
                dline[..self.width].copy_from_slice(sline);
            }
            data[pal_off..][..768].copy_from_slice(&self.pal);
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.frame.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for DVCDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_dvc() -> Box<dyn NADecoder + Send> {
    Box::new(DVCDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_qpeg_dvc() {
        let mut dmx_reg = RegisteredDemuxers::new();
        misc_register_all_demuxers(&mut dmx_reg);
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Inside Multimedia 1994 October
        test_decoding("qpeg", "qpeg-dvc", "assets/Misc/club01.dvc", Some(3), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x90f20329, 0x305bf02c, 0xb14217d1, 0x9213dfe6],
                            [0x22e11154, 0x51deb566, 0xda570987, 0xa044b123],
                            [0x7eba24ae, 0x3b786669, 0xb49681b1, 0x447d48c9],
                            [0xdad2b6bf, 0xa533f767, 0x6ace31cf, 0x5d8a7318]]));
    }
}
