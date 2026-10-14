//! Bink Audio decoder for raw blocks, as stored in Miles Sound System `BCF1` streams.
//!
//! The bitstream is the DCT variant of the Bink Audio in `.bik` files (see `binkaud.rs`), with
//! three differences from the demuxer-driven decoder:
//! - a block is the payload of one frame, with no `u32` sample count in front;
//! - a block holds one sub-stream per two channels (`ceil(channels / 2)`), each aligned to 32
//!   bits from the start of the payload, so any channel count works;
//! - "Bink Audio 2" streams use 7-bit quantiser indices and store the magnitudes of a run
//!   before its signs.
//!
//! The decoder keeps the overlap state between blocks, so blocks must be fed in order.
use nihav_core::codecs::{DecoderError, DecoderResult};
use nihav_core::io::bitreader::*;
use nihav_codec_support::dsp::dct::*;

use super::binkaud::overlap;
use super::binkauddata::*;

/// Coefficients one run covers at most (`RUN_TAB` has 64 at most, times 8).
const MAX_RUN: usize = 64 * 8;

/// A Bink float: 5-bit exponent, 23-bit mantissa, sign; `mantissa * 2^(exponent - 23)`, so an
/// all-zero field is exactly 0.
fn read_float(br: &mut BitReader) -> DecoderResult<f32> {
    let exp = br.read(5)? as i32;
    let mant = br.read(23)?;
    let neg = br.read_bool()?;
    let v = (mant as f32) * 2.0f32.powi(exp - 23);
    Ok(if neg { -v } else { v })
}

/// Decoder of raw Bink Audio (DCT) blocks.
pub struct BinkAudioBlocks {
    channels:   usize,
    version2:   bool,
    len:        usize,
    duration:   usize,
    dct:        DCT,
    quants:     [f32; 96],
    bands:      [usize; MAX_BANDS + 1],
    num_bands:  usize,
    scale:      f32,
    coeffs:     Vec<f32>,
    delay:      Vec<Vec<f32>>,
    first:      Vec<bool>,
}

impl BinkAudioBlocks {
    /// Creates a decoder. `bink_audio_2` selects the Bink Audio 2 bitstream.
    ///
    /// Fails for a zero sample rate or channel count.
    pub fn new(sample_rate: u32, channels: usize, bink_audio_2: bool) -> DecoderResult<Self> {
        if sample_rate == 0 || channels == 0 {
            return Err(DecoderError::InvalidData);
        }
        let frame_bits = if sample_rate < 22050 { 9 } else if sample_rate < 44100 { 10 } else { 11 };
        let len = 1usize << frame_bits;
        let mut num_bands = 0;
        let mut bands = [MAX_BANDS + 1; MAX_BANDS + 1];
        init_bands((sample_rate >> 1) as usize, len, &mut num_bands, &mut bands);
        Ok(Self {
            channels,
            version2:   bink_audio_2,
            len,
            duration:   len - (len >> 4),
            dct:        DCT::new(DCTMode::DCT_III, len),
            quants:     get_quants_table(),
            bands,
            num_bands,
            scale:      (4.0 / (len as f32)).sqrt() / 1024.0,
            coeffs:     vec![0.0; len],
            delay:      vec![vec![0.0; len >> 4]; channels],
            first:      vec![true; channels],
        })
    }

    /// Samples per channel one block produces.
    pub fn samples_per_block(&self) -> usize { self.duration }

    /// Forgets the overlap state (call after a seek).
    pub fn reset(&mut self) {
        for d in self.delay.iter_mut() {
            d.fill(0.0);
        }
        self.first.fill(true);
    }

    /// Decodes one block, giving `samples_per_block()` samples for each channel (planar).
    pub fn decode(&mut self, payload: &[u8]) -> DecoderResult<Vec<Vec<f32>>> {
        let mut br = BitReader::new(payload, BitReaderMode::LE);
        let mut out = vec![vec![0.0f32; self.duration]; self.channels];
        let mut ch = 0;
        while ch < self.channels {
            br.skip(2)?;
            let in_stream = (self.channels - ch).min(2);
            for c in ch..ch + in_stream {
                self.decode_coeffs(&mut br)?;
                self.output(c, &mut out[c]);
            }
            ch += in_stream;
            if ch < self.channels {
                let pad = (32 - (br.tell() % 32)) % 32;
                br.skip(pad as u32)?;
            }
        }
        Ok(out)
    }

    fn decode_coeffs(&mut self, br: &mut BitReader) -> DecoderResult<()> {
        let c0 = read_float(br)? * self.scale;
        let c1 = read_float(br)? * self.scale;
        let qbits = if self.version2 { 7 } else { 8 };
        let mut quants = [0.0f32; MAX_BANDS];
        for quant in quants[..self.num_bands].iter_mut() {
            let idx = br.read(qbits)? as usize;
            *quant = self.quants[idx.min(self.quants.len() - 1)] * self.scale;
        }
        self.coeffs.fill(0.0);
        self.coeffs[0] = c0 * std::f32::consts::SQRT_2;
        self.coeffs[1] = c1;
        let mut idx = 2;
        let mut band_idx = 0;
        let mut mags = [0u32; MAX_RUN];
        while idx < self.len {
            let width = if br.read_bool()? { RUN_TAB[br.read(4)? as usize] * 8 } else { 8 };
            let end = (idx + width).min(self.len);
            let bits = br.read(4)? as u8;
            if bits != 0 {
                if self.version2 {
                    for m in mags[..end - idx].iter_mut() {
                        *m = br.read(bits)?;
                    }
                }
                for i in idx..end {
                    while self.bands[band_idx] <= i { band_idx += 1; }
                    let q = quants[band_idx - 1];
                    let mag = if self.version2 { mags[i - idx] } else { br.read(bits)? };
                    if mag != 0 {
                        let v = (mag as f32) * q;
                        self.coeffs[i] = if br.read_bool()? { -v } else { v };
                    }
                }
            }
            idx = end;
        }
        Ok(())
    }

    fn output(&mut self, ch: usize, dst: &mut [f32]) {
        self.dct.do_dct_inplace(&mut self.coeffs[0..]);
        let overlap_len = if self.first[ch] { 0 } else { self.len >> 4 };
        overlap(&self.delay[ch], &self.coeffs, dst, overlap_len, 1);
        dst[overlap_len..].copy_from_slice(&self.coeffs[overlap_len..self.duration]);
        let tail = self.coeffs[self.duration..self.duration + (self.len >> 4)].to_vec();
        self.delay[ch].copy_from_slice(&tail);
        self.first[ch] = false;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Writes bits least significant first, the order of `BitReaderMode::LE`.
    struct Bits(Vec<u8>, usize);
    impl Bits {
        fn new() -> Self { Self(vec![0; 8192], 0) }
        fn put(&mut self, value: u32, n: usize) {
            for i in 0..n {
                if i < 32 && (value >> i) & 1 != 0 {
                    self.0[self.1 / 8] |= 1 << (self.1 % 8);
                }
                self.1 += 1;
            }
        }
    }

    #[test]
    fn silence_has_the_right_shape() {
        for (channels, rate, bink2) in [(1, 48000, false), (2, 48000, false), (3, 44100, false), (6, 22050, true), (2, 16000, true)] {
            let mut dec = BinkAudioBlocks::new(rate, channels, bink2).unwrap();
            for _ in 0..2 {
                let out = dec.decode(&vec![0u8; 8192]).unwrap();
                assert_eq!(out.len(), channels);
                assert!(out.iter().all(|c| c.len() == dec.samples_per_block() && c.iter().all(|&s| s == 0.0)), "{channels} ch {rate}");
            }
        }
        assert_eq!(BinkAudioBlocks::new(48000, 2, false).unwrap().samples_per_block(), 1920);
        assert_eq!(BinkAudioBlocks::new(22050, 2, false).unwrap().samples_per_block(), 960);
        assert_eq!(BinkAudioBlocks::new(16000, 2, false).unwrap().samples_per_block(), 480);
    }

    #[test]
    fn a_dc_coefficient_gives_a_constant() {
        // c0 = 1.0 (mantissa 2^22, exponent 1), the rest zero; mono, old bitstream.
        let mut b = Bits::new();
        b.put(0, 2);
        b.put(1, 5);
        b.put(1 << 22, 23);
        let mut dec = BinkAudioBlocks::new(48000, 1, false).unwrap();
        let out = dec.decode(&b.0).unwrap();
        let first = out[0][0];
        assert!(first > 0.0);
        assert!(out[0].iter().all(|&s| (s - first).abs() < first * 1e-4), "{:?}", &out[0][..4]);
    }

    #[test]
    fn rejects_empty_setups_and_short_data() {
        assert!(BinkAudioBlocks::new(0, 2, false).is_err());
        assert!(BinkAudioBlocks::new(48000, 0, false).is_err());
        let mut dec = BinkAudioBlocks::new(48000, 2, false).unwrap();
        assert!(dec.decode(&[0u8; 4]).is_err());
    }

    #[test]
    fn version2_reads_magnitudes_before_signs() {
        // mono, Bink Audio 2: first run of 8 coefficients at index 2 with width 1 bit:
        // magnitudes 1,0,0,0,0,0,0,0 then one sign bit (negative).
        let mut b = Bits::new();
        b.put(0, 2);
        b.put(0, 29);
        b.put(0, 29);
        let mut dec = BinkAudioBlocks::new(48000, 1, true).unwrap();
        b.put(0, 7 * dec.num_bands);
        b.put(0, 1); // run of 8
        b.put(1, 4); // 1 bit per value
        b.put(1, 1);
        b.put(0, 7);
        b.put(1, 1); // the sign of the only non-zero value
        let out = dec.decode(&b.0).unwrap();
        assert_eq!(out[0].len(), 1920);
        assert!(out[0].iter().any(|&s| s != 0.0), "the coefficient was not decoded");
        // The same run with the magnitude after the sign would read a different bit as the
        // magnitude of the second value; the sign bit must have been consumed after all magnitudes.
        let mut c = Bits::new();
        c.put(0, 2);
        c.put(0, 58);
        c.put(0, 7 * dec.num_bands);
        c.put(0, 1);
        c.put(1, 4);
        c.put(1, 1);
        c.put(0, 7);
        c.put(0, 1); // positive
        dec.reset();
        let pos = dec.decode(&c.0).unwrap();
        dec.reset();
        let neg = dec.decode(&b.0).unwrap();
        assert!(pos[0].iter().zip(&neg[0]).all(|(p, n)| (p + n).abs() < 1e-6), "the sign flips the block");
    }
}
