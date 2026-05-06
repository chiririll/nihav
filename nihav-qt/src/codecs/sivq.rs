use nihav_core::codecs::*;

struct SIVQDecoder {
    info:       NACodecInfoRef,
    width:      usize,
    height:     usize,
}

impl SIVQDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            width:      0,
            height:     0,
        }
    }
}

impl NADecoder for SIVQDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = (vinfo.get_width()  + 1) & !1;
            self.height = (vinfo.get_height() + 1) & !1;
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
        const CODEBOOK_SIZE: usize = 256 * 4 * 3;
        validate!(src.len() >= CODEBOOK_SIZE + self.width * self.height / 4);

        let (cb, indices) = src.split_at(CODEBOOK_SIZE);

        let buftype = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 1)?;
        let mut buf = buftype.get_vbuf().unwrap();
        let stride = buf.get_stride(0);
        let dst = buf.get_data_mut().unwrap();

        for (strip, idx_row) in dst.chunks_exact_mut(stride * 2)
                .zip(indices.chunks_exact(self.width / 2)) {
            for (x, &idx) in idx_row.iter().enumerate() {
                for (line, cb_row) in strip.chunks_exact_mut(stride)
                        .zip(cb[usize::from(idx) * 4 * 3..].chunks_exact(2 * 3)) {
                    line[x * 2 * 3..][..2 * 3].copy_from_slice(cb_row);
                }
            }
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buftype);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {}
}

impl NAOptionHandler for SIVQDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(SIVQDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::qt_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    // sample from QuickTime beta disc
    #[test]
    fn test_sivq() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        test_decoding("mov-macbin", "qt-sivq", "assets/QT/Football", Some(2), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x92628d33, 0xe3513f25, 0x7f887907, 0xe94d0d45],
                            [0x55758a95, 0x6e27c158, 0x718eae11, 0x100e76ee],
                            [0x084c60f2, 0xfbf75d07, 0x7f9d9caf, 0x713ef103]]));
    }
}
