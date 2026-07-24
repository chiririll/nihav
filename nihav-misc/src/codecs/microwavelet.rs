use nihav_core::io::byteio::*;
use nihav_core::io::bitreader::*;
use nihav_core::io::codebook::*;
use nihav_core::codecs::*;

const ESCAPE_SYM: u16 = 0xFFFF;

#[derive(Debug,PartialEq)]
enum Symbol {
    Code(i8, i8, i8, i8, i8),
    Skip(u8),
    SkipBack,
    SkipMany,
    Unknown(u16),
}

impl From<u16> for Symbol {
    fn from(sym_code: u16) -> Symbol {
        match sym_code {
            0..=63374 => // 13 * 15 * 13 * 5 * 5 {
                Symbol::Code((sym_code % 13) as i8 - 6,
                             ((sym_code / 13) % 15) as i8 - 7,
                             ((sym_code / 13 / 15) % 13) as i8 - 6,
                             ((sym_code / 13 / 15 / 13) % 5) as i8 - 2,
                             ((sym_code / 13 / 15 / 13 / 5) % 5) as i8 - 2),
            63375 => Symbol::SkipBack,
            63376..=63488 => Symbol::Skip((sym_code - 63374) as u8),
            0xFFFE => Symbol::SkipMany,
            _ => Symbol::Unknown(sym_code)
        }
    }
}

fn read_huff_desc(cbs: &mut Vec<Codebook<u16>>, br: &mut MemoryReader) -> DecoderResult<()> {
    let num_desc        = usize::from(br.read_u16be()?);
    validate!((1..=16).contains(&num_desc));
    let num_lens        = usize::from(br.read_u16be()?);
    validate!((1..=16).contains(&num_lens));
                          br.read_u16be()?;
    let num_syms        = usize::from(br.read_u16be()?);
    validate!(num_syms > 0);

    let mut code_def = Vec::with_capacity(num_syms);
    let mut syms = vec![0; num_syms];
    let mut counts = Vec::with_capacity(num_desc);

    for _ in 0..num_desc {
        let mut cnt = [0; 16];
                          br.read_u16be_arr(&mut cnt[..num_lens])?;
        counts.push(cnt);
    }
                          br.read_u16be_arr(&mut syms)?;

    for &sym in syms.iter() {
        code_def.push(FullCodebookDesc{ code: 0, bits: 0, sym });
    }

    for cnt in counts.iter() {
        let tot_count = cnt.iter().fold(0u32, |acc, &a| acc + u32::from(a));
        validate!(tot_count == 0 || tot_count == (num_syms as u32));
        if tot_count == 0 {
            continue;
        }
        let mut citer = code_def.iter_mut();
        for (i, &bcount) in cnt.iter().enumerate() {
            for _ in 0..bcount {
                citer.next().unwrap().bits = i as u8 + 1;
            }
        }

        let mut prefix = 0;
        for bitlen in 1..=16 {
            for code in code_def.iter_mut() {
                if code.bits == bitlen {
                    code.code = prefix;
                    prefix += 1;
                }
            }
            prefix <<= 1;
        }

        code_def.push(FullCodebookDesc{ code: 0xFFFF, bits: 16, sym: ESCAPE_SYM });

        let mut cbr = FullCodebookDescReader::new(&code_def);
        let cb = Codebook::new(&mut cbr, CodebookMode::MSB).unwrap();
        cbs.push(cb);

        code_def.pop();
    }

    Ok(())
}

fn decode_huff_data(br: &mut BitReader, cb: &Codebook<u16>, dst: &mut Vec<u16>) -> DecoderResult<()> {
    dst.clear();

    br.skip(8)?;
    loop {
        let sym = br.read_cb(cb)?;
        if sym == ESCAPE_SYM {
            let sym = br.read(16)? as u16;
            if sym == ESCAPE_SYM {
                return Ok(());
            }
            dst.push(sym);
        } else {
            dst.push(sym);
        }
    }
}

fn decode_lz_data(br: &mut dyn ByteIO, dst: &mut Vec<u16>) -> DecoderResult<()> {
    dst.clear();
    loop {
        let mut flags = br.read_byte()?;
        for _ in 0..8 {
            let code = br.read_u16be()?;
            if (flags & 0x80) != 0 {
                let len = usize::from(code >> 13);
                if len == 0 {
                    return Ok(());
                }
                let offset = (1 << 13) - usize::from(code & 0x1FFF);
                validate!(offset <= dst.len());
                let pos = dst.len() - offset;
                for i in 0..len {
                    let old = dst[pos + i];
                    dst.push(old);
                }
            } else {
                dst.push(code);
            }
            flags <<= 1;
        }
    }
}

fn reconstruct_frame(dst: &mut [u8], width: usize, height: usize, stride: usize, syms: &[u16]) -> DecoderResult<()> {

    const RBSTEPS: [i8; 13] = [-0x4E, -0x38, -0x26, -0x17, -0x0B, -0x04, 0x00, 0x04, 0x0B, 0x17, 0x26, 0x38, 0x4E];
    const GSTEPS: [i8; 15] = [-0x4D, -0x37, -0x25, -0x17, -0x0C, -0x05, -0x01, 0x00, 0x01, 0x05, 0x0C, 0x17, 0x25, 0x37, 0x4D];
    const DELTAS: [i8; 5] = [-0x1B, -0x09, 0x00, 0x09, 0x1B];

    let mut to_skip = 0;
    let mut pos = 0;
    for lines in dst.chunks_exact_mut(stride * 2).take(height / 2) {
        let (line0, line1) = lines.split_at_mut(stride);
        let mut rpred = 0i16;
        let mut gpred = 0i16;
        let mut bpred = 0i16;
        for x in (0..width).step_by(2) {
            if to_skip > 0 {
                to_skip -= 1;
                continue;
            }
            if pos >= syms.len() {
                return Ok(());
            }
            let sym = Symbol::from(syms[pos]);
            pos += 1;

            match sym {
                Symbol::Code(rd, gd, bd, hor, ver) => {
                    let rdelta = i16::from(RBSTEPS[(rd + 6) as usize]);
                    let gdelta = i16::from(GSTEPS [(gd + 7) as usize]);
                    let bdelta = i16::from(RBSTEPS[(bd + 6) as usize]);
                    let hor   = i16::from(DELTAS[(hor + 2) as usize]);
                    let ver   = i16::from(DELTAS[(ver + 2) as usize]);

                    rpred += rdelta;
                    gpred += gdelta;
                    bpred += bdelta;

                    let add0 = ( hor + ver) >> 1;
                    let add1 = ( hor - ver) >> 1;
                    let add2 = (-hor + ver) >> 1;
                    let add3 = (-hor - ver) >> 1;

                    let red   = bpred;
                    let green = gpred;
                    let blue  = rpred;

                    line0[x * 3]     = (red   + add0 + 128).clamp(0, 255) as u8;
                    line0[x * 3 + 1] = (green + add0 + 128).clamp(0, 255) as u8;
                    line0[x * 3 + 2] = (blue  + add0 + 128).clamp(0, 255) as u8;
                    line0[x * 3 + 3] = (red   + add1 + 128).clamp(0, 255) as u8;
                    line0[x * 3 + 4] = (green + add1 + 128).clamp(0, 255) as u8;
                    line0[x * 3 + 5] = (blue  + add1 + 128).clamp(0, 255) as u8;
                    line1[x * 3]     = (red   + add2 + 128).clamp(0, 255) as u8;
                    line1[x * 3 + 1] = (green + add2 + 128).clamp(0, 255) as u8;
                    line1[x * 3 + 2] = (blue  + add2 + 128).clamp(0, 255) as u8;
                    line1[x * 3 + 3] = (red   + add3 + 128).clamp(0, 255) as u8;
                    line1[x * 3 + 4] = (green + add3 + 128).clamp(0, 255) as u8;
                    line1[x * 3 + 5] = (blue  + add3 + 128).clamp(0, 255) as u8;
                },
                Symbol::Skip(skip) => {
                    to_skip = usize::from(skip) - 1;
                },
                Symbol::SkipBack => {
                    println!("skip back");
                    return Err(DecoderError::NotImplemented);
                },
                Symbol::SkipMany => {
                    to_skip = 0x830 - 1;
                },
                Symbol::Unknown(val) => {
                    println!("unknown symbol {val:04X}");
                    return Err(DecoderError::InvalidData);
                },
            }
        }
    }
    Ok(())
}

struct MicroWaveletDecoder {
    info:       NACodecInfoRef,
    cbs:        Vec<Codebook<u16>>,
    method:     u32,
    syms:       Vec<u16>,
}

impl MicroWaveletDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfo::new_dummy(),
            cbs:        Vec::new(),
            method:     0,
            syms:       Vec::new(),
        }
    }
}

impl NADecoder for MicroWaveletDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            validate!(((w | h) & 1) == 0);
            if let Some(edata) = info.get_extradata() {
                let mut br = MemoryReader::new_read(&edata);
                let tag                 = br.read_tag()?;
                validate!(&tag == b"NC00");
                let cfg_size            = br.read_u32be()?;
                validate!(cfg_size == 0x638 && br.left() >= i64::from(cfg_size));
                                          br.read_skip(0x61B)?;
                let num_defs            = usize::from(br.read_byte()?);
                                          br.read_skip(4)?;
                validate!(num_defs <= 1);
                let method              = br.read_u32be()?;
                match method {
                    0 => {
                        println!("raw decoding is not implemented");
                        return Err(DecoderError::NotImplemented);
                    },
                    1 | 2 => {},
                    3 => {
                        println!("LZSS mode is not tested");
                    },
                    _ => return Err(DecoderError::InvalidData),
                }
                self.method = method;
                let format              = br.read_u32be()?;
                if format != 0 {
                    println!(" unsupported format {format}");
                    return Err(DecoderError::NotImplemented);
                }
                let width               = br.read_u32be()? as usize;
                let height              = br.read_u32be()? as usize;
                validate!(width == w && height == h);
                                          br.read_u32be()?;
                                          br.read_u32be()?;
                if num_defs > 0 {
                    self.cbs.clear();
                    read_huff_desc(&mut self.cbs, &mut br)?;
                }
            } else {
                println!("Extradata needs to be passed (from track udta)");
                return Err(DecoderError::InvalidData);
            }
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, RGB24_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 1);
        let cb_idx = usize::from(src[0]);
        validate!(cb_idx < self.cbs.len());
        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 4)?;

        if let Some(mut buf) = bufinfo.get_vbuf() {
            let (width, height) = buf.get_dimensions(0);
            let stride = buf.get_stride(0);
            let data = buf.get_data_mut().unwrap();
            match self.method {
                1 => {
                    let mut br = MemoryReader::new_read(&src);
                    validate!(br.left() > 0 && (br.left() & 1) == 0);
                    let nsyms = (br.left() / 2) as usize;
                    self.syms.resize(nsyms, 0);
                    br.read_u16be_arr(&mut self.syms)?;
                },
                2 => {
                    let mut br = BitReader::new(&src[1..], BitReaderMode::BE);
                    decode_huff_data(&mut br, &self.cbs[cb_idx], &mut self.syms)?;
                },
                3 => {
                    let mut br = MemoryReader::new_read(&src);
                    validate!(br.left() > 3);
                    decode_lz_data(&mut br, &mut self.syms)?;
                },
                _ => unreachable!(),
            }
            reconstruct_frame(data, width, height, stride, &self.syms)?;
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for MicroWaveletDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(MicroWaveletDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::misc_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_microwavelet() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);
        // sample from NeXTIME CD
        test_decoding("mov", "next-uwavelet", "assets/Misc/Omni_98k.mov", Some(2), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x2978b089, 0xee15118e, 0xe89b2887, 0x1d170b42],
                            [0xd7dcc30b, 0xa0eab984, 0xd97cf4cf, 0xadb4172b],
                            [0xe2ec789e, 0xf0791284, 0x8286e1f8, 0x6550d8cd]]));
    }
}
