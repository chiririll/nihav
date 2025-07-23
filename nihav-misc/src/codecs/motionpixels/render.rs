use nihav_core::codecs::{DecoderError,DecoderResult};

use super::*;

#[derive(Clone,Copy,Debug,Default)]
pub struct DeltaMod(bool);

impl DeltaMod {
    fn assign(&mut self, dval: i8) -> i8 {
        let ret = if self.0 { dval * 2 } else { dval };
        self.0 = dval.is_escape();
        ret
    }
}

#[derive(Clone,Copy,Debug,PartialEq)]
#[allow(non_camel_case_types)]
#[allow(clippy::upper_case_acronyms)]
pub enum RenderMode {
    Unknown,
    // MVI0-specific formats
    RGB_Old,
    YUV2x1_Old,
    YUV2x2_Old,
    YUV4x2_Old,
    YUV4x4_Old,
    // MVI 1 and 2 formats
    RGB,
    YUV2x1,
    YUV2x2,
    YUV4x2,
    YUV4x4,
    // MVI 2 formats with smooth delta mode
    RGBSmooth,
    YUV2x1Smooth,
    YUV2x2Smooth,
    YUV4x2Smooth,
    YUV4x4Smooth,
}

#[derive(Clone,Copy)]
pub struct ReconFuncs {
    pub col_pred:   fn(&mut MPFrame) -> DecoderResult<()>,
    pub line_pred:  fn(&mut MPFrame) -> DecoderResult<()>,
}

impl Default for ReconFuncs {
    fn default() -> Self { ReconFuncs { col_pred: col_pred_dummy, line_pred: line_pred_dummy } }
}

impl std::convert::From<RenderMode> for ReconFuncs {
    fn from(mode: RenderMode) -> ReconFuncs {
        match mode {
            RenderMode::RGB_Old =>
                ReconFuncs { col_pred: col_pred_rgb, line_pred: line_pred_rgb },
            RenderMode::YUV2x1_Old =>
                ReconFuncs { col_pred: col_pred_full, line_pred: line_pred_2x1 },
            RenderMode::YUV2x2_Old =>
                ReconFuncs { col_pred: col_pred_even, line_pred: line_pred_2x2 },
            RenderMode::YUV4x2_Old =>
                ReconFuncs { col_pred: col_pred_even, line_pred: line_pred_4x2 },
            RenderMode::YUV4x4_Old =>
                ReconFuncs { col_pred: col_pred_4, line_pred: line_pred_4x4 },

            RenderMode::RGB =>
                ReconFuncs { col_pred: col_pred_rgb, line_pred: line_pred_rgb_mvi },
            RenderMode::YUV2x1 =>
                ReconFuncs { col_pred: col_pred_full, line_pred: line_pred_2x1_mvi },
            RenderMode::YUV2x2 =>
                ReconFuncs { col_pred: col_pred_even_mvi, line_pred: line_pred_2x2_mvi },
            RenderMode::YUV4x2 =>
                ReconFuncs { col_pred: col_pred_even_mvi, line_pred: line_pred_4x2_mvi },
            RenderMode::YUV4x4 =>
                ReconFuncs { col_pred: col_pred_4_mvi, line_pred: line_pred_4x4_mvi },

            RenderMode::YUV2x1Smooth =>
                ReconFuncs { col_pred: col_pred_full_mvi2, line_pred: line_pred_2x1_mvi2 },
            RenderMode::YUV2x2Smooth =>
                ReconFuncs { col_pred: col_pred_even_mvi2, line_pred: line_pred_2x2_mvi2 },
            RenderMode::YUV4x2Smooth =>
                ReconFuncs { col_pred: col_pred_even_mvi2, line_pred: line_pred_4x2_mvi2 },
            RenderMode::YUV4x4Smooth =>
                ReconFuncs { col_pred: col_pred_4_mvi2, line_pred: line_pred_4x4_mvi2 },

            _ => ReconFuncs::default(),
        }
    }
}

pub const MVI0_MODES: [RenderMode; 6] = [
    RenderMode::RGB_Old,
    RenderMode::YUV2x1_Old,
    RenderMode::YUV2x2_Old,
    RenderMode::YUV4x2_Old,
    RenderMode::YUV4x4_Old,
    RenderMode::Unknown // YUV440 probably
];

pub const MVI_MODES: [RenderMode; 5] = [
    RenderMode::RGB,
    RenderMode::YUV2x1,
    RenderMode::YUV2x2,
    RenderMode::YUV4x2,
    RenderMode::YUV4x4,
];

pub const MVI2_MODES: [RenderMode; 5] = [
    RenderMode::RGBSmooth,
    RenderMode::YUV2x1Smooth,
    RenderMode::YUV2x2Smooth,
    RenderMode::YUV4x2Smooth,
    RenderMode::YUV4x4Smooth,
];

fn col_pred_dummy(_mp: &mut MPFrame) -> DecoderResult<()> {
    Err(DecoderError::NotImplemented)
}
fn line_pred_dummy(_mp: &mut MPFrame) -> DecoderResult<()> {
    Err(DecoderError::NotImplemented)
}

// common prediction functions

fn col_pred_rgb(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    mpf.dmod = [DeltaMod::default(); 3];
    for (dst, (mline, fline)) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width))) {
        if mline[0] != 0 {
            pix = YuvPixel::unpack(fline[0]);
            mpf.dmod = [DeltaMod::default(); 3];
        } else {
            let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
            pix.y = (pix.y + delta).clip_y();
            let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
            pix.v = (pix.v + delta).clip_y();
            let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
            pix.u = (pix.u + delta).clip_y();

            fline[0] = pix.pack();
        }
        *dst = pix;
    }
    Ok(())
}
fn col_pred_full(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    mpf.dmod = [DeltaMod::default(); 3];
    for (dst, (mline, fline)) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width))) {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
            mpf.dmod = [DeltaMod::default(); 3];
        } else {
            let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
            pix.y = (pix.y + delta).clip_y();
            let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
            pix.v = (pix.v + delta).clip_c();
            let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
            pix.u = (pix.u + delta).clip_c();

            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}
fn col_pred_even(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    mpf.dmod = [DeltaMod::default(); 3];
    for (y, (dst, (mline, fline))) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width)))
            .enumerate() {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
            mpf.dmod = [DeltaMod::default(); 3];
        } else {
            let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
            pix.y = (pix.y + delta).clip_y();

            if (y & 1) == 0 {
                let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                pix.v = (pix.v + delta).clip_c();
                let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                pix.u = (pix.u + delta).clip_c();
            }
            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}
fn col_pred_4(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    mpf.dmod = [DeltaMod::default(); 3];
    for (y, (dst, (mline, fline))) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width)))
            .enumerate() {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
            mpf.dmod = [DeltaMod::default(); 3];
        } else {
            let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
            pix.y = (pix.y + delta).clip_y();

            if (y & 3) == 0 {
                let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                pix.v = (pix.v + delta).clip_c();
                let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                pix.u = (pix.u + delta).clip_c();
            }
            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}

fn line_pred_rgb(mpf: &mut MPFrame) -> DecoderResult<()> {
    for (&cpred, (fline, mline)) in mpf.col_pred.iter()
            .zip(mpf.frame.chunks_exact_mut(mpf.width)
                .zip(mpf.map.chunks_exact(mpf.width))) {
        let mut pix = cpred;
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                x += mp;
                mpf.dmod = [DeltaMod::default(); 3];
                if x < mpf.width {
                    pix = YuvPixel::unpack(fline[x - 1]);
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                pix.v = (pix.v + delta).clip_y();
                let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                pix.u = (pix.u + delta).clip_y();
                fline[x] = pix.pack();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_2x1(mpf: &mut MPFrame) -> DecoderResult<()> {
    for (&cpred, (fline, mline)) in mpf.col_pred.iter()
            .zip(mpf.frame.chunks_exact_mut(mpf.width)
                .zip(mpf.map.chunks_exact(mpf.width))) {
        let mut pix = cpred;
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                x += mp;
                mpf.dmod = [DeltaMod::default(); 3];
                if x < mpf.width {
                    pix = fline[x - 1].into();
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                if (x & 1) == 0 {
                    let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                    pix.v = (pix.v + delta).clip_c();
                    let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                    pix.u = (pix.u + delta).clip_c();
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_2x2(mpf: &mut MPFrame) -> DecoderResult<()> {
    for (y, (&cpred, (fline, mline))) in mpf.col_pred.iter()
            .zip(mpf.frame.chunks_exact_mut(mpf.width)
                .zip(mpf.map.chunks_exact(mpf.width)))
            .enumerate() {
        let mut pix = cpred;
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if (y & 1) == 0 {
                    for x1 in (((x + 1) & !1)..(x + mp)).step_by(2) {
                        mpf.chroma[x1 / 2] = fline[x1].into();
                    }
                }
                x += mp;
                mpf.dmod = [DeltaMod::default(); 3];
                if x < mpf.width {
                    pix = fline[x - 1].into();
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                if (x & 1) == 0 {
                    if (y & 1) == 0 {
                        let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 2] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 2].u;
                        pix.v = mpf.chroma[x / 2].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_4x2(mpf: &mut MPFrame) -> DecoderResult<()> {
    for (y, (&cpred, (fline, mline))) in mpf.col_pred.iter()
            .zip(mpf.frame.chunks_exact_mut(mpf.width)
                .zip(mpf.map.chunks_exact(mpf.width)))
            .enumerate() {
        let mut pix = cpred;
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if (y & 1) == 0 {
                    for x1 in (((x + 3) & !3)..(x + mp)).step_by(4) {
                        mpf.chroma[x1 / 4] = fline[x1].into();
                    }
                }
                x += mp;
                mpf.dmod = [DeltaMod::default(); 3];
                if x < mpf.width {
                    pix = fline[x - 1].into();
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                if (x & 3) == 0 {
                    if (y & 1) == 0 {
                        let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 4] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 4].u;
                        pix.v = mpf.chroma[x / 4].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_4x4(mpf: &mut MPFrame) -> DecoderResult<()> {
    for (y, (&cpred, (fline, mline))) in mpf.col_pred.iter()
            .zip(mpf.frame.chunks_exact_mut(mpf.width)
                .zip(mpf.map.chunks_exact(mpf.width)))
            .enumerate() {
        let mut pix = cpred;
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if (y & 3) == 0 {
                    for x1 in (((x + 3) & !3)..(x + mp)).step_by(4) {
                        mpf.chroma[x1 / 4] = fline[x1].into();
                    }
                }
                x += mp;
                mpf.dmod = [DeltaMod::default(); 3];
                if x < mpf.width {
                    pix = fline[x - 1].into();
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                if (x & 3) == 0 {
                    if (y & 3) == 0 {
                        let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 4] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 4].u;
                        pix.v = mpf.chroma[x / 4].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}

// MVI1 and MVI2 non-extended prediction functions

fn col_pred_even_mvi(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    mpf.dmod = [DeltaMod::default(); 3];
    for (y, (dst, (mline, fline))) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width)))
            .enumerate() {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
            mpf.dmod = [DeltaMod::default(); 3];
        } else {
            let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
            pix.y = (pix.y + delta).clip_y();

            if (y & 1) == 1 {
                let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                pix.v = (pix.v + delta).clip_c();
                let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                pix.u = (pix.u + delta).clip_c();
            }
            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}
fn col_pred_4_mvi(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    mpf.dmod = [DeltaMod::default(); 3];
    for (y, (dst, (mline, fline))) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width)))
            .enumerate() {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
            mpf.dmod = [DeltaMod::default(); 3];
        } else {
            let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
            pix.y = (pix.y + delta).clip_y();

            if ((y + 1) & 3) == 0 {
                let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                pix.v = (pix.v + delta).clip_c();
                let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                pix.u = (pix.u + delta).clip_c();
            }
            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}

fn line_pred_rgb_mvi(mpf: &mut MPFrame) -> DecoderResult<()> {
    mpf.deltas.switch_line();
    line_pred_rgb(mpf)
}
fn line_pred_2x1_mvi(mpf: &mut MPFrame) -> DecoderResult<()> {
    mpf.deltas.switch_line();
    line_pred_2x1(mpf)
}
fn line_pred_2x2_mvi(mpf: &mut MPFrame) -> DecoderResult<()> {
    for y in 0..mpf.height {
        let fline = &mut mpf.frame[(y ^ 1) * mpf.width..][..mpf.width];
        let mline = &mpf.map[(y ^ 1) * mpf.width..][..mpf.width];

        let mut pix = mpf.col_pred[y ^ 1];
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if (y & 1) == 0 {
                    for x1 in (((x + 1) & !1)..(x + mp)).step_by(2) {
                        mpf.chroma[x1 / 2] = fline[x1].into();
                    }
                }
                x += mp;
                mpf.dmod = [DeltaMod::default(); 3];
                if x < mpf.width {
                    pix = fline[x - 1].into();
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                if (x & 1) == 0 {
                    if (y & 1) == 0 {
                        let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 2] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 2].u;
                        pix.v = mpf.chroma[x / 2].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_4x2_mvi(mpf: &mut MPFrame) -> DecoderResult<()> {
    for y in 0..mpf.height {
        let fline = &mut mpf.frame[(y ^ 1) * mpf.width..][..mpf.width];
        let mline = &mpf.map[(y ^ 1) * mpf.width..][..mpf.width];

        let mut pix = mpf.col_pred[y ^ 1];
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if (y & 1) == 0 {
                    for x1 in (((x + 3) & !3)..(x + mp)).step_by(4) {
                        mpf.chroma[x1 / 4] = fline[x1].into();
                    }
                }
                x += mp;
                mpf.dmod = [DeltaMod::default(); 3];
                if x < mpf.width {
                    pix = fline[x - 1].into();
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                if (x & 3) == 0 {
                    if (y & 1) == 0 {
                        let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 4] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 4].u;
                        pix.v = mpf.chroma[x / 4].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_4x4_mvi(mpf: &mut MPFrame) -> DecoderResult<()> {
    for y in 0..mpf.height {
        let fline = &mut mpf.frame[(y ^ 3) * mpf.width..][..mpf.width];
        let mline = &mpf.map[(y ^ 3) * mpf.width..][..mpf.width];
        let eline = &mpf.lrmap[(y ^ 3) * mpf.width..][..mpf.width];

        let mut pix = mpf.col_pred[y ^ 3];
        let mut x = if mline[0] == 0 {
                mpf.dmod = [DeltaMod::default(); 3];
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if !eline[x] {
                    if (y & 3) == 0 {
                        for x1 in (((x + 3) & !3)..(x + mp)).step_by(4) {
                            mpf.chroma[x1 / 4] = fline[x1].into();
                        }
                    }
                    x += mp;
                    mpf.dmod = [DeltaMod::default(); 3];
                    if x < mpf.width {
                        pix = fline[x - 1].into();
                    }
                } else {
                    let mut left = mp;
                    // adjacent lowres areas are treated as one
                    let mut end_x = x + mp;
                    while end_x < mpf.width && mline[end_x] != 0 && eline[end_x] {
                        left  += usize::from(mline[end_x]);
                        end_x += usize::from(mline[end_x]);
                    }

                    let last_interp = end_x < mpf.width && mline[end_x] != 0;
                    let cand_pos = ((y & 1) == 0 && (end_x & 1) == 0) || ((y & 1) == 1 && (end_x & 1) == 1);
                    if !last_interp {
                        // last low-res sample can be merged with the first normal delta sample
                        if end_x < mpf.width && cand_pos && mline[end_x] == 0 {
                            left  += 1;
                            end_x += 1;
                        }
                    } else if cand_pos {
                        left -= 1;
                    }

                    match y & 3 {
                        0 => lowres_pred4(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mut mpf.chroma, &mut mpf.dmod)?,
                        2 => lowres_pred4_even(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mpf.chroma, &mut mpf.dmod)?,
                        _ => lowres_pred4_odd(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mpf.chroma, &mut mpf.dmod)?,
                    }
                    if last_interp && x < end_x {
                        let ydelta = (YuvPixel::from(fline[x + 1]).y - pix.y) >> 1;
                        let mut lpix = pix;
                        lpix.y = (lpix.y + ydelta).clip_y();
                        fline[x] = lpix.into();
                        x += 1;
                    }
                }
            } else {
                let delta = mpf.dmod[0].assign(mpf.deltas.next()?);
                pix.y = (pix.y + delta).clip_y();
                if (x & 3) == 0 {
                    if (y & 3) == 0 {
                        let delta = mpf.dmod[1].assign(mpf.deltas.next()?);
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.dmod[2].assign(mpf.deltas.next()?);
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 4] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 4].u;
                        pix.v = mpf.chroma[x / 4].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}

fn lowres_pred4(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &mut [YuvPixel], dmod: &mut [DeltaMod; 3]) -> DecoderResult<()> {
    let mut last_delta = None;
    for _ in 0..len {
        if (*x & 1) == 0 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else {
                    dmod[0].assign(deltas.next()?)
                };
            pix.y = (pix.y + ydelta).clip_y();
            if (*x & 3) == 0 {
                let vdelta = dmod[1].assign(deltas.next()?);
                pix.v = (pix.v + vdelta).clip_c();
                let udelta = dmod[2].assign(deltas.next()?);
                pix.u = (pix.u + udelta).clip_c();
                chroma[*x / 4] = *pix;
            }
            fline[*x] = (*pix).into();
        } else {
            let ydelta = dmod[0].assign(deltas.next()?);
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}
fn lowres_pred4_even(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &[YuvPixel], dmod: &mut [DeltaMod; 3]) -> DecoderResult<()> {
    let mut last_delta = None;
    for _ in 0..len {
        if (*x & 1) == 0 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else {
                    dmod[0].assign(deltas.next()?)
                };
            pix.y = (pix.y + ydelta).clip_y();
            if (*x & 3) == 0 {
                pix.u = chroma[*x / 4].u;
                pix.v = chroma[*x / 4].v;
            }
            fline[*x] = (*pix).into();
        } else {
            let ydelta = dmod[0].assign(deltas.next()?);
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}
fn lowres_pred4_odd(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &[YuvPixel], dmod: &mut [DeltaMod; 3]) -> DecoderResult<()> {
    let mut last_delta = None;
    for _ in 0..len {
        if (*x & 1) == 1 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else {
                    dmod[0].assign(deltas.next()?)
                };
            pix.y = (pix.y + ydelta).clip_y();
            if (*x & 3) == 1 {
                pix.u = chroma[*x / 4].u;
                pix.v = chroma[*x / 4].v;
            }
            fline[*x] = (*pix).into();
        } else {
            let ydelta = dmod[0].assign(deltas.next()?);
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            if (*x & 3) == 0 {
                lpix.u = chroma[*x / 4].u;
                lpix.v = chroma[*x / 4].v;
            }
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}

// MVI2 extended prediction functions

fn col_pred_full_mvi2(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    for (dst, (mline, fline)) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width))) {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
        } else {
            let delta = mpf.deltas.next_smooth()?;
            pix.y = (pix.y + delta).clip_y();
            let delta = mpf.deltas.next_smooth()?;
            pix.v = (pix.v + delta).clip_c();
            let delta = mpf.deltas.next_smooth()?;
            pix.u = (pix.u + delta).clip_c();

            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}
fn col_pred_even_mvi2(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    for (y, (dst, (mline, fline))) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width)))
            .enumerate() {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
        } else {
            let delta = mpf.deltas.next_smooth()?;
            pix.y = (pix.y + delta).clip_y();

            if (y & 1) == 1 {
                let delta = mpf.deltas.next_smooth()?;
                pix.v = (pix.v + delta).clip_c();
                let delta = mpf.deltas.next_smooth()?;
                pix.u = (pix.u + delta).clip_c();
            }
            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}
fn col_pred_4_mvi2(mpf: &mut MPFrame) -> DecoderResult<()> {
    let mut pix = YuvPixel::default();
    for (y, (dst, (mline, fline))) in mpf.col_pred.iter_mut()
            .zip(mpf.map.chunks_exact(mpf.width)
                    .zip(mpf.frame.chunks_exact_mut(mpf.width)))
            .enumerate() {
        if mline[0] != 0 {
            pix = YuvPixel::from(fline[0]);
        } else {
            let delta = mpf.deltas.next_smooth()?;
            pix.y = (pix.y + delta).clip_y();

            if ((y + 1) & 3) == 0 {
                let delta = mpf.deltas.next_smooth()?;
                pix.v = (pix.v + delta).clip_c();
                let delta = mpf.deltas.next_smooth()?;
                pix.u = (pix.u + delta).clip_c();
            }
            fline[0] = pix.into();
        }
        *dst = pix;
    }
    Ok(())
}

fn line_pred_2x1_mvi2(mpf: &mut MPFrame) -> DecoderResult<()> {
    for y in 0..mpf.height {
        let fline = &mut mpf.frame[(y ^ 1) * mpf.width..][..mpf.width];
        let mline = &mpf.map[(y ^ 1) * mpf.width..][..mpf.width];
        let eline = &mpf.lrmap[(y ^ 1) * mpf.width..][..mpf.width];

        let mut pix = mpf.col_pred[y ^ 1];
        let mut x = if mline[0] == 0 { 1 } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if !eline[x] {
                    x += mp;
                    if x < mpf.width {
                        pix = fline[x - 1].into();
                    }
                } else {
                    let mut left = mp;
                    // adjacent lowres areas are treated as one
                    let mut end_x = x + mp;
                    while end_x < mpf.width && mline[end_x] != 0 && eline[end_x] {
                        left  += usize::from(mline[end_x]);
                        end_x += usize::from(mline[end_x]);
                    }

                    let last_interp = end_x < mpf.width && mline[end_x] != 0;
                    let cand_pos = (end_x & 1) == 0;
                    if !last_interp {
                        // last low-res sample can be merged with the first normal delta sample
                        if end_x < mpf.width && cand_pos && mline[end_x] == 0 {
                            left  += 1;
                            end_x += 1;
                        }
                    } else if cand_pos {
                        left -= 1;
                    }

                    lowres_pred2_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mut mpf.chroma)?;
                    if last_interp && x < end_x {
                        let ydelta = (YuvPixel::from(fline[x + 1]).y - pix.y) >> 1;
                        let mut lpix = pix;
                        lpix.y = (lpix.y + ydelta).clip_y();
                        fline[x] = lpix.into();
                        x += 1;
                    }
                }
            } else {
                let delta = mpf.deltas.next_smooth()?;
                pix.y = (pix.y + delta).clip_y();
                if (x & 1) == 0 {
                    let delta = mpf.deltas.next_smooth()?;
                    pix.v = (pix.v + delta).clip_c();
                    let delta = mpf.deltas.next_smooth()?;
                    pix.u = (pix.u + delta).clip_c();
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_2x2_mvi2(mpf: &mut MPFrame) -> DecoderResult<()> {
    for y in 0..mpf.height {
        let fline = &mut mpf.frame[(y ^ 1) * mpf.width..][..mpf.width];
        let mline = &mpf.map[(y ^ 1) * mpf.width..][..mpf.width];
        let eline = &mpf.lrmap[(y ^ 1) * mpf.width..][..mpf.width];

        let mut pix = mpf.col_pred[y ^ 1];
        let mut x = if mline[0] == 0 { 1 } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if !eline[x] {
                    if (y & 1) == 0 {
                        for x1 in (((x + 1) & !1)..(x + mp)).step_by(2) {
                            mpf.chroma[x1 / 2] = fline[x1].into();
                        }
                    }
                    x += mp;
                    if x < mpf.width {
                        pix = fline[x - 1].into();
                    }
                } else {
                    let mut left = mp;
                    // adjacent lowres areas are treated as one
                    let mut end_x = x + mp;
                    while end_x < mpf.width && mline[end_x] != 0 && eline[end_x] {
                        left  += usize::from(mline[end_x]);
                        end_x += usize::from(mline[end_x]);
                    }

                    let last_interp = end_x < mpf.width && mline[end_x] != 0;
                    let cand_pos = ((y & 1) == 0 && (end_x & 1) == 0) || ((y & 1) == 1 && (end_x & 1) == 1);
                    if !last_interp {
                        // last low-res sample can be merged with the first normal delta sample
                        if end_x < mpf.width && cand_pos && mline[end_x] == 0 {
                            left  += 1;
                            end_x += 1;
                        }
                    } else if cand_pos {
                        left -= 1;
                    }

                    if (y & 1) == 0 {
                        lowres_pred2_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mut mpf.chroma)?;
                    } else {
                        lowres_pred2_odd_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mpf.chroma)?;
                    }
                    if last_interp && x < end_x {
                        let ydelta = (YuvPixel::from(fline[x + 1]).y - pix.y) >> 1;
                        let mut lpix = pix;
                        lpix.y = (lpix.y + ydelta).clip_y();
                        fline[x] = lpix.into();
                        x += 1;
                    }
                }
            } else {
                let delta = mpf.deltas.next_smooth()?;
                pix.y = (pix.y + delta).clip_y();
                if (x & 1) == 0 {
                    if (y & 1) == 0 {
                        let delta = mpf.deltas.next_smooth()?;
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.deltas.next_smooth()?;
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 2] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 2].u;
                        pix.v = mpf.chroma[x / 2].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_4x2_mvi2(mpf: &mut MPFrame) -> DecoderResult<()> {
    for y in 0..mpf.height {
        let fline = &mut mpf.frame[(y ^ 1) * mpf.width..][..mpf.width];
        let mline = &mpf.map[(y ^ 1) * mpf.width..][..mpf.width];
        let eline = &mpf.lrmap[(y ^ 1) * mpf.width..][..mpf.width];

        let mut pix = mpf.col_pred[y ^ 1];
        let mut x = if mline[0] == 0 {
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if !eline[x] {
                    if (y & 1) == 0 {
                        for x1 in (((x + 3) & !3)..(x + mp)).step_by(4) {
                            mpf.chroma[x1 / 4] = fline[x1].into();
                        }
                    }
                    x += mp;
                    if x < mpf.width {
                        pix = fline[x - 1].into();
                    }
                } else {
                    let mut left = mp;
                    // adjacent lowres areas are treated as one
                    let mut end_x = x + mp;
                    while end_x < mpf.width && mline[end_x] != 0 && eline[end_x] {
                        left  += usize::from(mline[end_x]);
                        end_x += usize::from(mline[end_x]);
                    }

                    let last_interp = end_x < mpf.width && mline[end_x] != 0;
                    let cand_pos = ((y & 1) == 0 && (end_x & 1) == 0) || ((y & 1) == 1 && (end_x & 1) == 1);
                    if !last_interp {
                        // last low-res sample can be merged with the first normal delta sample
                        if end_x < mpf.width && cand_pos && mline[end_x] == 0 {
                            left  += 1;
                            end_x += 1;
                        }
                    } else if cand_pos {
                        left -= 1;
                    }

                    if (y & 1) == 0 {
                        lowres_pred4_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mut mpf.chroma)?;
                    } else {
                        lowres_pred4_odd_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mpf.chroma)?;
                    }
                    if last_interp && x < end_x {
                        let ydelta = (YuvPixel::from(fline[x + 1]).y - pix.y) >> 1;
                        let mut lpix = pix;
                        lpix.y = (lpix.y + ydelta).clip_y();
                        fline[x] = lpix.into();
                        x += 1;
                    }
                }
            } else {
                let delta = mpf.deltas.next_smooth()?;
                pix.y = (pix.y + delta).clip_y();
                if (x & 3) == 0 {
                    if (y & 1) == 0 {
                        let delta = mpf.deltas.next_smooth()?;
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.deltas.next_smooth()?;
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 4] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 4].u;
                        pix.v = mpf.chroma[x / 4].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}
fn line_pred_4x4_mvi2(mpf: &mut MPFrame) -> DecoderResult<()> {
    for y in 0..mpf.height {
        let fline = &mut mpf.frame[(y ^ 3) * mpf.width..][..mpf.width];
        let mline = &mpf.map[(y ^ 3) * mpf.width..][..mpf.width];
        let eline = &mpf.lrmap[(y ^ 3) * mpf.width..][..mpf.width];

        let mut pix = mpf.col_pred[y ^ 3];
        let mut x = if mline[0] == 0 {
                1
            } else { 0 };
        while x < mpf.width {
            let mp = usize::from(mline[x]);
            if mp != 0 {
                if !eline[x] {
                    if (y & 3) == 0 {
                        for x1 in (((x + 3) & !3)..(x + mp)).step_by(4) {
                            mpf.chroma[x1 / 4] = fline[x1].into();
                        }
                    }
                    x += mp;
                    if x < mpf.width {
                        pix = fline[x - 1].into();
                    }
                } else {
                    let mut left = mp;
                    // adjacent lowres areas are treated as one
                    let mut end_x = x + mp;
                    while end_x < mpf.width && mline[end_x] != 0 && eline[end_x] {
                        left  += usize::from(mline[end_x]);
                        end_x += usize::from(mline[end_x]);
                    }

                    let last_interp = end_x < mpf.width && mline[end_x] != 0;
                    let cand_pos = ((y & 1) == 0 && (end_x & 1) == 0) || ((y & 1) == 1 && (end_x & 1) == 1);
                    if !last_interp {
                        // last low-res sample can be merged with the first normal delta sample
                        if end_x < mpf.width && cand_pos && mline[end_x] == 0 {
                            left  += 1;
                            end_x += 1;
                        }
                    } else if cand_pos {
                        left -= 1;
                    }

                    match y & 3 {
                        0 => lowres_pred4_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mut mpf.chroma)?,
                        2 => lowres_pred4_even_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mpf.chroma)?,
                        _ => lowres_pred4_odd_mvi2(fline, &mut x, left, &mut pix, &mut mpf.deltas, &mpf.chroma)?,
                    }
                    if last_interp && x < end_x {
                        let ydelta = (YuvPixel::from(fline[x + 1]).y - pix.y) >> 1;
                        let mut lpix = pix;
                        lpix.y = (lpix.y + ydelta).clip_y();
                        fline[x] = lpix.into();
                        x += 1;
                    }
                }
            } else {
                let delta = mpf.deltas.next_smooth()?;
                pix.y = (pix.y + delta).clip_y();
                if (x & 3) == 0 {
                    if (y & 3) == 0 {
                        let delta = mpf.deltas.next_smooth()?;
                        pix.v = (pix.v + delta).clip_c();
                        let delta = mpf.deltas.next_smooth()?;
                        pix.u = (pix.u + delta).clip_c();
                        mpf.chroma[x / 4] = pix;
                    } else {
                        pix.u = mpf.chroma[x / 4].u;
                        pix.v = mpf.chroma[x / 4].v;
                    }
                }
                fline[x] = pix.into();
                x += 1;
            }
        }
        mpf.deltas.switch_line();
    }
    Ok(())
}

fn lowres_pred2_mvi2(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &mut [YuvPixel]) -> DecoderResult<()> {
    let mut last_delta = None;
    for i in 0..len {
        if (*x & 1) == 0 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else if i == 0 {
                    deltas.next_smooth()?
                } else {
                    deltas.next()?
                };
            pix.y = (pix.y + ydelta).clip_y();
            if (*x & 1) == 0 {
                let vdelta = deltas.next_smooth()?;
                pix.v = (pix.v + vdelta).clip_c();
                let udelta = deltas.next_smooth()?;
                pix.u = (pix.u + udelta).clip_c();
                chroma[*x / 2] = *pix;
            }
            fline[*x] = (*pix).into();
        } else {
            let ydelta = deltas.next()?;
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}
fn lowres_pred2_odd_mvi2(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &[YuvPixel]) -> DecoderResult<()> {
    let mut last_delta = None;
    let start = if (*x & 1) == 1 && len > 1 {
            let ydelta = deltas.next_smooth()?;
            pix.y = (pix.y + ydelta).clip_y();
            fline[*x] = (*pix).into();
            *x += 1;
            1
        } else { 0 };
    for _i in start..len {
        if (*x & 1) == 1 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else {
                    deltas.next()?
                };
            pix.y = (pix.y + ydelta).clip_y();
            pix.u = chroma[*x / 2].u;
            pix.v = chroma[*x / 2].v;
            fline[*x] = (*pix).into();
        } else {
            let ydelta = deltas.next()?;
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            lpix.u = chroma[*x / 2].u;
            lpix.v = chroma[*x / 2].v;
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}

fn lowres_pred4_mvi2(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &mut [YuvPixel]) -> DecoderResult<()> {
    let mut last_delta = None;
    for i in 0..len {
        if (*x & 1) == 0 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else if i == 0 {
                    deltas.next_smooth()?
                } else {
                    deltas.next()?
                };
            pix.y = (pix.y + ydelta).clip_y();
            if (*x & 3) == 0 {
                let vdelta = deltas.next_smooth()?;
                pix.v = (pix.v + vdelta).clip_c();
                let udelta = deltas.next_smooth()?;
                pix.u = (pix.u + udelta).clip_c();
                chroma[*x / 4] = *pix;
            }
            fline[*x] = (*pix).into();
        } else {
            let ydelta = deltas.next()?;
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}
fn lowres_pred4_even_mvi2(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &[YuvPixel]) -> DecoderResult<()> {
    let mut last_delta = None;
    for i in 0..len {
        if (*x & 1) == 0 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else if i == 0 {
                    deltas.next_smooth()?
                } else {
                    deltas.next()?
                };
            pix.y = (pix.y + ydelta).clip_y();
            if (*x & 3) == 0 {
                pix.u = chroma[*x / 4].u;
                pix.v = chroma[*x / 4].v;
            }
            fline[*x] = (*pix).into();
        } else {
            let ydelta = deltas.next()?;
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}
fn lowres_pred4_odd_mvi2(fline: &mut [u16], x: &mut usize, len: usize, pix: &mut YuvPixel, deltas: &mut MPDeltas, chroma: &[YuvPixel]) -> DecoderResult<()> {
    let mut last_delta = None;
    for i in 0..len {
        if (*x & 1) == 1 {
            let ydelta = if let Some(delta) = last_delta {
                    last_delta = None;
                    delta
                } else if i == 0 {
                    deltas.next_smooth()?
                } else {
                    deltas.next()?
                };
            pix.y = (pix.y + ydelta).clip_y();
            if (*x & 3) == 1 {
                pix.u = chroma[*x / 4].u;
                pix.v = chroma[*x / 4].v;
            }
            fline[*x] = (*pix).into();
        } else {
            let ydelta = deltas.next()?;
            last_delta = Some(ydelta);
            let mut lpix = *pix;
            if (*x & 3) == 0 {
                lpix.u = chroma[*x / 4].u;
                lpix.v = chroma[*x / 4].v;
            }
            lpix.y = (lpix.y + (ydelta >> 1)).clip_y();
            fline[*x] = lpix.into();
        }
        *x += 1;
    }
    Ok(())
}
