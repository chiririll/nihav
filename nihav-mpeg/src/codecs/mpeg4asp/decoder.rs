use nihav_core::codecs::*;
use nihav_codec_support::codecs::{MV, ZERO_MV};

use super::bitstream::*;
use super::dispatch::*;
use super::dsp::*;
use super::types::*;

#[derive(Clone,Copy,Default)]
pub struct FrameTimestamp {
    pub base:       u32,
    pub offset:     u32,
}

impl FrameTimestamp {
    pub fn get_ts(self, base_rate: u32) -> u64 {
        u64::from(self.base) * u64::from(base_rate) + u64::from(self.offset)
    }
}

pub type OptionalRef = Option<NAVideoBufferRef<u8>>;

pub struct FrameContext {
    fdec:           FrameDecoder,
    cur_ts:         u64,
    bwd_ref:        OptionalRef,
    bwd_ts:         u64,
    fwd_ref:        OptionalRef,
    fwd_ts:         u64,
    pred:           PredState,
    dsp:            DSP,
}

impl FrameContext {
    pub fn new() -> Self {
        Self {
            fdec:           FrameDecoder::new(),
            cur_ts:         0,
            bwd_ref:        None,
            bwd_ts:         0,
            fwd_ref:        None,
            fwd_ts:         0,
            pred:           PredState::new(),
            dsp:            DSP::new(),
        }
    }
    pub fn use_xvid_idct(&mut self) {
        self.dsp.use_xvid_idct();
    }
    pub fn resize(&mut self, mb_w: usize) {
        self.pred.resize(mb_w);
    }
    pub fn get_dimensions(&self) -> (usize, usize) { self.fdec.get_dimensions() }
    pub fn decode_frame_header(&mut self, src: &[u8], seq: &SequenceContext) -> DecoderResult<PicInfo> {
        self.fdec.init(seq.width, seq.height);
        if let Some(ref vol) = seq.vol {
            self.fdec.parse_vop_header(src, vol)
        } else {
            self.fdec.parse_vop_short_header(src)
        }
    }
    pub fn set_refs(&mut self, cur_ts: u64, fwd_ref: OptionalRef, fwd_ts: u64, bwd_ref: OptionalRef, bwd_ts: u64) {
        self.cur_ts  = cur_ts;
        self.fwd_ref = fwd_ref;
        self.fwd_ts  = fwd_ts;
        self.bwd_ref = bwd_ref;
        self.bwd_ts  = bwd_ts;
    }
    fn calc_mvpos_blk(mv: MV, x: usize, y: usize, mb_w: usize, mb_h: usize, bsize: usize) -> (usize, usize) {
        let mut xpos = (x as isize) + (((mv.x + 1) >> 1) as isize) + (bsize as isize);
        if (mv.x & 1) != 0 {
            xpos += 1;
        }
        let mut ypos = (y as isize) + (((mv.y + 1) >> 1) as isize) + (bsize as isize);
        if (mv.y & 1) != 0 {
            ypos += 1;
        }
        let xpos = (xpos.max(1) as usize).min(mb_w * 16);
        let ypos = (ypos.max(1) as usize).min(mb_h * 16);
        (xpos, ypos)
    }
    fn calc_mvpos(mv: MV, mb_x: usize, mb_y: usize, mb_w: usize, mb_h: usize) -> (usize, usize) {
        Self::calc_mvpos_blk(mv, mb_x * 16, mb_y * 16, mb_w, mb_h, 16)
    }
    fn calc_mvpos_4mv(mvs: &[MV; 4], mb_x: usize, mb_y: usize, mb_w: usize, mb_h: usize) -> (usize, usize) {
        let sum_mv = mvs[0] + mvs[1] + mvs[2] + mvs[3];
        let avg_mv = MV {x: (sum_mv.x + 3) >> 2, y: (sum_mv.y + 3) >> 2 };
        let (mut xpos, mut ypos) = Self::calc_mvpos_blk(avg_mv, mb_x * 16, mb_y * 16, mb_w, mb_h, 8);
        for (n, &mv) in mvs.iter().enumerate() {
            let (xcand, ycand) = Self::calc_mvpos_blk(mv, mb_x * 16 + (n & 1) * 8, mb_y * 16 + (n / 2) * 8, mb_w, mb_h, 8);
            if (ycand > ypos) || (ycand == ypos && xcand > xpos) {
                xpos = xcand;
                ypos = ycand;
            }
        }
        (xpos, ypos)
    }
    pub fn decode_frame(&mut self, src: &[u8], mut frm: NASimpleVideoFrame<u8>, seq: &SequenceContext, pic_info: &PicInfo, pstate: &mut PicState, helper: &mut dyn MTHelper) -> DecoderResult<()> {
        let (mb_w, mb_h) = self.fdec.get_dimensions();
        let ftype = pic_info.pic_type;

        let mut mb = Macroblock::new();
        self.pred.reset();
        self.pred.set_fcodes(pic_info.fcode_f, pic_info.fcode_b);
        self.fdec.start_decoding();
        self.dsp.set_rounding(pic_info.rounding);
        let mut mb_addr = 0;
        for mb_y in 0..mb_h {
            for mb_x in 0..mb_w {
                let colo_not_coded = ftype == FrameType::B && helper.is_colo_mb_not_coded(mb_addr, self.fwd_ts as u32)?;
                self.fdec.decode_mb(src, &mut mb, colo_not_coded, mb_x, &mut self.pred, &seq.qinfo)?;

                pstate.mb_type[mb_addr] = mb.mb_type;
                match mb.mb_type {
                    MBType::Intra => {
                        self.dsp.put_mb(&mut frm, &mut mb, mb_x, mb_y);
                    },
                    MBType::Skip => {
                        if let Some(pframe) = self.fwd_ref.clone() {
                            helper.wait_for_mb(mb_x * 16 + 16, mb_y * 16 + 16, self.fwd_ts as u32)?;
                            self.dsp.mb_mv(&mut frm, mb_x, mb_y, pframe, ZERO_MV);
                        }
                    },
                    MBType::Inter => {
                        if let Some(pframe) = self.fwd_ref.clone() {
                            let (xpos, ypos) = Self::calc_mvpos(mb.mvs[0], mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.fwd_ts as u32)?;
                            self.dsp.mb_mv(&mut frm, mb_x, mb_y, pframe, mb.mvs[0]);
                            self.dsp.add_coeffs(&mut frm, &mut mb, mb_x, mb_y);
                        }
                    },
                    MBType::Inter4MV => {
                        if let Some(pframe) = self.fwd_ref.clone() {
                            let (xpos, ypos) = Self::calc_mvpos_4mv(&mb.mvs, mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.fwd_ts as u32)?;
                            self.dsp.mb_4mv(&mut frm, mb_x, mb_y, pframe, &mb.mvs);
                            self.dsp.add_coeffs(&mut frm, &mut mb, mb_x, mb_y);
                        }
                    },
                    MBType::Direct => {
                        let trd = (self.fwd_ts - self.bwd_ts) as i32;
                        let trb = (self.cur_ts - self.bwd_ts) as i32;
                        validate!(trd > 0 && trb > 0);
                        let diff_mv = mb.mvs[0];
                        let mut f_mvs = [ZERO_MV; 4];
                        let mut b_mvs = [ZERO_MV; 4];
                        let colo_mvs = helper.get_colo_mvs(mb_addr, self.fwd_ts as u32)?;
                        for ((f_mv, b_mv), &src_mv) in f_mvs.iter_mut().zip(b_mvs.iter_mut()).zip(colo_mvs.iter()) {
                            let ref_mv = src_mv.scale(trb, trd);
                            *f_mv = ref_mv + diff_mv;
                            *b_mv = MV::b_sub(src_mv, *f_mv, diff_mv, trb, trd);
                        }
                        if let (Some(f_frame), Some(b_frame)) = (self.fwd_ref.clone(), self.bwd_ref.clone()) {
                            let (xpos, ypos) = Self::calc_mvpos_4mv(&f_mvs, mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.fwd_ts as u32)?;
                            self.dsp.mb_4mv(&mut frm, mb_x, mb_y, f_frame, &f_mvs);
                            let (xpos, ypos) = Self::calc_mvpos_4mv(&b_mvs, mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.bwd_ts as u32)?;
                            self.dsp.avg_mb_4mv(&mut frm, mb_x, mb_y, b_frame, &b_mvs);
                            self.dsp.add_coeffs(&mut frm, &mut mb, mb_x, mb_y);
                        }
                    },
                    MBType::Interpolate => {
                        if let (Some(f_frame), Some(b_frame)) = (self.fwd_ref.clone(), self.bwd_ref.clone()) {
                            let (xpos, ypos) = Self::calc_mvpos(mb.mvs[0], mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.fwd_ts as u32)?;
                            self.dsp.mb_mv(&mut frm, mb_x, mb_y, f_frame, mb.mvs[0]);
                            let (xpos, ypos) = Self::calc_mvpos(mb.mvs[1], mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.bwd_ts as u32)?;
                            self.dsp.avg_mb_mv(&mut frm, mb_x, mb_y, b_frame, mb.mvs[1]);
                            self.dsp.add_coeffs(&mut frm, &mut mb, mb_x, mb_y);
                        }
                    },
                    MBType::Backward => {
                        if let Some(pframe) = self.bwd_ref.clone() {
                            let (xpos, ypos) = Self::calc_mvpos(mb.mvs[0], mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.bwd_ts as u32)?;
                            self.dsp.mb_mv(&mut frm, mb_x, mb_y, pframe, mb.mvs[0]);
                            self.dsp.add_coeffs(&mut frm, &mut mb, mb_x, mb_y);
                        }
                    },
                    MBType::Forward => {
                        if let Some(pframe) = self.fwd_ref.clone() {
                            let (xpos, ypos) = Self::calc_mvpos(mb.mvs[0], mb_x, mb_y, mb_w, mb_h);
                            helper.wait_for_mb(xpos, ypos, self.fwd_ts as u32)?;
                            self.dsp.mb_mv(&mut frm, mb_x, mb_y, pframe, mb.mvs[0]);
                            self.dsp.add_coeffs(&mut frm, &mut mb, mb_x, mb_y);
                        }
                    },
                }
                pstate.mv[mb_addr] = mb.mvs;
                mb_addr += 1;
                helper.update_pos(mb_addr);
            }
            self.pred.update_row();
        }
        Ok(())
    }
}

pub struct SequenceContext {
    width:          usize,
    height:         usize,
    vol:            Option<VideoObjectLayer>,
    qinfo:          QuantInfo,
    bwd_ts:         FrameTimestamp,
    fwd_ts:         FrameTimestamp,
}

impl SequenceContext {
    pub fn new() -> Self {
        Self {
            width:          0,
            height:         0,
            vol:            None,
            qinfo:          QuantInfo::default(),
            bwd_ts:         FrameTimestamp::default(),
            fwd_ts:         FrameTimestamp::default(),
        }
    }
    pub fn get_dimensions(&self) -> (usize, usize) { (self.width, self.height) }
    pub fn set_dimensions(&mut self, width: usize, height: usize) {
        self.width  = width;
        self.height = height;
    }
    pub fn set_vol(&mut self, vol: VideoObjectLayer) {
        self.set_dimensions(vol.width, vol.height);
        self.qinfo = vol.quant_info.clone();
        self.vol = Some(vol);
    }
    pub fn update_ts(&mut self, pic_info: &PicInfo) -> (u64, u64, u64) {
        match pic_info.pic_type {
            FrameType::I | FrameType::P | FrameType::Skip => {
                self.bwd_ts = self.fwd_ts;
                if let Some(ref vol) = self.vol {
                    self.fwd_ts.base += pic_info.modulo_time_base;
                    self.fwd_ts.offset = pic_info.vop_time_increment;
                    (self.fwd_ts.get_ts(vol.time_res), self.bwd_ts.get_ts(vol.time_res), 0)
                } else {
                    self.fwd_ts.offset += 1;
                    (self.fwd_ts.get_ts(0), self.bwd_ts.get_ts(0), 0)
                }
            },
            FrameType::B => {
                let mut cur_ts = self.bwd_ts;
                cur_ts.base += pic_info.modulo_time_base;
                cur_ts.offset = pic_info.vop_time_increment;
                if let Some(ref vol) = self.vol {
                    (cur_ts.get_ts(vol.time_res),
                     self.fwd_ts.get_ts(vol.time_res),
                     self.bwd_ts.get_ts(vol.time_res))
                } else {
                    unreachable!()
                }
            },
            _ => unimplemented!()
        }
    }
}
