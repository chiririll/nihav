use nihav_codec_support::codecs::MV;

#[repr(u8)]
#[derive(Clone,Copy,Debug,PartialEq)]
pub enum BMode {
    L0,
    L1,
    Bi,
}

#[derive(Clone,Copy,Debug,PartialEq,Default)]
pub enum MBType {
    #[default]
    Intra4x4,
    Intra8x8,
    Intra16x16(u8, u8, u8),
    PCM,

    P16x16,
    P16x8,
    P8x16,
    P8x8,
    P8x8Ref0,
    PSkip,

    Direct,
    B16x16(BMode),
    B16x8(BMode, BMode),
    B8x16(BMode, BMode),
    B8x8,
    BSkip,
}

impl MBType {
    pub fn is_intra(self) -> bool {
        matches!(self, MBType::Intra4x4 | MBType::Intra8x8 | MBType::Intra16x16(_, _, _) | MBType::PCM)
    }
    pub fn is_intra16x16(self) -> bool {
        matches!(self, MBType::Intra16x16(_, _, _))
    }
    pub fn is_skip(self) -> bool {
        matches!(self, MBType::PSkip | MBType::BSkip)
    }
    pub fn is_4x4(self) -> bool { self.num_parts() == 4 }
    pub fn is_l0(self, part: usize) -> bool {
        match self {
            MBType::B16x16(mode) => mode == BMode::L0,
            MBType::B16x8(mode0, mode1) | MBType::B8x16(mode0, mode1) => {
                if part == 0 {
                    mode0 == BMode::L0
                } else {
                    mode1 == BMode::L0
                }
            },
            MBType::Direct | MBType::BSkip => false,
            _ => true,
        }
    }
    pub fn is_l1(self, part: usize) -> bool {
        match self {
            MBType::B16x16(mode) => mode == BMode::L1,
            MBType::B16x8(mode0, mode1) | MBType::B8x16(mode0, mode1) => {
                if part == 0 {
                    mode0 == BMode::L1
                } else {
                    mode1 == BMode::L1
                }
            },
            _ => false,
        }
    }
    pub fn num_parts(self) -> usize {
        match self {
            MBType::Intra4x4 | MBType::Intra8x8 | MBType::Intra16x16(_, _, _) | MBType::PCM |
            MBType::PSkip |
            MBType::Direct | MBType::BSkip
                => 1,
            MBType::P16x16 |
            MBType::B16x16(_)
                => 1,
            MBType::P16x8 | MBType::P8x16 |
            MBType::B16x8(_, _) | MBType::B8x16(_, _)
                => 2,
            _ => 4,
        }
    }
    pub fn size(self) -> (usize, usize) {
        match self {
            MBType::Intra4x4 |
            MBType::Intra8x8 |
            MBType::Intra16x16(_, _, _) |
            MBType::PCM |
            MBType::P16x16 |
            MBType::PSkip |
            MBType::Direct |
            MBType::B16x16(_) |
            MBType::BSkip
                => (16, 16),
            MBType::P16x8 | MBType::B16x8(_, _) => (16, 8),
            MBType::P8x16 | MBType::B8x16(_, _) => (8, 16),
            _ => (8, 8),
        }
    }
}

#[derive(Clone,Copy,Debug,PartialEq,Default)]
pub enum SubMBType {
    P8x8,
    P8x4,
    P4x8,
    P4x4,
    #[default]
    Direct8x8,
    B8x8(BMode),
    B8x4(BMode),
    B4x8(BMode),
    B4x4(BMode),
}

impl SubMBType {
    pub fn num_parts(self) -> usize {
        match self {
            SubMBType::P8x8 | SubMBType::Direct8x8 | SubMBType::B8x8(_) => 1,
            SubMBType::P4x4 | SubMBType::B4x4(_) => 4,
            _ => 2,
        }
    }
    pub fn size(self) -> (usize, usize) {
        match self {
            SubMBType::P8x8 | SubMBType::Direct8x8 | SubMBType::B8x8(_) => (8, 8),
            SubMBType::P8x4 | SubMBType::B8x4(_) => (8, 4),
            SubMBType::P4x8 | SubMBType::B4x8(_) => (4, 8),
            SubMBType::P4x4 | SubMBType::B4x4(_) => (4, 4),
        }
    }
    pub fn is_l0(self) -> bool {
        match self {
            SubMBType::B8x8(mode) | SubMBType::B8x4(mode) |
            SubMBType::B4x8(mode) | SubMBType::B4x4(mode) => {
                mode == BMode::L0
            },
            _ => true,
        }
    }
    pub fn is_l1(self) -> bool {
        match self {
            SubMBType::B8x8(mode) | SubMBType::B8x4(mode) |
            SubMBType::B4x8(mode) | SubMBType::B4x4(mode) => {
                mode == BMode::L1
            },
            _ => false,
        }
    }
}

#[repr(u8)]
#[derive(Clone,Copy,Debug,PartialEq,Default)]
pub enum CompactMBType {
    Intra4x4,
    Intra8x8,
    Intra16x16,
    PCM,

    P16x16,
    P16x8,
    P8x16,
    P8x8,
    P8x8Ref0,
    PSkip,

    Direct,
    B16x16,
    B16x8,
    B8x16,
    B8x8,
    BSkip,

    #[default]
    None,
}

impl CompactMBType {
    pub fn is_intra(self) -> bool {
        matches!(self, CompactMBType::Intra4x4 | CompactMBType::Intra8x8 | CompactMBType::Intra16x16)
    }
    pub fn is_intra16orpcm(self) -> bool {
        matches!(self, CompactMBType::Intra16x16 | CompactMBType::PCM)
    }
    pub fn is_skip(self) -> bool {
        matches!(self, CompactMBType::PSkip | CompactMBType::BSkip)
    }
    pub fn is_direct(self) -> bool {
        matches!(self, CompactMBType::BSkip | CompactMBType::Direct | CompactMBType::None)
    }
    pub fn is_inter(self) -> bool {
        !self.is_intra() && !self.is_skip() && self != CompactMBType::PCM
    }
    pub fn is_16x16_ref(self) -> bool {
        matches!(self,
            CompactMBType::Intra4x4 |
            CompactMBType::Intra8x8 |
            CompactMBType::Intra16x16 |
            CompactMBType::PCM |
            CompactMBType::P16x16 |
            CompactMBType::B16x16)
    }
}

impl From<MBType> for CompactMBType {
    fn from(mbtype: MBType) -> Self {
        match mbtype {
            MBType::Intra4x4 => CompactMBType::Intra4x4,
            MBType::Intra8x8 => CompactMBType::Intra8x8,
            MBType::Intra16x16(_, _, _) => CompactMBType::Intra16x16,
            MBType::PCM => CompactMBType::PCM,
            MBType::P16x16 => CompactMBType::P16x16,
            MBType::P16x8 => CompactMBType::P16x8,
            MBType::P8x16 => CompactMBType::P8x16,
            MBType::P8x8 => CompactMBType::P8x8,
            MBType::P8x8Ref0 => CompactMBType::P8x8Ref0,
            MBType::PSkip => CompactMBType::PSkip,
            MBType::Direct => CompactMBType::Direct,
            MBType::B16x16(_) => CompactMBType::B16x16,
            MBType::B16x8(_, _) => CompactMBType::B16x8,
            MBType::B8x16(_, _) => CompactMBType::B8x16,
            MBType::B8x8 => CompactMBType::B8x8,
            MBType::BSkip => CompactMBType::BSkip,
        }
    }
}

#[repr(u8)]
#[derive(Clone,Copy,Debug,PartialEq,Default)]
pub enum IntraPredMode {
    Vertical,
    Horizontal,
    DC,
    DiagDownLeft,
    DiagDownRight,
    VerRight,
    HorDown,
    VerLeft,
    HorUp,
    #[default]
    None,
}

impl IntraPredMode {
    pub fn is_none(self) -> bool { self == IntraPredMode::None }
    pub fn into_pred_idx(self) -> i8 {
        if !self.is_none() {
            self as u8 as i8
        } else {
            -1
        }
    }
}

impl From<u8> for IntraPredMode {
    fn from(val: u8) -> Self {
        match val {
            0 => IntraPredMode::Vertical,
            1 => IntraPredMode::Horizontal,
            2 => IntraPredMode::DC,
            3 => IntraPredMode::DiagDownLeft,
            4 => IntraPredMode::DiagDownRight,
            5 => IntraPredMode::VerRight,
            6 => IntraPredMode::HorDown,
            7 => IntraPredMode::VerLeft,
            8 => IntraPredMode::HorUp,
            _ => IntraPredMode::None,
        }
    }
}

impl From<IntraPredMode> for u8 {
    fn from(val: IntraPredMode) -> Self {
        match val {
            IntraPredMode::Vertical      => 0,
            IntraPredMode::Horizontal    => 1,
            IntraPredMode::DC            => 2,
            IntraPredMode::DiagDownLeft  => 3,
            IntraPredMode::DiagDownRight => 4,
            IntraPredMode::VerRight      => 5,
            IntraPredMode::HorDown       => 6,
            IntraPredMode::VerLeft       => 7,
            IntraPredMode::HorUp         => 8,
            _ => 9,
        }
    }
}

pub const MISSING_POC: u16 = 0xFFFF;

#[derive(Clone,Copy,Debug)]
pub struct PicRef {
    ref_idx: u8
}

pub const MISSING_REF: PicRef = PicRef { ref_idx: 0xFF };
pub const INVALID_REF: PicRef = PicRef { ref_idx: 0xFE };
pub const ZERO_REF: PicRef = PicRef { ref_idx: 0 };
const DIRECT_FLAG: u8 = 0x40;

impl PicRef {
    pub fn new(ref_idx: u8) -> Self {
        Self { ref_idx }
    }
    pub fn not_avail(self) -> bool {
        self == MISSING_REF || self == INVALID_REF
    }
    pub fn index(self) -> usize { (self.ref_idx & !DIRECT_FLAG) as usize }
    pub fn is_direct(self) -> bool { (self.ref_idx & DIRECT_FLAG) != 0 }
    pub fn set_direct(&mut self) { self.ref_idx |= DIRECT_FLAG; }
    pub fn min_pos(self, other: Self) -> Self {
        match (self.not_avail(), other.not_avail()) {
            (true,  true)   => self,
            (false, true)   => self,
            (true,  false)  => other,
            (false, false)  => PicRef::new((self.ref_idx & !DIRECT_FLAG).min(other.ref_idx & !DIRECT_FLAG)),
        }
    }
}

impl Default for PicRef {
    fn default() -> Self { MISSING_REF }
}

impl PartialEq for PicRef {
    fn eq(&self, other: &Self) -> bool {
        (self.ref_idx | DIRECT_FLAG) == (other.ref_idx | DIRECT_FLAG)
    }
}

impl std::fmt::Display for PicRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if *self == MISSING_REF {
            write!(f, "-1")
        } else if *self == INVALID_REF {
            write!(f, "-2")
        } else {
            write!(f, "{}", self.ref_idx & !DIRECT_FLAG)
        }
    }
}

#[derive(Clone,Copy,Default)]
pub struct MBData {
    pub mb_type:        CompactMBType,
    pub cbp:            u8,
    pub coded_flags:    u32,
    pub cmode:          u8,
    pub qp_y:           u8,
    pub qp_u:           u8,
    pub qp_v:           u8,
    pub transform_8x8:  bool,
}

pub fn blk4_to_blk8(blk4: usize) -> usize {
    /*const MAP: [usize; 16] = [ 0, 0, 1, 1, 0, 0, 1, 1, 2, 2, 3, 3, 2, 2, 3, 3 ];
    MAP[blk4 & 0xF]*/
    ((blk4 & 2) >> 1) | ((blk4 & 8) >> 2)
}

#[derive(Clone,Copy)]
pub struct Blk8Data {
    pub ref_idx:    [PicRef; 2],
    pub ncoded_c:   [u8; 2],
}

impl Default for Blk8Data {
    fn default() -> Self {
        Self {
            ref_idx:        [MISSING_REF; 2],
            ncoded_c:       [0; 2],
        }
    }
}

#[derive(Clone,Copy,Default)]
pub struct Blk4Data {
    pub ncoded:     u8,
    pub ipred:      IntraPredMode,
    pub mv:         [MV; 2],
    pub mvd:        [MV; 2],
}

#[repr(align(8))]
#[derive(Default)]
pub struct MVCache {
    pub data:   [[MV; 2]; 25]
}

#[cfg(not(target_arch="x86_64"))]
pub fn mvdiff4(mv1: &[MV; 2], mv2: &[MV; 2]) -> bool {
    let mvd0 = mv1[0] - mv2[0];
    let mvd1 = mv1[1] - mv2[1];
    (mvd0.x.abs() >= 4) || (mvd0.y.abs() >= 4) || (mvd1.x.abs() >= 4) || (mvd1.y.abs() >= 4)
}

#[cfg(target_arch="x86_64")]
pub fn mvdiff4(mv1: &[MV; 2], mv2: &[MV; 2]) -> bool {
    unsafe {
        let mut flag = false;
        let ptr = std::mem::transmute::<*const MV, *const u64>(mv1.as_ptr());
        let mut m0 = *ptr;
        let ptr = std::mem::transmute::<*const MV, *const u64>(mv2.as_ptr());
        let mut m1 = *ptr;
        for _ in 0..4 {
            let tmp = m0.wrapping_sub(m1) as u16;
            flag |= tmp.wrapping_add(3) > 6;
            m0 >>= 16;
            m1 >>= 16;
        }
        flag
    }
}
