use std::convert::TryInto;
use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_codec_support::codecs::imaadpcm::*;

const PACKET_LEN: usize = 34;
const PACKET_SAMPLES: usize = 64;

struct NibbleWriter<'a> {
    buf:    &'a mut [u8; PACKET_LEN],
    pos:    usize,
    val:    u8,
    first:  bool,
}

impl<'a> NibbleWriter<'a> {
    fn new(buf: &'a mut [u8; PACKET_LEN]) -> Self {
        Self { buf, pos: 2, val: 0, first: true }
    }
    fn write(&mut self, nib: u8) {
        if self.first {
            self.val = nib;
            self.first = false;
        } else {
            self.val |= nib << 4;
            self.buf[self.pos] = self.val;
            self.pos += 1;
            self.first = true;
        }
    }
}

fn encode_packet(samples: &[i16; PACKET_SAMPLES], dst: &mut [u8; PACKET_LEN]) {
    if samples == &[0; PACKET_SAMPLES] { return; }

    let mut pred = IMAState::new();
    pred.reset(samples[0] & !0x7F, calc_step(samples[0], samples[1]));

    let hdr = (pred.predictor as u16) | (pred.step as u16);
    let _ = write_u16be(dst, hdr);
    let mut nw = NibbleWriter::new(dst);
    for &samp in samples.iter() {
        let nib = pred.compress_sample(samp);
        pred.expand_sample(nib);
        nw.write(nib);
    }
}
fn calc_step(samp1: i16, samp2: i16) -> u8 {
    let diff = (i32::from(samp1) - i32::from(samp2)).abs();
    for (i, &step) in IMA_STEP_TABLE.iter().enumerate() {
        if step >= diff {
            return i as u8;
        }
    }
    IMA_MAX_STEP
}

#[derive(Default)]
struct IMAADPCMEncoder {
    stream:     Option<NAStreamRef>,
    samples:    Vec<Vec<i16>>,
    time:       u64,
    block_len:  usize,
    block_size: usize,
    flush:      bool,
    srate:      u32,
}

impl IMAADPCMEncoder {
    fn new() -> Self { Self::default() }
    fn encode_packet(&mut self) -> EncoderResult<NAPacket> {
        if self.flush && (1..PACKET_SAMPLES).contains(&self.samples[0].len()) {
            for smp in self.samples.iter_mut() {
                smp.resize(PACKET_SAMPLES, 0);
            }
        }
        let nblocks = self.samples[0].len() / PACKET_SAMPLES;
        if nblocks == 0 {
            return Err(EncoderError::TryAgain);
        }

        let mut dbuf = vec![0u8; nblocks * self.samples.len() * PACKET_LEN];

        for (n, oblock) in dbuf.chunks_exact_mut(PACKET_LEN * self.samples.len()).enumerate() {
            for (dst, samps) in oblock.chunks_exact_mut(PACKET_LEN).zip(self.samples.iter()) {
                let blk = dst.try_into().unwrap();
                let src = samps[n * PACKET_SAMPLES..][..PACKET_SAMPLES].try_into().unwrap();
                encode_packet(src, blk);
            }
        }

        for samps in self.samples.iter_mut() {
            samps.drain(..nblocks * PACKET_SAMPLES);
        }

        let ts = NATimeInfo::new(Some(self.time), None, Some(nblocks as u64), PACKET_SAMPLES as u32, self.srate);
        self.time += nblocks as u64;
        Ok(NAPacket::new(self.stream.clone().unwrap(), ts, true, dbuf))
    }
}

impl NAEncoder for IMAADPCMEncoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                Ok(EncodeParameters {
                    format: NACodecTypeInfo::Audio(NAAudioInfo::new(0, 1, SND_S16_FORMAT, PACKET_SAMPLES)),
                    ..Default::default() })
            },
            NACodecTypeInfo::Video(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(ainfo) => {
                let mut outinfo = ainfo;
                outinfo.channels = outinfo.channels.max(1);
                if outinfo.format != SND_S16P_FORMAT && outinfo.format != SND_S16_FORMAT {
                    outinfo.format = SND_S16_FORMAT;
                }
                if outinfo.block_len <= PACKET_SAMPLES {
                    outinfo.block_len = PACKET_SAMPLES;
                }
                outinfo.block_len /= PACKET_SAMPLES;
                outinfo.block_len *= PACKET_SAMPLES;
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
                if ainfo.format != SND_S16P_FORMAT && ainfo.format != SND_S16_FORMAT {
                    return Err(EncoderError::FormatError);
                }
                if ainfo.block_len < PACKET_SAMPLES || (ainfo.block_len % PACKET_SAMPLES) != 0 {
                    return Err(EncoderError::FormatError);
                }
                let channels = usize::from(ainfo.channels);
                self.samples.clear();
                for _ in 0..channels {
                    self.samples.push(Vec::new());
                }
                self.block_len = ainfo.block_len;
                self.block_size = (self.block_len / PACKET_SAMPLES) * PACKET_LEN * channels;

                let out_ainfo = NAAudioInfo::new(ainfo.sample_rate, ainfo.channels, SND_S16P_FORMAT, self.block_size);
                let info = NACodecInfo::new("ima-adpcm-qt", NACodecTypeInfo::Audio(out_ainfo), None);
                let mut stream = NAStream::new(StreamType::Audio, stream_id, info, PACKET_SAMPLES as u32, ainfo.sample_rate, 0);
                stream.set_num(stream_id as usize);
                let stream = stream.into_ref();

                self.stream = Some(stream.clone());
                self.srate = ainfo.sample_rate;
                self.flush = false;

                Ok(stream)
            },
        }
    }
    fn encode(&mut self, frm: &NAFrame) -> EncoderResult<()> {
        let buf = frm.get_buffer();
        if let Some(ref abuf) = buf.get_abuf_i16() {
            let src = abuf.get_data();
            let len = abuf.get_length();
            let ch  = abuf.get_chmap().num_channels();
            if abuf.get_step() == 1 || ch == 1 {
                let astride = abuf.get_stride().max(len);
                for (samp, src) in self.samples.iter_mut().zip(src.chunks_exact(astride)) {
                    samp.extend_from_slice(&src[..len]);
                }
            } else {
                for samps in src.chunks_exact(ch) {
                    for (dst, &samp) in self.samples.iter_mut().zip(samps.iter()) {
                        dst.push(samp);
                    }
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

impl NAOptionHandler for IMAADPCMEncoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) {}
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_encoder() -> Box<dyn NAEncoder + Send> {
    Box::new(IMAADPCMEncoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::*;
    use nihav_core::muxers::RegisteredMuxers;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::enc_video::*;
    use crate::*;
    use nihav_commonfmt::*;

    #[test]
    fn test_encoder() {
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
                enc_name:       "ima-adpcm-qt",
                out_name:       "ima-adpcm-qt.mov",
                mux_reg, enc_reg,
            };
        let dst_ainfo = NAAudioInfo {
                sample_rate:    0,
                channels:       0,
                format:         SND_S16_FORMAT,
                block_len:      128,
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
                          &[0x656ee676, 0x807322f6, 0xbb99d846, 0x222e269b]);
    }
}
