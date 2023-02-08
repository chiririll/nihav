use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

const INVALID_CLR: u16 = 0x8000;

trait InvalidPixel {
    fn is_invalid(self) -> bool;
}
impl InvalidPixel for u16 {
    fn is_invalid(self) -> bool { self == INVALID_CLR }
}

trait PixelOps {
    fn unpack(&self) -> [u16; 4];
    fn dist<T: PixelOps>(&self, val: T) -> u32 {
        dist_core(self.unpack(), &val.unpack())
    }
}

impl PixelOps for u16 {
    fn unpack(&self) -> [u16; 4] {
        let val = *self;
        let r = (val >> 10) & 0x1F;
        let g = (val >>  5) & 0x1F;
        let b =  val        & 0x1F;
        [r, g, b, rgb2y(r, g, b)]
    }
}

fn dist_core(val: [u16; 4], other: &[u16; 4]) -> u32 {
    let sum = val.iter().zip(other.iter()).take(3).fold(0i32,
            |acc, (&a, &b)| {
                let diff = i32::from(a) - i32::from(b);
                acc + diff * diff
            });
    sum as u32
}


fn rgb2y(r: u16, g: u16, b: u16) -> u16 {
    (r * 77 + g * 150 + b * 29) >> 8
}

fn pack_rgb555(val: [u16; 4]) -> u16 {
    (val[0] << 10) | (val[1] << 5) | val[2]
}

#[derive(Default)]
struct PixelAverage {
    sum:    [u16; 4],
    count:  u16,
}

impl PixelAverage {
    fn new() -> Self { Self::default() }
    fn add(&mut self, val: &[u16; 4]) {
        for (dst, &src) in self.sum.iter_mut().zip(val.iter()) {
            *dst += src;
        }
        self.count += 1;
    }
    fn get_avg(&self) -> [u16; 4] {
        if self.count > 0 {
            let mut ret = self.sum;
            for el in ret.iter_mut() {
                *el /= self.count;
            }
            ret
        } else {
            [0; 4]
        }
    }
}

macro_rules! quant_template {
    ($name:ident, $N:expr) => {
        fn $name(pix: &[u16; $N]) -> ([u16; 2], u16, u32) {
            let mut tmp = [[0; 4]; $N];
            let mut avg = PixelAverage::new();
            let mut maxv = [0; 4];
            let mut minv = [255; 4];
            for (dst, &src) in tmp.iter_mut().zip(pix.iter()) {
                *dst = src.unpack();
                avg.add(dst);
                for ((maxv, minv), &comp) in maxv.iter_mut().zip(minv.iter_mut()).zip(dst.iter()) {
                    *maxv = (*maxv).max(comp);
                    *minv = (*minv).min(comp);
                }
            }
            let avg = avg.get_avg();

            let mut best_axis = 3;
            let mut best_dist = maxv[3] - minv[3];
            for (comp_no, (&minval, &maxval)) in minv.iter().zip(maxv.iter()).enumerate().take(3) {
                if maxval - minval > best_dist {
                    best_axis = comp_no;
                    best_dist = maxval - minval;
                }
            }
            if best_dist == 0 {
                let avg_pix = pack_rgb555(avg);
                let mut dist = 0;
                for el in tmp.iter() {
                    dist += dist_core(avg, el);
                }
                return ([avg_pix; 2], 0, dist);
            }

            let mut avg1 = PixelAverage::new();
            let mut avg2 = PixelAverage::new();
            let mut mask = 0;
            let mut mask_bit = 1;
            for clr in tmp.iter() {
                if clr[best_axis] > avg[best_axis] {
                    avg2.add(clr);
                    mask |= mask_bit;
                } else {
                    avg1.add(clr);
                }
                mask_bit <<= 1;
            }

            let clr0 = avg1.get_avg();
            let clr1 = avg2.get_avg();
            let mut dist = 0;
            for clr in tmp.iter() {
                let dist0 = dist_core(clr0, clr);
                let dist1 = dist_core(clr1, clr);
                dist += dist0.min(dist1);
            }
            ([pack_rgb555(clr0), pack_rgb555(clr1)], mask, dist)
        }
    }
}

quant_template!(quant2_16pix, 16);
quant_template!(quant2_4pix, 4);

#[derive(Default)]
struct BlockState {
    fill_dist:  u32,
    fill_val:   u16,
    clr2_dist:  u32,
    clr2_flags: u16,
    clr2:       [u16; 2],
    clr8_dist:  u32,
    clr8_flags: u16,
    clr8:       [[u16; 2]; 4],
}

impl BlockState {
    fn calc_clrs(buf: &[u16; 16]) -> Option<(u16, u16)> {
        let     clr0 = buf[0];
        let mut clr1 = INVALID_CLR;
        for &pix in buf[1..].iter() {
            if pix != clr0 && pix != clr1 {
                if clr1.is_invalid() {
                    clr1 = pix;
                } else {
                    return None;
                }
            }
        }
        Some((clr0, clr1))
    }
    fn calc_stats(&mut self, buf: &[u16; 16]) {
        let mut filled = false;
        let mut two_clr = false;
        if let Some((clr0, clr1)) = Self::calc_clrs(buf) {
            self.clr2[0] = clr0;
            self.clr2[1] = if !clr1.is_invalid() { clr1 } else { clr0 };
            if clr0 == clr1 {
                self.fill_val = buf[0] & !0x400;
                filled = true;
            }
            two_clr = true;
        }
        self.fill_dist = 0;
        if !filled {
            let mut avg = PixelAverage::new();
            for &pix in buf.iter() {
                avg.add(&pix.unpack());
            }
            self.fill_val = pack_rgb555(avg.get_avg()) & !0x400;
            for pix in buf.iter() {
                self.fill_dist += pix.dist(self.fill_val);
            }
        }
        if self.fill_dist == 0 {
            self.clr2_dist = std::u32::MAX;
            self.clr8_dist = std::u32::MAX;
            return;
        }

        self.clr2_flags = 0u16;
        if two_clr {
            let mut mask = 1;
            self.clr2_dist = 0;
            for &pix in buf.iter() {
                if pix == self.clr2[0] {
                    self.clr2_flags |= mask;
                } else {
                }
                mask <<= 1;
            }
            if (self.clr2_flags & 0x8000) != 0 {
                self.clr2_flags = !self.clr2_flags;
                self.clr2.swap(0, 1);
            }
        } else {
            let (clrs, mask, dist) = quant2_16pix(&buf);
            self.clr2 = clrs;
            self.clr2_flags = mask;
            self.clr2_dist = dist;
            if (self.clr2_flags & 0x8000) != 0 {
                self.clr2_flags = !self.clr2_flags;
                self.clr2.swap(0, 1);
            }
        }
        if self.clr2_dist == 0 {
            self.clr8_dist = std::u32::MAX;
            return;
        }

        self.clr8 = [[0; 2]; 4];
        self.clr8_flags = 0;
        self.clr8_dist = 0;
        for i in 0..4 {
            let off = (i & 1) * 2 + (i & 2) * 4;
            let src2 = [buf[off], buf[off + 1], buf[off + 4], buf[off + 5]];
            let (clrs, mask, dist) = quant2_4pix(&src2);
            self.clr8[i] = clrs;
            self.clr8_flags |= mask << (i * 4);
            self.clr8_dist += dist;
        }
        if (self.clr8_flags & 0x8000) != 0 {
            self.clr8_flags ^= 0xF000;
            self.clr8[3].swap(0, 1);
        }
    }
    fn put_fill(&self, dst: &mut [u16], dstride: usize) {
        for line in dst.chunks_mut(dstride) {
            for i in 0..4 {
                line[i] = self.fill_val;
            }
        }
    }
    fn put_clr2(&self, dst: &mut [u16], dstride: usize) {
        for j in 0..4 {
            for i in 0..4 {
                if (self.clr2_flags & (1 << (i + j * 4))) == 0 {
                    dst[i + j * dstride] = self.clr2[0];
                } else {
                    dst[i + j * dstride] = self.clr2[1];
                }
            }
        }
    }
    fn put_clr8(&self, dst: &mut [u16], dstride: usize) {
        for i in 0..4 {
            let off = (i & 1) * 2 + (i & 2) * dstride;
            let cur_flg = (self.clr8_flags >> (i * 4)) & 0xF;
            dst[off]               = self.clr8[i][( !cur_flg       & 1) as usize];
            dst[off + 1]           = self.clr8[i][((!cur_flg >> 1) & 1) as usize];
            dst[off +     dstride] = self.clr8[i][((!cur_flg >> 2) & 1) as usize];
            dst[off + 1 + dstride] = self.clr8[i][((!cur_flg >> 3) & 1) as usize];
        }
    }
    fn write_fill(&self, bw: &mut ByteWriter) -> EncoderResult<()> {
        bw.write_u16le(self.fill_val | 0x8000)?;
        Ok(())
    }
    fn write_clr2(&self, bw: &mut ByteWriter) -> EncoderResult<()> {
        bw.write_u16le(self.clr2_flags)?;
        bw.write_u16le(self.clr2[0])?;
        bw.write_u16le(self.clr2[1])?;
        Ok(())
    }
    fn write_clr8(&self, bw: &mut ByteWriter) -> EncoderResult<()> {
        bw.write_u16le(self.clr8_flags)?;
        bw.write_u16le(self.clr8[0][0] | 0x8000)?;
        bw.write_u16le(self.clr8[0][1])?;
        bw.write_u16le(self.clr8[1][0])?;
        bw.write_u16le(self.clr8[1][1])?;
        bw.write_u16le(self.clr8[2][0])?;
        bw.write_u16le(self.clr8[2][1])?;
        bw.write_u16le(self.clr8[3][0])?;
        bw.write_u16le(self.clr8[3][1])?;
        Ok(())
    }
}

struct MSVideo1Encoder {
    stream:     Option<NAStreamRef>,
    pkt:        Option<NAPacket>,
    pool:       NAVideoBufferPool<u16>,
    lastfrm:    Option<NAVideoBufferRef<u16>>,
    quality:    u8,
    frmcount:   u8,
    key_int:    u8,
}

impl MSVideo1Encoder {
    fn new() -> Self {
        Self {
            stream:     None,
            pkt:        None,
            pool:       NAVideoBufferPool::new(2),
            lastfrm:    None,
            quality:    0,
            frmcount:   0,
            key_int:    25,
        }
    }
    fn get_block(src: &[u16], sstride: usize, buf: &mut [u16; 16]) {
        for (line, dst) in src.chunks(sstride).zip(buf.chunks_mut(4)) {
            dst.copy_from_slice(&line[..4]);
        }
    }
    fn write_skips(bw: &mut ByteWriter, skips: usize) -> EncoderResult<()> {
        bw.write_u16le((skips as u16) | 0x8400)?;
        Ok(())
    }
    fn encode_inter(bw: &mut ByteWriter, cur_frm: &mut NAVideoBuffer<u16>, in_frm: &NAVideoBuffer<u16>, prev_frm: &NAVideoBuffer<u16>, _quality: u8) -> EncoderResult<bool> {
        let mut is_intra = true;
        let src = in_frm.get_data();
        let sstride = in_frm.get_stride(0);
        let soff = in_frm.get_offset(0);
        let (w, h) = in_frm.get_dimensions(0);
        let rsrc = prev_frm.get_data();
        let rstride = prev_frm.get_stride(0);
        let roff = prev_frm.get_offset(0);
        let dstride = cur_frm.get_stride(0);
        let doff = cur_frm.get_offset(0);
        let dst = cur_frm.get_data_mut().unwrap();
        let mut skip_run = 0;
        for ((sstrip, rstrip), dstrip) in (&src[soff..]).chunks(sstride * 4).take(h / 4).zip((&rsrc[roff..]).chunks(rstride * 4)).zip((&mut dst[doff..]).chunks_mut(dstride * 4)) {
            for x in (0..w).step_by(4) {
                let mut buf = [0; 16];
                let mut refbuf = [0; 16];
                Self::get_block(&sstrip[x..], sstride, &mut buf);
                Self::get_block(&rstrip[x..], rstride, &mut refbuf);

                let mut skip_dist = 0;
                for (pix, rpix) in buf.iter().zip(refbuf.iter()) {
                    skip_dist += pix.dist(*rpix);
                }
                if skip_dist == 0 {
                    skip_run += 1;
                    is_intra = false;
                    if skip_run == 1023 {
                        Self::write_skips(bw, skip_run)?;
                        skip_run = 0;
                    }
                    continue;
                }

                let mut bstate = BlockState::default();
                bstate.calc_stats(&buf);

                let dst = &mut dstrip[x..];
                if skip_dist <= bstate.fill_dist {
                    skip_run += 1;
                    is_intra = false;
                    if skip_run == 1023 {
                        Self::write_skips(bw, skip_run)?;
                        skip_run = 0;
                    }
                } else if bstate.fill_dist <= bstate.clr2_dist {
                    bstate.put_fill(dst, dstride);
                    if skip_run != 0 {
                        Self::write_skips(bw, skip_run)?;
                        skip_run = 0;
                    }
                    bstate.write_fill(bw)?;
                } else if bstate.clr8_dist < bstate.clr2_dist {
                    bstate.put_clr8(dst, dstride);
                    if skip_run != 0 {
                        Self::write_skips(bw, skip_run)?;
                        skip_run = 0;
                    }
                    bstate.write_clr8(bw)?;
                } else {
                    bstate.put_clr2(dst, dstride);
                    if skip_run != 0 {
                        Self::write_skips(bw, skip_run)?;
                        skip_run = 0;
                    }
                    bstate.write_clr2(bw)?;
                }
            }
        }
        if skip_run != 0 {
            Self::write_skips(bw, skip_run)?;
        }
        if is_intra {
            bw.write_u16le(0)?;
        } //xxx: something for inter?
        Ok(is_intra)
    }
    fn encode_intra(bw: &mut ByteWriter, cur_frm: &mut NAVideoBuffer<u16>, in_frm: &NAVideoBuffer<u16>, _quality: u8) -> EncoderResult<bool> {
        let src = in_frm.get_data();
        let sstride = in_frm.get_stride(0);
        let soff = in_frm.get_offset(0);
        let (w, h) = in_frm.get_dimensions(0);
        let dstride = cur_frm.get_stride(0);
        let doff = cur_frm.get_offset(0);
        let dst = cur_frm.get_data_mut().unwrap();
        for (sstrip, dstrip) in (&src[soff..]).chunks(sstride * 4).take(h / 4).zip((&mut dst[doff..]).chunks_mut(dstride * 4)) {
            for x in (0..w).step_by(4) {
                let mut buf = [0; 16];
                Self::get_block(&sstrip[x..], sstride, &mut buf);
                let mut bstate = BlockState::default();
                bstate.calc_stats(&buf);

                let dst = &mut dstrip[x..];
                if bstate.fill_dist <= bstate.clr2_dist {
                    bstate.put_fill(dst, dstride);
                    bstate.write_fill(bw)?;
                } else if bstate.clr8_dist < bstate.clr2_dist {
                    bstate.put_clr8(dst, dstride);
                    bstate.write_clr8(bw)?;
                } else {
                    bstate.put_clr2(dst, dstride);
                    bstate.write_clr2(bw)?;
                }
            }
        }
        bw.write_u16le(0)?;
        Ok(true)
    }
}

const RGB555_FORMAT: NAPixelFormaton = NAPixelFormaton {
        model: ColorModel::RGB(RGBSubmodel::RGB), components: 3,
        comp_info: [
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift: 10, comp_offs: 0, next_elem: 2 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  5, comp_offs: 0, next_elem: 2 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  0, comp_offs: 0, next_elem: 2 }),
            None, None],
        elem_size: 2, be: false, alpha: false, palette: false };

impl NAEncoder for MSVideo1Encoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                Ok(EncodeParameters {
                    format: NACodecTypeInfo::Video(NAVideoInfo::new(0, 0, true, RGB555_FORMAT)),
                    ..Default::default() })
            },
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(vinfo) => {
                let outinfo = NAVideoInfo::new((vinfo.width + 3) & !3, (vinfo.height + 3) & !3, true, RGB555_FORMAT);
                let mut ofmt = *encinfo;
                ofmt.format = NACodecTypeInfo::Video(outinfo);
                Ok(ofmt)
            }
        }
    }
    fn init(&mut self, stream_id: u32, encinfo: EncodeParameters) -> EncoderResult<NAStreamRef> {
        match encinfo.format {
            NACodecTypeInfo::None => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(vinfo) => {
                if vinfo.format != RGB555_FORMAT {
                    return Err(EncoderError::FormatError);
                }
                if ((vinfo.width | vinfo.height) & 3) != 0 {
                    return Err(EncoderError::FormatError);
                }

                let out_info = NAVideoInfo::new(vinfo.width, vinfo.height, true, RGB555_FORMAT);
                let info = NACodecInfo::new("msvideo1", NACodecTypeInfo::Video(out_info), None);
                let mut stream = NAStream::new(StreamType::Video, stream_id, info, encinfo.tb_num, encinfo.tb_den, 0);
                stream.set_num(stream_id as usize);
                let stream = stream.into_ref();
                if self.pool.prealloc_video(out_info, 2).is_err() {
                    return Err(EncoderError::AllocError);
                }

                self.stream = Some(stream.clone());
                self.quality = encinfo.quality;

                Ok(stream)
            },
        }
    }
    fn encode(&mut self, frm: &NAFrame) -> EncoderResult<()> {
        let buf = frm.get_buffer();
        if let Some(ref vbuf) = buf.get_vbuf16() {
            let mut cur_frm = self.pool.get_free().unwrap();
            let mut dbuf = Vec::with_capacity(4);
            let mut gw   = GrowableMemoryWriter::new_write(&mut dbuf);
            let mut bw   = ByteWriter::new(&mut gw);
            if self.frmcount == 0 {
                self.lastfrm = None;
            }
            let is_intra = if let Some(ref prev_buf) = self.lastfrm {
                    Self::encode_inter(&mut bw, &mut cur_frm, vbuf, prev_buf, self.quality)?
                } else {
                    Self::encode_intra(&mut bw, &mut cur_frm, vbuf, self.quality)?
                };
            self.lastfrm = Some(cur_frm);
            self.pkt = Some(NAPacket::new(self.stream.clone().unwrap(), frm.ts, is_intra, dbuf));
            self.frmcount += 1;
            if self.frmcount == self.key_int {
                self.frmcount = 0;
            }
            Ok(())
        } else {
            Err(EncoderError::InvalidParameters)
        }
    }
    fn get_packet(&mut self) -> EncoderResult<Option<NAPacket>> {
        let mut npkt = None;
        std::mem::swap(&mut self.pkt, &mut npkt);
        Ok(npkt)
    }
    fn flush(&mut self) -> EncoderResult<()> {
        self.frmcount = 0;
        Ok(())
    }
}

const ENCODER_OPTS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name: KEYFRAME_OPTION, description: KEYFRAME_OPTION_DESC,
        opt_type: NAOptionDefinitionType::Int(Some(0), Some(128)) },
];

impl NAOptionHandler for MSVideo1Encoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { ENCODER_OPTS }
    fn set_options(&mut self, options: &[NAOption]) {
        for option in options.iter() {
            for opt_def in ENCODER_OPTS.iter() {
                if opt_def.check(option).is_ok() {
                    match option.name {
                        KEYFRAME_OPTION => {
                            if let NAValue::Int(intval) = option.value {
                                self.key_int = intval as u8;
                            }
                        },
                        _ => {},
                    };
                }
            }
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match name {
            KEYFRAME_OPTION => Some(NAValue::Int(i64::from(self.key_int))),
            _ => None,
        }
    }
}

pub fn get_encoder() -> Box<dyn NAEncoder + Send> {
    Box::new(MSVideo1Encoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::*;
    use nihav_core::demuxers::*;
    use nihav_core::muxers::*;
    use crate::*;
    use nihav_commonfmt::*;
    use nihav_codec_support::test::enc_video::*;
    use super::RGB555_FORMAT;

    #[test]
    fn test_ms_video1_encoder() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        generic_register_all_decoders(&mut dec_reg);
        ms_register_all_decoders(&mut dec_reg);
        let mut mux_reg = RegisteredMuxers::new();
        generic_register_all_muxers(&mut mux_reg);
        let mut enc_reg = RegisteredEncoders::new();
        ms_register_all_encoders(&mut enc_reg);

        // sample: https://samples.mplayerhq.hu/V-codecs/UCOD/TalkingHead_352x288.avi
        let dec_config = DecoderTestParams {
                demuxer:        "avi",
                in_name:        "assets/Misc/TalkingHead_352x288.avi",
                stream_type:    StreamType::Video,
                limit:          Some(3),
                dmx_reg, dec_reg,
            };
        let enc_config = EncoderTestParams {
                muxer:          "avi",
                enc_name:       "msvideo1",
                out_name:       "msvideo1.avi",
                mux_reg, enc_reg,
            };
        let dst_vinfo = NAVideoInfo {
                width:   0,
                height:  0,
                format:  RGB555_FORMAT,
                flipped: true,
                bits:    16,
            };
        let enc_params = EncodeParameters {
                format:  NACodecTypeInfo::Video(dst_vinfo),
                quality: 0,
                bitrate: 0,
                tb_num:  0,
                tb_den:  0,
                flags:   0,
            };
        //test_encoding_to_file(&dec_config, &enc_config, enc_params, &[]);
        test_encoding_md5(&dec_config, &enc_config, enc_params, &[],
                          &[0x6921e67e, 0x4f2ada95, 0x009ffc62, 0xd4bfab6a]);
    }
}
