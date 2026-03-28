use nihav_core::codecs::*;
use nihav_core::io::bitreader::*;
use nihav_core::io::byteio::*;
use std::cmp::Ordering;

const START_BITS: u8 = 9;
const MAX_BITS:   u8 = 16;
const DICT_SIZE: usize = 1 << MAX_BITS;
const START_POS:   usize = 257;
const INVALID_POS: usize = 65536;

struct LZWState {
    dict_sym:   [u8; DICT_SIZE],
    dict_prev:  [u16; DICT_SIZE],
    dict_pos:   usize,
    dict_lim:   usize,
    idx_bits:   u8,
}

impl LZWState {
    fn new() -> Self {
        Self {
            dict_sym:   [0; DICT_SIZE],
            dict_prev:  [0; DICT_SIZE],
            dict_pos:   START_POS,
            dict_lim:   1 << START_BITS,
            idx_bits:   START_BITS,
        }
    }
    fn reset(&mut self) {
        self.dict_pos = START_POS;
        self.dict_lim = 1 << START_BITS;
        self.idx_bits = START_BITS;
    }
    fn add(&mut self, prev: usize, sym: u8) {
        if self.dict_pos < self.dict_lim {
            self.dict_sym [self.dict_pos] = sym;
            self.dict_prev[self.dict_pos] = prev as u16;
            self.dict_pos += 1;
        }
    }
    fn decode_idx(&self, dst: &mut [u8], pos: usize, idx: usize) -> DecoderResult<usize> {
        let mut tot_len = 1;
        let mut tidx = idx;
        while tidx > 256 {
            tidx = self.dict_prev[tidx] as usize;
            tot_len += 1;
        }
        validate!(pos + tot_len <= dst.len());

        let mut end = pos + tot_len - 1;
        let mut tidx = idx;
        while tidx > 256 {
            dst[end] = self.dict_sym[tidx];
            end -= 1;
            tidx = self.dict_prev[tidx] as usize;
        }
        dst[end] = tidx as u8;

        Ok(tot_len)
    }
    fn decode(&mut self, br: &mut BitReader, dst: &mut [u8]) -> DecoderResult<()> {
        self.reset();

        let mut pos = 0;
        let mut lastidx = INVALID_POS;
        br.skip(9)?;
        loop {
            let idx         = br.read(self.idx_bits)? as usize;
            if idx == 256 {
                break;
            }
            match idx.cmp(&self.dict_pos) {
                Ordering::Less => {
                    let len = self.decode_idx(dst, pos, idx)?;
                    if lastidx != INVALID_POS {
                        self.add(lastidx, dst[pos]);
                    }
                    pos += len;
                },
                Ordering::Equal => {
                    validate!(lastidx != INVALID_POS);
                    let len = self.decode_idx(dst, pos, lastidx)?;
                    let lastsym = dst[pos];
                    pos += len;
                    validate!(pos < dst.len());
                    dst[pos] = lastsym;
                    pos += 1;
                    self.add(lastidx, lastsym);
                },
                Ordering::Greater => return Err(DecoderError::InvalidData),
            }
            lastidx = idx;
            if self.dict_pos == self.dict_lim && self.idx_bits < MAX_BITS {
                self.dict_lim <<= 1;
                self.idx_bits += 1;
            }
        }
        validate!(pos == dst.len());
        Ok(())
    }
}

fn decode_rle(src: &[u8], dst: &mut [u8]) -> DecoderResult<()> {
    let mut br = MemoryReader::new_read(src);
    let mut pos = 0;
    while pos < dst.len() {
        let mut len = usize::from(br.read_byte()?);
        if len == 0 {
            len = usize::from(br.read_byte()?);
            if len == 0 {
                len = br.read_u24be()? as usize;
            } else {
                len = (len << 8) | usize::from(br.read_byte()?);
            }
        }
        let pix = if (len & 1) == 0 { br.read_byte()? } else { 0 };
        len >>= 1;
        validate!(pos + len <= dst.len());
        for el in dst[pos..][..len].iter_mut() {
            *el = pix;
        }
        pos += len;
    }
    Ok(())
}

struct EuclidDecoder {
    info:           NACodecInfoRef,
    width:          usize,
    height:         usize,
    lzw:            LZWState,
    pal:            [u8; 768],
    frame:          Vec<u8>,
    mode:           u32,
    method:         u32,
    update:         bool,
    buf:            Vec<u8>,
}

impl EuclidDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            width:      0,
            height:     0,
            lzw:        LZWState::new(),
            pal:        [0; 768],
            frame:      Vec::new(),
            mode:       0,
            method:     0,
            update:     false,
            buf:        Vec::new(),
        }
    }
    fn output_frame(&self, dst: &mut [u8], stride: usize, paloff: usize) {
        match self.mode {
            12 => {
                for (dlines, sline) in dst.chunks_exact_mut(stride * 2)
                        .zip(self.frame.chunks_exact(self.width / 2)).take(self.height / 2) {
                    let (dline0, dline1) = dlines.split_at_mut(stride);
                    for (pair, &b) in dline0.chunks_exact_mut(2).zip(sline.iter()) {
                        pair[0] = b & 0xF;
                        pair[1] = b >> 4;
                    }
                    dline1.copy_from_slice(dline0);
                }
            },
            15 | 40 => {
                for (dlines, sline) in dst.chunks_exact_mut(stride * 2)
                        .zip(self.frame.chunks_exact(self.width)).take(self.height / 2) {
                    let (dline0, dline1) = dlines.split_at_mut(stride);
                    dline0[..sline.len()].copy_from_slice(sline);
                    dline1[..sline.len()].copy_from_slice(sline);
                }
            },
            27 => {
                for (dline, sline) in dst.chunks_exact_mut(stride)
                        .zip(self.frame.chunks_exact(self.width / 2)).take(self.height) {
                    for (pair, &b) in dline.chunks_exact_mut(2).zip(sline.iter()) {
                        pair[0] = b & 0xF;
                        pair[1] = b >> 4;
                    }
                }
            },
            _ => {
                for (dline, sline) in dst.chunks_exact_mut(stride)
                        .zip(self.frame.chunks_exact(self.width)).take(self.height) {
                    dline[..sline.len()].copy_from_slice(sline);
                }
            },
        }

        dst[paloff..][..768].copy_from_slice(&self.pal);
    }
}

impl NADecoder for EuclidDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.get_width();
            self.height = vinfo.get_height();
            if let Some(edata) = info.get_extradata() {
                validate!(edata.len() >= 0x34);
                let hdr_size = read_u32le(&edata)? as usize;
                validate!(hdr_size >= 0x34 && hdr_size <= edata.len());
                self.mode = read_u32le(&edata[0x10..])?;
                self.method = read_u32le(&edata[0x14..])?;
                if self.method > 2 {
                    println!("Unknown Euclid compression method {}", self.method);
                    return Err(DecoderError::NotImplemented);
                }
                self.update = (read_u32le(&edata[0x18..])? & 1) != 0;

                if edata.len() > hdr_size {
                    let src_pal = &edata[hdr_size..];
                    let copy_size = src_pal.len().min(self.pal.len());
                    self.pal[..copy_size].copy_from_slice(&src_pal[..copy_size]);
                } else if matches!(self.mode, 12 | 27) {
                    self.pal[..DEFAULT_PAL.len()].copy_from_slice(&DEFAULT_PAL);
                } else {
                    for (i, clr) in self.pal.chunks_exact_mut(3).enumerate() {
                        clr[0] = i as u8;
                        clr[1] = i as u8;
                        clr[2] = i as u8;
                    }
                }
            } else {
                return Err(DecoderError::InvalidData);
            }
            let frm_size = match self.mode {
                    12 => {
                        validate!((self.width & 1) == 0);
                        validate!((self.height & 1) == 0);
                        (self.width / 2) * (self.height / 2)
                    },
                    15 | 40 => {
                        validate!((self.height & 1) == 0);
                        self.width * (self.height / 2)
                    },
                    27 => {
                        validate!((self.width & 1) == 0);
                        (self.width / 2) * self.height
                    },
                    28 => {
                        self.width * self.height
                    },
                    _ => return Err(DecoderError::NotImplemented),
                };
            self.frame = vec![0; frm_size];
            self.buf = vec![0; frm_size];

            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 1);

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;

        let mut buf = bufinfo.get_vbuf().unwrap();
        let paloff = buf.get_offset(1);
        let stride = buf.get_stride(0);
        let data = buf.get_data_mut().unwrap();

        let dst = if !self.update { &mut self.frame } else { &mut self.buf };
        match self.method {
            0 => decode_rle(&src, dst)?,
            1 => {
                let mut br = BitReader::new(&src, BitReaderMode::LE);
                self.lzw.decode(&mut br, dst)?;
            },
            2 => {
                let dlen = dst.len();
                validate!(dlen <= src.len());
                dst.copy_from_slice(&src[..dlen]);
            },
            _ => unreachable!(),
        }
        if self.update {
            for (pix, &b) in self.frame.iter_mut().zip(self.buf.iter()) {
                *pix ^= b;
            }
        }

        self.output_frame(data, stride, paloff);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(self.update);
        frm.set_frame_type(if !self.update { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {}
}

impl NAOptionHandler for EuclidDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(EuclidDecoder::new())
}

#[derive(Default)]
struct EuclidPacketiser {
    stream:     Option<NAStreamRef>,
    buf:        Vec<u8>,
    frameno:    u32,
}

impl EuclidPacketiser {
    fn new() -> Self { Self::default() }
}

impl NAPacketiser for EuclidPacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
        self.stream = Some(stream);
    }
    fn add_data(&mut self, src: &[u8]) -> bool {
        self.buf.extend_from_slice(src);
        false
    }
    fn parse_stream(&mut self, id: u32) -> DecoderResult<NAStreamRef> {
        if let Some(ref stream) = self.stream {
            let mut stream = NAStream::clone(stream);
            stream.id = id;
            Ok(stream.into_ref())
        } else {
            Err(DecoderError::MissingReference)
        }
    }
    fn skip_junk(&mut self) -> DecoderResult<usize> {
        Err(DecoderError::NotImplemented)
    }
    fn get_packet(&mut self, stream: NAStreamRef) -> DecoderResult<Option<NAPacket>> {
        if !self.buf.is_empty() {
            let mut data = Vec::new();
            std::mem::swap(&mut data, &mut self.buf);
            let ts = NATimeInfo::new(Some(u64::from(self.frameno)), None, None, stream.tb_num, stream.tb_den);
            self.frameno += 1;
            Ok(Some(NAPacket::new(stream, ts, false, data)))
        } else {
            Ok(None)
        }
    }
    fn reset(&mut self) {
        self.buf.clear();
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

pub fn get_packetiser() -> Box<dyn NAPacketiser + Send> {
    Box::new(EuclidPacketiser::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;

    #[test]
    fn test_euclid() {
        let mut dmx_reg = RegisteredDemuxers::new();
        acorn_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from RISC DISC 3
        test_decoding("tca", "euclid", "assets/Acorn/JUMPER", Some(1),
                          &dmx_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x5ce22d08, 0x5f3cbd1b, 0x61a8b85e, 0x58197ba6],
                                [0x1fc3cc2e, 0xdc107b42, 0xada9f7b5, 0x0882f3d6]]));
    }
    #[test]
    fn test_euclid_inter() {
        let mut dmx_reg = RegisteredDemuxers::new();
        acorn_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from RISC DISC 3
        test_decoding("tca", "euclid", "assets/Acorn/WIGGLFACE", Some(1),
                          &dmx_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0xd1362b20, 0x986be0ec, 0x59aae46e, 0x11123e70],
                                [0xc07ab19c, 0xc518aca6, 0xc1f632b3, 0x9baa9cb5]]));
    }
}

#[allow(dead_code)]
const DEFAULT_PAL: [u8; 16 * 3] = [
    0xFF, 0xFF, 0xFF,
    0xDD, 0xDD, 0xDD,
    0xBB, 0xBB, 0xBB,
    0x99, 0x99, 0x99,
    0x77, 0x77, 0x77,
    0x55, 0x55, 0x55,
    0x33, 0x33, 0x33,
    0x00, 0x00, 0x00,
    0x00, 0x44, 0x99,
    0xEE, 0xEE, 0x00,
    0x00, 0xCC, 0x00,
    0xDD, 0x00, 0x00,
    0xEE, 0xEE, 0xBB,
    0x55, 0x88, 0x00,
    0xFF, 0xBB, 0x00,
    0x00, 0xBB, 0xFF
];
