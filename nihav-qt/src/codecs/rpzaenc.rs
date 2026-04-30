use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use std::ops::{Add, AddAssign, Sub, Div, DivAssign};

const RGB555BE_FORMAT: NAPixelFormaton = NAPixelFormaton::make_rgb16_fmt(5, 5, 5, true, true);

#[derive(Clone,Copy,Debug,Default,PartialEq)]
struct Pixel([u16; 3]);

impl Pixel {
    fn interpolate(&self, p1: Pixel) -> Pixel {
        let mut ret = *self;
        for (dst, &comp1) in ret.0.iter_mut().zip(p1.0.iter()) {
            *dst = (*dst * 21 + comp1 * 11) >> 5;
        }
        ret
    }
    fn min(&self, p1: Pixel) -> Pixel {
        let mut ret = *self;
        for (c0, &c1) in ret.0.iter_mut().zip(p1.0.iter()) {
            *c0 = (*c0).min(c1);
        }
        ret
    }
    fn max(&self, p1: Pixel) -> Pixel {
        let mut ret = *self;
        for (c0, &c1) in ret.0.iter_mut().zip(p1.0.iter()) {
            *c0 = (*c0).max(c1);
        }
        ret
    }
    fn dist(&self, other: &Pixel) -> u32 {
        self.0.iter().zip(other.0.iter()).fold(0u32,
            |acc, (&a, &b)| acc + u32::from(a.abs_diff(b)) * u32::from(a.abs_diff(b)))
    }
}

impl From<u16> for Pixel {
    fn from(pix: u16) -> Pixel {
        Pixel([(pix >> 10) & 0x1F, (pix >> 5) & 0x1F, pix & 0x1F])
    }
}

impl From<Pixel> for u16 {
    fn from(pix: Pixel) -> u16 {
        (pix.0[0] << 10) | (pix.0[1] << 5) | pix.0[2]
    }
}

impl Sub for Pixel {
    type Output = Self;
    fn sub(self, rhs: Pixel) -> Self::Output {
        let mut ret = self;
        for (c0, &c1) in ret.0.iter_mut().zip(rhs.0.iter()) {
            *c0 = c0.abs_diff(c1);
        }
        ret
    }
}

impl Add for Pixel {
    type Output = Self;
    fn add(self, rhs: Pixel) -> Self::Output {
        let mut ret = self;
        for (c0, &c1) in ret.0.iter_mut().zip(rhs.0.iter()) {
            *c0 += c1;
        }
        ret
    }
}

impl AddAssign for Pixel {
    fn add_assign(&mut self, other: Pixel) {
        for (dst, &src) in self.0.iter_mut().zip(other.0.iter()) {
            *dst += src;
        }
    }
}

impl Div<u8> for Pixel {
    type Output = Self;
    fn div(self, val: u8) -> Pixel {
        let mut ret = self;
        for comp in ret.0.iter_mut() {
            *comp /= u16::from(val);
        }
        ret
    }
}

impl DivAssign<u8> for Pixel {
    fn div_assign(&mut self, val: u8) {
        for comp in self.0.iter_mut() {
            *comp /= u16::from(val);
        }
    }
}

impl std::fmt::Display for Pixel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{:02X},{:02X},{:02X}", self.0[0], self.0[1], self.0[2])
    }
}

fn interpolate_u16(c0: u16, c1: u16) -> u16 {
    Pixel::from(c0).interpolate(c1.into()).into()
}

#[derive(Clone,Copy,Debug,PartialEq)]
enum EncState {
    None,
    Skip(usize),
    Fill(u16, usize),
    Four(u16, u16, [u8; 4]),
    Raw,
}

impl EncState {
    fn write(&self, dst: &mut GrowableMemoryWriter<'_>, patterns: &mut Vec<[u8; 4]>) {
        match *self {
            EncState::None => {},
            EncState::Skip(skip) => {
                for _ in 0..(skip / 32) {
                    let _ = dst.write_byte(0x80 | 0x1F);
                }
                if (skip & 0x1F) > 0 {
                    let _ = dst.write_byte(0x80 | ((skip & 0x1F) as u8 - 1));
                }
            },
            EncState::Fill(clr, nblocks) => {
                for _ in 0..(nblocks / 32) {
                    let _ = dst.write_byte(0xA0 | 0x1F);
                    let _ = dst.write_u16be(clr);
                }
                if (nblocks & 0x1F) > 0 {
                    let _ = dst.write_byte(0xA0 | ((nblocks & 0x1F) as u8 - 1));
                    let _ = dst.write_u16be(clr);
                }
            },
            EncState::Four(clr0, clr1, _) => {
                if patterns.len() == 1 {
                    let _ = dst.write_u16be(clr0);
                    let _ = dst.write_u16be(clr1 | 0x8000);
                    let _ = dst.write_buf(&patterns[0]);
                } else {
                    for pats in patterns.chunks(32) {
                        let _ = dst.write_byte(0xC0 | (pats.len() as u8 - 1));
                        let _ = dst.write_u16be(clr0);
                        let _ = dst.write_u16be(clr1);
                        for pat in pats.iter() {
                            let _ = dst.write_buf(pat);
                        }
                    }
                }
                patterns.clear()
            },
            EncState::Raw => {},
        }
    }
}

#[derive(Clone,Copy,Debug,Default,PartialEq)]
enum Strategy {
    Lossless,
    #[default]
    Normal,
    Refined,
}

fn find_nearest(cc: &[Pixel; 4], pix: &Pixel) -> (usize, u32) {
    let mut best_idx = 0;
    let mut best_dist = u32::MAX;
    for (i, cand) in cc.iter().enumerate() {
        let dist = cand.dist(pix);
        if dist == 0 {
            return (i, 0);
        }
        if dist < best_dist {
            best_dist = dist;
            best_idx  = i;
        }
    }
    (best_idx, best_dist)
}

struct RoadPizzaEncoder {
    stream:     Option<NAStreamRef>,
    pkt:        Option<NAPacket>,
    key_int:    u8,
    frm_no:     u8,
    last_frm:   Vec<u16>,
    vinfo:      NAVideoInfo,
    mode:       Strategy,
}

impl RoadPizzaEncoder {
    fn new() -> Self {
        Self {
            stream:     None,
            pkt:        None,
            key_int:    25,
            frm_no:     254,
            last_frm:   Vec::new(),
            vinfo:      NAVideoInfo::new(0, 0, false, RGB555BE_FORMAT),
            mode:       Strategy::default(),
        }
    }
    fn paint_4clr(dst: &mut [u16], stride: usize, x: usize, c3: u16, c0: u16, pat: [u8; 4]) {
        let clrs = [c0, interpolate_u16(c0, c3), interpolate_u16(c3, c0), c3];
        for (line, &pp) in dst.chunks_exact_mut(stride).zip(pat.iter()) {
            let mut idx = usize::from(pp);
            for pix in line[x..][..4].iter_mut() {
                *pix = clrs[(idx >> 6) & 3];
                idx <<= 2;
            }
        }
    }
    fn decide_mode(mode: Strategy, _prev_state: EncState, skip_dist: u32, pixels: &[Pixel; 16]) -> EncState {
        let skip_thr = if mode == Strategy::Lossless { 0 } else { 32 };
        if skip_dist.saturating_sub(skip_thr) == 0 {
            return EncState::Skip(1);
        }
        match mode {
            Strategy::Lossless => {
                if pixels == &[pixels[0]; 16] {
                    EncState::Fill(pixels[0].into(), 1)
                } else {
                    EncState::Raw
                }
            },
            Strategy::Normal | Strategy::Refined => {
                let mut min_pix = Pixel([0x1F; 3]);
                let mut max_pix = Pixel([0x00; 3]);
                for pix in pixels.iter() {
                    min_pix = min_pix.min(*pix);
                    max_pix = max_pix.max(*pix);
                }
                let range = max_pix - min_pix;
                let ridx = if range.0[1] >= range.0[0] && range.0[1] >= range.0[2] {
                        1
                    } else if range.0[0] >= range.0[1] && range.0[0] >= range.0[2] {
                        0
                    } else if range.0[2] >= range.0[0] && range.0[2] >= range.0[1] {
                        2
                    } else {
                        1
                    };
                if range.0[ridx] < 2 {
                    let fill: Pixel = pixels.iter().fold(Pixel::default(), |acc, &p| acc + p) / 16;
                    return EncState::Fill(u16::from(fill), 1);
                }
                let mut cnt = [0u8; 2];
                let mut sum = [Pixel::default(); 2];
                for pix in pixels.iter() {
                    if pix.0[ridx].abs_diff(min_pix.0[ridx]) < 2 {
                        cnt[0] += 1;
                        sum[0] += *pix;
                    }
                    if pix.0[ridx].abs_diff(max_pix.0[ridx]) < 2 {
                        cnt[1] += 1;
                        sum[1] += *pix;
                    }
                }
                sum[0] /= cnt[0];
                sum[1] /= cnt[1];

                let mut centroids = [sum[0], sum[0].interpolate(sum[1]), sum[1].interpolate(sum[0]), sum[1]];
                if mode == Strategy::Refined {
                    let mut cur_dist = pixels.iter().fold(0u32, |acc, pix| acc + find_nearest(&centroids, pix).1);
                    loop {
                        let mut sum = [Pixel::default(); 4];
                        let mut cnt = [0; 4];
                        for pix in pixels.iter() {
                            let (idx, _dist) = find_nearest(&centroids, pix);
                            sum[idx] += *pix;
                            cnt[idx] += 1;
                        }
                        for (s, &cc) in sum.iter_mut().zip(cnt.iter()) {
                            *s /= cc.max(1);
                        }
                        let new_dist = pixels.iter().fold(0u32, |acc, pix| acc + find_nearest(&sum, pix).1);
                        if new_dist < cur_dist {
                            centroids[0] = sum[0];
                            centroids[3] = sum[3];
                            centroids[1] = centroids[0].interpolate(centroids[3]);
                            centroids[2] = centroids[3].interpolate(centroids[0]);
                            if new_dist >= cur_dist - cur_dist / 8 {
                                break;
                            }
                            cur_dist = new_dist;
                        } else {
                            break;
                        }
                    }
                }
                if u16::from(centroids[0]) > u16::from(centroids[3]) {
                    centroids.swap(0, 3);
                    centroids.swap(1, 2);
                }
                let mut pat = [0; 4];
                for (pp, row) in pat.iter_mut().zip(pixels.chunks_exact(4)) {
                    for pix in row.iter() {
                        let (idx, _) = find_nearest(&centroids, pix);
                        *pp <<= 2;
                        *pp |= idx as u8;
                    }
                }
                EncState::Four(centroids[3].into(), centroids[0].into(), pat)
            },
        }
    }
    fn encode(&mut self, dbuf: &mut Vec<u8>, src: &[u16], sstride: usize, force_intra: bool) -> bool {
        let mut is_intra = true;
        let mut pixels = [Pixel::default(); 16];

        let mut state = EncState::None;
        let mut patterns: Vec<[u8; 4]> = Vec::new();
        let pstride = self.vinfo.width;
        let mut gw = GrowableMemoryWriter::new_write(dbuf);
        let _ = gw.seek(SeekFrom::End(0));
        for (sstrip, pstrip) in src.chunks_exact(sstride * 4)
                .zip(self.last_frm.chunks_exact_mut(pstride * 4)) {
            for x in (0..self.vinfo.width).step_by(4) {
                for (row, line) in pixels.chunks_exact_mut(4).zip(sstrip.chunks_exact(sstride)) {
                    for (dst, &src) in row.iter_mut().zip(line[x..][..4].iter()) {
                        *dst = Pixel::from(src);
                    }
                }
                let mut skip_dist = if force_intra { u32::MAX } else { 0 };
                if !force_intra {
                    if self.mode == Strategy::Lossless {
                        for (line0, line1) in sstrip.chunks_exact(sstride)
                                .zip(pstrip.chunks_exact(pstride)) {
                            if line0[x..][..4] != line1[x..][..4] {
                                skip_dist = u32::MAX;
                                break;
                            }
                        }
                    } else {
                        for (line, prow) in pstrip.chunks_exact(pstride).zip(pixels.chunks_exact(4)) {
                            for (&ppix, npix) in line[x..][..4].iter().zip(prow.iter()) {
                                skip_dist += npix.dist(&ppix.into());
                            }
                        }
                    }
                }

                let new_state = Self::decide_mode(self.mode, state, skip_dist, &pixels);
                state = match (state, new_state) {
                        (_, EncState::None) => unreachable!(),
                        (EncState::Skip(skip), EncState::Skip(_)) => {
                            EncState::Skip(skip + 1)
                        },
                        (_, EncState::Skip(_)) => {
                            state.write(&mut gw, &mut patterns);
                            is_intra = false;
                            new_state
                        },
                        (EncState::Fill(old_clr, old_cnt), EncState::Fill(new_clr, _)) => {
                            for pline in pstrip.chunks_exact_mut(pstride) {
                                for pix in pline[x..][..4].iter_mut() {
                                    *pix = new_clr;
                                }
                            }
                            if old_clr == new_clr {
                                EncState::Fill(old_clr, old_cnt + 1)
                            } else {
                                state.write(&mut gw, &mut patterns);
                                new_state
                            }
                        },
                        (_, EncState::Fill(fill_clr, _)) => {
                            for pline in pstrip.chunks_exact_mut(pstride) {
                                for pix in pline[x..][..4].iter_mut() {
                                    *pix = fill_clr;
                                }
                            }
                            state.write(&mut gw, &mut patterns);
                            new_state
                        },
                        (EncState::Four(oc0, oc3, _), EncState::Four(c0, c3, cpat)) => {
                            Self::paint_4clr(pstrip, pstride, x, c0, c3, cpat);
                            if oc0 != c0 || oc3 != c3 {
                                state.write(&mut gw, &mut patterns);
                            }
                            patterns.push(cpat);
                            new_state
                        },
                        (_, EncState::Four(c0, c3, cpat)) => {
                            Self::paint_4clr(pstrip, pstride, x, c0, c3, cpat);
                            state.write(&mut gw, &mut patterns);
                            patterns.push(cpat);
                            new_state
                        },
                        (_, EncState::Raw) => {
                            for (line, pline) in sstrip.chunks_exact(sstride)
                                    .zip(pstrip.chunks_exact_mut(pstride)) {
                                pline[x..][..4].copy_from_slice(&line[x..][..4]);
                                let _ = gw.write_u16be_arr(&line[x..][..4]);
                            }
                            state.write(&mut gw, &mut patterns);
                            EncState::None
                        },
                    };
            }
        }
        state.write(&mut gw, &mut patterns);
        let _ = gw.flush();
        is_intra
    }
}

impl NAEncoder for RoadPizzaEncoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                Ok(EncodeParameters {
                    format: NACodecTypeInfo::Video(NAVideoInfo::new(0, 0, false, RGB555BE_FORMAT)),
                    ..Default::default()
                })
            },
            NACodecTypeInfo::Video(_vinfo) => {
                let mut info = *encinfo;
                if let NACodecTypeInfo::Video(ref mut vinfo) = info.format {
                    vinfo.format  = RGB555BE_FORMAT;
                    vinfo.width   = (vinfo.width  + 3) & !3;
                    vinfo.height  = (vinfo.height + 3) & !3;
                    vinfo.bits    = 16;
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
                if vinfo.format != RGB555BE_FORMAT || ((vinfo.width | vinfo.height) & 3) != 0 {
                    return Err(EncoderError::FormatError);
                }
                let info = NACodecInfo::new("apple-video", encinfo.format, None);
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

        let mut dbuf = vec![0xE1, 0, 0, 0]; // ID + 24-bit frame size
        let is_intra = if let Some(vbuf16) = buf.get_vbuf16() {
                let src = vbuf16.get_data();
                let stride = vbuf16.get_stride(0);
                self.last_frm.resize(self.vinfo.width * self.vinfo.height, 0);

                self.encode(&mut dbuf, src, stride, force_intra)
            } else if matches!(buf, NABufferType::None) {
                if force_intra {
                    if self.last_frm.is_empty() {
                        return Err(EncoderError::InvalidParameters);
                    }
                    let last_copy = self.last_frm.clone();
                    self.encode(&mut dbuf, &last_copy, self.vinfo.width, force_intra);
                    true
                } else {
                    let state = EncState::Skip((self.vinfo.width / 4) * (self.vinfo.height / 4));
                    let mut gw = GrowableMemoryWriter::new_write(&mut dbuf);
                    let _ = gw.seek(SeekFrom::End(0));
                    let mut pats = Vec::new();
                    state.write(&mut gw, &mut pats);
                    let _ = gw.flush();
                    false
                }
            } else {
                return Err(EncoderError::FormatError);
            };

        if is_intra {
            self.frm_no = 0;
        }

        let size = dbuf.len() as u32;
        let _ = write_u24be(&mut dbuf[1..], size);

        self.pkt = Some(NAPacket::new(self.stream.clone().unwrap(), frm.ts, is_intra, dbuf));
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
        opt_type: NAOptionDefinitionType::String(Some(&["lossless", "normal", "refined"])) },
];

impl NAOptionHandler for RoadPizzaEncoder {
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
                                    "lossless"  => { self.mode = Strategy::Lossless; },
                                    "normal"    => { self.mode = Strategy::Normal; },
                                    "refined"   => { self.mode = Strategy::Refined; },
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
                    Strategy::Lossless  => Some(NAValue::String("lossless".to_string())),
                    Strategy::Normal    => Some(NAValue::String("normal".to_string())),
                    Strategy::Refined   => Some(NAValue::String("refined".to_string())),
                }
            },
            _ => None,
        }
    }
}

pub fn get_encoder() -> Box<dyn NAEncoder + Send> {
    Box::new(RoadPizzaEncoder::new())
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
                enc_name:       "apple-video",
                out_name:       "rpza.mov",
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
    fn test_rpza_lossless() {
        test_core("assets/QT/Animation-Truecolour.mov", super::RGB555BE_FORMAT,
                  &[NAOption{ name: "mode", value: NAValue::String("lossless".to_owned()) }],
                  &[0x420025d3, 0x33b792c4, 0x8f211b13, 0xa1f6f8e6]);
    }
    #[test]
    fn test_rpza_normal() {
        test_core("assets/QT/Animation-Truecolour.mov", super::RGB555BE_FORMAT,
                  &[NAOption{ name: "mode", value: NAValue::String("normal".to_owned()) }],
                  &[0x42ca0fce, 0xe49fd607, 0xac393dd9, 0xd9d503cf]);
    }
}
