use super::cell::{INTRA_FLAT_THRESHOLD, INTER_FLAT_THRESHOLD};

#[derive(Default)]
pub struct RateControl {
    bitrate:    u32,
    br_pool:    u32,
    f_pos:      u32,
    fracs:      u32,
    key_int:    u32,
    tb_num:     u32,
    tb_den:     u32,
    quality:    Option<u8>,
}

impl RateControl {
    pub fn new() -> Self { Self{ key_int: 10, ..Default::default() } }
    pub fn set_quality(&mut self, quality: u8) {
        if quality > 0 {
            self.quality = Some((quality - 1).min(63));
        } else {
            self.quality = None;
        }
    }
    pub fn set_bitrate(&mut self, br: u32, tb_num: u32, tb_den: u32) {
        if tb_num > 0 && tb_den > 0 {
            self.bitrate = br / 8;
            self.tb_num  = tb_num;
            self.tb_den  = tb_den;
        } else {
            self.bitrate = 0;
            self.tb_num  = 0;
            self.tb_den  = 0;
        }
        self.quality = Some(0);
        self.reset();
    }
    pub fn set_key_int(&mut self, key_int: u32) {
        self.key_int = key_int;
        self.reset();
    }
    pub fn reset(&mut self) {
        self.br_pool = self.bitrate;
        self.f_pos   = 0;
        self.fracs   = self.tb_den;
    }
    pub fn get_key_int(&self) -> u32 { self.key_int }
    pub fn get_quant(&self, frameno: u32) -> (bool, Option<u8>, u32, u32) {
        let is_intra = self.key_int == 0 || (frameno % self.key_int) == 0;
        if let Some(q) = self.quality {
            // quality format mapping: LSB is altquant, then two bits of flat threshold, then three bits of quantiser
            let quant = ((q >> 3) & 7) + (q & 1) * 8;
            let qmul = (q >> 1) & 3;
            const I_MULT: [u32; 4] = [ 8, 12, 16, 24 ];
            const P_MULT: [u32; 4] = [ 8, 12, 16, 24 ];
            let ft_i = INTRA_FLAT_THRESHOLD * I_MULT[usize::from(qmul)] / 8;
            let ft_p = INTER_FLAT_THRESHOLD * P_MULT[usize::from(qmul)] / 8;
            (is_intra, Some(quant), ft_i, ft_p)
        } else {
            (is_intra, None, INTRA_FLAT_THRESHOLD, INTER_FLAT_THRESHOLD)
        }
    }
    pub fn advance(&mut self, size: u32, skipped: bool) {
        if self.bitrate != 0 {
            if !skipped {
                let expected = self.get_expected_size();
                let cur_quant = self.quality.unwrap_or(0);
                if (size > expected + expected / 10) && (cur_quant < 7) {
                    self.quality = Some(cur_quant + 1);
                } else if (size < expected - expected / 10) && (cur_quant > 0) {
                    self.quality = Some(cur_quant - 1);
                }
            }

            self.f_pos += self.tb_num;
            while self.f_pos >= self.tb_den {
                self.f_pos   -= self.tb_den;
                self.br_pool += self.bitrate;
                self.fracs   += self.tb_den;
            }
            self.fracs -= self.tb_num;

            self.br_pool = self.br_pool.saturating_sub(size).min(self.bitrate * 3);
        }
    }
    pub fn increase_quant(&mut self) {
        if let Some(q) = self.quality {
            if q < 63 {
                self.quality = Some(q + 1);
            }
        }
    }
    pub fn get_expected_size(&self) -> u32 {
        if self.bitrate != 0 {
            (u64::from(self.br_pool) * u64::from(self.tb_num) / u64::from(self.fracs)) as u32
        } else {
            0
        }
    }
}
