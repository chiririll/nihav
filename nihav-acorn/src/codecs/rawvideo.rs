use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_core::io::bitreader::*;

use super::RGB555_FORMAT;
use super::yuvtab::YUV2RGB;

const YUV422_FORMAT: NAPixelFormaton = NAPixelFormaton { model: ColorModel::
YUV(YUVSubmodel::YUVJ), components: 3,
        comp_info: [
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: false, depth: 8, shift: 0, comp_offs: 0, next_elem: 1}),
            Some(NAPixelChromaton{ h_ss: 1, v_ss: 0, packed: false, depth: 8, shift: 0, comp_offs: 1, next_elem: 1}),
            Some(NAPixelChromaton{ h_ss: 1, v_ss: 0, packed: false, depth: 8, shift: 0, comp_offs: 2, next_elem: 1}),
            None, None],
        elem_size: 0, be: false, alpha: false, palette: false };


trait ReadYUV5 {
    fn read_y(&mut self) -> DecoderResult<u8>;
    fn read_uv(&mut self) -> DecoderResult<u8>;
}

impl<'a> ReadYUV5 for BitReader<'a> {
    fn read_y(&mut self) -> DecoderResult<u8> {
        let v = self.read(5)? as u8;
        Ok((v << 3) | (v >> 2))
    }
    fn read_uv(&mut self) -> DecoderResult<u8> {
        const EXPAND: [u8; 16] = [
            0x00, 0x08, 0x11, 0x19, 0x22, 0x2A, 0x33, 0x3B, 0x44, 0x4C, 0x55, 0x5D, 0x66, 0x6E, 0x77, 0x7F];
        let v = self.read(5)? as u8;
        if v < 16 {
            Ok(EXPAND[v as usize] | 0x80)
        } else {
            Ok(EXPAND[(v & 0xF) as usize])
        }
    }
}

#[derive(Default)]
struct RawDecoder {
    info:           NACodecInfoRef,
    width:          usize,
    height:         usize,
    is_yuv:         bool,
    codec_id:       u16,
}

impl RawDecoder {
    fn new() -> Self { Self::default() }
}

impl NADecoder for RawDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.get_width();
            self.height = vinfo.get_height();
            validate!(info.get_extradata().is_some());

            if let Some(edata) = info.get_extradata() {
                validate!(edata.len() > 1);
                self.codec_id = u16::from(edata[0]) + 256 * u16::from(edata[1]);

                for triplet in edata.windows(3) {
                    if triplet == b"YUV" {
                        self.is_yuv = true;
                        break;
                    }
                }
            } else {
                return Err(DecoderError::InvalidData);
            }

            let fmt = match self.codec_id {
                    2 => RGB555_FORMAT,
                    3 if self.is_yuv => {
                        validate!((self.width  & 1) == 0);
                        YUV422_FORMAT
                    },
                    5 if self.is_yuv => {
                        validate!((self.width  & 1) == 0);
                        validate!((self.height & 1) == 0);
                        YUV420_FORMAT
                    },
                    _ => return Err(DecoderError::NotImplemented),
                };

            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(vinfo.get_width(), vinfo.get_height(), false, fmt));
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
        match self.codec_id {
            2 => {
                let mut mr = MemoryReader::new_read(&src);
                let mut br = ByteReader::new(&mut mr);

                let mut buf = bufinfo.get_vbuf16().unwrap();
                let stride = buf.get_stride(0);
                let data = buf.get_data_mut().unwrap();

                for dline in data.chunks_exact_mut(stride).take(self.height) {
                    for el in dline[..self.width].iter_mut().take(self.width) {
                        *el = br.read_u16le()?;
                        if self.is_yuv {
                            *el = YUV2RGB[(*el as usize) & 0x7FFF];
                        }
                    }
                }
            },
            3 => {
                let mut br = BitReader::new(&src, BitReaderMode::LE);
                let mut buf = bufinfo.get_vbuf().unwrap();
                let dst = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();

                let mut yoff = dst.offset[0];
                let mut uoff = dst.offset[1];
                let mut voff = dst.offset[2];
                for _y in 0..self.height {
                    for x in (0..self.width).step_by(2) {
                        dst.data[yoff + x]     = br.read_y()?;
                        dst.data[yoff + x + 1] = br.read_y()?;
                        dst.data[uoff + x / 2] = br.read_uv()?;
                        dst.data[voff + x / 2] = br.read_uv()?;
                    }
                    yoff += dst.stride[0];
                    uoff += dst.stride[1];
                    voff += dst.stride[2];
                }
            },
            5 => {
                let mut br = BitReader::new(&src, BitReaderMode::LE);
                let mut buf = bufinfo.get_vbuf().unwrap();
                let dst = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();

                let mut yoff = dst.offset[0];
                let mut uoff = dst.offset[1];
                let mut voff = dst.offset[2];
                for _y in (0..self.height).step_by(2) {
                    for x in (0..self.width).step_by(2) {
                        dst.data[yoff + x]                     = br.read_y()?;
                        dst.data[yoff + x + 1]                 = br.read_y()?;
                        dst.data[yoff + x + dst.stride[0]]     = br.read_y()?;
                        dst.data[yoff + x + dst.stride[0] + 1] = br.read_y()?;
                        dst.data[uoff + x / 2]                 = br.read_uv()?;
                        dst.data[voff + x / 2]                 = br.read_uv()?;
                                                                 br.skip(2)?;
                    }
                    yoff += dst.stride[0] * 2;
                    uoff += dst.stride[1];
                    voff += dst.stride[2];
                }
            },
            _ => unreachable!(),
        }

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(true);
        frm.set_frame_type(FrameType::I);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {}
}

impl NAOptionHandler for RawDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(RawDecoder::new())
}

#[derive(Default)]
struct RawPacketiser {
    stream:     Option<NAStreamRef>,
    buf:        Vec<u8>,
    frameno:    u32,
    size:       usize,
}

impl RawPacketiser {
    fn new() -> Self { Self::default() }
}

impl NAPacketiser for RawPacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
        let vinfo = stream.get_info().get_properties().get_video_info().unwrap();
        let width  = vinfo.width;
        let height = vinfo.height;
        if let Some(edata) = stream.get_info().get_extradata() {
            if edata.len() > 1 {
                let codec_id = u16::from(edata[0]) + 256 * u16::from(edata[1]);
                /*let mut is_yuv = false;
                for triplet in edata.windows(3) {
                    if triplet == b"YUV" {
                        is_yuv = true;
                        break;
                    }
                }*/
                self.size = match codec_id {
                        2 => width * height * 2,
                        3 => width * height * 10 / 8,
                        5 => width * height,
                        _ => unimplemented!(),
                    };
            }
        }
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
        if self.size == 0 {
            return Err(DecoderError::MissingReference);
        }
        if self.buf.len() < self.size {
            return Ok(None);
        }

        let mut data = Vec::with_capacity(self.size);
        data.extend_from_slice(&self.buf[..self.size]);
        self.buf.drain(..self.size);

        let ts = NATimeInfo::new(Some(u64::from(self.frameno)), None, None, stream.tb_num, stream.tb_den);
        self.frameno += 1;

        Ok(Some(NAPacket::new(stream, ts, true, data)))
    }
    fn reset(&mut self) {
        self.buf.clear();
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

pub fn get_packetiser() -> Box<dyn NAPacketiser + Send> {
    Box::new(RawPacketiser::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::{RegisteredDecoders, RegisteredPacketisers};
    use nihav_core::demuxers::RegisteredRawDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;

    #[test]
    fn test_format2() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Acorn Replay Demonstration Disc 2
        test_decoding_raw("armovie", "arm_rawvideo", "assets/Acorn/ROBIN2", Some(1),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x9a452976, 0x5fa64428, 0x71172412, 0x6db21372],
                                [0xabc70d88, 0x2431a96b, 0xfc8d58a6, 0xef1bb1c9]]));
    }

    #[test]
    fn test_format3() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Cine Clips by Oregan Software Developments
        test_decoding_raw("armovie", "arm_rawvideo", "assets/Acorn/TROPICLSUN", Some(1),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0xcd5fe3d0, 0x60454448, 0x9f91180a, 0x8e73370d],
                                [0x148b07bb, 0xbf647ddd, 0x2bf8c9e5, 0x4b37122a]]));
    }

    #[test]
    fn test_format5() {
        let mut dmx_reg = RegisteredRawDemuxers::new();
        acorn_register_all_raw_demuxers(&mut dmx_reg);
        let mut pkt_reg = RegisteredPacketisers::new();
        acorn_register_all_packetisers(&mut pkt_reg);
        let mut dec_reg = RegisteredDecoders::new();
        acorn_register_all_decoders(&mut dec_reg);

        // a sample from Empire video editor demo
        test_decoding_raw("armovie", "arm_rawvideo", "assets/Acorn/CLIP3", Some(1),
                          &dmx_reg, &pkt_reg, &dec_reg,
                          ExpectedTestResult::MD5Frames(vec![
                                [0x816ccb08, 0x5e86539c, 0x1bb51e98, 0x849936c4],
                                [0xa42cf122, 0x296f3825, 0xedb7f0fc, 0x25a7825e]]));
    }
}
