use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

pub const ARGB_FORMAT: NAPixelFormaton = NAPixelFormaton {
        model: ColourModel::RGB(RGBSubmodel::RGB), components: 4,
        comp_info: [
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 0, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 1, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 2, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 3, next_elem: 4 }),
            None ],
        elem_size: 4, be: false, alpha: true, palette: false };

#[derive(Default)]
struct PlanarRGBDecoder {
    info:       NACodecInfoRef,
    width:      usize,
    height:     usize,
    depth:      u8,
}

struct Frame<'a> {
    data:       &'a mut [u8],
    width:      usize,
    height:     usize,
    stride:     usize,
    step:       usize,
}

fn decode_plane(line_sizes: &[u8], src: &[u8], offset: usize, frm: &mut Frame, plane: usize) -> DecoderResult<usize> {
    let mut read_pos = offset;
    for (linesz, line) in line_sizes.chunks_exact(2)
            .zip(frm.data.chunks_exact_mut(frm.stride).take(frm.height)) {
        let size = usize::from(read_u16be(linesz).unwrap_or_default());
        validate!(read_pos + size <= src.len());
        let mut br = MemoryReader::new_read(&src[read_pos..][..size]);
        read_pos += size;

        let mut pos = 0;
        let mut off = plane;
        while pos < frm.width {
            let op = usize::from(br.read_byte()?);
            if op < 0x80 {
                let len = op + 1;
                validate!(pos + len <= frm.width);
                for _ in 0..len {
                    line[off] = br.read_byte()?;
                    off += frm.step
                }
                pos += len;
            } else {
                let len = 257 - op;
                validate!(pos + len <= frm.width);
                let pix = br.read_byte()?;
                for _ in 0..len {
                    line[off] = pix;
                    off += frm.step
                }
                pos += len;
            }
        }
    }
    Ok(read_pos)
}

impl NADecoder for PlanarRGBDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.get_width();
            self.height = vinfo.get_height();
            self.depth = vinfo.bits;
            let fmt = match self.depth {
                     8 => PAL8_FORMAT,
                    24 => RGB24_FORMAT,
                    32 => ARGB_FORMAT,
                     _ => return Err(DecoderError::InvalidData),
                };
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, fmt));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() >= 2);
        let planes = usize::from(self.depth / 8);

        let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;
        let mut buf = bufinfo.get_vbuf().unwrap();
        let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();

        let mut pos = 0;
        let mut fdata = Frame {
                data:   frm.data,
                width:  self.width,
                height: self.height,
                stride: frm.stride[0],
                step:   planes,
            };
        let plane_hdr_size = self.height * 2;
        if read_u16be(&src).unwrap_or(0) != 1 {
            let (size_data, rle_data) = src.split_at(plane_hdr_size * planes);
            for (planeno, line_sizes) in size_data.chunks_exact(plane_hdr_size).enumerate() {
                pos = decode_plane(line_sizes, rle_data, pos, &mut fdata, planeno)?;
            }
        } else {
            for planeno in 0..planes {
                validate!(2 + pos + plane_hdr_size < src.len());
                validate!(read_u16be(&src[pos..]).unwrap_or(0) == 1);
                pos += 2;
                let (line_sizes, rle_data) = src[pos..].split_at(plane_hdr_size);
                let rd_size = decode_plane(line_sizes, rle_data, 0, &mut fdata, planeno)?;
                pos += plane_hdr_size + rd_size;
            }
        }

        if self.depth == 8 {
            let paloff = frm.offset[1];
            let dpal = &mut frm.data[paloff..];
            for sd in pkt.side_data.iter() {
                match *sd {
                    NASideData::Palette(_, ref pal) => {
                        for (dst, src) in dpal.chunks_mut(3).zip(pal.chunks(4)) {
                            dst[0] = src[0];
                            dst[1] = src[1];
                            dst[2] = src[2];
                        }
                        break;
                    },
                    _ => {},
                };
            }
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for PlanarRGBDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::<PlanarRGBDecoder>::default()
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::qt_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    // samples from https://samples.mplayerhq.hu/V-codecs/8BPS-PlanarRGB/
    #[test]
    fn test_8bps_pal() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "8bps", "assets/QT/full9ironswing.mov", Some(0), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0xa0607989, 0xf75365ee, 0x96d9bed8, 0x49f92014]]));
    }
    #[test]
    fn test_8bps_rgb24() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "8bps", "assets/QT/aletrek-8BPS.mov", Some(1), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x4782f1e2, 0x88ef50e2, 0x3525a06e, 0xac5def8d]]));
    }
    #[test]
    fn test_8bps_rgb32() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        test_decoding("mov", "8bps", "assets/QT/8bpsbgra.mov", Some(1), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0xa578ced2, 0x6e480a5c, 0x14b3a859, 0x465a1cdd],
                            [0xa2db454b, 0x8d4130c9, 0x743fb0a8, 0x63766105]]));
    }
}
