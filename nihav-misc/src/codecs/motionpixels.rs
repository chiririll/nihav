use nihav_core::codecs::*;
use nihav_core::io::bitreader::*;
use nihav_core::io::codebook::*;
use nihav_core::io::byteio::*;

mod data;
use data::*;
mod render;
use render::*;

#[derive(Clone,Copy,Debug,PartialEq)]
enum MPVersion {
    MotionPixelsDOS,
    MVI1,
    MVI2
}

impl MPVersion {
    fn is_dos(self) -> bool { self == MPVersion::MotionPixelsDOS }
}

const BGR555_FORMAT: NAPixelFormaton = NAPixelFormaton { model: ColorModel::RGB(RGBSubmodel::RGB), components: 3,
                                        comp_info: [
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift: 10, comp_offs: 0, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  5, comp_offs: 0, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  0, comp_offs: 0, next_elem: 2 }),
                                            None, None],
                                        elem_size: 2, be: false, alpha: false, palette: false };

#[derive(Clone,Copy,Debug,Default)]
struct MPFlags(u16);

impl MPFlags {
    fn get_mode_idx(self) -> usize { usize::from(self.0 & 0xF) }
    fn has_dos_interframes(self) -> bool { (self.0 & 0x200) != 0 }
    fn is_extended(self) -> bool { (self.0 & 0x200) != 0 }
    fn has_koda(self) -> bool { (self.0 & 0x400) != 0 }
    fn has_lowres(self) -> bool { (self.0 & 0x800) != 0 }
    fn has_golden_frame(self) -> bool { (self.0 & 0x1000) != 0 }
    fn is_flipped(self) -> bool { (self.0 & 0x2000) == 0 }

    fn set_koda(&mut self, val: bool) {
        if val {
            self.0 |= 0x400;
        } else {
            self.0 &= !0x400;
        }
    }
    fn set_lowres(&mut self, val: bool) {
        if val {
            self.0 |= 0x800;
        } else {
            self.0 &= !0x800;
        }
    }
}

#[derive(Clone,Copy,Debug,PartialEq)]
enum MapType {
    Skip,
    Fill,
    GF,
    LoRes,
}

#[derive(Default)]
pub struct MPCodeDesc {
    bits:   [u8; 16],
    codes:  [u16; 16],
    syms:   [i8; 16],
    size:   usize,
}

enum MPCodebook {
    Single(i8),
    Codebook(Codebook<i8>),
}

#[allow(clippy::wrong_self_convention)]
pub trait IsEscDelta {
    fn is_escape(self) -> bool;
}

impl IsEscDelta for i8 {
    fn is_escape(self) -> bool { self.unsigned_abs() == 7 }
}

#[derive(Default)]
pub struct MPDeltas {
    pub dcount:     usize,
    pub dcount2:    usize,
    pub cur_end:    usize,
    pub end2:       usize,
    pub deltas:     Vec<i8>,
}

impl MPDeltas {
    pub fn next(&mut self) -> DecoderResult<i8> {
        validate!(self.dcount < self.deltas.len() && self.dcount < self.cur_end);
        let ret = self.deltas[self.dcount];
        self.dcount += 1;
        Ok(ret)
    }
    pub fn next_smooth(&mut self) -> DecoderResult<i8> {
        let mut dval = self.next()?;
        let mut delta = dval;
        while dval.is_escape() {
            dval = self.next()?;
            delta += dval * 2;
            validate!(delta.unsigned_abs() < 64);
        }
        Ok(delta)
    }
    pub fn switch_line(&mut self) {
        std::mem::swap(&mut self.dcount, &mut self.dcount2);
        std::mem::swap(&mut self.cur_end, &mut self.end2);
    }
    fn set_parts(&mut self, delta_count1: usize) {
        self.cur_end = delta_count1;
        self.dcount2 = delta_count1;
        self.end2    = self.deltas.len();
    }
    fn check_end(&self) -> DecoderResult<()> {
        validate!(self.dcount == self.cur_end);
        if self.end2 != 0 {
            // allow up to two deltas to be not used
            validate!(self.dcount2 >= self.end2.saturating_sub(2));
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct MPFrame {
    pub frame:      Vec<u16>,
    pub gframe:     Vec<u16>,
    pub width:      usize,
    pub height:     usize,
    pub map:        Vec<u8>,
    pub lrmap:      Vec<bool>,
    pub obits:      u8,
    pub cbits:      u8,
    pub codes:      Vec<FullCodebookDesc<i8>>,
    pub cdesc:      MPCodeDesc,
    pub col_pred:   Vec<YuvPixel>,
    pub chroma:     Vec<YuvPixel>,
    pub dmod:       [DeltaMod; 3],
    pub deltas:     MPDeltas,
    pub recon:      ReconFuncs,
}

impl MPFrame {
    pub fn new() -> Self { Self::default() }
    pub fn init(&mut self, width: usize, height: usize, mode: RenderMode, is_dos: bool) {
        self.width   = width;
        self.height  = height;
        self.frame   = vec![0; width * height];
        self.map     = vec![0; width * height];
        self.lrmap   = vec![false; width * height];
        self.col_pred = vec![YuvPixel::default(); height];
        self.chroma = vec![YuvPixel::default(); width];
        if !is_dos {
            self.gframe = vec![0; width * height];
        }

        self.obits = 1;
        while (1 << self.obits) < width * height {
            self.obits += 1;
        }
        self.cbits = if width * height > 320 * 240 { 19 } else { 18 };

        self.recon = mode.into();
    }
    pub fn reset_map(&mut self) {
        for el in self.map.iter_mut() {
            *el = 0;
        }
        for el in self.lrmap.iter_mut() {
            *el = false;
        }
    }
    fn read_map(&mut self, br: &mut BitReader, mtype: MapType, large: bool, count: usize) -> DecoderResult<()> {
        let bits = if large { 8 } else { 4 };

        for _ in 0..count {
            let offset = br.read(self.obits)? as usize;
            let tw     = br.read(bits)? as usize + 1;
            let th     = br.read(bits)? as usize + 1;
            self.fill_map_rect(br, mtype, offset, tw, th)?;
        }

        Ok(())
    }
    fn read_mvi2_map(&mut self, br: &mut BitReader, mtype: MapType) -> DecoderResult<()> {
        for pos in (0..(self.width * self.height)).step_by(0x2000) {
            let count_large = br.read(8)? as usize;
            let count_small = br.read(8)? as usize;
            for _ in 0..count_large {
                self.read_mvi2_map_piece(br, mtype, pos, 8)?;
            }
            for _ in 0..count_small {
                self.read_mvi2_map_piece(br, mtype, pos, 4)?;
            }
        }
        Ok(())
    }
    fn read_mvi2_map_piece(&mut self, br: &mut BitReader, mtype: MapType, base: usize, bits: u8) -> DecoderResult<()> {
        let offset = br.read(13)? as usize + base;
        let tw = br.read(bits)? as usize + 1;
        let th = br.read(bits)? as usize + 1;
        self.fill_map_rect(br, mtype, offset, tw, th)
    }
    fn fill_map_rect(&mut self, br: &mut BitReader, mtype: MapType, offset: usize, tw: usize, th: usize) -> DecoderResult<()> {
        validate!(tw != 256);

        let xoff = offset % self.width;
        let yoff = offset / self.width;
        let w = tw.min(self.width - xoff);
        let h = th.min(self.height.saturating_sub(yoff));
        for line in self.map.chunks_exact_mut(self.width).skip(yoff).take(h) {
            for el in line[xoff..][..w].iter_mut() {
                *el = w as u8;
            }
        }
        if mtype == MapType::Fill {
            let pix = br.read(15)? as u16;
            for line in self.frame.chunks_exact_mut(self.width).skip(yoff).take(h) {
                for el in line[xoff..][..w].iter_mut() {
                    *el = pix;
                }
            }
        }
        if mtype == MapType::GF {
            for (dline, sline) in self.frame.chunks_exact_mut(self.width)
                    .zip(self.gframe.chunks_exact(self.width)).skip(yoff).take(h) {
                dline[xoff..][..w].copy_from_slice(&sline[xoff..][..w]);
            }
        }
        if mtype == MapType::LoRes {
            for line in self.lrmap.chunks_exact_mut(self.width).skip(yoff).take(h) {
                for el in line[xoff..][..w].iter_mut() {
                    *el = true;
                }
            }
        }
        Ok(())
    }
    fn read_code(br: &mut BitReader, cdesc: &mut MPCodeDesc, mut code: u16, mut bits: u8) -> DecoderResult<()> {
        validate!(bits < 16);
        while br.read_bool()? {
            code <<= 1;
            bits  += 1;
            Self::read_code(br, cdesc, code | 1, bits)?;
        }
        validate!(cdesc.size < cdesc.bits.len());
        cdesc.codes[cdesc.size] = code;
        cdesc.bits[cdesc.size]  = bits;
        cdesc.size += 1;
        Ok(())
    }
    fn read_codes_desc(&mut self, br: &mut BitReader, ncodes: usize) -> DecoderResult<MPCodebook> {
        if ncodes == 1 {
            let def_val = br.read(4)? as i8 - 7;
            return Ok(MPCodebook::Single(def_val));
        }
        self.codes.clear();
        let code_bits = br.read(4)?;
        validate!(code_bits > 0);
        for sym in self.cdesc.syms.iter_mut().take(ncodes) {
            *sym = br.read(4)? as i8 - 7;
        }
        self.cdesc.size = 0;
        Self::read_code(br, &mut self.cdesc, 0, 0)?;
        validate!(self.cdesc.size == ncodes);
        for (&sym, (&code, &bits)) in self.cdesc.syms.iter()
                .zip(self.cdesc.codes.iter().zip(self.cdesc.bits.iter())).take(ncodes) {
            self.codes.push(FullCodebookDesc{ code: code.into(), bits, sym });
        }
        let mut cbr = FullCodebookDescReader::new(&self.codes);
        let cb = Codebook::new(&mut cbr, CodebookMode::MSB).map_err(|_| DecoderError::InvalidData)?;
        Ok(MPCodebook::Codebook(cb))
    }
    fn read_deltas(&mut self, br: &mut BitReader, mpcb: &MPCodebook, deltas: usize) -> DecoderResult<()> {
        self.deltas.dcount = 0;
        self.deltas.deltas.clear();
        match mpcb {
            MPCodebook::Single(val) => {
                self.deltas.deltas.resize(deltas, *val);
            },
            MPCodebook::Codebook(cb) => {
                self.deltas.deltas.reserve(deltas);
                for _ in 0..deltas {
                    let val = br.read_cb(cb)?;
                    self.deltas.deltas.push(val);
                }
            },
        }
        self.deltas.cur_end = deltas;
        self.deltas.end2 = 0;
        Ok(())
    }

    fn decode(&mut self, flags: &mut MPFlags, version: MPVersion, is_kf: bool, br: &mut BitReader) -> DecoderResult<bool> {
        let is_dos = version.is_dos();
        let is_intra = if !is_dos { br.read_bool()? } else { is_kf };
        let is_mvi2_2 = version == MPVersion::MVI2 && flags.is_extended();

        if is_intra && !is_dos && flags.is_extended() {
            let mode_idx = br.read(3)? as usize;
            let lowres = br.read_bool()?;
            let smooth = br.read_bool()?;

            let mode = if !smooth {
                    validate!(mode_idx < MVI_MODES.len());
                    MVI_MODES[mode_idx]
                } else {
                    validate!(mode_idx < MVI2_MODES.len());
                    MVI2_MODES[mode_idx]
                };
            match (mode, lowres) {
                (_, false) => {},
                (RenderMode::YUV4x4, true) => {},
                (RenderMode::YUV2x1Smooth, _) => {},
                (RenderMode::YUV2x2Smooth, _) => {},
                (RenderMode::YUV4x2Smooth, _) => {},
                (RenderMode::YUV4x4Smooth, _) => {},
                _ => {
                    println!("mode {mode:?} lowres={lowres} is not supported (yet)");
                    return Err(DecoderError::NotImplemented);
                }
            }
            self.recon = mode.into();

            flags.set_lowres(lowres);
            if br.read_bool()? {
                return Err(DecoderError::NotImplemented);
            }
            let subframe = br.read_bool()?;
            flags.set_koda(subframe);
        }

        if !is_dos && flags.has_koda() {
            return Err(DecoderError::NotImplemented);
        }

        self.reset_map();

        let has_skip_map = if !is_dos { !is_intra } else { flags.has_dos_interframes() };

        if !is_mvi2_2 {
            if has_skip_map {
                let large_count = br.read(12)? as usize;
                let small_count = br.read(12)? as usize;
                self.read_map(br, MapType::Skip, true,  large_count)?;
                self.read_map(br, MapType::Skip, false, small_count)?;
            }

            if !is_dos && flags.has_golden_frame() {
                let large_count = br.read(12)? as usize;
                let small_count = br.read(12)? as usize;
                self.read_map(br, MapType::GF, true,  large_count)?;
                self.read_map(br, MapType::GF, false, small_count)?;
            }

            let large_count = br.read(12)? as usize;
            let small_count = br.read(12)? as usize;
            self.read_map(br, MapType::Fill, true,  large_count)?;
            self.read_map(br, MapType::Fill, false, small_count)?;

            if !is_dos && flags.has_lowres() {
                let large_count = br.read(12)? as usize;
                let small_count = br.read(12)? as usize;
                self.read_map(br, MapType::LoRes, true,  large_count)?;
                self.read_map(br, MapType::LoRes, false, small_count)?;
            }
        } else {
            if has_skip_map {
                self.read_mvi2_map(br, MapType::Skip)?;
            }
            if flags.has_golden_frame() {
                self.read_mvi2_map(br, MapType::GF)?;
            }
            self.read_mvi2_map(br, MapType::Fill)?;
            if flags.has_lowres() {
                self.read_mvi2_map(br, MapType::LoRes)?;
            }
        }

        let ccount = br.read(4)? as usize;

        if ccount > 0 {
            if self.map[0] == 0 {
                self.frame[0] = br.read(15)? as u16;
                self.map[0] = 1;
            }
            let cb = self.read_codes_desc(br, ccount)?;
            let mut delta_count = br.read(self.cbits)? as usize;
            let delta_count1 = delta_count;
            delta_count += br.read(self.cbits)? as usize;
            self.read_deltas(br, &cb, delta_count)?;
            self.deltas.set_parts(delta_count1);
            if delta_count > 0 {
                (self.recon.col_pred)(self)?;
                (self.recon.line_pred)(self)?;
                self.deltas.check_end()?;
            }
        }

        Ok(is_intra)
    }

    fn render_frame(&self, dst: &mut [u16], stride: usize) {
        for (dline, sline) in dst.chunks_exact_mut(stride).zip(self.frame.chunks_exact(self.width)) {
            dline[..self.width].copy_from_slice(sline);
        }
    }
}

struct MVIDecoder {
    info:       NACodecInfoRef,
    flags:      MPFlags,
    dec:        MPFrame,
    version:    MPVersion,
    buf:        Vec<u8>,
}

impl MVIDecoder {
    fn new(version: MPVersion) -> Self {
        Self {
            info:       NACodecInfo::new_dummy(),
            flags:      MPFlags::default(),
            dec:        MPFrame::new(),
            version,
            buf:        Vec::new(),
        }
    }
}

impl NADecoder for MVIDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            validate!(((w | h) & 3) == 0);
            if let Some(edata) = info.get_extradata() {
                match self.version {
                    MPVersion::MotionPixelsDOS => {
                        validate!(edata.len() >= 2);
                    },
                    MPVersion::MVI1 => {
                        validate!(edata.len() >= 4);
                        validate!(&edata[2..4] == b"i1");
                    },
                    MPVersion::MVI2 => {
                        validate!(edata.len() >= 4);
                        validate!(&edata[2..4] == b"i2");
                    },
                };
                self.flags = MPFlags(read_u16le(&edata)?);
            } else {
                return Err(DecoderError::InvalidData);
            }
            let mode_idx = self.flags.get_mode_idx();
            let mode = match self.version {
                    MPVersion::MotionPixelsDOS => {
                        validate!(mode_idx < MVI0_MODES.len());
                        MVI0_MODES[mode_idx]
                    },
                    _ => {
                        validate!(mode_idx < MVI_MODES.len());
                        MVI_MODES[mode_idx]
                    }
                };
            // check for supported flag combinations
            match self.version {
                MPVersion::MotionPixelsDOS => {
                    if (self.flags.0 & !0x60F) != 0 {
                        println!("Unknown flags combination {:X}, decoding will likely produce wrong results", self.flags.0);
                    }
                },
                MPVersion::MVI1 => {
                    validate!(!(self.flags.is_extended() || self.flags.has_koda() || self.flags.has_lowres()));
                },
                MPVersion::MVI2 => {
                    if (self.flags.0 & !0x3E0F) != 0 {
                        println!("Unknown flags combination {:X}, decoding will likely produce wrong results", self.flags.0);
                    }
                    if self.flags.has_lowres() && mode != RenderMode::YUV4x4 {
                        println!("lowres mode for {mode:?} is not supported (yet)");
                        return Err(DecoderError::NotImplemented);
                    }
                },
            }
            self.dec.init(w, h, mode, self.version.is_dos());
            let flipped = !self.version.is_dos() && self.flags.is_flipped();
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, flipped, BGR555_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();

        let is_dos = self.version.is_dos();
        let is_kf = pkt.is_keyframe();

        let mut br = if is_dos {
                self.buf.clear();
                self.buf.extend_from_slice(&src[..src.len() & !3]);
                for quad in self.buf.chunks_exact_mut(4) {
                    quad.swap(0, 3);
                    quad.swap(1, 2);
                }
                if (src.len() & 3) != 0 {
                    let tail = src.len() & 3;
                    for i in (1..=tail).rev() {
                        self.buf.push(src[src.len() - i]);
                    }
                }
                BitReader::new(&self.buf, BitReaderMode::BE)
            } else {
                BitReader::new(&src, BitReaderMode::LE32MSB)
            };

        let is_intra = self.dec.decode(&mut self.flags, self.version, is_kf, &mut br)?;

        if is_intra && self.flags.has_golden_frame() && br.left() > 128 {
            let tail = br.tell() & 0x1F;
            if tail > 0 {
                br.skip((32 - tail) as u32)?;
            }
            while br.left() > 64 && (br.peek(32) & 0x80000000) == 0 {
                br.skip(32)?;
            }
            if br.left() > 64 {
                if self.dec.decode(&mut self.flags, self.version, is_kf, &mut br).is_ok() {
                    self.dec.gframe.copy_from_slice(&self.dec.frame);
                } else {
                    println!("Golden frame decoding failed, the rest of decoded video likely will not be correct");
                }
            } else {
                println!("Golden frame should've been here but it was not found, the rest of decoded video likely will not be correct");
            }
        }

        let vinfo = self.info.get_properties().get_video_info().unwrap();
        let bufinfo = alloc_video_buffer(vinfo, 0)?;

        if let Some(mut buf) = bufinfo.get_vbuf16() {
            let stride = buf.get_stride(0);
            let data = buf.get_data_mut().unwrap();

            self.dec.render_frame(data, stride);
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.dec.frame.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for MVIDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder0() -> Box<dyn NADecoder + Send> {
    Box::new(MVIDecoder::new(MPVersion::MotionPixelsDOS))
}

pub fn get_decoder1() -> Box<dyn NADecoder + Send> {
    Box::new(MVIDecoder::new(MPVersion::MVI1))
}

pub fn get_decoder2() -> Box<dyn NADecoder + Send> {
    Box::new(MVIDecoder::new(MPVersion::MVI2))
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_mvi0_2x1() {
        let mut dmx_reg = RegisteredDemuxers::new();
        misc_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from AD&D Core Rules CD
        test_decoding("mvi", "mvi0", "assets/Misc/MP/avtest.mvi", Some(10), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xfb8254ea, 0xdfd967df, 0x69a74f82, 0xd2103a40],
                            [0x7f762db9, 0x0d8949e7, 0xc66a7419, 0x464b1f82],
                            [0x32418d08, 0x787dacf9, 0xd236a771, 0x8309cfcb],
                            [0x171b70b9, 0x111ab2cd, 0xf54657cf, 0x02501c27],
                            [0x919ff0ac, 0x3b0f5598, 0x7b1e50f7, 0x60045d6d],
                            [0xb5b89223, 0x6111550d, 0xed4988e9, 0x97e9cb99],
                            [0x4eb650fe, 0x4df6ff00, 0xf21b549a, 0x7dd778c3],
                            [0xbc992f95, 0x7a46e897, 0x2bdabf65, 0x4095f81e],
                            [0x760284bc, 0x19cbd109, 0x54c90545, 0xf2752981],
                            [0x7180060c, 0x225470a8, 0x50d1faca, 0x001e2cb1],
                            [0xcd3d2bb9, 0xb4237176, 0xfb668edc, 0xce1c0a33]]));
    }
    #[test]
    fn test_mvi0_2x2() {
        let mut dmx_reg = RegisteredDemuxers::new();
        misc_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from AD&D Core Rules CD
        test_decoding("mvi", "mvi0", "assets/Misc/MP/tour.mvi", Some(100), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x05dbbd20, 0xd38dec75, 0x98e4ae3e, 0x255200ad],
                            [0x91b260c7, 0x749d2905, 0x38902276, 0xeb2e74cf],
                            [0xb4c618fd, 0xe76e591b, 0xc68ea1b4, 0x92bb4e36],
                            [0x52109f15, 0xf415826d, 0x7eca7235, 0xcbc6e403],
                            [0x88da1388, 0x5a9c04fe, 0x7515dc35, 0xd6b17c49],
                            [0x6416ad1d, 0x8b412357, 0x65c1d6b4, 0xa5b068a5],
                            [0xf3407271, 0xb56cf719, 0x958ce1ff, 0xb39ea5a9],
                            [0xc3c2500f, 0x5a51ecc6, 0xfa0495eb, 0xfd332905],
                            [0x1511ddb5, 0x909fab89, 0xdb568de1, 0xebb17e40],
                            [0xbb14afb9, 0xe1e030f7, 0x34bac5c1, 0x3b3050fd],
                            [0x9322e95f, 0x2dc979d4, 0xfdfeb99c, 0x0f192ccc],
                            [0x95870de7, 0x9d943d33, 0xe4de18eb, 0xeb8b35b6],
                            [0x77e3c415, 0x7d2684da, 0x62a1276e, 0xdb49212a],
                            [0x127c82bd, 0x14f399b1, 0xe8886b9a, 0x8b3873e8],
                            [0x8da7f5a1, 0xda4acfa4, 0x7ca6e5cd, 0x07c07ead],
                            [0x35075823, 0x56b47c66, 0x15276b43, 0x2c17b90f],
                            [0x96e5dfb7, 0xd6f32016, 0xc3b37838, 0x37d2ac09],
                            [0x6ef5387c, 0xf0656a5f, 0x32c367c1, 0x2ec4e4bc],
                            [0x1959f1e5, 0x8288de5f, 0x276c040b, 0x2a467c06],
                            [0x88a85aa0, 0xa5056299, 0xe1998ffb, 0xcbaa2ec5]]));
    }
    #[test]
    fn test_mvi0_4x4() {
        let mut dmx_reg = RegisteredDemuxers::new();
        misc_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from https://samples.mplayerhq.hu/mvi/INTRO.MVI
        test_decoding("mvi", "mvi0", "assets/Misc/MP/INTRO.MVI", Some(10), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xc073af04, 0xbbff079e, 0x87492974, 0xaf2c6ac8],
                            [0xf29cae10, 0x8eaf8abe, 0xf85373d5, 0xf1e633f9],
                            [0x7bd4312a, 0x03f05c9a, 0x2257c375, 0x3ab70de7],
                            [0x2f6d9a86, 0x8199f699, 0x13625d19, 0x808ae83d],
                            [0x5650b63e, 0x786b7abe, 0x63d5dafb, 0xf4d80406],
                            [0x9000dda2, 0xec440088, 0x2d4fb135, 0x7114199d],
                            [0x9132dd37, 0x7e8b3d00, 0x190899ba, 0xdfa75f5a],
                            [0xce0f74ec, 0x4c89b346, 0x506b461c, 0xa3429626],
                            [0xebb46261, 0x47946a2f, 0xf2fb77c0, 0x47777d53],
                            [0x16d6d31f, 0x239b0025, 0x00fbcd4a, 0xb57c22f8],
                            [0x9754f90c, 0x40fff3d8, 0x5daa2ed8, 0x4e2ba180]]));
    }
    #[test]
    fn test_mvi1_rgb() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Treasure Quest
        test_decoding("avi", "mvi1", "assets/Misc/MP/L1GS5.AVI", Some(10), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xd6b64e26, 0xd7399d9c, 0x96ca5a73, 0x9b3c28fb],
                            [0x4d61c3e7, 0x313501f7, 0x24ed1d80, 0x6faf672d],
                            [0x24a8e315, 0xebb56ce8, 0xf350f6bc, 0xa2be3036],
                            [0xb4d1d6f8, 0x6b5c9c74, 0x7a07220f, 0x749f1437],
                            [0x327d2952, 0x104d2d54, 0x78916063, 0x2326213b],
                            [0x55543e5a, 0xbda13a9f, 0xb3a9a833, 0x2e9b318c],
                            [0x047f12a4, 0xb6f2e202, 0x588c0b33, 0x8735239b],
                            [0x80a4ca20, 0xc0441b80, 0xf22db233, 0x27b74c44],
                            [0x84edf99a, 0x480ac8c9, 0xdf9ffb14, 0x363dd1ed],
                            [0xc0eff458, 0x829d0b9d, 0xf4d7e4f9, 0x784267af],
                            [0xfa71489e, 0xc60ba52c, 0x73846cde, 0x4d2e19e5]]));
    }
    #[test]
    fn test_mvi1_2x1() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Treasure Quest
        test_decoding("avi", "mvi1", "assets/Misc/MP/L3AS5.AVI", Some(25), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x3fb39db0, 0x98d44bcb, 0x5b235015, 0x53e8cfdb],
                            [0x438dc55f, 0x323358e0, 0x85bc40aa, 0x624c2132],
                            [0x31830363, 0x7e73a05e, 0xaa4b6e1f, 0x7a267520],
                            [0x6a12830d, 0x25f4c331, 0xcff6668c, 0xbbbabce8]]));
    }
    #[test]
    fn test_mvi1_2x2() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Treasure Quest
        test_decoding("avi", "mvi1", "assets/Misc/MP/BKGFILE2.AVI", None, &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xd87c4dea, 0xb4c0cf8f, 0x9d04dc3c, 0x3d528729],
                            [0xc6f44ac6, 0x53971edb, 0x235b4b8e, 0x3882987b],
                            [0x06ae8a01, 0xd80da962, 0xc7987c26, 0x4af64cec]]));
    }
    #[test]
    fn test_mvi1_4x2() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Apollo 18: The Moon Missions
        test_decoding("avi", "mvi1", "assets/Misc/MP/SHOT09.AVI", Some(10), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x056f36ec, 0x40850329, 0xd752807d, 0x8d3f2c27],
                            [0x780453db, 0x50eba6ee, 0xc78ecf9a, 0x279e4327],
                            [0x975c5e7a, 0x907bc160, 0xfeb80922, 0x630cc8c3],
                            [0xee70fa6b, 0xc0187089, 0x878840ab, 0x2ea9adba],
                            [0x12458eca, 0x9cdd2e4c, 0x9d2206a7, 0x2ac6713b],
                            [0x2fd8ec5f, 0x54214a76, 0x50e258a2, 0x647facf9],
                            [0xf0fef489, 0xb23eddc4, 0xfbf55d36, 0x17b41e74],
                            [0x5d8c94c0, 0xcdcc76d5, 0x7e3a78c2, 0x9c21f145],
                            [0x7ece4d2f, 0xee0675fc, 0xa23604e2, 0xa309ae69],
                            [0xf186402b, 0x144c79e1, 0x856d1e34, 0x008d472b],
                            [0x1c193e86, 0xdedd6339, 0x4f6f0a93, 0xb13fc38f]]));
    }
    #[test]
    fn test_mvi1_4x4() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Motion Pixels Release 2.7 Demonstration Software
        test_decoding("avi", "mvi1", "assets/Misc/MP/MVITEST.AVI", Some(10), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x06ae8a01, 0xd80da962, 0xc7987c26, 0x4af64cec],
                            [0x2979284c, 0xf5093343, 0x80062c55, 0x3608e56e],
                            [0xfdf18272, 0x9c06d1ba, 0xba61fd6e, 0xf2e266e4],
                            [0xccd0127c, 0x22f41d96, 0xeb67e1be, 0xb76b731b],
                            [0x63c4bf7d, 0x06687bd7, 0xa70789af, 0x40909d57],
                            [0xbf3a9ce5, 0xed9931d5, 0x003831aa, 0x69a317a7],
                            [0xefda6eaa, 0xbe6468eb, 0x3a53b1c8, 0xfbd530e7],
                            [0x3e8fbc7b, 0x8cc8f8a3, 0xf711f02b, 0xd7db134e],
                            [0xadfcf40f, 0x00d44d7e, 0xe90d9e8b, 0x77d74a90],
                            [0xc11bbc22, 0xd5188139, 0xce362409, 0x79624acc],
                            [0xb1e1e0ce, 0xa133684f, 0x84c55437, 0x0d32b987]]));
    }
    #[test]
    fn test_mvi1_gf() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Treasure Quest
        test_decoding("avi", "mvi1", "assets/Misc/MP/L2KPU.AVI", Some(20), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0xc8bfb97b, 0x3f80ca2b, 0x878ea600, 0x7111c1be],
                            [0x2eccc798, 0xc4cec880, 0x2f0919c4, 0xf26fd8b2],
                            [0x79d01f01, 0x8f22abeb, 0xc5192da6, 0x58fd4e5d],
                            [0x627f11e0, 0x01767fb0, 0xe98908bb, 0x8f5487c2],
                            [0x9fe2ef77, 0x3afc3352, 0xd7ac6e34, 0xe726705c],
                            [0xaeff0b5d, 0x1946d649, 0xd1cb8c08, 0x4fc27b5a],
                            [0xc945be0c, 0x14a2a3df, 0x9fcb2aab, 0x5cca962a]]));
    }
    #[test]
    fn test_mvi2_lowres() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Power Moves movieCD
        test_decoding("avi", "mvi2", "assets/Misc/MP/sirius.avi", Some(10), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x06ae8a01, 0xd80da962, 0xc7987c26, 0x4af64cec],
                            [0xe1a92b86, 0x821c055c, 0xfa5a60c7, 0xef815230],
                            [0x47452c0d, 0x4cc28125, 0x704b2052, 0x067adac6],
                            [0x9b3adacc, 0x3bae3514, 0xd3291c4d, 0xfb85cb73],
                            [0x04cf704a, 0x28ad007c, 0xa5eaed56, 0xafdb5268],
                            [0xa6374374, 0x5367418f, 0x5f395f72, 0xc6a92c04],
                            [0x0936bc88, 0x36df7c7c, 0xd557fdb1, 0xec938c89],
                            [0x8772aea4, 0xc3d408ff, 0x0c18921b, 0x13eef346],
                            [0x57433387, 0xec934411, 0x0918f310, 0x1009bdf1],
                            [0xe74ece43, 0x8b7c3515, 0x164ca241, 0xebf8bb74],
                            [0xedf08649, 0xd98e4ace, 0xa2d0286a, 0x68e43268]]));
    }
    #[test]
    fn test_mvi2_ext() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from https://samples.mplayerhq.hu/V-codecs/MVI2/SEVEN1.AVI
        test_decoding("avi", "mvi2", "assets/Misc/MP/SEVEN1.AVI", Some(6), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xa02d931f, 0xeab83f4f, 0x329e4d3c, 0xef994028],
                            [0x7d1a5752, 0x5b0b5a30, 0xe4fdbe93, 0xc37f842e],
                            [0xce0947d4, 0x7a53c495, 0xe9215ff8, 0x7d475bb4],
                            [0x97e3590f, 0x88e9ef4e, 0x758e67d2, 0x1ebd6fb6],
                            [0xf394acbc, 0x2a6849a7, 0x3fdc4d47, 0xf51d251b],
                            [0x9e07a0a0, 0xd5d55691, 0xec841f6b, 0x9aeee3f5],
                            [0x9e07a0a0, 0xd5d55691, 0xec841f6b, 0x9aeee3f5]]));
    }
}
