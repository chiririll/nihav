use nihav_core::demuxers::*;

struct TCADemuxer<'a> {
    src:        &'a mut ByteReader<'a>,
    frameno:    u64,
    data_end:   u64,
}

impl<'a> TCADemuxer<'a> {
    fn new(src: &'a mut ByteReader<'a>) -> Self {
        Self {
            src,
            frameno:    0,
            data_end:   0,
        }
    }
}

impl<'a> DemuxCore<'a> for TCADemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let tag                 = self.src.peek_tag()?;
        let is_acef = &tag == b"ACEF";
        let acef_size = if is_acef {
                                      self.src.read_skip(4)?;
                                      self.src.read_u32le()?
            } else { 0 };
        let size2                   = self.src.read_u32le()?;
        if is_acef {
            validate!(acef_size > 0x44 && size2 + 8 <= acef_size);
        }
        self.data_end = u64::from(size2 + 8);

                                      self.src.read_skip(12)?;

        const HDR_SIZE: usize = 0x30;

        let mut hdr = vec![0; HDR_SIZE + 4];
        write_u32le(&mut hdr, HDR_SIZE as u32 + 4)?;
                                      self.src.read_buf(&mut hdr[4..])?;

        let width  = read_u32le(&hdr[8..])? as usize;
        let height = read_u32le(&hdr[12..])? as usize;
        validate!(width > 0 && height > 0);
        validate!((width | height) & 1 == 0);

        if is_acef {
            let data_start = self.src.tell();

            // scan tail for palette and such
            if self.src.seek(SeekFrom::Start(u64::from(acef_size))).is_ok() {
                while let Ok(tag)   = self.src.read_tag() {
                    let size        = self.src.read_u32le()? as usize;
                    validate!(size >= 8);
                    if &tag == b"PALE" {
                        validate!((0x28..=0x428).contains(&size) && (size & 3) == 0);
                                      self.src.read_skip(0x1C)?;
                        let nclrs = (size - 0x24) / 4;
                        hdr.resize(HDR_SIZE + 4 + 256 * 3, 0);
                        for _ in 0..nclrs {
                            let idx = usize::from(self.src.read_byte()?);
                                      self.src.read_buf(&mut hdr[HDR_SIZE + 4 + idx * 3..][..3])?;
                        }
                    } else {
                                      self.src.read_skip(size - 8)?;
                    }
                }
            }
                                      self.src.seek(SeekFrom::Start(data_start))?;
        }

        let vci = NACodecTypeInfo::Video(NAVideoInfo::new(width / 2, height / 2, false, PAL8_FORMAT));
        let vinfo = NACodecInfo::new("euclid", vci, Some(hdr));
        let ret = strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, 1, 10, 0));
        if ret.is_none() {
            return Err(DemuxerError::MemoryError);
        }

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        if self.src.tell() >= self.data_end {
            return Err(DemuxerError::EOF);
        }
        let fsize               = self.src.read_u32le()? as usize;
        if fsize == 0 {
            return Err(DemuxerError::EOF);
        }
        validate!((9..=1048576).contains(&fsize));
        if let Some(stream) = strmgr.get_stream(0) {
            let ts = stream.make_ts(Some(self.frameno), None, None);
            self.frameno += 1;
            // last word is the current packet size for backwards seeking so it can be omitted
            self.src.read_packet(stream, ts, false, fsize - 4)
        } else {
            Err(DemuxerError::InvalidData)
        }
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
    fn new_demuxer<'a>(&self, br: &'a mut ByteReader<'a>) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(TCADemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "tca" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_tca_demux() {
        // a sample from RISC DISC 3
        let mut file = File::open("assets/Acorn/JUMPER").unwrap();
        let mut fr = FileReader::new_read(&mut file);
        let mut br = ByteReader::new(&mut fr);
        let mut dmx = TCADemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            match dmx.get_frame(&mut sm) {
                Ok(pkt) => {
                    println!("Got {pkt}");
                },
                Err(DemuxerError::EOF) => return,
                Err(_) => panic!("error"),
            }
        }
    }
}
