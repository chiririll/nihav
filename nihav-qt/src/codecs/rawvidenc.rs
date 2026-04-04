use nihav_core::codecs::*;
use std::str::FromStr;

#[derive(Clone,Copy,Debug,PartialEq)]
enum RawType {
    YUV2,
    YUV4,
}

struct RawEncoder {
    stream: Option<NAStreamRef>,
    pkt:    Option<NAPacket>,
    vinfo:  NAVideoInfo,
    rtype:  RawType,
}

impl RawEncoder {
    fn new(rtype: RawType) -> Self {
        Self {
            stream: None,
            pkt:    None,
            vinfo:  NAVideoInfo::new(0, 0, false, YUV420_FORMAT),
            rtype,
        }
    }
}

impl NAEncoder for RawEncoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                let fmt = match self.rtype {
                        RawType::YUV2 => NAPixelFormaton::from_str("yuv422p").unwrap(),
                        RawType::YUV4 => YUV420_FORMAT,
                    };
                Ok(EncodeParameters {
                    format: NACodecTypeInfo::Video(NAVideoInfo::new(0, 0, false, fmt)),
                    ..Default::default()
                })
            },
            NACodecTypeInfo::Video(_) => {
                let fmt = match self.rtype {
                        RawType::YUV2 => NAPixelFormaton::from_str("yuv422p").unwrap(),
                        RawType::YUV4 => YUV420_FORMAT,
                    };
                let mut info = *encinfo;
                if let NACodecTypeInfo::Video(ref mut vinfo) = info.format {
                    vinfo.format = fmt;
                    vinfo.width = (vinfo.width + 1) & !1;
                    if self.rtype == RawType::YUV4 {
                        vinfo.height = (vinfo.height + 1) & !1;
                    }
                } else {
                    return Err(EncoderError::FormatError);
                }
                Ok(info)
            },
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
        }
    }
    fn get_capabilities(&self) -> u64 { ENC_CAPS_CBR }
    fn init(&mut self, stream_id: u32, encinfo: EncodeParameters) -> EncoderResult<NAStreamRef> {
        match encinfo.format {
            NACodecTypeInfo::None => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(vinfo) => {
                self.vinfo = vinfo;
                let (name, fmt) = match self.rtype {
                        RawType::YUV2 => ("qt-yuv2", NAPixelFormaton::from_str("yuv422p").unwrap()),
                        RawType::YUV4 => ("qt-yuv4", YUV420_FORMAT),
                    };
                if vinfo.format != fmt || (vinfo.width & 1) != 0 || (name == "qt-yuv4" && (vinfo.height & 1) != 0) {
                    return Err(EncoderError::FormatError);
                }
                let info = NACodecInfo::new(name, encinfo.format, None);
                let mut stream = NAStream::new(StreamType::Video, stream_id, info, encinfo.tb_num, encinfo.tb_den, 0);
                stream.set_num(stream_id as usize);
                let stream = stream.into_ref();
                self.stream = Some(stream.clone());
                Ok(stream)
            }
        }
    }
    fn encode(&mut self, frm: &NAFrame) -> EncoderResult<()> {
        let buf = frm.get_buffer();
        let mut dbuf;
        if let Some(vinfo) = buf.get_video_info() {
            if vinfo != self.vinfo {
                println!("Input format differs from the initial one");
                return Err(EncoderError::FormatError);
            }
        }
        if let NABufferType::Video(ref vbuf) = buf {
            let vinfo = vbuf.get_info();
            let src = vbuf.get_data();
            let yoff = vbuf.get_offset(0);
            let uoff = vbuf.get_offset(1);
            let voff = vbuf.get_offset(2);
            let ystride = vbuf.get_stride(0);
            let ustride = vbuf.get_stride(1);
            let vstride = vbuf.get_stride(2);
            match self.rtype {
                RawType::YUV2 => {
                    dbuf = Vec::with_capacity(vinfo.width * vinfo.height * 2);
                    for (yline, (uline, vline)) in src[yoff..].chunks(ystride)
                            .zip(src[uoff..].chunks(ustride).zip(src[voff..].chunks(vstride)))
                            .take(vinfo.height) {
                        for (y2, (&u, &v)) in yline.chunks_exact(2).zip(uline.iter().zip(vline.iter())) {
                            dbuf.push(y2[0]);
                            dbuf.push(u ^ 0x80);
                            dbuf.push(y2[1]);
                            dbuf.push(v ^ 0x80);
                        }
                    }
                },
                RawType::YUV4 => {
                    dbuf = Vec::with_capacity(vinfo.width * vinfo.height * 3 / 2);
                    for (ylines, (uline, vline)) in src[yoff..].chunks(ystride * 2)
                            .zip(src[uoff..].chunks(ustride).zip(src[voff..].chunks(vstride)))
                            .take(vinfo.height) {
                        let (yline0, yline1) = ylines.split_at(ystride);
                        for ((y0, y1), (&u, &v)) in yline0.chunks_exact(2).zip(yline1.chunks_exact(2))
                                .zip(uline.iter().zip(vline.iter())) {
                            dbuf.push(u ^ 0x80);
                            dbuf.push(v ^ 0x80);
                            dbuf.push(y0[0]);
                            dbuf.push(y0[1]);
                            dbuf.push(y1[0]);
                            dbuf.push(y1[1]);
                        }
                    }
                },
            }
        } else {
            return Err(EncoderError::FormatError);
        }
        self.pkt = Some(NAPacket::new(self.stream.clone().unwrap(), frm.ts, true, dbuf));
        Ok(())
    }
    fn get_packet(&mut self) -> EncoderResult<Option<NAPacket>> {
        let mut npkt = None;
        std::mem::swap(&mut self.pkt, &mut npkt);
        Ok(npkt)
    }
    fn flush(&mut self) -> EncoderResult<()> {
        Ok(())
    }
}

impl NAOptionHandler for RawEncoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_encoder_yuv2() -> Box<dyn NAEncoder + Send> {
    Box::new(RawEncoder::new(RawType::YUV2))
}
pub fn get_encoder_yuv4() -> Box<dyn NAEncoder + Send> {
    Box::new(RawEncoder::new(RawType::YUV4))
}
