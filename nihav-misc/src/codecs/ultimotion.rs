use nihav_core::io::byteio::{ByteIO,MemoryReader};
use nihav_core::io::bitreader::*;
use nihav_core::codecs::*;
use nihav_codec_support::codecs::HAMShuffler;
use super::ultidata::*;

trait ReadLuma {
    fn read_y(&mut self) -> DecoderResult<u8>;
}

impl<'a> ReadLuma for BitReader<'a> {
    fn read_y(&mut self) -> DecoderResult<u8> {
        Ok(LUMA_MAP[self.read(6)? as usize])
    }
}

struct UltimotionDecoder {
    info:       NACodecInfoRef,
    frmmgr:     HAMShuffler<u8>,
    width:      usize,
    height:     usize,
}

impl UltimotionDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfo::new_dummy(),
            frmmgr:     HAMShuffler::new(),
            width:      0,
            height:     0,
        }
    }
}

fn paint_gradient(dst: &mut [u8], xoff: usize, yoff: usize, stride: usize, angle: u8, yy: [u8; 4]) {
    for (row, prow) in dst.chunks_exact_mut(stride).skip(yoff)
            .zip(PATTERN[usize::from(angle)].chunks_exact(4)) {
        for (el, &idx) in row[xoff..][..4].iter_mut().zip(prow.iter()) {
            *el = yy[usize::from(idx)];
        }
    }
}

impl NADecoder for UltimotionDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            self.width = w;
            self.height = h;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, YUV410_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(!src.is_empty() && src[src.len() - 1] == 0x73);
        let mut br = MemoryReader::new_read(&src);

        let mut buf = if pkt.is_keyframe() {
                let vinfo = self.info.get_properties().get_video_info().unwrap();
                let bufinfo = alloc_video_buffer(vinfo, 3)?;
                bufinfo.get_vbuf().unwrap()
            } else if let Some(vbuf) = self.frmmgr.clone_ref() {
                vbuf
            } else {
                return Err(DecoderError::MissingReference);
            };
        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();
        let uvsplit = frm.offset[1].min(frm.offset[2]);
        let (luma, chroma) = frm.data.split_at_mut(uvsplit);
        let mut mode0 = true;
        let mut uniq_c = false;
        let mut uniq_c_perm = false;
        let mut to_skip = 0;
        let mut is_intra = true;
        let mut uoff = frm.offset[1] - uvsplit;
        let mut voff = frm.offset[2] - uvsplit;
        let mut ybuf = [0; 12];
        'dec: for ystrip in luma.chunks_exact_mut(frm.stride[0] * 8)
                .take(self.height / 8) {
            'row_loop: for x in (0..self.width).step_by(8) {
                if to_skip > 0 {
                    to_skip -= 1;
                    continue;
                }
                let mut mode;
                loop {
                    mode = br.read_byte()?;
                    if (mode & 0xF8) != 0x70 {
                        break;
                    }
                    match mode & 0x7 {
                        0 => {
                            let new_mode = br.read_byte()?;
                            validate!(new_mode < 2);
                            mode0 = new_mode == 0;
                        },
                        1 => {
                            uniq_c = true;
                        },
                        2 => {
                            uniq_c_perm = !uniq_c_perm;
                        },
                        3 => {
                            is_intra = false;
                            break 'dec;
                        },
                        4 => {
                            to_skip = br.read_byte()?;
                            validate!(to_skip > 0);
                            to_skip -= 1;
                            is_intra = false;
                            continue 'row_loop;
                        },
                        _ => {
                            println!("unknown opcode {mode:02X}");
                            return Err(DecoderError::InvalidData);
                        },
                    }
                }
                uniq_c |= uniq_c_perm;
                let mut cval = if !uniq_c && mode != 0 { br.read_byte()? } else { 0 };
                for (quad, &(xoff, yoff)) in [(0, 0), (0, 4), (4, 4), (4, 0)].iter().enumerate() {
                    let qmode = (mode << (quad * 2)) >> 6;
                    if uniq_c && qmode != 0 {
                        cval = br.read_byte()?;
                    }
                    if qmode != 0 {
                        chroma[uoff + (x + xoff) / 4 + (yoff / 4) * frm.stride[1]] = CHROMA_MAP[usize::from(cval >> 4)];
                        chroma[voff + (x + xoff) / 4 + (yoff / 4) * frm.stride[2]] = CHROMA_MAP[usize::from(cval & 0xF)];
                    }
                    match (qmode, mode0) {
                        (0, _) => { // unchanged quadrant
                            is_intra = false;
                        },
                        (1, _) => { // homogeneous/shallow LTC quadrant
                            const ANGLES: [u8; 4] = [0x0, 0x2, 0x6, 0xC];
                            let m2 = br.read_byte()?;
                            let mut angle = ANGLES[usize::from(m2 >> 6)];
                            let y0 = usize::from(m2 & 0x3F);
                            let y2 = if angle != 0 { (y0 + 1).min(0x3F) } else { y0 };
                            let y0 = LUMA_MAP[y0];
                            let y2 = LUMA_MAP[y2];
                            let mut yy = [y0, y0, y2, y2];
                            if (angle & 8) != 0 {
                                yy.swap(0, 3);
                                yy.swap(1, 2);
                                angle &= 7;
                            }
                            paint_gradient(ystrip, x + xoff, yoff, frm.stride[0], angle, yy);
                        },
                        (2, true) => { // LTC quadrant
                            let m2 = br.read_u16be()?;
                            let mut angle = (m2 >> 12) as u8;
                            let mut yy = CODEBOOK[usize::from(m2 & 0xFFF)];
                            for el in yy.iter_mut() {
                                *el = LUMA_MAP[usize::from(*el)];
                            }
                            if (angle & 8) != 0 {
                                yy.swap(0, 3);
                                yy.swap(1, 2);
                                angle &= 7;
                            }
                            paint_gradient(ystrip, x + xoff, yoff, frm.stride[0], angle, yy);
                        },
                        (2, false) => { // subsampled 4-luminance quadrant
                            br.read_buf(&mut ybuf[..3])?;
                            let mut bbr = BitReader::new(&ybuf, BitReaderMode::BE);
                            let mut yy = [0; 4];
                            for el in yy.iter_mut() {
                                *el = bbr.read_y()?;
                            }
                            paint_gradient(ystrip, x + xoff, yoff, frm.stride[0], 0xA, yy);
                        },
                        (3, true) => { // statistical/extended LTC quadrant
                            br.read_buf(&mut ybuf[..2])?;
                            let mut bbr = BitReader::new(&ybuf, BitReaderMode::BE);
                            let yone = LUMA_MAP[usize::from(br.read_byte()?.min(0x3F))];
                            let ytwo = LUMA_MAP[usize::from(br.read_byte()?.min(0x3F))];
                            if bbr.read_bool()? {
                                let angle = bbr.read(3)? as u8;
                                let mut yy = [0; 4];
                                yy[0] = bbr.read_y()?;
                                yy[1] = bbr.read_y()?;
                                yy[2] = yone;
                                yy[3] = ytwo;
                                paint_gradient(ystrip, x + xoff, yoff, frm.stride[0], angle, yy);
                            } else {
                                let mut pattern = bbr.read(15)? as usize;
                                let yy = [yone, ytwo];
                                for row in ystrip.chunks_exact_mut(frm.stride[0])
                                        .skip(yoff).take(4) {
                                    for el in row[x + xoff..][..4].iter_mut() {
                                        *el = yy[(pattern >> 15) & 1];
                                        pattern <<= 1;
                                    }
                                }
                            }
                        },
                        (3, false) => { // 16-luminance quadrant
                            br.read_buf(&mut ybuf)?;
                            let mut bbr = BitReader::new(&ybuf, BitReaderMode::BE);
                            for row in ystrip.chunks_exact_mut(frm.stride[0])
                                    .skip(yoff).take(4) {
                                for el in row[x + xoff..][..4].iter_mut() {
                                    *el = bbr.read_y()?;
                                }
                            }
                        },
                        _ => unreachable!(),
                    }
                }
                uniq_c = false;
            }
            uoff += 2 * frm.stride[1];
            voff += 2 * frm.stride[2];
        }
        self.frmmgr.add_frame(buf.clone());

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), NABufferType::Video(buf));
        frm.set_keyframe(is_intra);
        frm.set_frame_type(if is_intra { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        self.frmmgr.clear();
    }
}

impl NAOptionHandler for UltimotionDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(UltimotionDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_ultimotion() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from https://samples.mplayerhq.hu/V-codecs/ULTI/hit12w.avi
        test_decoding("avi", "ultimotion", "assets/Misc/hit12w.avi", Some(3), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x3e420232, 0x3575f4e3, 0x8cd70d09, 0x76b6ec33],
                            [0xd0802190, 0x79769f38, 0xd6c9def3, 0xda61fe43],
                            [0xc56c1aff, 0x4a8de0b0, 0xce778675, 0x81fe7265],
                            [0x27c87c55, 0xa8c9077a, 0x1780fedf, 0x9f45d71b]]));
    }
}
