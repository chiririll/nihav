use nihav_core::frame::*;
use nihav_codec_support::codecs::{MV, ZERO_MV};
use nihav_codec_support::data::GenericCache;

pub trait FrameMV {
    fn add_pred(pred: MV, diff: MV, fcode: u8) -> MV;
    fn scale(&self, trb: i32, trd: i32) -> MV;
    fn b_sub(pvec: MV, fwdvec: MV, bvec: MV, trb: i32, trd: i32) -> MV;
}

impl FrameMV for MV {
    fn add_pred(pred: MV, diff: MV, fcode: u8) -> MV {
        let shift = fcode.saturating_sub(1);
        let high =  (1 << (shift + 5)) - 1;
        let low  = -(1 << (shift + 5));
        let range = 1 << (shift + 6);

        let mut mv = pred + diff;
        if mv.x < low {
            mv.x += range;
        }
        if mv.x > high {
            mv.x -= range;
        }
        if mv.y < low {
            mv.y += range;
        }
        if mv.y > high {
            mv.y -= range;
        }
        mv
    }
    fn scale(&self, trb: i32, trd: i32) -> MV {
        if (trd == 0) || (trb == 0) {
            ZERO_MV
        } else {
            MV { x: (((self.x as i32) * trb) / trd) as i16, y: (((self.y as i32) * trb) / trd) as i16 }
        }
    }
    fn b_sub(pvec: MV, fwdvec: MV, bvec: MV, trb: i32, trd: i32) -> MV {
        let bscale = trb  - trd;
        let x = if bvec.x != 0 { fwdvec.x - pvec.x } else if trd != 0 { (bscale * (pvec.x as i32) / trd) as i16 } else { 0 };
        let y = if bvec.y != 0 { fwdvec.y - pvec.y } else if trd != 0 { (bscale * (pvec.y as i32) / trd) as i16 } else { 0 };
        MV { x, y }
    }
}

#[derive(Copy,Clone,Debug,PartialEq)]
pub enum Shape {
    Rect,
    Binary,
    BinaryOnly,
    Grayscale
}

#[derive(Default,Copy,Clone,Debug,PartialEq)]
pub enum Sprite {
    #[default]
    NotUsed,
    Static,
    GMC,
}

#[derive(Default,Clone,Copy)]
pub struct SpriteInfo {
    pub sprite:             Sprite,
    pub num_warp_points:    usize,
    pub warping_accuracy:   u8,
    pub brightness_change:  bool,
    pub low_latency:        bool,
}

#[derive(Clone)]
pub struct QuantInfo {
    pub quant_type:         bool,
    pub intra_mat:          [u8; 64],
    pub inter_mat:          [u8; 64],
}

impl Default for QuantInfo {
    fn default() -> Self {
        Self {
            quant_type: false,
            intra_mat:  DEF_INTRA_MAT,
            inter_mat:  DEF_INTER_MAT,
        }
    }
}

#[allow(dead_code)]
pub struct VideoObjectLayer {
    pub vol_type:               u8,
    pub version:                u8,
    pub low_delay:              bool,
    pub shape:                  Shape,
    pub time_res:               u32,
    pub fixed_rate:             Option<u32>,
    pub width:                  usize,
    pub height:                 usize,
    pub obmc_disable:           bool,
    pub sprite_info:            SpriteInfo,
    pub quant_info:             QuantInfo,
    pub quarter_sample:         bool,
    pub complexity_estimation:  bool,
    pub resync_marker_disable:  bool,
    pub newpred_enable:         bool,
}

pub struct PicInfo {
    pub pic_type:               FrameType,
    pub vop_time_increment:     u32,
    pub modulo_time_base:       u32,
    pub fcode_f:                u8,
    pub fcode_b:                u8,
    pub rounding:               bool,
}

#[derive(Clone,Copy,Debug,Default,PartialEq)]
pub enum MBType {
    #[default]
    Intra,
    Skip,
    Inter,
    Inter4MV,
    Direct,
    Interpolate,
    Backward,
    Forward,
}

pub struct Macroblock {
    pub mb_type:                MBType,
    pub quant:                  u8,
    pub mvs:                    [MV; 4],
    pub cbp:                    u8,
    pub ac_pred:                bool,
    pub coeffs:                 [[i16; 64]; 6],
}

impl Macroblock {
    pub fn new() -> Self {
        Self {
            mb_type:    MBType::Intra,
            quant:      0,
            mvs:        [ZERO_MV; 4],
            cbp:        0,
            ac_pred:    false,
            coeffs:     [[0; 64]; 6],
        }
    }
}

#[allow(dead_code)]
pub struct PicState {
    pub timestamp:      u32,
    pub mb_type:        Vec<MBType>,
    pub mv:             Vec<[MV; 4]>,
    pub mb_w:           usize,
    pub mb_h:           usize,
}

impl PicState {
    pub fn new(timestamp: u32, mb_w: usize, mb_h: usize) -> Self {
        Self {
            timestamp,
            mb_w, mb_h,
            mb_type:    vec![MBType::Skip; mb_w * mb_h],
            mv:         vec![[ZERO_MV; 4]; mb_w * mb_h],
        }
    }
}

const DEF_INTRA_MAT: [u8; 64] = [
     8, 17, 18, 19, 21, 23, 25, 27,
    17, 18, 19, 21, 23, 25, 27, 28,
    20, 21, 22, 23, 24, 26, 28, 30,
    21, 22, 23, 24, 26, 28, 30, 32,
    22, 23, 24, 26, 38, 30, 32, 35,
    23, 24, 26, 38, 30, 32, 35, 38,
    25, 26, 28, 30, 32, 35, 38, 41,
    27, 28, 30, 32, 35, 38, 41, 45
];

const DEF_INTER_MAT: [u8; 64] = [
    16, 17, 18, 19, 20, 21, 22, 23,
    17, 18, 19, 20, 21, 22, 23, 24,
    18, 19, 20, 21, 22, 23, 24, 25,
    19, 20, 21, 22, 23, 24, 26, 27,
    20, 21, 22, 23, 25, 26, 27, 28,
    21, 22, 23, 24, 26, 27, 28, 30,
    22, 23, 24, 26, 27, 28, 30, 31,
    23, 24, 25, 27, 28, 30, 31, 33
];

#[derive(Default)]
pub struct NState {
    pub has_l:  bool,
    pub has_t:  bool,
    pub has_tr: bool,
}

pub type SavedAC = [[i16; 7]; 2];

pub struct PredState {
    pub y_dc:           GenericCache<i16>,
    pub y_ac:           GenericCache<SavedAC>,
    pub u_dc:           GenericCache<i16>,
    pub u_ac:           GenericCache<SavedAC>,
    pub v_dc:           GenericCache<i16>,
    pub v_ac:           GenericCache<SavedAC>,
    pub qps:            GenericCache<u8>,
    pub mv:             GenericCache<MV>,
    pub fcode_f:        u8,
    pub fcode_b:        u8,
    pub pred_mv_f:      MV,
    pub pred_mv_b:      MV,
}

impl PredState {
    pub fn new() -> Self {
        Self {
            y_dc:       GenericCache::new(0, 0, 0),
            y_ac:       GenericCache::new(0, 0, SavedAC::default()),
            u_dc:       GenericCache::new(0, 0, 0),
            u_ac:       GenericCache::new(0, 0, SavedAC::default()),
            v_dc:       GenericCache::new(0, 0, 0),
            v_ac:       GenericCache::new(0, 0, SavedAC::default()),
            qps:        GenericCache::new(0, 0, 0),
            mv:         GenericCache::new(0, 0, ZERO_MV),
            fcode_f:    0,
            fcode_b:    0,
            pred_mv_f:  ZERO_MV,
            pred_mv_b:  ZERO_MV,
        }
    }
    pub fn set_fcodes(&mut self, fcode_f: u8, fcode_b: u8) {
        self.fcode_f = fcode_f;
        self.fcode_b = fcode_b;
    }
    pub fn resize(&mut self, mb_w: usize) {
        self.y_dc = GenericCache::new(2, mb_w * 2 + 1, 1 << (8 + 2));
        self.y_ac = GenericCache::new(2, mb_w * 2 + 1, SavedAC::default());
        self.u_dc = GenericCache::new(1, mb_w + 1, 1 << (8 + 2));
        self.u_ac = GenericCache::new(1, mb_w + 1, SavedAC::default());
        self.v_dc = GenericCache::new(1, mb_w + 1, 1 << (8 + 2));
        self.v_ac = GenericCache::new(1, mb_w + 1, SavedAC::default());
        self.qps  = GenericCache::new(1, mb_w + 1, 1);
        self.mv   = GenericCache::new(2, mb_w * 2 + 1, ZERO_MV);
    }
    pub fn reset(&mut self) {
        self.y_dc.reset();
        self.y_ac.reset();
        self.u_dc.reset();
        self.u_ac.reset();
        self.v_dc.reset();
        self.v_ac.reset();
        self.qps.reset();
        self.mv.reset();
        self.pred_mv_f = ZERO_MV;
        self.pred_mv_b = ZERO_MV;
    }
    pub fn update_row(&mut self) {
        self.y_dc.update_row();
        self.y_ac.update_row();
        self.u_dc.update_row();
        self.u_ac.update_row();
        self.v_dc.update_row();
        self.v_ac.update_row();
        self.qps.update_row();
        self.mv.update_row();
        self.pred_mv_f = ZERO_MV;
        self.pred_mv_b = ZERO_MV;
    }
    pub fn dc_pred(dcs: &GenericCache<i16>, off: usize) -> (i16, bool) {
        let addr = dcs.xpos + off;
        let a_dc = dcs.data[addr - 1];
        let b_dc = dcs.data[addr - 1 - dcs.stride];
        let c_dc = dcs.data[addr     - dcs.stride];
        if (a_dc - b_dc).abs() < (b_dc - c_dc).abs() {
            (c_dc, true)
        } else {
            (a_dc, false)
        }
    }
    pub fn set_quant(&mut self, mb_x: usize, quant: u8) {
        self.qps.data[self.qps.xpos + mb_x] = quant;
    }
    pub fn predict_dc(&mut self, mb_x: usize, cur_dc: i16, dc_scale: u8, n: usize) -> (i16, bool) {
        match n {
            0..=3 => {
                let cur_addr = mb_x * 2 + (n & 1) + (n / 2) * self.y_dc.stride;
                let (diff, dir) = Self::dc_pred(&self.y_dc, cur_addr);
                let dc = ((cur_dc + (diff + i16::from(dc_scale >> 1)) / i16::from(dc_scale)) * i16::from(dc_scale)).max(-2048).min(2047);
                self.y_dc.data[self.y_dc.xpos + cur_addr] = dc;
                (dc, dir)
            },
            4 => {
                let (diff, dir) = Self::dc_pred(&self.u_dc, mb_x);
                let dc = ((cur_dc + (diff + i16::from(dc_scale >> 1)) / i16::from(dc_scale)) * i16::from(dc_scale)).max(-2048).min(2047);
                self.u_dc.data[self.u_dc.xpos + mb_x] = dc;
                (dc, dir)
            },
            5 => {
                let (diff, dir) = Self::dc_pred(&self.v_dc, mb_x);
                let dc = ((cur_dc + (diff + i16::from(dc_scale >> 1)) / i16::from(dc_scale)) * i16::from(dc_scale)).max(-2048).min(2047);
                self.v_dc.data[self.v_dc.xpos + mb_x] = dc;
                (dc, dir)
            },
            _ => unreachable!(),
        }

    }
    pub fn predict_ac(&mut self, blk: &mut [i16; 64], mb_x: usize, n: usize, dir: bool) {
        let quant = self.qps.data[self.qps.xpos + mb_x];
        let (pq, src) = match (n, dir) {
                (4, true)  => {
                    (self.qps.data[self.qps.xpos + mb_x - self.qps.stride],
                     &self.u_ac.data[self.u_ac.xpos + mb_x - self.u_ac.stride][1])
                },
                (4, false) => {
                    (self.qps.data[self.qps.xpos + mb_x - 1],
                     &self.u_ac.data[self.u_ac.xpos + mb_x - 1][0])
                },
                (5, true)  => {
                    (self.qps.data[self.qps.xpos + mb_x - self.qps.stride],
                     &self.v_ac.data[self.v_ac.xpos + mb_x - self.v_ac.stride][1])
                },
                (5, false) => {
                    (self.qps.data[self.qps.xpos + mb_x - 1],
                     &self.v_ac.data[self.v_ac.xpos + mb_x - 1][0])
                },
                (0 | 1, true) => {
                    (self.qps.data[self.qps.xpos + mb_x - self.qps.stride],
                     &self.y_ac.data[self.y_ac.xpos + mb_x * 2 + n - self.y_ac.stride][1])
                },
                (_, true) => {
                    (quant, &self.y_ac.data[self.y_ac.xpos + mb_x * 2 + (n & 1)][1])
                },
                (0 | 2, false) => {
                    (self.qps.data[self.qps.xpos + mb_x - 1],
                     &self.y_ac.data[self.y_ac.xpos + mb_x * 2 + (n / 2) * self.y_ac.stride - 1][0])
                },
                (_, false) => {
                    (quant, &self.y_ac.data[self.y_ac.xpos + mb_x * 2 + (n / 2) * self.y_ac.stride][0])
                },
            };
        if dir {
            for (dst, &src) in blk[1..8].iter_mut().zip(src.iter()) {
                *dst += (i32::from(src) * i32::from(pq) / i32::from(quant)) as i16;
            }
        } else {
            for (dst, &src) in blk[8..].chunks_exact_mut(8).zip(src.iter()) {
                dst[0] += (i32::from(src) * i32::from(pq) / i32::from(quant)) as i16;
            }
        }
        let dblock = match n {
                0..=3 => &mut self.y_ac.data[self.y_ac.xpos + mb_x * 2 + (n & 1) + (n / 2) * self.y_ac.stride],
                4 => &mut self.u_ac.data[self.u_ac.xpos + mb_x],
                _ => &mut self.v_ac.data[self.v_ac.xpos + mb_x],
            };
        for (dst, srow) in dblock[0].iter_mut().zip(blk[8..].chunks_exact(8)) {
            *dst = srow[0];
        }
        dblock[1].copy_from_slice(&blk[1..8]);
    }
    pub fn save_ac(&mut self, blk: &[i16; 64], mb_x: usize, n: usize) {
        let dblock = match n {
                0..=3 => &mut self.y_ac.data[self.y_ac.xpos + mb_x * 2 + (n & 1) + (n / 2) * self.y_ac.stride],
                4 => &mut self.u_ac.data[self.u_ac.xpos + mb_x],
                _ => &mut self.v_ac.data[self.v_ac.xpos + mb_x],
            };
        for (dst, srow) in dblock[0].iter_mut().zip(blk[8..].chunks_exact(8)) {
            *dst = srow[0];
        }
        dblock[1].copy_from_slice(&blk[1..8]);
    }
    pub fn pred_mv(&mut self, mb_x: usize, nstate: &NState, mv: MV) -> MV {
        let addr = self.mv.xpos + mb_x * 2;
        let mv_1 = self.mv.data[addr - 1];
        let mv_2 = self.mv.data[addr - self.mv.stride];
        let mv_3 = self.mv.data[addr - self.mv.stride + 2];
        let pred = match (nstate.has_l as u8) + (nstate.has_t as u8) + (nstate.has_tr as u8) {
                0 => ZERO_MV,
                1 => MV{ x: mv_1.x | mv_2.x | mv_3.x, y: mv_1.y | mv_2.y | mv_3.y },
                _ => MV::pred(mv_1, mv_2, mv_3),
            };
        let new_mv = MV::add_pred(pred, mv, self.fcode_f);
        self.mv.data[addr] = new_mv;
        self.mv.data[addr + 1] = new_mv;
        self.mv.data[addr + self.mv.stride] = new_mv;
        self.mv.data[addr + self.mv.stride + 1] = new_mv;
        new_mv
    }
    pub fn pred_mv_f(&mut self, mv: MV) -> MV {
        let new_mv = MV::add_pred(self.pred_mv_f, mv, self.fcode_f);
        self.pred_mv_f = new_mv;
        new_mv
    }
    pub fn pred_mv_b(&mut self, mv: MV) -> MV {
        let new_mv = MV::add_pred(self.pred_mv_b, mv, self.fcode_b);
        self.pred_mv_b = new_mv;
        new_mv
    }
    pub fn pred_4mv(&mut self, mb_x: usize, nstate: &NState, mvs: &mut [MV; 4]) {
        const MV_3_OFFS: [u8; 4] = [0, 1, 1, 3]; // real xpos = 2 - offs
        let base_addr = self.mv.xpos + mb_x * 2;
        for (n, (mv, &mv3_off)) in mvs.iter_mut().zip(MV_3_OFFS.iter()).enumerate() {
            let cur_addr = base_addr + (n & 1) + (n / 2) * self.mv.stride;
            let mv_1 = self.mv.data[cur_addr - 1];
            let mv_2 = self.mv.data[cur_addr - self.mv.stride];
            let mv_3 = self.mv.data[cur_addr - self.mv.stride + 2 - usize::from(mv3_off)];
            let mut num_cand = 0;
            if n == 1 || n == 3 || nstate.has_l {
                num_cand += 1;
            }
            if n == 2 || n == 3 || nstate.has_t {
                num_cand += 1;
            }
            if n == 2 || n == 3 || nstate.has_tr {
                num_cand += 1;
            }
            let pred = match num_cand {
                    0 => ZERO_MV,
                    1 => MV{ x: mv_1.x | mv_2.x | mv_3.x, y: mv_1.y | mv_2.y | mv_3.y },
                    _ => MV::pred(mv_1, mv_2, mv_3),
                };
            *mv = MV::add_pred(pred, *mv, self.fcode_f);
            self.mv.data[cur_addr] = *mv;
        }
    }
}
