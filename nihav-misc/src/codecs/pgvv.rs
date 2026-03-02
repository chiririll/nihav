// Somewhat simplified JPEG decoder

use nihav_core::io::byteio::{ByteIO,MemoryReader};
use nihav_core::codecs::*;
use nihav_codec_support::codecs::jpeg::*;
use std::str::FromStr;

struct RadiusStudioDecoder {
    info:       NACodecInfoRef,
    jpeg:       JPEGDecoder,
}

fn pgvv_blk_pred(comp_no: usize, blk: &mut [i16; 64], last_dc: &mut [i16; MAX_CHROMATONS]) {
    let cno = comp_no.min(1);
    blk[0] += last_dc[cno];
    last_dc[cno] = blk[0];
}

impl RadiusStudioDecoder {
    fn new() -> Self {
        let mut jpeg = JPEGDecoder::new();
        jpeg.cb = JCodebooks::default_jpeg_codebooks();
        jpeg.subsamp = [0x21, 0x11, 0x11, 0, 0];
        jpeg.blk_pred = pgvv_blk_pred;
        jpeg.max_h = 2;
        jpeg.max_v = 1;

        Self {
            info:       NACodecInfo::new_dummy(),
            jpeg,
        }
    }
}

impl NADecoder for RadiusStudioDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            self.jpeg.width  = w;
            self.jpeg.height = h;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, YUV420_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        if src.len() < 2048 { return Err(DecoderError::ShortData); }
        let mut br = MemoryReader::new_read(&src);
        let size_v0             = br.read_u32be()? as usize;
        let field2_size         = br.read_u32be()? as usize;
        let interlaced = field2_size != 0;
        let version             = br.read_u16be()?;
        validate!(version <= 2);
        let hdr_size = if version == 0 { 16 } else { 32 };
                                  br.read_skip(1)?;
        let quant               = br.read_byte()?;
                                  br.read_skip(8)?;
        let size_v1             = br.read_u32be()? as usize;

        let field0_size = if version == 0 { size_v0 } else { size_v1 };

        let formaton = NAPixelFormaton::from_str("yuv422p").unwrap();
        let vinfo = NAVideoInfo::new(self.jpeg.width, self.jpeg.height, false, formaton);
        let bufinfo = alloc_video_buffer(vinfo, 4)?;

        for (i, (&luma_q, &chroma_q)) in DEF_LUMA_QUANT.iter().zip(DEF_CHROMA_QUANT.iter()).enumerate() {
            self.jpeg.quant[0][i] = ((i16::from(quant) * i16::from(luma_q) + 25) / 50).clamp(1, 255);
            self.jpeg.quant[1][i] = ((i16::from(quant) * i16::from(chroma_q) + 25) / 50).clamp(1, 255);
        }

        if let Some(buf) = bufinfo.get_vbuf() {
            let ci = [
                ComponentInfo { component_id: 0, dc_table_id: 0, ac_table_id: 0 },
                ComponentInfo { component_id: 1, dc_table_id: 1, ac_table_id: 1 },
                ComponentInfo { component_id: 2, dc_table_id: 1, ac_table_id: 1 },
            ];

            let src2 = if interlaced {
                    let field2_off = hdr_size + ((field0_size + 0x7FF) & !0x7FF);
                    validate!(field2_off < src.len());
                    Some(&src[field2_off..])
                } else { None };
            self.jpeg.decode_image(JPEGType::Baseline, &src[hdr_size..], src2, buf, &ci)?;
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for RadiusStudioDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(RadiusStudioDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::misc_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_pgvv() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);
        // sample: https://samples.mplayerhq.hu/V-codecs/PGVV-RadiusStudio/airfone.mov
        test_decoding("mov", "pgvv", "assets/Misc/airfone.mov", Some(6), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x35a92349, 0x7817f341, 0x204c8116, 0xee5b5d3a],
                            [0x72300fde, 0x64071e12, 0x653fe45f, 0xd948ec97],
                            [0x8d610362, 0x1328776c, 0x231cec68, 0xf2706c00],
                            [0xde75bd2f, 0xbcadf8e9, 0xf47c36c0, 0x8c326890]]));
    }
}
