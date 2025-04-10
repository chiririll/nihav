use nihav_core::codecs::*;
use nihav_core::io::bitreader::*;

const ORDER: usize = 10;
const BLOCK_SIZE: usize = 160;
const MAX_PITCH: usize = 143;

#[derive(Clone,Copy,Debug,PartialEq)]
enum QcelpMode {
    Full,
    Half,
    Quarter,
    Octavo,
    Silence,
}

fn get_mode(src: &[u8]) -> DecoderResult<QcelpMode> {
    match src.len() {
        35 => Ok(QcelpMode::Full),
        17 => Ok(QcelpMode::Half),
         8 => Ok(QcelpMode::Quarter),
         4 => Ok(QcelpMode::Octavo),
         1 => Ok(QcelpMode::Silence),
         _ => Err(DecoderError::InvalidData),
    }
}

#[derive(Copy,Clone,Default)]
struct LSP {
    data:   [f32; ORDER],
}

impl LSP {
    fn new_init() -> Self {
        let mut data = [0.0; ORDER];
        for (i, el) in data.iter_mut().enumerate() {
            *el = ((i + 1) as f32) / 11.0;
        }
        Self { data }
    }
    fn from_cb(lsp_idx: &[usize; 5]) -> Self {
        const LSP_SCALE: f32 = 0.0001;
        let mut data = [0.0; ORDER];

        let mut acc = 0.0;
        for (pair, (&cb, &idx)) in data.chunks_exact_mut(2)
                .zip(LSP_CBS.iter().zip(lsp_idx.iter())) {
            acc += (cb[idx][0] as f32) * LSP_SCALE;
            pair[0] = acc;
            acc += (cb[idx][1] as f32) * LSP_SCALE;
            pair[1] = acc;
        }

        Self { data }
    }
    fn blend(&mut self, other: &Self, weight: f32) {
        for (dst, &src) in self.data.iter_mut().zip(other.data.iter()) {
            *dst = *dst * weight + src * (1.0 - weight);
        }
    }
    fn get_lpc(&self) -> [f32; ORDER] {
        const BANDWIDTH_EXPANSION: f64 = 0.9883;
        let mut tmp = [0.0f64; ORDER];

        for (dst, &src) in tmp.iter_mut().zip(self.data.iter()) {
            *dst = (f64::from(src) * std::f64::consts::PI).cos();
        }

        // LSP to LPC
        let mut p = [0.0f64; ORDER / 2 + 1];
        let mut q = [0.0f64; ORDER / 2 + 1];

        Self::get_lsp_coeffs(&tmp, &mut p);
        Self::get_lsp_coeffs(&tmp[1..], &mut q);

        let (half0, half1) = tmp.split_at_mut(ORDER / 2);
        for ((dst1, dst2), (pp, qp)) in half0.iter_mut().rev().zip(half1.iter_mut())
                .zip(p.windows(2).rev().zip(q.windows(2).rev())) {
            let p1 = pp[1] + pp[0];
            let q1 = qp[1] - qp[0];
            *dst1 = (p1 + q1) * 0.5;
            *dst2 = (p1 - q1) * 0.5;
        }

        let mut ret = [0.0; ORDER];
        let mut scale = BANDWIDTH_EXPANSION;
        for (dst, &src) in ret.iter_mut().zip(tmp.iter()) {
            *dst = (src * scale) as f32;
            scale *= BANDWIDTH_EXPANSION;
        }

        ret
    }
    fn get_lsp_coeffs(src: &[f64], dst: &mut [f64; ORDER / 2 + 1]) {
        dst[0] = 1.0;
        dst[1] = -2.0 * src[0];
        for (i, src_coef) in src[2..].chunks(2).enumerate() {
            let scale = -2.0 * src_coef[0];
            dst[i + 2] = scale * dst[i + 1] + 2.0 * dst[i];
            for j in (2..=i+1).rev() {
                dst[j] += scale * dst[j - 1] + dst[j - 2];
            }
            dst[1] += scale;
        }
    }
}

#[derive(Default)]
struct QcelpFrame {
    cb_sign:    [bool; 16],
    cb_gain:    [u8; 16],
    cb_index:   [usize; 16],
    lag:        [u8; 4],
    lag_frac:   [bool; 4],
    gain:       [u8; 4],
    lsp_data:   [usize; ORDER / 2],
    lsp_flags:  [bool; ORDER],
    oct_seed:   u16,
    cb_seed:    u8,
}

impl QcelpFrame {
    fn read(&mut self, src: &[u8], mode: QcelpMode) -> DecoderResult<()> {
        let mut br = BitReader::new(src, BitReaderMode::BE);

        br.skip(8)?; // mode ID
        match mode {
            QcelpMode::Full => {
                for &(idx, nbits) in [(2u8, 3u8), (1, 7), (0, 6), (4, 6), (3, 6)].iter() {
                    self.lsp_data[usize::from(idx)] = br.read(nbits)? as usize;
                }
                self.lsp_data[2] |= (br.read(4)? as usize) << 3;

                self.cb_sign[0]     = br.read_bool()?;
                self.cb_gain[0]     = br.read(4)? as u8;
                self.lag_frac[0]    = br.read_bool()?;
                self.lag[0]         = br.read(7)? as u8;
                self.gain[0]        = br.read(3)? as u8;
                self.cb_index[1]    = br.read(4)? as usize;
                self.cb_sign[1]     = br.read_bool()?;
                self.cb_gain[1]     = br.read(4)? as u8;
                self.cb_index[0]    = br.read(7)? as usize;
                self.cb_gain[3]     = br.read(1)? as u8;
                self.cb_index[2]    = br.read(7)? as usize;
                self.cb_sign[2]     = br.read_bool()?;
                self.cb_gain[2]     = br.read(4)? as u8;
                self.cb_index[1]   |= (br.read(3)? as usize) << 4;
                self.lag[1]         = br.read(3)? as u8;
                self.gain[1]        = br.read(3)? as u8;
                self.cb_index[3]    = br.read(7)? as usize;
                self.cb_sign[3]     = br.read_bool()?;
                self.cb_gain[3]    |= (br.read(2)? as u8) << 1;
                self.cb_index[4]    = br.read(6)? as usize;
                self.cb_sign[4]     = br.read_bool()?;
                self.cb_gain[4]     = br.read(4)? as u8;
                self.lag_frac[1]    = br.read_bool()?;
                self.lag[1]        |= (br.read(4)? as u8) << 3;
                self.cb_gain[6]     = br.read(3)? as u8;
                self.cb_index[5]    = br.read(7)? as usize;
                self.cb_sign[5]     = br.read_bool()?;
                self.cb_gain[5]     = br.read(4)? as u8;
                self.cb_index[4]   |= (br.read(1)? as usize) << 6;
                self.cb_index[7]    = br.read(3)? as usize;
                self.cb_sign[7]     = br.read_bool()?;
                self.cb_gain[7]     = br.read(3)? as u8;
                self.cb_index[6]    = br.read(7)? as usize;
                self.cb_sign[6]     = br.read_bool()?;
                self.cb_gain[6]    |= (br.read(1)? as u8) << 3;
                self.cb_gain[8]     = br.read(1)? as u8;
                self.lag_frac[2]    = br.read_bool()?;
                self.lag[2]         = br.read(7)? as u8;
                self.gain[2]        = br.read(3)? as u8;
                self.cb_index[7]   |= (br.read(4)? as usize) << 3;
                self.cb_sign[9]     = br.read_bool()?;
                self.cb_gain[9]     = br.read(4)? as u8;
                self.cb_index[8]    = br.read(7)? as usize;
                self.cb_sign[8]     = br.read_bool()?;
                self.cb_gain[8]    |= (br.read(3)? as u8) << 1;
                self.cb_index[10]   = br.read(4)? as usize;
                self.cb_sign[10]    = br.read_bool()?;
                self.cb_gain[10]    = br.read(4)? as u8;
                self.cb_index[9]    = br.read(7)? as usize;
                self.gain[3]        = br.read(2)? as u8;
                self.cb_index[11]   = br.read(7)? as usize;
                self.cb_sign[11]    = br.read_bool()?;
                self.cb_gain[11]    = br.read(3)? as u8;
                self.cb_index[10]  |= (br.read(3)? as usize) << 4;
                self.cb_index[12]   = br.read(2)? as usize;
                self.cb_sign[12]    = br.read_bool()?;
                self.cb_gain[12]    = br.read(4)? as u8;
                self.lag_frac[3]    = br.read_bool()?;
                self.lag[3]         = br.read(7)? as u8;
                self.gain[3]       |= (br.read(1)? as u8) << 2;
                self.cb_index[13]   = br.read(6)? as usize;
                self.cb_sign[13]    = br.read_bool()?;
                self.cb_gain[13]    = br.read(4)? as u8;
                self.cb_index[12]  |= (br.read(5)? as usize) << 2;
                self.cb_gain[15]    = br.read(3)? as u8;
                self.cb_index[14]   = br.read(7)? as usize;
                self.cb_sign[14]    = br.read_bool()?;
                self.cb_gain[14]    = br.read(4)? as u8;
                self.cb_index[13]  |= (br.read(1)? as usize) << 6;
                let reserved        = br.read(2)? as u8;
                validate!(reserved == 0);
                self.cb_index[15]   = br.read(7)? as usize;
                self.cb_sign[15]    = br.read_bool()?;
            },
            QcelpMode::Half => {
                for &(idx, nbits) in [(2u8, 3u8), (1, 7), (0, 6), (4, 6), (3, 6)].iter() {
                    self.lsp_data[usize::from(idx)] = br.read(nbits)? as usize;
                }
                self.lsp_data[2] |= (br.read(4)? as usize) << 3;

                self.cb_sign[0]     = br.read_bool()?;
                self.cb_gain[0]     = br.read(4)? as u8;
                self.lag_frac[0]    = br.read_bool()?;
                self.lag[0]         = br.read(7)? as u8;
                self.gain[0]        = br.read(3)? as u8;
                self.lag[1]         = br.read(6)? as u8;
                self.gain[1]        = br.read(3)? as u8;
                self.cb_index[0]    = br.read(7)? as usize;
                self.gain[2]        = br.read(2)? as u8;
                self.cb_index[1]    = br.read(7)? as usize;
                self.cb_sign[1]     = br.read_bool()?;
                self.cb_gain[1]     = br.read(4)? as u8;
                self.lag_frac[1]    = br.read_bool()?;
                self.lag[1]        |= (br.read(1)? as u8) << 6;
                self.cb_index[2]    = br.read(2)? as usize;
                self.cb_sign[2]     = br.read_bool()?;
                self.cb_gain[2]     = br.read(4)? as u8;
                self.lag_frac[2]    = br.read_bool()?;
                self.lag[2]         = br.read(7)? as u8;
                self.gain[2]       |= (br.read(1)? as u8) << 2;
                self.lag_frac[3]    = br.read_bool()?;
                self.lag[3]         = br.read(7)? as u8;
                self.gain[3]        = br.read(3)? as u8;
                self.cb_index[2]   |= (br.read(5)? as usize) << 2;
                self.cb_index[3]    = br.read(7)? as usize;
                self.cb_sign[3]     = br.read_bool()?;
                self.cb_gain[3]     = br.read(4)? as u8;
            },
            QcelpMode::Quarter => {
                self.lsp_data[2] = br.read(3)? as usize;
                self.lsp_data[1] = br.read(7)? as usize;
                self.lsp_data[0] = br.read(6)? as usize;
                self.lsp_data[4] = br.read(6)? as usize;
                self.lsp_data[3] = br.read(6)? as usize;
                self.lsp_data[2] |= (br.read(4)? as usize) << 3;
                self.cb_gain[3] = br.read(4)? as u8;
                self.cb_gain[2] = br.read(4)? as u8;
                self.cb_gain[1] = br.read(4)? as u8;
                self.cb_gain[0] = br.read(4)? as u8;
                let reserved    = br.read(2)? as u8;
                validate!(reserved == 0);
                self.cb_gain[4] = br.read(4)? as u8;

                let mut pdiff = 0;
                for pair in self.cb_gain[..5].windows(2) {
                    let diff = (pair[1] as i8) - (pair[0] as i8);
                    validate!(diff.abs() <= 10);
                    validate!((diff - pdiff).abs() <= 12);
                    pdiff = diff;
                }
            },
            QcelpMode::Octavo => {
                self.oct_seed = br.peek(16) as u16;
                validate!(self.oct_seed != 0xFFFF);
                self.cb_seed     |= (br.read(1)? as u8) << 3;
                self.lsp_flags[0] = br.read_bool()?;
                self.lsp_flags[1] = br.read_bool()?;
                self.lsp_flags[2] = br.read_bool()?;
                self.cb_seed     |= (br.read(1)? as u8) << 2;
                self.lsp_flags[3] = br.read_bool()?;
                self.lsp_flags[4] = br.read_bool()?;
                self.lsp_flags[5] = br.read_bool()?;
                self.cb_seed     |= (br.read(1)? as u8) << 1;
                self.lsp_flags[6] = br.read_bool()?;
                self.lsp_flags[7] = br.read_bool()?;
                self.lsp_flags[8] = br.read_bool()?;
                self.cb_seed     |= br.read(1)? as u8;
                self.lsp_flags[9] = br.read_bool()?;
                self.cb_gain[0]   = br.read(2)? as u8;
                let reserved      = br.read(4)? as u8;
                validate!(reserved == 0);
            },
            QcelpMode::Silence => {},
        }

        // check for invalid pitch lag
        if matches!(mode, QcelpMode::Full | QcelpMode::Half) {
            for (&lag, &lag_frac) in self.lag.iter().zip(self.lag_frac.iter()) {
                validate!(lag < 124 || !lag_frac);
            }
        }

        Ok(())
    }
}

trait Rnd {
    fn next_rnd(&mut self) -> f32;
}

impl Rnd for u16 {
    fn next_rnd(&mut self) -> f32 {
        *self = self.wrapping_mul(521).wrapping_add(259);
        *self as i16 as f32
    }
}

struct QcelpDecoder {
    ainfo:              NAAudioInfo,
    chmap:              NAChannelMap,
    prev_lsp:           LSP,
    predictor:          LSP,
    quant_lsp:          LSP,
    gain:               [f32; 16],
    prev_gain_idx:      usize,
    pprev_gain_idx:     usize,
    last_cb_gain:       f32,
    rnd:                [f32; BLOCK_SIZE + 20],
    prev_mode:          QcelpMode,
    oct_count:          usize,
    pitch_gain:         [f32; 4],
    pitch_lag:          [usize; 4],
    pitch_synth_mem:    [f32; BLOCK_SIZE + MAX_PITCH],
    pitch_prefilt_mem:  [f32; BLOCK_SIZE + MAX_PITCH],
    formant_mem:        [f32; BLOCK_SIZE + ORDER],
    last_lpc:           [f32; ORDER],
    postfilt_hist:      [f32; ORDER],
    postfilt_agc:       f32,
    postfilt_tilt:      f32,
}

impl QcelpDecoder {
    fn new() -> Self {
        Self {
            ainfo:              NAAudioInfo::new(0, 1, SND_F32P_FORMAT, 1),
            chmap:              NAChannelMap::new(),
            prev_lsp:           LSP::new_init(),
            predictor:          LSP::default(),
            quant_lsp:          LSP::default(),
            gain:               [0.0; 16],
            prev_gain_idx:      0,
            pprev_gain_idx:     0,
            last_cb_gain:       0.0,
            rnd:                [0.0; BLOCK_SIZE + 20],
            prev_mode:          QcelpMode::Silence,
            oct_count:          0,
            pitch_gain:         [0.0; 4],
            pitch_lag:          [0; 4],
            pitch_synth_mem:    [0.0; 303],
            pitch_prefilt_mem:  [0.0; 303],
            formant_mem:        [0.0; BLOCK_SIZE + ORDER],
            last_lpc:           [0.0; ORDER],
            postfilt_hist:      [0.0; ORDER],
            postfilt_agc:       0.0,
            postfilt_tilt:      0.0,
        }
    }
    fn decode_gain(&mut self, mode: QcelpMode, frame: &mut QcelpFrame) -> DecoderResult<()> {
        match mode {
            QcelpMode::Full | QcelpMode::Half | QcelpMode::Quarter => {
                let subframes = match mode {
                        QcelpMode::Full => 16,
                        QcelpMode::Half => 4,
                        QcelpMode::Quarter => 5,
                        _ => unreachable!(),
                    };

                let mut tmp = [0; 16];
                for (i, gain) in self.gain[..subframes].iter_mut().enumerate() {
                    tmp[i] = usize::from(frame.cb_gain[i]) * 4;
                    if mode == QcelpMode::Full && ((i & 3) == 3) {
                        tmp[i] += ((tmp[i - 1] + tmp[i - 2] + tmp[i - 3]) / 3).saturating_sub(6).min(32);
                    }
                    validate!(tmp[i] < QCELP_GAINS.len());
                    *gain = QCELP_GAINS[tmp[i]];
                    if frame.cb_sign[i] {
                        *gain = -*gain;
                        frame.cb_index[i]  = frame.cb_index[i].wrapping_sub(89) & 0x7F;
                    }
                }
                self.pprev_gain_idx = tmp[subframes - 2];
                self.prev_gain_idx  = tmp[subframes - 1];
                self.last_cb_gain = QCELP_GAINS[self.prev_gain_idx];

                if mode == QcelpMode::Quarter {
                    self.gain[7] = self.gain[4];
                    self.gain[6] = self.gain[4] * 0.6 + self.gain[3] * 0.4;
                    self.gain[5] = self.gain[3];
                    self.gain[4] = self.gain[3] * 0.2 + self.gain[2] * 0.8;
                    self.gain[3] = self.gain[2] * 0.8 + self.gain[1] * 0.2;
                    self.gain[2] = self.gain[1];
                    self.gain[1] = self.gain[1] * 0.4 + self.gain[0] * 0.6;
                }
            },
            QcelpMode::Octavo => {
                let idx = 2 * usize::from(frame.cb_gain[0]) + ((self.prev_gain_idx + self.pprev_gain_idx) / 2).saturating_sub(5).max(54);

                let delta = (QCELP_GAINS[self.pprev_gain_idx] - self.last_cb_gain) / 8.0;
                for (i, gain) in self.gain[..=8].iter_mut().enumerate().skip(1) {
                    *gain = self.last_cb_gain + delta * (i as f32);
                }
                self.last_cb_gain = self.gain[7];
                self.pprev_gain_idx = self.prev_gain_idx;
                self.prev_gain_idx = idx;
            },
            QcelpMode::Silence => {},
        }
        Ok(())
    }
    fn compute_cb_vector(&mut self, mode: QcelpMode, frame: &QcelpFrame, dst: &mut [f32]) {
        match mode {
            QcelpMode::Full => {
                for ((&gain, &cb_idx), seg) in self.gain.iter().zip(frame.cb_index.iter())
                        .zip(dst.chunks_exact_mut(10)).take(16) {
                    let scale = gain * 0.01;
                    let mut idx = (!cb_idx).wrapping_add(1);
                    for el in seg.iter_mut() {
                        *el = scale * (QCELP_FULL_MODE_CB[idx & 0x7F] as f32);
                        idx = idx.wrapping_add(1);
                    }
                }
            },
            QcelpMode::Half => {
                for ((&gain, &cb_idx), seg) in self.gain.iter().zip(frame.cb_index.iter())
                        .zip(dst.chunks_exact_mut(40)).take(4) {
                    let scale = gain * 0.5;
                    let mut idx = (!cb_idx).wrapping_add(1);
                    for el in seg.iter_mut() {
                        *el = scale * (QCELP_HALF_MODE_CB[idx & 0x7F] as f32);
                        idx = idx.wrapping_add(1);
                    }
                }
            },
            QcelpMode::Quarter => {
                let mut seed = (((frame.lsp_data[4] & 0x03) << 14) |
                                ((frame.lsp_data[3] & 0x3F) <<  8) |
                                ((frame.lsp_data[2] & 0x60) <<  1) |
                                ((frame.lsp_data[1] & 0x07) <<  3) |
                                ((frame.lsp_data[0] & 0x38) >>  3)) as u16;

                let mut rnd_idx = 20;
                for (&gain, seg) in self.gain.iter().zip(dst.chunks_exact_mut(20)) {
                    let scale = gain * 1.373681186 / 32768.0;
                    for el in seg.iter_mut() {
                        self.rnd[rnd_idx] = seed.next_rnd();

                        let mut sum = self.rnd[rnd_idx - 20..].iter()
                                .zip(self.rnd[..rnd_idx].iter().rev())
                                .zip(RND_FIR_COEFFS.iter())
                                .fold(0.0f32, |acc, ((&a, &b), &coef)| acc + coef * (a + b));
                        sum += RND_FIR_COEF10 * self.rnd[rnd_idx - 10];
                        *el = scale * sum;
                        rnd_idx += 1;
                    }
                }
                let (head, tail) = self.rnd.split_at_mut(20);
                head.copy_from_slice(&tail[BLOCK_SIZE - 20..]);
            },
            QcelpMode::Octavo => {
                let mut seed = frame.oct_seed;
                for (&gain, seg) in self.gain.iter().zip(dst.chunks_exact_mut(40)).take(8) {
                    let scale = gain * 1.373681186 / 32768.0;
                    for el in seg.iter_mut() {
                        *el = scale * seed.next_rnd();
                    }
                }
            },
            QcelpMode::Silence => {},
        }
    }
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn decode_lsp(&mut self, mode: QcelpMode, frame: &QcelpFrame) -> DecoderResult<()> {
        const SPREAD_FACTOR: f32 = 0.02;
        const OCTAVO_PRED_WEIGHT: f32 = 29.0 / 32.0;
        match mode {
            QcelpMode::Full | QcelpMode::Half | QcelpMode::Quarter => {
                self.quant_lsp = LSP::from_cb(&frame.lsp_data);

                let lsp = &self.quant_lsp;
                if mode != QcelpMode::Quarter {
                    validate!(lsp.data[9] > 0.66 && lsp.data[9] < 0.985);
                    for i in 4..10 {
                        validate!((lsp.data[i] - lsp.data[i - 4]).abs() >= 0.0931);
                    }
                } else {
                    validate!(lsp.data[9] > 0.80 && lsp.data[9] < 0.97);
                    for i in 3..10 {
                        validate!((lsp.data[i] - lsp.data[i - 2]).abs() >= 0.08);
                    }
                }
            },
            QcelpMode::Octavo => {
                let src = if self.prev_mode == QcelpMode::Octavo { &mut self.predictor } else { &mut self.prev_lsp };
                let mut lsp = LSP::new_init();
                for ((el, &src), &f_lsp) in lsp.data.iter_mut()
                        .zip(src.data.iter()).zip(frame.lsp_flags.iter()) {
                    *el = (if f_lsp { SPREAD_FACTOR } else { -SPREAD_FACTOR }) +
                          src * OCTAVO_PRED_WEIGHT + *el * (1.0 - OCTAVO_PRED_WEIGHT);
                }
                self.predictor = lsp;

                let smooth = if self.oct_count < 10 { 0.875 } else { 0.1 };
                lsp.data[0] = lsp.data[0].max(SPREAD_FACTOR);
                for i in 1..10 {
                    lsp.data[i] = lsp.data[i].max(lsp.data[i - 1] + SPREAD_FACTOR);
                }
                lsp.data[9] = lsp.data[9].min(1.0 - SPREAD_FACTOR);
                for i in (1..10).rev() {
                    lsp.data[i - 1] = lsp.data[i - 1].max(lsp.data[i] - SPREAD_FACTOR);
                }

                lsp.blend(&self.prev_lsp, smooth);
                self.quant_lsp = lsp;
            },
            QcelpMode::Silence => {},
        }
        Ok(())
    }
    fn apply_pitch_filters(&mut self, mode: QcelpMode, frame: &mut QcelpFrame, buf: &mut [f32]) {
        match mode {
            QcelpMode::Full | QcelpMode::Half => {
                for ((pgain, plag), (&lag, &gain)) in self.pitch_gain.iter_mut()
                        .zip(self.pitch_lag.iter_mut())
                        .zip(frame.lag.iter().zip(frame.gain.iter())) {
                    *pgain = if lag > 0 { ((gain + 1) as f32) * 0.25 } else { 0.0 };
                    *plag  = usize::from(lag) + 16;
                }
            },
            QcelpMode::Quarter | QcelpMode::Octavo => {
                self.pitch_synth_mem[..MAX_PITCH].copy_from_slice(&buf[BLOCK_SIZE - MAX_PITCH..]);
                self.pitch_prefilt_mem[..MAX_PITCH].copy_from_slice(&buf[BLOCK_SIZE - MAX_PITCH..]);
                self.pitch_gain = [0.0; 4];
                self.pitch_lag = [0; 4];
                return;
            },
            QcelpMode::Silence => {
                for el in self.pitch_gain.iter_mut() {
                    *el = el.min(1.0);
                }
                frame.lag_frac = [false; 4];
            },
        }

        Self::pitch_filter(&mut self.pitch_synth_mem, buf, &self.pitch_gain, &self.pitch_lag, &frame.lag_frac);

        for gain in self.pitch_gain.iter_mut() {
            *gain = gain.min(1.0) * 0.5;
        }

        Self::pitch_filter(&mut self.pitch_prefilt_mem, &self.pitch_synth_mem[MAX_PITCH..], &self.pitch_gain, &self.pitch_lag, &frame.lag_frac);

        for (dst, (src1, src2)) in buf.chunks_exact_mut(BLOCK_SIZE / 4)
                .zip(self.pitch_synth_mem[MAX_PITCH..].chunks_exact(BLOCK_SIZE / 4)
                    .zip(self.pitch_prefilt_mem[MAX_PITCH..].chunks_exact(BLOCK_SIZE / 4))) {
            let energy1 = src1.iter().fold(0.0f32, |acc, &a| acc + a * a);
            let energy2 = src2.iter().fold(0.0f32, |acc, &a| acc + a * a);
            let scale = (energy1 / energy2).sqrt();
            for (el, &src) in dst.iter_mut().zip(src2.iter()) {
                *el = src * scale;
            }
        }
    }
    fn pitch_filter(mem: &mut [f32; BLOCK_SIZE + MAX_PITCH], src: &[f32], gain: &[f32; 4], lags: &[usize; 4], pitch_frac: &[bool; 4]) {
        const HAMMSINC_FILTER: [f32; 4] = [ -0.006822,  0.041249, -0.143459,  0.588863 ];

        let mut out_idx = MAX_PITCH;
        for (sf, in_blk) in src.chunks_exact(BLOCK_SIZE / 4).enumerate() {
            if gain[sf] != 0.0 {
                let lag_idx = out_idx - lags[sf];
                if !pitch_frac[sf] {
                    for (i, &src) in in_blk.iter().enumerate() {
                        mem[out_idx + i] = mem[lag_idx + i] * gain[sf] + src;
                    }
                } else {
                    for (i, &src) in in_blk.iter().enumerate() {
                        let sum = mem[lag_idx + i - 4..].iter()
                                    .zip(mem[..lag_idx + i + 4].iter().rev())
                                    .zip(HAMMSINC_FILTER.iter())
                                    .fold(0.0f32, |acc, ((&a, &b), &coef)| acc + (a + b) * coef);
                        mem[out_idx + i] = sum * gain[sf] + src;
                    }
                }
            } else {
                mem[out_idx..][..BLOCK_SIZE / 4].copy_from_slice(in_blk);
            }
            out_idx += BLOCK_SIZE / 4;
        }

        let (head, tail) = mem.split_at_mut(BLOCK_SIZE);
        head[..MAX_PITCH].copy_from_slice(tail);
    }
    fn interpolate_lpc(&mut self, mode: QcelpMode, sf: usize) -> [f32; ORDER] {
        let weight = match mode {
                QcelpMode::Full | QcelpMode::Half | QcelpMode::Quarter => ((sf + 1) as f32) * 0.25,
                QcelpMode::Octavo if sf == 0 => 0.625,
                _ => 1.0,
            };

        if weight != 1.0 {
            let mut lsp = self.quant_lsp;
            lsp.blend(&self.prev_lsp, weight);
            lsp.get_lpc()
        } else if matches!(mode, QcelpMode::Full | QcelpMode::Half | QcelpMode::Quarter) {
            self.quant_lsp.get_lpc()
        } else if mode == QcelpMode::Silence && sf == 0 {
            self.prev_lsp.get_lpc()
        } else {
            self.last_lpc
        }
    }
    fn postfilter(&mut self, buf: &mut [f32]) {
        let mut lpc_z = self.last_lpc;
        let mut lpc_p = self.last_lpc;
        for (dst, &scale) in lpc_z.iter_mut().zip(POW_0_625.iter()) {
            *dst *= scale;
        }
        for (dst, &scale) in lpc_p.iter_mut().zip(POW_0_775.iter()) {
            *dst *= scale;
        }

        let mut zero_out = [0.0; BLOCK_SIZE];
        for (i, dst) in zero_out.iter_mut().enumerate() {
            *dst = self.formant_mem[i + ORDER];
            for (j, &coef) in lpc_z.iter().enumerate() {
                *dst += coef * self.formant_mem[i + ORDER - j - 1];
            }
        }

        let mut pole_out = [0.0; BLOCK_SIZE + ORDER];
        pole_out[..ORDER].copy_from_slice(&self.postfilt_hist);
        for (i, &src) in zero_out.iter().enumerate() {
            pole_out[i + ORDER] = src;
            for (j, &coef) in lpc_p.iter().enumerate() {
                pole_out[i + ORDER] -= coef * pole_out[i + ORDER - 1 - j];
            }
        }
        self.postfilt_hist.copy_from_slice(&pole_out[BLOCK_SIZE..]);

        let tilt_hist = pole_out[pole_out.len() - 1];
        let tilt = 0.3;
        for i in (1..BLOCK_SIZE).rev() {
            pole_out[ORDER + i] -= tilt * pole_out[ORDER + i - 1];
        }
        pole_out[ORDER] -= tilt * self.postfilt_tilt;
        self.postfilt_tilt = tilt_hist;

        const AGC_ALPHA: f32 = 0.9375;
        let in_energy = pole_out[ORDER..].iter().fold(0.0f32, |acc, &a| acc + a * a);
        let f_energy = self.formant_mem[ORDER..].iter().fold(0.0f32, |acc, &a| acc + a * a);
        let mut agc = self.postfilt_agc;

        let gain_factor = (if in_energy > 0.0 { (f_energy / in_energy).sqrt() } else { 1.0 }) * (1.0 - AGC_ALPHA);
        for (dst, &src) in buf.iter_mut().zip(pole_out[ORDER..].iter()) {
            agc = agc * AGC_ALPHA + gain_factor;
            *dst = src * agc;
        }
        self.postfilt_agc = agc;
    }
}

impl NADecoder for QcelpDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Audio(ainfo) = info.get_properties() {
            self.ainfo = NAAudioInfo::new(ainfo.sample_rate, 1, SND_F32P_FORMAT, BLOCK_SIZE);
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let info = pkt.get_stream().get_info();
        if let NACodecTypeInfo::Audio(_) = info.get_properties() {
            let pktbuf = pkt.get_buffer();

            let abuf = alloc_audio_buffer(self.ainfo, BLOCK_SIZE, self.chmap.clone())?;
            let mut adata = abuf.get_abuf_f32().unwrap();
            let dst = adata.get_data_mut().unwrap();

            let mode = get_mode(&pktbuf)?;
            let mut frame = QcelpFrame::default();
            frame.read(&pktbuf, mode)?;
            self.decode_gain(mode, &mut frame)?;
            self.compute_cb_vector(mode, &frame, dst);
            self.decode_lsp(mode, &frame)?;
            self.apply_pitch_filters(mode, &mut frame, dst);

            let mut fpos = 10;
            for (sf, data) in dst.chunks_exact_mut(BLOCK_SIZE / 4).enumerate() {
                let lpc = self.interpolate_lpc(mode, sf);
                self.last_lpc = lpc;
                for (i, &src) in data.iter().enumerate() {
                    self.formant_mem[fpos + i] = src;
                    for (j, &coef) in lpc.iter().enumerate() {
                        self.formant_mem[fpos + i] -= coef * self.formant_mem[fpos + i - 1 - j];
                    }
                }
                fpos += BLOCK_SIZE / 4;
            }

            self.postfilter(dst);

            let (head, tail) = self.formant_mem.split_at_mut(ORDER);
            head.copy_from_slice(&tail[BLOCK_SIZE - ORDER..]);

            self.prev_lsp = self.quant_lsp;

            self.prev_mode = mode;
            if mode == QcelpMode::Octavo {
                self.oct_count += 1;
            } else {
                self.oct_count = 0;
            }

            let mut frm = NAFrame::new_from_pkt(pkt, info.replace_info(NACodecTypeInfo::Audio(self.ainfo)), abuf);
            frm.set_duration(Some(BLOCK_SIZE as u64));
            frm.set_keyframe(false);
            Ok(frm.into_ref())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for QcelpDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(QcelpDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::qt_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_qcelp() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        // sample: https://samples.mplayerhq.hu/A-codecs/qclp/oldepisode.mov
        test_decoding("mov", "qualcomm-purevoice", "assets/QT/oldepisode.mov", Some(32), &dmx_reg, &dec_reg,
                      ExpectedTestResult::Decodes);
    }
}

const QCELP_GAINS: [f32; 61] = [
    1.000 / 8192.0,   1.125 / 8192.0,   1.250 / 8192.0,   1.375 / 8192.0,
    1.625 / 8192.0,   1.750 / 8192.0,   2.000 / 8192.0,   2.250 / 8192.0,
    2.500 / 8192.0,   2.875 / 8192.0,   3.125 / 8192.0,   3.500 / 8192.0,
    4.000 / 8192.0,   4.500 / 8192.0,   5.000 / 8192.0,   5.625 / 8192.0,
    6.250 / 8192.0,   7.125 / 8192.0,   8.000 / 8192.0,   8.875 / 8192.0,
   10.000 / 8192.0,  11.250 / 8192.0,  12.625 / 8192.0,  14.125 / 8192.0,
   15.875 / 8192.0,  17.750 / 8192.0,  20.000 / 8192.0,  22.375 / 8192.0,
   25.125 / 8192.0,  28.125 / 8192.0,  31.625 / 8192.0,  35.500 / 8192.0,
   39.750 / 8192.0,  44.625 / 8192.0,  50.125 / 8192.0,  56.250 / 8192.0,
   63.125 / 8192.0,  70.750 / 8192.0,  79.375 / 8192.0,  89.125 / 8192.0,
  100.000 / 8192.0, 112.250 / 8192.0, 125.875 / 8192.0, 141.250 / 8192.0,
  158.500 / 8192.0, 177.875 / 8192.0, 199.500 / 8192.0, 223.875 / 8192.0,
  251.250 / 8192.0, 281.875 / 8192.0, 316.250 / 8192.0, 354.875 / 8192.0,
  398.125 / 8192.0, 446.625 / 8192.0, 501.125 / 8192.0, 562.375 / 8192.0,
  631.000 / 8192.0, 708.000 / 8192.0, 794.375 / 8192.0, 891.250 / 8192.0,
 1000.000 / 8192.0
];

const QCELP_FULL_MODE_CB: [i16; 128] = [
     10,  -65,  -59,   12,  110,   34, -134,  157,
    104,  -84,  -34, -115,   23, -101,    3,   45,
   -101,  -16,  -59,   28,  -45,  134,  -67,   22,
     61,  -29,  226,  -26,  -55, -179,  157,  -51,
   -220,  -93,  -37,   60,  118,   74,  -48,  -95,
   -181,  111,   36,  -52, -215,   78, -112,   39,
    -17,  -47, -223,   19,   12,  -98, -142,  130,
     54, -127,   21,  -12,   39,  -48,   12,  128,
      6, -167,   82, -102,  -79,   55,  -44,   48,
    -20,  -53,    8,  -61,   11,  -70, -157, -168,
     20,  -56,  -74,   78,   33,  -63, -173,   -2,
    -75,  -53, -146,   77,   66,  -29,    9,  -75,
     65,  119,  -43,   76,  233,   98,  125, -156,
    -27,   78,   -9,  170,  176,  143, -148,   -7,
     27, -136,    5,   27,   18,  139,  204,    7,
   -184, -197,   52,   -3,   78, -189,    8,  -65
];
const QCELP_HALF_MODE_CB: [i16; 128] = [
     0, -4,  0, -3,  0,  0,  0,  0,
     0,  0,  0,  0,  0,  0,  0,  0,
     0, -3, -2,  0,  0,  0,  0,  0,
     0,  0,  0,  0,  0,  0,  0,  5,
     0,  0,  0,  0,  0,  0,  4,  0,
     0,  3,  2,  0,  3,  4,  0,  0,
     0,  0,  0,  0,  0,  0,  0,  0,
     0,  0,  0,  0,  0,  3,  0,  0,
    -3,  3,  0,  0, -2,  0,  3,  0,
     0,  0,  0,  0,  0,  0, -5,  0,
     0,  0,  0,  3,  0,  0,  0,  3,
     0,  0,  0,  0,  0,  0,  0,  4,
     0,  0,  0,  0,  0,  0,  0,  0,
     0,  3,  6, -3, -4,  0, -3, -3,
     3, -3,  0,  0,  0,  0,  0,  0,
     0,  0,  0,  0,  0,  0,  0,  0
];

const RND_FIR_COEFFS: [f32; 10] = [
    -0.1344519,  0.01735384, -0.06905826, 0.02434368,
    -0.08210701, 0.03041388, -0.09251384, 0.03501983,
    -0.09918777, 0.03749518
];
const RND_FIR_COEF10: f32 = 0.8985137;

const LSP_CB0: [[i16; 2]; 64] = [
    [  327,  118 ], [  919,  111 ], [  427,  440 ], [ 1327,  185 ],
    [  469,   50 ], [ 1272,   91 ], [  892,   59 ], [ 1771,  193 ],
    [  222,  158 ], [ 1100,  127 ], [  827,   55 ], [  978,  791 ],
    [  665,   47 ], [  700, 1401 ], [  670,  859 ], [ 1913, 1048 ],
    [  471,  215 ], [ 1046,  125 ], [  645,  298 ], [ 1599,  160 ],
    [  593,   39 ], [ 1187,  462 ], [  749,  341 ], [ 1520,  511 ],
    [  290,  792 ], [  909,  362 ], [  753,   81 ], [ 1111, 1058 ],
    [  519,  253 ], [  828,  839 ], [  685,  541 ], [ 1421, 1258 ],
    [  386,  130 ], [  962,  119 ], [  542,  387 ], [ 1431,  185 ],
    [  526,   51 ], [ 1175,  260 ], [  831,  167 ], [ 1728,  510 ],
    [  273,  437 ], [ 1172,  113 ], [  771,  144 ], [ 1122,  751 ],
    [  619,  119 ], [  492, 1276 ], [  658,  695 ], [ 1882,  615 ],
    [  415,  200 ], [ 1018,   88 ], [  681,  339 ], [ 1436,  325 ],
    [  555,  122 ], [ 1042,  485 ], [  826,  345 ], [ 1374,  743 ],
    [  383, 1018 ], [ 1005,  358 ], [  704,   86 ], [ 1301,  586 ],
    [  597,  241 ], [  832,  621 ], [  555,  573 ], [ 1504,  839 ]
];
const LSP_CB1: [[i16; 2]; 128] = [
    [  255,  293 ], [  904,  219 ], [  151, 1211 ], [ 1447,  498 ],
    [  470,  253 ], [ 1559,  177 ], [ 1547,  994 ], [ 2394,  242 ],
    [   91,  813 ], [  857,  590 ], [  934, 1326 ], [ 1889,  282 ],
    [  813,  472 ], [ 1057, 1494 ], [  450, 3315 ], [ 2163, 1895 ],
    [  538,  532 ], [ 1399,  218 ], [  146, 1552 ], [ 1755,  626 ],
    [  822,  202 ], [ 1299,  663 ], [  706, 1732 ], [ 2656,  401 ],
    [  418,  745 ], [  762, 1038 ], [  583, 1748 ], [ 1746, 1285 ],
    [  527, 1169 ], [ 1314,  830 ], [  556, 2116 ], [ 1073, 2321 ],
    [  297,  570 ], [  981,  403 ], [  468, 1103 ], [ 1740,  243 ],
    [  725,  179 ], [ 1255,  474 ], [ 1374, 1362 ], [ 1922,  912 ],
    [  285,  947 ], [  930,  700 ], [  593, 1372 ], [ 1909,  576 ],
    [  588,  916 ], [ 1110, 1116 ], [  224, 2719 ], [ 1633, 2220 ],
    [  402,  520 ], [ 1061,  448 ], [  402, 1352 ], [ 1499,  775 ],
    [  664,  589 ], [ 1081,  727 ], [  801, 2206 ], [ 2165, 1157 ],
    [  566,  802 ], [  911, 1116 ], [  306, 1703 ], [ 1792,  836 ],
    [  655,  999 ], [ 1061, 1038 ], [  298, 2089 ], [ 1110, 1753 ],
    [  361,  311 ], [  970,  239 ], [  265, 1231 ], [ 1495,  573 ],
    [  566,  262 ], [ 1569,  293 ], [ 1341, 1144 ], [ 2271,  544 ],
    [  214,  877 ], [  847,  719 ], [  794, 1384 ], [ 2067,  274 ],
    [  703,  688 ], [ 1099, 1306 ], [  391, 2947 ], [ 2024, 1670 ],
    [  471,  525 ], [ 1245,  290 ], [  264, 1557 ], [ 1568,  807 ],
    [  718,  399 ], [ 1193,  685 ], [  883, 1594 ], [ 2729,  764 ],
    [  500,  754 ], [  809, 1108 ], [  541, 1648 ], [ 1523, 1385 ],
    [  614, 1196 ], [ 1209,  847 ], [  345, 2242 ], [ 1442, 1747 ],
    [  199,  560 ], [ 1092,  194 ], [  349, 1253 ], [ 1653,  507 ],
    [  625,  354 ], [ 1376,  431 ], [ 1187, 1465 ], [ 2164,  872 ],
    [  360,  974 ], [ 1008,  698 ], [  704, 1346 ], [ 2114,  452 ],
    [  720,  816 ], [ 1240, 1089 ], [  439, 2475 ], [ 1498, 2040 ],
    [  336,  718 ], [ 1213,  187 ], [  451, 1450 ], [ 1368,  885 ],
    [  592,  578 ], [ 1131,  531 ], [  861, 1855 ], [ 1764, 1500 ],
    [  444,  970 ], [  935,  903 ], [  424, 1687 ], [ 1633, 1102 ],
    [  793,  897 ], [ 1060,  897 ], [  185, 2011 ], [ 1205, 1855 ]
];
const LSP_CB2: [[i16; 2]; 128] = [
    [  225,  283 ], [ 1296,  355 ], [  543,  343 ], [ 2073,  274 ],
    [  204, 1099 ], [ 1562,  523 ], [ 1388,  161 ], [ 2784,  274 ],
    [  112,  849 ], [ 1870,  175 ], [ 1189,  160 ], [ 1490, 1088 ],
    [  969, 1115 ], [  659, 3322 ], [ 1158, 1073 ], [ 3183, 1363 ],
    [  517,  223 ], [ 1740,  223 ], [  704,  387 ], [ 2637,  234 ],
    [  692, 1005 ], [ 1287, 1610 ], [  952,  532 ], [ 2393,  646 ],
    [  490,  552 ], [ 1619,  657 ], [  845,  670 ], [ 1784, 2280 ],
    [  191, 1775 ], [  272, 2868 ], [  942,  952 ], [ 2628, 1479 ],
    [  278,  579 ], [ 1565,  218 ], [  814,  180 ], [ 2379,  187 ],
    [  276, 1444 ], [ 1199, 1223 ], [ 1200,  349 ], [ 3009,  307 ],
    [  312,  844 ], [ 1898,  306 ], [  863,  470 ], [ 1685, 1241 ],
    [  513, 1727 ], [  711, 2233 ], [ 1085,  864 ], [ 3398,  527 ],
    [  414,  440 ], [ 1356,  612 ], [  964,  147 ], [ 2173,  738 ],
    [  465, 1292 ], [  877, 1749 ], [ 1104,  689 ], [ 2105, 1311 ],
    [  580,  864 ], [ 1895,  752 ], [  652,  609 ], [ 1485, 1699 ],
    [  514, 1400 ], [  386, 2131 ], [  933,  798 ], [ 2473,  986 ],
    [  334,  360 ], [ 1375,  398 ], [  621,  276 ], [ 2183,  280 ],
    [  311, 1114 ], [ 1382,  807 ], [ 1284,  175 ], [ 2605,  636 ],
    [  230,  816 ], [ 1739,  408 ], [ 1074,  176 ], [ 1619, 1120 ],
    [  784, 1371 ], [  448, 3050 ], [ 1189,  880 ], [ 3039, 1165 ],
    [  424,  241 ], [ 1672,  186 ], [  815,  333 ], [ 2432,  324 ],
    [  584, 1029 ], [ 1137, 1546 ], [ 1015,  585 ], [ 2198,  995 ],
    [  574,  581 ], [ 1746,  647 ], [  733,  740 ], [ 1938, 1737 ],
    [  347, 1710 ], [  373, 2429 ], [  787, 1061 ], [ 2439, 1438 ],
    [  185,  536 ], [ 1489,  178 ], [  703,  216 ], [ 2178,  487 ],
    [  154, 1421 ], [ 1414,  994 ], [ 1103,  352 ], [ 3072,  473 ],
    [  408,  819 ], [ 2055,  168 ], [  998,  354 ], [ 1917, 1140 ],
    [  665, 1799 ], [  993, 2213 ], [ 1234,  631 ], [ 3003,  762 ],
    [  373,  620 ], [ 1518,  425 ], [  913,  300 ], [ 1966,  836 ],
    [  402, 1185 ], [  948, 1385 ], [ 1121,  555 ], [ 1802, 1509 ],
    [  474,  886 ], [ 1888,  610 ], [  739,  585 ], [ 1231, 2379 ],
    [  661, 1335 ], [  205, 2211 ], [  823,  822 ], [ 2480, 1179 ]
];
const LSP_CB3: [[i16; 2]; 64] = [
    [  348,  311 ], [  812, 1145 ], [  552,  461 ], [ 1826,  263 ],
    [  601,  675 ], [ 1730,  172 ], [ 1523,  193 ], [ 2449,  277 ],
    [  334,  668 ], [  805, 1441 ], [ 1319,  207 ], [ 1684,  910 ],
    [  582, 1318 ], [ 1403, 1098 ], [  979,  832 ], [ 2700, 1359 ],
    [  624,  228 ], [ 1292,  979 ], [  800,  195 ], [ 2226,  285 ],
    [  730,  862 ], [ 1537,  601 ], [ 1115,  509 ], [ 2720,  354 ],
    [  218, 1167 ], [ 1212, 1538 ], [ 1074,  247 ], [ 1674, 1710 ],
    [  322, 2142 ], [ 1263,  777 ], [  981,  556 ], [ 2119, 1710 ],
    [  193,  596 ], [ 1035,  957 ], [  694,  397 ], [ 1997,  253 ],
    [  743,  603 ], [ 1584,  321 ], [ 1346,  346 ], [ 2221,  708 ],
    [  451,  732 ], [ 1040, 1415 ], [ 1184,  230 ], [ 1853,  919 ],
    [  310, 1661 ], [ 1625,  706 ], [  856,  843 ], [ 2902,  702 ],
    [  467,  348 ], [ 1108, 1048 ], [  859,  306 ], [ 1964,  463 ],
    [  560, 1013 ], [ 1425,  533 ], [ 1142,  634 ], [ 2391,  879 ],
    [  397, 1084 ], [ 1345, 1700 ], [  976,  248 ], [ 1887, 1189 ],
    [  644, 2087 ], [ 1262,  603 ], [  877,  550 ], [ 2203, 1307 ]
];
const LSP_CB4: [[i16; 2]; 64] = [
    [  360,  222 ], [  820, 1097 ], [  601,  319 ], [ 1656,  198 ],
    [  604,  513 ], [ 1552,  141 ], [ 1391,  155 ], [ 2474,  261 ],
    [  269,  785 ], [ 1463,  646 ], [ 1123,  191 ], [ 2015,  223 ],
    [  785,  844 ], [ 1202, 1011 ], [  980,  807 ], [ 3014,  793 ],
    [  570,  180 ], [ 1135, 1382 ], [  778,  256 ], [ 1901,  179 ],
    [  807,  622 ], [ 1461,  458 ], [ 1231,  178 ], [ 2028,  821 ],
    [  387,  927 ], [ 1496, 1004 ], [  888,  392 ], [ 2246,  341 ],
    [  295, 1462 ], [ 1156,  694 ], [ 1022,  473 ], [ 2226, 1364 ],
    [  210,  478 ], [ 1029, 1020 ], [  722,  181 ], [ 1730,  251 ],
    [  730,  488 ], [ 1465,  293 ], [ 1303,  326 ], [ 2595,  387 ],
    [  458,  584 ], [ 1569,  742 ], [ 1029,  173 ], [ 1910,  495 ],
    [  605, 1159 ], [ 1268,  719 ], [  973,  646 ], [ 2872,  428 ],
    [  443,  334 ], [  835, 1465 ], [  912,  138 ], [ 1716,  442 ],
    [  620,  778 ], [ 1316,  450 ], [ 1186,  335 ], [ 1446, 1665 ],
    [  486, 1050 ], [ 1675, 1019 ], [  880,  278 ], [ 2214,  202 ],
    [  539, 1564 ], [ 1142,  533 ], [  984,  391 ], [ 2130, 1089 ]
];
const LSP_CBS: [&[[i16; 2]]; 5] = [&LSP_CB0, &LSP_CB1, &LSP_CB2, &LSP_CB3, &LSP_CB4];

const POW_0_775: [f32; ORDER] = [
    0.775000, 0.600625, 0.465484, 0.360750, 0.279582,
    0.216676, 0.167924, 0.130141, 0.100859, 0.078166
];
const POW_0_625: [f32; ORDER] = [
    0.625000, 0.390625, 0.244141, 0.152588, 0.095367,
    0.059605, 0.037253, 0.023283, 0.014552, 0.009095
];
