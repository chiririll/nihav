use nihav_core::codecs::*;
use nihav_core::io::byteio::*;

struct Palette {
    data:       [u8; 1024],
}

impl Default for Palette {
    fn default() -> Self {
        Self { data: [0; 1024] }
    }
}

impl Palette {
    fn read_data(&mut self, src: &[u8]) {
        for (dst, src) in self.data.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let clr = read_u32le(src).unwrap();
            let r = (clr & 0x1F) as u8;
            dst[0] = (r << 3) | (r >> 2);
            let g = ((clr >> 5) & 0x1F) as u8;
            dst[1] = (g << 3) | (g >> 2);
            let b = ((clr >> 10) & 0x1F) as u8;
            dst[2] = (b << 3) | (b >> 2);
            dst[3] = 0;
        }
    }
    fn get_side_data(&self) -> NASideData {
        NASideData::Palette(true, Arc::new(self.data))
    }
}

#[derive(Default)]
struct WssPacketiser {
    stream:     Option<NAStreamRef>,
    buf:        Vec<u8>,
    csizes:     Vec<usize>,
    frameno:    u32,
    frame_size: usize,
    palette:    Palette,
    pal_mode:   bool,
    has_pal:    bool,
}

impl WssPacketiser {
    fn new() -> Self { Self::default() }
    fn remove_payload(&mut self, size: usize) {
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
    }
}

impl NAPacketiser for WssPacketiser {
    fn attach_stream(&mut self, stream: NAStreamRef) {
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
        loop {
            if self.buf.len() < 4 || self.buf.len() < self.frame_size {
                return Ok(None);
            }

            if self.frame_size == 0 {
                self.frame_size = read_u32le(&self.buf)? as usize;
                if self.frame_size == 0 {
                    self.remove_payload(4);
                    self.frameno += 1;
                    continue;
                }
                if self.frame_size == 0xFFFFFFFF {
                    self.pal_mode = true;
                    self.frame_size = 1024;
                }
                self.frame_size += 4;
                validate!(self.frame_size > 16);
                if self.buf.len() >= self.frame_size {
                    if self.pal_mode {
                        self.palette.read_data(&self.buf[4..][..1024]);
                        self.remove_payload(self.frame_size);
                        self.pal_mode = false;
                        self.has_pal = true;
                        self.frame_size = 0;
                        continue;
                    }
                    break;
                }
            }
        }

        let mut data = Vec::with_capacity(self.frame_size);
        data.extend_from_slice(&self.buf[16..self.frame_size]);
        self.remove_payload(self.frame_size);

        let mut is_intra = false;
        if let Some(ref stream) = self.stream {
            #[allow(clippy::single_match)]
            match stream.get_info().get_name() {
                "cinepak" => is_intra = (data[0] & 1) == 0,
                _ => {},
            };
        }

        let ts = NATimeInfo::new(Some(u64::from(self.frameno)), None, None, stream.tb_num, stream.tb_den);
        self.frameno += 1;
        self.frame_size = 0;

        let mut pkt = NAPacket::new(stream, ts, is_intra, data);
        if self.has_pal {
            pkt.add_side_data(self.palette.get_side_data());
        }

        Ok(Some(pkt))
    }
    fn reset(&mut self) {
        self.buf.clear();
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

#[cfg(feature="packetiser_cinepak")]
pub fn get_packetiser_cinepak() -> Box<dyn NAPacketiser + Send> {
    Box::new(WssPacketiser::new())
}

#[cfg(feature="packetiser_msvideo1")]
pub fn get_packetiser_msvideo1() -> Box<dyn NAPacketiser + Send> {
    Box::new(WssPacketiser::new())
}
