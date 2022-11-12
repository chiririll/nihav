use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_core::io::bitreader::*;

const HEADER_SIZE: usize = 0x2F;

const MV_2BIT: [(i8, i8); 4] = [(-1, 0), (-1, -1), (1, -1), (0, -2)];
const MV_4BIT: [(i8, i8); 16] = [
    (-2, -3), ( 2, -3), (-1, -4), ( 1, -4),
    (-1, -2), ( 1, -2), ( 0, -3), ( 0, -4),
    (-2,  0), (-2, -1), ( 1, -1), (-2, -2),
    ( 2, -2), (-1, -3), ( 1, -3), ( 0, -5)
];

const BPP: usize = 4;

struct ArxelVideoDecoder {
    info:       NACodecInfoRef,
    tiles:      [u8; 65536],
}

impl ArxelVideoDecoder {
    fn new() -> Self {
        Self {
            info:   NACodecInfoRef::default(),
            tiles:  [0; 65536],
        }
    }
}

const RGBA_FORMAT: NAPixelFormaton = NAPixelFormaton {
        model: ColorModel::RGB(RGBSubmodel::RGB), components: 4,
        comp_info: [
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 2, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 1, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 0, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 3, next_elem: 4 }),
            None ],
        elem_size: 4, be: false, alpha: true, palette: false };

impl NADecoder for ArxelVideoDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), true, RGBA_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > HEADER_SIZE);

        let mut mr = MemoryReader::new_read(&src);
        let mut br = ByteReader::new(&mut mr);

        let size                            = br.read_u32le()? as usize;
        validate!(src.len() >= size + HEADER_SIZE);
        let part2_off                       = br.read_u32le()? as u64;
        validate!(part2_off > 0 && part2_off < (size as u64));
        let num_tiles                       = br.read_u16le()? as usize;
        validate!(num_tiles > 0 && num_tiles < 4096);
        let tile_size                       = br.read_u16le()? as usize;
        let width                           = br.read_u32le()? as usize;
        let height                          = br.read_u32le()? as usize;

        let vinfo = self.info.get_properties().get_video_info().unwrap();
        validate!(width == vinfo.get_width());
        validate!(height == vinfo.get_height());

                                              br.seek(SeekFrom::Start(part2_off + (HEADER_SIZE as u64)))?;
        match tile_size {
            2 => {
                return Err(DecoderError::NotImplemented);
            },
            4 => {
                                              br.read_buf(&mut self.tiles[..16])?;
                let off = br.tell() as usize;
                let mut bir = BitReader::new(&src[off..], BitReaderMode::BE);
                for tile in 1..num_tiles {
                    for i in 0..16 {
                        self.tiles[tile * 16 + i] = self.tiles[tile * 16 + i - 16];
                    }
                    let bits                = bir.read(3)? as u8 + 1;
                    validate!(bits < 8);
                    for el in self.tiles[tile * 16..][..16].iter_mut() {
                        let mut delta       = bir.read(bits)? as i16;
                        if delta != 0 && bir.read_bool()? {
                            delta = -delta;
                        }
                        *el = (i16::from(*el) + delta) as u8;
                    }
                }
            },
            _ => {
                validate!(tile_size > 0);
                                              br.read_buf(&mut self.tiles[..tile_size])?;
            },
        };

        let bufinfo = alloc_video_buffer(vinfo, 0)?;
        let bufo = bufinfo.get_vbuf();
        let mut buf = bufo.unwrap();
        let stride = buf.get_stride(0);
        let data = buf.get_data_mut().unwrap();
        let dst = data.as_mut_slice();

        let mut br = BitReader::new(&src[HEADER_SIZE..], BitReaderMode::BE);
        let tile_w = 4;
        let tsize = tile_w * BPP;
        let idx_bits = if num_tiles < 0x400 { 10 } else if num_tiles < 0x800 { 11 } else { 12 };
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(tile_w) {
                let dst_pos = x * BPP + y * stride;
                if !br.read_bool()? {
                    let idx             = br.read(idx_bits)? as usize;
                    validate!(idx < num_tiles);
                    dst[dst_pos..][..tsize].copy_from_slice(&self.tiles[idx * tsize..][..tsize]);
                } else {
                    let (mv_x, mv_y) = if br.read_bool()? {
                            (0, -1)
                        } else if br.read_bool()? {
                            MV_2BIT[br.read(2)? as usize]
                        } else {
                            MV_4BIT[br.read(4)? as usize]
                        };

                    let isrc = (dst_pos as isize) + isize::from(mv_x) * (tsize as isize) + isize::from(mv_y) * ((stride * 2) as isize);
                    validate!(isrc >= 0);
                    let src_pos = isrc as usize;
                    validate!(src_pos + tsize <= dst.len());
                    let (src, dst) = dst.split_at_mut(dst_pos);
                    dst[..tsize].copy_from_slice(&src[src_pos..][..tsize]);
                }
            }
            // double lines
            let lines = &mut dst[y * stride..];
            let (src, dst) = lines.split_at_mut(stride);
            dst[..stride].copy_from_slice(src);
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for ArxelVideoDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}


pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(ArxelVideoDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::game_register_all_decoders;
    use crate::game_register_all_demuxers;
    // sample from the Ring game
    #[test]
    fn test_arxel_video() {
        let mut dmx_reg = RegisteredDemuxers::new();
        game_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        test_decoding("arxel-cnm", "arxel-video", "assets/Game/logo.cnm", Some(10), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0xcf12f83a, 0xfdce1ed2, 0x2d183394, 0xa265f164]));
    }
}
