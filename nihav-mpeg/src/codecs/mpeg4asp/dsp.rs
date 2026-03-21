use nihav_core::frame::*;
use nihav_codec_support::codecs::MV;
use nihav_codec_support::codecs::blockdsp::{BlkInterpFunc, edge_emu};
use nihav_codec_support::codecs::h263::code::*;
use nihav_codec_support::codecs::h263::data::H263_CHROMA_ROUND;
use super::types::*;

// IDCT based on C version of Walken IDCT from Xvid

const ROW_ROUNDING: [i32; 8] = [ 65536, 3597, 2260, 1203, 0, 120, 512, 512 ];
const ROW_TAB0: &[i32; 7] = &[ 22725, 21407, 19266, 16384, 12873,  8867, 4520 ];
const ROW_TAB1: &[i32; 7] = &[ 31521, 29692, 26722, 22725, 17855, 12299, 6270 ];
const ROW_TAB2: &[i32; 7] = &[ 29692, 27969, 25172, 21407, 16819, 11585, 5906 ];
const ROW_TAB3: &[i32; 7] = &[ 26722, 25172, 22654, 19266, 15137, 10426, 5315 ];
const ROW_TABS: [&[i32; 7]; 8] = [ ROW_TAB0, ROW_TAB1, ROW_TAB2, ROW_TAB3, ROW_TAB0, ROW_TAB1, ROW_TAB2, ROW_TAB3 ];

const COL_C1: i32 =  13036;
const COL_C2: i32 =  27146;
const COL_C3: i32 = -21746;
const CSQRT2: i32 =  23170;

macro_rules! idct {
    (row; $c0:expr, $c1:expr, $c2:expr, $c3:expr, $c4:expr, $c5:expr, $c6:expr, $c7:expr, $tab:expr, $round:expr) => {
        let t00 = $tab[3] * i32::from($c0) + $round;
        let t01 = $tab[3] * i32::from($c4);
        let t02 = $tab[1] * i32::from($c2) + $tab[5] * i32::from($c6);
        let t03 = $tab[5] * i32::from($c2) - $tab[1] * i32::from($c6);

        let t10 = t00 + t01 + t02;
        let t11 = t00 - t01 - t03;
        let t12 = t00 - t01 + t03;
        let t13 = t00 + t01 - t02;

        let t20 = $tab[0] * i32::from($c1) + $tab[2] * i32::from($c3) + $tab[4] * i32::from($c5) + $tab[6] * i32::from($c7);
        let t21 = $tab[2] * i32::from($c1) - $tab[6] * i32::from($c3) - $tab[0] * i32::from($c5) - $tab[4] * i32::from($c7);
        let t22 = $tab[4] * i32::from($c1) - $tab[0] * i32::from($c3) + $tab[6] * i32::from($c5) + $tab[2] * i32::from($c7);
        let t23 = $tab[6] * i32::from($c1) - $tab[4] * i32::from($c3) + $tab[2] * i32::from($c5) - $tab[0] * i32::from($c7);

        $c0 = ((t10 + t20) >> 11) as i16;
        $c7 = ((t10 - t20) >> 11) as i16;
        $c1 = ((t11 + t21) >> 11) as i16;
        $c6 = ((t11 - t21) >> 11) as i16;
        $c2 = ((t12 + t22) >> 11) as i16;
        $c5 = ((t12 - t22) >> 11) as i16;
        $c3 = ((t13 + t23) >> 11) as i16;
        $c4 = ((t13 - t23) >> 11) as i16;
    };
    (col; $c0:expr, $c1:expr, $c2:expr, $c3:expr, $c4:expr, $c5:expr, $c6:expr, $c7:expr) => {
        let t00 = (i32::from($c7) * COL_C1 >> 16) + i32::from($c1);
        let t01 = (i32::from($c1) * COL_C1 >> 16) - i32::from($c7);
        let t02 = (i32::from($c5) * COL_C3 >> 16) + i32::from($c3);
        let t03 = (i32::from($c3) * COL_C3 >> 16) - i32::from($c5);

        let t10 = t00 + t02;
        let t11 = t00 - t02;
        let t12 = t01 + t03;
        let t13 = t01 - t03;
        let t14 = (t11 + t12) * CSQRT2 >> 15;
        let t15 = (t11 - t12) * CSQRT2 >> 15;

        let t20 =  i32::from($c2) + (i32::from($c6) * COL_C2 >> 16);
        let t21 = -i32::from($c6) + (i32::from($c2) * COL_C2 >> 16);
        let t22 = i32::from($c0 + $c4);
        let t23 = i32::from($c0 - $c4);

        $c0 = ((t22 + t20 + t10) >> 6) as i16;
        $c7 = ((t22 + t20 - t10) >> 6) as i16;

        $c3 = ((t22 - t20 + t13) >> 6) as i16;
        $c4 = ((t22 - t20 - t13) >> 6) as i16;

        $c1 = ((t23 + t21 + t14) >> 6) as i16;
        $c6 = ((t23 + t21 - t14) >> 6) as i16;

        $c2 = ((t23 - t21 + t15) >> 6) as i16;
        $c5 = ((t23 - t21 - t15) >> 6) as i16;
    }
}

fn xvidish_idct(block: &mut [i16; 64]) {
    for (row, (&tab, &rounding)) in block.chunks_exact_mut(8)
            .zip(ROW_TABS.iter().zip(ROW_ROUNDING.iter())) {
        idct!(row; row[0], row[1], row[2], row[3], row[4], row[5], row[6], row[7], tab, rounding);
    }
    for col in 0..8 {
        idct!(col; block[col], block[col + 8], block[col + 8 * 2], block[col + 8 * 3],
              block[col + 8 * 4], block[col + 8 * 5], block[col + 8 * 6], block[col + 8 * 7]);
    }
}

trait Clip8 {
    fn clip8(self) -> u8;
}

impl Clip8 for i16 {
    fn clip8(self) -> u8 { self.clamp(0, 255) as u8 }
}

trait ClipCoef {
    fn clip_coef(self) -> i16;
}

impl ClipCoef for i32 {
    fn clip_coef(self) -> i16 { self.clamp(-2048, 2047) as i16 }
}

pub fn get_dc_scales(quant: u8) -> [u8; 2] {
    match quant {
        1..=4 => [8; 2],
        5..=8 => [quant * 2, (quant + 13) / 2],
        9..=24 => [quant + 8, (quant + 13) / 2],
        _ => [quant * 2 - 16, quant - 6],
    }
}

fn idct_dc(block: &mut [i16; 64]) -> u8 { (block[0] / 8).clip8() }

trait MVOps {
    fn get_chroma_mv(&self) -> MV;
    fn get_chroma_4mv(&self) -> MV;
}

impl MVOps for MV {
    fn get_chroma_mv(&self) -> MV {
        MV{ x: self.x >> 1, y: self.y >> 1 }
    }
    fn get_chroma_4mv(&self) -> MV {
        MV{ x: (self.x >> 3) + H263_CHROMA_ROUND[(self.x & 0xF) as usize],
            y: (self.y >> 3) + H263_CHROMA_ROUND[(self.y & 0xF) as usize] }
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_block(dst: &mut NASimpleVideoFrame<u8>, src: NAVideoBufferRef<u8>, comp: usize,
              dx: usize, dy: usize, mv_x: i16, mv_y: i16, bw: usize, bh: usize,
              preborder: usize, postborder: usize,
              mode: usize, interp: &[BlkInterpFunc], align: u8)
{
    let pre  = if mode != 0 { preborder  as isize } else { 0 };
    let post = if mode != 0 { postborder as isize } else { 0 };
    let (w, h) = src.get_dimensions(comp);
    let sx = (dx as isize) + (mv_x as isize);
    let sy = (dy as isize) + (mv_y as isize);

    if (sx - pre < 0) || (sx + (bw as isize) + post > (w as isize)) ||
       (sy - pre < 0) || (sy + (bh as isize) + post > (h as isize)) {
        let ebuf_stride: usize = 32;
        let mut ebuf: Vec<u8> = vec![0; ebuf_stride * (bh + ((pre + post) as usize))];

        let dstride = dst.stride[comp];
        let doff    = dst.offset[comp];
        let edge = (pre + post) as usize;
        edge_emu(&src, sx - pre, sy - pre, bw + edge, bh + edge,
                 ebuf.as_mut_slice(), ebuf_stride, comp, align);
        (interp[mode])(&mut dst.data[doff + dx + dy * dstride..], dstride,
                       ebuf.as_slice(), ebuf_stride, bw, bh);
    } else {
        let sstride = src.get_stride(comp);
        let soff    = src.get_offset(comp);
        let sdta    = src.get_data();
        let sbuf: &[u8] = sdta.as_slice();
        let dstride = dst.stride[comp];
        let doff    = dst.offset[comp];
        let saddr = soff + ((sx - pre) as usize) + ((sy - pre) as usize) * sstride;
        (interp[mode])(&mut dst.data[doff + dx + dy * dstride..], dstride,
                       &sbuf[saddr..], sstride, bw, bh);
    }
}

pub struct DSP {
    idct:           fn(block: &mut [i16; 64]),
    mc_funcs:       &'static [BlkInterpFunc],
    avg_mc_funcs:   &'static [BlkInterpFunc],
    align:          u8,
}

impl DSP {
    pub fn new() -> Self {
        DSP {
            idct:           h263_idct,
            mc_funcs:       ASP_INTERP_FUNCS_NR,
            avg_mc_funcs:   ASP_INTERP_AVG_FUNCS_NR,
            align:          4,
        }
    }
    pub fn use_xvid_idct(&mut self) {
        self.idct = xvidish_idct;
    }
    pub fn set_rounding(&mut self, rounding: bool) {
        if rounding {
            self.mc_funcs       = ASP_INTERP_FUNCS_NR;
            self.avg_mc_funcs   = ASP_INTERP_AVG_FUNCS_NR;
        } else {
            self.mc_funcs       = H263_INTERP_FUNCS;
            self.avg_mc_funcs   = H263_INTERP_AVG_FUNCS;
        }
    }
    pub fn set_no_align(&mut self) {
        self.align = 0;
    }
    pub fn put_mb(&self, frm: &mut NASimpleVideoFrame<u8>, mb: &mut Macroblock, mb_x: usize, mb_y: usize) {
        let mut cbp = mb.cbp;
        let y_offset = frm.offset[0] + mb_x * 16 + mb_y * 16 * frm.stride[0];
        let y_stride = frm.stride[0];
        if mb.ac_pred { // to be safe
            cbp = 0x3F;
        }
        for (n, y_blk) in mb.coeffs[..4].iter_mut().enumerate() {
            let dst = &mut frm.data[y_offset + (n & 1) * 8 + (n / 2) * 8 * y_stride..];
            if (cbp & 0x20) != 0 {
                (self.idct)(y_blk);
                for (drow, srow) in dst.chunks_mut(y_stride).zip(y_blk.chunks_exact(8)) {
                    for (dst, &src) in drow.iter_mut().zip(srow.iter()) {
                        *dst = src.clip8();
                    }
                }
            } else {
                let dc = idct_dc(y_blk);
                for row in dst.chunks_mut(y_stride).take(8) {
                    for el in row[..8].iter_mut() {
                        *el = dc;
                    }
                }
            }
            cbp <<= 1;
        }

        for (plane, c_blk) in mb.coeffs[4..].iter_mut().enumerate() {
            let c_stride = frm.stride[plane + 1];
            let dst = &mut frm.data[frm.offset[plane + 1] + mb_x * 8 + mb_y * 8 * c_stride..];
            if (cbp & 0x20) != 0 {
                (self.idct)(c_blk);
                for (drow, srow) in dst.chunks_mut(c_stride).zip(c_blk.chunks_exact(8)) {
                    for (dst, &src) in drow.iter_mut().zip(srow.iter()) {
                        *dst = src.clip8();
                    }
                }
            } else {
                let dc = idct_dc(c_blk);
                for row in dst.chunks_mut(c_stride).take(8) {
                    for el in row[..8].iter_mut() {
                        *el = dc;
                    }
                }
            }
            cbp <<= 1;
        }
    }

    pub fn add_coeffs(&self, frm: &mut NASimpleVideoFrame<u8>, mb: &mut Macroblock, mb_x: usize, mb_y: usize) {
        let mut cbp = mb.cbp;
        for (n, block) in mb.coeffs.iter_mut().enumerate() {
            if (cbp & 0x20) != 0 {
                (self.idct)(block);
                let (plane, dx, dy) = if n < 4 {
                        (0, mb_x * 16 + (n & 1) * 8, mb_y * 16 + (n & 2) * 4)
                    } else {
                        (n - 3, mb_x * 8, mb_y * 8)
                    };
                let stride = frm.stride[plane];
                let off = frm.offset[plane] + dx + dy * stride;
                for (drow, brow) in frm.data[off..].chunks_mut(stride).zip(block.chunks_exact(8)) {
                    for (coef, &diff) in drow.iter_mut().zip(brow.iter()) {
                        *coef = (i16::from(*coef) + diff).clip8();
                    }
                }
            }
            cbp <<= 1;
        }
    }

    pub fn mb_mv(&self, frm: &mut NASimpleVideoFrame<u8>, mb_x: usize, mb_y: usize, pframe: NAVideoBufferRef<u8>, mv: MV) {
        let mode = ((mv.x & 1) + (mv.y & 1) * 2) as usize;
        copy_block(frm, pframe.clone(), 0, mb_x * 16, mb_y * 16, mv.x >> 1, mv.y >> 1, 16, 16, 0, 1, mode, self.mc_funcs, self.align);
        let cmv = mv.get_chroma_mv();
        let cmode = (if (mv.x & 3) != 0 { 1 } else { 0 }) + (if (mv.y & 3) != 0 { 2 } else { 0 });
        copy_block(frm, pframe.clone(), 1, mb_x * 8, mb_y * 8, cmv.x >> 1, cmv.y >> 1, 8, 8, 0, 1, cmode, self.mc_funcs, self.align);
        copy_block(frm, pframe,         2, mb_x * 8, mb_y * 8, cmv.x >> 1, cmv.y >> 1, 8, 8, 0, 1, cmode, self.mc_funcs, self.align);
    }

    pub fn avg_mb_mv(&self, frm: &mut NASimpleVideoFrame<u8>, mb_x: usize, mb_y: usize, pframe: NAVideoBufferRef<u8>, mv: MV) {
        let mode = ((mv.x & 1) + (mv.y & 1) * 2) as usize;
        copy_block(frm, pframe.clone(), 0, mb_x * 16, mb_y * 16, mv.x >> 1, mv.y >> 1, 16, 16, 0, 1, mode, self.avg_mc_funcs, self.align);
        let cmv = mv.get_chroma_mv();
        let cmode = (if (mv.x & 3) != 0 { 1 } else { 0 }) + (if (mv.y & 3) != 0 { 2 } else { 0 });
        copy_block(frm, pframe.clone(), 1, mb_x * 8, mb_y * 8, cmv.x >> 1, mv.y >> 1, 8, 8, 0, 1, cmode, self.avg_mc_funcs, self.align);
        copy_block(frm, pframe,         2, mb_x * 8, mb_y * 8, cmv.x >> 1, mv.y >> 1, 8, 8, 0, 1, cmode, self.avg_mc_funcs, self.align);
    }

    pub fn mb_4mv(&self, frm: &mut NASimpleVideoFrame<u8>, mb_x: usize, mb_y: usize, pframe: NAVideoBufferRef<u8>, mvs: &[MV; 4]) {
        for (n, &mv) in mvs.iter().enumerate() {
            let mode = ((mv.x & 1) + (mv.y & 1) * 2) as usize;
            copy_block(frm, pframe.clone(), 0, mb_x * 16 + (n & 1) * 8, mb_y * 16 + (n / 2) * 8, mv.x >> 1, mv.y >> 1, 8, 8, 0, 1, mode, self.mc_funcs, self.align);
        }
        let sum_mv = mvs[0] + mvs[1] + mvs[2] + mvs[3];
        let cmv = sum_mv.get_chroma_4mv();
        let cmode = ((cmv.x & 1) + (cmv.y & 1) * 2) as usize;
        copy_block(frm, pframe.clone(), 1, mb_x * 8, mb_y * 8, cmv.x >> 1, cmv.y >> 1, 8, 8, 0, 1, cmode, self.mc_funcs, self.align);
        copy_block(frm, pframe,         2, mb_x * 8, mb_y * 8, cmv.x >> 1, cmv.y >> 1, 8, 8, 0, 1, cmode, self.mc_funcs, self.align);
    }

    pub fn avg_mb_4mv(&self, frm: &mut NASimpleVideoFrame<u8>, mb_x: usize, mb_y: usize, pframe: NAVideoBufferRef<u8>, mvs: &[MV; 4]) {
        for (n, &mv) in mvs.iter().enumerate() {
            let mode = ((mv.x & 1) + (mv.y & 1) * 2) as usize;
            copy_block(frm, pframe.clone(), 0, mb_x * 16 + (n & 1) * 8, mb_y * 16 + (n / 2) * 8, mv.x >> 1, mv.y >> 1, 8, 8, 0, 1, mode, self.avg_mc_funcs, self.align);
        }
        let sum_mv = mvs[0] + mvs[1] + mvs[2] + mvs[3];
        let cmv = sum_mv.get_chroma_4mv();
        let cmode = ((cmv.x & 1) + (cmv.y & 1) * 2) as usize;
        copy_block(frm, pframe.clone(), 1, mb_x * 8, mb_y * 8, cmv.x, cmv.y, 8, 8, 0, 1, cmode, self.avg_mc_funcs, self.align);
        copy_block(frm, pframe,         2, mb_x * 8, mb_y * 8, cmv.x, cmv.y, 8, 8, 0, 1, cmode, self.avg_mc_funcs, self.align);
    }
}

#[allow(clippy::needless_late_init)]
pub fn dequant_intra_block(block: &mut [i16; 64], ac_scale: u8, qinfo: &QuantInfo) {
    if qinfo.quant_type {
        for (coef, &weight) in block.iter_mut().zip(qinfo.intra_mat.iter()).skip(1) {
            *coef = (i32::from(*coef * 2) * i32::from(ac_scale) * i32::from(weight) / 16).clip_coef();
        }
    } else {
        let scale = i32::from(ac_scale);
        let bbias = if (ac_scale & 1) == 0 { 1 } else { 0 };
        for coef in block.iter_mut().skip(1) {
            if *coef != 0 {
                let mut c0 = i32::from(*coef * 2);
                let bias;
                if c0 > 0 {
                    c0 += 1;
                    bias = -bbias;
                } else {
                    c0 -= 1;
                    bias = bbias;
                }
                *coef = (c0 * scale + bias).clip_coef();
            }
        }
    }
}

#[allow(clippy::needless_late_init)]
pub fn dequant_inter(mb: &mut Macroblock, qinfo: &QuantInfo) {
    let ac_scale = mb.quant;
    let mut cbp = mb.cbp;
    if qinfo.quant_type {
        for block in mb.coeffs.iter_mut() {
            if (cbp & 0x20) != 0 {
                for (coef, &weight) in block.iter_mut().zip(qinfo.inter_mat.iter()) {
                    if *coef != 0 {
                        let mut c0 = i32::from(*coef * 2);
                        if c0 > 0 { c0 += 1; }
                        else      { c0 -= 1; }
                        *coef = (c0 * i32::from(ac_scale) * i32::from(weight) / 16).clip_coef();
                    }
                }
            }
            cbp <<= 1;
        }
    } else {
        let scale = i32::from(ac_scale);
        let bbias = if (ac_scale & 1) == 0 { 1 } else { 0 };
        for block in mb.coeffs.iter_mut() {
            if (cbp & 0x20) != 0 {
                for coef in block.iter_mut() {
                    if *coef != 0 {
                        let mut c0 = i32::from(*coef * 2);
                        let bias;
                        if c0 > 0 {
                            c0 += 1;
                            bias = -bbias;
                        } else {
                            c0 -= 1;
                            bias = bbias;
                        }
                        *coef = (c0 * scale + bias).clip_coef();
                    }
                }
            }
            cbp <<= 1;
        }
    }
}

fn asp_interp00nr(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    for (drow, srow) in dst.chunks_mut(dstride).zip(src.chunks(sstride)).take(bh) {
        drow[..bw].copy_from_slice(&srow[..bw]);
    }
}

fn asp_interp01nr(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    for (drow, srow) in dst.chunks_mut(dstride).zip(src.chunks(sstride)).take(bh) {
        for (dst, src) in drow.iter_mut().zip(srow.windows(2)).take(bw) {
            *dst = ((u16::from(src[0]) + u16::from(src[1])) >> 1) as u8;
        }
    }
}

fn asp_interp10nr(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    for (drow, (srow0, srow1)) in dst.chunks_mut(dstride)
            .zip(src.chunks(sstride).zip(src[sstride..].chunks(sstride))).take(bh) {
        for (dst, (&src0, &src1)) in drow.iter_mut().zip(srow0.iter().zip(srow1.iter())).take(bw) {
            *dst = ((u16::from(src0) + u16::from(src1)) >> 1) as u8;
        }
    }
}

fn asp_interp11nr(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    for (drow, (srow0, srow1)) in dst.chunks_mut(dstride)
            .zip(src.chunks(sstride).zip(src[sstride..].chunks(sstride))).take(bh) {
        for (dst, (src0, src1)) in drow.iter_mut()
                .zip(srow0.windows(2).zip(srow1.windows(2))).take(bw) {
            *dst = ((u16::from(src0[0]) + u16::from(src0[1]) + u16::from(src1[0]) + u16::from(src1[1]) + 1) >> 2) as u8;
        }
    }
}

pub const ASP_INTERP_FUNCS_NR: &[BlkInterpFunc] = &[
        asp_interp00nr, asp_interp01nr, asp_interp10nr, asp_interp11nr ];

fn asp_interp00nr_avg(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    for (dline, sline) in dst.chunks_mut(dstride).zip(src.chunks(sstride)).take(bh) {
        for (dst, &src) in dline.iter_mut().zip(sline.iter()).take(bw) {
            *dst = ((u16::from(*dst) + u16::from(src) + 1) >> 1) as u8;
        }
    }
}

fn asp_interp01nr_avg(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    let mut didx = 0;
    let mut sidx = 0;
    for _ in 0..bh {
        for x in 0..bw {
            let a = dst[didx + x] as u16;
            let b = ((src[sidx + x] as u16) + (src[sidx + x + 1] as u16)) >> 1;
            dst[didx + x] = ((a + b + 1) >> 1) as u8;
        }
        didx += dstride;
        sidx += sstride;
    }
}

fn asp_interp10nr_avg(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    let mut didx = 0;
    let mut sidx = 0;
    for _ in 0..bh {
        for x in 0..bw {
            let a = dst[didx + x] as u16;
            let b = ((src[sidx + x] as u16) + (src[sidx + x + sstride] as u16)) >> 1;
            dst[didx + x] = ((a + b + 1) >> 1) as u8;
        }
        didx += dstride;
        sidx += sstride;
    }
}

fn asp_interp11nr_avg(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, bw: usize, bh: usize)
{
    let mut didx = 0;
    let mut sidx = 0;
    for _ in 0..bh {
        for x in 0..bw {
            let a = dst[didx + x] as u16;
            let b = ((src[sidx + x] as u16) +
                     (src[sidx + x + 1] as u16) +
                     (src[sidx + x + sstride] as u16) +
                     (src[sidx + x + sstride + 1] as u16) + 1) >> 2;
            dst[didx + x] = ((a + b + 1) >> 1) as u8;
        }
        didx += dstride;
        sidx += sstride;
    }
}

pub const ASP_INTERP_AVG_FUNCS_NR: &[BlkInterpFunc] = &[
        asp_interp00nr_avg, asp_interp01nr_avg, asp_interp10nr_avg, asp_interp11nr_avg ];
