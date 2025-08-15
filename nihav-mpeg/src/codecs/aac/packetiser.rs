use nihav_core::codecs::*;
use nihav_core::io::bitreader::*;
use super::info::*;

#[derive(Default,Clone,Copy)]
pub struct ADTSHeader {
    pub srate:      u32,
    pub channels:   u8,
    pub hdr_size:   usize,
    pub frame_size: usize,
    pub samples:    usize,
    pub hdr:        u32,
}

impl PartialEq for ADTSHeader {
    fn eq(&self, other: &Self) -> bool {
        self.hdr == other.hdr
    }
}

impl ADTSHeader {
    pub fn parse(src: &[u8]) -> DecoderResult<ADTSHeader> {
        if src.len() < 7 { return Err(DecoderError::ShortData); }

        let mut br = BitReader::new(src, BitReaderMode::BE);

        let syncword                = br.read(12)?;
        if syncword != 0xFFF { return Err(DecoderError::InvalidData); }
        let _id                     = br.read(1)?;
        let layer                   = br.read(2)?;
        if layer != 0 { return Err(DecoderError::InvalidData); }
        let protection_absent       = br.read_bool()?;
        let profile                 = br.read(2)?;
        validate!(profile != 3);
        let sf_idx                  = br.read(4)?;
        validate!(sf_idx <= 12);
        let _private_bit            = br.read_bool()?;
        let ch_config               = br.read(3)?;
        validate!(ch_config != 0);
        let _original               = br.read_bool()?;
        let _home                   = br.read_bool()?;

        let _copy_id_bit            = br.read_bool()?;
        let _copy_id_start          = br.read_bool()?;
        let frame_length            = br.read(13)? as usize;
        let _buffer_fullness        = br.read(11)?;
        let num_raw_blocks          = br.read(2)? as usize;

        let hdr = ((profile + 1) << 27) | (sf_idx << 23) | (ch_config << 19);

        if num_raw_blocks == 0 {
            if !protection_absent {
                br.skip(16)?; // CRC
            }
            let hdr_size = br.tell() / 8;
            validate!(frame_length > hdr_size);
            let frame_size = frame_length - hdr_size;

            Ok(ADTSHeader {
                srate:    AAC_SAMPLE_RATES[sf_idx as usize],
                channels: AAC_CHANNELS[ch_config as usize] as u8,
                hdr_size, frame_size, hdr,
                samples: 1024
            })
        } else {
            Err(DecoderError::NotImplemented)
        }
    }
}

#[derive(Default)]
struct PacketiserADTS {
    buf:        Vec<u8>,
    hdr:        Option<ADTSHeader>,
}

impl PacketiserADTS {
    fn new() -> Self { Self::default() }
    fn skip_id3(buf: &[u8]) -> DecoderResult<usize> {
        // check for ID3 tags in concatenated stream fragments
        if &buf[0..3] == b"ID3" && (1..=4).contains(&buf[3]) && buf[4] == 0 {
            if buf.len() < 10 {
                return Err(DecoderError::ShortData);
            }
            let mut size = 0;
            for &b in buf[6..10].iter() {
                if (b & 0x80) != 0 {
                    return Err(DecoderError::InvalidData);
                }
                size = (size << 7) | usize::from(b);
            }
            Ok(size + 10)
        } else {
            Ok(0)
        }
    }
}

impl NAPacketiser for PacketiserADTS {
    fn attach_stream(&mut self, _stream: NAStreamRef) {}
    fn add_data(&mut self, src: &[u8]) -> bool {
        self.buf.extend_from_slice(src);
        self.buf.len() < 65536
    }
    fn parse_stream(&mut self, id: u32) -> DecoderResult<NAStreamRef> {
        if self.hdr.is_none() {
            if self.buf.len() < 7 {
                return Err(DecoderError::ShortData);
            }
            let id3_len = Self::skip_id3(&self.buf)?;
            if id3_len + 7 > self.buf.len() {
                return Err(DecoderError::ShortData);
            }
            let hdr = ADTSHeader::parse(&self.buf[id3_len..])?;
            self.hdr = Some(hdr);
        }
        let hdr = self.hdr.unwrap();
        let ainfo = NAAudioInfo::new(hdr.srate, hdr.channels, SND_F32P_FORMAT, hdr.samples);
        let edata = vec![(hdr.hdr >> 24) as u8, (hdr.hdr >> 16) as u8];
        let info = NACodecInfo::new("aac", NACodecTypeInfo::Audio(ainfo), Some(edata));
        Ok(NAStream::new(StreamType::Audio, id, info, hdr.samples as u32, hdr.srate, 0).into_ref())
    }
    fn skip_junk(&mut self) -> DecoderResult<usize> {
        if self.buf.len() <= 8 {
            return Ok(0);
        }
        let mut off = 0;
        let mut hdr = u16::from(self.buf[0]) * 256 + u16::from(self.buf[1]);
        let mut iter = self.buf[2..].iter();
        loop {
            if (hdr & 0xFFF6) == 0xFFF6 {
                let ret = ADTSHeader::parse(&self.buf[off..]);
                match ret {
                    Ok(hdr) => {
                        if self.hdr.is_none() {
                            self.hdr = Some(hdr);
                        }
                        if self.hdr.unwrap() != hdr { // header is valid but mismatches
                            self.buf.drain(..off + 1);
                            return Err(DecoderError::InvalidData);
                        }
                        break;
                    },
                    Err(err) => {
                        self.buf.drain(..off + 1);
                        return Err(err);
                    },
                };
            }
            off += 1;
            if let Some(&b) = iter.next() {
                hdr = (hdr << 8) | u16::from(b);
            } else {
                break;
            }
        }
        self.buf.drain(..off);
        Ok(off)
    }
    fn get_packet(&mut self, stream: NAStreamRef) -> DecoderResult<Option<NAPacket>> {
        if self.buf.len() < 8 {
            return Err(DecoderError::ShortData);
        }

        let id3_len = Self::skip_id3(&self.buf)?;
        if id3_len + 8 <= self.buf.len() {
            self.buf.drain(..id3_len);
        } else {
            return Err(DecoderError::ShortData);
        }

        let hdr = ADTSHeader::parse(&self.buf)?;
        if self.hdr.is_none() {
            self.hdr = Some(hdr);
        }
        if self.hdr.unwrap() != hdr {
            return Err(DecoderError::InvalidData);
        }
        if hdr.frame_size + hdr.hdr_size <= self.buf.len() {
            let mut data = Vec::with_capacity(hdr.frame_size);
            data.extend_from_slice(&self.buf[hdr.hdr_size..][..hdr.frame_size]);
            self.buf.drain(..hdr.hdr_size + hdr.frame_size);
            let ts = NATimeInfo::new(None, None, Some(1), hdr.samples as u32, hdr.srate);
            Ok(Some(NAPacket::new(stream, ts, true, data)))
        } else {
            Ok(None)
        }
    }
    fn reset(&mut self) {
        self.buf.clear();
    }
    fn bytes_left(&self) -> usize { self.buf.len() }
}

pub fn get_packetiser_adts() -> Box<dyn NAPacketiser + Send> {
    Box::new(PacketiserADTS::new())
}
