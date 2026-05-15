use nihav_core::codecs::*;
use super::macecommon::*;

const MIN_BLOCK_LEN: usize = 16 * 6;
const DEFAULT_BLOCK_LEN: usize = 128 * 3;

struct MaceEncoder {
    stream:     Option<NAStreamRef>,
    ch_pred:    [ChannelPredictor; 2],
    is_mace6:   bool,
    samples:    Vec<Vec<u8>>,
    rate:       u32,
    block_len:  usize,
    flush:      bool,
    ts:         u32,
}

fn samp_dist(a: u8, b: u8) -> u16 {
    let diff = u16::from(a.abs_diff(b));
    diff * diff
}

impl MaceEncoder {
    fn new(is_mace6: bool) -> Self {
        Self {
            stream:     None,
            samples:    Vec::new(),
            ch_pred:    [ChannelPredictor::default(); 2],
            block_len:  0,
            rate:       0,
            ts:         0,
            flush:      false,
            is_mace6,
        }
    }
    fn encode_packet(&mut self) -> EncoderResult<NAPacket> {
        let align = 6;
        let len = self.samples[0].len().min(self.block_len) / align * align;
        if len == 0 || (len < self.block_len && !self.flush) {
            return Err(EncoderError::TryAgain);
        }

        let mut dbuf = Vec::with_capacity(len * self.samples.len() / align);
        if !self.is_mace6 {
            let mut iters = Vec::new();
            for smp in self.samples.iter() {
                iters.push(smp.chunks_exact(3));
            }
            for _ in (0..len).step_by(6) {
                for (src, cpred) in iters.iter_mut().zip(self.ch_pred.iter_mut()) {
                    for _ in 0..2 {
                        let samples = src.next().unwrap();

                        let mut best_idx0 = 0;
                        let mut best_dist = u16::MAX;
                        for idx0 in 0..8 {
                            let mut cp = *cpred;
                            let samp = cp.pred_mace3(idx0, false);
                            let dist = samp_dist(samp, samples[0]);
                            if dist < best_dist {
                                best_idx0 = idx0;
                                best_dist = dist;
                            }
                        }
                        cpred.pred_mace3(best_idx0, false);

                        let mut best_idx1 = 0;
                        let mut best_dist = u16::MAX;
                        for idx1 in 0..4 {
                            let mut cp = *cpred;
                            let samp = cp.pred_mace3(idx1, true);
                            let dist = samp_dist(samp, samples[1]);
                            if dist < best_dist {
                                best_idx1 = idx1;
                                best_dist = dist;
                            }
                        }
                        cpred.pred_mace3(best_idx1, true);

                        let mut best_idx2 = 0;
                        let mut best_dist = u16::MAX;
                        for idx2 in 0..8 {
                            let mut cp = *cpred;
                            let samp = cp.pred_mace3(idx2, false);
                            let dist = samp_dist(samp, samples[2]);
                            if dist < best_dist {
                                best_idx2 = idx2;
                                best_dist = dist;
                            }
                        }
                        cpred.pred_mace3(best_idx2, false);

                        dbuf.push(((best_idx2 << 5) | (best_idx1 << 3) | best_idx0) as u8);
                    }
                }
            }
        } else {
            let mut iters = Vec::new();
            for smp in self.samples.iter() {
                iters.push(smp.chunks_exact(6));
            }
            for _ in (0..len).step_by(6) {
                for (src, cpred) in iters.iter_mut().zip(self.ch_pred.iter_mut()) {
                    let samples = src.next().unwrap();

                    let mut best_idx0 = 0;
                    let mut best_dist = u16::MAX;
                    for idx0 in 0..8 {
                        let mut cp = *cpred;
                        let (samp0, samp1) = cp.pred_mace6(idx0, false);
                        let dist = samp_dist(samp0, samples[0]) + samp_dist(samp1, samples[1]);
                        if dist < best_dist {
                            best_idx0 = idx0;
                            best_dist = dist;
                        }
                    }
                    cpred.pred_mace6(best_idx0, false);

                    let mut best_idx1 = 0;
                    let mut best_dist = u16::MAX;
                    for idx1 in 0..4 {
                        let mut cp = *cpred;
                        let (samp0, samp1) = cp.pred_mace6(idx1, true);
                        let dist = samp_dist(samp0, samples[2]) + samp_dist(samp1, samples[3]);
                        if dist < best_dist {
                            best_idx1 = idx1;
                            best_dist = dist;
                        }
                    }
                    cpred.pred_mace6(best_idx1, true);

                    let mut best_idx2 = 0;
                    let mut best_dist = u16::MAX;
                    for idx2 in 0..8 {
                        let mut cp = *cpred;
                        let (samp0, samp1) = cp.pred_mace6(idx2, false);
                        let dist = samp_dist(samp0, samples[4]) + samp_dist(samp1, samples[5]);
                        if dist < best_dist {
                            best_idx2 = idx2;
                            best_dist = dist;
                        }
                    }
                    cpred.pred_mace6(best_idx2, false);

                    dbuf.push(((best_idx0 << 5) | (best_idx1 << 3) | best_idx2) as u8);
                }
            }
        }
        for samps in self.samples.iter_mut() {
            samps.drain(..len);
        }

        let ts = NATimeInfo::new(Some(u64::from(self.ts)), None, Some(len as u64), 1, self.rate);
        self.ts += len as u32;
        Ok(NAPacket::new(self.stream.clone().unwrap(), ts, true, dbuf))
    }
}

impl NAEncoder for MaceEncoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                Ok(EncodeParameters {
                    format: NACodecTypeInfo::Audio(NAAudioInfo::new(0, 1, SND_S16_FORMAT, DEFAULT_BLOCK_LEN)),
                    ..Default::default() })
            },
            NACodecTypeInfo::Video(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(ainfo) => {
                let mut outinfo = ainfo;
                outinfo.channels = outinfo.channels.clamp(1, 2);
                outinfo.format = if outinfo.channels == 2 || outinfo.format == SND_U8P_FORMAT { SND_U8P_FORMAT } else { SND_U8_FORMAT };
                if outinfo.block_len < MIN_BLOCK_LEN {
                    outinfo.block_len = DEFAULT_BLOCK_LEN;
                }
                if (outinfo.block_len % 6) != 0 {
                    outinfo.block_len = (outinfo.block_len / 6 + 1) * 6;
                }
                let mut ofmt = *encinfo;
                ofmt.format = NACodecTypeInfo::Audio(outinfo);
                Ok(ofmt)
            }
        }
    }
    fn get_capabilities(&self) -> u64 { ENC_CAPS_CBR }
    fn init(&mut self, stream_id: u32, encinfo: EncodeParameters) -> EncoderResult<NAStreamRef> {
        match encinfo.format {
            NACodecTypeInfo::None => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(ainfo) => {
                if ainfo.format != SND_U8P_FORMAT && (ainfo.format != SND_U8_FORMAT || ainfo.channels != 1) {
                    return Err(EncoderError::FormatError);
                }
                let align = if self.is_mace6 { 6 } else { 3 };
                if ainfo.block_len < MIN_BLOCK_LEN || (ainfo.block_len % 6) != 0 {
                    return Err(EncoderError::FormatError);
                }
                if ainfo.channels != 1 && ainfo.channels != 2 {
                    return Err(EncoderError::FormatError);
                }
                for _ in 0..ainfo.channels {
                    self.samples.push(Vec::new());
                }
                self.block_len = ainfo.block_len;

                let out_ainfo = NAAudioInfo::new(ainfo.sample_rate, ainfo.channels, SND_U8P_FORMAT, self.block_len * usize::from(ainfo.channels) / align);
                let info = NACodecInfo::new(if self.is_mace6 { "mace-6" } else { "mace-3" }, NACodecTypeInfo::Audio(out_ainfo), None);
                let mut stream = NAStream::new(StreamType::Audio, stream_id, info, 1, ainfo.sample_rate, 0);
                stream.set_num(stream_id as usize);
                let stream = stream.into_ref();

                self.stream = Some(stream.clone());
                self.rate = ainfo.sample_rate;
                self.flush = false;

                Ok(stream)
            },
        }
    }
    fn encode(&mut self, frm: &NAFrame) -> EncoderResult<()> {
        let buf = frm.get_buffer();
        if let Some(ref abuf) = buf.get_abuf_u8() {
            let src = abuf.get_data();
            let len = abuf.get_length();
            let ch  = abuf.get_chmap().num_channels();
            if ch != self.samples.len() {
                return Err(EncoderError::InvalidParameters);
            }
            if abuf.get_step() == 1 || ch == 1 {
                let astride = abuf.get_stride();
                for (dst, samp) in self.samples.iter_mut().zip(src.chunks_exact(astride.max(len))) {
                    dst.extend_from_slice(&samp[..len]);
                }
            } else {
                for pair in src.chunks_exact(2).take(len) {
                    self.samples[0].push(pair[0]);
                    self.samples[1].push(pair[1]);
                }
            }
            Ok(())
        } else {
            Err(EncoderError::InvalidParameters)
        }
    }
    fn get_packet(&mut self) -> EncoderResult<Option<NAPacket>> {
        if let Ok(pkt) = self.encode_packet() {
            Ok(Some(pkt))
        } else {
            Ok(None)
        }
    }
    fn flush(&mut self) -> EncoderResult<()> {
        self.flush = true;
        Ok(())
    }
}

impl NAOptionHandler for MaceEncoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_encoder_3() -> Box<dyn NAEncoder + Send> {
    Box::new(MaceEncoder::new(false))
}

pub fn get_encoder_6() -> Box<dyn NAEncoder + Send> {
    Box::new(MaceEncoder::new(true))
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::*;
    use nihav_core::muxers::RegisteredMuxers;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::enc_video::*;
    use crate::*;
    use nihav_commonfmt::*;

    fn test_encoder(name: &'static str, enc_name: &'static str, hash: &[u32; 4]) {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        generic_register_all_decoders(&mut dec_reg);
        qt_register_all_decoders(&mut dec_reg);
        let mut mux_reg = RegisteredMuxers::new();
        generic_register_all_muxers(&mut mux_reg);
        let mut enc_reg = RegisteredEncoders::new();
        qt_register_all_encoders(&mut enc_reg);

        // sample from The Wonders of Electricity: An Adventure in Safety
        let dec_config = DecoderTestParams {
                demuxer:        "mov-resfork",
                in_name:        "assets/QT/car.mov",
                stream_type:    StreamType::Audio,
                limit:          None,
                dmx_reg, dec_reg,
            };
        let enc_config = EncoderTestParams {
                muxer:          "mov",
                enc_name,
                out_name:       name,
                mux_reg, enc_reg,
            };
        let dst_ainfo = NAAudioInfo {
                sample_rate:    0,
                channels:       0,
                format:         SND_U8_FORMAT,
                block_len:      128 * 3,
            };
        let enc_params = EncodeParameters {
                format:  NACodecTypeInfo::Audio(dst_ainfo),
                quality: 0,
                bitrate: 0,
                tb_num:  0,
                tb_den:  0,
                flags:   0,
            };
        let enc_options = &[];
//        test_encoding_to_file(&dec_config, &enc_config, enc_params, enc_options);

        test_encoding_md5(&dec_config, &enc_config, enc_params, enc_options,
                          hash);
    }
    #[test]
    fn test_mace3() {
        test_encoder("mace3.mov", "mace-3", &[0xa9d4302f, 0xc78d80f4, 0xc12c5485, 0x43943976]);
    }
    #[test]
    fn test_mace6() {
        test_encoder("mace6.mov", "mace-6", &[0xca9efa62, 0xa475a762, 0x8d5c70b9, 0x9a7f46b6]);
    }
}
