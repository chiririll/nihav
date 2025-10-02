use nihav_core::io::byteio::*;
use nihav_core::io::bitreader::*;
use nihav_core::io::codebook::*;
use nihav_core::formats;
use nihav_core::codecs::*;

const ESCAPE: u8 = 0x80;

static YV92C_SYMS: [u8; 46] = [
 0x00, 0x01, 0xFF, 0x02, 0xFE, 0x03, 0xFD, 0xFC,
 0x04, 0xFB, 0x05, 0xFA, 0x06, 0xF9, 0x07, 0x08,
 0xF8, ESCAPE, 0xF7, 0x09, 0xF6, 0x0A, 0xF5, 0x0B,
 0xF4, 0x0C, 0xF3, 0x0D, 0x0E, 0xF2, 0x0F, 0xF1,
 0x10, 0xF0, 0x11, 0xEF, 0x12, 0xEE, 0x13, 0xED,
 0x14, 0xEC, 0x15, 0xEB, 0x16, 0x17
];

static YV92C_CODE_BITS: &[u16] = &[
 0x0001, 0x0003, 0x0002, 0x0006, 0x0004, 0x000C, 0x0008, 0x0028,
 0x0018, 0x0038, 0x0010, 0x0050, 0x0030, 0x0070, 0x0020, 0x00A0,
 0x0060, 0x00E0, 0x0040, 0x0140, 0x00C0, 0x01C0, 0x0080, 0x0480,
 0x0280, 0x0680, 0x0180, 0x0580, 0x0380, 0x0780, 0x0100, 0x0500,
 0x0300, 0x0700, 0x0200, 0x0A00, 0x0600, 0x0E00, 0x0400, 0x1400,
 0x0C00, 0x1C00, 0x0800, 0x2800, 0x1800, 0x3800
];

static YV92C_CODE_LENGTHS: &[u8] = &[
  2,  2,  3,  3,  4,  4,  6,  6,  6,  6,  7,  7,  7,  7,  8,  8,
  8,  8,  9,  9,  9,  9, 11, 11, 11, 11, 11, 11, 11, 11, 11, 11,
 11, 11, 12, 12, 12, 12, 13, 13, 13, 13, 14, 14, 14, 14
];

struct YV92Decoder {
    info:    NACodecInfoRef,
    cb:      Codebook<u8>,
    top:     Vec<u8>,
}

#[allow(clippy::identity_op)]
impl YV92Decoder {
    fn new() -> Self {
        let dummy_info = NACodecInfo::new_dummy();
        let mut coderead = TableCodebookDescReader::new(YV92C_CODE_BITS, YV92C_CODE_LENGTHS, |idx| YV92C_SYMS[idx]);
        let cb = Codebook::new(&mut coderead, CodebookMode::LSB).unwrap();
        YV92Decoder { info: dummy_info, cb, top: Vec::new() }
    }

    fn decode_plane(&mut self, br: &mut BitReader,
                    buf: &mut NAVideoBuffer<u8>, planeno: usize) -> DecoderResult<()> {
        let offs = buf.get_offset(planeno);
        let (w, h) = buf.get_dimensions(planeno);
        let stride = buf.get_stride(planeno);
        let cb = &self.cb;

        let data = buf.get_data_mut().unwrap();
        let framebuf: &mut [u8] = data.as_mut_slice();

        let mut pred = 0x80u8;
        for dst in framebuf[offs..][..w].iter_mut() {
            let mut delta = br.read_cb(cb)?;
            if delta == ESCAPE {
                delta = br.read(8)? as u8;
            }
            pred = pred.wrapping_add(delta << 1);
            *dst = pred;
        }
        self.top.resize(w, 0);
        self.top.copy_from_slice(&framebuf[offs..][..w]);

        for line in framebuf[offs..].chunks_exact_mut(stride).take(h).skip(1) {
            let mut last_top = 0x80;
            pred = 0x80;
            for (dst, top) in line.iter_mut().zip(self.top.iter_mut()) {
                let mut delta = br.read_cb(cb)?;
                if delta == ESCAPE {
                    delta = br.read(8)? as u8;
                }
                pred = ((i16::from(*top) - last_top + i16::from(pred)).clamp(0, 255) as u8).wrapping_add(delta << 1);
                last_top = i16::from(*top);
                *dst = pred;
                *top = pred;
            }
        }
        Ok(())
    }
}

impl NADecoder for YV92Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let (w, h) = (vinfo.width, vinfo.height);
            let f = vinfo.is_flipped();
            let fmt = formats::YUV410_FORMAT;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, f, fmt));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        const YV92_START: usize = 32;

        let src = pkt.get_buffer();
        if src.len() <= YV92_START { return Err(DecoderError::ShortData); }

        let vinfo = self.info.get_properties().get_video_info().unwrap();
        let bufinfo = alloc_video_buffer(vinfo, 2)?;
        let mut buf = bufinfo.get_vbuf().unwrap();
        for plane in 0..3 {
            let plane_off = read_u32le(&src[0x10 + plane * 4..])? as usize;
            validate!(plane_off >= YV92_START && plane_off < src.len());
            let mut br = BitReader::new(&src[plane_off..], BitReaderMode::LE);
            let planeno = if plane == 0 { 0 } else { plane ^ 3 };
            self.decode_plane(&mut br, &mut buf, planeno)?;
        }
        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {}
}

impl NAOptionHandler for YV92Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(YV92Decoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_yv92() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        indeo_register_all_decoders(&mut dec_reg);

        // sample from PC Pro 1996 April - The Windows Show
        test_decoding("avi", "yv92", "assets/Indeo/pcpos.avi", Some(2),
                      &dmx_reg, &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xad4ef973, 0x49b97ea3, 0xe1d2868a, 0xd5462621],
                            [0x2802891d, 0x7163b4e6, 0x160144f2, 0xd87529aa],
                            [0xb95d4aee, 0xbb0275f8, 0xe250ea17, 0xc7666495]]));
    }
}
