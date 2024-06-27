use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_core::compr::lz_copy;

const FRAME_HEADER: usize = 24;

struct RobotVideoDecoder {
    info:       NACodecInfoRef,
    pal:        [u8; 768],
    frame:      Vec<u8>,
    cell_buf:   Vec<u8>,
    width:      usize,
    height:     usize,
    version:    u8,
}

impl RobotVideoDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfoRef::default(),
            pal:        [0; 768],
            frame:      Vec::new(),
            cell_buf:   Vec::new(),
            width:      0,
            height:     0,
            version:    0,
        }
    }
}

struct BitReader<'a, 'b> {
    br:     &'a mut ByteReader<'b>,
    end:    u64,
    bbuf:   u32,
    bits:   u8,
}

impl<'a, 'b> BitReader<'a, 'b> {
    fn new(br: &'a mut ByteReader<'b>, size: usize) -> Self {
        let end = br.tell() + (size as u64);
        Self {
            br, end,
            bbuf:   0,
            bits:   0,
        }
    }
    fn refill(&mut self) -> DecoderResult<()> {
        while self.bits <= 24 && self.br.tell() < self.end {
            self.bbuf |= u32::from(self.br.read_byte()?) << (24 - self.bits);
            self.bits += 8;
        }
        Ok(())
    }
    fn read_bit(&mut self) -> DecoderResult<bool> {
        self.refill()?;
        if self.bits == 0 { return Err(DecoderError::ShortData); }
        let bit = (self.bbuf >> 31) != 0;
        self.bbuf <<= 1;
        self.bits  -= 1;
        Ok(bit)
    }
    fn read(&mut self, nbits: u8) -> DecoderResult<u32> {
        self.refill()?;
        if self.bits < nbits { return Err(DecoderError::ShortData); }
        let ret = self.bbuf >> (32 - nbits);
        self.bbuf <<= nbits;
        self.bits  -= nbits;
        Ok(ret)
    }
}

fn lzs_unpack(br: &mut ByteReader, csize: usize, dst: &mut [u8]) -> DecoderResult<()> {
    let mut br = BitReader::new(br, csize);

    let mut dpos = 0;
    loop {
        if br.read_bit()? {
            let offset = (if br.read_bit()? {
                    let off = br.read(7)?;
                    if off == 0 {
                        validate!(dpos == dst.len());
                        return Ok(());
                    }
                    off
                } else {
                    br.read(11)?
                }) as usize;

            let mut len = br.read(2)?;
            if len < 3 {
                len += 2;
            } else {
                len = br.read(2)?;
                if len < 3 {
                    len += 5;
                } else {
                    len = 8;
                    loop {
                        let t = br.read(4)?;
                        len += t;
                        if t != 0xF {
                            break;
                        }
                    }
                }
            }
            let len = len as usize;

            validate!(offset <= dpos);
            validate!(dpos + len <= dst.len());
            lz_copy(dst, dpos, offset, len);
            dpos += len;
        } else {
            dst[dpos] = br.read(8)? as u8;
            dpos += 1;
        }
    }
}

fn unpack_cell(br: &mut ByteReader, cell_size: usize, nchunks: usize, dst: &mut Vec<u8>, limit: usize) -> DecoderResult<()> {
    let mut data_left = cell_size;
    dst.clear();
    dst.reserve(limit);
    for _ in 0..nchunks {
        validate!(data_left >= 10);
        let csize       = br.read_u32le()? as usize;
        validate!(csize <= data_left);
        let rsize       = br.read_u32le()? as usize;
        validate!(rsize + dst.len() <= limit);
        let method      = br.read_u16le()?;

        data_left -= 10;

        let cur_size = dst.len();
        dst.resize(cur_size + rsize, 0);
        match method {
            0 => { lzs_unpack(br, csize, &mut dst[cur_size..])?; },
            2 => {
                validate!(rsize == csize);
                br.read_buf(&mut dst[cur_size..])?;
            },
            _ => return Err(DecoderError::NotImplemented),
        }
        data_left -= csize;
    }
    Ok(())
}

fn blit(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize) {
    for (dline, sline) in dst.chunks_mut(dstride).zip(src.chunks(sstride)) {
        dline[..sstride].copy_from_slice(sline);
    }
}

fn blit_scaled(dst: &mut [u8], dstride: usize, src: &[u8], sstride: usize, scale: u8) {
    let mut slines = src.chunks(sstride);
    let mut acc = 0;

    let mut cur_line = slines.next().unwrap();

    for dline in dst.chunks_mut(dstride) {
        dline[..sstride].copy_from_slice(cur_line);
        acc += scale;
        if acc >= 100 {
            acc -= 100;
            if let Some(line) = slines.next() {
                cur_line = line;
            } else {
                break;
            }
        }
    }
}

impl NADecoder for RobotVideoDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.get_width();
            self.height = vinfo.get_height();
            self.frame.resize(self.width * self.height, 0);
            if let Some(ref edata) = info.get_extradata() {
                validate!(edata.len() > 2);
                self.version = edata[0];
                validate!(self.version >= 4 && self.version <= 6);
                if edata[1] != 0 {
                    validate!(edata.len() > 39);
                    let pal_start = read_u16le(&edata[25+2..])? as usize;
                    let pal_len   = read_u16le(&edata[29+2..])? as usize;
                    validate!(pal_len > 0 && pal_start + pal_len <= 256);
                    match edata[32+2] {
                        0 => {
                            let dpal = self.pal[pal_start * 3..].chunks_exact_mut(3);
                            for (dst, quad) in dpal.zip(edata[37+2..].chunks_exact(4)) {
                                dst.copy_from_slice(&quad[1..]);
                            }
                        },
                        1 => self.pal[pal_start * 3..][..pal_len * 3].copy_from_slice(&edata[37+2..][..pal_len * 3]),
                        _ => return Err(DecoderError::NotImplemented),
                    }
                } else {
                    for (i, entry) in self.pal.chunks_exact_mut(3).enumerate() {
                        entry[0] = i as u8;
                        entry[1] = i as u8;
                        entry[2] = i as u8;
                    }
                }
            } else {
                return Err(DecoderError::InvalidData);
            }
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, PAL8_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > FRAME_HEADER);

        let mut mr = MemoryReader::new_read(&src);
        let mut br = ByteReader::new(&mut mr);

        let ncells              = br.read_u16le()? as usize;
        validate!(ncells > 0 && ncells <= 10);
        for el in self.frame.iter_mut() { *el = 0xFF; }
        for _ in 0..ncells {
                                  br.read_byte()?;
            let scale           = br.read_byte()?;
            let width           = br.read_u16le()? as usize;
            let height          = br.read_u16le()? as usize;
                                  br.read_skip(4)?;
            let xoff            = br.read_u16le()? as usize;
            let yoff            = br.read_u16le()? as usize;
            validate!(xoff + width <= self.width && yoff + height <= self.height);
            let mut cell_size   = br.read_u16le()? as usize;
            // hack
            if self.version == 6 && ncells == 1 && (src.len() - 18 >= 0x10000) {
                cell_size += (src.len() - 18) & !0xFFFF;
            }
            if self.version > 4 {
                let nchunks     = br.read_u16le()? as usize;
                validate!(nchunks > 0);
                                  br.read_skip(4)?;

                unpack_cell(&mut br, cell_size, nchunks, &mut self.cell_buf, width * height)?;
            } else {
                                  br.read_skip(6)?;
                self.cell_buf.resize(width * height, 0);
                lzs_unpack(&mut br, cell_size, &mut self.cell_buf)?;
            }
            if scale == 100 {
                blit(&mut self.frame[xoff + yoff * self.width..], self.width, &self.cell_buf, width);
            } else {
                blit_scaled(&mut self.frame[xoff + yoff * self.width..], self.width, &self.cell_buf, width, scale);
            }
        }

        let buf = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;
        let mut vbuf = buf.get_vbuf().unwrap();
        let paloff = vbuf.get_offset(1);
        let stride = vbuf.get_stride(0);
        let data = vbuf.get_data_mut().unwrap();

        blit(data, stride, &self.frame, self.width);
        data[paloff..][..768].copy_from_slice(&self.pal);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buf);
        let ftype = if pkt.keyframe { FrameType::I } else { FrameType::P };
        frm.set_frame_type(ftype);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for RobotVideoDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(RobotVideoDecoder::new())
}

struct RobotAudioDecoder {
    info:       NACodecInfoRef,
    ainfo:      NAAudioInfo,
    chmap:      NAChannelMap,
}

impl RobotAudioDecoder {
    fn new() -> Self {
        Self {
            ainfo:  NAAudioInfo::new(11025, 1, SND_S16_FORMAT, 1),
            info:   NACodecInfo::new_dummy(),
            chmap:  NAChannelMap::from_str("C").unwrap(),
        }
    }
    fn pred16(pred: i32, val: u8) -> i32 {
        if (val & 0x80) != 0 {
            pred - i32::from(SOL_AUD_STEPS16[(val & 0x7F) as usize])
        } else {
            pred + i32::from(SOL_AUD_STEPS16[(val & 0x7F) as usize])
        }
    }
}

impl NADecoder for RobotAudioDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Audio(_ainfo) = info.get_properties() {
            self.info = info.replace_info(NACodecTypeInfo::Audio(self.ainfo));

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() > 1);

        let samples = match src[0] {
            0 => src.len() - 1,
            1 => src.len() - 1 - 8,
            _ => return Err(DecoderError::InvalidData),
        };

        let abuf = alloc_audio_buffer(self.ainfo, samples / 2, self.chmap.clone())?;
        let mut adata = abuf.get_abuf_i16().unwrap();
        let dst = adata.get_data_mut().unwrap();

        match src[0] {
            0 => {
                let mut pred = 0;
                for (dst, &b) in dst.iter_mut().zip(src[1..][..src.len()/2].iter()) {
                    pred = Self::pred16(pred, b);
                    *dst = pred as i16;
                }
            },
            1 => {
                validate!(src.len() > 8);
                let mut pred = 0;
                for &b in src[1..9].iter() {
                    pred = Self::pred16(pred, b);
                }
                for (dst, &b) in dst.iter_mut().zip(src[9..].iter()) {
                    pred = Self::pred16(pred, b);
                    *dst = pred as i16;
                }
            },
            _ => unreachable!(),
        }

        let frm = NAFrame::new_from_pkt(pkt, self.info.clone(), abuf);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for RobotAudioDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder_audio() -> Box<dyn NADecoder + Send> {
    Box::new(RobotAudioDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    #[test]
    fn test_rbt_v4() {
        let mut dmx_reg = RegisteredDemuxers::new();
        game_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from SWAT demo
        test_decoding("sierra-rbt", "rbt-video", "assets/Game/sierra/12.rbt",
                      Some(2), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x2a00775d, 0xef8da06a, 0x015b6f06, 0xa22d0158],
                            [0xf2acb558, 0x0d9c5c54, 0x32c43af4, 0xd9776b68],
                            [0x386e02e9, 0x76dbd5a6, 0x4e9da3d7, 0xa47fdca3]]));
    }
    #[test]
    fn test_rbt_v5() {
        let mut dmx_reg = RegisteredDemuxers::new();
        game_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from Phantasmagora (with scaling)
        test_decoding("sierra-rbt", "rbt-video", "assets/Game/sierra/162.RBT",
                      None, &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0x4912fa8f, 0xae201d9e, 0x59707ea0, 0xc50bf0e2]));
    }
    #[test]
    fn test_rbt_v6() {
        let mut dmx_reg = RegisteredDemuxers::new();
        game_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from Rama
        test_decoding("sierra-rbt", "rbt-video", "assets/Game/sierra/7531.RBT",
                      Some(2), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x49db87f3, 0x57881095, 0x676d1600, 0x5ddaa50b],
                            [0xa75ff558, 0xb6815b27, 0x5f9d872f, 0xd7f56470],
                            [0x60bca745, 0xc47d6882, 0xc193fe70, 0x7b8738c9]]));
    }
    #[test]
    fn test_rbt_audio() {
        let mut dmx_reg = RegisteredDemuxers::new();
        game_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        game_register_all_decoders(&mut dec_reg);

        // sample from Space Quest 6
        test_decoding("sierra-rbt", "rbt-audio", "assets/Game/sierra/410.rbt",
                      Some(2), &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5([0xdd44e3ca, 0x6cfc1bc1, 0xdcd4214a, 0x443cf5ed]));
    }
}

const SOL_AUD_STEPS16: [i16; 128] = [
     0x00,   0x08,   0x10,   0x20,   0x30,   0x40,   0x50,   0x60,
     0x70,   0x80,   0x90,   0xA0,   0xB0,   0xC0,   0xD0,   0xE0,
     0xF0,  0x100,  0x110,  0x120,  0x130,  0x140,  0x150,  0x160,
    0x170,  0x180,  0x190,  0x1A0,  0x1B0,  0x1C0,  0x1D0,  0x1E0,
    0x1F0,  0x200,  0x208,  0x210,  0x218,  0x220,  0x228,  0x230,
    0x238,  0x240,  0x248,  0x250,  0x258,  0x260,  0x268,  0x270,
    0x278,  0x280,  0x288,  0x290,  0x298,  0x2A0,  0x2A8,  0x2B0,
    0x2B8,  0x2C0,  0x2C8,  0x2D0,  0x2D8,  0x2E0,  0x2E8,  0x2F0,
    0x2F8,  0x300,  0x308,  0x310,  0x318,  0x320,  0x328,  0x330,
    0x338,  0x340,  0x348,  0x350,  0x358,  0x360,  0x368,  0x370,
    0x378,  0x380,  0x388,  0x390,  0x398,  0x3A0,  0x3A8,  0x3B0,
    0x3B8,  0x3C0,  0x3C8,  0x3D0,  0x3D8,  0x3E0,  0x3E8,  0x3F0,
    0x3F8,  0x400,  0x440,  0x480,  0x4C0,  0x500,  0x540,  0x580,
    0x5C0,  0x600,  0x640,  0x680,  0x6C0,  0x700,  0x740,  0x780,
    0x7C0,  0x800,  0x900,  0xA00,  0xB00,  0xC00,  0xD00,  0xE00,
    0xF00, 0x1000, 0x1400, 0x1800, 0x1C00, 0x2000, 0x3000, 0x4000
];
