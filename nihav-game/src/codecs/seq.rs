use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

const WIDTH:  usize = 320;
const HEIGHT: usize = 200;
const FRAME_HEADER: usize = 24;

struct SequenceDecoder {
    info:       NACodecInfoRef,
    pal:        [u8; 768],
    frame:      [u8; WIDTH * HEIGHT],
}

impl SequenceDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            pal:        [0; 768],
            frame:      [0; WIDTH * HEIGHT],
        }
    }
}

impl NADecoder for SequenceDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(_vinfo) = info.get_properties() {
            if let Some(ref edata) = info.get_extradata() {
                validate!(edata.len() > 32);
                let pal_start = read_u16le(&edata[25..])? as usize;
                let pal_len   = read_u16le(&edata[29..])? as usize;
                validate!(pal_len > 0 && pal_start + pal_len <= 256);
                match edata[32] {
                    0 => {
                        let dpal = self.pal[pal_start * 3..].chunks_exact_mut(3);
                        for (dst, quad) in dpal.zip(edata[37..].chunks_exact(4)) {
                            dst.copy_from_slice(&quad[1..]);
                        }
                    },
                    1 => self.pal[pal_start * 3..][..pal_len * 3].copy_from_slice(&edata[37..][..pal_len * 3]),
                    _ => return Err(DecoderError::NotImplemented),
                }
            } else {
                return Err(DecoderError::InvalidData);
            }
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(WIDTH, HEIGHT, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > FRAME_HEADER);

        let mut br = MemoryReader::new_read(&src);

        let width       = br.read_u16le()? as usize;
        let height      = br.read_u16le()? as usize;
        let xoff        = br.read_u16le()? as usize;
        let yoff        = br.read_u16le()? as usize;
        validate!(width + xoff <= WIDTH && height + yoff <= HEIGHT);

        let _ckey       = br.read_byte()?;
        let frm_type    = br.read_byte()?;
                          br.read_skip(6)?;
        let opcode_size = br.read_u16le()? as usize;
        validate!(opcode_size <= src.len() - FRAME_HEADER);

        let opcodes = &src[FRAME_HEADER..][..opcode_size];
        let clr_data = &src[FRAME_HEADER + opcode_size..];
        match frm_type {
            0 => {
                validate!(opcodes.is_empty());
                validate!(clr_data.len() == width * height);

                let dst = &mut self.frame[xoff + yoff * WIDTH..];
                for (dline, sline) in dst.chunks_mut(WIDTH).zip(clr_data.chunks(width)) {
                    dline[..width].copy_from_slice(sline);
                }
            },
            1 | 11 => {
                validate!(!opcodes.is_empty());
                let mut ops = MemoryReader::new_read(opcodes);
                let mut clr = MemoryReader::new_read(clr_data);

                let mut x = xoff;
                let mut y = yoff;
                while y < yoff + height {
                    let op = ops.read_byte()?;
                    let mut len = (op & 0x3F) as usize;
                    if len == 0 {
                        len = width + xoff - x;
                    }
                    match op >> 6 {
                        3 => x += len,
                        2 => {
                            clr.read_buf(&mut self.frame[x + y * WIDTH..][..len])?;
                            x += len;
                        },
                        _ => {
                            let len = ((u16::from(op & 0x7) << 8) | u16::from(ops.read_byte()?)) as usize;
                            match op >> 3 {
                                2 => x += len,
                                3 => {
                                    for _ in 0..len {
                                        validate!(y < height + yoff);
                                        self.frame[x + y * WIDTH] = clr.read_byte()?;
                                        x += 1;
                                        if x == width + xoff {
                                            y += 1;
                                            x = xoff;
                                        }
                                    }
                                },
                                6 => {
                                    let len = if len != 0 { len } else { height + yoff - y };
                                    for _ in 0..(len * width) {
                                        validate!(y < height + yoff);
                                        self.frame[x + y * WIDTH] = clr.read_byte()?;
                                        x += 1;
                                        if x == width + xoff {
                                            y += 1;
                                            x = xoff;
                                        }
                                    }
                                },
                                7 => {
                                    if len > 0 {
                                        y += len;
                                    } else {
                                        y = height + yoff;
                                    }
                                },
                                _ => return Err(DecoderError::NotImplemented),
                            }
                        },
                    }
                    if x == width + xoff {
                        x = xoff;
                        y += 1;
                    }
                }
            },
            _ => return Err(DecoderError::InvalidData),
        }


        let buf = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;
        let mut vbuf = buf.get_vbuf().unwrap();
        let paloff = vbuf.get_offset(1);
        let stride = vbuf.get_stride(0);
        let data = vbuf.get_data_mut().unwrap();

        for (drow, srow) in data.chunks_mut(stride).zip(self.frame.chunks_exact(WIDTH)) {
            drow[..WIDTH].copy_from_slice(srow);
        }
        data[paloff..][..768].copy_from_slice(&self.pal);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buf);
        let ftype = if pkt.keyframe { FrameType::I } else { FrameType::P };
        frm.set_frame_type(ftype);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for SequenceDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(SequenceDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_seq1() {
        let mut dmx_reg = RegisteredDemuxers::new();
        game_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from King's Quest VI
        test_decoding("sierra-seq", "seq-video", "assets/Game/sierra/FS1.SEQ",
                      Some(2), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x5a7472da, 0xa9e242fd, 0x867efa52, 0x9625f05c],
                            [0x720ab982, 0x704970a0, 0xf854af8b, 0x3b86bed9],
                            [0xaa1426e1, 0x79750652, 0x87b7a727, 0xc582a561]]));
    }
    #[test]
    fn test_seq2() {
        let mut dmx_reg = RegisteredDemuxers::new();
        game_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from Gabriel Knight
        test_decoding("sierra-seq", "seq-video", "assets/Game/sierra/blood.seq",
                      Some(2), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x989422ee, 0x5892beae, 0x0ca9db17, 0xe25ab710],
                            [0x0d5f395e, 0x2eeac229, 0x1504ece0, 0xa7d3401e],
                            [0x988d3fa6, 0x68be4639, 0x7ab7137c, 0x72e69e26]]));
    }
}
