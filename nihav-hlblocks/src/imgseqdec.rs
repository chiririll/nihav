//! Image sequence demuxer
//!
//! This module offers a functionality of representing an image sequence with an object having `Demuxer`-like interface.
use nihav_core::frame::*;
use nihav_core::demuxers::*;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;

struct TemplateName {
    prefix:         String,
    pad_size:       usize,
    suffix:         String,
    single:         bool,
}

trait Deescape {
    fn deescape(&mut self);
}

impl Deescape for String {
    fn deescape(&mut self) {
        while let Some(idx) = self.find("%%") {
            self.remove(idx + 1);
        }
    }
}

impl TemplateName {
    fn new(name: &str) -> Self {
        let mut off = 0;
        let mut tmpl_start = 0;
        let mut tmpl_end = 0;
        let mut pad_size = 0;
        'parse_loop:
        while let Some(idx) = name[off..].find('%') {
            let idx = idx + off;
            if idx + 1 == name.len() {
                break;
            }
            if name[idx + 1..].starts_with('%') { // escape, skip it
                off += 1;
            }
            if name[idx + 1..].starts_with('0') {
                if let Some(end_idx) = name[idx + 2..].find('d') {
                    if let Ok(val) = name[idx + 2..][..end_idx].parse::<usize>() {
                        if val <= 32 {
                            tmpl_start = idx;
                            pad_size = val;
                            tmpl_end = idx + 2 + end_idx + 1;
                        }
                    }
                    break 'parse_loop;
                }
            }
            if name[idx + 1..].starts_with('d') {
                tmpl_start = idx;
                tmpl_end = idx + 2;
                break;
            }
            off += idx;
        }

        if tmpl_end == 0 {
            let mut prefix = name.to_owned();
            prefix.deescape();
            Self {
                prefix,
                pad_size:   0,
                suffix:     String::new(),
                single:     true,
            }
        } else {
            let mut prefix = name[..tmpl_start].to_string();
            prefix.deescape();
            let mut suffix = name[tmpl_end..].to_string();
            suffix.deescape();
            Self {
                prefix, suffix, pad_size,
                single: false,
            }
        }
    }
    fn format<T: Sized+ToString>(&self, id: T) -> String {
        let mut number = id.to_string();
        while number.len() < self.pad_size {
            number.insert(0, '0');
        }
        let mut fname = String::with_capacity(self.prefix.len() + number.len() + self.suffix.len());
        fname.push_str(&self.prefix);
        fname.push_str(&number);
        fname.push_str(&self.suffix);
        fname
    }
}

/// Image sequence demuxer
///
/// Use [`ImgSeqDemuxerCreator`] to create an instance of it.
///
/// [`ImgSeqDemuxerCreator`]: ./struct.ImgSeqDemuxerCreator.html
pub struct ImgSeqDemuxer {
    pub stream:     NAStreamRef,
    pub sm:         StreamManager,
    cur_frame:      u64,
    template:       TemplateName,
    pgmyuv:         bool,
}

impl ImgSeqDemuxer {
    fn new(stream: NAStreamRef, cur_frame: u64, template: TemplateName, pgmyuv: bool) -> Self {
        let mut sm = StreamManager::new();
        sm.add_stream_ref(stream.clone());
        Self {
            stream, sm, cur_frame, template, pgmyuv,
        }
    }
    /// Seeks to the requested time if possible.
    pub fn seek(&mut self, time: NATimePoint) -> DemuxerResult<()> {
        self.cur_frame = match time {
                NATimePoint::None => return Ok(()),
                NATimePoint::Milliseconds(ms) => NATimeInfo::rescale_ts(ms, 1, 1000, self.stream.tb_num, self.stream.tb_den),
                NATimePoint::PTS(pts) => pts,
            };
        Ok(())
    }
    /// Demuxes a packet.
    pub fn get_frame(&mut self) -> DemuxerResult<NAPacket> {
        if self.cur_frame > 0 && self.template.single {
            return Err(DemuxerError::EOF);
        }
        let fname = self.template.format(self.cur_frame);
        if let Ok(file) = File::open(fname.as_str()) {
            let mut file = BufReader::new(file);
            let vinfo = read_pnm_header(&mut file, self.pgmyuv)?;
            let pkt_size = if vinfo.format.model.is_yuv() && vinfo.format.components == 3 {
                    vinfo.width * (vinfo.height * 3 / 2) * if vinfo.format.get_max_depth() > 8 { 2 } else { 1 }
                } else {
                    vinfo.width * vinfo.height * usize::from(vinfo.format.components)
                };
            let mut buf = vec![0; pkt_size];
            file.read_exact(&mut buf).map_err(|_| DemuxerError::IOError)?;
            let ts = NATimeInfo::new(Some(self.cur_frame), None, None, self.stream.tb_num, self.stream.tb_den);
            let pkt = NAPacket::new(self.stream.clone(), ts, true, buf);
            self.cur_frame += 1;
            Ok(pkt)
        } else {
            Err(DemuxerError::EOF)
        }
    }
}

impl NAOptionHandler for ImgSeqDemuxer {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) {}
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

/// Helper object that creates image sequence demuxer
///
/// By default it will try to open images using the provided simplified `printf`-like template e.g. `file%d.ppm` will be expanded into `file0.ppm`, `file1.ppm`, `file2.ppm` ... and `img%03d.pnm` will be expanded into `img000.pnm`, `img001.pnm` etc.
/// Please note that passing single image names like `picture.pgm` is fine, also if `img000.ppm` is not found but `img001.ppm` is then decoding will start from it. You can set start image in the sequence with `start` option.
///
/// Other useful options are:
/// * `tb_num`/`tb_den` pair allows to set custom timebase for the sequence
/// * `pgmyuv` -- tells the demuxer that input PGM images are not greyscale images but rather YUV 4:2:0 data with chroma planes next to each other and below luma plane data.
pub struct ImgSeqDemuxerCreator<'a> {
    name:       &'a str,
    start:      Option<u64>,
    pgmyuv:     bool,
    tb_num:     u32,
    tb_den:     u32,
}

impl<'a> ImgSeqDemuxerCreator<'a> {
    /// Creates a new instance of `ImgSeqDemuxerCreator`, allowing to set options before actual demuxer is created.
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            start:      None,
            pgmyuv:     false,
            tb_num:     1,
            tb_den:     25,
        }
    }
    /// Tries to create new image sequence demuxer.
    ///
    /// Note: for setting stream properties the first image in the sequence is checked.
    pub fn open(&mut self) -> DemuxerResult<ImgSeqDemuxer> {
        let template = TemplateName::new(self.name);

        let fname = template.format(self.start.unwrap_or(0));

        if let Ok(file) = File::open(fname.as_str()) {
            let mut file = BufReader::new(file);
            let vinfo = read_pnm_header(&mut file, self.pgmyuv)?;
            let cinfo = NACodecInfo::new("rawvideo", NACodecTypeInfo::Video(vinfo), None);
            let stream = NAStream::new(StreamType::Video, 0, cinfo, self.tb_num, self.tb_den, 0).into_ref();
            return Ok(ImgSeqDemuxer::new(stream, self.start.unwrap_or(0), template, self.pgmyuv));
        }

        // if start is not given, try also starting from one
        if self.start.is_none() {
            let new_start = 1;
            let fname = template.format(new_start);
            if let Ok(file) = File::open(fname.as_str()) {
                let mut file = BufReader::new(file);
                let vinfo = read_pnm_header(&mut file, self.pgmyuv)?;
                let cinfo = NACodecInfo::new("rawvideo", NACodecTypeInfo::Video(vinfo), None);
                let stream = NAStream::new(StreamType::Video, 0, cinfo, self.tb_num, self.tb_den, 0).into_ref();
                return Ok(ImgSeqDemuxer::new(stream, new_start, template, self.pgmyuv));
            }
        }

        Err(DemuxerError::NoSuchInput)
    }
}

const IMGSEQ_OPTIONS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name: "start", description: "start frame number",
        opt_type: NAOptionDefinitionType::Int(Some(0), None) },
    NAOptionDefinition {
        name: "pgmyuv", description: "Input is in PGMYUV format",
        opt_type: NAOptionDefinitionType::Bool },
    NAOptionDefinition {
        name: "tb_num", description: "timebase numerator",
        opt_type: NAOptionDefinitionType::Int(Some(1), Some(1000000)) },
    NAOptionDefinition {
        name: "tb_den", description: "timebase denominator",
        opt_type: NAOptionDefinitionType::Int(Some(1), Some(1000000)) },
];

impl<'a> NAOptionHandler for ImgSeqDemuxerCreator<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { IMGSEQ_OPTIONS }
    fn set_options(&mut self, options: &[NAOption]) {
        for option in options.iter() {
            for opt_def in IMGSEQ_OPTIONS.iter() {
                if opt_def.check(option).is_ok() {
                    match (option.name, &option.value) {
                        ("start", NAValue::Int(intval)) => {
                            self.start = Some(*intval as u64);
                        },
                        ("pgmyuv", NAValue::Bool(bval)) => {
                            self.pgmyuv = *bval;
                        },
                        ("tb_num", NAValue::Int(intval)) => {
                            self.tb_num = *intval as u32;
                        },
                        ("tb_den", NAValue::Int(intval)) => {
                            self.tb_den = *intval as u32;
                        },
                        _ => {},
                    }
                }
            }
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match name {
            "start" => Some(NAValue::Int(self.start.unwrap_or(0) as i64)),
            "pgmyuv" => Some(NAValue::Bool(self.pgmyuv)),
            "tb_num" => Some(NAValue::Int(self.tb_num as i64)),
            "tb_den" => Some(NAValue::Int(self.tb_den as i64)),
            _ => None,
        }
    }
}

fn read_pnm_header(file: &mut BufReader<File>, pgmyuv: bool) -> DemuxerResult<NAVideoInfo> {
    let mut br = FileReader::new_read(file);

    let mut magic = [0; 2];
    br.read_buf(&mut magic)?;
    if magic[0] != b'P' { return Err(DemuxerError::InvalidData); }
    match magic[1] {
        b'4' | // PBM, PBM ASCII
        b'1' => return Err(DemuxerError::NotImplemented),
        b'5' => { // PGM
        },
        b'2' => return Err(DemuxerError::NotImplemented), // PGM ASCII
        b'6' => { // PPM
        },
        b'3' => return Err(DemuxerError::NotImplemented), // PPM ASCII
        _ => return Err(DemuxerError::InvalidData),
    };
    if br.read_byte()? != b'\n' { return Err(DemuxerError::InvalidData); }
    let w = read_number(&mut br)?;
    let h = read_number(&mut br)?;
    let maxval = if matches!(magic[1], b'4' | b'1') { 1 } else { read_number(&mut br)? };
    if maxval > 65535 || (maxval & (maxval + 1)) != 0 { return Err(DemuxerError::InvalidData); }
    let bits = maxval.count_ones() as u8;

    let mut vinfo = NAVideoInfo::new(w, h, false, RGB24_FORMAT);
    match magic[1] {
        b'5' | b'2' if !pgmyuv => {
            vinfo.format = NAPixelFormaton {
                    model: ColorModel::YUV(YUVSubmodel::YUVJ),
                    components: 1,
                    comp_info: [Some(NAPixelChromaton{h_ss: 0, v_ss: 0, packed: false, depth: 8, shift: 0, comp_offs: 0, next_elem: 1}), None, None, None, None],
                    elem_size: 1,
                    be: true,
                    alpha: false,
                    palette: false,
                };
            vinfo.bits = bits;
        },
        b'5' | b'2' => {
            if ((w & 1) != 0) || ((h % 3) != 0) { return Err(DemuxerError::InvalidData); }
            vinfo.format = YUV420_FORMAT;
            vinfo.height = h * 2 / 3;
            vinfo.bits = bits * 3 / 2;
        },
        b'6' | b'3' => {
            vinfo.format = RGB24_FORMAT;
            vinfo.bits = bits;
        },
        _ => unreachable!(),
    };
    if bits != 8 {
        for chr in vinfo.format.comp_info.iter_mut().flatten() {
            chr.depth = bits;
            if bits > 8 {
                chr.next_elem = 2;
            }
        }
        if bits > 8 {
            vinfo.format.elem_size <<= 1;
        }
    }

    Ok(vinfo)
}

fn read_number(br: &mut dyn ByteIO) -> DemuxerResult<usize> {
    let mut val = 0;
    loop {
        let c = br.read_byte()?;
        match c {
            b'0'..=b'9' => {
                if val > 1048576 {
                    return Err(DemuxerError::InvalidData);
                }
                val = val * 10 + usize::from(c - b'0');
            },
            b' ' | b'\n' => break,
            _ => return Err(DemuxerError::InvalidData),
        };
    }
    Ok(val)
}
