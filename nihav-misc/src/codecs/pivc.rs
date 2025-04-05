use std::cmp::Ordering;
use nihav_core::io::byteio::*;
use nihav_core::io::bitreader::*;
use nihav_core::codecs::*;

const DICT_SIZE: usize = 4096;
const START_BITS: u8 = 9;
const START_POS:   usize = 257;
const INVALID_POS: usize = 65536;

struct LZWState {
    dict_sym:   [u8; DICT_SIZE + 1],
    dict_prev:  [u16; DICT_SIZE + 1],
    dict_pos:   usize,
    idx_bits:   u8,
    last_idx:   usize,
}

impl LZWState {
    fn new() -> Self {
        Self {
            dict_sym:   [0; DICT_SIZE + 1],
            dict_prev:  [0; DICT_SIZE + 1],
            dict_pos:   START_POS,
            idx_bits:   START_BITS,
            last_idx:   INVALID_POS,
        }
    }
    fn reset(&mut self) {
        self.dict_pos = START_POS;
        self.idx_bits = START_BITS;
        self.last_idx = INVALID_POS;
    }
    fn add(&mut self, sym: u8) {
        if self.dict_pos < DICT_SIZE {
            self.dict_sym [self.dict_pos] = sym;
            self.dict_prev[self.dict_pos] = self.last_idx as u16;
            if self.last_idx != INVALID_POS {
                self.dict_pos += 1;
            }
        }
    }
    fn decode_idx(&self, dst: &mut Vec<u8>, idx: usize) -> DecoderResult<u8> {
        let mut tot_len = 1;
        let mut tidx = idx;
        while tidx > 256 {
            tidx = self.dict_prev[tidx] as usize;
            tot_len += 1;
        }
        let pos = dst.len();
        for _ in 0..tot_len {
            dst.push(0);
        }

        let mut end = pos + tot_len - 1;
        let mut tidx = idx;
        while tidx > 256 {
            dst[end] = self.dict_sym[tidx];
            end -= 1;
            tidx = self.dict_prev[tidx] as usize;
        }
        dst[end] = tidx as u8;

        Ok(tidx as u8)
    }
    fn decode(&mut self, src: &[u8], dst: &mut Vec<u8>) -> DecoderResult<()> {
        let mut br = BitReader::new(src, BitReaderMode::LE32MSB);

        self.idx_bits = START_BITS;
        self.last_idx = INVALID_POS;

        loop {
            let idx         = br.read(self.idx_bits)? as usize;
//println!("  LZW idx {idx:X} last {:X} ({}) / {}", self.last_idx, self.idx_bits, self.dict_pos);
            if idx == 256 {
                let mode    = br.read(2)?;
//println!("    mode {mode}");
                match mode {
                    0 => break,
                    1 => { self.reset(); },
                    _ => { self.idx_bits += 1; },
                }
                continue;
            }
            match idx.cmp(&self.dict_pos) {
                Ordering::Less => {
                    let lastsym = self.decode_idx(dst, idx)?;
                    self.add(lastsym);
                },
                Ordering::Equal => {
                    validate!(self.last_idx != INVALID_POS);
                    let lastsym = self.decode_idx(dst, self.last_idx)?;
                    dst.push(lastsym);
                    self.add(lastsym);
                },
                Ordering::Greater => return Err(DecoderError::InvalidData),
            }
            self.last_idx = idx;
        }
        Ok(())
    }
}

#[derive(Default)]
struct PIFrame {
    frame:      Vec<u8>,
    width:      usize,
    height:     usize,
    stride:     usize,
    tile_size:  usize,
}

impl PIFrame {
    fn new() -> Self { Self::default() }
    fn set_dimensions(&mut self, w: usize, h: usize) {
        self.width = w;
        self.height = h;
        let mut tile_size = 0x4000;
        while tile_size >= w.max(h) {
            tile_size >>= 2;
        }
        tile_size <<= 2;
        self.tile_size = tile_size;
        self.stride = (w + tile_size - 1) / tile_size * tile_size;
        self.frame = vec![0; self.stride * ((h + tile_size - 1) / tile_size * tile_size)];
    }
    fn decode_tile_intra(&mut self, xpos: usize, ypos: usize, tsize: usize, flags: &mut ByteReader, pixels: &mut ByteReader) -> DecoderResult<()> {
        let mut flg = flags.read_u16le()?;
        for yoff in (0..tsize).step_by(tsize / 4) {
            for xoff in (0..tsize).step_by(tsize / 4) {
                if xpos + xoff >= self.width || ypos + yoff >= self.height {
                    continue;
                }
                let cur_dst = &mut self.frame[xpos + xoff + (ypos + yoff) * self.stride..];
                if (flg & 1) == 0 {
                    let clr = pixels.read_byte()?;
                    for line in cur_dst.chunks_mut(self.stride).take(tsize / 4) {
                        for el in line[..tsize / 4].iter_mut() {
                            *el = clr;
                        }
                    }
                } else if tsize > 16 {
                    self.decode_tile_intra(xpos + xoff, ypos + yoff, tsize / 4, flags, pixels)?;
                } else {
                    for line in cur_dst.chunks_mut(self.stride).take(4) {
                        pixels.read_buf(&mut line[..4])?;
                    }
                }
                flg >>= 1;
            }
        }
        Ok(())
    }
    fn decode_intra(&mut self, flags: &mut ByteReader, pixels: &mut ByteReader) -> DecoderResult<()> {
        for y in (0..self.height).step_by(self.tile_size) {
            for x in (0..self.width).step_by(self.tile_size) {
                self.decode_tile_intra(x, y, self.tile_size, flags, pixels)?;
            }
        }
        Ok(())
    }

    fn decode_tile_inter(&mut self, xpos: usize, ypos: usize, tsize: usize, flags: &mut ByteReader, pixels: &mut ByteReader) -> DecoderResult<()> {
        let mut flg = flags.read_u16le()?;
        for yoff in (0..tsize).step_by(tsize / 4) {
            for xoff in (0..tsize).step_by(tsize / 4) {
                if xpos + xoff >= self.width || ypos + yoff >= self.height {
                    continue;
                }
                let cur_dst = &mut self.frame[xpos + xoff + (ypos + yoff) * self.stride..];
                if (flg & 1) == 0 {
                    let clr = pixels.read_byte()?;
                    if clr != 0 {
                        for line in cur_dst.chunks_mut(self.stride).take(tsize / 4) {
                            for el in line[..tsize / 4].iter_mut() {
                                *el = clr;
                            }
                        }
                    }
                } else if tsize > 16 {
                    self.decode_tile_inter(xpos + xoff, ypos + yoff, tsize / 4, flags, pixels)?;
                } else {
                    for line in cur_dst.chunks_mut(self.stride).take(4) {
                        pixels.read_buf(&mut line[..4])?;
                    }
                }
                flg >>= 1;
            }
        }
        Ok(())
    }
    fn decode_inter(&mut self, flags: &mut ByteReader, pixels: &mut ByteReader) -> DecoderResult<()> {
        for y in (0..self.height).step_by(self.tile_size) {
            for x in (0..self.width).step_by(self.tile_size) {
                self.decode_tile_inter(x, y, self.tile_size, flags, pixels)?;
            }
        }
        Ok(())
    }

    fn output(&self, dst: &mut [u8], dstride: usize) {
        for (dline, sline) in dst.chunks_mut(dstride)
                .zip(self.frame.chunks_exact(self.stride)).take(self.height) {
            dline[..self.width].copy_from_slice(&sline[..self.width]);
        }
    }
}

struct PIVideoDecoder {
    info:       NACodecInfoRef,
    pal:        [u8; 768],
    ubuf:       Vec<u8>,
    lzw:        LZWState,
    frame:      PIFrame,
}

impl PIVideoDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfo::new_dummy(),
            pal:        [0; 768],
            frame:      PIFrame::new(),
            ubuf:       Vec::new(),
            lzw:        LZWState::new(),
        }
    }
}

impl NADecoder for PIVideoDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            validate!(w >= 4 && h >= 4);
            self.frame.set_dimensions(w, h);
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            if let Some(edata) = info.get_extradata() {
                validate!(edata.len() > 16);
                let depth = read_u16le(&edata[4..])?;
                validate!(matches!(depth & 0xF0FF, 0x8 | 0x1008));
                let size = read_u32le(&edata[8..])? as usize;
                validate!(size <= edata.len() - 16);
                let nclrs = read_u16le(&edata[12..])? as usize;
                validate!((1..=256).contains(&nclrs));
                validate!(16 + size + nclrs * 4 <= edata.len());
                for (dclr, sclr) in self.pal.chunks_exact_mut(3)
                        .zip(edata[16 + size..].chunks_exact(4).take(nclrs)) {
                    dclr[0] = sclr[2];
                    dclr[1] = sclr[1];
                    dclr[2] = sclr[0];
                }
            } else {
                return Err(DecoderError::InvalidData);
            }
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();

        validate!(!src.is_empty());
        if src[0] == 0 {
            let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), NABufferType::None);
            frm.set_keyframe(false);
            frm.set_frame_type(FrameType::Skip);
            return Ok(frm.into_ref());
        }

        validate!(src.len() > 16);
        let frame_flags = read_u32le(&src)?;
        let frame_size  = read_u32le(&src[4..])? as usize;
//println!(" frame flags {frame_flags:X} size {frame_size}/{}", src.len());
        validate!(frame_size > 12 && frame_size <= src.len() - 4);
        let flags_size  = read_u32le(&src[8..])? as usize;
        validate!(flags_size > 4 && (flags_size & 1) == 0 && flags_size < frame_size - 4);

        let tree_flags = &src[12..][..flags_size - 4];
        let pixel_data = if (frame_flags & 2) == 0 {
                &src[flags_size + 8..]
            } else {
                self.ubuf.clear();
                self.lzw.decode(&src[flags_size + 8..], &mut self.ubuf)?;
                &self.ubuf
            };

        let mut mr = MemoryReader::new_read(tree_flags);
        let mut flags = ByteReader::new(&mut mr);

        let mut mr = MemoryReader::new_read(pixel_data);
        let mut pixels = ByteReader::new(&mut mr);

        let is_intra = (frame_flags & 1) == 0;

        if is_intra {
            self.frame.decode_intra(&mut flags, &mut pixels)?;
        } else {
            self.frame.decode_inter(&mut flags, &mut pixels)?;
        }
//println!(" left: flags {} / {} pixels {} / {} ({})", flags.tell(), flags_size-4, pixels.tell(), pixel_data.len(), src[flags_size + 8..].len());

        let vinfo = NAVideoInfo::new(self.frame.width, self.frame.height, false, PAL8_FORMAT);
        let bufinfo = alloc_video_buffer(vinfo, 0)?;

        if let Some(mut buf) = bufinfo.get_vbuf() {
            let stride = buf.get_stride(0);
            let pal_off = buf.get_offset(1);
            let data = buf.get_data_mut().unwrap();

            self.frame.output(data, stride);

            data[pal_off..][..768].copy_from_slice(&self.pal);
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.frame.frame.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for PIVideoDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(PIVideoDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_pivc() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from "How Does It Work?" encyclopedia
        test_decoding("avi", "pivideo", "assets/Misc/MAGEL.AVI", Some(10), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0xa45b5a6c, 0xf9e030e4, 0x47f9effe, 0xa8a36423],
                            [0x876196b8, 0xccc76094, 0x6aa10f1a, 0x3e7f526e],
                            [0x636e3261, 0xe6c09a88, 0xdbb3bc3e, 0x4cb20454]]));
    }
}
