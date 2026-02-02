use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

const fn chord_val(val: u8) -> u32 {
    (val as u32) * 0x7FFF / 247
}

fn vidc_mu_law(val: u8) -> i16 {
    const CHORDS: [u32; 9] = [chord_val(0), chord_val(1), chord_val(3), chord_val(7),
        chord_val(15), chord_val(31), chord_val(63), chord_val(127), chord_val(247) ];

    let chord = ((val >> 4) & 7) as usize;
    let point = u32::from(val & 0xF);

    (CHORDS[chord] + point * (CHORDS[chord + 1] - CHORDS[chord]) / 8) as i16
}

#[derive(Default,Debug,Clone,Copy,PartialEq)]
enum CodecType {
    S8,
    #[default]
    U8,
    S16,
    Logarithmic,
}

#[derive(Default)]
struct RawDecoder {
    info:           NACodecInfoRef,
    chmap:          NAChannelMap,
    blk_size:       usize,
    codec_type:     CodecType,
}

impl RawDecoder {
    fn new() -> Self { Self::default() }
}

impl NADecoder for RawDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Audio(ainfo) = info.get_properties() {
            let channels = ainfo.channels;
            let srate    = ainfo.sample_rate;
            let bits     = ainfo.format.bits;
            validate!(info.get_extradata().is_some());

            if let Some(edata) = info.get_extradata() {
                let mut is_vidc = false;
                let mut is_unsigned = false;

                for i in 0..edata.len() {
                    let head = &edata[i..];
                    if (head.len() >= 4 && &head[..4] == b"VIDC") || (head.len() >= 11 && &head[..11] == b"exponential") {
                        is_vidc = true;
                        break;
                    }
                    if head.len() >= 8 && (&head[..8] == b"unsigned" || &head[..8] == b"UNSIGNED") {
                        is_unsigned = true;
                    }
                }
                self.codec_type = if is_vidc && bits != 16 {
                        CodecType::Logarithmic
                    } else if bits == 8 {
                        if is_unsigned {
                            CodecType::U8
                        } else {
                            CodecType::S8
                        }
                    } else {
                        CodecType::S16
                    };
            } else {
                return Err(DecoderError::InvalidData);
            }

            match channels {
                1 => self.chmap.add_channel(NAChannelType::C),
                2 => self.chmap.add_channels(&[NAChannelType::L, NAChannelType::R]),
                _ => return Err(DecoderError::InvalidData),
            }

            self.blk_size = match self.codec_type {
                    CodecType::U8 | CodecType::S8 | CodecType::Logarithmic => channels as usize,
                    CodecType::S16 => channels as usize * 2,
                };

            let fmt = match self.codec_type {
                    CodecType::U8 | CodecType::S8 => SND_U8_FORMAT,
                    CodecType::S16 | CodecType::Logarithmic => SND_S16_FORMAT,
                };

            let myinfo = NACodecTypeInfo::Audio(NAAudioInfo::new(srate, channels, fmt, 0));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(!src.is_empty() && (src.len() % self.blk_size) == 0);

        let bufinfo = alloc_audio_buffer(self.info.get_properties().get_audio_info().unwrap(), src.len() / self.blk_size, self.chmap.clone())?;
        match self.codec_type {
            CodecType::S8 => {
                let mut buf = bufinfo.get_abuf_u8().unwrap();
                let dst = buf.get_data_mut().unwrap();
                for (dst, &src) in dst.iter_mut().zip(src.iter()) {
                    *dst = src ^ 0x80;
                }
            },
            CodecType::U8 => {
                let mut buf = bufinfo.get_abuf_u8().unwrap();
                let dst = buf.get_data_mut().unwrap();
                dst[..src.len()].copy_from_slice(&src);
            },
            CodecType::S16 => {
                let mut buf = bufinfo.get_abuf_i16().unwrap();
                let dst = buf.get_data_mut().unwrap();

                for (dst, src) in dst.iter_mut().zip(src.chunks_exact(2)) {
                    *dst = read_u16le(src)? as i16;
                }
            },
            CodecType::Logarithmic => {
                let mut buf = bufinfo.get_abuf_i16().unwrap();
                let dst = buf.get_data_mut().unwrap();

                for (dst, &val) in dst.iter_mut().zip(src.iter()) {
                    let sign = (val & 0x01) != 0;
                    *dst = vidc_mu_law(val >> 1);
                    if sign {
                        *dst = -*dst;
                    }
                }
            },
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {}
}

impl NAOptionHandler for RawDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(RawDecoder::new())
}

#[derive(Default)]
struct RawPacketiser {
    stream:     Option<NAStreamRef>,
    blk_size:   usize,
    asize:      u64,
    buf:        Vec<u8>,
}

impl RawPacketiser {
    fn new() -> Self { Self::default() }
}

impl NAPacketiser for RawPacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
        if let NACodecTypeInfo::Audio(ainfo) = stream.get_info().get_properties() {
            self.blk_size = (ainfo.channels as usize) * (ainfo.format.bits as usize) / 8;
        }
        self.stream = Some(stream);
    }
    fn add_data(&mut self, src: &[u8]) -> bool {
        self.buf.extend_from_slice(src);
        self.buf.len() < (1 << 10)
    }
    fn parse_stream(&mut self, id: u32) -> DecoderResult<NAStreamRef> {
        if let Some(ref stream) = self.stream {
            let mut stream = NAStream::clone(stream);
            stream.id = id;
            Ok(stream.into_ref())
        } else {
            Err(DecoderError::MissingReference)
        }
    }
    fn skip_junk(&mut self) -> DecoderResult<usize> {
        Err(DecoderError::NotImplemented)
    }
    fn get_packet(&mut self, stream: NAStreamRef) -> DecoderResult<Option<NAPacket>> {
        if self.blk_size == 0 {
            return Err(DecoderError::MissingReference);
        }

        if self.buf.len() < 2 {
            return Ok(None);
        }

        let data_len = self.buf.len();
        let mut data = Vec::new();
        std::mem::swap(&mut self.buf, &mut data);

        let ts = NATimeInfo::new(Some(self.asize), None, None, stream.tb_num, stream.tb_den);
        self.asize += (data_len / self.blk_size) as u64;

        Ok(Some(NAPacket::new(stream, ts, true, data)))
    }
    fn reset(&mut self) {
        self.buf.clear();
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

pub fn get_packetiser() -> Box<dyn NAPacketiser + Send> {
    Box::new(RawPacketiser::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::{RegisteredDecoders, RegisteredPacketisers};
    use nihav_core::demuxers::RegisteredRawDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;

    #[test]
    fn test_format_s8() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Acorn Replay Demonstration Disc 2
        test_decoding_raw("armovie", "arm_rawaudio", "assets/Acorn/CHEMSET2", Some(1),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5([0x167985bf, 0xd82c3fb0, 0x125ff24e, 0xa7408c57]));
    }

    #[test]
    fn test_format_u8() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Cine Clips by Oregan Software Developments
        test_decoding_raw("armovie", "arm_rawaudio", "assets/Acorn/COLOURPLUS", Some(1),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5([0x2f0dc76b, 0x208372ad, 0xa986fb0b, 0x1024dcc8]));
    }

    #[test]
    fn test_format_vidc() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Cine Clips by Oregan Software Developments
        test_decoding_raw("armovie", "arm_rawaudio", "assets/Acorn/CFC2", Some(1),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5([0x3c2a6e48, 0x6e511c72, 0xd30b5813, 0x42d98a71]));
    }

    #[test]
    fn test_format_s16() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Cine Clips by Oregan Software Developments
        test_decoding_raw("armovie", "arm_rawaudio", "assets/Acorn/SKYHIGH", Some(1),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5([0xd2003a1c, 0xfe38974f, 0xa1850a5b, 0xb4313217]));
    }
}
