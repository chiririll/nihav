use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

const BGR555_FORMAT: NAPixelFormaton = NAPixelFormaton { model: ColorModel::RGB(RGBSubmodel::RGB), components: 3,
                                        comp_info: [
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  0, comp_offs: 0, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  5, comp_offs: 1, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift: 10, comp_offs: 2, next_elem: 2 }),
                                            None, None],
                                        elem_size: 2, be: false, alpha: false, palette: false };

struct HybridReader<'a> {
    src:    MemoryReader<'a>,
    bits:   u8,
    bitbuf: u32
}

impl<'a> HybridReader<'a> {
    fn new(src: &'a [u8]) -> Self {
        Self { src: MemoryReader::new_read(src), bits: 0, bitbuf: 0 }
    }
    fn read_byte(&mut self) -> DecoderResult<u8> { Ok(self.src.read_byte()?) }
    fn read_u16(&mut self) -> DecoderResult<u16> { Ok(self.src.read_u16le()?) }
    fn read_bit(&mut self) -> DecoderResult<u8> {
        if self.bits == 0 {
            self.bitbuf = self.src.read_u32le()?;
            self.bits   = 32;
        }
        let bit = (self.bitbuf & 1) as u8;
        self.bitbuf >>= 1;
        self.bits    -= 1;
        Ok(bit)
    }
    fn read_bool(&mut self) -> DecoderResult<bool> { Ok(self.read_bit()? != 0) }
}

fn lz_decompress(src: &[u8], dst: &mut Vec<u8>) -> DecoderResult<()> {
    let mut hr = HybridReader::new(src);
    dst.clear();
    loop {
        if !hr.read_bool()? {
            dst.push(hr.read_byte()?);
        } else {
            let (offset, len) = if !hr.read_bool()? {
                    let off = hr.read_byte()?;
                    let mut len = hr.read_bit()?;
                    len = len * 2 + hr.read_bit()?;
                    (256 - usize::from(off), usize::from(len))
                } else {
                    let lo = usize::from(hr.read_byte()?);
                    let hi = usize::from(hr.read_byte()?);
                    let mut off = 0x2000 - ((hi >> 3) << 8) - lo;
                    let len = hi & 7;
                    if len > 0 {
                        (off, len)
                    } else {
                        let b = hr.read_byte()?;
                        if (b & 0x80) != 0 {
                            off += 0x2000;
                        }
                        let len = usize::from(b & 0x7F);
                        match len {
                            0 => {
                                let len = usize::from(hr.read_u16()?);
                                (off, len.wrapping_sub(2))
                            },
                            1 => return Ok(()),
                            _ => (off, len),
                        }
                    }
                };
            let length = len.wrapping_add(2);
            validate!(offset <= dst.len());
            if offset == 1 {
                let sym = dst[dst.len() - 1];
                for _ in 0..length {
                    dst.push(sym);
                }
            } else {
                for _ in 0..length {
                    let sym = dst[dst.len() - offset];
                    dst.push(sym);
                }
            }
        }
    }
}

struct PDQ2Decoder {
    info:       NACodecInfoRef,
    pal:        [u8; 768],
    frame:      Vec<u8>,
    frame16:    Vec<u16>,
    dbuf:       Vec<u8>,
    width:      usize,
    height:     usize,
    stride:     usize,
    saturn:     bool,
}

impl PDQ2Decoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            pal:        [0; 768],
            frame:      Vec::new(),
            frame16:    Vec::new(),
            dbuf:       Vec::new(),
            width:      0,
            height:     0,
            stride:     0,
            saturn:     false,
        }
    }
    fn decode_pal(&mut self, src: &[u8]) -> DecoderResult<NABufferType> {
        let data = match src[0] {
                0 => &src[1..],
                1 => {
                    lz_decompress(&src[1..], &mut self.dbuf)?;
                    &self.dbuf
                },
                _ => return Err(DecoderError::InvalidData),
            };

        let field2 = usize::from(read_u16le(data)?);
        if field2 == 0 {
            for el in self.frame.iter_mut() {
                *el = 0;
            }
        }
        let ops_size = usize::from(read_u16le(&data[2..])?);
        validate!(ops_size > 1);
        let mv_size  = usize::from(read_u16le(&data[4..])?);
        let unk_size = usize::from(read_u16le(&data[6..])?);
        validate!(ops_size + mv_size + unk_size + 8 <= data.len());
        if unk_size != 0 {
            println!("Unknown data is non-zero");
            return Err(DecoderError::NotImplemented);
        }

        let mut ops = MemoryReader::new_read(&data[9..8 + ops_size]);
        let mut mv  = MemoryReader::new_read(&data[8 + ops_size + unk_size..]);
        let mut clr = MemoryReader::new_read(&data[8 + ops_size + unk_size + mv_size..]);

        let mut tile_no = 0;
        let tile_w = self.width / 4;
        let ntiles = tile_w * ((self.height + 3) / 4);
        let img_size = (self.width * self.height) as isize;

        let mut run = 0;
        let mut blk = [[0; 4]; 4];
        let mut ref_off = 0;
        while tile_no < ntiles {
            let op = ops.read_byte()?;
            let (dx, dy) = match op {
                    0x85 => {
                        let dx = mv.read_u16le()? as i16;
                        let dy = mv.read_u16le()? as i16;
                        (dx, dy)
                    },
                    0x86 => return Err(DecoderError::InvalidData),
                    0x87 => {
                        let mode = ops.read_byte()?;
                        if (mode & 0x80) == 0 {
                            tile_no += usize::from(mode + 1);
                        } else {
                            run = usize::from(mode & 0x7F);
                        }
                        continue;
                    },
                    0x88 => {
                        let dx = i16::from(mv.read_byte()?);
                        let dy = i16::from(mv.read_byte()? as i8);
                        (dx, dy)
                    },
                    0x89 => {
                        for row in blk.iter_mut() {
                            clr.read_buf(row)?;
                        }
                        (0, 0)
                    },
                    0x8A => {
                        let dx = i16::from(mv.read_byte()?) - 256;
                        let dy = i16::from(mv.read_byte()? as i8);
                        (dx, dy)
                    },
                    _ => {
                        (i16::from((op as i8) >> 4), i16::from((op as i8) << 4 >> 4))
                    },
                };
            let mut dst_addr = (tile_no % tile_w) * 4 + (tile_no / tile_w) * 4 * self.width;
            if op != 0x89 {
                ref_off += isize::from(dx) + isize::from(dy) * (self.width as isize);
                let mut offset = ref_off + (dst_addr as isize);
                if offset < 0 {
                    offset += img_size;
                    ref_off += img_size;
                } else if offset >= img_size {
                    offset -= img_size;
                    ref_off -= img_size;
                }
                let src_addr = offset as usize;
                validate!(src_addr + 4 + 3 * self.width <= self.frame.len());
                for (row, sline) in blk.iter_mut().zip(self.frame[src_addr..].chunks(self.width)) {
                    row.copy_from_slice(&sline[..4]);
                }
            }
            validate!(tile_no + run < ntiles);
            validate!((tile_no % tile_w) + run < tile_w);
            for _ in 0..=run {
                for (line, srow) in self.frame[dst_addr..].chunks_mut(self.width).zip(blk.iter()) {
                    line[..4].copy_from_slice(srow);
                }
                dst_addr += 4;
                tile_no += 1;
            }
            run = 0;
        }

        let buf = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 2)?;
        let mut vbuf = buf.get_vbuf().unwrap();
        let paloff = vbuf.get_offset(1);
        let stride = vbuf.get_stride(0);
        let data = vbuf.get_data_mut().unwrap();

        for (drow, srow) in data.chunks_mut(stride).zip(self.frame.chunks(self.width)) {
            drow[..self.width].copy_from_slice(srow);
        }
        data[paloff..][..768].copy_from_slice(&self.pal);

        Ok(buf)
    }
    fn decode_saturn(&mut self, src: &[u8]) -> DecoderResult<NABufferType> {
        validate!(src[0] == 0x80);
        let data = &src[1..];

        let field2 = usize::from(read_u16be(data)?);
        if field2 == 0 {
            for el in self.frame16.iter_mut() {
                *el = 0;
            }
        }
        let ops_size = usize::from(read_u16be(&data[2..])?);
        validate!(ops_size > 1);
        let mv_size  = usize::from(read_u16be(&data[4..])?);
        let unk_size = usize::from(read_u16be(&data[6..])?);
        if unk_size != 0 {
            println!("Unknown data is non-zero");
            return Err(DecoderError::NotImplemented);
        }

        let ops_size = (ops_size + 3) & !3;
        let unk_size = (unk_size + 3) & !3;
        let mv_size  = (mv_size  + 3) & !3;
        validate!(ops_size + mv_size + unk_size + 11 <= data.len());
        let mut ops = MemoryReader::new_read(&data[12..11 + ops_size]);
        let mut mv  = MemoryReader::new_read(&data[11 + ops_size + unk_size..]);
        let mut clr = MemoryReader::new_read(&data[11 + ops_size + unk_size + mv_size..]);

        let mut tile_no = 0;
        let tile_w = self.width / 4;
        let ntiles = tile_w * ((self.height + 3) / 4);
        let img_size = (self.width * self.height) as isize;

        let mut run = 0;
        let mut blk = [[0; 4]; 4];
        let mut ref_off = 0;
        while tile_no < ntiles {
            let op = ops.read_byte()?;
            let (dx, dy) = match op {
                    0x85 => {
                        let dx = mv.read_u16be()? as i16;
                        let dy = mv.read_u16be()? as i16;
                        (dx, dy)
                    },
                    0x86 => return Err(DecoderError::InvalidData),
                    0x87 => {
                        let mode = ops.read_byte()?;
                        if (mode & 0x80) == 0 {
                            tile_no += usize::from(mode + 1);
                        } else {
                            run = usize::from(mode & 0x7F);
                        }
                        continue;
                    },
                    0x88 => {
                        let dy = i16::from(mv.read_byte()? as i8);
                        let dx = i16::from(mv.read_byte()?);
                        (dx, dy)
                    },
                    0x89 => {
                        for row in blk.iter_mut() {
                            clr.read_u16be_arr(row)?;
                        }
                        (0, 0)
                    },
                    0x8A => {
                        let dy = i16::from(mv.read_byte()? as i8);
                        let dx = i16::from(mv.read_byte()?) - 256;
                        (dx, dy)
                    },
                    _ => {
                        (i16::from((op as i8) >> 4), i16::from((op as i8) << 4 >> 4))
                    },
                };
            let mut dst_addr = (tile_no % tile_w) * 4 + (tile_no / tile_w) * 4 * self.width;
            if op != 0x89 {
                ref_off += isize::from(dx) + isize::from(dy) * (self.width as isize);
                let mut offset = ref_off + (dst_addr as isize);
                if offset < 0 {
                    offset += img_size;
                    ref_off += img_size;
                } else if offset >= img_size {
                    offset -= img_size;
                    ref_off -= img_size;
                }
                let src_addr = offset as usize;
                validate!(src_addr + 4 + 3 * self.width <= self.frame16.len());
                for (row, sline) in blk.iter_mut().zip(self.frame16[src_addr..].chunks(self.width)) {
                    row.copy_from_slice(&sline[..4]);
                }
            }
            validate!(tile_no + run < ntiles);
            validate!((tile_no % tile_w) + run < tile_w);
            for _ in 0..=run {
                for (line, srow) in self.frame16[dst_addr..].chunks_mut(self.width).zip(blk.iter()) {
                    line[..4].copy_from_slice(srow);
                }
                dst_addr += 4;
                tile_no += 1;
            }
            run = 0;
        }

        let buf = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 2)?;
        let mut vbuf = buf.get_vbuf16().unwrap();
        let stride = vbuf.get_stride(0);
        let data = vbuf.get_data_mut().unwrap();

        for (drow, srow) in data.chunks_mut(stride).zip(self.frame16.chunks(self.width)) {
            drow[..self.width].copy_from_slice(srow);
        }

        Ok(buf)
    }
}

impl NADecoder for PDQ2Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.width;
            self.height = vinfo.height;
            validate!(((self.width | self.height) & 3) == 0);
            self.stride = (vinfo.width + 3) & !3;
            self.saturn = !vinfo.format.is_paletted();
            if !self.saturn {
                self.frame = vec![0; self.width * self.height];
            } else {
                self.frame16 = vec![0; self.width * self.height];
            }
            self.dbuf   = Vec::new();
            self.pal    = [0; 768];
            let fmt = if !self.saturn { PAL8_FORMAT } else { BGR555_FORMAT };
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, true, fmt));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 9);

        let buf = if !self.saturn {
            for sd in pkt.side_data.iter() {
                if let NASideData::Palette(true, ref pal) = sd {
                    for (dst, src) in self.pal.chunks_mut(3).zip(pal.chunks(4)) {
                        dst[0] = src[0];
                        dst[1] = src[1];
                        dst[2] = src[2];
                    }
                    break;
                }
            }
            self.decode_pal(&src)?
        } else {
            self.decode_saturn(&src)?
        };

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buf);
        let ftype = if pkt.keyframe { FrameType::I } else { FrameType::P };
        frm.set_frame_type(ftype);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for PDQ2Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(PDQ2Decoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::game_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    // sample from Incredible Hulk: The Pantheon Saga DOS demo
    #[test]
    fn test_pdq2_video() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        test_decoding("avi", "pdq2", "assets/Game/sdream.str",
                      Some(2), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x764b6a32, 0x4b21fd07, 0x99a27eea, 0xe93a1f5a],
                            [0xd3965312, 0x12aabd4d, 0xe210468a, 0xf0698653],
                            [0x46370416, 0x9dad4793, 0xddba16cf, 0x621c7b70]]));
    }
    // sample from Incredible Hulk: The Pantheon Saga prototype Saturn version
    #[test]
    fn test_pdq2_saturn_video() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        test_decoding("avi", "pdq2", "assets/Game/ATDLOGO.STR",
                      Some(5), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x40c420d0, 0x093daf05, 0xfb812fe4, 0x960dc2b3],
                            [0x55a00c1e, 0x112a2b41, 0x1ca3086c, 0x2c6d24ec],
                            [0xe6080028, 0x19d9df8f, 0x7394f910, 0x51fbdac2],
                            [0xd130a7e7, 0xf3077e29, 0x9a67f1b0, 0xe97faae1],
                            [0xc7dc60df, 0xb69cc421, 0x2a2fc233, 0xed299865],
                            [0x6f2f5380, 0xba586542, 0x4309a74a, 0xd24c029b]]));
    }
}
