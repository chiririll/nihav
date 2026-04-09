use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use super::ultidata::*;
use std::ops::{BitOr, BitOrAssign};

trait SampleMap {
    fn map_luma(self) -> u8;
    fn map_chroma(self) -> u8;
    fn luma_diff(self, other: u8) -> u32;
    fn chroma_diff(self, other: u8) -> u32;
}

impl SampleMap for u8 {
    fn map_luma(self) -> u8 { INV_LUMA_MAP[usize::from(self)] }
    fn map_chroma(self) -> u8 { INV_CHROMA_MAP[usize::from(self)] }
    fn luma_diff(self, other: u8) -> u32 {
        u32::from(LUMA_MAP[usize::from(self)].abs_diff(LUMA_MAP[usize::from(other)]))
    }
    fn chroma_diff(self, other: u8) -> u32 {
        u32::from(CHROMA_MAP[usize::from(self)].abs_diff(CHROMA_MAP[usize::from(other)]))
    }
}

fn pack_luma4(yy: &[u8]) -> u32 {
    let mut word = 0;
    for &y in yy[..4].iter() {
        word <<= 6;
        word |= u32::from(y);
    }
    word
}
fn pack_chroma(u: u8, v: u8) -> u8 { u * 16 + v }

fn luma_dist(y0: &[u8; 16], y1: &[u8; 16]) -> u32 {
    y0.iter().zip(y1.iter()).fold(0u32, |acc, (&a, &b)| acc + a.luma_diff(b))
}

fn avg4(a: u8, b: u8, c: u8, d: u8) -> u8 {
    ((u16::from(a) + u16::from(b) + u16::from(c) + u16::from(d)) / 4) as u8
}

#[derive(Clone,Copy,Debug,Default,PartialEq)]
enum BlockMode {
    #[default]
    Any,
    Normal,
    Unique,
    Invalid,
}

impl BlockMode {
    fn check_compat(self, other: Self) -> bool {
        !matches!((self, other),
            (BlockMode::Normal, BlockMode::Unique) |
            (BlockMode::Unique, BlockMode::Normal))
    }
}

impl BitOr for BlockMode {
    type Output = BlockMode;
    fn bitor(self, rhs: Self) -> Self::Output {
        if self == BlockMode::Any {
            rhs
        } else if rhs == BlockMode::Any || rhs == self {
            self
        } else {
            BlockMode::Invalid
        }
    }
}

impl BitOrAssign for BlockMode {
    fn bitor_assign(&mut self, rhs: Self) {
        if *self == BlockMode::Any {
            *self = rhs;
        } else if rhs != BlockMode::Any && rhs != *self {
            *self = BlockMode::Invalid
        }
    }
}

const ANGLES: [u8; 4] = [0x0, 0x2, 0x6, 0xC];

fn estimate_gradient(yy: &[u16; 16], blk_thr: u32, ordered: bool) -> (u8, [u8; 4], bool) {
    let mut sums: [u16; 4];
    let mut vars: [u32; 4];
    let mut cnts: [u16; 4];

    let mut best_idx  = 0;
    let mut best_y4   = [0; 4];
    let mut best_dist = u32::MAX;
    let mut best_ord  = false;

    for (angle_no, pat) in PATTERN.iter().take(8).enumerate() {
        sums = [0; 4];
        vars = [0; 4];
        cnts = [0; 4];
        for (&y, &idx) in yy.iter().zip(pat.iter()) {
            sums[usize::from(idx)] += y;
            cnts[usize::from(idx)] += 1;
        }
        for (sum, &cnt) in sums.iter_mut().zip(cnts.iter()) {
            *sum = (*sum + cnt / 2) / cnt;
        }
        for (&y, &idx) in yy.iter().zip(pat.iter()) {
            let diff = u32::from(y.abs_diff(sums[usize::from(idx)]));
            vars[usize::from(idx)] += diff * diff;
        }

        let cur_y4 = [sums[0] as u8, sums[1] as u8, sums[2] as u8, sums[3] as u8];
        let cur_ordered = (cur_y4[0] <= cur_y4[1] && cur_y4[1] <= cur_y4[2] && cur_y4[2] <= cur_y4[3])
                       || (cur_y4[0] >= cur_y4[1] && cur_y4[1] >= cur_y4[2] && cur_y4[2] >= cur_y4[3]);

        let dist = vars.iter().sum();
        if dist < best_dist && (!ordered || cur_ordered || best_dist == u32::MAX) {
            best_dist = dist;
            best_idx  = angle_no;
            best_y4   = cur_y4;
            best_ord  = cur_ordered;
            if best_dist <= blk_thr {
                break;
            }
        }
    }
    (best_idx as u8, best_y4, best_ord)
}

#[derive(Clone,Copy,Debug,Default,PartialEq)]
#[allow(clippy::upper_case_acronyms)]
enum QuadToken {
    #[default]
    Skip,
    Shallow(u8, u8), // mode + luma
    LTC(u8, u16), // angle + codebook index
    Subsampled([u8; 4]), // luma
    Statistical(u16, [u8; 2]), // flags + luma
    Extended(u8, [u8; 4]), // angle + luma
    Raw([u8; 16]),
}

impl QuadToken {
    fn is_skip(&self) -> bool { matches!(*self, QuadToken::Skip) }
    fn get_mode(&self) -> BlockMode {
        match *self {
            QuadToken::LTC(_, _) |
            QuadToken::Statistical(_, _) |
            QuadToken::Extended(_, _) => BlockMode::Normal,

            QuadToken::Subsampled(_) |
            QuadToken::Raw(_) => BlockMode::Unique,

            _ => BlockMode::Any,
        }
    }
    fn get_mode_id(&self) -> u8 {
        match *self {
            QuadToken::Skip => 0,
            QuadToken::Shallow(_, _) => 1,
            QuadToken::LTC(_, _) => 2,
            QuadToken::Subsampled(_) => 2,
            QuadToken::Statistical(_, _) => 3,
            QuadToken::Extended(_, _) => 3,
            QuadToken::Raw(_) => 3,
        }
    }
    fn cost(&self) -> u32 {
        match *self {
            QuadToken::Skip => 0,
            QuadToken::Shallow(_, _) => 1,
            QuadToken::LTC(_, _) => 2,
            QuadToken::Subsampled(_) => 3,
            QuadToken::Statistical(_, _) => 4,
            QuadToken::Extended(_, _) => 4,
            QuadToken::Raw(_) => 12,
        }
    }
    fn recon_quad(&self, quad: &mut Quadrant) {
        match *self {
            QuadToken::Shallow(mode, y0) => {
                let mut angle = ANGLES[usize::from(mode)];
                let y2 = if angle > 0 { (y0 + 1).min(0x3F) } else { y0 };
                let mut yy = [y0, y0, y2, y2];
                if (angle & 8) != 0 {
                    yy.swap(0, 3);
                    yy.swap(1, 2);
                    angle &= 7;
                }
                Self::paint_gradient(&mut quad.y, angle, yy);
            },
            QuadToken::LTC(angle, idx) => {
                let mut yy = CODEBOOK[usize::from(idx)];
                if (angle & 8) != 0 {
                    yy.swap(0, 3);
                    yy.swap(1, 2);
                }
                Self::paint_gradient(&mut quad.y, angle & 7, yy);
            },
            QuadToken::Subsampled(yy) => {
                Self::paint_gradient(&mut quad.y, 0xA, yy);
            },
            QuadToken::Statistical(flags, y2) => {
                let mut pattern = usize::from(flags);
                for dst in quad.y.iter_mut() {
                    *dst = y2[(pattern >> 15) & 1];
                    pattern <<= 1;
                }
            },
            QuadToken::Extended(angle, yy) => {
                Self::paint_gradient(&mut quad.y, angle, yy);
            },
            QuadToken::Raw(ref blk) => { quad.y.copy_from_slice(blk); },
            _ => {},
        }
    }
    fn paint_gradient(y: &mut [u8; 16], angle: u8, yy: [u8; 4]) {
        for (row, prow) in y.chunks_exact_mut(4)
                .zip(PATTERN[usize::from(angle)].chunks_exact(4)) {
            for (el, &idx) in row.iter_mut().zip(prow.iter()) {
                *el = yy[usize::from(idx)];
            }
        }
    }

    fn lossless_opt(y: [u8; 16], mode: BlockMode) -> Self {
        let mut qt = QuadToken::Raw(y);
        let mut cost = qt.cost();
        let mut tmp = Quadrant::new();

        if mode != BlockMode::Normal &&
            y[ 0] == y[ 1] && y[ 0] == y[ 4] && y[ 0] == y[ 5] &&
            y[ 2] == y[ 3] && y[ 2] == y[ 6] && y[ 2] == y[ 7] &&
            y[ 8] == y[ 9] && y[ 8] == y[12] && y[ 8] == y[13] &&
            y[10] == y[11] && y[10] == y[14] && y[10] == y[15] {
            let new_qt = QuadToken::Subsampled([y[0], y[2], y[8], y[10]]);
            if new_qt.cost() < cost {
                qt = new_qt;
                cost = qt.cost();
            }
        }
        if mode != BlockMode::Unique {
            let y0 = y[0];
            let mut y1 = None;
            let mut more = false;
            for &el in y.iter() {
                if el != y0 {
                    if y1.is_none() {
                        y1 = Some(el);
                    } else if Some(el) != y1 {
                        more = true;
                        break;
                    }
                }
            }
            if !more {
                let y1 = y1.unwrap_or(y0);
                let mut pat = 0;
                for &el in y.iter() {
                    pat <<= 1;
                    if el == y1 {
                        pat |= 1;
                    }
                }
                let new_qt = QuadToken::Statistical(pat, [y0, y1]);
                if new_qt.cost() < cost {
                    qt = new_qt;
                    cost = qt.cost();
                }
            }
        }
        let shallow_cost = QuadToken::Shallow(0, 0).cost();
        if shallow_cost < cost {
            for mode in 0..4 {
                let new_qt = QuadToken::Shallow(mode as u8, y[0]);
                new_qt.recon_quad(&mut tmp);
                if tmp.y == y {
                    qt = new_qt;
                    cost = shallow_cost;
                    break;
                }
            }
        }
        let _ = cost;
        qt
    }
    fn lossy_normal(y: [u8; 16], blk_thr: u32) -> Self {
        let mut qt = QuadToken::Raw(y);
        let mut dist = u32::MAX;
        let mut tmp = Quadrant::new();

        {
            let avg = (y.iter().fold(0u16, |acc, &a| acc + u16::from(a)) / 16) as u8;
            let mut avg0 = 0;
            let mut avg1 = 0;
            let mut cnt0 = 0;
            let mut cnt1 = 0;
            let mut pat = 0;
            for &el in y.iter() {
                pat <<= 1;
                if el <= avg {
                    avg0 += u16::from(el);
                    cnt0 += 1;
                } else {
                    avg1 += u16::from(el);
                    cnt1 += 1;
                    pat |= 1;
                }
            }
            let mut y2 = [(avg0 / cnt0.max(1)) as u8, (avg1 / cnt1.max(1)) as u8];
            if (pat & 0x8000) != 0 {
                pat = !pat;
                y2.swap(0, 1);
            }
            let new_qt = QuadToken::Statistical(pat, y2);
            new_qt.recon_quad(&mut tmp);
            let new_dist = luma_dist(&y, &tmp.y);
            if new_dist < dist {
                qt = new_qt;
                dist = new_dist;
                if dist <= blk_thr {
                    return qt;
                }
            }
        }

        {
            let avg = (y.iter().fold(0u16, |acc, &a| acc + u16::from(a)) / 16) as u8;
            let mut avg0 = 0;
            let mut avg1 = 0;
            let mut cnt0 = 0;
            let mut cnt1 = 0;
            let mut pat = 0;
            for &el in y.iter() {
                pat <<= 1;
                if el <= avg {
                    avg0 += u16::from(el);
                    cnt0 += 1;
                } else {
                    avg1 += u16::from(el);
                    cnt1 += 1;
                    pat |= 1;
                }
            }
            let mut y2 = [(avg0 / cnt0.max(1)) as u8, (avg1 / cnt1.max(1)) as u8];
            if (pat & 0x8000) != 0 {
                pat = !pat;
                y2.swap(0, 1);
            }
            let new_qt = QuadToken::Statistical(pat, y2);
            new_qt.recon_quad(&mut tmp);
            let new_dist = luma_dist(&y, &tmp.y);
            if new_dist < dist {
                qt = new_qt;
                dist = new_dist;
                if dist <= blk_thr {
                    return qt;
                }
            }
        }
        for idx in 0..CODEBOOK.len() {
            for angle in 0..16 {
                let new_qt = QuadToken::LTC(angle as u8, idx as u16);
                new_qt.recon_quad(&mut tmp);
                let new_dist = luma_dist(&y, &tmp.y);
                if new_dist < dist {
                    qt = new_qt;
                    dist = new_dist;
                    if dist <= blk_thr {
                        return qt;
                    }
                }
            }
        }

        assert!(dist < u32::MAX);
        qt
    }
    fn lossy_full(y: [u8; 16], mut dist: u32, blk_thr: u32, mode: BlockMode, cat: u8) -> Self {
        let mut qt = QuadToken::Skip;
        if dist <= blk_thr {
            return qt;
        }
        let mut tmp = Quadrant::new();

        let y16: [u16; 16] = std::array::from_fn(|i| u16::from(y[i]));
        let full_sum: u16 = y16.iter().sum();
        {
            let vsum0 = y16.chunks_exact(4).fold(0u16, |acc, chunk| acc + chunk[0] + chunk[1]);
            let y0 = ((vsum0 + 4) / 8) as u8;
            let y1 = ((full_sum - vsum0 + 4) / 8) as u8;
            if (y0..=y0 + 2).contains(&y1) {
                let new_qt = QuadToken::Shallow(0, y0);
                new_qt.recon_quad(&mut tmp);
                let new_dist = luma_dist(&y, &tmp.y);
                if new_dist < dist {
                    qt = new_qt;
                    dist = new_dist;
                    if dist <= blk_thr {
                        return qt;
                    }
                }
            }

            let hsum0: u16 = y16[..8].iter().sum();
            let y0 = ((hsum0 + 4) / 8) as u8;
            let y1 = ((full_sum - hsum0 + 4) / 8) as u8;
            if (y0..=y0 + 2).contains(&y1) {
                let new_qt = QuadToken::Shallow(3, y0);
                new_qt.recon_quad(&mut tmp);
                let new_dist = luma_dist(&y, &tmp.y);
                if new_dist < dist {
                    qt = new_qt;
                    dist = new_dist;
                    if dist <= blk_thr {
                        return qt;
                    }
                }
            }

            let dsum0 = y16[0] + y16[4] + y16[8] + y16[9] + y16[10] + y16[12] + y16[13] + y16[14];
            let y0 = ((dsum0 + 4) / 8) as u8;
            let y1 = ((full_sum - dsum0 + 4) / 8) as u8;
            if (y0..=y0 + 2).contains(&y1) {
                let new_qt = QuadToken::Shallow(1, y0);
                new_qt.recon_quad(&mut tmp);
                let new_dist = luma_dist(&y, &tmp.y);
                if new_dist < dist {
                    qt = new_qt;
                    dist = new_dist;
                    if dist <= blk_thr {
                        return qt;
                    }
                }
            }

            let dsum0 = y16[6] + y16[7] + y16[10] + y16[11] + y16[12] + y16[13] + y16[14] + y16[15];
            let y0 = ((dsum0 + 4) / 8) as u8;
            let y1 = ((full_sum - dsum0 + 4) / 8) as u8;
            if (y0..=y0 + 2).contains(&y1) {
                let new_qt = QuadToken::Shallow(2, y0);
                new_qt.recon_quad(&mut tmp);
                let new_dist = luma_dist(&y, &tmp.y);
                if new_dist < dist {
                    qt = new_qt;
                    dist = new_dist;
                    if dist <= blk_thr {
                        return qt;
                    }
                }
            }
        }

        if mode != BlockMode::Unique && (cat >= 3 || dist == u32::MAX) {
            let avg = (y.iter().fold(0u16, |acc, &a| acc + u16::from(a)) / 16) as u8;
            let mut avg0 = 0;
            let mut avg1 = 0;
            let mut cnt0 = 0;
            let mut cnt1 = 0;
            let mut pat = 0;
            for &el in y.iter() {
                pat <<= 1;
                if el <= avg {
                    avg0 += u16::from(el);
                    cnt0 += 1;
                } else {
                    avg1 += u16::from(el);
                    cnt1 += 1;
                    pat |= 1;
                }
            }
            let mut y2 = [(avg0 / cnt0.max(1)) as u8, (avg1 / cnt1.max(1)) as u8];
            if (pat & 0x8000) != 0 {
                pat = !pat;
                y2.swap(0, 1);
            }
            let new_qt = QuadToken::Statistical(pat, y2);
            new_qt.recon_quad(&mut tmp);
            let new_dist = luma_dist(&y, &tmp.y);
            if new_dist < dist {
                qt = new_qt;
                dist = new_dist;
                if dist <= blk_thr {
                    return qt;
                }
            }
        }

        if mode != BlockMode::Unique && (cat > 1 || dist == u32::MAX) {
            let (angle, y4, ordered) = estimate_gradient(&y16, blk_thr, false);
            let (ltc_ang, ltc_y4) = if ordered {
                    if y4[0] < y4[3] {
                        (angle, y4)
                    } else {
                        (angle ^ 8, [y4[3], y4[2], y4[1], y4[0]])
                    }
                } else {
                    let (l_angle, l_y4, _) = estimate_gradient(&y16, blk_thr, true);
                    if l_y4[0] < l_y4[3] {
                        (l_angle, l_y4)
                    } else {
                        (l_angle ^ 8, [l_y4[3], l_y4[2], l_y4[1], l_y4[0]])
                    }
                };
            for (idx, cb) in CODEBOOK.iter().enumerate() {
                if cb[0] < ltc_y4[0].saturating_sub(2) {
                    continue;
                }
                if cb[0] > ltc_y4[0] + 2 {
                    break;
                }
                if  cb[1].abs_diff(ltc_y4[1]) <= 2 &&
                    cb[2].abs_diff(ltc_y4[2]) <= 2 &&
                    cb[3].abs_diff(ltc_y4[3]) <= 2 {

                    let new_qt = QuadToken::LTC(ltc_ang, idx as u16);
                    new_qt.recon_quad(&mut tmp);
                    let new_dist = luma_dist(&y, &tmp.y);
                    if new_dist < dist {
                        qt = new_qt;
                        dist = new_dist;
                        if dist <= blk_thr {
                            return qt;
                        }
                    }
                }
            }

            if cat > 2 || dist == u32::MAX {
                let new_qt = QuadToken::Extended(angle, y4);
                let new_dist = luma_dist(&y, &tmp.y);
                if new_dist < dist {
                    qt = new_qt;
                    dist = new_dist;
                    if dist <= blk_thr {
                        return qt;
                    }
                }
            }
        }

        if mode != BlockMode::Normal && (cat >= 2 || dist == u32::MAX) {
            let y0 = avg4(y[ 0], y[ 1], y[ 4], y[ 5]);
            let y1 = avg4(y[ 2], y[ 3], y[ 6], y[ 7]);
            let y2 = avg4(y[ 8], y[ 9], y[12], y[13]);
            let y3 = avg4(y[10], y[11], y[14], y[15]);
            let new_qt = QuadToken::Subsampled([y0, y1, y2, y3]);
            new_qt.recon_quad(&mut tmp);
            let new_dist = luma_dist(&y, &tmp.y);
            if new_dist < dist {
                qt = new_qt;
                dist = new_dist;
                if dist <= blk_thr {
                    return qt;
                }
            }

            if cat >= 3 {
                return QuadToken::Raw(y);
            }
        }

        assert!(dist < u32::MAX);
        qt
    }
}

impl std::fmt::Display for QuadToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match *self {
            QuadToken::Skip => write!(f, "skip"),
            QuadToken::Shallow(_, _) => write!(f, "shallow"),
            QuadToken::LTC(_, _) => write!(f, "LTC"),
            QuadToken::Subsampled(_) => write!(f, "subsampled"),
            QuadToken::Statistical(_, _) => write!(f, "statistical"),
            QuadToken::Extended(_, _) => write!(f, "extended"),
            QuadToken::Raw(_) => write!(f, "raw"),
        }
    }
}

#[derive(Clone,Copy,Debug,Default)]
struct BlockToken {
    chroma: [[u8; 2]; 4],
    common: bool,
    quad:   [QuadToken; 4],
}

impl BlockToken {
    fn new() -> Self { Self::default() }
    fn is_skip(&self) -> bool { matches!(self.quad, [QuadToken::Skip, QuadToken::Skip, QuadToken::Skip, QuadToken::Skip]) }
}

#[derive(Clone,Copy,Default,PartialEq)]
struct Quadrant {
    y:  [u8; 16],
    u:  u8,
    v:  u8,
}

impl Quadrant {
    fn new() -> Self { Self::default() }
}

#[derive(Clone,Copy,Default,PartialEq)]
struct Block {
    quad:   [Quadrant; 4],
}

#[derive(Clone,Copy,Default,PartialEq)]
enum WorkMode {
    Raw,
    Lossless,
    #[default]
    Lossy,
    Fast,
}

impl std::fmt::Display for WorkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match *self {
            WorkMode::Raw       => write!(f, "raw"),
            WorkMode::Lossless  => write!(f, "lossless"),
            WorkMode::Lossy     => write!(f, "lossy"),
            WorkMode::Fast      => write!(f, "fast"),
        }
    }
}

struct UltimotionEncoder {
    stream:     Option<NAStreamRef>,
    pkt:        Option<NAPacket>,
    frmcount:   u8,
    key_int:    u8,
    mode:       WorkMode,
    quality:    u8,
    cur_blks:   Vec<Block>,
    prev_blks:  Vec<Block>,
    width:      usize,
    height:     usize,
    tokens:     Vec<BlockToken>,
    blk_thr:    u32,
    chr_thr:    u32,
    var_thr:    u32,
}

impl UltimotionEncoder {
    fn new() -> Self {
        Self {
            stream:     None,
            pkt:        None,
            frmcount:   0,
            mode:       WorkMode::default(),
            key_int:    25,
            quality:    0,
            cur_blks:   Vec::new(),
            prev_blks:  Vec::new(),
            tokens:     Vec::new(),
            width:      0,
            height:     0,
            blk_thr:    0,
            chr_thr:    0,
            var_thr:    0,
        }
    }
    fn load_blocks(&mut self, in_frm: &NAVideoBuffer<u8>) -> EncoderResult<()> {
        let (width, height) = in_frm.get_dimensions(0);
        if width != self.width || height != self.height {
            return Err(EncoderError::FormatError);
        }
        let (yoff, uoff, voff) = (in_frm.get_offset(0), in_frm.get_offset(1), in_frm.get_offset(2));
        let (ystr, ustr, vstr) = (in_frm.get_stride(0), in_frm.get_stride(1), in_frm.get_stride(2));
        let src = in_frm.get_data();

        for (brow, (ystrip, (ustrip, vstrip))) in self.cur_blks.chunks_exact_mut(width / 8)
                .zip(src[yoff..].chunks(ystr * 8).zip(src[uoff..].chunks(ustr * 2)
                    .zip(src[voff..].chunks(vstr * 2)))) {
            for (blk_x, blk) in brow.iter_mut().enumerate() {
                for (quad, &(xoff, yoff)) in blk.quad.iter_mut()
                        .zip([(0, 0), (0, 4), (4, 4), (4, 0)].iter()) {
                    for (dy, yy) in quad.y.chunks_exact_mut(4)
                            .zip(ystrip[blk_x * 8 + xoff + yoff * ystr..].chunks(ystr)) {
                        for (dst, &src) in dy.iter_mut().zip(yy.iter()) {
                            *dst = src.map_luma();
                        }
                    }
                    quad.u = ustrip[blk_x * 2 + xoff / 4 + (yoff / 4) * ustr].map_chroma();
                    quad.v = vstrip[blk_x * 2 + xoff / 4 + (yoff / 4) * vstr].map_chroma();
                }
            }
        }
        Ok(())
    }
    fn encode_intra(&mut self) {
        self.tokens.clear();
        for blk in self.cur_blks.iter() {
            let mut tok = BlockToken::new();
            let mut mode = BlockMode::Any;
            let mut done = true;
            let mut cats = [3; 4];
            if self.mode == WorkMode::Fast && self.var_thr > 0 {
                let mut avgs = [0; 4];
                for (avg, quad) in avgs.iter_mut().zip(blk.quad.iter()) {
                    *avg = ((quad.y.iter().fold(0u16, |acc, &el| acc + u16::from(el)) + 8) / 16) as u8;
                }
                let mut var = 0;
                for (cat, (&avg, quad)) in cats.iter_mut().zip(avgs.iter().zip(blk.quad.iter())) {
                    let qvar = quad.y.iter().fold(0u32, |acc, &el| acc + u32::from(avg.abs_diff(el)) * u32::from(avg.abs_diff(el)));
                    if qvar < 4 {
                        *cat = 1;
                    } else if qvar < 10 {
                        *cat = 2;
                    }
                    var = var.max(qvar);
                }
                mode = if var > self.var_thr { BlockMode::Unique } else { BlockMode::Normal };
            }
            for ((qt, cm), (quad, &cat)) in tok.quad.iter_mut()
                    .zip(tok.chroma.iter_mut()).zip(blk.quad.iter().zip(cats.iter())) {
                cm[0] = quad.u;
                cm[1] = quad.v;
                *qt = match self.mode {
                        WorkMode::Raw => QuadToken::Raw(quad.y),
                        WorkMode::Lossless => QuadToken::lossless_opt(quad.y, mode),
                        WorkMode::Lossy |
                        WorkMode::Fast => QuadToken::lossy_full(quad.y, u32::MAX, self.blk_thr, mode, cat),
                    };
                let new_mode = qt.get_mode();
                if !mode.check_compat(new_mode) {
                    done = false;
                    break;
                }
                mode |= new_mode;
            }
            if !done {
                if self.mode == WorkMode::Fast && mode == BlockMode::Any {
                    mode = BlockMode::Unique;
                }
                for ((qt, cm), (quad, &cat)) in tok.quad.iter_mut()
                        .zip(tok.chroma.iter_mut()).zip(blk.quad.iter().zip(cats.iter())) {
                    cm[0] = quad.u;
                    cm[1] = quad.v;
                    *qt = match self.mode {
                            WorkMode::Raw => QuadToken::Raw(quad.y),
                            WorkMode::Lossless => QuadToken::lossless_opt(quad.y, BlockMode::Unique),
                            WorkMode::Lossy => QuadToken::lossy_full(quad.y, u32::MAX, self.blk_thr, mode, cat),
                            WorkMode::Fast => QuadToken::lossy_full(quad.y, u32::MAX, self.blk_thr, BlockMode::Unique, cat),
                        };
                }
            }
            if matches!(self.mode, WorkMode::Raw | WorkMode::Lossless) {
                tok.common = true;
                let pair = tok.chroma[0];
                for cpair in tok.chroma.iter() {
                    if pair != *cpair {
                        tok.common = false;
                        break;
                    }
                }
            } else {
                let csum = tok.chroma.iter().fold((0u8, 0u8), |acc, pair| (acc.0 + pair[0], acc.1 + pair[1]));
                let avg = [(csum.0 + 2) / 4, (csum.1 + 2) / 4];
                let dist = tok.chroma.iter().fold(0u32, |acc, pair|
                        acc + pair[0].chroma_diff(avg[0]) + pair[1].chroma_diff(avg[1]));
                tok.common = (dist + 3) / 4 <= self.chr_thr;
                if tok.common {
                    tok.chroma = [avg; 4];
                }
            }
            self.tokens.push(tok);
        }
    }
    fn encode_inter(&mut self) -> bool {
        self.tokens.clear();
        let mut is_intra = true;
        for (blk, pblk) in self.cur_blks.iter().zip(self.prev_blks.iter()) {
            let mut tok = BlockToken::new();
            if blk == pblk {
                self.tokens.push(tok);
                is_intra = false;
                continue;
            }
            let mut blk_mode = BlockMode::Any;
            let mut done = true;
            let mut cats = [3; 4];
            if self.mode == WorkMode::Fast && self.var_thr > 0 {
                let mut avgs = [0; 4];
                for (avg, quad) in avgs.iter_mut().zip(blk.quad.iter()) {
                    *avg = ((quad.y.iter().fold(0u16, |acc, &el| acc + u16::from(el)) + 8) / 16) as u8;
                }
                let mut var = 0;
                for (cat, (&avg, quad)) in cats.iter_mut().zip(avgs.iter().zip(blk.quad.iter())) {
                    let qvar = quad.y.iter().fold(0u32, |acc, &el| acc + u32::from(avg.abs_diff(el)) * u32::from(avg.abs_diff(el)));
                    if qvar < self.var_thr / 4 {
                        *cat = 1;
                    } else if qvar < self.var_thr / 2 {
                        *cat = 2;
                    }
                    var = var.max(qvar);
                }
                blk_mode = if var > self.var_thr { BlockMode::Unique } else { BlockMode::Normal };
            }
            for (((qt, cm), (quad, pquad)), &cat) in tok.quad.iter_mut()
                    .zip(tok.chroma.iter_mut())
                    .zip(blk.quad.iter().zip(pblk.quad.iter()))
                    .zip(cats.iter()) {
                cm[0] = quad.u;
                cm[1] = quad.v;
                *qt = if quad == pquad {
                        QuadToken::Skip
                    } else {
                        match self.mode {
                            WorkMode::Raw => QuadToken::Raw(quad.y),
                            WorkMode::Lossless => QuadToken::lossless_opt(quad.y, blk_mode),
                            WorkMode::Lossy |
                            WorkMode::Fast => {
                                let skip_dist = luma_dist(&quad.y, &pquad.y) + (quad.u.chroma_diff(pquad.u) + quad.v.chroma_diff(pquad.v)) * 4;
                                QuadToken::lossy_full(quad.y, skip_dist, self.blk_thr, blk_mode, cat)
                            },
                        }
                    };
                if qt.is_skip() {
                    is_intra = false;
                }
                let new_mode = qt.get_mode();
                if !blk_mode.check_compat(new_mode) {
                    done = false;
                    break;
                }
                blk_mode |= new_mode;
            }
            if !done {
                if self.mode == WorkMode::Fast && blk_mode == BlockMode::Any {
                    blk_mode = BlockMode::Unique;
                }
                if self.mode == WorkMode::Lossy {
                    blk_mode = BlockMode::Unique;
                }
                for (((qt, cm), (quad, pquad)), &cat) in tok.quad.iter_mut()
                        .zip(tok.chroma.iter_mut())
                        .zip(blk.quad.iter().zip(pblk.quad.iter()))
                        .zip(cats.iter()) {
                    cm[0] = quad.u;
                    cm[1] = quad.v;
                    *qt = if quad == pquad {
                            QuadToken::Skip
                        } else {
                            match self.mode {
                                WorkMode::Raw => QuadToken::Raw(quad.y),
                                WorkMode::Lossless => QuadToken::lossless_opt(quad.y, BlockMode::Unique),
                                WorkMode::Lossy |
                                WorkMode::Fast => {
                                    let skip_dist = luma_dist(&quad.y, &pquad.y) + (quad.u.chroma_diff(pquad.u) + quad.v.chroma_diff(pquad.v)) * 4;
                                    QuadToken::lossy_full(quad.y, skip_dist, self.blk_thr, blk_mode, cat)
                                },
                            }
                        };
                    if qt.is_skip() {
                        is_intra = false;
                    }
                }
            }

            if tok.is_skip() {
                self.tokens.push(tok);
                is_intra = false;
                continue;
            }

            let mut pair = None;
            if matches!(self.mode, WorkMode::Raw | WorkMode::Lossless) {
                tok.common = true;
                for (quad, cpair) in tok.quad.iter().zip(tok.chroma.iter()) {
                    if !quad.is_skip() {
                        if pair.is_none() {
                            pair = Some(*cpair);
                        } else if pair != Some(*cpair) {
                            tok.common = false;
                            pair = None;
                            break;
                        }
                    }
                }
            } else {
                let mut avg = [0u8; 2];
                let mut cnt = 0u8;
                for (quad, cpair) in tok.quad.iter().zip(tok.chroma.iter()) {
                    if !quad.is_skip() {
                        avg[0] += cpair[0];
                        avg[1] += cpair[1];
                        cnt += 1;
                    }
                }
                avg[0] /= cnt;
                avg[1] /= cnt;
                let mut dist = 0;
                for (quad, cpair) in tok.quad.iter().zip(tok.chroma.iter()) {
                    if !quad.is_skip() {
                        dist += cpair[0].chroma_diff(avg[0]) + cpair[1].chroma_diff(avg[1]);
                    }
                }
                tok.common = ((dist + u32::from(cnt - 1)) / u32::from(cnt)) <= self.chr_thr;
                if tok.common {
                    pair = Some(avg);
                }
            }
            if let Some(val) = pair {
                tok.chroma = [val; 4];
            }
            self.tokens.push(tok);
        }
        is_intra
    }
    fn write_tokens(&mut self, bw: &mut dyn ByteIO) -> EncoderResult<()> {
        while let Some(last) = self.tokens.pop() {
            if !last.is_skip() {
                self.tokens.push(last);
                break;
            }
        }

        let mut toks = self.tokens.iter_mut().peekable();
        let mut mode0 = true;
        let mut uniq_c_perm = false;
        let mut run = 0;
        for (blk, pblk) in self.cur_blks.iter_mut().zip(self.prev_blks.iter()) {
            if let Some(tok) = toks.next() {
                if run == 255 {
                    bw.write_byte(0x74)?;
                    bw.write_byte(run)?;
                    run = 0;
                }
                if tok.is_skip() {
                    run += 1;
                    *blk = *pblk;
                    continue;
                }
                if run > 0 {
                    bw.write_byte(0x74)?;
                    bw.write_byte(run)?;
                    run = 0;
                }

                let mut has_skipped = false;
                for q in tok.quad.iter() {
                    if q.is_skip() {
                        has_skipped = true;
                        break;
                    }
                }
                if has_skipped {
                    *blk = *pblk;
                }

                let mut mode = 0;
                for quad in tok.quad.iter() {
                    mode <<= 2;
                    mode |= quad.get_mode_id();
                }
                if (mode & 0xF8) == 0x70 {
                    let blk_mode = tok.quad.iter().fold(BlockMode::Any,
                        |m, quad|  m | quad.get_mode());
                    let mut use_unique = blk_mode == BlockMode::Unique;
                    if !mode0 || self.mode == WorkMode::Lossless {
                        use_unique = true;
                    }
                    if blk_mode == BlockMode::Normal {
                        use_unique = false;
                    }
                    if use_unique {
                        tok.quad[0] = QuadToken::Raw(blk.quad[0].y);
                    } else {
                        tok.quad[0] = QuadToken::lossy_normal(blk.quad[0].y, self.blk_thr);
                    }
                }

                let blk_mode = tok.quad.iter().fold(BlockMode::Any,
                    |m, quad|  m | quad.get_mode());

                assert!(blk_mode != BlockMode::Invalid);

                if mode0 && blk_mode == BlockMode::Unique {
                    bw.write_byte(0x70)?;
                    bw.write_byte(0x01)?;
                    mode0 = false;
                } else if !mode0 && blk_mode == BlockMode::Normal {
                    bw.write_byte(0x70)?;
                    bw.write_byte(0x00)?;
                    mode0 = true;
                }

                if tok.common {
                    if uniq_c_perm {
                        bw.write_byte(0x72)?;
                        uniq_c_perm = false;
                    }
                } else if !uniq_c_perm {
                    let run = if let Some(next_tok) = toks.peek() {
                            !next_tok.common
                        } else {
                            false
                        };
                    if run {
                        bw.write_byte(0x72)?;
                        uniq_c_perm = true;
                    } else {
                        bw.write_byte(0x71)?;
                    }
                }
                let mut mode = 0;
                for quad in tok.quad.iter() {
                    mode <<= 2;
                    mode |= quad.get_mode_id();
                }
                assert!((mode & 0xF8) != 0x70);
                bw.write_byte(mode)?;
                if tok.common {
                    bw.write_byte(pack_chroma(tok.chroma[0][0], tok.chroma[0][1]))?;
                }
                for (quad, chr) in tok.quad.iter().zip(tok.chroma.iter()) {
                    if !quad.is_skip() && !tok.common {
                        let chroma_byte = pack_chroma(chr[0], chr[1]);
                        bw.write_byte(chroma_byte)?;
                    }
                    match quad {
                        QuadToken::Skip => {},
                        QuadToken::Shallow(mode, y0) => {
                            bw.write_byte((*mode << 6) | *y0)?;
                        },
                        QuadToken::LTC(angle, idx) => {
                            bw.write_u16be((u16::from(*angle) << 12) | *idx)?;
                        },
                        QuadToken::Subsampled(yy) => {
                            let yw = pack_luma4(yy);
                            bw.write_u24be(yw)?;
                        },
                        QuadToken::Statistical(pattern, y2) => {
                            bw.write_u16be(*pattern)?;
                            bw.write_byte(y2[0])?;
                            bw.write_byte(y2[1])?;
                        },
                        QuadToken::Extended(angle, yy) => {
                            bw.write_u16be(0x8000 | (u16::from(*angle) << 12) | (u16::from(yy[0]) << 6) | u16::from(yy[1]))?;
                            bw.write_byte(yy[2])?;
                            bw.write_byte(yy[3])?;
                        },
                        QuadToken::Raw(ref raw) => {
                            for row in raw.chunks_exact(4) {
                                let yw = pack_luma4(row);
                                bw.write_u24be(yw)?;
                            }
                        },
                    }
                }

                for (dquad, (squad, schr)) in blk.quad.iter_mut()
                        .zip(tok.quad.iter().zip(tok.chroma.iter())) {
                    if !squad.is_skip() {
                        dquad.u = schr[0];
                        dquad.v = schr[1];
                    }
                    squad.recon_quad(dquad);
                }
            } else {
                *blk = *pblk;
            }
        }
        bw.write_byte(0x73)?; // end marker
        Ok(())
    }
}

impl NAEncoder for UltimotionEncoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                Ok(EncodeParameters {
                        format: NACodecTypeInfo::Video(NAVideoInfo::new(0, 0, true, YUV410_FORMAT)),
                        ..Default::default()
                    })
            },
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(vinfo) => {
                let outinfo = NAVideoInfo::new((vinfo.width + 7) & !7, (vinfo.height + 7) & !7, false, YUV410_FORMAT);
                let mut ofmt = *encinfo;
                ofmt.format = NACodecTypeInfo::Video(outinfo);
                Ok(ofmt)
            }
        }
    }
    fn get_capabilities(&self) -> u64 { ENC_CAPS_SKIPFRAME }
    fn init(&mut self, stream_id: u32, encinfo: EncodeParameters) -> EncoderResult<NAStreamRef> {
        match encinfo.format {
            NACodecTypeInfo::None => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(vinfo) => {
                if vinfo.format != YUV410_FORMAT {
                    return Err(EncoderError::FormatError);
                }
                if ((vinfo.width | vinfo.height) & 7) != 0 {
                    return Err(EncoderError::FormatError);
                }
                if !(8..=1280).contains(&vinfo.width) || !(8..=1024).contains(&vinfo.height) {
                    return Err(EncoderError::FormatError);
                }

                let out_info = NAVideoInfo::new(vinfo.width, vinfo.height, false, vinfo.format);
                let info = NACodecInfo::new("ultimotion", NACodecTypeInfo::Video(out_info), None);
                let mut stream = NAStream::new(StreamType::Video, stream_id, info, encinfo.tb_num, encinfo.tb_den, 0);
                stream.set_num(stream_id as usize);
                let stream = stream.into_ref();

                self.stream = Some(stream.clone());
                self.quality = encinfo.quality;

                if self.quality == 0 || self.quality >= 100 {
                    self.blk_thr = 0;
                    self.chr_thr = 0;
                    self.var_thr = 0;
                } else {
                    self.blk_thr = u32::from(100 - self.quality);
                    self.chr_thr = u32::from(100 - self.quality) / 10;
                    self.var_thr = 16 * u32::from(100 - self.quality);
                }

                let num_blocks = vinfo.width / 8 * vinfo.height / 8;
                self.cur_blks  = vec![Block::default(); num_blocks];
                self.prev_blks = vec![Block::default(); num_blocks];
                self.tokens = Vec::with_capacity(num_blocks);
                self.width  = vinfo.width;
                self.height = vinfo.height;

                Ok(stream)
            },
        }
    }
    fn encode(&mut self, frm: &NAFrame) -> EncoderResult<()> {
        let buf = frm.get_buffer();
        if let NABufferType::None = buf {
            let data = vec![0x73];
            self.pkt = Some(NAPacket::new(self.stream.clone().unwrap(), frm.ts, false, data));
            self.frmcount += 1;
            if self.frmcount == self.key_int {
                self.frmcount = 0;
            }
            return Ok(());
        }
        if let Some(ref vbuf) = buf.get_vbuf() {
            self.load_blocks(vbuf)?;

            let mut dbuf = Vec::with_capacity(4);
            let mut bw   = GrowableMemoryWriter::new_write(&mut dbuf);
            let is_intra = if self.frmcount == 0 {
                    self.encode_intra();
                    true
                } else {
                    self.encode_inter()
                };
            self.write_tokens(&mut bw)?;
            std::mem::swap(&mut self.cur_blks, &mut self.prev_blks);
            self.pkt = Some(NAPacket::new(self.stream.clone().unwrap(), frm.ts, is_intra, dbuf));
            if is_intra {
                self.frmcount = 0;
            }
            self.frmcount += 1;
            if self.frmcount == self.key_int {
                self.frmcount = 0;
            }
            Ok(())
        } else {
            Err(EncoderError::InvalidParameters)
        }
    }
    fn get_packet(&mut self) -> EncoderResult<Option<NAPacket>> {
        let mut npkt = None;
        std::mem::swap(&mut self.pkt, &mut npkt);
        Ok(npkt)
    }
    fn flush(&mut self) -> EncoderResult<()> {
        self.frmcount = 0;
        Ok(())
    }
}

const ENCODER_OPTS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name: KEYFRAME_OPTION, description: KEYFRAME_OPTION_DESC,
        opt_type: NAOptionDefinitionType::Int(Some(0), Some(128)) },
    NAOptionDefinition {
        name: "mode", description: "Encoder block coding mode",
        opt_type: NAOptionDefinitionType::String(Some(&["raw", "lossless", "lossy", "fast"])) },
];

impl NAOptionHandler for UltimotionEncoder {
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
                        "mode" => {
                            if let NAValue::String(ref strval) = option.value {
                                match strval.as_str() {
                                    "raw"       => self.mode = WorkMode::Raw,
                                    "lossless"  => self.mode = WorkMode::Lossless,
                                    "lossy"     => self.mode = WorkMode::Lossy,
                                    "fast"      => self.mode = WorkMode::Fast,
                                    _ => {},
                                };
                            }
                        },
                        _ => {},
                    };
                }
            }
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match name {
            KEYFRAME_OPTION => Some(NAValue::Int(i64::from(self.key_int))),
            "mode" => Some(NAValue::String(self.mode.to_string())),
            _ => None,
        }
    }
}

pub fn get_encoder() -> Box<dyn NAEncoder + Send> {
    Box::new(UltimotionEncoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::*;
    use nihav_core::demuxers::*;
    use nihav_core::muxers::*;
    use nihav_commonfmt::*;
    use crate::*;
    use nihav_codec_support::test::enc_video::*;

    fn test_core(enc_options: &[NAOption], quality: u8, hash: &[u32; 4]) {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        generic_register_all_decoders(&mut dec_reg);
        let mut mux_reg = RegisteredMuxers::new();
        generic_register_all_muxers(&mut mux_reg);
        let mut enc_reg = RegisteredEncoders::new();
        misc_register_all_encoders(&mut enc_reg);

        // sample from private collection
        let dec_config = DecoderTestParams {
                demuxer:        "yuv4mpeg",
                in_name:        "assets/day3b.y4m",
                stream_type:    StreamType::Video,
                limit:          Some(5),
                dmx_reg, dec_reg,
            };
        let enc_config = EncoderTestParams {
                muxer:          "avi",
                enc_name:       "ultimotion",
                out_name:       "ultimotion.avi",
                mux_reg, enc_reg,
            };
        let dst_vinfo = NAVideoInfo {
                width:   0,
                height:  0,
                format:  YUV410_FORMAT,
                flipped: false,
                bits:    12,
            };
        let enc_params = EncodeParameters {
                format:  NACodecTypeInfo::Video(dst_vinfo),
                quality,
                bitrate: 0,
                tb_num:  0,
                tb_den:  0,
                flags:   0,
            };
        //test_encoding_to_file(&dec_config, &enc_config, enc_params, enc_options);
        test_encoding_md5(&dec_config, &enc_config, enc_params, enc_options, hash);
    }
    #[test]
    fn test_ultimotion_encoder_raw() {
        let enc_options = &[
                NAOption { name: "mode", value: NAValue::String("raw".to_string()) },
            ];
        test_core(enc_options, 0, &[0x0c02cb27, 0xb3c3860a, 0xfba1823b, 0x444095f7]);
    }
    #[test]
    fn test_ultimotion_encoder_lossless() {
        let enc_options = &[
                NAOption { name: "mode", value: NAValue::String("lossless".to_string()) },
            ];
        test_core(enc_options, 0, &[0xce360cfb, 0x7bfb0130, 0x5e529bb1, 0x21d8f85e]);
    }
    #[test]
    fn test_ultimotion_encoder_lossy() {
        let enc_options = &[
                NAOption { name: "mode", value: NAValue::String("lossy".to_string()) },
            ];
        test_core(enc_options, 80, &[0xed52dee7, 0xb7d0ae2e, 0x56625189, 0xf15df0cc]);
    }
    #[test]
    fn test_ultimotion_encoder_fast() {
        let enc_options = &[
                NAOption { name: "mode", value: NAValue::String("fast".to_string()) },
            ];
        test_core(enc_options, 80, &[0x67d34919, 0x7cee1fa9, 0xc19e4afd, 0x857abe08]);
    }
}

static INV_LUMA_MAP: [u8; 256] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x02, 0x02, 0x02, 0x03, 0x03, 0x03, 0x03, 0x04, 0x04, 0x04,
    0x05, 0x05, 0x05, 0x05, 0x06, 0x06, 0x06, 0x07, 0x07, 0x07, 0x07, 0x08, 0x08, 0x08, 0x09, 0x09,
    0x09, 0x09, 0x0A, 0x0A, 0x0A, 0x0B, 0x0B, 0x0B, 0x0B, 0x0C, 0x0C, 0x0C, 0x0D, 0x0D, 0x0D, 0x0D,
    0x0E, 0x0E, 0x0E, 0x0F, 0x0F, 0x0F, 0x0F, 0x10, 0x10, 0x10, 0x11, 0x11, 0x11, 0x11, 0x12, 0x12,
    0x12, 0x13, 0x13, 0x13, 0x13, 0x14, 0x14, 0x14, 0x15, 0x15, 0x15, 0x16, 0x16, 0x16, 0x16, 0x17,
    0x17, 0x17, 0x18, 0x18, 0x18, 0x18, 0x19, 0x19, 0x19, 0x1A, 0x1A, 0x1A, 0x1A, 0x1B, 0x1B, 0x1B,
    0x1C, 0x1C, 0x1C, 0x1C, 0x1D, 0x1D, 0x1D, 0x1E, 0x1E, 0x1E, 0x1E, 0x1F, 0x1F, 0x1F, 0x20, 0x20,
    0x20, 0x20, 0x21, 0x21, 0x21, 0x22, 0x22, 0x22, 0x22, 0x23, 0x23, 0x23, 0x24, 0x24, 0x24, 0x24,
    0x25, 0x25, 0x25, 0x26, 0x26, 0x26, 0x26, 0x27, 0x27, 0x27, 0x28, 0x28, 0x28, 0x28, 0x29, 0x29,
    0x29, 0x2A, 0x2A, 0x2A, 0x2B, 0x2B, 0x2B, 0x2B, 0x2C, 0x2C, 0x2C, 0x2D, 0x2D, 0x2D, 0x2D, 0x2E,
    0x2E, 0x2E, 0x2F, 0x2F, 0x2F, 0x2F, 0x30, 0x30, 0x30, 0x31, 0x31, 0x31, 0x31, 0x32, 0x32, 0x32,
    0x33, 0x33, 0x33, 0x33, 0x34, 0x34, 0x34, 0x35, 0x35, 0x35, 0x35, 0x36, 0x36, 0x36, 0x37, 0x37,
    0x37, 0x37, 0x38, 0x38, 0x38, 0x39, 0x39, 0x39, 0x39, 0x3A, 0x3A, 0x3A, 0x3B, 0x3B, 0x3B, 0x3B,
    0x3C, 0x3C, 0x3C, 0x3D, 0x3D, 0x3D, 0x3D, 0x3E, 0x3E, 0x3E, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F,
    0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F, 0x3F,
];
static INV_CHROMA_MAP: [u8; 256] = [
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x2, 0x2, 0x2, 0x2, 0x2,
    0x2, 0x3, 0x3, 0x3, 0x3, 0x3, 0x3, 0x4, 0x4, 0x4, 0x4, 0x4, 0x4, 0x4, 0x5, 0x5,
    0x5, 0x5, 0x5, 0x5, 0x6, 0x6, 0x6, 0x6, 0x6, 0x6, 0x7, 0x7, 0x7, 0x7, 0x7, 0x7,
    0x7, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x9, 0x9, 0x9, 0x9, 0x9, 0x9, 0xA, 0xA, 0xA,
    0xA, 0xA, 0xA, 0xA, 0xB, 0xB, 0xB, 0xB, 0xB, 0xB, 0xC, 0xC, 0xC, 0xC, 0xC, 0xC,
    0xD, 0xD, 0xD, 0xD, 0xD, 0xD, 0xD, 0xE, 0xE, 0xE, 0xE, 0xE, 0xE, 0xF, 0xF, 0xF,
    0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF,
    0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF,
    0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF,
    0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF, 0xF,
];
