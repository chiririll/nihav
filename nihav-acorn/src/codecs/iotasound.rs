use nihav_core::codecs::*;
use nihav_codec_support::codecs::imaadpcm::*;
use std::str::FromStr;

struct IotaSoundDecoder {
    ainfo:      NAAudioInfo,
    chmap:      NAChannelMap,
    ch_state:   [IMAState; 2],
}

impl IotaSoundDecoder {
    fn new() -> Self {
        Self {
            ainfo:      NAAudioInfo::new(8000, 1, SND_S16P_FORMAT, 0),
            chmap:      NAChannelMap::new(),
            ch_state:   [IMAState::new(), IMAState::new()],
        }
    }
}

impl NADecoder for IotaSoundDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Audio(ainfo) = info.get_properties() {
            let channels = ainfo.get_channels();
            validate!(channels == 1 || channels == 2);
            self.ainfo = NAAudioInfo::new(ainfo.get_sample_rate(), channels, SND_S16_FORMAT, 2);
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
            let channels = self.ainfo.get_channels();
            let nsamples = pktbuf.len() * 2 / usize::from(channels);
            let abuf = alloc_audio_buffer(self.ainfo, nsamples, self.chmap.clone())?;
            let mut adata = abuf.get_abuf_i16().unwrap();
            let dst = adata.get_data_mut().unwrap();
            let idx2 = if channels == 1 { 0 } else { 1 };
            for (dpair, &src) in dst.chunks_exact_mut(2).zip(pktbuf.iter()) {
                dpair[0] = self.ch_state[0].expand_sample(src >> 4);
                dpair[1] = self.ch_state[idx2].expand_sample(src & 0xF);
            }

            let mut frm = NAFrame::new_from_pkt(pkt, info, abuf);
            frm.set_duration(Some(nsamples as u64));
            frm.set_keyframe(false);
            Ok(frm.into_ref())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn flush(&mut self) {
        self.ch_state[0].reset(0, 0);
        self.ch_state[1].reset(0, 0);
    }
}

impl NAOptionHandler for IotaSoundDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(IotaSoundDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::{RegisteredDecoders, RegisteredPacketisers};
    use nihav_core::demuxers::RegisteredRawDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_iota_sound() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // sample from All About Planes
        test_decoding_raw("armovie", "iota-sound", "assets/Acorn/wessex", None,
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5([0x19509699, 0x0a2134c9, 0xef46040e, 0xa4ccd672]));
    }
}
