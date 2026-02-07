use nihav_core::demuxers::*;
use nihav_registry::register;
use std::str::FromStr;

fn read_extradata(src: &mut dyn ByteIO, size: usize) -> DemuxerResult<Option<Vec<u8>>> {
    if size == 0 { return Ok(None); }
    let mut edvec: Vec<u8> = vec![0; size];
    src.read_buf(&mut edvec)?;
    Ok(Some(edvec))
}

fn parse_audio_header(src: &mut dyn ByteIO, strmgr: &mut StreamManager, size: usize) -> DemuxerResult<u32> {
    validate!(size >= 16);
    let w_format_tag        = src.read_u16le()?;
    let channels            = src.read_u16le()?;
    let samplespersec       = src.read_u32le()?;
    let _avgbytespersec     = src.read_u32le()?;
    let block_align         = src.read_u16le()?;
    let bits_per_sample     = src.read_u16le()?;

    let signed = bits_per_sample > 8;
    let soniton = NASoniton::new(bits_per_sample as u8, if signed { SONITON_FLAG_SIGNED } else { 0 });
    let ahdr = NAAudioInfo::new(samplespersec, channels as u8, soniton, block_align as usize);
    let edata = if size > 18 {
            let edata_size  = src.read_u16le()? as usize;
            validate!(edata_size + 18 <= size);
            read_extradata(src, size - 18)?
        } else if size > 16 {
                              src.read_skip(size - 16)?;
            None
        } else {
            None
        };
    let cname = match register::find_codec_from_wav_twocc(w_format_tag) {
                    None => "unknown",
                    Some(name) => name,
                };
    let ainfo = NACodecInfo::new(cname, NACodecTypeInfo::Audio(ahdr), edata);
    strmgr.add_stream(NAStream::new(StreamType::Audio, 1, ainfo, 1, samplespersec, 0))
        .ok_or(DemuxerError::MemoryError)?;
    Ok(u32::from(block_align))
}

#[derive(Clone,Copy)]
struct IdxEntry {
    vsize:          u32,
    asize:          u32,
}

struct AVDemuxer<'a> {
    src:            &'a mut dyn ByteIO,
    idx:            Vec<IdxEntry>,
    frame:          usize,
    audio:          bool,
    pal:            Option<Arc<[u8; 1024]>>,
    pal_changed:    bool,
}

impl<'a> AVDemuxer<'a> {
    fn new(src: &'a mut dyn ByteIO) -> Self {
        AVDemuxer {
            src,
            idx:            Vec::new(),
            frame:          0,
            audio:          false,
            pal:            None,
            pal_changed:    false,
        }
    }
}

impl<'a> DemuxCore<'a> for AVDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let bh_size         = self.src.read_u32le()? as usize;
        validate!(bh_size >= 40);
        let bi_size         = self.src.read_u32le()? as usize;
        validate!(bi_size >= 40 && bi_size <= bh_size);
        let width           = self.src.read_u32le()?;
        let height          = self.src.read_u32le()? as i32;
        let planes          = self.src.read_u16le()?;
        let bitcount        = self.src.read_u16le()?;
        let compression     = self.src.read_tag()?;
        let _img_size       = self.src.read_u32le()?;
        let _xdpi           = self.src.read_u32le()?;
        let _ydpi           = self.src.read_u32le()?;
        let colors          = self.src.read_u32le()?;
        validate!(colors <= 256);
        let _imp_colors     = self.src.read_u32le()?;

        let flip = height < 0;
        let format = if bitcount > 8 { RGB24_FORMAT } else { PAL8_FORMAT };
        let mut vhdr = NAVideoInfo::new(width as usize, height.unsigned_abs() as usize, flip, format);
        vhdr.bits = (planes as u8) * (bitcount as u8);
        let cname = if find_raw_rgb_fmt(&compression, planes, bitcount, flip, &mut vhdr) {
                "rawvideo-ms"
            } else {
                match register::find_codec_from_avi_fourcc(&compression) {
                    None => "unknown",
                    Some(name) => name,
                }
            };
        let vci = NACodecTypeInfo::Video(vhdr);
        let edata = read_extradata(&mut *self.src, bh_size - 40)?;
        if colors > 0 {
            if let Some(ref buf) = edata {
                let mut pal = [0u8; 1024];
                for (dpal, spal) in pal.chunks_mut(4).take(colors as usize).zip(buf.chunks(4)) {
                    dpal[0] = spal[2];
                    dpal[1] = spal[1];
                    dpal[2] = spal[0];
                    dpal[3] = 0;
                }
                self.pal = Some(Arc::new(pal));
            }
        }
        let vinfo = NACodecInfo::new(cname, vci, edata);

        let tag             = self.src.read_tag()?;
        validate!(&tag == b"vids");
        let _handler        = self.src.read_tag()?;
                              self.src.read_skip(12)?;
        let tb_num          = self.src.read_u32le()?;
        let tb_den          = self.src.read_u32le()?;
        validate!(tb_num != 0 && tb_den != 0);
                              self.src.read_u32le()?;
        let nframes         = self.src.read_u32le()?;
        validate!(nframes > 0);
                              self.src.read_skip(0x28)?;
                              self.src.read_skip(0x40)?; // video stream name
        strmgr.add_stream(NAStream::new(StreamType::Video, 0, vinfo, tb_num, tb_den, u64::from(nframes)))
            .ok_or(DemuxerError::MemoryError)?;

        let wfx_size        = self.src.read_u32le()? as usize;
        let mut blk_align = 0;
        if wfx_size > 0 {
            blk_align = parse_audio_header(&mut *self.src, strmgr, wfx_size)?;
            let tag         = self.src.read_tag()?;
            validate!(&tag == b"auds");
                              self.src.read_skip(0x48)?;
                              self.src.read_skip(0x40)?; // audio stream name
        }

        self.idx.clear();
        seek_index.add_stream(0);
        let mut pos = (self.src.tell() + u64::from(nframes * 12) + 0x1F) & !0x1F;
        for ts in 0..nframes {
            let flags       = self.src.read_u32le()?;
            let vsize       = self.src.read_u32le()?;
            if vsize == 0 {
                validate!((flags & 0x10000000) != 0);
            }
            let asize       = self.src.read_u32le()?;
            validate!(asize < 0x1000000 / blk_align.max(1));
            let asize = asize * blk_align;
            self.idx.push(IdxEntry{ vsize, asize });

            if flags == 0 {
                let time = NATimeInfo::rescale_ts(u64::from(ts), tb_num, tb_den, 1, 1000);
                seek_index.add_entry(0, SeekEntry { time, pts: u64::from(ts), pos });
            }
            pos += u64::from((vsize + 0x1F) & !0x1F);
            pos += u64::from((asize + 0x1F) & !0x1F);
        }
        self.frame = 0;
        self.audio = false;
        self.pal_changed = self.pal.is_some();

        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        loop {
            if self.frame >= self.idx.len() {
                return Err(DemuxerError::EOF);
            }

            let frm = &self.idx[self.frame];
            let frm_no = self.frame as u64;
            let (stream_no, size) = if !self.audio { (0, frm.vsize) } else { (1, frm.asize) };
            if self.audio {
                self.frame += 1;
            }
            self.audio = !self.audio;
            if size == 0 {
                continue;
            }

            let pos = self.src.tell();
            if (pos & 0x1F) != 0 {
                self.src.read_skip(32 - (pos & 0x1F) as usize)?;
            }

            if let Some(stream) = strmgr.get_stream(stream_no) {
                let mut ts = stream.make_ts(None, None, None);
                if stream.get_media_type() == StreamType::Video || frm_no == 0 {
                    ts.pts = Some(frm_no);
                }
                let is_keyframe = frm_no == 0 || stream.get_media_type() == StreamType::Audio;
                let mut pkt = self.src.read_packet(stream, ts, is_keyframe, size as usize)?;
                if self.pal_changed {
                    if let Some(ref pal) = self.pal {
                        pkt.add_side_data(NASideData::Palette(true, pal.clone()));
                    }
                    self.pal_changed = false;
                }

                return Ok(pkt);
            } else {
                self.src.read_skip(size as usize)?;
            }
        }
    }

    fn seek(&mut self, time: NATimePoint, seek_index: &SeekIndex) -> DemuxerResult<()> {
        if let Some(seek_info) = seek_index.find_pos(time) {
            self.frame = seek_info.pts as usize;
            self.audio = false;
            self.src.seek(SeekFrom::Start(seek_info.pos))?;
            self.pal_changed = self.pal.is_some();
            Ok(())
        } else {
            Err(DemuxerError::SeekError)
        }
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for AVDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) {}
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

fn find_raw_rgb_fmt(compr: &[u8; 4], planes: u16, bitcount: u16, flip: bool, vhdr: &mut NAVideoInfo) -> bool {
    match compr {
        &[0, 0, 0, 0] | b"DIB " => {
            if planes != 1 {
                return false;
            }
            let fmt_name = match bitcount {
                     8 => "pal8",
                    16 => "bgr555",
                    24 => "bgr24",
                    32 => "bgra24",
                    _ => return false,
                };
            if let Ok(fmt) = NAPixelFormaton::from_str(fmt_name) {
                vhdr.format = fmt;
                vhdr.flipped = !flip;
                true
            } else {
                false
            }
        },
        _ => false,
    }
}

pub struct AVDemuxerCreator { }

impl DemuxerCreator for AVDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut dyn ByteIO) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(AVDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "av" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_av_video_and_audio_demux() {
        // sample can be found on various CD-Romek discs
        let mut file = File::open("assets/Misc/CURSOR01.AV").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = AVDemuxer::new(&mut br);
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
    fn test_av_video_demux() {
        // sample can be found on various CD-Romek discs
        let mut file = File::open("assets/Misc/cursor04ns.av").unwrap();
        let mut br = FileReader::new_read(&mut file);
        let mut dmx = AVDemuxer::new(&mut br);
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
