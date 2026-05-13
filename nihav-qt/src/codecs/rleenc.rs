use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

const RGB555BE_FORMAT: NAPixelFormaton = NAPixelFormaton::make_rgb16_fmt(5, 5, 5, true, true);
const ARGB_FORMAT: NAPixelFormaton = NAPixelFormaton {
        model: ColourModel::RGB(RGBSubmodel::RGB), components: 4,
        comp_info: [
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 1, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 2, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 3, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 0, next_elem: 4 }),
            None ],
        elem_size: 4, be: false, alpha: true, palette: false };

#[derive(Clone,Copy,Default,PartialEq)]
enum Strategy {
    Dumb,
    #[default]
    Greedy,
    Slow,
}

#[derive(Clone,Copy,PartialEq)]
enum EncState {
    Skip(u8),
    Run(usize, u8),
    Copy(usize, u8),
}

impl EncState {
    fn is_skip(&self) -> bool { matches!(*self, EncState::Skip(_)) }
    fn write_token(&self, dst: &mut Vec<u8>, line: &[u8], pix_size: usize) {
        match *self {
            EncState::Skip(skip) => { dst.push(skip + 1); },
            EncState::Run(_, 0) => {},
            EncState::Run(pos, 1) => {
                dst.push(1);
                dst.extend_from_slice(&line[pos * pix_size..][..pix_size]);
            },
            EncState::Run(pos, len) => {
                dst.push(0u8.wrapping_sub(len));
                dst.extend_from_slice(&line[pos * pix_size..][..pix_size]);
            },
            EncState::Copy(pos, len) => {
                dst.push(len);
                dst.extend_from_slice(&line[pos * pix_size..][..pix_size * usize::from(len)]);
            },
        }
    }
}

#[derive(Clone,Copy)]
struct Trellis {
    state:      EncState,
    prev:       usize,
    cost:       u32,
}

const MAX_COST: u32 = u32::MAX;
const INVALID_NODE: Trellis = Trellis { state: EncState::Skip(0), prev: 0, cost: MAX_COST };

#[derive(Default)]
struct TrellisHelper {
    skips:      Vec<bool>,
    runs:       Vec<bool>,
    trellis:    Vec<[Trellis; 3]>,
    stack:      Vec<EncState>,
}

struct RunLengthEncoder {
    stream:     Option<NAStreamRef>,
    pkt:        Option<NAPacket>,
    key_int:    u8,
    frm_no:     u8,
    lpal:       Arc<[u8; 1024]>,
    last_frm:   Vec<u8>,
    cur_frm:    Vec<u8>,
    pstride:    usize,
    vinfo:      NAVideoInfo,
    mode:       Strategy,
    helper:     TrellisHelper,
}

impl RunLengthEncoder {
    fn new() -> Self {
        Self {
            stream:     None,
            pkt:        None,
            key_int:    25,
            frm_no:     254,
            lpal:       Arc::new([0; 1024]),
            last_frm:   Vec::new(),
            cur_frm:    Vec::new(),
            pstride:    0,
            vinfo:      NAVideoInfo::new(0, 0, false, RGB24_FORMAT),
            mode:       Strategy::default(),
            helper:     TrellisHelper::default(),
        }
    }
    fn pack_frame(&mut self, src: &[u8], stride: usize) {
        self.cur_frm.clear();
        let mut gw = GrowableMemoryWriter::new_write(&mut self.cur_frm);
        match self.vinfo.bits {
            1 => {
                self.pstride = self.vinfo.width / 8;
                for line in src.chunks_exact(stride).take(self.vinfo.height) {
                    for p16 in line.chunks_exact(16).take(self.vinfo.width / 16) {
                        let mut pp = 0u16;
                        for &pix in p16.iter() {
                            pp = pp * 2 + u16::from(pix);
                        }
                        let _ = gw.write_u16be(pp);
                    }
                }
            },
            2 => {
                self.pstride = self.vinfo.width / 4;
                for line in src.chunks_exact(stride).take(self.vinfo.height) {
                    for p16 in line.chunks_exact(16).take(self.vinfo.width / 16) {
                        let mut pp = 0u32;
                        for &pix in p16.iter() {
                            pp = (pp << 2) + u32::from(pix);
                        }
                        let _ = gw.write_u32be(pp);
                    }
                }
            },
            4 => {
                self.pstride = self.vinfo.width / 2;
                for line in src.chunks_exact(stride).take(self.vinfo.height) {
                    for p8 in line.chunks_exact(8).take(self.vinfo.width / 8) {
                        let mut pp = 0u32;
                        for &pix in p8.iter() {
                            pp = (pp << 4) + u32::from(pix);
                        }
                        let _ = gw.write_u32be(pp);
                    }
                }
            },
            8 | 15 | 16 | 24 | 32 => {
                let bpp = usize::from(self.vinfo.bits + 1) / 8;
                self.pstride = self.vinfo.width * bpp;
                for line in src.chunks_exact(stride).take(self.vinfo.height) {
                    let _ = gw.write_buf(&line[..self.pstride]);
                }
            },
            _ => unreachable!(),
        }
        let _ = gw.flush();
    }
    fn pack_frame16(&mut self, src: &[u16], stride: usize) {
        self.cur_frm.clear();
        let mut gw = GrowableMemoryWriter::new_write(&mut self.cur_frm);
        self.pstride = self.vinfo.width * 2;
        for line in src.chunks_exact(stride).take(self.vinfo.height) {
            let _ = gw.write_u16be_arr(&line[..self.vinfo.width]);
        }
        let _ = gw.flush();
    }
    fn encode(&mut self, dst: &mut Vec<u8>, force_intra: bool) -> bool {
        match self.vinfo.bits {
            1 => unimplemented!(),
            2 | 4 | 8 => self.encode_generic::<4>(dst, force_intra),
            15 | 16 =>   self.encode_generic::<2>(dst, force_intra),
            24 =>        self.encode_generic::<3>(dst, force_intra),
            32 =>        self.encode_generic::<4>(dst, force_intra),
            _ => unreachable!(),
        }
    }
    fn encode_generic<const PIX_SIZE: usize>(&mut self, dst: &mut Vec<u8>, force_intra: bool) -> bool {
        let mut is_intra = true;
        let mut start = 0;
        let mut height = self.vinfo.height;
        if !force_intra && self.mode != Strategy::Dumb {
            for (line, pline) in self.cur_frm.chunks_exact(self.pstride)
                    .zip(self.last_frm.chunks_exact(self.pstride)) {
                if line == pline {
                    start += 1;
                } else {
                    break;
                }
            }
            if start == height {
                Self::encode_skip_frame(dst);
                return false;
            }
            height -= start;
            for (line, pline) in self.cur_frm.chunks_exact(self.pstride)
                    .zip(self.last_frm.chunks_exact(self.pstride)).skip(start).rev() {
                if line == pline {
                    height -= 1;
                } else {
                    break;
                }
            }
        }
        if start != 0 || height != self.vinfo.height {
            dst[5] |= 0x8;

            let mut hdr = [0; 8];
            let _ = write_u16be(&mut hdr[0..], start as u16);
            let _ = write_u16be(&mut hdr[4..], height as u16);
            dst.extend_from_slice(&hdr);
        }
        for (line, pline) in self.cur_frm.chunks_exact(self.pstride)
                .zip(self.last_frm.chunks_exact(self.pstride)).skip(start).take(height) {
            if !force_intra && line == pline {
                dst.push(0x01);
                dst.push(0xFF);
                is_intra = false;
                continue;
            }
            is_intra &= match self.mode {
                    Strategy::Dumb => Self::encode_line_dumb::<PIX_SIZE>(dst, line, pline, force_intra),
                    Strategy::Greedy => Self::encode_line_greedy::<PIX_SIZE>(dst, line, pline, force_intra),
                    Strategy::Slow => Self::encode_line_slow::<PIX_SIZE>(dst, line, pline, force_intra, &mut self.helper),
                };
        }
        dst.push(0x00); // end of frame
        is_intra
    }
    fn encode_line_dumb<const PIX_SIZE: usize>(dst: &mut Vec<u8>, line: &[u8], _pline: &[u8], _force_intra: bool) -> bool {
        dst.push(1); // skip mode off
        for chunk in line.chunks(PIX_SIZE * 127) {
            let len = (chunk.len() / PIX_SIZE) as u8;
            dst.push(len);
            dst.extend_from_slice(chunk);
        }
        dst.push(0xFF); // run code at the end of line
        true
    }
    fn encode_line_greedy<const PIX_SIZE: usize>(dst: &mut Vec<u8>, line: &[u8], pline: &[u8], force_intra: bool) -> bool {
        let mut is_intra = true;
        let mut state = EncState::Skip(0);

        for (i, (cpix, ppix)) in line.chunks_exact(PIX_SIZE)
                .zip(pline.chunks_exact(PIX_SIZE)).enumerate() {
            let cur_skip = !force_intra && cpix == ppix;

            state = match (state, cur_skip) {
                    (EncState::Skip(254), _) | (EncState::Skip(_), false) => {
                        state.write_token(dst, line, PIX_SIZE);
                        EncState::Run(i, 1)
                    },
                    (EncState::Skip(skip), true) => {
                        is_intra = false;
                        EncState::Skip(skip + 1)
                    },
                    (EncState::Run(_, 128), _) | (EncState::Copy(_, 127), _) => {
                        state.write_token(dst, line, PIX_SIZE);
                        EncState::Run(i, 1)
                    },
                    (EncState::Run(_, _), true) | (EncState::Copy(_, _), true) => {
                        state.write_token(dst, line, PIX_SIZE);
                        dst.push(0);
                        is_intra = false;
                        EncState::Skip(1)
                    },
                    (EncState::Run(pos, len), false) => {
                        if cpix == &line[pos * PIX_SIZE..][..PIX_SIZE] {
                            EncState::Run(pos, len + 1)
                        } else if len > 1 {
                            state.write_token(dst, line, PIX_SIZE);
                            EncState::Run(i, 1)
                        } else {
                            EncState::Copy(pos, 2)
                        }
                    },
                    (EncState::Copy(pos, len), false) => {
                        if len > 2 && cpix == &line[(pos + usize::from(len - 1)) * PIX_SIZE..][..PIX_SIZE] {
                            let tmp = EncState::Copy(pos, len - 1);
                            tmp.write_token(dst, line, PIX_SIZE);
                            EncState::Run(i - 1, 2)
                        } else {
                            EncState::Copy(pos, len + 1)
                        }
                    },
                };
        }
        if state.is_skip() {
            is_intra = false;
        }
        state.write_token(dst, line, PIX_SIZE);
        dst.push(0xFF); // run code at the end of line

        is_intra
    }
    fn encode_line_slow<const PIX_SIZE: usize>(dst: &mut Vec<u8>, line: &[u8], pline: &[u8], force_intra: bool, helper: &mut TrellisHelper) -> bool {
        helper.skips.clear();
        if force_intra {
            helper.skips.resize(line.len() / PIX_SIZE, false);
        } else {
            for (cpix, ppix) in line.chunks_exact(PIX_SIZE).zip(pline.chunks_exact(PIX_SIZE)) {
                helper.skips.push(cpix == ppix);
            }
        }
        helper.runs.clear();
        helper.runs.push(true);
        for (cpix, ppix) in line.chunks_exact(PIX_SIZE).skip(1).zip(line.chunks_exact(PIX_SIZE)) {
            helper.runs.push(cpix == ppix);
        }
        let trellis = &mut helper.trellis;
        trellis.clear();
        trellis.resize(helper.runs.len() + 1, [INVALID_NODE; 3]);
        trellis[0][0] = Trellis{ state: EncState::Skip(0), prev: 0, cost: 1 };

        let pix_cost = PIX_SIZE as u32;
        for pos in 0..trellis.len() - 1 {
            for i in 0..3 {
                if trellis[pos][i].cost == MAX_COST { continue; }
                for skip in 1..=254 {
                    if pos + skip > helper.skips.len() || !helper.skips[pos + skip - 1] {
                        break;
                    }
                    let cost = if pos != 0 { 2 } else { 0 };
                    if trellis[pos + skip][0].cost > trellis[pos][i].cost + cost {
                        trellis[pos + skip][0].cost = trellis[pos][i].cost + cost;
                        trellis[pos + skip][0].prev = pos * 4 + i;
                        trellis[pos + skip][0].state = EncState::Skip(skip as u8);
                    }
                }
                for run in 2..=128 {
                    if pos + run > helper.runs.len() || !helper.runs[pos + run - 1] {
                        break;
                    }
                    let cost = 1 + pix_cost;
                    if trellis[pos + run][1].cost > trellis[pos][i].cost + cost {
                        trellis[pos + run][1].cost = trellis[pos][i].cost + cost;
                        trellis[pos + run][1].prev = pos * 4 + i;
                        trellis[pos + run][1].state = EncState::Run(pos, run as u8);
                    }
                }
                for copy in 1..=127 {
                    if pos + copy > helper.runs.len() {
                        break;
                    }
                    let cost = 1 + pix_cost * (copy as u32);
                    if trellis[pos + copy][2].cost > trellis[pos][i].cost + cost {
                        trellis[pos + copy][2].cost = trellis[pos][i].cost + cost;
                        trellis[pos + copy][2].prev = pos * 4 + i;
                        trellis[pos + copy][2].state = EncState::Copy(pos, copy as u8);
                    }
                }
            }
        }

        helper.stack.clear();
        let mut pos = (helper.trellis.len() - 1) * 4;
        let mut best_cost = MAX_COST;
        for (i, nn) in helper.trellis[helper.trellis.len() - 1].iter().enumerate() {
            if nn.cost < best_cost {
                pos = (pos & !3) | i;
                best_cost = nn.cost;
            }
        }
        while pos > 0 {
            let node = helper.trellis[pos >> 2][pos & 3];
            helper.stack.push(node.state);
            pos = node.prev;
        }
        if !helper.stack[helper.stack.len() - 1].is_skip() {
            helper.stack.push(EncState::Skip(0));
        }

        let mut skip_mode = true;
        let mut is_intra = true;
        //xxx: skip last codes if they are skips?
        for state in helper.stack.iter().rev() {
            if state.is_skip() {
                if *state != EncState::Skip(0) {
                    is_intra = false;
                }
                if !skip_mode {
                    dst.push(0);
                }
                skip_mode = false;
            }
            state.write_token(dst, line, PIX_SIZE);
        }
        dst.push(0xFF); // run code at the end of line
        is_intra
    }

    fn encode_skip_frame(dst: &mut Vec<u8>) {
        dst[5] |= 0x8;
        for _ in 0..8 { // zero bounding rect
            dst.push(0);
        }
    }
}

fn get_fmt_and_align(bits: u8) -> (NAPixelFormaton, usize) {
    match bits {
        1 => (PAL8_FORMAT, 16),
        2 => (PAL8_FORMAT, 16),
        4 => (PAL8_FORMAT, 8),
        8 => (PAL8_FORMAT, 4),
        15 | 16 => (RGB555BE_FORMAT, 1),
        24 => (RGB24_FORMAT, 1),
        32 => (ARGB_FORMAT, 1),
        _ => (RGB24_FORMAT, 1),
    }
}

impl NAEncoder for RunLengthEncoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                Ok(EncodeParameters {
                    format: NACodecTypeInfo::Video(NAVideoInfo::new(0, 0, false, RGB24_FORMAT)),
                    ..Default::default()
                })
            },
            NACodecTypeInfo::Video(vinfo) => {
                let bits = if vinfo.format.is_paletted() {
                        vinfo.bits.min(8)
                    } else if vinfo.format.model.is_rgb() {
                        vinfo.format.get_total_depth()
                    } else if vinfo.format.has_alpha() {
                        32
                    } else {
                        24
                    };
                let (fmt, align) = get_fmt_and_align(bits);
                let mut info = *encinfo;
                if let NACodecTypeInfo::Video(ref mut vinfo) = info.format {
                    vinfo.format = fmt;
                    vinfo.width = (vinfo.width + align - 1) & !(align - 1);
                    vinfo.bits = bits;
                    vinfo.flipped = false;
                } else {
                    return Err(EncoderError::FormatError);
                }
                Ok(info)
            },
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
        }
    }
    fn get_capabilities(&self) -> u64 { ENC_CAPS_SKIPFRAME }
    fn init(&mut self, stream_id: u32, encinfo: EncodeParameters) -> EncoderResult<NAStreamRef> {
        match encinfo.format {
            NACodecTypeInfo::None => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(vinfo) => {
                self.vinfo = vinfo;
                let bits = if vinfo.format.is_paletted() {
                        vinfo.bits.min(8)
                    } else if vinfo.format.model.is_rgb() {
                        vinfo.format.get_total_depth()
                    } else if vinfo.format.has_alpha() {
                        32
                    } else {
                        24
                    };
                self.vinfo.bits = bits;
                self.vinfo.flipped = false;
                let (fmt, align) = get_fmt_and_align(bits);
                if vinfo.format != fmt || (vinfo.width & (align - 1)) != 0 {
                    return Err(EncoderError::FormatError);
                }
                let info = NACodecInfo::new("qt-rle", encinfo.format, None);
                let mut stream = NAStream::new(StreamType::Video, stream_id, info, encinfo.tb_num, encinfo.tb_den, 0);
                stream.set_num(stream_id as usize);
                let stream = stream.into_ref();
                self.stream = Some(stream.clone());
                Ok(stream)
            }
        }
    }
    fn encode(&mut self, frm: &NAFrame) -> EncoderResult<()> {
        let buf = frm.get_buffer();
        if let Some(vinfo) = buf.get_video_info() {
            if vinfo != self.vinfo {
                println!("Input format differs from the initial one");
                return Err(EncoderError::FormatError);
            }
        }
        self.frm_no += 1;
        let force_intra = self.frm_no >= self.key_int;

        let mut dbuf = vec![0x00, 0, 0, 0, 0, 0]; // ID, 24-bit frame size and 16-bit flags placeholder
        let is_intra = if let Some(vbuf) = buf.get_vbuf() {
                let src = vbuf.get_data();
                let stride = vbuf.get_stride(0);

                self.pack_frame(src, stride);
                if self.last_frm.is_empty() {
                    self.last_frm.extend_from_slice(&self.cur_frm);
                }

                self.encode(&mut dbuf, force_intra)
            } else if let Some(vbuf16) = buf.get_vbuf16() {
                let src = vbuf16.get_data();
                let stride = vbuf16.get_stride(0);

                self.pack_frame16(src, stride);
                if self.last_frm.is_empty() {
                    self.last_frm.extend_from_slice(&self.cur_frm);
                }

                self.encode(&mut dbuf, force_intra)
            } else if matches!(buf, NABufferType::None) {
                self.cur_frm.clear();
                self.cur_frm.extend_from_slice(&self.last_frm);
                if force_intra {
                    if self.cur_frm.is_empty() {
                        return Err(EncoderError::InvalidParameters);
                    }
                    self.encode(&mut dbuf, force_intra);
                    true
                } else {
                    Self::encode_skip_frame(&mut dbuf);
                    false
                }
            } else {
                return Err(EncoderError::FormatError);
            };
        std::mem::swap(&mut self.last_frm, &mut self.cur_frm);

        if is_intra {
            self.frm_no = 0;
            dbuf[0] = 0x40;
        }

        let size = dbuf.len() as u32;
        let _ = write_u24be(&mut dbuf[1..], size);

        let mut pkt = NAPacket::new(self.stream.clone().unwrap(), frm.ts, is_intra, dbuf);
        if let Some(vbuf) = buf.get_vbuf() {
            if self.vinfo.bits <= 8 {
                let mut npal = [0; 1024];
                let src = vbuf.get_data();
                for (dst, src) in npal.chunks_exact_mut(4).zip(src[vbuf.get_offset(1)..].chunks_exact(3)) {
                    dst[..3].copy_from_slice(src);
                }
                let new_pal = npal != *self.lpal;
                if new_pal {
                    self.lpal = Arc::new(npal);
                }
                pkt.add_side_data(NASideData::Palette(new_pal, Arc::clone(&self.lpal)));
            }
        }

        self.pkt = Some(pkt);
        Ok(())
    }
    fn get_packet(&mut self) -> EncoderResult<Option<NAPacket>> {
        let mut npkt = None;
        std::mem::swap(&mut self.pkt, &mut npkt);
        Ok(npkt)
    }
    fn flush(&mut self) -> EncoderResult<()> {
        Ok(())
    }
}

const MODE_OPTION: &str = "mode";

const ENCODER_OPTS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name: KEYFRAME_OPTION, description: KEYFRAME_OPTION_DESC,
        opt_type: NAOptionDefinitionType::Int(Some(0), Some(128)) },
    NAOptionDefinition {
        name: MODE_OPTION, description: "encoding strategy",
        opt_type: NAOptionDefinitionType::String(Some(&["dumb", "greedy", "slow"])) },
];

impl NAOptionHandler for RunLengthEncoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { ENCODER_OPTS }
    fn set_options(&mut self, options: &[NAOption]) {
        for option in options.iter() {
            for opt_def in ENCODER_OPTS.iter() {
                if opt_def.check(option).is_ok() {
                    match option.name {
                        KEYFRAME_OPTION => {
                            if let NAValue::Int(intval) = option.value {
                                self.key_int = intval as u8;
                            }
                        },
                        MODE_OPTION => {
                            if let NAValue::String(ref strval) = option.value {
                                match strval.as_str() {
                                    "dumb"   => { self.mode = Strategy::Dumb; },
                                    "greedy" => { self.mode = Strategy::Greedy; },
                                    "slow"   => { self.mode = Strategy::Slow; },
                                    _ => {},
                                }
                            }
                        },
                        _ => {},
                    }
                }
            }
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match name {
            KEYFRAME_OPTION => Some(NAValue::Int(i64::from(self.key_int))),
            MODE_OPTION => {
                match self.mode {
                    Strategy::Dumb => Some(NAValue::String("dumb".to_string())),
                    Strategy::Greedy => Some(NAValue::String("greedy".to_string())),
                    Strategy::Slow => Some(NAValue::String("slow".to_string())),
                }
            },
            _ => None,
        }
    }
}

pub fn get_encoder() -> Box<dyn NAEncoder + Send> {
    Box::new(RunLengthEncoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::*;
    use nihav_core::demuxers::*;
    use nihav_core::muxers::*;
    use nihav_commonfmt::*;
    use nihav_codec_support::test::enc_video::*;
    use crate::*;

    // samples from https://samples.mplayerhq.hu/V-codecs/QTRLE
    fn test_core(in_name: &'static str, format: NAPixelFormaton, enc_options: &[NAOption], hash: &[u32; 4]) {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);
        let mut mux_reg = RegisteredMuxers::new();
        generic_register_all_muxers(&mut mux_reg);
        let mut enc_reg = RegisteredEncoders::new();
        qt_register_all_encoders(&mut enc_reg);

        let dec_config = DecoderTestParams {
                demuxer:        "mov",
                in_name,
                stream_type:    StreamType::Video,
                limit:          Some(4),
                dmx_reg, dec_reg,
            };
        let enc_config = EncoderTestParams {
                muxer:          "mov",
                enc_name:       "qt-rle",
                out_name:       "qt_rle.mov",
                mux_reg, enc_reg,
            };
        let dst_vinfo = NAVideoInfo {
                width:   0,
                height:  0,
                format,
                flipped: false,
                bits:    format.get_total_depth(),
            };
        let enc_params = EncodeParameters {
                format:  NACodecTypeInfo::Video(dst_vinfo),
                quality: 0,
                bitrate: 0,
                tb_num:  0,
                tb_den:  0,
                flags:   0,
            };
        //test_encoding_to_file(&dec_config, &enc_config, enc_params, enc_options);
        test_encoding_md5(&dec_config, &enc_config, enc_params, enc_options, hash);
    }

    #[test]
    fn test_qt_rle_16bit_modes() {
        test_core("assets/QT/Animation-Highcolour.mov", super::RGB555BE_FORMAT,
                  &[NAOption{ name: "mode", value: NAValue::String("dumb".to_string()) }],
                  &[0x1719ce85, 0x88ffd54c, 0xbaf89408, 0x097b117b]);
        test_core("assets/QT/Animation-Highcolour.mov", super::RGB555BE_FORMAT,
                  &[NAOption{ name: "mode", value: NAValue::String("greedy".to_string()) }],
                  &[0x44f517f1, 0x42dc363f, 0xfc5ddc45, 0xc5aaf0f9]);
        test_core("assets/QT/Animation-Highcolour.mov", super::RGB555BE_FORMAT,
                  &[NAOption{ name: "mode", value: NAValue::String("slow".to_string()) }],
                  &[0x0e112054, 0x8053df6a, 0xcc17a897, 0x95ef202c]);
    }
    #[test]
    fn test_qt_rle_24bit() {
        test_core("assets/QT/Animation-Truecolour.mov", RGB24_FORMAT, &[],
                  &[0x766ed69d, 0x80a200f9, 0xb2383cdd, 0x275ef735]);
    }
    #[test]
    fn test_qt_rle_32bit() {
        test_core("assets/QT/Jag-finder-renaming.mov", super::ARGB_FORMAT, &[],
                  &[0x4b3685c2, 0x2dc09f73, 0xf29f5b61, 0xb47f2ff7]);
    }
}
