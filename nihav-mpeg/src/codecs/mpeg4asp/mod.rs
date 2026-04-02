use nihav_core::codecs::*;
use nihav_codec_support::codecs::{IPBShuffler, MV};

mod bitstream;
use bitstream::*;
mod decoder;
use decoder::*;
mod dispatch;
use dispatch::*;
mod dsp;
mod types;
use types::*;

struct SingleThreadHelper<'a> {
    last_ref_info:  &'a PicState,
}

impl<'a> MTHelper for SingleThreadHelper<'a> {
    fn wait_for_mb(&mut self, _xpos: usize, _ypos: usize, _id: u32) -> DecoderResult<()> { Ok(()) }
    fn is_colo_mb_not_coded(&mut self, mb_addr: usize, _id: u32) -> DecoderResult<bool> {
        Ok(self.last_ref_info.mb_type[mb_addr] == MBType::Skip)
    }
    fn get_colo_mvs(&mut self, mb_addr: usize, _id: u32) -> DecoderResult<[MV; 4]> {
        Ok(self.last_ref_info.mv[mb_addr])
    }
    fn update_pos(&mut self, _mb_addr: usize) {}
}

#[derive(Default,Debug,PartialEq)]
enum AssignMode {
    #[default]
    Undefined,
    Yes,
    No,
}

struct ASPDecoder {
    info:           NACodecInfoRef,
    skip_mode:      FrameSkipMode,
    ipbs:           IPBShuffler,
    seq:            SequenceContext,
    fctx:           FrameContext,
    saved:          Option<(FrameType, u64, NABufferType)>,
    last_ref_info:  PicState,
    pb_mode:        bool,
    assign_mode:    AssignMode,
    warned_qpel:    bool,
}

impl ASPDecoder {
    fn new() -> Self {
        ASPDecoder{
            info:           NACodecInfoRef::default(),
            skip_mode:      FrameSkipMode::default(),
            ipbs:           IPBShuffler::new(),
            seq:            SequenceContext::new(),
            fctx:           FrameContext::new(),
            saved:          None,
            last_ref_info:  PicState::new(0, 0, 0),
            pb_mode:        false,
            assign_mode:    AssignMode::default(),
            warned_qpel:    false,
        }
    }
}

impl NADecoder for ASPDecoder {
    fn init(&mut self, supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(0, 0, false, YUV420_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            let mut parsed_vol = false;
            if let Some(edata) = info.get_extradata() {
                // ES descriptor, usually happens in MOV
                if edata.len() > 16 && &edata[..4] == b"esds" {
                    let (vol, bugs) = parse_es_descriptor(&edata[8..])?;
                    if vol.quarter_sample && !self.warned_qpel {
                        println!("Warning: quarterpel decoding is not (really) supported");
                        self.warned_qpel = true;
                    }
                    self.fctx.set_qpel(vol.quarter_sample);
                    if bugs.is_xvid_dct() {
                        self.fctx.use_xvid_idct();
                    }
                    if bugs.is_divx_edge_emu() {
                        self.fctx.set_edge_emu_bug();
                    }
                    if bugs.is_direct_mb() {
                        self.fctx.set_solid_dmb();
                    }
                    self.seq.set_vol(vol);
                    parsed_vol = true;
                }
            }
            if !parsed_vol {
                self.seq.set_dimensions(vinfo.get_width(), vinfo.get_height());
            }

            let (width, height) = self.seq.get_dimensions();

            supp.pool_u8.set_dec_bufs(4);
            supp.pool_u8.prealloc_video(NAVideoInfo::new(width, height, false, YUV420_FORMAT), 4)?;

            let mb_w = (width + 15) / 16;
            self.fctx.resize(mb_w);

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();

        let offs = scan_start_codes(&src)?;

        let mut dst_frm = None;
        for range in offs.windows(2) {
            let obj_src = &src[range[0] + 3..range[1]];
            match obj_src[0] {
                0x00..=0x1F => {}, // video_object_start_code
                0x20..=0x3F => { // video_object_layer_start_code
                    let vol = parse_video_object_layer(&obj_src[1..])?;
                    if vol.quarter_sample && !self.warned_qpel {
                        println!("Warning: quarterpel decoding is not (really) supported");
                        self.warned_qpel = true;
                    }
                    self.fctx.set_qpel(vol.quarter_sample);
                    if vol.width != 0 {
                        if self.assign_mode == AssignMode::Undefined && pkt.get_dts().is_none() && !vol.low_delay {
                            let tinfo = pkt.get_time_information();
                            if tinfo.tb_num == 1 && tinfo.tb_den == vol.time_res {
                                self.assign_mode = AssignMode::Yes;
                            } else {
                                println!("Warning: container reported timebase is {}/{} while ASP stream timebase is 1/{} - PTS correction won't be possible!", tinfo.tb_num, tinfo.tb_den, vol.time_res);
                                self.assign_mode = AssignMode::No;
                            }
                        }
                        let tmp_vinfo = NAVideoInfo::new(vol.width, vol.height, false, YUV420_FORMAT);
                        if supp.pool_u8.get_info() != Some(tmp_vinfo) {
                            supp.pool_u8.reset();
                            supp.pool_u8.prealloc_video(tmp_vinfo, 4)?;
                            self.ipbs.clear();
                            let mb_w = (vol.width + 15) / 16;
                            self.fctx.resize(mb_w);
                            self.seq.set_dimensions(vol.width, vol.height);
                        }
                    }
                    self.seq.set_vol(vol);
                },
                0x30..=0xAF | 0xB7..=0xB9 | 0xC4..=0xC5 => {}, // reserved, system start codes
                0xB0 => { // visual_object_sequence_start_code
                    validate!(obj_src.len() >= 2);
                    // single byte is profile and level
                },
                0xB1 => {}, // visual_object_sequence_end_code
                0xB2 => { // user_data_start_code
                    let bugs = check_user_data(obj_src);
                    if bugs.is_xvid_dct() {
                        self.fctx.use_xvid_idct();
                    }
                    if bugs.is_divx_edge_emu() {
                        self.fctx.set_edge_emu_bug();
                    }
                    if bugs.is_direct_mb() {
                        self.fctx.set_solid_dmb();
                    }
                },
                0xB3 => { // group_of_vop_start_code
                    validate!(obj_src.len() == 4);
                    // time_code   - 18 bits
                    // closed_gov  - 1 bit
                    // broken_link - 1 bit
                    // stuffing    - 4 bits (0111)
                },
                0xB4 => return Err(DecoderError::NotImplemented), // video_session_error_code
                0xB5 => {}, // visual_object_start_code
                0xB6 => { // vop_start_code
                    validate!(dst_frm.is_none() || self.saved.is_none());
                    let ret = supp.pool_u8.get_free();
                    if ret.is_none() {
                        return Err(DecoderError::AllocError);
                    }
                    let mut buf = ret.unwrap();
                    let frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();

                    let pic_info = self.fctx.decode_frame_header(obj_src, &self.seq)?;

                    let ftype = pic_info.pic_type;
                    if ftype == FrameType::Other {
                        return Err(DecoderError::NotImplemented);
                    }
                    let (cur_ts, fwd_ts, bwd_ts) = if ftype != FrameType::Skip || !self.pb_mode {
                            let ref_pts = if self.assign_mode == AssignMode::Yes { pkt.ts.pts } else { None };
                            self.seq.update_ts(&pic_info, ref_pts)
                        } else { (0, 0, 0) };

                    if ftype == FrameType::Skip {
                        let mut saved = None;
                        std::mem::swap(&mut saved, &mut self.saved);
                        let (ftype, sv_ts, buf) = saved.unwrap_or((FrameType::Skip, cur_ts, NABufferType::None));
                        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buf);
                        frm.set_keyframe(ftype == FrameType::I);
                        if self.assign_mode == AssignMode::Yes {
                            frm.set_dts(frm.get_pts());
                            frm.set_pts(Some(sv_ts));
                        } else if frm.get_pts().is_none() {
                            frm.set_pts(Some(sv_ts));
                        }
                        frm.set_id(sv_ts as i64);
                        frm.set_frame_type(ftype);
                        return Ok(frm.into_ref());
                    }

                    match (self.skip_mode, pic_info.pic_type) {
                        (FrameSkipMode::IntraOnly, FrameType::P) |
                        (FrameSkipMode::IntraOnly, FrameType::B) |
                        (FrameSkipMode::KeyframesOnly, FrameType::B) |
                        (_, FrameType::Other) => {
                            let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), NABufferType::None);
                            frm.set_keyframe(false);
                            if self.assign_mode == AssignMode::Yes {
                                frm.set_dts(frm.get_pts());
                                frm.set_pts(Some(cur_ts));
                            } else if frm.get_pts().is_none() {
                                frm.set_pts(Some(cur_ts));
                            }
                            frm.set_id(cur_ts as i64);
                            frm.set_frame_type(FrameType::Skip);
                            return Ok(frm.into_ref());
                        },
                        _ => {},
                    }

                    let (fwd_ref, bwd_ref) = match ftype {
                            FrameType::P => {
                                let prev_frm = self.ipbs.get_lastref();
                                if prev_frm.is_none() {
                                    return Err(DecoderError::MissingReference);
                                }
                                (prev_frm, None)
                            },
                            FrameType::B => {
                                let fref = self.ipbs.get_b_fwdref();
                                let bref = self.ipbs.get_b_bwdref();
                                if fref.is_none() || bref.is_none() {
                                    return Err(DecoderError::MissingReference);
                                }
                                (fref, bref)
                            },
                            _ => (None, None),
                        };
                    self.fctx.set_refs(cur_ts, fwd_ref, fwd_ts, bwd_ref, bwd_ts);
                    let (mb_w, mb_h) = self.fctx.get_dimensions();
                    let mut pstate = PicState::new(cur_ts as u32, mb_w, mb_h);
                    let mut mt_helper = SingleThreadHelper { last_ref_info: &self.last_ref_info };
                    self.fctx.decode_frame(obj_src, frm, &self.seq, &pic_info, &mut pstate, &mut mt_helper)?;

                    if ftype == FrameType::I || ftype == FrameType::P {
                        self.last_ref_info = pstate;
                        self.ipbs.add_frame(buf.clone());
                    }

                    if dst_frm.is_none() {
                        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), NABufferType::Video(buf));
                        frm.set_keyframe(ftype == FrameType::I);
                        if self.assign_mode == AssignMode::Yes {
                            frm.set_dts(frm.get_pts());
                            frm.set_pts(Some(cur_ts));
                        } else if frm.get_pts().is_none() {
                            frm.set_pts(Some(cur_ts));
                        }
                        frm.set_id(cur_ts as i64);
                        frm.set_frame_type(ftype);
                        dst_frm = Some(frm.into_ref());
                    } else {
                        validate!(ftype == FrameType::B);
                        let frm = dst_frm.unwrap();

                        if !self.pb_mode {
                            println!("PB-mode");
                        }
                        self.pb_mode = true;
                        self.saved = Some((frm.get_frame_type(), frm.get_pts().unwrap_or(0), frm.get_buffer()));

                        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), NABufferType::Video(buf));
                        frm.set_keyframe(false);
                        if self.assign_mode == AssignMode::Yes {
                            frm.set_dts(frm.get_pts());
                            frm.set_pts(Some(cur_ts));
                        } else if frm.get_pts().is_none() {
                            frm.set_pts(Some(cur_ts));
                        }
                        frm.set_id(cur_ts as i64);
                        frm.set_frame_type(FrameType::P);
                        dst_frm = Some(frm.into_ref());
                    }
                },
                0xBA..=0xC2 => return Err(DecoderError::NotImplemented), // FBA, mesh, texture data
                0xC3 => {}, // stuffing_start_code
                0xC6..=0xFF => {}, // system start codes
            }
        }
        validate!(dst_frm.is_some());
        Ok(dst_frm.unwrap())
    }
    fn flush(&mut self) {
        self.ipbs.clear();
        self.saved = None;
    }
}

const NO_REORDER_OPTION: &str = "noreorder";
const EDGE_EMU_OPTION: &str = "edge_emu_bug";

const DECODER_OPTIONS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name: FRAME_SKIP_OPTION, description: FRAME_SKIP_OPTION_DESC,
        opt_type: NAOptionDefinitionType::String(Some(&[
                FRAME_SKIP_OPTION_VAL_NONE,
                FRAME_SKIP_OPTION_VAL_KEYFRAME,
                FRAME_SKIP_OPTION_VAL_INTRA
            ])) },
    NAOptionDefinition {
        name: NO_REORDER_OPTION, description: "Do not attempt to reorder frames",
        opt_type: NAOptionDefinitionType::None },
    NAOptionDefinition {
        name: EDGE_EMU_OPTION, description: "Use dimensions-aligned edge emulation (some old files may need it)",
        opt_type: NAOptionDefinitionType::None },
];

impl NAOptionHandler for ASPDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { DECODER_OPTIONS }
    fn set_options(&mut self, options: &[NAOption]) {
        for option in options.iter() {
            for opt_def in DECODER_OPTIONS.iter() {
                if opt_def.check(option).is_ok() {
                    match (option.name, &option.value) {
                        (FRAME_SKIP_OPTION, NAValue::String(ref strval)) => {
                            if let Ok(smode) = FrameSkipMode::from_str(strval) {
                                self.skip_mode = smode;
                            }
                        },
                        (NO_REORDER_OPTION, _) => {
                            self.assign_mode = AssignMode::No;
                        },
                        (EDGE_EMU_OPTION, _) => {
                            self.fctx.set_edge_emu_bug();
                        },
                        _ => {},
                    }
                }
            }
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match name {
            FRAME_SKIP_OPTION => Some(NAValue::String(self.skip_mode.to_string())),
            _ => None,
        }
    }
}


pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(ASPDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::*;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;

    #[test]
    fn test_mpeg4asp_basic() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        mpeg_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/V-codecs/MPEG4/WobblyWindowsIntro.avi
        test_decoding("avi", "mpeg4asp",
                      "assets/MPEG/asp/WobblyWindowsIntro.avi",
                      Some(8), &dmx_reg, &dec_reg, ExpectedTestResult::MD5Frames(vec![
                        [0x01264d54, 0xfb462b03, 0x34dd4dab, 0x8d976230],
                        [0x03be8499, 0x2234824e, 0x4b13d013, 0x635a8ca9],
                        [0x8069c490, 0x23a8d90d, 0xbe40713b, 0xbfd1f2bb],
                        [0xe9c3cdfc, 0x517dc772, 0x3def7eb5, 0x945cc329],
                        [0x6d1c5064, 0x1c8c863e, 0x2ea9bd7c, 0xefbe5791],
                        [0x73587bc1, 0x3cab079e, 0x017decb1, 0x437687de],
                        [0x25450ef1, 0xa5594f36, 0x8df05845, 0x6e1f1fb2],
                        [0x3f1cad0b, 0x16eaf81d, 0x08c43f87, 0x3a6881d2],
                        [0x2d0476a7, 0x291ac9bc, 0xc7147ab8, 0xaf8e3b24]]));
    }
    #[test]
    fn test_mpeg4asp_mov() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        mpeg_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/V-codecs/MPEG4/problem.mov
        test_decoding("mov", "mpeg4asp",
                      "assets/MPEG/asp/problem.mov",
                      Some(256), &dmx_reg, &dec_reg, ExpectedTestResult::MD5Frames(vec![
                        [0xb1ac5e3c, 0xef74b180, 0xcb5ed0dd, 0x60bdaf30],
                        [0x7feb72e4, 0x47837b2b, 0x8f66a93c, 0xe2ae7e17],
                        [0x5c7b532f, 0x263187bb, 0xac174cbf, 0x604ad2ef],
                        [0xd7cc655b, 0x92e4df5a, 0x7001b0ae, 0x6ccb0117]]));
    }
    #[test]
    fn test_mpeg4asp_bframes() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        mpeg_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/V-codecs/DX50-DivX5/test.b-frames.divx5.avi
        test_decoding("avi", "mpeg4asp",
                      "assets/MPEG/asp/test.b-frames.divx5.avi",
                      Some(2), &dmx_reg, &dec_reg, ExpectedTestResult::MD5Frames(vec![
                        [0x2c73036d, 0xa9c3e655, 0x50d82010, 0x51a5c928],
                        [0x2c73036d, 0xa9c3e655, 0x50d82010, 0x51a5c928],
                        [0x2c73036d, 0xa9c3e655, 0x50d82010, 0x51a5c928]]));
    }
    #[test]
    fn test_mpeg4asp_packed_b_frames() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        mpeg_register_all_decoders(&mut dec_reg);

        // https://samples.mplayerhq.hu/V-codecs/MPEG4/vtm_video.avi
        test_decoding("avi", "mpeg4asp",
                      "assets/MPEG/asp/vtm_video.avi",
                      Some(8), &dmx_reg, &dec_reg, ExpectedTestResult::MD5Frames(vec![
                        [0x536b8b3f, 0x6b33df1f, 0x3253605f, 0x92b6c801],
                        [0x9c7635e1, 0xb5ffafb2, 0x0f5e96bb, 0x42baa783],
                        [0x6d941dcc, 0x174efb79, 0xba65a60f, 0x208040ce],
                        [0x048cd555, 0xfac0e718, 0x56fc5efe, 0x0d2ab601],
                        [0xc6bf0541, 0x297a8b0a, 0xc2711e63, 0x8eb34b78],
                        [0xffef9c3d, 0xd5339ac1, 0x95fda2a9, 0x783851a1],
                        [0xab946927, 0xfdd4434a, 0x46e87ebf, 0x7bf70818],
                        [0x51057a2b, 0x97b9c93c, 0x6338f7f3, 0xaf4247fa],
                        [0x4b4421cc, 0x919b086c, 0x33b5b5f3, 0xbe9d5a87]]));
    }
}
