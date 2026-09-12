//! Common interface and functionality for demuxing data.
//!
//! Currently there are three types of demuxing possible:
//! * ordinary demuxer that returns packets from one or more embedded streams;
//! * raw streams demuxer (e.g. MPEG-2 TS) that returns chunks of raw data and requires a packetiser to form them into proper packets;
//! * raw stream without any additional headers (e.g. MP3) that also requires packetising.
//!
//! This module provides functionality to automatically detect input format and select appropriate
//! implementation that will still behave like an ordinary `Demuxer` object that returns full packets.

use std::io::SeekFrom;
use nihav_core::codecs::*;
use nihav_core::demuxers::*;
use nihav_core::muxers::RegisteredMuxers;
use nihav_registry::detect;
use nihav_core::io::byteio::ByteIO;
use nihav_core::sbbox::*;
#[cfg(feature="imgseq_dec")]
use crate::imgseqdec::ImgSeqDemuxer;

/// A helper structure to pass references to all registered (de)muxers, encoders, decoders and such.
///
/// The most convenient way to populate it is to invoke `nihav_register_all_{demuxers,encoders,ets}` from `nihav_allstuff` crate on corresponding structure members.
#[derive(Default)]
pub struct FullRegister {
    /// Registered demuxers
    pub dmx_reg:    RegisteredDemuxers,
    /// Registered raw stream demuxers
    pub rdmx_reg:   RegisteredRawDemuxers,
    /// Registered packetisers
    pub pkt_reg:    RegisteredPacketisers,
    /// Registered decoders
    pub dec_reg:    RegisteredDecoders,
    /// Registered multi-threaded decoders
    pub mtdec_reg:  RegisteredMTDecoders,
    /// Registered muxers
    pub mux_reg:    RegisteredMuxers,
    /// Registered encoders
    pub enc_reg:    RegisteredEncoders,
}

impl FullRegister {
    /// Creates a new empty instance of `FullRegister`.
    pub fn new() -> Self { Self::default() }
}

/// Auxiliary structure to deal with single input raw stream and packetising it. Not for the direct use.
pub struct RawStreamCtx<'a> {
    stream: NAStreamRef,
    sm:     StreamManager,
    pkt:    Box<dyn NAPacketiser + Send>,
    br:     &'a mut dyn ByteIO,
    pts:    u64,
    seek:   SeekIndex,
}

impl<'a> RawStreamCtx<'a> {
    fn new(stream: NAStreamRef, packetiser: Box<dyn NAPacketiser + Send>, br: &'a mut dyn ByteIO) -> Self {
        let mut sm = StreamManager::new();
        sm.add_stream_ref(stream.clone());
        let mut seek = SeekIndex::new();
        seek.add_stream(0);
        Self { stream, pkt: packetiser, br, pts: 0, sm, seek }
    }
    fn account_for_packet(&mut self, packet: &mut NAPacket) {
        let pos = self.br.tell() - (self.pkt.bytes_left() as u64);
        if packet.get_pts().is_none() && packet.get_duration().is_some() {
            packet.ts.pts = Some(self.pts);
        }
        if packet.is_keyframe() {
            let pts = packet.get_pts().unwrap_or(self.pts);
            let time = NATimeInfo::rescale_ts(pts, self.stream.tb_num, self.stream.tb_den, 1, 1000);
            let in_range = if let Some(last) = self.seek.seek_info[0].entries.last() {
                    last.pts >= pts
                } else {
                    false
                };
            if !in_range {
                self.seek.add_entry(0, SeekEntry { time, pts, pos });
            }
        }
        self.pts += packet.get_duration().unwrap_or(0);
    }
    fn get_frame(&mut self) -> DemuxerResult<NAPacket> {
        let mut buf = [0; 1048576];
        loop {
            match self.pkt.get_packet(self.stream.clone()) {
                Ok(Some(mut packet)) => {
                    self.account_for_packet(&mut packet);
                    return Ok(packet);
                },
                Ok(None) => {},
                Err(DecoderError::ShortData) => {},
                _ => return Err(DemuxerError::InvalidData),
            };
            match self.br.read_buf_some(&mut buf) {
                Ok(size) => {
                    self.pkt.add_data(&buf[..size]);
                },
                Err(_) => {
                    match self.pkt.get_packet(self.stream.clone()) {
                        Ok(Some(mut packet)) => {
                            self.account_for_packet(&mut packet);
                            return Ok(packet);
                        },
                        Ok(None) | Err(DecoderError::ShortData) => return Err(DemuxerError::EOF),
                        _ => return Err(DemuxerError::InvalidData),
                    };
                },
            };
        }
    }
}

/// Auxiliary structure to maintain the state of raw streams demuxer. Not for the direct use.
pub struct RawDemuxerState<'a> {
    dmx:    SBBox<ReaderBox, RawDemuxer<'a>>,
    pkts:   Vec<Option<Box<dyn NAPacketiser + Send>>>,
    eof:    bool,
    dname:  String,
}

/// Alias for input source
pub type ReaderBox = Box<dyn ByteIO>;

/// Wrapper that provides common interface a la `Demuxer` for different types of input.
pub enum DemuxerObject<'a> {
    /// Nothing created yet
    None,
    /// Ordinary demuxer
    Normal(SBBox<ReaderBox, Demuxer<'a>>, String),
    /// Raw stream demuxer with individual stream contexts
    Raw(RawDemuxerState<'a>),
    /// Elementary stream
    RawStream(SBBox<ReaderBox, RawStreamCtx<'a>>, String),
    #[cfg(feature="imgseq_dec")]
    /// Image sequence
    ImageSequence(ImgSeqDemuxer),
}

impl<'a> DemuxerObject<'a> {
    fn new_demuxer(mut br: ReaderBox, reg: &FullRegister, dmx_name: &str, opts: &[NAOption], verbose: bool, forced: bool) -> Result<DemuxerObject<'a>, ReaderBox> {
        if let Some(dmx_fact) = reg.dmx_reg.find_demuxer(dmx_name) {
            if verbose {
                println!(" {} demuxer {dmx_name}", if forced { "forcing" } else { "trying" });
            }
            br.seek(SeekFrom::Start(0)).unwrap();
            SelfBorrow::try_new(br, |br_| {
                unsafe {
                    create_demuxer_with_options(dmx_fact, (*br_).as_mut(), opts).ok()
                }
            }).map(|dmx| DemuxerObject::Normal(dmx, dmx_name.to_string()))
        } else {
            Err(br)
        }
    }
    fn new_raw_demuxer(mut br: ReaderBox, reg: &FullRegister, dmx_name: &str, opts: &[NAOption], verbose: bool, forced: bool) -> Result<DemuxerObject<'a>, ReaderBox> {
        if let Some(rdmx_fact) = reg.rdmx_reg.find_demuxer(dmx_name) {
            if verbose {
                println!(" {} raw demuxer {dmx_name}", if forced { "forcing" } else { "trying" });
            }
            br.seek(SeekFrom::Start(0)).unwrap();
            let dmx = SelfBorrow::try_new(br, |br_| {
                    unsafe {
                        create_raw_demuxer_with_options(rdmx_fact, (*br_).as_mut(), opts).ok()
                    }
                })?;
            let mut pkts = Vec::new();
            for stream in dmx.get_object().get_streams() {
                if let Some(pcreate) = reg.pkt_reg.find_packetiser(stream.get_info().get_name()) {
                    let mut packetiser = (pcreate)();
                    packetiser.attach_stream(stream);
                    pkts.push(Some(packetiser));
                } else {
                    pkts.push(None);
                }
            }
            Ok(DemuxerObject::Raw(RawDemuxerState{ dmx, pkts, eof: false, dname: rdmx_fact.get_name().to_string() }))
        } else {
            Err(br)
        }
    }

    /// Attempts to create a new instance of `DemuxerObject`.
    ///
    /// Input parameters:
    /// * `br` --- input I/O context
    /// * `reg` --- register with at least some of the demuxers or raw stream demuxers with packetisers being registered
    /// * `name` --- input file name (used for format detection)
    /// * `force_dmx` --- optionally provided demuxer name (set to `None` to have autodetection instead)
    /// * `is_raw` --- skips attempting to create a demuxer and treats input as an elementary stream
    /// * `opts` --- demuxer options
    /// * `verbose` -- enables printing messages e.g. for debugging purposes
    pub fn create(mut br: ReaderBox, reg: &FullRegister, name: &str, force_dmx: Option<&str>, is_raw: bool, opts: &[NAOption], verbose: bool) -> DemuxerObject<'a> {
        if !is_raw {
            if let Some(dmx_name) = force_dmx {
                match Self::new_demuxer(br, reg, dmx_name, opts, verbose, true) {
                    Ok(dmx) => return dmx,
                    Err(nbr) => br = nbr,
                };
                if let Ok(dmx) = Self::new_raw_demuxer(br, reg, dmx_name, opts, verbose, true) {
                    return dmx;
                } else {
                    return DemuxerObject::None;
                }
            }
            let res = detect::detect_format(name, &mut *br);
            let (dmx_name, _) = res.unwrap_or(("", detect::DetectionScore::No));
            if !dmx_name.is_empty() {
                match Self::new_demuxer(br, reg, dmx_name, opts, verbose, false) {
                    Ok(dmx) => return dmx,
                    Err(nbr) => br = nbr,
                }
            }
            if !dmx_name.is_empty() {
                match Self::new_raw_demuxer(br, reg, dmx_name, opts, verbose, false) {
                    Ok(dmx) => return dmx,
                    Err(nbr) => br = nbr,
                }
            }
            for rdmx in reg.rdmx_reg.iter() {
                if rdmx.check_format(&mut *br) {
                    if verbose {
                        println!(" detected as {}", rdmx.get_name());
                    }
                    match Self::new_raw_demuxer(br, reg, rdmx.get_name(), opts, false, false) {
                        Ok(dmx) => return dmx,
                        Err(nbr) => br = nbr,
                    }
                }
            }
        }
        br.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = vec![0; 1048576];
        let size = br.read_buf_some(&mut buf).unwrap_or(0);
        br.seek(SeekFrom::Start(0)).unwrap();
        let mut pname = "";

        for pinfo in reg.pkt_reg.iter() {
            let mut packetiser = (pinfo.get_packetiser)();
            packetiser.add_data(&buf[..size]);
            if packetiser.parse_stream(0).is_ok() {
                pname = pinfo.name;
                break;
            }
        }
        if !pname.is_empty() {
            if verbose {
                println!(" found raw stream of type {pname}");
            }
            let pcreate = reg.pkt_reg.find_packetiser(pname).unwrap();
            let rctx = SelfBorrow::new(br, |br_| {
                    unsafe {
                        let mut packetiser = (pcreate)();
                        packetiser.add_data(&buf[..size]);
                        let stream = packetiser.parse_stream(0).unwrap();
                        packetiser.reset();
                        RawStreamCtx::new(stream, packetiser, (*br_).as_mut())
                    }
                });
            DemuxerObject::RawStream(rctx, pname.to_string())
        } else {
            DemuxerObject::None
        }
    }
    /// Wraps image sequence demuxer.
    #[cfg(feature="imgseq_dec")]
    pub fn create_imgseq(isd: ImgSeqDemuxer) -> Self {
        DemuxerObject::ImageSequence(isd)
    }
    /// Checks if there is no actual demuxer created.
    pub fn is_none(&self) -> bool {
        matches!(*self, DemuxerObject::None)
    }
    /// Returns total file duration (or 0 if unknown).
    pub fn get_duration(&self) -> u64 {
        match *self {
            DemuxerObject::Normal(ref dmx, _) => dmx.get_object().get_duration(),
            DemuxerObject::Raw(ref dmxs) => dmxs.dmx.get_object().get_duration(),
            DemuxerObject::RawStream(ref ctx, _) => {
                let stream = &ctx.get_object().stream;
                NATimeInfo::rescale_ts(stream.duration, stream.tb_num, stream.tb_den, 1, 1000)
            },
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(_) => 0,
            _ => 0,
        }
    }
    /// Returns number of present streams.
    pub fn get_num_streams(&self) -> usize {
        match *self {
            DemuxerObject::None => 0,
            DemuxerObject::Normal(ref dmx, _) => dmx.get_object().get_num_streams(),
            DemuxerObject::Raw(ref dmxs) => dmxs.dmx.get_object().get_num_streams(),
            DemuxerObject::RawStream(_, _) => 1,
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(_) => 1,
        }
    }
    /// Returns stream with the requested index.
    pub fn get_stream(&self, idx: usize) -> Option<NAStreamRef> {
        match *self {
            DemuxerObject::Normal(ref dmx, _) => dmx.get_object().get_stream(idx),
            DemuxerObject::Raw(ref dmxs) => dmxs.dmx.get_object().get_stream(idx),
            DemuxerObject::RawStream(ref ctx, _) if idx == 0 => Some(ctx.get_object().stream.clone()),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref ctx) if idx == 0 => Some(ctx.stream.clone()),
            _ => None,
        }
    }
    /// Returns stream manager associated with the demuxer.
    pub fn get_stream_manager(&self) -> &StreamManager {
        match *self {
            DemuxerObject::Normal(ref dmx, _) => dmx.get_object().get_stream_manager(),
            DemuxerObject::Raw(ref dmxs) => dmxs.dmx.get_object().get_stream_manager(),
            DemuxerObject::RawStream(ref ctx, _) => &ctx.get_object().sm,
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref ctx) => &ctx.sm,
            _ => unreachable!(),
        }
    }
    /// Returns mutable reference to the stream manager associated with the demuxer.
    pub fn get_stream_manager_mut(&mut self) -> &mut StreamManager {
        match *self {
            DemuxerObject::Normal(ref mut dmx, _) => dmx.get_object_mut().get_stream_manager_mut(),
            DemuxerObject::Raw(ref mut dmxs) => dmxs.dmx.get_object_mut().get_stream_manager_mut(),
            DemuxerObject::RawStream(ref mut ctx, _) => &mut ctx.get_object_mut().sm,
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref mut ctx) => &mut ctx.sm,
            _ => unreachable!(),
        }
    }
    /// Returns 'ignored' marker for requested stream.
    pub fn is_ignored_stream(&self, idx: usize) -> bool {
        match *self {
            DemuxerObject::Normal(ref dmx, _) => dmx.get_object().is_ignored_stream(idx),
            DemuxerObject::Raw(ref dmxs) => dmxs.dmx.get_object().is_ignored_stream(idx),
            DemuxerObject::RawStream(ref ctx, _) => ctx.get_object().sm.is_ignored(idx),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref ctx) => ctx.sm.is_ignored(idx),
            _ => unreachable!(),
        }
    }
    /// Sets 'ignored' marker for requested stream.
    pub fn set_ignored_stream(&mut self, idx: usize) {
        match *self {
            DemuxerObject::Normal(ref mut dmx, _) => dmx.get_object_mut().set_ignored_stream(idx),
            DemuxerObject::Raw(ref mut dmxs) => dmxs.dmx.get_object_mut().set_ignored_stream(idx),
            DemuxerObject::RawStream(ref mut ctx, _) => ctx.get_object_mut().sm.set_ignored(idx),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref mut ctx) => ctx.sm.set_ignored(idx),
            _ => unreachable!(),
        }
    }
    /// Clears 'ignored' marker for requested stream.
    pub fn set_unignored_stream(&mut self, idx: usize) {
        match *self {
            DemuxerObject::Normal(ref mut dmx, _) => dmx.get_object_mut().set_unignored_stream(idx),
            DemuxerObject::Raw(ref mut dmxs) => dmxs.dmx.get_object_mut().set_unignored_stream(idx),
            DemuxerObject::RawStream(ref mut ctx, _) => ctx.get_object_mut().sm.set_unignored(idx),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref mut ctx) => ctx.sm.set_unignored(idx),
            _ => unreachable!(),
        }
    }
    /// Demuxes a packet.
    pub fn get_frame(&mut self) -> DemuxerResult<NAPacket> {
        match *self {
            DemuxerObject::Normal(ref mut dmx, _) => dmx.get_object_mut().get_frame(),
            DemuxerObject::Raw(ref mut dmxs) => {
                loop {
                    let mut has_some = false;
                    for (stream, p) in dmxs.dmx.get_object().get_streams().zip(dmxs.pkts.iter_mut()) {
                        if let Some(ref mut pkts) = p {
                            match pkts.get_packet(stream.clone()) {
                                Ok(Some(pkt)) => return Ok(pkt),
                                Ok(None) | Err(DecoderError::ShortData) => {
                                    if dmxs.eof {
                                        *p = None;
                                    }
                                },
                                Err(err) => {
                                    println!("packetisation error {:?}", err);
                                    return Err(DemuxerError::InvalidData);
                                }
                            };
                            has_some |= p.is_some();
                        }
                    }
                    if !has_some {
                        return Err(DemuxerError::EOF);
                    }
                    if let Ok(data) = dmxs.dmx.get_object_mut().get_data() {
                        let id = data.get_stream().get_id();
                        for (i, stream) in dmxs.dmx.get_object().get_streams().enumerate() {
                            if stream.get_id() == id {
                                if let Some(ref mut pkts) = dmxs.pkts[i] {
                                    pkts.add_data(&data.get_buffer());
                                }
                                break;
                            }
                        }
                    } else {
                        dmxs.eof = true;
                    }
                }
            },
            DemuxerObject::RawStream(ref mut ctx, _) => ctx.get_object_mut().get_frame(),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref mut ctx) => ctx.get_frame(),
            _ => unreachable!(),
        }
    }
    /// Seeks to the specified time.
    pub fn seek(&mut self, seek_time: NATimePoint) -> DemuxerResult<()> {
        match *self {
            DemuxerObject::Normal(ref mut dmx, _) => dmx.get_object_mut().seek(seek_time),
            DemuxerObject::Raw(ref mut dmxs) => dmxs.dmx.get_object_mut().seek(seek_time),
            DemuxerObject::RawStream(ref mut ctxobj, _) => {
                let ctx = ctxobj.get_object_mut();
                if seek_time == NATimePoint::None {
                    return Err(DemuxerError::SeekError);
                }
                if let Some(last) = ctx.seek.seek_info[0].entries.last() {
                    let in_index = match seek_time {
                            NATimePoint::None => unreachable!(),
                            NATimePoint::PTS(pts) => last.pts >= pts,
                            NATimePoint::Milliseconds(ms) => last.time >= ms,
                        };
                    if in_index {
                        if let Some(result) = ctx.seek.find_pos(seek_time) {
                            ctx.br.seek(SeekFrom::Start(result.pos))?;
                            ctx.pts = result.pts;
                            ctx.pkt.reset();
                            return Ok(());
                        }
                    }
                }
                if let Some(last) = ctx.seek.seek_info[0].entries.last() {
                    ctx.br.seek(SeekFrom::Start(last.pos))?;
                    ctx.pts = last.pts;
                    ctx.pkt.reset();
                }
                let mut key_pts = 0;
                while let Ok(pkt) = ctx.get_frame() {
                    if !pkt.ts.less_than(seek_time) && !pkt.ts.equal(seek_time) {
                        break;
                    }
                    if pkt.is_keyframe() {
                        key_pts = pkt.get_pts().unwrap_or(0);
                    }
                }
                let result = ctx.seek.find_pos(NATimePoint::PTS(key_pts)).unwrap();
                ctx.br.seek(SeekFrom::Start(result.pos))?;
                ctx.pts = result.pts;
                ctx.pkt.reset();
                Ok(())
            },
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref mut ctx) => ctx.seek(seek_time),
            _ => Err(DemuxerError::NotImplemented),
        }
    }
}

impl<'a> std::fmt::Display for DemuxerObject<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match *self {
            DemuxerObject::None => write!(f, "none"),
            DemuxerObject::Normal(_, ref dtype) => write!(f, "{dtype}"),
            DemuxerObject::Raw(ref dmxs) => write!(f, "{}(raw)", dmxs.dname),
            DemuxerObject::RawStream(_, ref dtype) => write!(f, "{dtype}(raw stream)"),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(_) => write!(f, "image sequence"),
        }
    }
}

impl<'a> NAOptionHandler for DemuxerObject<'a> {
    fn get_supported_options(&self) -> &[NAOptionDefinition] {
        match *self {
            DemuxerObject::Normal(ref dmx, _) => dmx.get_object().get_supported_options(),
            DemuxerObject::Raw(ref dmxs) => dmxs.dmx.get_object().get_supported_options(),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref ctx) => ctx.get_supported_options(),
            _ => &[],
        }
    }
    fn set_options(&mut self, options: &[NAOption]) {
        match *self {
            DemuxerObject::Normal(ref mut dmx, _) => dmx.get_object_mut().set_options(options),
            DemuxerObject::Raw(ref mut dmxs) => dmxs.dmx.get_object_mut().set_options(options),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref mut ctx) => ctx.set_options(options),
            _ => {},
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match *self {
            DemuxerObject::Normal(ref dmx, _) => dmx.get_object().query_option_value(name),
            DemuxerObject::Raw(ref dmxs) => dmxs.dmx.get_object().query_option_value(name),
            #[cfg(feature="imgseq_dec")]
            DemuxerObject::ImageSequence(ref ctx) => ctx.query_option_value(name),
            _ => None,
        }
    }
}

/// Checks input for various metadata tags.
///
/// Music files often have various metadata (e.g. title, artist, genre etc) associated with them but not always being a part of the file/stream format but rather a generic extension prepended or appended to the elementary stream.
/// MP3 with ID3 tags is the most common example of such files.
///
/// This functions allows detecting several of such extensions reporting whether it expects stream to be raw, start position and end position (if necessary).
/// For example, MP3 with first 4200 bytes being an ID3 tag will result in `(true, 4200, None)` value while FLAC file with APE tags at the end will return something like `(false, 0, Some(424242))`.
pub fn detect_tags(br: &mut dyn ByteIO, verbose: bool) -> (bool, u64, Option<u64>) {
    let mut is_raw = false;
    let mut start = 0;
    let mut end = None;

    // check for RIFF MP3 container
    {
        let mut buf = [0; 20];
        if br.peek_buf(&mut buf).is_ok() && &buf[..4] == b"RIFF" && &buf[8..16] == b"RMP3data" {
            let size = read_u32le(&buf[16..]).unwrap_or_default();
            return (true, 20, Some(u64::from(size) + 20));
        }
    }
    // check for ID3v{2-4}
    let mut buf = [0; 5];
    loop {
        if br.peek_buf(&mut buf).is_err() {
            break;
        }
        if &buf[0..3] == b"ID3" && buf[3] > 0 && buf[3] < 5 && buf[4] == 0 { //ID3 tag found, must be a raw stream
            br.read_skip(6).unwrap();
            let mut size = 0;
            for _ in 0..4 {
                let b = br.read_byte().unwrap();
                if (b & 0x80) != 0 {
                    if verbose {
                        println!("Invalid ID3 size");
                    }
                    break;
                }
                size = (size << 7) | u64::from(b);
            }
            start += size + 10;
            br.read_skip(size as usize).unwrap();
            while let Ok(0) = br.read_byte() {
                start += 1;
            }
            br.seek(SeekFrom::Start(start)).unwrap();
            is_raw = true;
        } else {
            break;
        }
    }
    // check for ID3v1
    if br.seek(SeekFrom::End(-128)).is_err() {
        return (is_raw, start, None); // the stream is too short for tail tags
    }
    let off = br.tell();
    br.peek_buf(&mut buf[..3]).unwrap();
    if &buf[0..3] == b"TAG" {
        end = Some(off);
        // check for Lyrics v2
        let mut sig = [0; 9];
        br.seek(SeekFrom::End(-128 - 9)).unwrap();
        br.peek_buf(&mut sig).unwrap();
        if &sig == b"LYRICS200" {
            br.seek(SeekFrom::Current(-6)).unwrap();
            let mut sizestr = [0; 6];
            br.peek_buf(&mut sizestr).unwrap();
            if let Ok(sstr) = std::str::from_utf8(&sizestr) {
                if let Ok(size) = sstr.parse::<u64>() {
                    end = Some(br.tell() - size);
                }
            }
        }
    }
    // check for APETAG
    let mut buf = [0; 8];
    if let Some(off) = end {
        br.seek(SeekFrom::Start(off - 32)).unwrap();
    } else {
        br.seek(SeekFrom::End(-32)).unwrap();
    }
    let off = br.tell();
    br.read_buf(&mut buf).unwrap();
    if &buf == b"APETAGEX" {
        let ver     = br.read_u32le().unwrap();
        let size    = u64::from(br.read_u32le().unwrap());
        let _items  = br.read_u32le().unwrap();
        let flags   = br.read_u32le().unwrap();
        if ver == 1000 || (flags & 0x80000000) == 0 {
            end = Some(off - size + 32);
        } else {
            end = Some(off - size);
        }
    }
    // check for MusicMatch tag
    let ret = if let Some(endpos) = end {
            br.seek(SeekFrom::Start(endpos - 0x30))
        } else {
            br.seek(SeekFrom::End(-0x30))
        };
    if ret.is_ok() && br.tell() > (0x2000 - 0x30) {
        let mut buf = [0; 19];
        br.peek_buf(&mut buf).unwrap();
        if &buf == b"Brava Software Inc." {
            br.seek(SeekFrom::Current(-20)).unwrap();
            let mut mm_start = u64::from(br.read_u32le().unwrap());
            if mm_start > 4 && mm_start + 0x2000 <= br.tell() {
                let diff = (br.tell() - mm_start) & 3;
                if diff != 0 {
                    mm_start -= 4 - diff;
                }
                end = Some(mm_start);
            }
        }
    }

    (is_raw, start, end)
}
