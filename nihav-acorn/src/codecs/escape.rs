use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_core::io::bitreader::*;
use nihav_codec_support::codecs::imaadpcm::*;
use std::convert::TryFrom;
use std::str::FromStr;
use super::RGB555_FORMAT;
use super::yuvtab::YUV2RGB;

const BGR555_FORMAT: NAPixelFormaton = NAPixelFormaton { model: ColorModel::RGB(RGBSubmodel::RGB), components: 3,
                                        comp_info: [
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift: 10, comp_offs: 0, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  5, comp_offs: 0, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  0, comp_offs: 0, next_elem: 2 }),
                                            None, None],
                                        elem_size: 2, be: false, alpha: false, palette: false };

trait ReadECode {
    fn read_ecode(&mut self) -> DecoderResult<usize>;
}

impl<'a> ReadECode for BitReader<'a> {
    fn read_ecode(&mut self) -> DecoderResult<usize> {
        if self.read_bool()? {
            let val3 = self.read(3)? as usize;
            if val3 == 7 {
                let val7 = self.read(7)? as usize;
                if val7 == 127 {
                    let val12 = self.read(12)? as usize;
                    Ok(val12 + 1 + 7 + 127)
                } else {
                    Ok(val7 + 1 + 7)
                }
            } else {
                Ok(val3 + 1)
            }
        } else {
            Ok(0)
        }
    }
}

struct Escape102Decoder {
    info:           NACodecInfoRef,
    frame:          Vec<u16>,
    width:          usize,
}

impl Escape102Decoder {
    fn new() -> Self {
        Self {
            info:   NACodecInfoRef::default(),
            frame:  Vec::new(),
            width:  0,
        }
    }
    fn read_skip(br: &mut BitReader) -> DecoderResult<usize> {
        if br.read_bool()? {
            let val3 = br.read(3)? as usize;
            if val3 == 7 {
                let val7 = br.read(7)? as usize;
                if val7 == 127 {
                    let val15 = br.read(15)? as usize;
                    Ok(val15 + 1 + 7 + 127)
                } else {
                    Ok(val7 + 1 + 7)
                }
            } else {
                Ok(val3 + 1)
            }
        } else {
            Ok(0)
        }
    }
    fn read_chroma(br: &mut BitReader) -> DecoderResult<u16> {
        let mut idx = br.read(6)? as usize;
        if idx > 0x30 {
            let hibits = br.read(2)?;
            idx += (hibits << 6) as usize;
        }
        Ok(E102_CHROMA_TAB[idx])
    }
}

impl NADecoder for Escape102Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, RGB555_FORMAT));
            validate!((vinfo.get_width() | vinfo.get_height()) & 1 == 0);
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            self.frame = vec![0; vinfo.get_width() * vinfo.get_height()];
            self.width = vinfo.get_width();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 8);
        let mut mr = MemoryReader::new_read(&src);
        let mut br = ByteReader::new(&mut mr);
        let codec_id = br.read_u32le()?;
        validate!(codec_id == 0x102);
        let vsize = br.read_u32le()? as usize;
        validate!(src.len() >= vsize);

        let mut is_intra = true;
        let mut br = BitReader::new(&src[8..], BitReaderMode::LE);
        let mut skip = 0;
        let mut new_skip = false;
        for strip in self.frame.chunks_exact_mut(self.width * 2) {
            for x in (0..self.width).step_by(2) {
                if !new_skip {
                    skip = Self::read_skip(&mut br)?;
                    new_skip = true;
                }
                if skip > 0 {
                    skip -= 1;
                    is_intra = false;
                    continue;
                }

                if !br.read_bool()? {
                    let chr = Self::read_chroma(&mut br)?;
                    for i in 0..4 {
                        strip[x + (i & 1) + (i >> 1) * self.width] &= 0x1F;
                        strip[x + (i & 1) + (i >> 1) * self.width] |= chr;
                    }
                } else {
                    let pattern = br.read(3)?;
                    let y0 = br.read(5)? as u16;
                    let y1 = if pattern == 0 { 0 } else { br.read(5)? as u16 };
                    let chr = if !br.read_bool()? {
                            strip[x] & !0x1F
                        } else {
                            Self::read_chroma(&mut br)?
                        };
                    let clr0 = y0 | chr;
                    let clr1 = y1 | chr;
                    strip[x]                  = clr0;
                    strip[x + 1]              = if (pattern & 1) == 0 { clr0 } else { clr1 };
                    strip[x     + self.width] = if (pattern & 2) == 0 { clr0 } else { clr1 };
                    strip[x + 1 + self.width] = if (pattern & 4) == 0 { clr0 } else { clr1 };
                }
                new_skip = false;
            }
        }

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;
        let mut buf = bufinfo.get_vbuf16().unwrap();
        let stride = buf.get_stride(0);
        let data = buf.get_data_mut().unwrap();

        for (dline, sline) in data.chunks_exact_mut(stride)
                    .zip(self.frame.chunks_exact(self.width)) {
            for (dst, &src) in dline.iter_mut().zip(sline.iter()) {
                *dst = YUV2RGB[src as usize];
            }
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.frame.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for Escape102Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

struct Escape122Decoder {
    info:           NACodecInfoRef,
    frame:          Vec<u8>,
    width:          usize,
    pal:            [u8; 768],
}

impl Escape122Decoder {
    fn new() -> Self {
        Self {
            info:   NACodecInfoRef::default(),
            frame:  Vec::new(),
            width:  0,
            pal:    [0; 768],
        }
    }
    fn read_blk2x2(br: &mut BitReader) -> DecoderResult<[u8; 4]> {
        let mask = br.read(4)? as usize;
        match mask {
            0x0 => {
                let idx = br.read(7)? as u8;
                Ok([idx * 2; 4])
            },
            0xF => {
                let idx = br.read(7)? as u8;
                Ok([idx * 2 + 1; 4])
            },
            _ => {
                let clr0 = br.read(8)? as u8;
                let clr1 = br.read(8)? as u8;
                let clrs = [clr0, clr1];
                Ok([clrs[mask & 1], clrs[(mask >> 1) & 1], clrs[(mask >> 2) & 1], clrs[mask >> 3]])
            }
        }
    }
}

impl NADecoder for Escape122Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, PAL8_FORMAT));
            validate!((vinfo.get_width() | vinfo.get_height()) & 7 == 0);
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            self.frame = vec![0; vinfo.get_width() * vinfo.get_height()];
            self.width = vinfo.get_width();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 8);
        let mut mr = MemoryReader::new_read(&src);
        let mut br = ByteReader::new(&mut mr);
        let codec_id = br.read_u32le()?;
        validate!(codec_id == 0x100 + 22);
        let vsize = br.read_u32le()? as usize;
        validate!(src.len() >= vsize);
        let pal_size = br.read_u16le()? as usize;
        let nentries = (pal_size / 3).min(256);
        if nentries > 0 {
            br.read_buf(&mut self.pal[..nentries * 3])?;
            for el in self.pal[..nentries * 3].iter_mut() {
                *el = (*el << 2) | (*el >> 4);
            }
        }
        br.read_skip(pal_size - nentries * 3)?;
        let frm_start = br.tell() as usize;

        let mut is_intra = true;
        let mut br = BitReader::new(&src[frm_start..], BitReaderMode::LE);
        let mut skip = 0;
        let mut new_skip = false;
        let mut offsets = [0; 16];
        for (i, dst) in offsets.iter_mut().enumerate() {
            *dst = (i & 3) * 2 + (i >> 2) * 2 * self.width;
        }
        for strip in self.frame.chunks_exact_mut(self.width * 8) {
            for x in (0..self.width).step_by(8) {
                if !new_skip {
                    skip = br.read_ecode()?;
                    new_skip = true;
                }
                if skip > 0 {
                    skip -= 1;
                    is_intra = false;
                    continue;
                }

                while !br.read_bool()? {
                    let blk = Self::read_blk2x2(&mut br)?;
                    let mut mask = br.read(16)?;
                    for &offset in offsets.iter() {
                        if (mask & 1) != 0 {
                            strip[offset + x] = blk[0];
                            strip[offset + x + 1] = blk[1];
                            strip[offset + x + self.width] = blk[2];
                            strip[offset + x + self.width + 1] = blk[3];
                        }
                        mask >>= 1;
                    }
                }

                if !br.read_bool()? {
                    let mut mask = br.read(16)?;
                    for &offset in offsets.iter() {
                        if (mask & 1) != 0 {
                            let blk = Self::read_blk2x2(&mut br)?;
                            strip[offset + x] = blk[0];
                            strip[offset + x + 1] = blk[1];
                            strip[offset + x + self.width] = blk[2];
                            strip[offset + x + self.width + 1] = blk[3];
                        }
                        mask >>= 1;
                    }
                }
                new_skip = false;
            }
        }

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 3)?;
        let mut buf = bufinfo.get_vbuf().unwrap();
        let stride = buf.get_stride(0);
        let paloff = buf.get_offset(1);
        let data = buf.get_data_mut().unwrap();

        for (dline, sline) in data.chunks_exact_mut(stride)
                    .zip(self.frame.chunks_exact(self.width)) {
            dline[..self.width].copy_from_slice(sline);
        }
        data[paloff..][..768].copy_from_slice(&self.pal);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.frame.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for Escape122Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

#[derive(Default)]
struct E124Codebook {
    entries:    Vec<[u16; 4]>,
    bits:       u8,
}

impl E124Codebook {
    fn read(&mut self, br: &mut BitReader, size: usize, bits: u8) -> DecoderResult<()> {
        self.entries.clear();
        self.entries.reserve(size);
        self.bits = bits;
        for _ in 0..size {
            let mask = br.read(4)? as usize;
            let clr0 = br.read(15)? as u16;
            let clr1 = br.read(15)? as u16;
            let clrs = [clr0, clr1];
            self.entries.push([clrs[mask & 1], clrs[(mask >> 1) & 1], clrs[(mask >> 2) & 1], clrs[mask >> 3]]);
        }
        Ok(())
    }
}

struct Escape124Decoder {
    info:           NACodecInfoRef,
    frame:          Vec<u16>,
    width:          usize,
    num_tiles:      usize,
    codebook:       [E124Codebook; 3],
}

impl Escape124Decoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            frame:      Vec::new(),
            width:      0,
            num_tiles:  0,
            codebook:   [E124Codebook::default(), E124Codebook::default(), E124Codebook::default()],
        }
    }
    fn read_block(br: &mut BitReader, cb_idx: &mut usize, codebook: &[E124Codebook; 3], blk_idx: usize) -> DecoderResult<[u16; 4]> {
        if br.read_bool()? {
            const NEXT_INDEX: [[usize; 2]; 3] = [[2, 1], [0, 2], [1, 0]];
            *cb_idx = NEXT_INDEX[*cb_idx][br.read(1)? as usize];
        }

        let mut idx = br.read(codebook[*cb_idx].bits)? as usize;
        if *cb_idx == 1 {
            idx += blk_idx << codebook[1].bits;
        }
        validate!(idx < codebook[*cb_idx].entries.len());
        Ok(codebook[*cb_idx].entries[idx])
    }
}

impl NADecoder for Escape124Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, BGR555_FORMAT));
            validate!((vinfo.get_width() | vinfo.get_height()) & 7 == 0);
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            self.frame = vec![0; vinfo.get_width() * vinfo.get_height()];
            self.width = vinfo.get_width();
            self.num_tiles = (vinfo.get_width() / 8) * (vinfo.get_height() / 8);
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 8);
        validate!(src[0] == 20 && src[1] == 1);
        let flags = read_u16le(&src[2..])?;
        let vsize = read_u32le(&src[4..])? as usize;
        validate!(src.len() >= vsize);

        let mut br = BitReader::new(&src[8..], BitReaderMode::LE);

        let mut is_intra = true;
        if (flags & 0x2) != 0 {
            let bits = br.read(4)? as u8;
            self.codebook[0].read(&mut br, 1 << bits, bits)?;
        }
        if (flags & 0x4) != 0 {
            let bits = br.read(4)? as u8;
            self.codebook[1].read(&mut br, self.num_tiles << bits, bits)?;
        }
        if (flags & 0x8) != 0 {
            let cb_size = br.read(20)? as usize;
            if cb_size > 0 {
                let bits = 32 - (cb_size as u32 - 1).leading_zeros();
                self.codebook[2].read(&mut br, cb_size, bits as u8)?;
            } else {
                self.codebook[2].bits = 0;
                self.codebook[2].entries.clear();
            }
        }

        const MASKS: [u32; 16] = [
            0x0001, 0x0002, 0x0010, 0x0020,
            0x0004, 0x0008, 0x0040, 0x0080,
            0x0100, 0x0200, 0x1000, 0x2000,
            0x0400, 0x0800, 0x4000, 0x8000
        ];
        let mut skip = 0;
        let mut new_skip = false;
        let mut offsets = [0; 16];
        for (i, dst) in offsets.iter_mut().enumerate() {
            *dst = (i & 3) * 2 + (i >> 2) * 2 * self.width;
        }
        let mut cb_idx = 1;
        let mut blk_idx = 0;
        for strip in self.frame.chunks_exact_mut(self.width * 8) {
            for x in (0..self.width).step_by(8) {
                blk_idx += 1;
                if !new_skip {
                    skip = br.read_ecode()?;
                    new_skip = true;
                }
                if skip > 0 {
                    skip -= 1;
                    is_intra = false;
                    continue;
                }

                let mut full_cbp = 0;
                while !br.read_bool()? {
                    let blk = Self::read_block(&mut br, &mut cb_idx, &self.codebook, blk_idx - 1)?;
                    let mask = br.read(16)?;
                    full_cbp |= mask;
                    for (&offset, &ref_mask) in offsets.iter().zip(MASKS.iter()) {
                        if (mask & ref_mask) != 0 {
                            strip[offset + x] = blk[0];
                            strip[offset + x + 1] = blk[1];
                            strip[offset + x + self.width] = blk[2];
                            strip[offset + x + self.width + 1] = blk[3];
                        }
                    }
                }

                if !br.read_bool()? {
                    let mut mask = br.read(4)?;
                    for i in 0..4 {
                        let seg = if (mask & 1) != 0 { 0xF } else { br.read(4)? };
                        full_cbp ^= seg << (i * 4);
                        mask >>= 1;
                    }
                    for (&offset, &ref_mask) in offsets.iter().zip(MASKS.iter()) {
                        if (full_cbp & ref_mask) != 0 {
                            let blk = Self::read_block(&mut br, &mut cb_idx, &self.codebook, blk_idx - 1)?;
                            strip[offset + x] = blk[0];
                            strip[offset + x + 1] = blk[1];
                            strip[offset + x + self.width] = blk[2];
                            strip[offset + x + self.width + 1] = blk[3];
                        }
                    }
                } else if (flags & 1) != 0 {
                    while !br.read_bool()? {
                        let blk = Self::read_block(&mut br, &mut cb_idx, &self.codebook, blk_idx - 1)?;
                        let offset = offsets[br.read(4)? as usize];
                        strip[offset + x] = blk[0];
                        strip[offset + x + 1] = blk[1];
                        strip[offset + x + self.width] = blk[2];
                        strip[offset + x + self.width + 1] = blk[3];
                    }
                }

                new_skip = false;
            }
        }

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 3)?;
        let mut buf = bufinfo.get_vbuf16().unwrap();
        let stride = buf.get_stride(0);
        let data = buf.get_data_mut().unwrap();

        for (dline, sline) in data.chunks_exact_mut(stride).zip(self.frame.chunks_exact(self.width)) {
            dline[..self.width].copy_from_slice(sline);
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.frame.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for Escape124Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

#[derive(Clone,Copy)]
struct E130Block {
    y:      [u8; 4],
    y_avg:  u8,
    cb:     u8,
    cr:     u8,
}

impl Default for E130Block {
    fn default() -> Self {
        Self {
            y:      [0; 4],
            y_avg:  0,
            cb:     0x10,
            cr:     0x10,
        }
    }
}

#[derive(Default)]
struct Escape130Decoder {
    info:           NACodecInfoRef,
    width:          usize,
    blocks:         Vec<E130Block>,
}

impl Escape130Decoder {
    fn new() -> Self { Self::default() }
    fn read_skip(br: &mut BitReader) -> DecoderResult<usize> {
        if br.read_bool()? {
            Ok(0)
        } else {
            let val3 = br.read(3)? as usize;
            if val3 != 0 {
                Ok(val3)
            } else {
                let val8 = br.read(8)? as usize;
                if val8 != 0 {
                    Ok(val8 + 7)
                } else {
                    let val15 = br.read(15)? as usize;
                    Ok(val15 + 7 + 255)
                }
            }
        }
    }
}

impl NADecoder for Escape130Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, YUV420_FORMAT));
            validate!((vinfo.get_width() | vinfo.get_height()) & 7 == 0);
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            self.width  = vinfo.get_width();
            self.blocks = vec![E130Block::default(); vinfo.get_width() * vinfo.get_height() / 4];
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 16);
        validate!(src[0] == 0x30 && src[1] == 1);
        let vsize = read_u32le(&src[4..])? as usize;
        validate!(src.len() >= vsize);

        let mut br = BitReader::new(&src[16..], BitReaderMode::LE);

        let mut is_intra = true;
        let mut blk = E130Block::default();
        let mut blk_pos = 0;
        while blk_pos < self.blocks.len() {
            let skip = Self::read_skip(&mut br)?;
            validate!(blk_pos + skip <= self.blocks.len());
            blk_pos += skip;
            if skip > 0 {
                if blk_pos >= self.blocks.len() {
                    break;
                }
                blk = self.blocks[blk_pos - 1];
                is_intra = false;
            }

            if br.read_bool()? {
                const Y_STEPS: [i16; 4] = [2, 4, 10, 20];
                let sign_idx = br.read(6)? as usize;
                let step_idx = br.read(2)? as usize;
                blk.y_avg = br.read(5)? as u8 * 2;
                for (dst, &sign) in blk.y.iter_mut().zip(E130_Y_SIGNS[sign_idx].iter()) {
                    *dst = (i16::from(blk.y_avg) + i16::from(sign) * Y_STEPS[step_idx]).max(0).min(0x3F) as u8;
                }
            } else if br.read_bool()? {
                blk.y_avg = if br.read_bool()? {
                        br.read(6)? as u8
                    } else {
                        const Y_DIFF: [i8; 8] = [ -4, -3, -2, -1, 1, 2, 3, 4 ];
                        let diff_idx = br.read(3)? as usize;
                        blk.y_avg.wrapping_add(Y_DIFF[diff_idx] as u8) & 0x3F
                    };
                blk.y = [blk.y_avg; 4];
            }

            if br.read_bool()? {
                if br.read_bool()? {
                    blk.cb = br.read(5)? as u8;
                    blk.cr = br.read(5)? as u8;
                } else {
                    const CB_DIFF: [i8; 8] = [ 1, 1, 0, -1, -1, -1,  0,  1 ];
                    const CR_DIFF: [i8; 8] = [ 0, 1, 1,  1,  0, -1, -1, -1 ];
                    let delta_idx = br.read(3)? as usize;
                    blk.cb = blk.cb.wrapping_add(CB_DIFF[delta_idx] as u8) & 0x1F;
                    blk.cr = blk.cr.wrapping_add(CR_DIFF[delta_idx] as u8) & 0x1F;
                }
            }

            self.blocks[blk_pos] = blk;
            blk_pos += 1;
        }

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 3)?;
        let mut buf = bufinfo.get_vbuf().unwrap();
        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();

        let mut yoff = frm.offset[0];
        let mut cboff = frm.offset[1];
        let mut croff = frm.offset[2];
        for row in self.blocks.chunks(self.width / 2) {
            for (x, blk) in row.iter().enumerate() {
                frm.data[yoff + x * 2] = blk.y[0] << 2;
                frm.data[yoff + x * 2 + 1] = blk.y[1] << 2;
                frm.data[yoff + x * 2 + frm.stride[0]] = blk.y[2] << 2;
                frm.data[yoff + x * 2 + frm.stride[0] + 1] = blk.y[3] << 2;
                frm.data[cboff + x] = CHROMA_VALUES[usize::from(blk.cb) & 0x1F];
                frm.data[croff + x] = CHROMA_VALUES[usize::from(blk.cr) & 0x1F];
            }
            yoff  += frm.stride[0] * 2;
            cboff += frm.stride[1];
            croff += frm.stride[2];
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.blocks.iter_mut() {
            *el = E130Block::default();
        }
    }
}

impl NAOptionHandler for Escape130Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder102() -> Box<dyn NADecoder + Send> {
    Box::new(Escape102Decoder::new())
}
pub fn get_decoder122() -> Box<dyn NADecoder + Send> {
    Box::new(Escape122Decoder::new())
}
pub fn get_decoder124() -> Box<dyn NADecoder + Send> {
    Box::new(Escape124Decoder::new())
}
pub fn get_decoder130() -> Box<dyn NADecoder + Send> {
    Box::new(Escape130Decoder::new())
}

#[derive(Debug,Copy,Clone,PartialEq)]
enum EscapeCodec {
    Escape102,
    Escape122,
    Escape124,
    Escape130,
}

impl TryFrom<u16> for EscapeCodec {
    type Error = ();
    fn try_from(val: u16) -> Result<EscapeCodec, Self::Error> {
        match val {
            0x102 => Ok(EscapeCodec::Escape102),
            0x116 => Ok(EscapeCodec::Escape122),
            0x114 => Ok(EscapeCodec::Escape124),
            0x130 => Ok(EscapeCodec::Escape130),
            _ => Err(())
        }
    }
}

#[derive(Default)]
#[allow(clippy::large_enum_variant)]
enum EscapeDecoderVariant {
    #[default]
    None,
    Escape102(Escape102Decoder),
    Escape122(Escape122Decoder),
    Escape124(Escape124Decoder),
    Escape130(Escape130Decoder),
}

#[derive(Default)]
struct EscapeAnyDecoder {
    info:           NACodecInfoRef,
    decoder:        EscapeDecoderVariant,
}

impl NADecoder for EscapeAnyDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, YUV420_FORMAT));
            validate!((vinfo.get_width() | vinfo.get_height()) & 7 == 0);
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 16);

        let id = read_u16le(&src).unwrap_or_default();
        let etype = EscapeCodec::try_from(id).map_err(|_| DecoderError::InvalidData)?;

        if matches!(self.decoder, EscapeDecoderVariant::None) {
            match etype {
                EscapeCodec::Escape102 => {
                    let mut dec = Escape102Decoder::new();
                    dec.init(supp, self.info.clone())?;
                    self.decoder = EscapeDecoderVariant::Escape102(dec);
                },
                EscapeCodec::Escape122 => {
                    let mut dec = Escape122Decoder::new();
                    dec.init(supp, self.info.clone())?;
                    self.decoder = EscapeDecoderVariant::Escape122(dec);
                },
                EscapeCodec::Escape124 => {
                    let mut dec = Escape124Decoder::new();
                    dec.init(supp, self.info.clone())?;
                    self.decoder = EscapeDecoderVariant::Escape124(dec);
                },
                EscapeCodec::Escape130 => {
                    let mut dec = Escape130Decoder::new();
                    dec.init(supp, self.info.clone())?;
                    self.decoder = EscapeDecoderVariant::Escape130(dec);
                },
            }
        }

        match (etype, &mut self.decoder) {
            (EscapeCodec::Escape102, EscapeDecoderVariant::Escape102(ref mut dec)) => {
                dec.decode(supp, pkt)
            },
            (EscapeCodec::Escape122, EscapeDecoderVariant::Escape122(ref mut dec)) => {
                dec.decode(supp, pkt)
            },
            (EscapeCodec::Escape124, EscapeDecoderVariant::Escape124(ref mut dec)) => {
                dec.decode(supp, pkt)
            },
            (EscapeCodec::Escape130, EscapeDecoderVariant::Escape130(ref mut dec)) => {
                dec.decode(supp, pkt)
            },
            _ => Err(DecoderError::InvalidData),
        }
    }
    fn flush(&mut self) {
        match self.decoder {
            EscapeDecoderVariant::None => {},
            EscapeDecoderVariant::Escape102(ref mut dec) => { dec.flush(); },
            EscapeDecoderVariant::Escape122(ref mut dec) => { dec.flush(); },
            EscapeDecoderVariant::Escape124(ref mut dec) => { dec.flush(); },
            EscapeDecoderVariant::Escape130(ref mut dec) => { dec.flush(); },
        }
    }
}

impl NAOptionHandler for EscapeAnyDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_any() -> Box<dyn NADecoder + Send> {
    Box::<EscapeAnyDecoder>::default()
}

pub struct EscapeIMAState {
    pub predictor:  i32,
    pub step:       usize,
}

impl EscapeIMAState {
    pub fn new() -> Self {
        Self {
            predictor:  0,
            step:       0,
        }
    }
    pub fn reset(&mut self, predictor: i16, step: u8) {
        self.predictor  = i32::from(predictor);
        self.step       = step.min(IMA_MAX_STEP) as usize;
    }
    pub fn expand_sample(&mut self, nibble: u8) -> i16 {
        let istep = (self.step as isize) + (IMA_STEPS[(nibble & 0xF) as usize] as isize);
        let sign = (nibble & 8) != 0;
        let diff = (i32::from(nibble & 7) * IMA_STEP_TABLE[self.step]) >> 2;
        let sample = if !sign { self.predictor + diff } else { self.predictor - diff };
        self.predictor = sample.max(i32::from(i16::MIN)).min(i32::from(i16::MAX));
        self.step = istep.max(0).min(IMA_MAX_STEP as isize) as usize;
        self.predictor as i16
    }
}

struct EscapeADPCMDecoder {
    ainfo:      NAAudioInfo,
    chmap:      NAChannelMap,
    ch_state:   [EscapeIMAState; 2],
}

impl EscapeADPCMDecoder {
    fn new() -> Self {
        Self {
            ainfo:      NAAudioInfo::new(0, 1, SND_S16P_FORMAT, 0),
            chmap:      NAChannelMap::new(),
            ch_state:   [EscapeIMAState::new(), EscapeIMAState::new()],
        }
    }
}

impl NADecoder for EscapeADPCMDecoder {
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
    #[allow(clippy::identity_op)]
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

impl NAOptionHandler for EscapeADPCMDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_audio() -> Box<dyn NADecoder + Send> {
    Box::new(EscapeADPCMDecoder::new())
}

#[derive(Default)]
struct EscapePacketiser {
    stream:     Option<NAStreamRef>,
    buf:        Vec<u8>,
    end:        usize,
    frameno:    u32,
}

impl EscapePacketiser {
    fn new() -> Self { Self::default() }
}

impl NAPacketiser for EscapePacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
        self.stream = Some(stream);
    }
    fn add_data(&mut self, src: &[u8]) -> bool {
        self.buf.extend_from_slice(src);
        self.buf.len() < (1 << 10)
    }
    fn parse_stream(&mut self, id: u32) -> DecoderResult<NAStreamRef> {
        if let Some(ref stream) = self.stream {
            let mut stream = NAStream::clone(stream);
            stream.id = id;
            Ok(stream.into_ref())
        } else {
            Err(DecoderError::MissingReference)
        }
    }
    fn skip_junk(&mut self) -> DecoderResult<usize> {
        Err(DecoderError::NotImplemented)
    }
    fn get_packet(&mut self, stream: NAStreamRef) -> DecoderResult<Option<NAPacket>> {
        if self.buf.len() >= 8 {
            if self.buf[1] != 1 {
                return Err(DecoderError::InvalidData);
            }
            let vsize = read_u32le(&self.buf[4..]).unwrap_or_default() as usize;
            if self.buf.len() < vsize {
                return Ok(None);
            }
            let mut data = vec![0; vsize];
            data.copy_from_slice(&self.buf[..vsize]);
            self.buf.drain(..vsize);
            let ts = NATimeInfo::new(Some(u64::from(self.frameno)), None, None, stream.tb_num, stream.tb_den);
            self.frameno += 1;
            Ok(Some(NAPacket::new(stream, ts, false, data)))
        } else {
            Ok(None)
        }
    }
    fn reset(&mut self) {
        self.buf.clear();
        self.end = 0;
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

pub fn get_packetiser() -> Box<dyn NAPacketiser + Send> {
    Box::new(EscapePacketiser::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::{RegisteredDecoders, RegisteredPacketisers};
    use nihav_core::demuxers::RegisteredRawDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_escape102() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // sample from Archive Magazine CD 1995
        test_decoding_raw("armovie", "escape102", "assets/Acorn/sprite1", Some(5),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x54a114a9, 0x2183370a, 0x4d12cd7d, 0x83da183b],
                                [0x8c2a3641, 0x2b81b458, 0xdae4df2c, 0x6c8d80c5],
                                [0x27d5f25d, 0x3cb31a7e, 0x9f3f9904, 0xe28c3419],
                                [0x82f2f3b6, 0xe2b9931d, 0x2df8f388, 0x33b4e307],
                                [0xf13e7c8e, 0xde49e589, 0x555a90fd, 0xa67f6641],
                                [0x4ab76426, 0x0ef60466, 0x5613869d, 0xbac97a4a]]));
    }
    #[test]
    fn test_escape122() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/game-formats/rpl/bigredracing-escape122/INTROSND.RPL
        test_decoding_raw("armovie", "escape122", "assets/Acorn/INTROSND.RPL", Some(5),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x26550b92, 0xfd6d69e4, 0x42e692da, 0x5d3772d7],
                                [0x23cdd0fd, 0x7eecc973, 0xd3c6b2f0, 0xefff8259],
                                [0x48df421d, 0x6bb3cfaf, 0xfde6eaa4, 0xd667c14a],
                                [0x73242dff, 0xe991da8b, 0x3626d3a5, 0x54ed4205],
                                [0x36afa58b, 0x421580c0, 0x9232c164, 0x94dc9659],
                                [0xc76b37de, 0x51ab2a18, 0x2fb67559, 0x2d3fef13]]));
    }
    #[test]
    fn test_escape124() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/game-formats/rpl/escape124/ESCAPE.RPL
        test_decoding_raw("armovie", "escape124", "assets/Acorn/ESCAPE.RPL", Some(5),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x7062e4b6, 0x75ec8ada, 0x924b63e0, 0xa160d83c],
                                [0x22ecee28, 0xe4651563, 0x606fd6de, 0x03eac684],
                                [0xd5b51cb5, 0x252df07f, 0x758864d1, 0x919dd7db],
                                [0x4cba97c5, 0x6f693716, 0x2698e741, 0xbe60be37],
                                [0x2bf69c95, 0xc61889b6, 0xaa60f946, 0xd9ee23ae],
                                [0xec79d033, 0xd0f394fb, 0xec139ab7, 0xdc89a3e9]]));
    }
    #[test]
    fn test_escape130() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/game-formats/rpl/warzone/res_struttech.rpl
        test_decoding_raw("armovie", "escape130", "assets/Acorn/res_struttech.rpl", Some(5),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0xdacfb155, 0xe729c6a5, 0xc5a77201, 0xf7818d6b],
                                [0xdacfb155, 0xe729c6a5, 0xc5a77201, 0xf7818d6b],
                                [0x0065306d, 0x5501862d, 0x4105ec8a, 0x823c0d53],
                                [0xf690e4de, 0xe0a12b58, 0xc5a4e00f, 0x0c183261],
                                [0xa0598121, 0xa434bead, 0xf605a6cc, 0xe8affc6a],
                                [0xd1444e15, 0xa23bd2f7, 0xfe66690a, 0x7f2d5a2f]]));
    }
    #[test]
    fn test_escape_adpcm() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/game-formats/rpl/bigredracing-escape122/INTROSND.RPL
        test_decoding_raw("armovie", "escape-adpcm", "assets/Acorn/INTROSND.RPL", Some(5),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5([0xcc1b7281, 0x30f005d2, 0xac7e9622, 0x4af24026]));
    }
}

const CHROMA_VALUES: [u8; 32] = [
     20,  28,  36,  44,  52,  60,  68,  76,
     84,  92, 100, 106, 112, 116, 120, 124,
    128, 132, 136, 140, 144, 150, 156, 164,
    172, 180, 188, 196, 204, 212, 220, 228
];
const E130_Y_SIGNS: [[i8; 4]; 64] = [
    [  0,  0,  0,  0 ], [ -1,  1,  0,  0 ], [  1, -1,  0,  0 ], [ -1,  0,  1,  0 ],
    [ -1,  1,  1,  0 ], [  0, -1,  1,  0 ], [  1, -1,  1,  0 ], [ -1, -1,  1,  0 ],
    [  1,  0, -1,  0 ], [  0,  1, -1,  0 ], [  1,  1, -1,  0 ], [ -1,  1, -1,  0 ],
    [  1, -1, -1,  0 ], [ -1,  0,  0,  1 ], [ -1,  1,  0,  1 ], [  0, -1,  0,  1 ],
    [  0,  0,  0,  0 ], [  1, -1,  0,  1 ], [ -1, -1,  0,  1 ], [ -1,  0,  1,  1 ],
    [ -1,  1,  1,  1 ], [  0, -1,  1,  1 ], [  1, -1,  1,  1 ], [ -1, -1,  1,  1 ],
    [  0,  0, -1,  1 ], [  1,  0, -1,  1 ], [ -1,  0, -1,  1 ], [  0,  1, -1,  1 ],
    [  1,  1, -1,  1 ], [ -1,  1, -1,  1 ], [  0, -1, -1,  1 ], [  1, -1, -1,  1 ],
    [  0,  0,  0,  0 ], [ -1, -1, -1,  1 ], [  1,  0,  0, -1 ], [  0,  1,  0, -1 ],
    [  1,  1,  0, -1 ], [ -1,  1,  0, -1 ], [  1, -1,  0, -1 ], [  0,  0,  1, -1 ],
    [  1,  0,  1, -1 ], [ -1,  0,  1, -1 ], [  0,  1,  1, -1 ], [  1,  1,  1, -1 ],
    [ -1,  1,  1, -1 ], [  0, -1,  1, -1 ], [  1, -1,  1, -1 ], [ -1, -1,  1, -1 ],
    [  0,  0,  0,  0 ], [  1,  0, -1, -1 ], [  0,  1, -1, -1 ], [  1,  1, -1, -1 ],
    [ -1,  1, -1, -1 ], [  1, -1, -1, -1 ], [  0,  0,  0,  0 ], [  0,  0,  0,  0 ],
    [  0,  0,  0,  0 ], [  0,  0,  0,  0 ], [  0,  0,  0,  0 ], [  0,  0,  0,  0 ],
    [  0,  0,  0,  0 ], [  0,  0,  0,  0 ], [  0,  0,  0,  0 ], [  0,  0,  0,  0 ]
];

const E102_CHROMA_TAB: [u16; 256] = [
    0x77A0, 0x77C0, 0x77E0, 0x7400, 0x7420, 0x7440, 0x7460, 0x7BA0,
    0x7BC0, 0x7BE0, 0x7800, 0x7820, 0x7840, 0x7860, 0x7FA0, 0x7FC0,
    0x7FE0, 0x7C00, 0x7C20, 0x7C40, 0x7C60, 0x03A0, 0x03C0, 0x03E0,
    0x0000, 0x0020, 0x0040, 0x0060, 0x07A0, 0x07C0, 0x07E0, 0x0400,
    0x0420, 0x0440, 0x0460, 0x0BA0, 0x0BC0, 0x0BE0, 0x0800, 0x0820,
    0x0840, 0x0860, 0x0FA0, 0x0FC0, 0x0FE0, 0x0C00, 0x0C20, 0x0C40,
    0x0C60, 0x3340, 0x30C0, 0x2400, 0x1A80, 0x1800, 0x1980, 0x0CC0,
    0x02E0, 0x0180, 0x74C0, 0x6AE0, 0x6860, 0x5EE0, 0x5C60, 0x53A0,
    0x77A0, 0x77C0, 0x77E0, 0x7400, 0x7420, 0x7440, 0x7460, 0x7BA0,
    0x7BC0, 0x7BE0, 0x7800, 0x7820, 0x7840, 0x7860, 0x7FA0, 0x7FC0,
    0x7FE0, 0x7C00, 0x7C20, 0x7C40, 0x7C60, 0x03A0, 0x03C0, 0x03E0,
    0x0000, 0x0020, 0x0040, 0x0060, 0x07A0, 0x07C0, 0x07E0, 0x0400,
    0x0420, 0x0440, 0x0460, 0x0BA0, 0x0BC0, 0x0BE0, 0x0800, 0x0820,
    0x0840, 0x0860, 0x0FA0, 0x0FC0, 0x0FE0, 0x0C00, 0x0C20, 0x0C40,
    0x0C60, 0x33A0, 0x26E0, 0x2460, 0x1AE0, 0x1860, 0x0E80, 0x0D20,
    0x0340, 0x7680, 0x7520, 0x6B40, 0x68C0, 0x5F40, 0x5CC0, 0x5000,
    0x77A0, 0x77C0, 0x77E0, 0x7400, 0x7420, 0x7440, 0x7460, 0x7BA0,
    0x7BC0, 0x7BE0, 0x7800, 0x7820, 0x7840, 0x7860, 0x7FA0, 0x7FC0,
    0x7FE0, 0x7C00, 0x7C20, 0x7C40, 0x7C60, 0x03A0, 0x03C0, 0x03E0,
    0x0000, 0x0020, 0x0040, 0x0060, 0x07A0, 0x07C0, 0x07E0, 0x0400,
    0x0420, 0x0440, 0x0460, 0x0BA0, 0x0BC0, 0x0BE0, 0x0800, 0x0820,
    0x0840, 0x0860, 0x0FA0, 0x0FC0, 0x0FE0, 0x0C00, 0x0C20, 0x0C40,
    0x0C60, 0x3000, 0x2740, 0x24C0, 0x1B40, 0x18C0, 0x0EE0, 0x0D80,
    0x00C0, 0x76E0, 0x7580, 0x6BA0, 0x6920, 0x5FA0, 0x5D20, 0x5060,
    0x77A0, 0x77C0, 0x77E0, 0x7400, 0x7420, 0x7440, 0x7460, 0x7BA0,
    0x7BC0, 0x7BE0, 0x7800, 0x7820, 0x7840, 0x7860, 0x7FA0, 0x7FC0,
    0x7FE0, 0x7C00, 0x7C20, 0x7C40, 0x7C60, 0x03A0, 0x03C0, 0x03E0,
    0x0000, 0x0020, 0x0040, 0x0060, 0x07A0, 0x07C0, 0x07E0, 0x0400,
    0x0420, 0x0440, 0x0460, 0x0BA0, 0x0BC0, 0x0BE0, 0x0800, 0x0820,
    0x0840, 0x0860, 0x0FA0, 0x0FC0, 0x0FE0, 0x0C00, 0x0C20, 0x0C40,
    0x0C60, 0x33A0, 0x27A0, 0x2520, 0x1BA0, 0x1920, 0x0F40, 0x0280,
    0x0120, 0x7740, 0x6A80, 0x6800, 0x6980, 0x5C00, 0x5340, 0x50C0
];
