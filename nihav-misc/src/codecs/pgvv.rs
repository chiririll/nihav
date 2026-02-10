// Somewhat simplified JPEG decoder
// TODO: move common JPEG decoding into codec-support

use nihav_core::io::byteio::{ByteIO,MemoryReader};
use nihav_core::io::bitreader::*;
use nihav_core::io::codebook::*;
use nihav_core::codecs::*;
use nihav_codec_support::codecs::ZIGZAG;

const W1: i32 = 2841;
const W2: i32 = 2676;
const W3: i32 = 2408;
const W5: i32 = 1609;
const W6: i32 = 1108;
const W7: i32 =  565;
const W8: i32 =  181;

const ROW_SHIFT: u8 = 8;
const COL_SHIFT: u8 = 14;

#[allow(clippy::erasing_op)]
fn idct_row(row: &mut [i16]) {
    let in0 = ((i32::from(row[0])) << 11) + (1 << (ROW_SHIFT - 1));
    let in1 =  (i32::from(row[4])) << 11;
    let in2 =   i32::from(row[6]);
    let in3 =   i32::from(row[2]);
    let in4 =   i32::from(row[1]);
    let in5 =   i32::from(row[7]);
    let in6 =   i32::from(row[5]);
    let in7 =   i32::from(row[3]);

    let tmp = W7 * (in4 + in5);
    let a4 = tmp + (W1 - W7) * in4;
    let a5 = tmp - (W1 + W7) * in5;

    let tmp = W3 * (in6 + in7);
    let a6 = tmp - (W3 - W5) * in6;
    let a7 = tmp - (W3 + W5) * in7;

    let tmp = in0 + in1;

    let a0 = in0 - in1;
    let t1 = W6 * (in2 + in3);
    let a2 = t1 - (W2 + W6) * in2;
    let a3 = t1 + (W2 - W6) * in3;
    let b1 = a4 + a6;

    let b4 = a4 - a6;
    let t2 = a5 - a7;
    let b6 = a5 + a7;
    let b7 = tmp + a3;
    let b5 = tmp - a3;
    let b3 = a0 + a2;
    let b0 = a0 - a2;
    let b2 = (W8 * (b4 + t2) + 128) >> 8;
    let b4 = (W8 * (b4 - t2) + 128) >> 8;

    row[0] = ((b7 + b1) >> ROW_SHIFT) as i16;
    row[7] = ((b7 - b1) >> ROW_SHIFT) as i16;
    row[1] = ((b3 + b2) >> ROW_SHIFT) as i16;
    row[6] = ((b3 - b2) >> ROW_SHIFT) as i16;
    row[2] = ((b0 + b4) >> ROW_SHIFT) as i16;
    row[5] = ((b0 - b4) >> ROW_SHIFT) as i16;
    row[3] = ((b5 + b6) >> ROW_SHIFT) as i16;
    row[4] = ((b5 - b6) >> ROW_SHIFT) as i16;
}

#[allow(clippy::erasing_op)]
#[allow(clippy::identity_op)]
fn idct_col(blk: &mut [i16; 64], off: usize) {
    let in0 = ((i32::from(blk[off + 0*8])) << 8) + (1 << (COL_SHIFT - 1));
    let in1 =  (i32::from(blk[off + 4*8])) << 8;
    let in2 =   i32::from(blk[off + 6*8]);
    let in3 =   i32::from(blk[off + 2*8]);
    let in4 =   i32::from(blk[off + 1*8]);
    let in5 =   i32::from(blk[off + 7*8]);
    let in6 =   i32::from(blk[off + 5*8]);
    let in7 =   i32::from(blk[off + 3*8]);

    let tmp = W7 * (in4 + in5);
    let a4 = (tmp + (W1 - W7) * in4) >> 3;
    let a5 = (tmp - (W1 + W7) * in5) >> 3;

    let tmp = W3 * (in6 + in7);
    let a6 = (tmp - (W3 - W5) * in6) >> 3;
    let a7 = (tmp - (W3 + W5) * in7) >> 3;

    let tmp = in0 + in1;

    let a0 = in0 - in1;
    let t1 = W6 * (in2 + in3);
    let a2 = (t1 - (W2 + W6) * in2) >> 3;
    let a3 = (t1 + (W2 - W6) * in3) >> 3;
    let b1 = a4 + a6;

    let b4 = a4 - a6;
    let t2 = a5 - a7;
    let b6 = a5 + a7;
    let b7 = tmp + a3;
    let b5 = tmp - a3;
    let b3 = a0 + a2;
    let b0 = a0 - a2;
    let b2 = (W8 * (b4 + t2) + 128) >> 8;
    let b4 = (W8 * (b4 - t2) + 128) >> 8;

    blk[off + 0*8] = ((b7 + b1) >> COL_SHIFT) as i16;
    blk[off + 7*8] = ((b7 - b1) >> COL_SHIFT) as i16;
    blk[off + 1*8] = ((b3 + b2) >> COL_SHIFT) as i16;
    blk[off + 6*8] = ((b3 - b2) >> COL_SHIFT) as i16;
    blk[off + 2*8] = ((b0 + b4) >> COL_SHIFT) as i16;
    blk[off + 5*8] = ((b0 - b4) >> COL_SHIFT) as i16;
    blk[off + 3*8] = ((b5 + b6) >> COL_SHIFT) as i16;
    blk[off + 4*8] = ((b5 - b6) >> COL_SHIFT) as i16;
}

fn idct(blk: &mut [i16; 64]) {
    for i in 0..8 { idct_row(&mut blk[i*8..(i+1)*8]); }
    for i in 0..8 { idct_col(blk, i); }
}

fn put_block(blk: &[i16; 64], dst: &mut [u8], stride: usize) {
    for (drow, srow) in dst.chunks_mut(stride).zip(blk.chunks(8)) {
        for (del, &pix) in drow.iter_mut().zip(srow.iter()) {
            *del = pix.clamp(0, 255) as u8;
        }
    }
}

#[derive(Clone,Copy,Default)]
struct ComponentInfo {
    component_id:   usize,
    dc_table_id:    usize,
    ac_table_id:    usize,
}

struct RadiusStudioDecoder {
    info:       NACodecInfoRef,
    quant:      [[i16; 64]; 4],
    qselect:    [u8; MAX_CHROMATONS],
    subsamp:    [u8; MAX_CHROMATONS],
    codebook:   [[Option<Codebook<u8>>; 4]; 2],
    width:      usize,
    height:     usize,
    ilaced:     bool,
}

fn read_dc(br: &mut BitReader, cb: &Codebook<u8>) -> DecoderResult<i16> {
    let cat                             = br.read_cb(cb)?;
    if cat == 0 {
        Ok(0)
    } else {
        validate!(cat < 12);
        let add_bits                    = br.read(cat)? as i16;
        let pivot = 1 << (cat - 1);
        if add_bits < pivot {
            Ok(add_bits + 1 - pivot * 2)
        } else {
            Ok(add_bits)
        }
    }
}

fn read_ac(br: &mut BitReader, cb: &Codebook<u8>) -> DecoderResult<(usize, i16)> {
    let val                             = br.read_cb(cb)?;
    let run = usize::from(val >> 4);
    let cat = val & 0xF;
    let level = if cat != 0 {
            validate!(cat < 11);
            let add_bits                = br.read(cat)? as i16;
            let pivot = 1 << (cat - 1);
            if add_bits < pivot {
                add_bits + 1 - pivot * 2
            } else {
                add_bits
            }
        } else {
            validate!(run == 0 || run == 15);
            0
        };
    Ok((run, level))
}

fn read_block(br: &mut BitReader, blk: &mut [i16; 64], dc_cb: &Codebook<u8>, ac_cb: &Codebook<u8>, ss: usize, se: usize, qmat: &[i16; 64]) -> DecoderResult<()> {
    if ss == 0 {
        blk[0] = read_dc(br, dc_cb)?;
        blk[0] *= qmat[0];
    }
    let mut idx = 1;
    while idx <= se {
        let (run, level) = read_ac(br, ac_cb)?;
        if run == 0 && level == 0 {
            break;
        }
        idx += run;
        validate!(idx < 64);
        blk[ZIGZAG[idx]] = level * qmat[idx];
        idx += 1;
    }
    Ok(())
}

impl RadiusStudioDecoder {
    fn new() -> Self {
        let dummy_info = NACodecInfo::new_dummy();

        Self {
            info:       dummy_info,
            quant:      [[0; 64]; 4],
            qselect:    [0, 1, 1, 0, 0],
            subsamp:    [0x21, 0x11, 0x11, 0, 0],
            codebook:   [[None, None, None, None], [None, None, None, None]],
            width:      0,
            height:     0,
            ilaced:     false,
        }
    }

    #[allow(clippy::needless_range_loop)]
    fn decode_scan(&mut self, src: &[u8], mut buf: NAVideoBufferRef<u8>, ci: &[ComponentInfo], ss: usize, se: usize, field2: bool) -> DecoderResult<usize> {
        let num_components = ci.len();
        let mut last_dc = [1024; MAX_CHROMATONS];
        let mut dc_cbs = Vec::with_capacity(num_components);
        let mut ac_cbs = Vec::with_capacity(num_components);
        let mut qmats = [&self.quant[0]; MAX_CHROMATONS];
        for (i, cinfo) in ci.iter().enumerate() {
            dc_cbs.push(if let Some(ref cb) = self.codebook[0][cinfo.dc_table_id] {
                    cb
                } else { unreachable!(); });
            ac_cbs.push(if let Some(ref cb) = self.codebook[1][cinfo.ac_table_id] {
                    cb
                } else { unreachable!(); });
            qmats[i] = &self.quant[self.qselect[cinfo.component_id] as usize];
        }

        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();

        let mut br = BitReader::new(src, BitReaderMode::BE);

        let mut offs = frm.offset;
        let mut nblks = [0; MAX_CHROMATONS];
        let mut xstep = [0; MAX_CHROMATONS];
        let mut ystep = [0; MAX_CHROMATONS];
        let mut hstep = 8;
        let mut vstep = 8;
        for i in 0..num_components {
            let hs = (self.subsamp[i] >> 4)  as usize;
            let vs = (self.subsamp[i] & 0xF) as usize;
            hstep = hstep.max(hs * 8);
            vstep = vstep.max(vs * 8);
            nblks[i] = hs * vs;
            xstep[i] = hs * 8;
            ystep[i] = vs * 8;
        }

        let mut blocks;
        let height = if !self.ilaced { self.height } else { self.height / 2 };
        let mut stride = frm.stride;
        if self.ilaced {
            for (strd, off) in stride.iter_mut().zip(offs.iter_mut()) {
                if field2 {
                    *off += *strd;
                }
                *strd <<= 1;
            }
        }
        for _y in (0..height).step_by(vstep) {
            for x in 0..(self.width + hstep - 1) / hstep {
                for i in 0..num_components {
                    blocks = [[0; 64]; 4];
                    for blk in blocks[..nblks[i]].iter_mut() {
                        read_block(&mut br, blk, dc_cbs[i], ac_cbs[i], ss, se, qmats[i])?;
                        if i == 0 {
                            blk[0] += last_dc[i];
                            last_dc[i] = blk[0];
                        } else {
                            blk[0] += last_dc[1];
                            last_dc[1] = blk[0];
                        }
                        idct(blk);
                    }
                    match self.subsamp[i] {
                        0x11 => {
                            put_block(&blocks[0], &mut frm.data[offs[i] + x * 8..], stride[i]);
                        },
                        0x21 => {
                            put_block(&blocks[0], &mut frm.data[offs[i] + x * 16..], stride[i]);
                            put_block(&blocks[1], &mut frm.data[offs[i] + x * 16 + 8..], stride[i]);
                        },
                        0x12 => {
                            put_block(&blocks[0], &mut frm.data[offs[i] + x * 8..], stride[i]);
                            put_block(&blocks[1], &mut frm.data[offs[i] + x * 8 + stride[i] * 8..], stride[i]);
                        },
                        0x22 => {
                            for j in 0..4 {
                                put_block(&blocks[j], &mut frm.data[offs[i] + x * 16 + (j & 1) * 8 + (j >> 1) * 8 * stride[i]..], stride[i]);
                            }
                        },
                        _ => unreachable!(),
                    };
                }
            }
            for i in 0..num_components {
                offs[i] += stride[i] * ystep[i];
            }
        }

        Ok((br.tell() + 7) / 8)
    }
}

struct HuffDescReader<'a> {
    codes:  &'a [u16],
    bits:   &'a [u8],
    syms:   &'a [u8],
}

impl<'a> CodebookDescReader<u8> for HuffDescReader<'a> {
    fn bits(&mut self, idx: usize) -> u8 { self.bits[idx] }
    fn code(&mut self, idx: usize) -> u32 { u32::from(self.codes[idx]) }
    fn sym (&mut self, idx: usize) -> u8 { self.syms[idx] }
    fn len(&mut self) -> usize { self.syms.len() }
}

fn generate_cb(lens: &[u8; 16], syms: &[u8]) -> DecoderResult<Codebook<u8>> {
    let mut codes = [0; 256];
    let mut bits = [0; 256];

    let mut iter = bits.iter_mut();
    for (i, &len) in lens.iter().enumerate() {
        for _ in 0..len {
            *iter.next().unwrap() = (i + 1) as u8;
        }
    }
    let mut code = 0;
    let mut si = bits[0];
    let mut idx = 0;
    while idx < syms.len() {
        while idx < syms.len() && bits[idx] == si {
            codes[idx] = code;
            code += 1;
            idx  += 1;
        }
        while idx < syms.len() && bits[idx] != si {
            code <<= 1;
            si += 1;
        }
    }

    let mut cbr = HuffDescReader { codes: &codes, bits: &bits, syms };
    Ok(Codebook::new(&mut cbr, CodebookMode::MSB)?)
}

fn build_default_cb(dc: bool, idx: usize) -> DecoderResult<Codebook<u8>> {
    if dc {
        generate_cb(&DC_LENS[idx], &DC_SYMS)
    } else {
        generate_cb(&AC_LENS[idx], AC_SYMS[idx])
    }
}

impl NADecoder for RadiusStudioDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            self.width = w;
            self.height = h;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, YUV420_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        if src.len() < 2048 { return Err(DecoderError::ShortData); }
        let mut br = MemoryReader::new_read(&src);
        let size_v0             = br.read_u32be()? as usize;
        let field2_size         = br.read_u32be()? as usize;
        if field2_size != 0 {
            self.ilaced = true;
        }
        let version             = br.read_u16be()?;
        validate!(version <= 2);
        let hdr_size = if version == 0 { 16 } else { 32 };
                                  br.read_skip(1)?;
        let quant               = br.read_byte()?;
                                  br.read_skip(8)?;
        let size_v1             = br.read_u32be()? as usize;

        let field0_size = if version == 0 { size_v0 } else { size_v1 };

        let formaton = NAPixelFormaton {
                model:      ColorModel::YUV(YUVSubmodel::YUVJ),
                components: 3,
                comp_info:  [
                    Some(NAPixelChromaton {
                        h_ss: 0, v_ss: 0,
                        packed:     false,
                        depth:      8,
                        shift:      0,
                        comp_offs:  0,
                        next_elem:  1,
                    }),
                    Some(NAPixelChromaton {
                        h_ss: 1, v_ss: 0,
                        packed:     false,
                        depth:      8,
                        shift:      0,
                        comp_offs:  1,
                        next_elem:  1,
                    }),
                    Some(NAPixelChromaton {
                        h_ss: 1, v_ss: 0,
                        packed:     false,
                        depth:      8,
                        shift:      0,
                        comp_offs:  2,
                        next_elem:  1,
                    }),
                    None,
                    None
                ],
                elem_size:  0,
                be:         false,
                alpha:      false,
                palette:    false,
            };
        let vinfo = NAVideoInfo::new(self.width, self.height, false, formaton);
        let bufinfo = alloc_video_buffer(vinfo, 4)?;

        for (dc_id, dc_cb) in self.codebook[0].iter_mut().enumerate().take(2) {
            if dc_cb.is_none() {
                validate!(dc_id < 2);
                *dc_cb = Some(build_default_cb(true, dc_id)?);
            }
        }
        for (ac_id, ac_cb) in self.codebook[1].iter_mut().enumerate().take(2) {
            if ac_cb.is_none() {
                validate!(ac_id < 2);
                *ac_cb = Some(build_default_cb(false, ac_id)?);
            }
        }

        const DEF_LUMA_QUANT: [u8; 64] = [
             16,  11,  12,  14,  12,  10,  16,  14,
             13,  14,  18,  17,  16,  19,  24,  40,
             26,  24,  22,  22,  24,  49,  35,  37,
             29,  40,  58,  51,  61,  60,  57,  51,
             56,  55,  64,  72,  92,  78,  64,  68,
             87,  69,  55,  56,  80, 109,  81,  87,
             95,  98, 103, 104, 103,  62,  77, 113,
            121, 112, 100, 120,  92, 101, 103,  99
        ];
        const DEF_CHROMA_QUANT: [u8; 64] = [
            17, 18, 18, 24, 21, 24, 47, 26,
            26, 47, 99, 66, 56, 66, 99, 99,
            99, 99, 99, 99, 99, 99, 99, 99,
            99, 99, 99, 99, 99, 99, 99, 99,
            99, 99, 99, 99, 99, 99, 99, 99,
            99, 99, 99, 99, 99, 99, 99, 99,
            99, 99, 99, 99, 99, 99, 99, 99,
            99, 99, 99, 99, 99, 99, 99, 99
        ];

        for (i, (&luma_q, &chroma_q)) in DEF_LUMA_QUANT.iter().zip(DEF_CHROMA_QUANT.iter()).enumerate() {
            self.quant[0][i] = ((i16::from(quant) * i16::from(luma_q) + 25) / 50).clamp(1, 255);
            self.quant[1][i] = ((i16::from(quant) * i16::from(chroma_q) + 25) / 50).clamp(1, 255);
        }

        if let Some(buf) = bufinfo.get_vbuf() {
            let ci = [
                ComponentInfo { component_id: 1, dc_table_id: 0, ac_table_id: 0 },
                ComponentInfo { component_id: 2, dc_table_id: 1, ac_table_id: 1 },
                ComponentInfo { component_id: 3, dc_table_id: 1, ac_table_id: 1 },
            ];
            self.decode_scan(&src[hdr_size..], buf.clone(), &ci, 0, 63, false)?;
            if self.ilaced {
                let field2_off = hdr_size + ((field0_size + 0x7FF) & !0x7FF);
                validate!(field2_off < src.len());
                self.decode_scan(&src[field2_off..], buf, &ci, 0, 63, true)?;
            }
        } else { unreachable!(); }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for RadiusStudioDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(RadiusStudioDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::misc_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_pgvv() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);
        // sample: https://samples.mplayerhq.hu/V-codecs/PGVV-RadiusStudio/airfone.mov
        test_decoding("mov", "pgvv", "assets/Misc/airfone.mov", Some(6), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x7fe38055, 0x55fdd28b, 0x5af93d05, 0x3950e603],
                            [0x45968583, 0x8a9a8cfb, 0x7f6039ab, 0x43cd0df4],
                            [0x3acba878, 0x87792a3b, 0x6a7dab48, 0x91b931d7],
                            [0x68b4b35e, 0x8a918647, 0x9d997c5d, 0x4ec1bc34]]));
    }
}

const DC_LENS: [[u8; 16]; 2] = [
    [ 0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0 ],
    [ 0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0 ]
];
const DC_SYMS: [u8; 12] = [ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11 ];
const AC_LENS: [[u8; 16]; 2] = [
    [ 0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125 ],
    [ 0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119 ]
];
const AC_SYMS: [&[u8]; 2] = [
  &[
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12,
    0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
    0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
    0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
    0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98,
    0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
    0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
    0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa
  ],
  &[
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21,
    0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
    0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
    0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
    0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
    0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
    0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
    0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
    0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa
  ]
];
