use nihav_core::demuxers::*;
use nihav_codec_support::codecs::qt_pal::MOV_DEFAULT_PAL_8BIT;

struct Track {
    offsets:        Vec<u32>,
    sizes:          Vec<u32>,
    cur_frame:      usize,
}

struct WarholDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    cur_track:      usize,
    tracks:         Vec<Track>,
}

impl<'a> DemuxCore<'a> for WarholDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
                          self.src.seek(SeekFrom::Start(0x41))?;
        let tag         = self.src.read_tag()?;
        validate!(&tag == b"MooV");
                          self.src.read_skip(14)?;
        let data_end    = u64::from(self.src.read_u32be()?) + 0x80;
        let rsrc_size   = self.src.read_u32be()?;
        validate!(rsrc_size > 0x100);
                          self.src.seek(SeekFrom::Start((data_end + 0x7F) & !0x7F))?;
        let rsrc_end = self.src.tell() + u64::from(rsrc_size);

        let mut moov_end = 0;
        while self.src.tell() + 12 < rsrc_end {
            let size    = self.src.read_u32be()?;
            validate!(size >= 12 && self.src.tell() + u64::from(size - 4) <= rsrc_end);
            let _size2  = self.src.read_u32be()?;
            let tag     = self.src.read_tag()?;
            if &tag == b"moov" {
                moov_end = self.src.tell() + u64::from(size - 12);
                break;
            }
                          self.src.read_skip(size as usize - 12)?;
        }
                          self.src.read_skip(0x70)?; // unknown media header data
        while self.src.tell() + 4 < moov_end {
            let tsize   = self.src.read_u32be()?;
            let tag     = self.src.read_tag()?;
            validate!(tsize > 0x170 && self.src.tell() + u64::from(tsize - 8) <= moov_end);
            validate!(&tag == b"trak");
            let trk_end = self.src.tell() + u64::from(tsize - 8);
                          self.src.read_skip(0x68)?;
            let msize   = self.src.read_u32be()?;
            let tag     = self.src.read_tag()?;
            validate!(msize > 0x70 && self.src.tell() + u64::from(msize - 8) + 4 == trk_end);
            validate!(&tag == b"mdia");
                          self.src.read_skip(0x14)?;
            let fps     = self.src.read_u32be()?;
            validate!((1..=1000).contains(&fps));
                          self.src.read_skip(0x8)?;
            let ndesc   = self.src.read_u32be()? as usize;
            validate!((1..=16).contains(&ndesc));
                          self.src.read_u32be()?;
            let nmaps   = self.src.read_u32be()? as usize;
            validate!((1..=16).contains(&nmaps));
            let nts     = self.src.read_u32be()? as usize;
            let duration = self.src.read_u32be()?;
                          self.src.read_u32be()?;
            let smth    = self.src.read_u32be()?;
                          self.src.read_skip(smth as usize + 8)?;
            let tag     = self.src.read_tag()?;
            validate!(&tag == b"vide");
            let _vendor = self.src.read_tag()?;
            let nlen    = usize::from(self.src.read_byte()?);
                          self.src.read_skip(nlen)?;
                          self.src.read_skip(8)?;
            let dsize   = self.src.read_u32be()?;
            validate!(dsize >= 20);
            let desc_end = self.src.tell() + u64::from(dsize - 4);
            let fcc     = self.src.read_tag()?;
                          self.src.read_skip(0x18)?;
            let width   = usize::from(self.src.read_u16be()?);
            let height  = usize::from(self.src.read_u16be()?);
            validate!((1..=1024).contains(&width) && (1..=1024).contains(&height));
                          self.src.read_skip(14)?; // dpi and data size?
                          self.src.read_skip(32)?; // codec name
            let depth   = self.src.read_u16be()?;
            let _ct_id  = self.src.read_u16be()?;
            let grayscale = depth > 0x20 || depth == 1;
            let depth = if grayscale { depth & 0x1F } else { depth };
                          self.src.read_skip(6)?;

            if depth != 8 {
                return Err(DemuxerError::NotImplemented);
            }
                          self.src.seek(SeekFrom::Start(desc_end))?;
            if ndesc > 1 {
                //println!("skipping {} track descriptor(s)", ndesc - 1);
                for _ in 1..ndesc {
                    let dsize   = self.src.read_u32be()? as usize;
                    validate!(dsize >= 20);
                                  self.src.read_skip(dsize - 4)?;
                }
            }
            let nframes = self.src.read_u32be()? as usize;
            validate!((1..=10000).contains(&nframes));
                          self.src.read_skip(nts * 4)?;
                          self.src.read_skip(nmaps * 4 * 4)?; // chunk mapping??
            let _one    = self.src.read_u32be()?;
            let mut offsets = Vec::with_capacity(nframes);
            let mut sizes = Vec::with_capacity(nframes);
            for _ in 0..nframes {
                let off = self.src.read_u32be()?;
                let sz  = self.src.read_u32be()?;
                validate!(sz < 10485760);
                offsets.push(off);
                sizes.push(sz);
            }
                          self.src.seek(SeekFrom::Start(trk_end))?;

            let mut vhdr = NAVideoInfo::new(width, height, false, PAL8_FORMAT);
            vhdr.bits = 8;
            let codec_info = NACodecInfo::new(if &fcc == b"rle " { "qt-rle" } else { "unknown" }, NACodecTypeInfo::Video(vhdr), None);
            strmgr.add_stream(NAStream::new(StreamType::Video, 0, codec_info, 1, fps, u64::from(duration))).ok_or(DemuxerError::MemoryError)?;
            self.tracks.push(Track{ offsets, sizes, cur_frame: 0 });
        }
        validate!(!self.tracks.is_empty());

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        let start_track = self.cur_track;
        let ntracks = self.tracks.len();
        loop {
            let track = &mut self.tracks[self.cur_track];
            let stream = strmgr.get_stream(self.cur_track).unwrap();
            self.cur_track += 1;
            if self.cur_track >= ntracks {
                self.cur_track = 0;
            }
            if track.cur_frame >= track.sizes.len() {
                if self.cur_track == start_track {
                    return Err(DemuxerError::EOF);
                }
                continue;
            }
            let size    = track.sizes[track.cur_frame] as usize;
            let offset  = track.offsets[track.cur_frame];
            let ts = stream.make_ts(Some(track.cur_frame as u64), None, None);
            let keyframe = track.cur_frame == 0;
            track.cur_frame += 1;
                              self.src.seek(SeekFrom::Start(u64::from(offset) + 0x80))?;
            let mut pkt     = self.src.read_packet(stream, ts, keyframe, size)?;
            if keyframe {
                pkt.add_side_data(NASideData::Palette(true, Arc::new(MOV_DEFAULT_PAL_8BIT)));
            }
            return Ok(pkt);
        }
    }

    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::SeekError)
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for WarholDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

impl<'a> WarholDemuxer<'a> {
    fn new(src: &'a mut dyn ByteIO) -> Self {
        WarholDemuxer {
            src,
            cur_track:  0,
            tracks:     Vec::new(),
        }
    }
}

pub struct WarholDemuxerCreator { }

impl DemuxerCreator for WarholDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(WarholDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "warhol" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    //test samples from Apple Reference & Presentation Library 8
    #[test]
    fn test_warhol_single_desc() {
        let mut file = File::open("assets/QT/MultiTasking.movie").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = WarholDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }
    #[test]
    fn test_warhol_multiple_desc() {
        let mut file = File::open("assets/QT/VirtualMemory.movie").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = WarholDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_frame(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }
}
