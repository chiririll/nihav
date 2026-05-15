use nihav_core::codecs::*;
use std::str::FromStr;
use super::macecommon::*;

struct MaceDecoder {
    ainfo:      NAAudioInfo,
    chmap:      NAChannelMap,
    ch_pred:    [ChannelPredictor; 2],
    is_mace6:   bool,
}

impl MaceDecoder {
    fn new(is_mace6: bool) -> Self {
        Self {
            ainfo:      NAAudioInfo::new(0, 1, SND_U8P_FORMAT, 1),
            chmap:      NAChannelMap::new(),
            ch_pred:    [ChannelPredictor::default(); 2],
            is_mace6,
        }
    }
}

impl NADecoder for MaceDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Audio(ainfo) = info.get_properties() {
            let channels = ainfo.get_channels() as usize;
            validate!(channels == 2 || channels == 1);
            self.ainfo = NAAudioInfo::new(ainfo.get_sample_rate(), channels as u8, SND_U8P_FORMAT, 1);
            self.chmap = NAChannelMap::from_str(if channels == 1 { "C" } else { "L,R" }).unwrap();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let info = pkt.get_stream().get_info();
        if let NACodecTypeInfo::Audio(_) = info.get_properties() {
            let pktbuf = pkt.get_buffer();
            let channels = self.chmap.num_channels();
            let nsamples = pktbuf.len() * (if self.is_mace6 { 6 } else { 3 }) / channels;
            let abuf = alloc_audio_buffer(self.ainfo, nsamples, self.chmap.clone())?;
            let mut adata = abuf.get_abuf_u8().unwrap();
            let mut off = [adata.get_offset(0), adata.get_offset(1)];
            let dst = adata.get_data_mut().unwrap();

            if !self.is_mace6 {
                for src in pktbuf.chunks(channels) {
                    for ch in 0..channels {
                        let val = src[ch];
                        let idx0 = val & 7;
                        let idx1 = (val >> 3) & 3;
                        let idx2 = val >> 5;
                        dst[off[ch]]     = self.ch_pred[ch].pred_mace3(idx0 as usize, false);
                        dst[off[ch] + 1] = self.ch_pred[ch].pred_mace3(idx1 as usize, true);
                        dst[off[ch] + 2] = self.ch_pred[ch].pred_mace3(idx2 as usize, false);
                        off[ch] += 3;
                    }
                }
            } else {
                for src in pktbuf.chunks(channels) {
                    for ch in 0..channels {
                        let val = src[ch];
                        let idx0 = val >> 5;
                        let idx1 = (val >> 3) & 3;
                        let idx2 = val & 7;
                        let (s0, s1) = self.ch_pred[ch].pred_mace6(idx0 as usize, false);
                        dst[off[ch]]     = s0;
                        dst[off[ch] + 1] = s1;
                        let (s0, s1) = self.ch_pred[ch].pred_mace6(idx1 as usize, true);
                        dst[off[ch] + 2] = s0;
                        dst[off[ch] + 3] = s1;
                        let (s0, s1) = self.ch_pred[ch].pred_mace6(idx2 as usize, false);
                        dst[off[ch] + 4] = s0;
                        dst[off[ch] + 5] = s1;
                        off[ch] += 6;
                    }
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

impl NAOptionHandler for MaceDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_3() -> Box<dyn NADecoder + Send> {
    Box::new(MaceDecoder::new(false))
}

pub fn get_decoder_6() -> Box<dyn NADecoder + Send> {
    Box::new(MaceDecoder::new(true))
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::qt_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    // samples from https://samples.mplayerhq.hu/A-codecs/MACE/surge-samples/
    #[test]
    fn test_mace3() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "mace-3", "assets/QT/surge-1-8-MAC3.mov", None, &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0x2df88db3, 0xa6167019, 0x6d4c64e7, 0xc89da2a5]));
    }
    #[test]
    fn test_mace6() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "mace-6", "assets/QT/surge-1-8-MAC6.mov", None, &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0xc32857e2, 0xc1ea1ce8, 0x2d77dacf, 0xef504f1f]));
    }
}
