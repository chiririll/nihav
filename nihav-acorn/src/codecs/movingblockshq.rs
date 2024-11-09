use nihav_core::codecs::*;
use nihav_core::io::bitreader::*;
use nihav_core::io::codebook::*;

use super::RGB555_FORMAT;
use super::yuvtab::YUV2RGB;

struct DeltaCodebook {
    cb: Codebook<u16>,
}

impl Default for DeltaCodebook {
    fn default() -> Self {
        let mut cr = TableCodebookDescReader::new(&LUMA_CODES, &LUMA_BITS, |idx| idx as u16);
        let cb = Codebook::new(&mut cr, CodebookMode::LSB).unwrap();
        Self { cb }
    }
}

fn get_mv(br: &mut BitReader, is_4x4: bool) -> DecoderResult<((i8, i8), bool)> {
    match br.read(2)? {
        0b00 => Ok((MV_TAB1[br.read(3)? as usize], false)),
        0b10 => Ok((MV_TAB2[br.read(4)? as usize], false)),
        0b01 => {
            let idx = br.read(5)? as usize;
            let self_tab = if is_4x4 { &MV_TAB_SELF_4X4 } else { &MV_TAB_SELF_2X2 };
            if idx < self_tab.len() {
                Ok((self_tab[idx], true))
            } else {
                Ok((MV_TAB3[idx - self_tab.len()], false))
            }
        },
        0b11 => {
            let idx = br.read(8)? as usize;
            validate!(idx < MV_TAB8.len());
            Ok((MV_TAB8[idx], false))
        },
        _ => unreachable!(),
    }
}

#[derive(Default)]
struct MBDecoder {
    info:           NACodecInfoRef,
    cur_frm:        Vec<u16>,
    prev_frm:       Vec<u16>,
    width:          usize,
    height:         usize,
    is_yuv:         bool,
    cb:             DeltaCodebook,
}

impl MBDecoder {
    fn new() -> Self { Self::default() }
}

impl NADecoder for MBDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, RGB555_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            self.cur_frm  = vec![0; vinfo.get_width() * vinfo.get_height()];
            self.prev_frm = vec![0; vinfo.get_width() * vinfo.get_height()];
            self.width  = vinfo.get_width();
            self.height = vinfo.get_height();
            validate!((self.width  & 3) == 0);
            validate!((self.height & 3) == 0);
            self.is_yuv = true;
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 2);
        let mut br = BitReader::new(&src, BitReaderMode::LE);

        let mut is_intra = true;
        let mut dpos = 0;
        let mut avg_y = 0;
        for _y in (0..self.height).step_by(4) {
            for x in (0..self.width).step_by(4) {
                match br.read(2)? {
                    0b00 => { // skip
                        for (dline, sline) in self.cur_frm[dpos + x..].chunks_mut(self.width)
                                    .zip(self.prev_frm[dpos + x..].chunks(self.width)).take(4) {
                            dline[..4].copy_from_slice(&sline[..4]);
                        }
                    },
                    0b10 => { // MV
                        let ((dx, dy), copy_cur) = get_mv(&mut br, true)?;
                        let src_pos = ((dpos + x) as isize) + (dx as isize) + (dy as isize) * (self.width as isize);
                        validate!(src_pos >= 0);
                        let src_pos = src_pos as usize;
                        validate!(src_pos + 4 + self.width * 3 <= self.cur_frm.len());
                        if !copy_cur {
                            let src = &self.prev_frm[src_pos..];
                            for (drow, srow) in self.cur_frm[dpos + x..].chunks_mut(self.width)
                                        .zip(src.chunks(self.width)).take(4) {
                                drow[..4].copy_from_slice(&srow[..4]);
                            }
                            is_intra = false;
                        } else {
                            let mut ooff = dpos + x;
                            let mut soff = src_pos;
                            for _ in 0..4 {
                                for i in 0..4 {
                                    self.cur_frm[ooff + i] = self.cur_frm[soff + i];
                                }
                                ooff += self.width;
                                soff += self.width;
                            }
                        }
                    },
                    0b01 => { // raw
                        let uv = (br.read(10)? as u16) << 5;

                        let mut luma = [0; 16];
                        for el in luma.iter_mut() {
                            *el = br.read_cb(&self.cb.cb)?;
                        }
                        let mut luma_sum = 0;
                        for row in (0..16).step_by(4) {
                            for col in 0..4 {
                                let pred_val = match (col, row) {
                                        (0, 0) => avg_y,
                                        (0, _) => luma[col + row - 4],
                                        (_, 0) => luma[col - 1],
                                        _      => (luma[col + row - 1] + luma[col + row - 4]) >> 1,
                                    };
                                luma[col + row] = (luma[col + row] + pred_val) & 0x1F;
                                luma_sum += luma[col + row];
                            }
                        }
                        avg_y = luma_sum >> 4;

                        for (drow, yrow) in self.cur_frm[dpos + x..].chunks_mut(self.width)
                                    .zip(luma.chunks_exact(4)) {
                            for (dst, &src_y) in drow.iter_mut().zip(yrow.iter()) {
                                *dst = src_y | uv;
                            }
                        }
                    },
                    _ => { // subdivision
                        let offsets = [dpos + x, dpos + x + 2, dpos + x + self.width * 2, dpos + x + 2 + self.width * 2];
                        for &offset in offsets.iter() {
                            if br.read_bool()? { // MV
                                let ((dx, dy), copy_cur) = get_mv(&mut br, false)?;
                                let src_pos = (offset as isize) + (dx as isize) + (dy as isize) * (self.width as isize);
                                validate!(src_pos >= 0);
                                let src_pos = src_pos as usize;
                                validate!(src_pos + 2 + self.width <= self.cur_frm.len());
                                if !copy_cur {
                                    let src = &self.prev_frm[src_pos..];
                                    for (drow, srow) in self.cur_frm[offset..].chunks_mut(self.width)
                                                .zip(src.chunks(self.width)).take(2) {
                                        drow[..2].copy_from_slice(&srow[..2]);
                                    }
                                    is_intra = false;
                                } else {
                                    let mut ooff = offset;
                                    let mut soff = src_pos;
                                    for _ in 0..2 {
                                        for i in 0..2 {
                                            self.cur_frm[ooff + i] = self.cur_frm[soff + i];
                                        }
                                        ooff += self.width;
                                        soff += self.width;
                                    }
                                }
                            } else if br.read_bool()? { // raw
                                let uv = (br.read(10)? as u16) << 5;

                                let mut luma = [0; 4];
                                for el in luma.iter_mut() {
                                    *el = br.read_cb(&self.cb.cb)?;
                                }
                                luma[0] = (luma[0] + avg_y) & 0x1F;
                                luma[1] = (luma[1] + luma[0]) & 0x1F;
                                luma[2] = (luma[2] + luma[0]) & 0x1F;
                                luma[3] = (luma[3] + ((luma[1] + luma[2]) >> 1)) & 0x1F;
                                avg_y = luma.iter().sum::<u16>() >> 2;

                                self.cur_frm[offset]                  = luma[0] | uv;
                                self.cur_frm[offset              + 1] = luma[1] | uv;
                                self.cur_frm[offset + self.width]     = luma[2] | uv;
                                self.cur_frm[offset + self.width + 1] = luma[3] | uv;
                            } else { // skip
                                for (dline, sline) in self.cur_frm[offset..].chunks_mut(self.width)
                                            .zip(self.prev_frm[offset..].chunks(self.width)).take(2) {
                                    dline[..2].copy_from_slice(&sline[..2]);
                                }
                            }
                        }
                    },
                };
            }
            dpos += self.width * 4;
        }

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;
        let mut buf = bufinfo.get_vbuf16().unwrap();
        let stride = buf.get_stride(0);
        let data = buf.get_data_mut().unwrap();

        for (dline, sline) in data.chunks_exact_mut(stride)
                    .zip(self.cur_frm.chunks_exact(self.width)) {
            dline[..self.width].copy_from_slice(sline);
        }
        if self.is_yuv {
            for el in data.iter_mut() {
                *el = YUV2RGB[(*el as usize) & 0x7FFF];
            }
        }

        std::mem::swap(&mut self.cur_frm, &mut self.prev_frm);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        for el in self.cur_frm.iter_mut() {
            *el = 0;
        }
        for el in self.prev_frm.iter_mut() {
            *el = 0;
        }
    }
}

impl NAOptionHandler for MBDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(MBDecoder::new())
}

#[derive(Default,Debug,PartialEq)]
enum ParseState {
    #[default]
    Start,
    BlockMode,
    Raw(u8),
    Subblock(u8),
    SubblockRaw(u8, u8),
}

#[derive(Default)]
struct MBPacketiser {
    stream:     Option<NAStreamRef>,
    buf:        Vec<u8>,
    frameno:    u32,
    intra:      bool,
    bitpos:     usize,
    x:          usize,
    y:          usize,
    state:      ParseState,
    width:      usize,
    height:     usize,
    csizes:     Vec<usize>,
}

impl MBPacketiser {
    fn new() -> Self { Self::default() }
    fn peek_bits(&mut self, nbits: u8) -> Option<u8> {
        if self.bitpos + usize::from(nbits) <= self.buf.len() * 8 {
            let tail = (self.bitpos as u8) & 7;
            let mask = 0xFF >> (8 - nbits);
            let cw = if tail + nbits <= 8 {
                    u16::from(self.buf[self.bitpos >> 3])
                } else {
                    let b0 = self.buf[self.bitpos >> 3];
                    let b1 = self.buf[(self.bitpos >> 3) + 1];
                    u16::from(b0) + u16::from(b1) * 256
                };
            Some(((cw >> tail) as u8) & mask)
        } else {
            None
        }
    }
    fn peek_code(&mut self) -> Option<u8> {
        let mut cur_code = 0;
        let mut avail_bits = 0;
        let tail = self.bitpos & 7;
        while (avail_bits < (9 + tail)) && (self.bitpos + avail_bits + 8 <= self.buf.len() * 8) {
            cur_code |= u32::from(self.buf[(self.bitpos + avail_bits) >> 3]) << avail_bits;
            avail_bits += 8;
        }
        if avail_bits <= tail {
            return None;
        }
        let cur_code = (cur_code >> tail) as u16;
        let avail_bits = (avail_bits - tail) as u8;
        for (&code, &len) in LUMA_CODES.iter().zip(LUMA_BITS.iter()) {
            if len <= avail_bits && (cur_code & ((1 << len) - 1)) == code {
                return Some(len);
            }
        }
        None
    }
    fn skip_bits(&mut self, nbits: u8) {
        self.bitpos += usize::from(nbits);
    }
    fn advance_block(&mut self) {
        self.x += 4;
        if self.x == self.width {
            self.x = 0;
            self.y += 4;
        }
    }
}

impl NAPacketiser for MBPacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
        let vinfo = stream.get_info().get_properties().get_video_info().unwrap();
        self.width  = vinfo.width;
        self.height = vinfo.height;
        self.stream = Some(stream);
    }
    fn add_data(&mut self, src: &[u8]) -> bool {
        self.csizes.push(src.len());
        self.buf.extend_from_slice(src);
        self.buf.len() < (1 << 10)
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
        if self.buf.len() * 8 < self.bitpos {
            return Ok(None);
        }

        if self.state == ParseState::Start {
            self.intra = true;
            self.x = 0;
            self.y = 0;
            self.state = ParseState::BlockMode;
            self.bitpos = 0;
        }

        while self.y < self.height {
            match self.state {
                ParseState::Start => unreachable!(),
                ParseState::BlockMode => {
                    if let Some(mode) = self.peek_bits(2) {
                        match mode {
                            0b00 => { // skip
                                self.skip_bits(2);
                                self.intra = false;
                                self.advance_block();
                            },
                            0b10 => { // MV block
                                if let Some(ret) = self.peek_bits(4) {
                                    let mv_mode = ret >> 2;
                                    match mv_mode {
                                        0b00 => self.skip_bits(3),
                                        0b10 => self.skip_bits(4),
                                        0b01 => self.skip_bits(5),
                                        _    => self.skip_bits(8),
                                    }
                                    if mv_mode != 0b01 {
                                        self.intra = false;
                                    }
                                } else {
                                    return Ok(None);
                                }
                                self.skip_bits(4); // block mode + MV mode
                                self.advance_block();
                            },
                            0b11 => { // subblocks
                                self.skip_bits(2);
                                self.state = ParseState::Subblock(0);
                            },
                            _ => { // raw block
                                self.skip_bits(2);
                                self.skip_bits(10); // UV
                                self.state = ParseState::Raw(0);
                            },
                        }
                    } else {
                        return Ok(None);
                    }
                },
                ParseState::Raw(coef) => {
                    if let Some(bits) = self.peek_code() {
                        self.skip_bits(bits);
                    } else {
                        return Ok(None);
                    }
                    self.state = if coef < 15 {
                            ParseState::Raw(coef + 1)
                        } else {
                            self.advance_block();
                            ParseState::BlockMode
                        };
                },
                ParseState::Subblock(sblk) => {
                    if let Some(mode) = self.peek_bits(2) {
                        match mode {
                            0b00 => { // skip
                                self.intra = false;
                                self.skip_bits(2); // subblock mode
                            },
                            0b10 => { // raw
                                self.skip_bits(2); // subblock mode
                                self.skip_bits(10); // UV
                                self.state = ParseState::SubblockRaw(sblk, 0);
                                continue;
                            },
                            _ => { // MV
                                if let Some(ret) = self.peek_bits(3) {
                                    let mv_mode = ret >> 1;
                                    match mv_mode {
                                        0b00 => self.skip_bits(3),
                                        0b10 => self.skip_bits(4),
                                        0b01 => self.skip_bits(5),
                                        _    => self.skip_bits(8),
                                    }
                                    if mv_mode != 0b01 {
                                        self.intra = false;
                                    }
                                    self.skip_bits(3); // subblock mode + MV mode
                                } else {
                                    return Ok(None);
                                }
                            },
                        };
                        self.state = if sblk < 3 {
                                ParseState::Subblock(sblk + 1)
                            } else {
                                self.advance_block();
                                ParseState::BlockMode
                            };
                    } else {
                        return Ok(None);
                    }
                },
                ParseState::SubblockRaw(sblk, coef) => {
                    if let Some(bits) = self.peek_code() {
                        self.skip_bits(bits);
                    } else {
                        return Ok(None);
                    }
                    self.state = if coef < 3 {
                            ParseState::SubblockRaw(sblk, coef + 1)
                        } else if sblk < 3 {
                            ParseState::Subblock(sblk + 1)
                        } else {
                            self.advance_block();
                            ParseState::BlockMode
                        };
                },
            }
        }

        let size = (self.bitpos + 7) >> 3;

        let mut data = Vec::with_capacity(size);
        data.extend_from_slice(&self.buf[..size]);
        self.buf.drain(..size);

        if !self.csizes.is_empty() {
            if self.csizes[0] >= size {
                self.csizes[0] -= size;
                // skip possible padding at the end of chunk
                if self.csizes[0] == 1 {
                    self.buf.remove(0);
                    self.csizes[0] -= 1;
                }
                if self.csizes[0] == 0 {
                    self.csizes.remove(0);
                }
            } else {
                println!("ran past input chunk end!");
                self.csizes.clear();
                self.buf.clear();
            }
        }

        let ts = NATimeInfo::new(Some(u64::from(self.frameno)), None, None, stream.tb_num, stream.tb_den);
        self.frameno += 1;

        self.state = ParseState::Start;

        Ok(Some(NAPacket::new(stream, ts, self.intra, data)))
    }
    fn reset(&mut self) {
        self.buf.clear();
        self.bitpos = 0;
        self.state = ParseState::Start;
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

pub fn get_packetiser() -> Box<dyn NAPacketiser + Send> {
    Box::new(MBPacketiser::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::{RegisteredDecoders, RegisteredPacketisers};
    use nihav_core::demuxers::RegisteredRawDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_movingblockshq() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from RISC DISC 3
        test_decoding_raw("armovie", "movingblockshq", "assets/Acorn/EXPLODE", Some(3),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0xc684fd94, 0x9e9d8c0a, 0xa37d639a, 0x12312d08],
                                [0x33c29033, 0xe2ad4765, 0xde9d5e71, 0x5db43fda],
                                [0xe3ab66e3, 0xd3303ce8, 0xe3df8b08, 0x52f6b83e],
                                [0x2403a32a, 0x047f46bf, 0xd236fa90, 0xcc1939be]]));
    }
}

const LUMA_CODES: [u16; 32] = [
    0x002, 0x007, 0x004, 0x008, 0x01D, 0x03B, 0x035, 0x05B,
    0x065, 0x070, 0x050, 0x0ED, 0x0A5, 0x0C5, 0x090, 0x19B,
    0x16D, 0x06D, 0x09B, 0x010, 0x045, 0x025, 0x01B, 0x030,
    0x005, 0x02D, 0x015, 0x00D, 0x000, 0x00B, 0x003, 0x001
];
const LUMA_BITS: [u8; 32] = [
    2, 3, 3, 4, 5, 6, 6, 7, 7, 7, 7, 8, 8, 8, 8, 9,
    9, 9, 9, 8, 8, 8, 8, 7, 7, 7, 6, 6, 5, 5, 4, 3
];

const MV_TAB1: [(i8, i8); 8] = [
    (-1, -1), (0, -1), (1, -1),
    (-1,  0),          (1,  0),
    (-1,  1), (0,  1), (1,  1)
];

const MV_TAB2: [(i8, i8); 16] = [
    (-2, -2), (-1, -2), (0, -2), (1, -2), (2, -2),
    (-2, -1),                             (2, -1),
    (-2,  0),                             (2,  0),
    (-2,  1),                             (2,  1),
    (-2,  2), (-1,  2), (0,  2), (1,  2), (2,  2)
];

const MV_TAB3: [(i8, i8); 24] = [
    (-3, -3), (-2, -3), (-1, -3), ( 0, -3), (1, -3), (2, -3), (3, -3),
    (-3, -2),                                                 (3, -2),
    (-3, -1),                                                 (3, -1),
    (-3,  0),                                                 (3,  0),
    (-3,  1),                                                 (3,  1),
    (-3,  2),                                                 (3,  2),
    (-3,  3), (-2,  3), (-1,  3), ( 0,  3), (1,  3), (2,  3), (3,  3)
];

const MV_TAB8: [(i8, i8); 240] = [
    (-8,-8), (-7,-8), (-6,-8), (-5,-8), (-4,-8), (-3,-8), (-2,-8), (-1,-8), (0,-8), (1,-8), (2,-8), (3,-8), (4,-8), (5,-8), (6,-8), (7,-8), (8,-8),
    (-8,-7), (-7,-7), (-6,-7), (-5,-7), (-4,-7), (-3,-7), (-2,-7), (-1,-7), (0,-7), (1,-7), (2,-7), (3,-7), (4,-7), (5,-7), (6,-7), (7,-7), (8,-7),
    (-8,-6), (-7,-6), (-6,-6), (-5,-6), (-4,-6), (-3,-6), (-2,-6), (-1,-6), (0,-6), (1,-6), (2,-6), (3,-6), (4,-6), (5,-6), (6,-6), (7,-6), (8,-6),
    (-8,-5), (-7,-5), (-6,-5), (-5,-5), (-4,-5), (-3,-5), (-2,-5), (-1,-5), (0,-5), (1,-5), (2,-5), (3,-5), (4,-5), (5,-5), (6,-5), (7,-5), (8,-5),
    (-8,-4), (-7,-4), (-6,-4), (-5,-4), (-4,-4), (-3,-4), (-2,-4), (-1,-4), (0,-4), (1,-4), (2,-4), (3,-4), (4,-4), (5,-4), (6,-4), (7,-4), (8,-4),
    (-8,-3), (-7,-3), (-6,-3), (-5,-3), (-4,-3),                                                            (4,-3), (5,-3), (6,-3), (7,-3), (8,-3),
    (-8,-2), (-7,-2), (-6,-2), (-5,-2), (-4,-2),                                                            (4,-2), (5,-2), (6,-2), (7,-2), (8,-2),
    (-8,-1), (-7,-1), (-6,-1), (-5,-1), (-4,-1),                                                            (4,-1), (5,-1), (6,-1), (7,-1), (8,-1),
    (-8, 0), (-7, 0), (-6, 0), (-5, 0), (-4, 0),                                                            (4, 0), (5, 0), (6, 0), (7, 0), (8, 0),
    (-8, 1), (-7, 1), (-6, 1), (-5, 1), (-4, 1),                                                            (4, 1), (5, 1), (6, 1), (7, 1), (8, 1),
    (-8, 2), (-7, 2), (-6, 2), (-5, 2), (-4, 2),                                                            (4, 2), (5, 2), (6, 2), (7, 2), (8, 2),
    (-8, 3), (-7, 3), (-6, 3), (-5, 3), (-4, 3),                                                            (4, 3), (5, 3), (6, 3), (7, 3), (8, 3),
    (-8, 4), (-7, 4), (-6, 4), (-5, 4), (-4, 4), (-3, 4), (-2, 4), (-1, 4), (0, 4), (1, 4), (2, 4), (3, 4), (4, 4), (5, 4), (6, 4), (7, 4), (8, 4),
    (-8, 5), (-7, 5), (-6, 5), (-5, 5), (-4, 5), (-3, 5), (-2, 5), (-1, 5), (0, 5), (1, 5), (2, 5), (3, 5), (4, 5), (5, 5), (6, 5), (7, 5), (8, 5),
    (-8, 6), (-7, 6), (-6, 6), (-5, 6), (-4, 6), (-3, 6), (-2, 6), (-1, 6), (0, 6), (1, 6), (2, 6), (3, 6), (4, 6), (5, 6), (6, 6), (7, 6), (8, 6),
    (-8, 7), (-7, 7), (-6, 7), (-5, 7), (-4, 7), (-3, 7), (-2, 7), (-1, 7), (0, 7), (1, 7), (2, 7), (3, 7), (4, 7), (5, 7), (6, 7), (7, 7), (8, 7),
    (-8, 8), (-7, 8), (-6, 8), (-5, 8), (-4, 8), (-3, 8), (-2, 8), (-1, 8), (0, 8), (1, 8), (2, 8), (3, 8), (4, 8), (5, 8), (6, 8), (7, 8), (8, 8)
];

const MV_TAB_SELF_4X4: [(i8, i8); 8] = [
    (-2, -4), (-1, -4), ( 0, -4), (1, -4), (2, -4),
    (-4,  0), (-4, -1), (-4, -2),
];
const MV_TAB_SELF_2X2: [(i8, i8); 8] = [
    (-2, -2), (-1, -2), ( 0, -2), (1, -2), (2, -2),
    (-2, -1), (-2,  0), (-3,  0),
];
