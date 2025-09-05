use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_codec_support::codecs::imaadpcm::*;
use std::str::FromStr;

struct DVIADPCMDecoder {
    ainfo:      NAAudioInfo,
    chmap:      NAChannelMap,
    ch_state:   [IMAState; 2],
}

impl DVIADPCMDecoder {
    fn new() -> Self {
        Self {
            ainfo:      NAAudioInfo::new(0, 1, SND_S16P_FORMAT, 0),
            chmap:      NAChannelMap::new(),
            ch_state:   [IMAState::new(), IMAState::new()],
        }
    }
}

fn expand_word(dst: &mut [i16], mut word: u16, state: &mut IMAState) {
    for dsamp in dst.iter_mut().take(4) {
        *dsamp = state.expand_sample((word >> 12) as u8);
        word <<= 4;
    }
}

impl NADecoder for DVIADPCMDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Audio(ainfo) = info.get_properties() {
            let channels = ainfo.get_channels() as usize;
            validate!(channels == 2 || channels == 1);
            self.ainfo = NAAudioInfo::new(ainfo.get_sample_rate(), channels as u8, SND_S16P_FORMAT, 1);
            self.chmap = NAChannelMap::from_str(if channels == 1 { "C" } else { "L,R" }).unwrap();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let info = pkt.get_stream().get_info();
        if let NACodecTypeInfo::Audio(_) = info.get_properties() {
            let src = pkt.get_buffer();
            let channels = self.chmap.num_channels();
            validate!(src.len() >= 16);
            let _frameno = read_u32le(&src)?;
            let _time = read_u32le(&src[4..])?;
            let size = read_u32le(&src[12..])? as usize;
            validate!(src.len() == size + 16);

            if size == 0 {
                let len = 22050 / 30;
                let abuf = alloc_audio_buffer(self.ainfo, len, self.chmap.clone())?;
                let mut frm = NAFrame::new_from_pkt(pkt, info.replace_info(NACodecTypeInfo::Audio(self.ainfo)), abuf);
                frm.set_duration(Some(len as u64));
                frm.set_keyframe(false);
                return Ok(frm.into_ref());
            }

            validate!(size > 24);
            let abuf;
            let nsamples;
            if src[18] == 0xFF {
                let size = read_u16le(&src[16..])? as usize;
                validate!(size + 24 == src.len());
                let id = src[19];
                validate!((channels == 1 && id == 1) || (channels == 2 && id == 3));

                const BLOCK_SAMPLES: usize = 61;
                const BLOCK_SIZE: usize = 32;
                nsamples = (src.len() - 24) / BLOCK_SIZE * BLOCK_SAMPLES;
                abuf = alloc_audio_buffer(self.ainfo, nsamples, self.chmap.clone())?;
                let mut adata = abuf.get_abuf_i16().unwrap();
                //let mut off = [adata.get_offset(0), adata.get_offset(1)];
                let dst = adata.get_data_mut().unwrap();

                if channels == 1 {
                    for (dblk, block) in dst.chunks_exact_mut(BLOCK_SAMPLES).zip(src[24..].chunks_exact(BLOCK_SIZE)) {
                        expand_word(&mut dblk[..4], read_u16le(block).unwrap_or_default(), &mut self.ch_state[0]);
                        let init = read_u16le(&block[2..]).unwrap_or_default();
                        let pred = (init as i16) & !0x7F;
                        let step = (init & 0x7F) as u8;
                        validate!(step <= IMA_MAX_STEP);
                        dblk[4] = pred;
                        self.ch_state[0].reset(pred, step);
                        for (quad, w) in dblk[5..].chunks_exact_mut(4).zip(block[4..].chunks_exact(2)) {
                            let word = read_u16le(w).unwrap_or_default();
                            expand_word(quad, word, &mut self.ch_state[0]);
                        }
                    }
                } else {
                    return Err(DecoderError::NotImplemented);
                }
            } else { // this is a guesswork but it's better than nothing
                nsamples = (src.len() - 20) * 2 + 1;
                abuf = alloc_audio_buffer(self.ainfo, nsamples, self.chmap.clone())?;
                let mut adata = abuf.get_abuf_i16().unwrap();
                //let mut off = [adata.get_offset(0), adata.get_offset(1)];
                let dst = adata.get_data_mut().unwrap();

                if channels == 1 {
                    let step = src[16];
                    let pred = read_u16le(&src[18..]).unwrap_or_default() as i16;
                    validate!(step <= IMA_MAX_STEP);
                    dst[0] = pred;
                    self.ch_state[0].reset(pred, step);
                    for (pair, &b) in dst[1..].chunks_exact_mut(2).zip(src[20..].iter()) {
                        pair[0] = self.ch_state[0].expand_sample(b & 0xF);
                        pair[1] = self.ch_state[0].expand_sample(b >> 4);
                    }
                } else {
                    return Err(DecoderError::NotImplemented);
                }
            }

            let mut frm = NAFrame::new_from_pkt(pkt, info.replace_info(NACodecTypeInfo::Audio(self.ainfo)), abuf);
            frm.set_duration(Some(nsamples as u64));
            frm.set_keyframe(false);
            Ok(frm.into_ref())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for DVIADPCMDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(DVIADPCMDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_dvi_adpcm_old() {
        let mut dmx_reg = RegisteredDemuxers::new();
        indeo_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        indeo_register_all_decoders(&mut dec_reg);

        // sample from IBM CD Showcase
        test_decoding("dvi", "dvi-adpcm", "assets/Indeo/video.avs", None, &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0xfd1f6ee5, 0xf9e2c670, 0xfa51bc92, 0x70a2e488]));
    }
    #[test]
    fn test_dvi_adpcm_new() {
        let mut dmx_reg = RegisteredDemuxers::new();
        indeo_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        indeo_register_all_decoders(&mut dec_reg);

        // sample from D\Vision PRO v2.1
        test_decoding("dvi", "dvi-adpcm", "assets/Indeo/AUDM400.AVS", None, &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0x8dc1e085, 0x1e51e54f, 0x562f9add, 0x275a9c6f]));
    }
}
