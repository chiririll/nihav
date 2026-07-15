use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_codec_support::codecs::HAMShuffler;

#[derive(Default)]
struct Pixar0Decoder {
    info:       NACodecInfoRef,
    width:      usize,
    height:     usize,
}

impl Pixar0Decoder {
    fn new() -> Self {
        Self::default()
    }
}

impl NADecoder for Pixar0Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = (vinfo.get_width()  + 3) & !3;
            self.height = (vinfo.get_height() + 3) & !3;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() == self.width * self.height);

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 2)?;
        let mut buf = bufinfo.get_vbuf().unwrap();

        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();
        let stride = frm.stride[0];
        let doff = frm.offset[0];

        let mut tiles = src.chunks_exact(16);
        for row in frm.data[doff..].chunks_exact_mut(stride * 4).take(frm.height[0] / 4) {
            for x in (0..self.width).step_by(4) {
                let tile = tiles.next().unwrap();
                for (dline, sline) in row.chunks_exact_mut(stride).zip(tile.chunks_exact(4)) {
                    dline[x..][..4].copy_from_slice(sline);
                }
            }
        }
        let dpal = &mut frm.data[frm.offset[1]..];
        for sd in pkt.side_data.iter() {
            if let NASideData::Palette(_, ref pal) = sd {
                for (dst, src) in dpal.chunks_mut(3).zip(pal.chunks(4)) {
                    dst[0] = src[0];
                    dst[1] = src[1];
                    dst[2] = src[2];
                }
            }
        }

        let buftype = NABufferType::Video(buf);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buftype);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {}
}

impl NAOptionHandler for Pixar0Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

#[derive(Default)]
struct PixarDecoder {
    info:       NACodecInfoRef,
    hams:       HAMShuffler<u8>,
    width:      usize,
    height:     usize,
    tiles:      Vec<[u8; 16]>,
    version:    u8,
}

impl PixarDecoder {
    fn new(version: u8) -> Self {
        Self { version, tiles: vec![[0; 16]; 65535], ..Default::default() }
    }
}

impl NADecoder for PixarDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = (vinfo.get_width()  + 3) & !3;
            self.height = (vinfo.get_height() + 3) & !3;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() >= 2);
        let mut br = MemoryReader::new_read(src.as_slice());

        if self.version == 5 {
            let _frameno                = br.read_u16be()?;
            let tot_tiles               = usize::from(br.read_u16be()?);
            if tot_tiles != 0 && tot_tiles != 0xFFFF {
                                          br.read_u16be()?;
                                          br.read_u16be()?;
            }
        }

        let ntiles                  = usize::from(br.read_u16be()?);
        for _ in 0..ntiles {
            let id                  = usize::from(br.read_u16be()?);
                                      br.read_buf(&mut self.tiles[id])?;
        }

        let bufret = self.hams.clone_ref();
        let mut buf;
        if let Some(bbuf) = bufret {
            buf = bbuf;
        } else {
            let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 2)?;
            buf = bufinfo.get_vbuf().unwrap();
            self.hams.add_frame(buf);
            buf = self.hams.get_output_frame().unwrap();
        }
        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();
        let stride = frm.stride[0];
        let doff = frm.offset[0];

        let mut is_intra = true;
        for row in frm.data[doff..].chunks_exact_mut(stride * 4).take(frm.height[0] / 4) {
            let mut x = 0;
            loop {
                let op = usize::from(br.read_u16be()?);
                match op {
                    0 => break,
                    1..=159 => {
                        is_intra = false;
                        x += op * 4;
                    },
                    160..=319 => {
                        let copy_size = (op - 159) * 4;
                        validate!(x + copy_size <= frm.width[0]);
                        for line in row.chunks_exact_mut(stride) {
                            for el in line[x..][..copy_size].iter_mut() {
                                *el = 0;
                            }
                        }
                        x += copy_size;
                    },
                    _ => {
                        validate!(x < frm.width[0]);
                        let tile_idx = op - 320;
                        for (dline, sline) in row.chunks_exact_mut(stride)
                                .zip(self.tiles[tile_idx].chunks_exact(4)) {
                            dline[x..][..4].copy_from_slice(sline);
                        }
                        x += 4;
                    },
                }
            }
        }

        let pal_off = frm.offset[1];
        let dpal = &mut frm.data[pal_off..];
        for sd in pkt.side_data.iter() {
            if let NASideData::Palette(_, ref pal) = sd {
                for (dst, src) in dpal.chunks_mut(3).zip(pal.chunks(4)) {
                    dst[0] = src[0];
                    dst[1] = src[1];
                    dst[2] = src[2];
                }
            }
        }

        let buftype = NABufferType::Video(buf);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buftype);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        self.hams.clear();
    }
}

impl NAOptionHandler for PixarDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}
#[derive(Default)]
struct PixarADecoder {
    info:       NACodecInfoRef,
    width:      usize,
    height:     usize,
}

impl PixarADecoder {
    fn new() -> Self { Self::default() }
}

impl NADecoder for PixarADecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = (vinfo.get_width()  + 3) & !3;
            self.height = (vinfo.get_height() + 3) & !3;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, GREY_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() >= 4);
        let mut br = MemoryReader::new_read(src.as_slice());

        let buftype = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 2)?;
        let mut buf = buftype.get_vbuf().unwrap();
        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();
        let stride = frm.stride[0];
        let doff = frm.offset[0];

        br.read_skip(2)?;

        while br.left() >= 2 {
            let size                    = u64::from(br.read_u16be()?);
            if size == 0 {
                break;
            }
            validate!(size > 12);
            let end = br.tell() + size - 2;
            validate!(size - 2 <= (br.left() as u64));
            let ftype                   = br.read_u16be()?;
            validate!(ftype == 0x4100);
            let top                     = usize::from(br.read_u16be()?);
            let left                    = usize::from(br.read_u16be()?);
            let bottom                  = usize::from(br.read_u16be()?);
            let right                   = usize::from(br.read_u16be()?);
            validate!(bottom > top && right > left);
            validate!(bottom <= self.height && right <= self.width);

            for line in frm.data[doff..].chunks_exact_mut(stride).take(bottom).skip(top) {
                let mut x = 0;
                loop {
                    let op = br.read_byte()?;
                    let len = usize::from(op & 0x3F) + 1;
                    validate!(x + len <= self.width);
                    match op {
                        0x00 => break,
                        0x01..=0x3F => {},
                        0x40..=0x7F | 0xC0..=0xFF => {
                            for pix in line[x..][..len].iter_mut() {
                                br.read_u16le()?;
                                *pix = 0xFF;
                            }
                        },
                        _ => {
                            br.read_buf(&mut line[x..][..len])?;
                        },
                    }
                    x += len;
                }
            }
            validate!(br.tell() == end);
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buftype);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for PixarADecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_0() -> Box<dyn NADecoder + Send> {
    Box::new(Pixar0Decoder::new())
}
pub fn get_decoder_3() -> Box<dyn NADecoder + Send> {
    Box::new(PixarDecoder::new(3))
}
pub fn get_decoder_4() -> Box<dyn NADecoder + Send> {
    Box::new(Pixar0Decoder::new())
}
pub fn get_decoder_5() -> Box<dyn NADecoder + Send> {
    Box::new(PixarDecoder::new(5))
}
pub fn get_decoder_a() -> Box<dyn NADecoder + Send> {
    Box::new(PixarADecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::misc_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    // samples from Toy Story game
    #[test]
    fn test_pixar_0() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "pixar-0", "assets/QT/tiplogo.m0", Some(1), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x9606ff61, 0xd359dd8a, 0xafa47809, 0x30e99eb9]]));
    }
    #[test]
    fn test_pixar_3() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "pixar-3", "assets/QT/tititle2.m3u", Some(6), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0xdef765d0, 0x9e5ece3d, 0xd8b5f5b1, 0x977bf647],
                            [0x93fe1182, 0x8846a90b, 0xbb6f6c2c, 0x6ad2d91a],
                            [0xc9cbcb9f, 0x69e270a4, 0xaf029a57, 0xf2607685],
                            [0x87cefdc3, 0x8479612e, 0x43f19f3e, 0x476685fa],
                            [0x439b57bf, 0xfb6f9b75, 0xe4a9c2fc, 0xdf021c7a],
                            [0x717638ab, 0xa310198f, 0x5b597402, 0xcec4cdd8],
                            [0x14cd0745, 0x7535856c, 0xf4102662, 0x3653062b]]));
    }
    #[test]
    fn test_pixar_4() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        test_decoding("mov-macbin", "pixar-4", "assets/QT/mmOpen.m4", Some(1), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x78aeadb9, 0xd4314dbd, 0x5e0dde85, 0xb1aff360]]));
    }
    #[test]
    fn test_pixar_5() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "pixar-5", "assets/QT/FRARMYA1.M5", Some(6), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0xe171c38d, 0xdfdc767a, 0x5594a451, 0xf6aa0748],
                            [0xee0bf633, 0xbd5411b0, 0x75a1ddff, 0xed3c929b],
                            [0x4b2069f3, 0x1532ead6, 0x2f836693, 0x868711a8],
                            [0x23e5764f, 0xc72b4827, 0x6baaa8ad, 0x0684228f],
                            [0xd6fa531e, 0xc51f4eae, 0xbf4c421a, 0x403c96ed],
                            [0xafd66217, 0xdc4ec112, 0xdf206f66, 0xba3d4415],
                            [0x6c529cc9, 0x81f1ed56, 0x80ababf2, 0x499dc64f]]));
    }
    #[test]
    fn test_pixar_a() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "pixar-a", "assets/QT/frtkew2.ma", Some(100), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x4222e315, 0x9a964380, 0x581f2ed0, 0xc7a22942],
                            [0x4222e315, 0x9a964380, 0x581f2ed0, 0xc7a22942],
                            [0x1becb994, 0x49236ec9, 0xcfe33239, 0x43cc501e]]));
    }
}
