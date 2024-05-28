use nihav_core::codecs::*;
use nihav_core::io::bitreader::*;

use super::RGB555_FORMAT;
use super::yuvtab::YUV2RGB;

fn get_mv(br: &mut BitReader, is_4x4: bool) -> DecoderResult<((i8, i8), bool)> {
    match br.read(2)? {
        0b00 => Ok(((0, 0), false)),
        0b10 => Ok((MV_TAB1[br.read(3)? as usize], false)),
        0b01 => Ok((MV_TAB2[br.read(4)? as usize], false)),
        _ => {
            let idx = br.read(6)? as usize;
            if idx < MV_TAB3.len() {
                Ok((MV_TAB3[idx], false))
            } else {
                let idx = idx - MV_TAB3.len();
                if is_4x4 {
                    Ok((MV_TAB_SELF_4X4[idx], true))
                } else {
                    Ok((MV_TAB_SELF_2X2[idx], true))
                }
            }
        },
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
            if let Some(edata) = info.get_extradata() {
                for triplet in edata.windows(3) {
                    if triplet == b"YUV" {
                        self.is_yuv = true;
                        break;
                    }
                }
            }
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
        for _y in (0..self.height).step_by(4) {
            for x in (0..self.width).step_by(4) {
                if br.read_bool()? { // raw block
                    let mut luma = [0; 16];
                    for el in luma.iter_mut() {
                        *el = br.read(5)? as u16;
                    }
                    let uv = (br.read(10)? as u16) << 5;
                    for (drow, yrow) in self.cur_frm[dpos + x..].chunks_mut(self.width)
                                .zip(luma.chunks_exact(4)) {
                        for (dst, &src_y) in drow.iter_mut().zip(yrow.iter()) {
                            *dst = src_y | uv;
                        }
                    }
                } else if br.read_bool()? { // split block
                    let offsets = [dpos + x, dpos + x + 2, dpos + x + self.width * 2, dpos + x + 2 + self.width * 2];
                    for &offset in offsets.iter() {
                        if br.read_bool()? { // raw subblock
                            let mut luma = [0; 4];
                            for el in luma.iter_mut() {
                                *el = br.read(5)? as u16;
                            }
                            let uv = (br.read(10)? as u16) << 5;
                            for (drow, yrow) in self.cur_frm[offset..].chunks_mut(self.width)
                                        .zip(luma.chunks_exact(2)) {
                                for (dst, &src_y) in drow.iter_mut().zip(yrow.iter()) {
                                    *dst = src_y | uv;
                                }
                            }
                        } else { // MV subblock
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
                        }
                    }
                } else { // MV block
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
                }
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
    Subblock(u8),
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
                            0b10 => { // subblocks
                                self.skip_bits(2);
                                self.state = ParseState::Subblock(0);
                            },
                            0b00 => { // MV block
                                if let Some(ret) = self.peek_bits(4) {
                                    let mv_mode = ret >> 2;
                                    match mv_mode {
                                        0b10 => self.skip_bits(3),
                                        0b01 => self.skip_bits(4),
                                        0b11 => self.skip_bits(6),
                                        _ => {},
                                    }
                                } else {
                                    return Ok(None);
                                }
                                self.skip_bits(4); // block mode + MV mode
                                self.advance_block();
                            },
                            _ => { // raw block
                                self.skip_bits(1);
                                self.skip_bits(90); // 16 Y + UV 5-bit samples
                                self.advance_block();
                            },
                        }
                    } else {
                        return Ok(None);
                    }
                },
                ParseState::Subblock(sblk) => {
                    if let Some(mode) = self.peek_bits(1) {
                        if mode == 1 { // raw subblock
                            self.skip_bits(1);
                            self.skip_bits(30); // 4 Y + UV 5-bit samples
                        } else { // MV subblock
                            if let Some(ret) = self.peek_bits(3) {
                                let mv_mode = ret >> 1;
                                match mv_mode {
                                    0b10 => self.skip_bits(3),
                                    0b01 => self.skip_bits(4),
                                    0b11 => self.skip_bits(6),
                                    _ => {},
                                }
                            } else {
                                return Ok(None);
                            }
                            self.skip_bits(3); // block mode + MV mode
                        }
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
    fn test_movingblocks() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Cine Clips by Oregan Software Developments
        test_decoding_raw("armovie", "movingblocks", "assets/Acorn/CANWHARF", Some(3),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x451ec0a2, 0xfeab8603, 0x5cc28d22, 0x8d8bfff4],
                                [0x4d2c291a, 0x30df0600, 0xef1a4085, 0x84edcf0d],
                                [0x70a76ed9, 0x346288a1, 0x263f2256, 0xd6106242],
                                [0x640c31c6, 0x3260309b, 0x0b0bf52b, 0x2d70d2df]]));
    }
}

const MV_TAB1: [(i8, i8); 8] = [
    (-1, -1), (0, -1), (1, -1),
    (-1,  0), (1,  0),
    (-1,  1), (0,  1), (1,  1)
];

const MV_TAB2: [(i8, i8); 16] = [
    (-2, -2), (-1, -2), (0, -2), (1, -2), (2, -2),
    (-2, -1), ( 2, -1),
    (-2,  0), ( 2,  0),
    (-2,  1), ( 2,  1),
    (-2,  2), (-1,  2), (0,  2), (1,  2), (2,  2)
];

const MV_TAB3: [(i8, i8); 56] = [
    (-4, -4), (-3, -4), (-2, -4), (-1, -4), (0, -4), (1, -4), (2, -4), (3, -4), (4, -4),
    (-4, -3), ( 4, -3),
    (-4, -2), ( 4, -2),
    (-4, -1), ( 4, -1),
    (-4,  0), ( 4,  0),
    (-4,  1), ( 4,  1),
    (-4,  2), ( 4,  2),
    (-4,  3), ( 4,  3),
    (-4,  4), (-3,  4), (-2,  4), (-1,  4), (0,  4), (1,  4), (2,  4), (3,  4), (4,  4),
    (-3, -3), (-2, -3), (-1, -3), ( 0, -3), (1, -3), (2, -3), (3, -3),
    (-3, -2), ( 3, -2),
    (-3, -1), ( 3, -1),
    (-3,  0), ( 3,  0),
    (-3,  1), ( 3,  1),
    (-3,  2), ( 3,  2),
    (-3,  3), (-2,  3), (-1,  3), ( 0,  3), (1,  3), (2,  3), (3,  3)
];

const MV_TAB_SELF_2X2: [(i8, i8); 8] = [
    (-2, -2), (-1, -2), ( 0, -2), (1, -2), (2, -2),
    (-2, -1), (-2,  0), (-3,  0),
];
const MV_TAB_SELF_4X4: [(i8, i8); 8] = [
    (-2, -4), (-1, -4), ( 0, -4), (1, -4), (2, -4),
    (-4,  0), (-4, -1), (-4, -2)
];
