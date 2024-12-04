use std::collections::VecDeque;
use nihav_core::demuxers::*;

const GRAY_FORMAT: NAPixelFormaton = NAPixelFormaton {
    model: ColorModel::YUV(YUVSubmodel::YUVJ),
    components: 1,
    comp_info: [Some(NAPixelChromaton{h_ss: 0, v_ss: 0, packed: false, depth: 8, shift: 0, comp_offs: 0, next_elem: 1}), None, None, None, None],
    elem_size: 1,
    be: true,
    alpha: false,
    palette: false,
};

struct DVIDemuxer<'a> {
    src:            &'a mut ByteReader<'a>,
    data_end:       u64,
    packets:        VecDeque<NAPacket>,
    fsizes:         Vec<usize>,
}

impl<'a> DVIDemuxer<'a> {
    fn new(src: &'a mut ByteReader<'a>) -> Self {
        Self {
            src,
            data_end:   0,
            packets:    VecDeque::new(),
            fsizes:     Vec::new(),
        }
    }
}

impl<'a> DemuxCore<'a> for DVIDemuxer<'a> {
    fn open(&mut self, strmgr: &mut StreamManager, _seek_index: &mut SeekIndex) -> DemuxerResult<()> {
        let br = &mut self.src;

        // standard file header
        let tag                         = br.read_tag()?;
        validate!(&tag == b"IVDV");
        let std_hdr_size                = br.read_u16le()? as usize;
        validate!(std_hdr_size >= 12);
        let _version                    = br.read_u16le()?;
        let _annotation_offset          = br.read_u32le()?;
                                          br.read_skip(std_hdr_size - 12)?;

        // AVL file header
        let tag                         = br.read_tag()?;
        validate!(&tag == b"SSVA");
        let avl_hdr_size                = br.read_u16le()? as usize;
        validate!(avl_hdr_size >= 120);
        let data_start = (std_hdr_size + avl_hdr_size) as u32;
        let _version                    = br.read_u16le()?;
        let _stream_group_count         = br.read_u16le()? as usize;
        let _stream_group_size          = br.read_u16le()? as usize;
        let _stream_group_offset        = br.read_u32le()?;
        let _stream_group_version       = br.read_u16le()?;
        let _stream_size                = br.read_u16le()? as usize;
        let _stream_version             = br.read_u16le()?;
        let stream_count                = br.read_u16le()? as usize;
        validate!(stream_count > 0);
        let stream_offset               = br.read_u32le()?;
        validate!(stream_offset >= data_start);
        let hdr_pool_offset             = br.read_u32le()?;
        validate!(hdr_pool_offset >= data_start);
        let _label_count                = br.read_u32le()?;
        let _label_offset               = br.read_u32le()?;
        let _label_size                 = br.read_u16le()? as usize;
        let _label_version              = br.read_u16le()?;
        let _video_seq_hdr_off          = br.read_u32le()?;
        let _video_seq_hdr_size         = br.read_u16le()? as usize;
        let _frame_hdr_version          = br.read_u16le()?;
        let frame_count                 = br.read_u32le()? as usize;
        let _frame_size                 = br.read_u32le()? as usize;
        let first_frame_offset          = br.read_u32le()?;
        let end_of_frame_data           = br.read_u32le()?;
        let frame_hdr_size              = br.read_u16le()? as usize;
        if frame_hdr_size != 16 { return Err(DemuxerError::NotImplemented); }
        let frame_dir_size              = br.read_u16le()? as usize;
        if frame_dir_size != 4 { return Err(DemuxerError::NotImplemented); }
        let frame_dir_offset            = br.read_u32le()?;
        validate!((frame_dir_offset >= data_start && frame_dir_offset < first_frame_offset) || (frame_dir_offset >= end_of_frame_data));
        let _frame_dir_version          = br.read_u16le()?;
        let fps                         = br.read_u16le()?;
        validate!(fps > 0);
        let _update_flag                = br.read_u32le()?;
        let _free_block_offset          = br.read_u32le()?;
                                          br.read_skip(32)?; // patch - unused

        br.seek(SeekFrom::Start(u64::from(stream_offset)))?;

        self.fsizes = vec![0; stream_count];
        let mut substream_pos = u64::from(hdr_pool_offset);
        for stream_id in 0..stream_count {
            let tag                     = br.read_tag()?;
            validate!(&tag == b"MRTS");
            let hdr_type                = br.read_u16le()?;
            let hdr_subtype             = br.read_u16le()?;
            let hdr_count               = br.read_u16le()?;
            validate!(hdr_count == 1);
            let _next_stream            = br.read_u16le()?;
            let _group_num              = br.read_u16le()?;
                                          br.read_skip(2)?; // pad
            let _flags                  = br.read_u32le()?;
            let _max_frame_size         = br.read_u32le()? as usize;
            let _first_offset           = br.read_u32le()?;
                                          br.read_skip(16)?; // stream name
            let stream_hdr_pos = br.tell();
            br.seek(SeekFrom::Start(substream_pos))?;
            match hdr_type {
                2 => { // audio
                    let tag             = br.read_tag()?;
                    validate!(&tag == b"IDUA");
                    let size            = br.read_u16le()? as usize;
                    validate!(size >= 168);
                    // TODO support PCM or IMA ADPCM if there are any samples
                                          br.read_skip(size - 6)?;
                },
                3 => { // video
                    let tag             = br.read_tag()?;
                    validate!(&tag == b"GMIC");
                    let size            = br.read_u16le()? as usize;
                    validate!(size >= 136);
                    let _header_version = br.read_u16le()?;
                                          br.read_skip(80)?; // original file name
                    let _orig_frame     = br.read_u32le()?;
                    let _orig_id        = br.read_u16le()?;
                                          br.read_skip(2)?; // pad
                    let _frame_count    = br.read_u32le()? as usize;
                    let _next_offset    = br.read_u32le()?;
                    let _xpos           = br.read_u16le()? as usize;
                    let _ypos           = br.read_u16le()? as usize;
                    let width           = br.read_u16le()? as usize;
                    let height          = br.read_u16le()? as usize;
                    validate!(width > 0 && height > 0);
                    let _xkoda          = br.read_u16le()? as usize;
                    let _ykoda          = br.read_u16le()? as usize;
                    let _drop_frame     = br.read_u16le()?;
                    let _drop_phrase    = br.read_u16le()?;
                    let _still_period   = br.read_u32le()?;
                    let _buf_min        = br.read_u16le()? as usize;
                    let _buf_max        = br.read_u16le()? as usize;
                    let codec_id        = br.read_u16le()?;
                                          br.read_skip(2)?; // pad
                    let _dcfid          = br.read_u32le()?;
                                          br.read_skip(size - 136)?;

                    let cname = match codec_id {
                            0x01 | 0x02 | 0x80 => "ima-pic",
                            0x05 | 0x06 => "ima-plv",
                            0x14 | 0x15 => "ima-plv2",
                            0x81 => "jpeg",
                            0xC0 | 0xC2 | 0xC3 => "ima-rtv1",
                            0xC8 | 0xC9 | 0xCA => "ima-rtv2",
                            _ => "unknown",
                        };

                    let fmt = match hdr_subtype {
                            1 | 11 | 12 => GRAY_FORMAT, // Y/U/V component only
                            _ => YUV410_FORMAT,
                        };

                    let mut vhdr = NAVideoInfo::new(width, height, false, fmt);
                    vhdr.bits = 24;

                    let vinfo = NACodecInfo::new(cname, NACodecTypeInfo::Video(vhdr), None);
                    if strmgr.add_stream(NAStream::new(StreamType::Video, stream_id as u32, vinfo, 1, u32::from(fps), frame_count as u64)).is_none() {
                        return Err(DemuxerError::MemoryError);
                    }
                },
                _ => {
                    let _tag            = br.read_tag()?;
                    let size            = br.read_u16le()? as usize;
                    validate!(size >= 6);
                                          br.read_skip(size - 6)?;
                },
            }
            substream_pos = br.tell();
            br.seek(SeekFrom::Start(stream_hdr_pos))?;
        }
        br.seek(SeekFrom::Start(u64::from(first_frame_offset)))?;
        self.data_end = u64::from(end_of_frame_data);
        Ok(())
    }

    fn get_frame(&mut self, strmgr: &mut StreamManager) -> DemuxerResult<NAPacket> {
        while self.src.tell() < self.data_end {
            if let Some(pkt) = self.packets.pop_front() {
                return Ok(pkt);
            }

            let mut hdr = [0; 12];
                                      self.src.read_buf(&mut hdr)?;
            let frame_id  = read_u32le(&hdr).unwrap_or(0);
            for el in self.fsizes.iter_mut() {
                *el                 = self.src.read_u32le()? as usize;
            }
            for (id, &size) in self.fsizes.iter().enumerate() {
                if let Some(stream) = strmgr.get_stream_by_id(id as u32) {
                    let mut buf = vec![0; size + 16];
                    buf[..12].copy_from_slice(&hdr);
                    write_u32le(&mut buf[12..16], size as u32).unwrap();
                                      self.src.read_buf(&mut buf[16..])?;

                    let ts = stream.make_ts(Some(u64::from(frame_id)), None, None);
                    let pkt = NAPacket::new_from_refbuf(stream, ts, frame_id == 0, NABufferRef::new(buf));
                    self.packets.push_back(pkt);
                } else {
                                      self.src.read_skip(size)?;
                }
            }
        }
        Err(DemuxerError::EOF)
    }
    fn seek(&mut self, _time: NATimePoint, _seek_index: &SeekIndex) -> DemuxerResult<()> {
        Err(DemuxerError::NotPossible)
    }
    fn get_duration(&self) -> u64 { 0 }
}

impl<'a> NAOptionHandler for DVIDemuxer<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) {}
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub struct DVIDemuxerCreator { }

impl DemuxerCreator for DVIDemuxerCreator {
    fn new_demuxer<'a>(&self, br: &'a mut ByteReader<'a>) -> Box<dyn DemuxCore<'a> + 'a> {
        Box::new(DVIDemuxer::new(br))
    }
    fn get_name(&self) -> &'static str { "dvi" }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_dvi_demux() {
        // sample comes from a demo by Digital Video Arts
        let mut file = File::open("assets/Indeo/YULELOG.AVS").unwrap();
        let mut fr = FileReader::new_read(&mut file);
        let mut br = ByteReader::new(&mut fr);
        let mut dmx = DVIDemuxer::new(&mut br);
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
