use nihav_core::demuxers::*;

const VIDEO_CODECS: &[(i32, &str)] = &[
    (  1, "movinglines"),
    (  2, "arm_rawvideo"),
    (  3, "arm_rawvideo"),
    (  5, "arm_rawvideo"),
    (  7, "movingblocks"),
    ( 17, "movingblockshq"),
    ( 19, "supermovingblocks"),
    (100, "escape100"),
    (102, "escape102"),
    (122, "escape122"),
    (124, "escape124"),
    (130, "escape130"),
    (800, "linepack"),
    (802, "movie16_3"),
];

trait ReadString {
    fn read_string(&mut self) -> DemuxerResult<Vec<u8>>;
}

impl<'a> ReadString for ByteReader<'a> {
    fn read_string(&mut self) -> DemuxerResult<Vec<u8>> {
        let mut res = Vec::new();
        loop {
            let c = self.read_byte()?;
            if c == b'\n' {
                break;
            }
            res.push(c);
            validate!(res.len() < (1 << 10)); // insanity check
        }
        Ok(res)
    }
}

fn parse_int(src: &[u8]) -> DemuxerResult<i32> {
    let mut val = 0;
    let mut parsed = false;
    let mut sign = false;
    for &c in src.iter() {
        match c {
            b'-' if !parsed => { sign = true; },
            b'-' => return Err(DemuxerError::InvalidData),
            b'0'..=b'9' => {
                val = val * 10 + ((c - b'0') as i32);
                if val > (1 << 27) {
                    return Err(DemuxerError::InvalidData);
                }
                parsed = true;
            },
            b' ' | b'\t' if !parsed => {},
            _ => break,
        }
    }
    if parsed {
        Ok(if !sign { val } else { -val })
    } else {
        Err(DemuxerError::InvalidData)
    }
}

fn parse_uint(src: &[u8]) -> DemuxerResult<u32> {
    let val = parse_int(src)?;
    if val < 0 { return Err(DemuxerError::InvalidData); }
    Ok(val as u32)
}

fn parse_float(src: &[u8]) -> DemuxerResult<f32> {
    let mut val = 0.0f32;
    let mut parsed = false;
    let mut frac_part = 1.0;
    for &c in src.iter() {
        match c {
            b'0'..=b'9' => {
                if frac_part == 1.0 {
                    val = val * 10.0 + ((c - b'0') as f32);
                    if val > 1000.0 {
                        return Err(DemuxerError::InvalidData);
                    }
                } else {
                    val += ((c - b'0') as f32) * frac_part;
                    frac_part *= 0.1;
                }
                parsed = true;
            },
            b'.' if frac_part != 1.0 => return Err(DemuxerError::InvalidData),
            b'.' => {
                frac_part = 0.1;
            },
            b' ' | b'\t' => {},
            _ => break,
        }
    }
    if parsed {
        Ok(val)
    } else {
        Err(DemuxerError::InvalidData)
    }
}

#[allow(clippy::while_let_on_iterator)]
fn split_sound_str(string: &[u8]) -> DemuxerResult<Vec<&[u8]>> {
    let mut start = 0;
    let mut ret = Vec::new();
    let mut ref_trk_id = 2;

    let mut iter = string.iter().enumerate();
    while let Some((pos, &c)) = iter.next() {
        if c == b'|' {
            ret.push(&string[start..pos]);

            validate!(pos + 2 < string.len());

            let mut num_end = pos + 2;
            while let Some((pos2, c)) = iter.next() {
                if !c.is_ascii_digit() {
                    num_end = pos2 + 1;
                    break;
                }
            }
            let trk_id = parse_uint(&string[pos + 1..num_end])?;
            validate!(trk_id == ref_trk_id);
            ref_trk_id += 1;
            start = num_end;
        }
    }
    if start < string.len() {
        ret.push(&string[start..]);
    }
    Ok(ret)
}

struct ChunkInfo {
    offset:     u32,
    vid_size:   u32,
    aud_sizes:  Vec<u32>,
}

enum ReadState {
    None,
    Video,
    Audio(usize),
}

struct ARMovieDemuxer<'a> {
    src:            &'a mut ByteReader<'a>,
    chunk_offs:     Vec<ChunkInfo>,
    cur_chunk:      usize,
    state:          ReadState,
    video_id:       Option<usize>,
    audio_ids:      Vec<usize>,
}

impl<'a> ARMovieDemuxer<'a> {
    fn new(src: &'a mut ByteReader<'a>) -> Self {
        Self {
            src,
            chunk_offs:     Vec::new(),
            cur_chunk:      0,
            state:          ReadState::None,
            video_id:       None,
            audio_ids:      Vec::new(),
        }
    }
    fn parse_catalogue(&mut self, offset: u32, num_chunks: usize, even_csize: usize, odd_csize: usize, aud_tracks: usize) -> DemuxerResult<()> {
        self.src.seek(SeekFrom::Start(u64::from(offset)))?;
        self.chunk_offs.clear();
        for i in 0..num_chunks {
            let cur_chunk_size = if (i & 1) == 0 { even_csize } else { odd_csize };

            let entry           = self.src.read_string()?;
            let comma_pos = entry.iter().position(|&c| c == b',');
            let semicolon_pos = entry.iter().position(|&c| c == b';');
            if let (Some(c_pos), Some(sc_pos)) = (comma_pos, semicolon_pos) {
                validate!(c_pos > 0 && c_pos + 1 < sc_pos);
                let offset      = parse_uint(&entry[..c_pos])?;
                let vid_size    = parse_uint(&entry[c_pos + 1..sc_pos])?;
                let astring = &entry[sc_pos + 1..];
                let asizes = split_sound_str(astring)?;

                let mut aud_sizes = Vec::with_capacity(aud_tracks);
                if aud_tracks > 0 {
                    let aud_size = parse_uint(asizes[0])?;
                    aud_sizes.push(aud_size);
                }
                for &aud_entry in asizes.iter().skip(1) {
                    let aud_size = parse_uint(aud_entry)?;
                    aud_sizes.push(aud_size);
                }

                let tot_size: u32 = vid_size + aud_sizes.iter().sum::<u32>();
                validate!((tot_size as usize) <= cur_chunk_size);
                self.chunk_offs.push(ChunkInfo { offset, vid_size, aud_sizes });
            } else {
                return Err(DemuxerError::InvalidData);
            }
        }

        Ok(())
    }
}

impl<'a> RawDemuxCore<'a> for ARMovieDemuxer<'a> {
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let magic               = self.src.read_string()?;
        validate!(&magic == b"ARMovie");
        let _name               = self.src.read_string()?;
        let _date_and_copyright = self.src.read_string()?;
        let _author             = self.src.read_string()?;

        let video_id            = self.src.read_string()?;
        let video_codec = parse_int(&video_id)?;
        let width               = self.src.read_string()?;
        let width = parse_int(&width)?;
        let height              = self.src.read_string()?;
        let height = parse_int(&height)?;
        validate!((video_codec <= 0) ^ (width > 0 && height > 0));
        let width  = width as usize;
        let height = height as usize;
        let vformat             = self.src.read_string()?;
        let fps                 = self.src.read_string()?;
        let fps = parse_float(&fps)?;

        let sound_id            = self.src.read_string()?;
        let sound_ids = split_sound_str(&sound_id)?;
        let mut num_sound = sound_ids.len();
        if num_sound == 1 {
            let sound_codec = parse_int(sound_ids[0])?;
            if sound_codec < 1 {
                num_sound = 0;
            }
        }
        let srate               = self.src.read_string()?;
        let srates = split_sound_str(&srate)?;
        let chan                = self.src.read_string()?;
        let channels = split_sound_str(&chan)?;
        let sndformat           = self.src.read_string()?;
        let sndformats = split_sound_str(&sndformat)?;

        let frm_per_chunk       = self.src.read_string()?;
        let frm_per_chunk = parse_uint(&frm_per_chunk)? as usize;
        validate!(frm_per_chunk > 0);
        let num_chunks          = self.src.read_string()?;
        let num_chunks = parse_uint(&num_chunks)? as usize + 1;
        let even_chunk_size     = self.src.read_string()?;
        let even_chunk_size = parse_uint(&even_chunk_size)? as usize;
        let odd_chunk_size      = self.src.read_string()?;
        let odd_chunk_size = parse_uint(&odd_chunk_size)? as usize;
        let cat_offset          = self.src.read_string()?;
        let cat_offset = parse_uint(&cat_offset)?;

        let _sprite_offset      = self.src.read_string()?;
        let _sprite_size        = self.src.read_string()?;
        let _kf_offset_res      = self.src.read_string(); // may be not present for older ARMovies

        self.parse_catalogue(cat_offset, num_chunks, even_chunk_size, odd_chunk_size, num_sound)?;

        let mut stream_id = 0;
        if video_codec > 0 {
            let codec_name = if let Some(idx) = VIDEO_CODECS.iter().position(|&(id, _)| id == video_codec) {
                    VIDEO_CODECS[idx].1
                } else {
                    "unknown"
                };
            validate!(fps > 1.0e-4);
            let mut tbase = fps;
            let mut tb_num = 1;
            while tbase.fract() > 1.0e-4 {
                tb_num *= 10;
                tbase *= 10.0;
            }
            let tb_den = tbase as u32;

            let mut edata = vec![video_codec as u8, (video_codec >> 8) as u8];
            edata.extend_from_slice(&vformat);

            let vci = NACodecTypeInfo::Video(NAVideoInfo::new(width, height, false, YUV420_FORMAT));
            let vinfo = NACodecInfo::new(codec_name, vci, Some(edata));
            let ret = strmgr.add_stream(NAStream::new(StreamType::Video, stream_id, vinfo, tb_num, tb_den, (frm_per_chunk * num_chunks) as u64));
            if ret.is_some() {
                stream_id += 1;
                self.video_id = ret;
            } else {
                return Err(DemuxerError::MemoryError);
            }
        }

        if num_sound > 0 {
            validate!(sound_ids.len() == srates.len());
            validate!(sound_ids.len() == channels.len());
            validate!(sound_ids.len() == sndformats.len());
            for ((&id, &sratestr), (&chan, &fmt)) in sound_ids.iter().zip(srates.iter())
                        .zip(channels.iter().zip(sndformats.iter())) {
                let codec_id = parse_uint(id)?;
                let codec_name = if codec_id == 1 { "pcm" } else { "unknown" };
                let channels = parse_uint(chan)?;
                validate!(channels > 0 && channels < 16);
                let bits = parse_uint(fmt)?;
                let mut srate = parse_uint(sratestr)?;
                if srate > 0 && srate < 1000 { // probably in microseconds instead of Hertz
                    srate = 1000000 / srate;
                }
//println!(" codec id {codec_id} srate {srate} chan {channels} bits {bits}");
                let fmt = if bits == 8 { SND_U8_FORMAT } else { SND_S16_FORMAT };

                let aci = NACodecTypeInfo::Audio(NAAudioInfo::new(srate, channels as u8, fmt, 0));
                let ainfo = NACodecInfo::new(codec_name, aci, None);
                let ret = strmgr.add_stream(NAStream::new(StreamType::Audio, stream_id, ainfo, 1, srate, 0));
                if let Some(id) = ret {
                    self.audio_ids.push(id);
                    stream_id += 1;
                } else {
                    return Err(DemuxerError::MemoryError);
                }
            }
        }

        Ok(())
    }

    fn get_data(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NARawData> {
        while self.cur_chunk < self.chunk_offs.len() {
            let chunk = &self.chunk_offs[self.cur_chunk];
            match self.state {
                ReadState::None => {
                    self.src.seek(SeekFrom::Start(u64::from(chunk.offset)))?;
                    self.state = ReadState::Video;
                }
                ReadState::Video => {
                    self.state = ReadState::Audio(0);
                    if chunk.vid_size > 0 {
                        validate!(self.video_id.is_some());
                        if let Some(stream) = strmgr.get_stream(self.video_id.unwrap_or(0)) {
                            let mut buf = vec![0; chunk.vid_size as usize];
                            self.src.read_buf(&mut buf)?;
                            return Ok(NARawData::new(stream, buf));
                        } else {
                            return Err(DemuxerError::InvalidData);
                        }
                    }
                },
                ReadState::Audio(idx) => {
                    if idx < chunk.aud_sizes.len() {
                        self.state = ReadState::Audio(idx + 1);
                        if chunk.aud_sizes[idx] > 0 {
                            if let Some(stream) = strmgr.get_stream(self.audio_ids[idx]) {
                                let mut buf = vec![0; chunk.aud_sizes[idx] as usize];
                                self.src.read_buf(&mut buf)?;
                                return Ok(NARawData::new(stream, buf));
                            } else {
                                return Err(DemuxerError::InvalidData);
                            }
                        }
                    } else {
                        self.cur_chunk += 1;
                        self.state = ReadState::None;
                    }
                },
            }
        }

        Err(DemuxerError::EOF)
    }

    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for ARMovieDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub struct ARMovieDemuxerCreator { }

impl RawDemuxerCreator for ARMovieDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut ByteReader<'a>) -> Box<dyn RawDemuxCore<'a> + 'a> {
        Box::new(ARMovieDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "armovie" }
    fn check_format(&self, br: &mut ByteReader) -> bool {
        let mut hdr = [0; 8];
        br.read_buf(&mut hdr).is_ok() && &hdr == b"ARMovie\n"
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_armovie_demux() {
        // a sample from Acorn Replay Demonstration Disc 2
        let mut file = File::open("assets/Acorn/CHEMSET2").unwrap();
        let mut fr = FileReader::new_read(&mut file);
        let mut br = ByteReader::new(&mut fr);
        let mut dmx = ARMovieDemuxer::new(&mut br);
        let mut sm = StreamManager::new();
        let mut si = SeekIndex::new();
        dmx.open(&mut sm, &mut si).unwrap();

        loop {
            let pktres = dmx.get_data(&mut sm);
            if let Err(e) = pktres {
                if e == DemuxerError::EOF { break; }
                panic!("error");
            }
            let pkt = pktres.unwrap();
            println!("Got {}", pkt);
        }
    }
}
