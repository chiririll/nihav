use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

struct HybridReader<'a> {
    src:    &'a [u8],
    pos:    usize,
    bitbuf: u8,
    bits:   u8,
}

impl<'a> HybridReader<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self {
            src,
            pos: 1,
            bitbuf: if !src.is_empty() { src[0] } else { 0 },
            bits: 8,
        }
    }
    fn read_byte(&mut self) -> DecoderResult<u8> {
        if self.pos < self.src.len() {
            let ret = self.src[self.pos];
            self.pos += 1;
            Ok(ret)
        } else {
            Err(DecoderError::ShortData)
        }
    }
    fn read_bit(&mut self) -> DecoderResult<bool> {
        let ret = (self.bitbuf & 0x80) != 0;
        self.bitbuf <<= 1;
        self.bits    -= 1;
        if self.bits == 0 {
            self.bitbuf = self.read_byte()?;
            self.bits = 8;
        }
        Ok(ret)
    }
}

struct FrameDecoder<'a, 'b> {
    width:      usize,
    height:     usize,
    stride:     usize,
    dst:        &'a mut [u8],
    prev_frm:   &'a [u8],
    hr:         HybridReader<'b>,
}

impl<'a, 'b> FrameDecoder<'a, 'b> {
    fn new(dst: &'a mut [u8], prev_frm: &'a [u8], width: usize, height: usize, hr: HybridReader<'b>) -> Self {
        Self {
            width, height,
            stride: width,
            dst, prev_frm,
            hr,
        }
    }
    fn decode_intra1(&mut self, blk_size: usize) -> DecoderResult<()> {
        for y in (0..self.height).step_by(blk_size) {
            for x in (0..self.width).step_by(blk_size) {
                self.decode_block1(x, y, blk_size)?;
            }
        }
        Ok(())
    }
    fn decode_block1(&mut self, x: usize, y: usize, blk_size: usize) -> DecoderResult<()> {
        let cur_addr = x + y * self.stride;
        if blk_size == 1 {
            self.dst[cur_addr] = self.hr.read_byte()?;
            return Ok(());
        }
        if !self.hr.read_bit()? {
            let fill = self.hr.read_byte()?;
            for line in self.dst[cur_addr..].chunks_mut(self.stride).take(blk_size) {
                for el in line[..blk_size].iter_mut() {
                    *el = fill;
                }
            }
        } else {
            let hsize = blk_size >> 1;
            for n in 0..4 {
                self.decode_block1(x + (n & 1) * hsize, y + (n >> 1) * hsize, hsize)?;
            }
        }
        Ok(())
    }
    fn decode_inter2(&mut self, blk_size: usize) -> DecoderResult<()> {
        for y in (0..self.height).step_by(blk_size) {
            for x in (0..self.width).step_by(blk_size) {
                self.decode_block2(x, y, blk_size)?;
            }
        }
        Ok(())
    }
    fn decode_block2(&mut self, x: usize, y: usize, blk_size: usize) -> DecoderResult<()> {
        let cur_addr = x + y * self.stride;
        if blk_size == 1 {
            self.dst[cur_addr] = self.hr.read_byte()?;
            return Ok(());
        }
        if !self.hr.read_bit()? {
            if self.hr.read_bit()? {
                let fill = self.hr.read_byte()?;
                for line in self.dst[cur_addr..].chunks_mut(self.stride).take(blk_size) {
                    for el in line[..blk_size].iter_mut() {
                        *el = fill;
                    }
                }
            } else {
                for (dline, sline) in self.dst[cur_addr..].chunks_mut(self.stride)
                        .zip(self.prev_frm[cur_addr..].chunks(self.stride)).take(blk_size) {
                    dline[..blk_size].copy_from_slice(&sline[..blk_size]);
                }
            }
        } else {
            let hsize = blk_size >> 1;
            for n in 0..4 {
                self.decode_block2(x + (n & 1) * hsize, y + (n >> 1) * hsize, hsize)?;
            }
        }
        Ok(())
    }
    fn decode_intra3(&mut self, blk_size: usize) -> DecoderResult<()> {
        for y in (0..self.height).step_by(blk_size) {
            for x in (0..self.width).step_by(blk_size) {
                self.decode_block3(x, y, blk_size, true)?;
            }
        }
        Ok(())
    }
    fn decode_block3(&mut self, x: usize, y: usize, blk_size: usize, top: bool) -> DecoderResult<()> {
        let mut cur_addr = x + y * self.stride;
        if blk_size == 1 {
            self.dst[cur_addr] = self.hr.read_byte()?;
            return Ok(());
        }
        if !self.hr.read_bit()? {
            if top || !self.hr.read_bit()? {
                let fill = self.hr.read_byte()?;
                for line in self.dst[cur_addr..].chunks_mut(self.stride).take(blk_size) {
                    for el in line[..blk_size].iter_mut() {
                        *el = fill;
                    }
                }
            } else {
                let mv = self.hr.read_byte()? as usize;
                let offset = (mv & 0xF) + (mv >> 4) * self.stride;
                validate!(offset <= cur_addr);
                for _ in 0..blk_size {
                    for x in 0..blk_size {
                        self.dst[cur_addr + x] = self.dst[cur_addr + x - offset];
                    }
                    cur_addr += self.stride;
                }
            }
        } else {
            let hsize = blk_size >> 1;
            for n in 0..4 {
                self.decode_block3(x + (n & 1) * hsize, y + (n >> 1) * hsize, hsize, false)?;
            }
        }
        Ok(())
    }
    fn decode_inter4(&mut self, blk_size: usize) -> DecoderResult<()> {
        for y in (0..self.height).step_by(blk_size) {
            for x in (0..self.width).step_by(blk_size) {
                self.decode_block4(x, y, blk_size, true)?;
            }
        }
        Ok(())
    }
    fn decode_block4(&mut self, x: usize, y: usize, blk_size: usize, top: bool) -> DecoderResult<()> {
        let cur_addr = x + y * self.stride;
        if blk_size == 1 {
            self.dst[cur_addr] = self.hr.read_byte()?;
            return Ok(());
        }
        if !self.hr.read_bit()? {
            if !self.hr.read_bit()? {
                let fill = self.hr.read_byte()?;
                for line in self.dst[cur_addr..].chunks_mut(self.stride).take(blk_size) {
                    for el in line[..blk_size].iter_mut() {
                        *el = fill;
                    }
                }
            } else if top {
                for (dline, sline) in self.dst[cur_addr..].chunks_mut(self.stride)
                        .zip(self.prev_frm[cur_addr..].chunks(self.stride)).take(blk_size) {
                    dline[..blk_size].copy_from_slice(&sline[..blk_size]);
                }
            } else {
                let mv = self.hr.read_byte()? as usize;
                let mv_x = mv & 0xF;
                let mv_y = mv >> 4;
                validate!(x + mv_x >= 8 && x + mv_x - 8 + blk_size <= self.width);
                validate!(y + mv_y >= 8 && y + mv_y - 8 + blk_size <= self.height);
                let ref_addr = cur_addr + mv_x - 8 + mv_y * self.stride - 8 * self.stride;
                for (dline, sline) in self.dst[cur_addr..].chunks_mut(self.stride)
                        .zip(self.prev_frm[ref_addr..].chunks(self.stride)).take(blk_size) {
                    dline[..blk_size].copy_from_slice(&sline[..blk_size]);
                }
            }
        } else {
            let hsize = blk_size >> 1;
            for n in 0..4 {
                self.decode_block4(x + (n & 1) * hsize, y + (n >> 1) * hsize, hsize, false)?;
            }
        }
        Ok(())
    }
}

struct KMVCDecoder {
    info:       NACodecInfoRef,
    pal:        [u8; 768],
    palsize:    usize,
    frame:      Vec<u8>,
    pframe:     Vec<u8>,
    width:      usize,
    height:     usize,
    firstpal:   bool,
}

impl KMVCDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            pal:        [0; 768],
            palsize:    127,
            frame:      Vec::new(),
            pframe:     Vec::new(),
            width:      0,
            height:     0,
            firstpal:   false,
        }
    }
}

impl NADecoder for KMVCDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.width;
            self.height = vinfo.height;
            self.frame  = vec![0; self.width * self.height];
            self.pframe = vec![0; self.width * self.height];
            self.pal    = [0; 768];
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            if let Some(edata) = info.get_extradata() {
                if edata.len() >= 12 {
                    let palsize = read_u16le(&edata[10..12])? as usize;
                    validate!(palsize > 0 && palsize < 256);
                    self.palsize = palsize;
                }
                if edata.len() == 1036 {
                    for (dst, src) in self.pal.chunks_exact_mut(3).zip(edata[12..].chunks_exact(4)) {
                        dst[0] = src[2];
                        dst[1] = src[1];
                        dst[2] = src[0];
                    }
                    self.firstpal = true;
                }
            }

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() >= 2);

        for sd in pkt.side_data.iter() {
            if let NASideData::Palette(true, ref pal) = sd {
                if !self.firstpal {
                    for (dst, src) in self.pal.chunks_exact_mut(3).zip(pal.chunks_exact(4)) {
                        dst[0] = src[0];
                        dst[1] = src[1];
                        dst[2] = src[2];
                    }
                } else {
                    self.firstpal = false;
                }
                break;
            }
        }

        let mut br = MemoryReader::new_read(&src);

        let header = br.read_byte()?;
        let method = header & 0xF;
        if (header & 0x80) != 0 && method < 3 {
            let start_clr = br.read_byte()? as usize;
            let nclrs     = br.read_byte()? as usize;
            validate!(start_clr + nclrs <= 256);
            br.read_buf(&mut self.pal[start_clr * 3..][..nclrs * 3])?;
        }
        if (header & 0x40) != 0 {
            br.read_buf(&mut self.pal[3..][..self.palsize * 3])?;
        }
        let blk_size = br.read_byte()? as usize;
        validate!(blk_size > 0 && (blk_size & (blk_size - 1)) == 0);
        validate!(self.width % blk_size == 0);

        let hr = HybridReader::new(&src[br.tell() as usize..]);
        std::mem::swap(&mut self.frame, &mut self.pframe);
        let mut dec = FrameDecoder::new(&mut self.frame, &self.pframe, self.width, self.height, hr);
        match method {
            1 => dec.decode_intra1(blk_size)?,
            2 => dec.decode_inter2(blk_size)?,
            3 => dec.decode_intra3(blk_size)?,
            4 => dec.decode_inter4(blk_size)?,
            _ => return Err(DecoderError::NotImplemented),
        }

        let buf = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;
        let mut vbuf = buf.get_vbuf().unwrap();
        let paloff = vbuf.get_offset(1);
        let stride = vbuf.get_stride(0);
        let data = vbuf.get_data_mut().unwrap();

        for (drow, srow) in data.chunks_mut(stride).zip(self.frame.chunks(self.width)) {
            drow[..self.width].copy_from_slice(srow);
        }
        data[paloff..][..768].copy_from_slice(&self.pal);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buf);
        let ftype = if (header & 0x80) != 0 { FrameType::I } else { FrameType::P };
        frm.set_frame_type(ftype);
        frm.set_keyframe(ftype == FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for KMVCDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(KMVCDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::game_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_kmvc_0() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from Alien Breed (PC version)
        test_decoding("avi", "kmvc", "assets/Game/OUTRO.ANM",
                      Some(12), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x2b2cd2eb, 0x02c7d77e, 0xbf70ce2e, 0x8bad43b0],
                            [0x1a12d41f, 0x3309992e, 0xba018966, 0xa6349e19],
                            [0xe8793445, 0x0f3eb58a, 0x5d218de3, 0xe45f0a23],
                            [0xf42c5018, 0x98cd93c7, 0x01f2c546, 0xb9a825ce],
                            [0x7c01159b, 0x4688930a, 0x00d82ebb, 0x29db5b96],
                            [0xb3b659ac, 0xe16138a4, 0xc45369d4, 0xc139db89],
                            [0xdd024a89, 0x58063f17, 0xc50b8975, 0x3e0465d7],
                            [0x842f6bcf, 0xf0f6b4ac, 0xda0d2653, 0xa858d7ec],
                            [0xa6aec7ba, 0x2c234945, 0x8a04525f, 0x5f6a4ab7],
                            [0x7b6345fe, 0x0ab6f159, 0x438307f6, 0xf442ad08],
                            [0x89a7ed16, 0x4e2cfcea, 0xb75a002d, 0xbac95a16],
                            [0x6b18e35a, 0xe96bd87c, 0x071e5bd9, 0x77c19d1c],
                            [0xc7e3e93f, 0xaffeb4f8, 0x2908e8ed, 0x9f3e4ce9]]));
    }
    #[test]
    fn test_kmvc_1() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from https://samples.mplayerhq.hu/V-codecs/KMVC/FLAME.AVI
        test_decoding("avi", "kmvc", "assets/Game/FLAME.AVI",
                      Some(12), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0xf0ad790f, 0x7e3a57d0, 0xee9282c1, 0x15547d92],
                            [0x3a6b2a58, 0x6eaf7960, 0x9d4ff92e, 0x1833677a],
                            [0xfe29ef44, 0x53ad9d6b, 0x0f90644e, 0x2e063cc0],
                            [0x2888fc3e, 0x3931e3c5, 0xf152a8ac, 0xde4bfc6f],
                            [0xc1fb2980, 0x7921d476, 0xd9946a83, 0xed437fdb],
                            [0x8fcca880, 0x884260e9, 0x2f147ae1, 0x1ac3a77d],
                            [0xcb93573f, 0xb9f00b90, 0xac09cbd6, 0xb65e852b],
                            [0x293d955c, 0x15a0ec08, 0x2decbeeb, 0xb00a6845],
                            [0x7b75471a, 0x0e720f1a, 0x679b3b76, 0x6f0acfa2],
                            [0xcd4cef26, 0x9315612f, 0x6f7066a5, 0x9e931046],
                            [0x8eaced5f, 0xa1e04658, 0xbb4784eb, 0xe5e64c93],
                            [0x54a1e90d, 0xe9db5d38, 0x247ff005, 0x3f9c8e2b],
                            [0xa65668a6, 0x581883d1, 0xaca638cc, 0x8e459a9f]]));
    }
    #[test]
    fn test_kmvc_2() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from https://samples.mplayerhq.hu/V-codecs/KMVC/baseball1.avi
        test_decoding("avi", "kmvc", "assets/Game/baseball1.avi",
                      Some(24), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0xf8778594, 0xd044befd, 0x054f3dcd, 0x72178a7b],
                            [0x5aefe81d, 0xa193c130, 0x960ffb88, 0x576405b2],
                            [0x95db7747, 0xdf4d34d7, 0x959c5c67, 0x8cf55708],
                            [0xcd000e61, 0xd5c852f0, 0xe97ea320, 0x7465ddc7],
                            [0xddcc347d, 0xb87eff08, 0xe6b5e9ab, 0xbc2e2f72],
                            [0x98f0f17f, 0x30b5ea8c, 0xb6a2eddc, 0x2f15fff6],
                            [0xe2fa257a, 0x2a24eda0, 0x0fe85644, 0x7b796130],
                            [0x96521427, 0x5a9f9e12, 0x7b1ffeff, 0x4b94f0c3],
                            [0x5d72afa7, 0x954e8404, 0x5598f536, 0x63e60d08],
                            [0xce3fc2df, 0x98b067a2, 0xf85c597f, 0x45797ff5],
                            [0x9f86d7bd, 0x3cac2a08, 0xdd00788e, 0x5d8a4731],
                            [0x6f0b9bda, 0x0613670c, 0xe9f3f75d, 0x0ef112c9],
                            [0x549ef3ee, 0xb7c582b7, 0xf995b7fe, 0x62140120],
                            [0xcd29a18d, 0x0c51e581, 0x3dc8e1eb, 0x3dea5759],
                            [0xb8bd405c, 0xd674557f, 0x9d783ccc, 0x75893613],
                            [0x922084d6, 0xc36fdcfc, 0x46bfe658, 0x7bb5922a],
                            [0xc237a20b, 0xbaacd94f, 0xc09a1665, 0xd988132d]]));
    }
}
