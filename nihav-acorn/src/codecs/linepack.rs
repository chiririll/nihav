use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

use super::yuvtab::YUV2RGB;

#[derive(Default)]
struct LinePackDecoder {
    info:           NACodecInfoRef,
    cur_frm:        Vec<u16>,
    prev_frm:       Vec<u16>,
    width:          usize,
    is_yuv:         bool,
}

impl LinePackDecoder {
    fn new() -> Self { Self::default() }
}

impl NADecoder for LinePackDecoder {
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
            let val = br.read_u16le()?;
            if (val & 0x8000) == 0 { // raw pixel
                self.cur_frm[dpos] = val;
                dpos += 1;
            } else {
                let op = (val >> 12) & 7;
                match op {
                    0 => { // skip
                        let len = (val & 0xFFF) as usize;
                        validate!(dpos + len <= self.cur_frm.len());
                        self.cur_frm[dpos..][..len].copy_from_slice(&self.prev_frm[dpos..][..len]);
                        dpos += len;
                        is_intra = false;
                    },
                    1 => { // motion
                        let dx = (( val       & 7) as isize) - 4;
                        let dy = (((val >> 3) & 7) as isize) - 4;
                        let len = ((val >> 6) & 0x3F) as usize;
                        validate!(dpos + len <= self.cur_frm.len());
                        if dx == 0 && dy == 0 { // previous line
                            validate!(dpos >= self.width);
                            for _ in 0..len {
                                self.cur_frm[dpos] = self.cur_frm[dpos - self.width];
                                dpos += 1;
                            }
                        } else {
                            let offset = (dpos as isize) + dx + dy * (self.width as isize);
                            validate!(offset >= 0);
                            let offset = offset as usize;
                            validate!(offset + len <= self.prev_frm.len());
                            self.cur_frm[dpos..][..len].copy_from_slice(&self.prev_frm[offset..][..len]);
                            dpos += len;
                            is_intra = false;
                        }
                    },
                    2 => { // run
                        let len = (val & 0xFFF) as usize;
                        validate!(dpos + len <= self.cur_frm.len());
                        let pix = br.read_u16le()?;
                        for _ in 0..len {
                            self.cur_frm[dpos] = pix;
                            dpos += 1;
                        }
                    },
                    3 => { // raw
                        let len = (val & 0xFFF) as usize;
                        validate!(dpos + len <= self.cur_frm.len());
                        for _ in 0..len {
                            self.cur_frm[dpos] = br.read_u16le()?;
                            dpos += 1;
                        }
                    },
                    4 => { // four-colour pattern
                        let len = (val & 0xFF) as usize;
                        validate!(dpos + len <= self.cur_frm.len());
                        let clrs = [
                                br.read_u16le()?,
                                br.read_u16le()?,
                                br.read_u16le()?,
                                br.read_u16le()?
                            ];
                        let mut mask = 0;
                        let mut pos = 8;

                        for _i in 0..len {
                            if pos == 8 {
                                mask = br.read_u16le()? as usize;
                                pos = 0;
                            }
                            self.cur_frm[dpos] = clrs[mask & 3];
                            dpos += 1;
                            mask >>= 2;
                            pos += 1;
                        }
                    },
                    5 => { // interleaved
                        let len = (val & 0xFFF) as usize;
                        validate!(dpos + len * 2 <= self.cur_frm.len());
                        let clrs = [
                                br.read_u16le()?,
                                br.read_u16le()?
                            ];
                        for _ in 0..len {
                            self.cur_frm[dpos] = clrs[0];
                            dpos += 1;
                            self.cur_frm[dpos] = clrs[1];
                            dpos += 1;
                        }
                    },
                    _ => return Err(DecoderError::NotImplemented),
                }
            }
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

impl NAOptionHandler for LinePackDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(LinePackDecoder::new())
}

#[derive(Default)]
struct LinePackPacketiser {
    stream:     Option<NAStreamRef>,
    buf:        Vec<u8>,
    end:        usize,
    frameno:    u32,
    intra:      bool,
    pos:        usize,
    img_size:   usize,
}

impl LinePackPacketiser {
    fn new() -> Self { Self::default() }
}

impl NAPacketiser for LinePackPacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
        let vinfo = stream.get_info().get_properties().get_video_info().unwrap();
        self.img_size = vinfo.width * vinfo.height;
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
            self.pos = 0;
        }

        while self.end + 2 <= self.buf.len() && self.pos < self.img_size {
            let val = u16::from(self.buf[self.end + 1]) * 256 + u16::from(self.buf[self.end]);
            self.end += 2;

            if (val & 0x8000) == 0 {
                self.pos += 1;
            } else {
                let op = (val >> 12) & 7;
                let common_len = (val & 0xFFF) as usize;
                self.pos += match op {
                        0 => common_len, // skip size
                        1 => ((val >> 6) & 0x3F) as usize, // motion size
                        2 => common_len, // run
                        3 => common_len, // raw
                        4 => common_len & 0xFF, // four-colour pattern
                        5 => common_len * 2, // interleaved
                        _ => 0, // ???
                    };
                self.end += match op {
                        2 => 2, // run value
                        3 => common_len * 2, // raw values
                        4 => 8 + ((common_len & 0xFF) + 7) / 8 * 2, // 4 colours + masks
                        5 => 4, // two values
                        _ => 0,
                    };
                if (op == 0) || (op == 1 && (val & 0x3F) != 0x24) || (op == 2) {
                    self.intra = false;
                }
            }
        }

        if self.pos >= self.img_size && self.end <= self.buf.len() {
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
    Box::new(LinePackPacketiser::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::{RegisteredDecoders, RegisteredPacketisers};
    use nihav_core::demuxers::RegisteredRawDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_linepack() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Cine Clips by Oregan Software Developments
        test_decoding_raw("armovie", "linepack", "assets/Acorn/COLOURPLUS", Some(5),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x3189751c, 0x497c9af4, 0x7d3add86, 0x48c9c77d],
                                [0x3189751c, 0x497c9af4, 0x7d3add86, 0x48c9c77d],
                                [0x3189751c, 0x497c9af4, 0x7d3add86, 0x48c9c77d],
                                [0x3189751c, 0x497c9af4, 0x7d3add86, 0x48c9c77d],
                                [0x3189751c, 0x497c9af4, 0x7d3add86, 0x48c9c77d],
                                [0x6b56cca6, 0xa35d8057, 0x8eff6a2d, 0x53a07f52]]));
    }
}
