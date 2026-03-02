use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

use super::yuvtab::YUV2RGB;

const END_CODE: u16 = 0x7300;

#[derive(Default)]
struct MLDecoder {
    info:           NACodecInfoRef,
    cur_frm:        Vec<u16>,
    prev_frm:       Vec<u16>,
    width:          usize,
    is_yuv:         bool,
}

impl MLDecoder {
    fn new() -> Self { Self::default() }
}

impl NADecoder for MLDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, super::BGR555_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            self.cur_frm  = vec![0; vinfo.get_width() * vinfo.get_height()];
            self.prev_frm = vec![0; vinfo.get_width() * vinfo.get_height()];
            self.width = vinfo.get_width();
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
        validate!(src.len() > 2 && (src.len() & 1) == 0);
        let mut br = MemoryReader::new_read(&src);

        let mut is_intra = true;
        let mut dpos = 0;
        while dpos < self.cur_frm.len() {
            let op = br.read_u16le()?;
            let raw_flag = (op & 1) == 0;
            let val = op >> 1;
            if raw_flag {
                self.cur_frm[dpos] = val;
                dpos += 1;
            } else {
                match val {
                    0..=0x47FF => { // copy prev
                        let len = ((val & 0x3F) + 2) as usize;
                        validate!(dpos + len <= self.cur_frm.len());

                        let mut idx = (val >> 6) as isize;
                        if idx >= 144 { // skip (0,0)
                            idx += 1;
                        }
                        let dy = idx / 17 - 8;
                        let dx = idx % 17 - 8;
                        let spos = dpos as isize + dx + dy * (self.width as isize);
                        validate!(spos >= 0);
                        let mut spos = spos as usize;
                        validate!(spos + len <= self.prev_frm.len());

                        for _ in 0..len {
                            self.cur_frm[dpos] = self.prev_frm[spos];
                            dpos += 1;
                            spos += 1;
                        }

                        is_intra = false;
                    },
                    0x4800..=0x72FF => { // copy cur
                        let len = ((val & 0x3F) + 2) as usize;
                        validate!(dpos + len <= self.cur_frm.len());

                        let idx = ((val >> 6) as usize) - 0x120;
                        let dx = idx % 19;
                        let dy = 9 - (idx / 19);
                        validate!(dpos + dx >= dy * self.width + 9);
                        let mut spos = dpos + dx - 9 - dy * self.width;

                        for _ in 0..len {
                            self.cur_frm[dpos] = self.cur_frm[spos];
                            dpos += 1;
                            spos += 1;
                        }
                    },
                    END_CODE => break, // end of frame
                    0x7301..=0x77FF => { // run
                        let len = ((val & 0x3F) + 2) as usize;
                        let pix = br.read_u16le()?;
                        validate!(dpos + len <= self.cur_frm.len());
                        for _ in 0..len {
                            self.cur_frm[dpos] = pix;
                            dpos += 1;
                        }
                    },
                    0x7800..=0x7BFF => { // skip
                        let len = ((val & 0x3FF) + 1) as usize;
                        validate!(dpos + len <= self.cur_frm.len());
                        for _ in 0..len {
                            self.cur_frm[dpos] = self.prev_frm[dpos];
                            dpos += 1;
                        }
                        is_intra = false;
                    },
                    0x7C00.. => { // raw
                        let len = ((val & 0x3FF) + 1) as usize;
                        validate!(dpos + len <= self.cur_frm.len());
                        let mut bitbuf = u32::from(br.read_u16le()?);
                        let mut bits = 16;
                        for _ in 0..len {
                            if bits < 15 {
                                bitbuf |= u32::from(br.read_u16le()?) << bits;
                                bits += 16;
                            }
                            self.cur_frm[dpos] = (bitbuf & 0x7FFF) as u16;
                            bitbuf >>= 15;
                            bits -= 15;
                            dpos += 1;
                        }
                    },
                }
            }
        }
        validate!(br.left() == 2 && br.read_u16le()? == (END_CODE * 2 + 1));

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

impl NAOptionHandler for MLDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(MLDecoder::new())
}

#[derive(Default)]
struct MLPacketiser {
    stream:     Option<NAStreamRef>,
    buf:        Vec<u8>,
    end:        usize,
    frameno:    u32,
    intra:      bool,
}

impl MLPacketiser {
    fn new() -> Self { Self::default() }
}

impl NAPacketiser for MLPacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
        self.stream = Some(stream);
    }
    fn add_data(&mut self, src: &[u8]) -> bool {
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
        if self.buf.len() < self.end {
            return Ok(None);
        }

        if self.end == 0 {
            self.intra = true;
        }

        let mut found = false;
        while self.end + 2 <= self.buf.len() {
            let op = u16::from(self.buf[self.end + 1]) * 256 + u16::from(self.buf[self.end]);
            self.end += 2;

            if op == (END_CODE * 2 + 1) {
                found = true;
                break;
            }
            // run
            if ((op & 1) == 1) && (0xE603..=0xEFFF).contains(&op) {
                self.end += 2;
            }
            // raw data
            if ((op & 1) == 1) && (op > 0xF800) {
                let raw_size = (((op >> 1) & 0x3FF) + 1) as usize;
                self.end += ((raw_size * 15 + 15) & !15) >> 3;
            }
            // copy from previous frame
            if ((op & 1) == 1) && ((op < 0x9000) || (0xF001..=0xF7FF).contains(&op)) {
                self.intra = false;
            }
        }

        if found {
            let mut data = Vec::with_capacity(self.end);
            data.extend_from_slice(&self.buf[..self.end]);
            self.buf.drain(..self.end);
            let ts = NATimeInfo::new(Some(u64::from(self.frameno)), None, None, stream.tb_num, stream.tb_den);
            self.end = 0;
            self.frameno += 1;

            return Ok(Some(NAPacket::new(stream, ts, self.intra, data)));
        }

        Ok(None)
    }
    fn reset(&mut self) {
        self.buf.clear();
        self.end = 0;
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

pub fn get_packetiser() -> Box<dyn NAPacketiser + Send> {
    Box::new(MLPacketiser::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::{RegisteredDecoders, RegisteredPacketisers};
    use nihav_core::demuxers::RegisteredRawDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_movinglines() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Acorn Replay Demonstration Disc 2
        test_decoding_raw("armovie", "movinglines", "assets/Acorn/CHEMSET2", Some(3),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x991f08b9, 0x5a1a8718, 0x2b52edd5, 0x446ba3aa],
                                [0x3721a673, 0x96e1cf7d, 0x4ea1f8b4, 0xfb2f0483],
                                [0xe83f7fd1, 0xe80290be, 0x2eb5723e, 0x74a767cb],
                                [0x3a69b78b, 0xc53c1356, 0xa4bf9f96, 0xaec1a38c]]));
    }
}
