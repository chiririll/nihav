use nihav_core::demuxers::*;

struct TCADemuxer<'a> {
    src:        &'a mut dyn ByteIO,
    tca:        TCACoreDemuxer,
}

impl<'a> TCADemuxer<'a> {
    fn new(src: &'a mut dyn ByteIO) -> Self {
        Self {
            src,
            tca:    TCACoreDemuxer::default()
        }
    }
}

impl<'a> DemuxCore<'a> for TCADemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        self.tca.open(self.src, strmgr, 0, 0)
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        self.tca.get_frame(self.src, strmgr)
    }

    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for TCADemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub struct TCADemuxerCreator { }

impl DemuxerCreator for TCADemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(TCADemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "tca" }
}

#[derive(Default)]
pub(crate) struct TCACoreDemuxer {
    data_end:   u64,
    frameno:    u64,
    video_pos:  u64,
    sound_pos:  u64,
    sound_end:  u64,
    asize:      u64,
    sblk_len:   usize,
    audio:      bool,
}

impl TCACoreDemuxer {
    pub(crate) fn open(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager, mut tb_num: u32, mut tb_den: u32) -> DemuxerResult<()> {
        let start_pos = src.tell();
        let tag                     = src.peek_tag()?;
        let is_acef = &tag == b"ACEF";
        let acef_size = if is_acef {
                                      src.read_skip(4)?;
                                      src.read_u32le()?
            } else { 0 };
        let size2                   = src.read_u32le()?;
        if is_acef {
            validate!(acef_size > 0x44 && size2 + 8 <= acef_size);
        }
        self.data_end = start_pos + u64::from(size2 + 8);

                                      src.read_skip(12)?;

        const HDR_SIZE: usize = 0x30;

        let mut hdr = vec![0; HDR_SIZE + 4];
        write_u32le(&mut hdr, HDR_SIZE as u32 + 4)?;
                                      src.read_buf(&mut hdr[4..])?;

        let width  = read_u32le(&hdr[8..])? as usize;
        let height = read_u32le(&hdr[12..])? as usize;
        validate!(width > 0 && height > 0);
        validate!((width | height) & 1 == 0);

        let mut sound_start = 0;
        let mut sound_end = 0;

        if is_acef {
            let data_start = src.tell();

            // scan tail for palette and such
            if src.seek(SeekFrom::Start(start_pos + u64::from(acef_size))).is_ok() {
                while let Ok(tag)   = src.read_tag() {
                    let size        = src.read_u32le()? as usize;
                    validate!(size >= 8);
                    match &tag {
                        b"PALE" => {
                            validate!((0x28..=0x428).contains(&size) && (size & 3) == 0);
                                      src.read_skip(0x1C)?;
                            let nclrs = (size - 0x24) / 4;
                            hdr.resize(HDR_SIZE + 4 + 256 * 3, 0);
                            for _ in 0..nclrs {
                                let idx = usize::from(src.read_byte()?);
                                      src.read_buf(&mut hdr[HDR_SIZE + 4 + idx * 3..][..3])?;
                            }
                        },
                        b"RATE" if tb_num == 0 => {
                            if size == 0x14 {
                                let fixed = src.read_u32le()?;
                                validate!(fixed < 2);
                                tb_num    = src.read_u32le()?;
                                tb_den    = src.read_u32le()? * 2;
                                if fixed == 1 {
                                    tb_den = tb_num;
                                    tb_num = 1;
                                }
                                validate!(tb_num > 0 && tb_den > 0);
                            } else {
                                      src.read_skip(size - 8)?;
                            }
                        },
                        b"SOUN" => {
                            sound_start = src.tell();
                            let stag        = src.read_tag()?;
                            validate!(&stag == b"WAV1" || &stag == b"WAV2");
                            let ssize       = src.read_u32le()?;
                            validate!(ssize as usize <= size - 8 && ssize > 0x1C);
                            sound_end = sound_start + u64::from(ssize);
                                              src.read_u32le()?; // usually 1
                                              src.read_u32le()?; // usually 20
                                              src.read_u32le()?; // actual sound size
                                              src.read_u32le()?; // some number, usually in 20000..30000 range
                                              src.read_u32le()?; // usually -1
                            sound_start = src.tell();
                                              src.read_skip(size - 0x1C)?;
                        },
                        _ => {
                                      src.read_skip(size - 8)?;
                        },
                    }
                }
            }
                                      src.seek(SeekFrom::Start(data_start))?;
        }

        if tb_num == 0 { // invent some default framerate
            tb_num = 2;
            tb_den = 25;
        }

        let vci = NACodecTypeInfo::Video(NAVideoInfo::new(width / 2, height / 2, false, PAL8_FORMAT));
        let vinfo = NACodecInfo::new("euclid", vci, Some(hdr));
        let ret = strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, tb_num, tb_den, 0));
        if ret.is_none() {
            return Err(DemuxerError::MemoryError);
        }

        if sound_end > 0 {
            let aci = NACodecTypeInfo::Audio(NAAudioInfo::new(8000, 1, SND_S16_FORMAT, 1));
            let ainfo = NACodecInfo::new("iota-sound", aci, None);
            let ret = strmgr.add_stream(NAStream::new(StreamType::Audio, 1, ainfo, 1, 8000, 0));
            if ret.is_none() {
                return Err(DemuxerError::MemoryError);
            }
            self.sound_pos = sound_start;
            self.sound_end = sound_end;
            self.sblk_len = (8000 * u64::from(tb_num) / u64::from(tb_den)).max(1) as usize;
        }

        self.audio = false;
        self.video_pos = src.tell();

        Ok(())
    }

    pub(crate) fn get_frame(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        let has_video = self.video_pos + 4 < self.data_end;
        let has_audio = self.audio && self.sound_pos < self.sound_end;

        match (has_video, has_audio) {
            (true, false) => {
                                      src.seek(SeekFrom::Start(self.video_pos))?;
                let fsize           = src.read_u32le()? as usize;
                if fsize == 0 {
                    return Err(DemuxerError::EOF);
                }
                validate!((9..=1048576).contains(&fsize));
                if let Some(stream) = strmgr.get_stream(0) {
                    let ts = stream.make_ts(Some(self.frameno), None, None);
                    self.frameno += 1;
                    self.audio = true;
                    self.video_pos += fsize as u64;
                    src.read_packet(stream, ts, false, fsize - 4)
                } else {
                    Err(DemuxerError::InvalidData)
                }
            },
            (_, true) => {
                                      src.seek(SeekFrom::Start(self.sound_pos))?;
                self.audio = false;
                let cur_blk_len = self.sblk_len.min((self.sound_end - self.sound_pos) as usize);
                if let Some(stream) = strmgr.get_stream(1) {
                    let ts = stream.make_ts(Some(self.asize * 2), None, None);
                    self.asize += cur_blk_len as u64;
                    self.audio = !has_video;
                    self.sound_pos += cur_blk_len as u64;
                    src.read_packet(stream, ts, false, cur_blk_len)
                } else {
                    Err(DemuxerError::InvalidData)
                }
            },
            (false, false) => Err(DemuxerError::EOF),
        }
    }

    pub(crate) fn get_frame_raw(&mut self, src: &mut dyn ByteIO, strmgr: &mut StreamManager) -> DemuxerResult<NARawData> {
        let pkt = self.get_frame(src, strmgr)?;
        Ok(NARawData::new_from_refbuf(pkt.get_stream(), pkt.get_buffer()))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use nihav_codec_support::test::dmx_test::*;

    #[test]
    fn test_tca_demux() {
        // a sample from RISC DISC 3
        test_file_demux("assets/Acorn/JUMPER", &TCADemuxerCreator{}, 0, DemuxerTestMode::Demux);
    }
}
