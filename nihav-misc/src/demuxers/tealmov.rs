use nihav_core::demuxers::*;

struct TealMovieDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    frameno:        usize,
    offsets:        Vec<u32>,
    afrm_start:     usize,
    acodec:         u8,
    parse_frames:   bool,
    ppos:           usize,
    buf:            Vec<u8>,
    vframeno:       u64,
    width:          usize,
    height:         usize,
}

impl<'a> TealMovieDemuxer<'a> {
    fn new(src: &'a mut dyn ByteIO) -> Self {
        TealMovieDemuxer {
            src,
            frameno:        0,
            offsets:        Vec::new(),
            afrm_start:     0,
            acodec:         0,
            parse_frames:   false,
            ppos:           0,
            buf:            Vec::new(),
            vframeno:       0,
            width:          0,
            height:         0,
        }
    }
    fn skip_block2(br: &mut dyn ByteIO, b2_run: &mut usize, b2_type: &mut u8) -> DemuxerResult<()> {
        let op = if *b2_run > 0 {
                *b2_run -= 1;
                *b2_type
            } else {
                let opcode = br.read_byte()?;
                if opcode != 0xF3 {
                    opcode
                } else {
                    *b2_run = usize::from(br.read_byte()?) + 3;
                    *b2_type = br.read_byte()?;
                    validate!(*b2_type != 0xF3);
                    *b2_type
                }
            };
        match op {
            0x00..=0xE0 => {},
            0xF6..=0xFD => {},
            0xFE => {
                br.read_skip(1)?;
            },
            0xFF => {
                br.read_skip(4)?;
            },
            _ => {},
        }
        Ok(())
    }
    fn skip_block4(br: &mut dyn ByteIO, split: bool, b4_run: &mut usize, b4_type: &mut u8, b2_run: &mut usize, b2_type: &mut u8) -> DemuxerResult<()> {
        let op = if split {
                0xFF
            } else if *b4_run > 0 {
                *b4_run -= 1;
                *b4_type
            } else {
                let opcode = br.read_byte()?;
                if opcode != 0xF3 {
                    opcode
                } else {
                    *b4_run = usize::from(br.read_byte()?) + 3;
                    *b4_type = br.read_byte()?;
                    validate!(*b4_type != 0xF3);
                    *b4_type
                }
            };
        match op {
            0x00..=0xE0 => {},
            0xF5 => {
                br.read_skip(3)?;
            },
            0xF6..=0xFD => {},
            0xFE => {
                br.read_skip(1)?;
            },
            0xFF => {
                for _ in 0..4 {
                    Self::skip_block2(br, b2_run, b2_type)?;
                }
            },
            _ => {},
        }
        Ok(())
    }
    fn find_type2_size(br: &mut dyn ByteIO, width: usize, height: usize) -> DemuxerResult<usize> {
        let mut b8_type = 0;
        let mut b8_run  = 0;
        let mut b4_type = 0;
        let mut b4_run  = 0;
        let mut b2_type = 0;
        let mut b2_run  = 0;
        br.read_skip(20)?;
        for _y in (0..height).step_by(8) {
            for _ in (0..width).step_by(8) {
                let op = if b8_run > 0 {
                        b8_run -= 1;
                        b8_type
                    } else {
                        let cop = br.read_byte()?;
                        if cop == 0xF3 {
                            b8_run = usize::from(br.read_byte()?) + 3;
                            b8_type = br.read_byte()?;
                            validate!(b8_type != 0xF3);
                            b8_type
                        } else {
                            cop
                        }
                    };
                match op {
                    0x00..=0xE0 => {},
                    0xE1..=0xF0 => {
                        let cbp = (op - 1) & 0xF;
                        for i in 0..4 {
                            Self::skip_block4(br, ((cbp >> i) & 1) != 0, &mut b4_run, &mut b4_type, &mut b2_run, &mut b2_type)?;
                        }
                    },
                    0xF5 => {
                        br.read_skip(3)?;
                    },
                    0xF6..=0xFD => {},
                    0xFE => {
                        br.read_skip(1)?;
                    },
                    0xFF => {
                        for _ in 0..4 {
                            Self::skip_block4(br, false, &mut b4_run, &mut b4_type, &mut b2_run, &mut b2_type)?;
                        }
                    },
                    _ => {},
                }
            }
        }
        Ok(br.tell() as usize)
    }
    fn parse_frame(&mut self) -> DemuxerResult<(bool, Vec<u8>)> {
        let mut br = MemoryReader::new_read(&self.buf[self.ppos..]);
        let ftype = br.read_u16be()?;
        let flags = br.read_u16be()?;
        if (flags & 1) == 0 {
            let data = self.buf[self.ppos..].to_vec();
            self.ppos = self.buf.len();
            return Ok((ftype == 1, data));
        }
        let mut size = match ftype {
                1 if self.width == 160 => {
                    br.read_skip(8)?;
                    let data_off = br.read_u32be()? as usize;
                    data_off + self.width * self.height
                },
                1 => {
                    br.read_skip(8)?;
                    let data_off = br.read_u32be()? as usize;
                    data_off + self.width * self.height / 4
                },
                2 => {
                    Self::find_type2_size(&mut br, self.width, self.height)
                        .map_err(|_| DemuxerError::InvalidData)?
                },
                _ => {
                    4
                },
            };
        if self.width == 160 && (size & 1) == 1 {
            size += 1;
        }
        validate!(self.ppos + size <= self.buf.len());
        let data = self.buf[self.ppos..][..size].to_vec();
        self.ppos += size;
        Ok((ftype == 1, data))
    }
}

impl<'a> DemuxCore<'a> for TealMovieDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
                                      self.src.seek(SeekFrom::Start(0x3C))?;
        let tag1                    = self.src.read_tag()?;
        let tag2                    = self.src.read_tag()?;
        validate!(&tag1 == b"Mvie" && &tag2 == b"TlMv");
                                      self.src.read_skip(8)?;
        let nframes                 = self.src.read_u16be()? as usize;
        validate!(nframes > 1);
        self.offsets.resize(nframes, 0);

        let mut poffset = 0x4C + 2 + (nframes as u32 * 8) - 1;
        for el in self.offsets.iter_mut() {
            let offset              = self.src.read_u32be()?;
                                      self.src.read_u32be()?;
            validate!(offset > poffset);
            *el = offset;
            poffset = offset;
        }
                                      self.src.seek(SeekFrom::End(0))?;
        let size = self.src.tell();
        validate!(size > u64::from(self.offsets[self.offsets.len() - 1]) && size < (1 << 28));
        self.offsets.push(size as u32);

        let frame0_size = (self.offsets[1] - self.offsets[0]) as usize;
        validate!(frame0_size >= 44);
                                      self.src.seek(SeekFrom::Start(u64::from(self.offsets[0])))?;
        let _hash                   = self.src.read_u16be()?;
        let hdr_size                = self.src.read_u16be()? as usize;
        validate!(hdr_size >= 44 && hdr_size <= frame0_size);
        let width                   = self.src.read_u16be()? as usize;
        let height                  = self.src.read_u16be()? as usize;
        validate!((1..=640).contains(&width) && (1..=640).contains(&height));
        let depth                   = self.src.read_u16be()?; // 8 or 16
        validate!(depth == 8 || depth == 16);
        self.acodec                 = self.src.read_byte()?;
        let video_type              = self.src.read_byte()?;
        validate!(self.acodec <= 2);
        validate!(video_type <= 6);
                                      self.src.read_u16be()?; // always 0x43?
        let fps                     = self.src.read_u16be()?;
        validate!((1..=60).contains(&fps));
                                      self.src.read_u16be()?;
        let flags                   = self.src.read_u16be()?;

                                      self.src.read_skip(0x10)?;
        let num_vframes             = self.src.read_u16be()?;

        self.frameno = 1;

        let vci = if video_type < 4 {
                NACodecTypeInfo::Video(NAVideoInfo::new(width, height, false, PAL8_FORMAT))
            } else {
                NACodecTypeInfo::Video(NAVideoInfo::new(width, height * 2 / 3, false, RGB565_FORMAT))
            };
        let vinfo = NACodecInfo::new("teal-video", vci, Some(vec![video_type]));
        if strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, 1, u32::from(fps), u64::from(num_vframes))).is_none() {
            return Err(DemuxerError::MemoryError);
        }

        if (flags & 2) != 0 {
            self.afrm_start         = self.src.read_u16be()? as usize;
            validate!(self.afrm_start > 1 && self.afrm_start < nframes);

            let (cname, fmt) = if self.acodec == 1 { ("teal-audio", SND_S16_FORMAT) } else { ("pcm", SND_U8_FORMAT) };
            let arate = 10000; //XXX: it may be 20000 if some flag is set
            let aci = NACodecTypeInfo::Audio(NAAudioInfo::new(arate, 1, fmt, 2));
            let ainfo = NACodecInfo::new(cname, aci, None);
            let size = self.offsets[self.offsets.len() - 1] - self.offsets[self.afrm_start];
            if strmgr.add_stream(NAStream::new(StreamType::Audio, 1, ainfo, 1, arate, u64::from(size))).is_none() {
                return Err(DemuxerError::MemoryError);
            }
        } else {
            self.afrm_start = self.offsets.len() - 1;
        }

        if usize::from(num_vframes) + 1 > self.afrm_start {
            self.parse_frames = true;
            self.width = width;
            self.height = height;
        }

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        if self.frameno + 1 >= self.offsets.len() {
            return Err(DemuxerError::EOF);
        }

        let strm_no = if self.frameno >= self.afrm_start { 1 } else { 0 };
        let frame_pos = self.offsets[self.frameno];
        let frame_size = (self.offsets[self.frameno + 1] - frame_pos) as usize;
        if self.parse_frames && strm_no == 0 {
            if self.ppos >= self.buf.len() {
                self.frameno += 1;
                                      self.src.seek(SeekFrom::Start(u64::from(frame_pos)))?;
                self.buf.resize(frame_size, 0);
                                      self.src.read_buf(&mut self.buf)?;
                self.ppos = 0;
            }
            let (is_intra, buf) = self.parse_frame()?;
            let stream = strmgr.get_stream(strm_no).ok_or(DemuxerError::InvalidData)?;
            let ts = stream.make_ts(Some(self.vframeno), None, None);
            self.vframeno += 1;
            return Ok(NAPacket::new(stream, ts, is_intra, buf));
        }
        self.frameno += 1;
                                      self.src.seek(SeekFrom::Start(u64::from(frame_pos)))?;
        if let Some(stream) = strmgr.get_stream(strm_no) {
            let pts = match (strm_no, self.acodec) {
                    (0, _) => self.frameno as u64 - 2,
                    (1, 1) => u64::from(frame_pos - self.offsets[self.afrm_start]) * 2,
                    _ => u64::from(frame_pos - self.offsets[self.afrm_start]),
                };
            let ts = stream.make_ts(Some(pts), None, None);
            self.src.read_packet(stream, ts, self.frameno == 2, frame_size)
        } else {
            Err(DemuxerError::InvalidData)
        }
    }

    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for TealMovieDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub struct TealMovieDemuxerCreator { }

impl DemuxerCreator for TealMovieDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(TealMovieDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "tealmov" }
}

#[cfg(test)]
mod test {
    use super::*;
    use nihav_codec_support::test::dmx_test::*;

    #[test]
    fn test_teal_movie_demux() {
        // sample bundled with TealMovie software
        test_file_demux("assets/Misc/ASTRONUT.PDB", &TealMovieDemuxerCreator{}, 0, DemuxerTestMode::Demux);
    }
}
