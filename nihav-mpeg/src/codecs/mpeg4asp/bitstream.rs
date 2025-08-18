use nihav_core::frame::FrameType;
use nihav_core::codecs::{DecoderResult, DecoderError};
use nihav_core::io::byteio::*;
use nihav_core::io::bitreader::*;
use nihav_core::io::codebook::*;
use nihav_core::io::intcode::*;
use nihav_codec_support::codecs::{MV, ZERO_MV, ZIGZAG};
use nihav_codec_support::codecs::h263::data::*;

use super::dsp::*;
use super::types::*;

trait ReadMarkerBit {
    fn read_marker(&mut self) -> DecoderResult<()>;
    fn read_long_val(&mut self, first_part_bits: u8, second_part_bits: u8) -> DecoderResult<u32>;
}

impl<'a> ReadMarkerBit for BitReader<'a> {
    fn read_marker(&mut self) -> DecoderResult<()> {
        if self.read(1)? == 1 {
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn read_long_val(&mut self, first_part_bits: u8, second_part_bits: u8) -> DecoderResult<u32> {
        let part1 = self.read(first_part_bits)?;
                    self.read_marker()?;
        let part2 = self.read(second_part_bits)?;
                    self.read_marker()?;
        Ok((part1 << second_part_bits) | part2)
    }
}

pub fn scan_start_codes(src: &[u8]) -> DecoderResult<Vec<usize>> {
    let mut br = MemoryReader::new_read(src);

    let mut offs = Vec::new();
    let mut pfx = br.read_u32be()?;
    validate!((pfx >> 8) == 1);
    offs.push(0);
    while br.left() >= 4 {
        pfx = (pfx << 8) | u32::from(br.read_byte()?);
        if (pfx >> 8) == 1 {
            offs.push(br.tell() as usize - 4);
        }
    }

    offs.push(src.len());
    Ok(offs)
}

pub fn check_xvid_user_data(src: &[u8]) -> bool {
    src.len() > 7 && &src[1..5] == b"XviD"
}

fn read_var_len(br: &mut BitReader) -> DecoderResult<u32> {
    let mut len = 0;
    loop {
        let b                       = br.read(8)?;
        len = (len << 7) | (b & 0x7F);
        if (b & 0x80) == 0 {
            break;
        }
    }
    Ok(len)
}

pub fn parse_es_descriptor(src: &[u8]) -> DecoderResult<(VideoObjectLayer, bool)> {
    let mut br = BitReader::new(src, BitReaderMode::BE);

    // ES_Descriptor
    let tag                         = br.read(8)?;
    validate!(tag == 0x03);
    let len                         = read_var_len(&mut br)?;
    validate!((len as isize * 8) <= br.left());
    let esd_end = br.tell() + (len as usize) * 8;
    let _id                         = br.read(16)?;
    //validate!(id != 0 && id != 0xFFFF);
    let stream_dependence           = br.read_bool()?;
    let url                         = br.read_bool()?;
    let ocr_stream                  = br.read_bool()?;
    let _stream_priority            = br.read(5)?;
    if stream_dependence {
                                    br.skip(16)?; // depends_on_ES_ID
    }
    if url {
        let url_len                 = br.read(8)?;
                                    br.skip(url_len * 8)?;
    }
    if ocr_stream {
                                    br.skip(16)?; // OCR_ES_ID
    }
    // DecoderConfigDescr
    let tag                         = br.read(8)?;
    validate!(tag == 0x04);
    let len                         = read_var_len(&mut br)?;
    let dcd_end = br.tell() + (len as usize) * 8;
    validate!(dcd_end <= esd_end);
    if len < 16 { // no MPEG-4 ASP decoder config data
        return Err(DecoderError::InvalidData);
    }
    let object_type                 = br.read(8)?;
    validate!(object_type == 0x20);
    let stream_type                 = br.read(6)?;
    validate!(stream_type == 4);
    let _up_stream                  = br.read_bool()?;
                                    br.read_marker()?;
                                    br.skip(24)?; // bufferSizeDB
                                    br.skip(32)?; // maxBitrate
                                    br.skip(32)?; // avgBitrate
    // DecoderSpecificInfo
    let tag                         = br.read(8)?;
    validate!(tag == 0x05);
    let len                         = read_var_len(&mut br)?;
    let dsi_end = br.tell() + (len as usize) * 8;
    validate!(dsi_end <= dcd_end);

    let dsi = &src[br.tell() / 8..][..len as usize];
    let offs = scan_start_codes(dsi)?;
    let mut vol = None;
    let mut is_xvid = false;
    for range in offs.windows(2) {
        let obj_src = &dsi[range[0] + 3..range[1]];
        match obj_src[0] {
            0x00..=0x1F => {}, // video_object_start_code
            0x20..=0x3F => { // video_object_layer_start_code
                vol = Some(parse_video_object_layer(&obj_src[1..])?);
            },
            0x30..=0xAF | 0xB7..=0xB9 | 0xC4..=0xC5 => {}, // reserved, system start codes
            0xB0 => { // visual_object_sequence_start_code
                validate!(obj_src.len() >= 2);
                // single byte is profile and level
            },
            0xB1 => {}, // visual_object_sequence_end_code
            0xB2 => { // user_data_start_code
                is_xvid |= check_xvid_user_data(obj_src);
            },
            0xB3 => return Err(DecoderError::InvalidData), // group_of_vop_start_code
            0xB4 => return Err(DecoderError::NotImplemented), // video_session_error_code
            0xB5 => {}, // visual_object_start_code
            0xB6 => return Err(DecoderError::InvalidData), // vop_start_code
            0xBA..=0xC2 => return Err(DecoderError::NotImplemented), // FBA, mesh, texture data
            0xC3 => {}, // stuffing_start_code
            0xC6..=0xFF => {}, // system start codes
        }
    }
    // the rest - who cares?
    validate!(vol.is_some());
    Ok((vol.unwrap(), is_xvid))
}

pub fn parse_video_object_layer(src: &[u8]) -> DecoderResult<VideoObjectLayer> {
    let mut br = BitReader::new(src, BitReaderMode::BE);

    let _random_accessible          = br.read_bool()?;
    let vol_type                    = br.read(8)? as u8;
    /*if vol_type > 1 {
        println!("vol_type {vol_type}");
        return Err(DecoderError::NotImplemented);
    }*/
    let version = if br.read_bool()? {
            let verid               = br.read(4)? as u8;
            let _priority           = br.read(3)? as u8;
            verid
        } else { 1 };

    let ar_info                     = br.read(4)?;
    if ar_info == 0b1111 {
        let _par_width              = br.read(8)?;
        let _par_height             = br.read(8)?;
    }

    let vol_control_parameters      = br.read_bool()?;
    let mut low_delay = false;
    if vol_control_parameters {
        let chroma_format           = br.read(2)?;
        if chroma_format != 1 { println!("chroma!"); return Err(DecoderError::NotImplemented); }
        low_delay                   = br.read_bool()?;
        if br.read_bool()? { // vbv_parameters
            let _bit_rate           = br.read_long_val(15, 15)?;
            let _vbv_buf_size_part1 = br.read(15)?;
                                      br.read_marker()?;
            let _vbv_buf_size_part2 = br.read(3)?;
            let _occupancy          = br.read_long_val(11, 15)?;
        }
    }

    const SHAPES: [Shape; 4] = [ Shape::Rect, Shape::Binary, Shape::BinaryOnly, Shape::Grayscale ];
    let shape                       = SHAPES[br.read(2)? as usize];
    if shape != Shape::Rect { println!("non-rect shape!"); return Err(DecoderError::NotImplemented); }

    if shape == Shape::Grayscale && version != 1 {
        let _shape_extension        = br.read(4)?;
    }
                                    br.read_marker()?;
    let time_res                    = br.read(16)?;
    validate!(time_res != 0);
                                    br.read_marker()?;
    let fixed_rate = if br.read_bool()? {
            let mut res_bits = 0;
            while (1 << res_bits) < (time_res - 1) {
                res_bits += 1;
            }
            Some(br.read(res_bits)?)
        } else { None };

    let resync_marker_disable;
    let mut width = 0;
    let mut height = 0;
    let mut obmc_disable = false;
    let mut sprite_info = SpriteInfo::default();
    let mut quant_info = QuantInfo::default();
    let mut quarter_sample = false;
    let mut newpred_enable = false;
    let mut complexity_estimation = false;
    if shape != Shape::BinaryOnly {
        if shape == Shape::Rect {
                                    br.read_marker()?;
            width                   = br.read(13)? as usize;
                                    br.read_marker()?;
            validate!(width > 0);
            height                  = br.read(13)? as usize;
                                    br.read_marker()?;
            validate!(height > 0);
        }
        let interlaced              = br.read_bool()?;
        if interlaced { println!("interlaced!"); return Err(DecoderError::NotImplemented); }
        obmc_disable                = br.read_bool()?;
        if version == 1 {
            if br.read_bool()? {
                sprite_info.sprite = Sprite::Static;
            }
        } else {
            let code                = br.read(2)?;
            sprite_info.sprite = match code {
                    0 => Sprite::NotUsed,
                    1 => Sprite::Static,
                    2 => Sprite::GMC,
                    _ => { println!("wrong sprite code"); return Err(DecoderError::NotImplemented); },
                };
        }
        if matches!(sprite_info.sprite, Sprite::Static | Sprite::GMC) {
            if sprite_info.sprite != Sprite::GMC {
                let sprite_width    = br.read(13)? as usize;
                                    br.read_marker()?;
                validate!(sprite_width > 0);
                let sprite_height   = br.read(13)? as usize;
                                    br.read_marker()?;
                validate!(sprite_height > 0);
                let _sprite_left    = br.read(13)? as usize;
                                    br.read_marker()?;
                let _sprite_right   = br.read(13)? as usize;
                                    br.read_marker()?;
            }
            sprite_info.num_warp_points     = br.read(6)? as usize;
            sprite_info.warping_accuracy    = br.read(2)? as u8;
            sprite_info.brightness_change   = br.read_bool()?;
            if sprite_info.sprite != Sprite::GMC {
                sprite_info.low_latency     = br.read_bool()?;
            }
        }

        if version != 1 && shape != Shape::Rect {
            let _sadct_disable      = br.read_bool()?;
        }

        let not_8_bit               = br.read_bool()?;
        if not_8_bit {
            //let quant_precision = br.read(4)?;
            //let bits_per_pixel  = br.read(4)? as u8 + 8;
            println!("non-8bit");
            return Err(DecoderError::NotImplemented);
        }

        if shape == Shape::Grayscale {
            let _no_gray_quant_update = br.read_bool()?;
            let _composition_method   = br.read_bool()?;
            let _linear_composition   = br.read_bool()?;
        }

        read_quant_info(&mut br, &mut quant_info, shape)?;

        if version != 1 {
            quarter_sample          = br.read_bool()?;
        }

        complexity_estimation       = !br.read_bool()?;
        if complexity_estimation {
            read_vop_complexity_estimation_header(&mut br)?;
        }

        resync_marker_disable       = br.read_bool()?;
        let data_partitioned        = br.read_bool()?;
        if data_partitioned {
            //let reversible_vlc = br.read_bool()?;
            println!("partitioned");
            return Err(DecoderError::NotImplemented);
        }

        if version != 1 {
            newpred_enable          = br.read_bool()?;
            if newpred_enable {
                let _req_upstream_msg_type = br.read(2)?;
                let _newpred_segment_type  = br.read_bool()?;
            }
            let reduced_res_vop_enable = br.read_bool()?;
            if reduced_res_vop_enable { return Err(DecoderError::NotImplemented); }
        }
        let scalability             = br.read_bool()?;
        if scalability {
            // lots of fields
            println!("scalable");
            return Err(DecoderError::NotImplemented);
        }
    } else {
        if version != 1 {
            let scalability         = br.read_bool()?;
            if scalability {
                //let ref_layer_id  = br.read(4)?;
                //let shape_hor_sampling_factor_m = br.read(5)?;
                //let shape_hor_sampling_factor_n = br.read(5)?;
                //let shape_vert_sampling_factor_m = br.read(5)?;
                //let shape_vert_sampling_factor_n = br.read(5)?;
                return Err(DecoderError::NotImplemented);
            }
        }
        resync_marker_disable       = br.read_bool()?;
    }

    Ok(VideoObjectLayer {
        vol_type, version, low_delay, shape,
        time_res, fixed_rate,
        width, height,
        sprite_info, quant_info, quarter_sample, newpred_enable,
        complexity_estimation,
        obmc_disable, resync_marker_disable,
    })
}

fn read_quant_info(br: &mut BitReader, qinfo: &mut QuantInfo, shape: Shape) -> DecoderResult<()> {
    qinfo.quant_type        = br.read_bool()?;
    if qinfo.quant_type {
        if br.read_bool()? {
            read_quant_mat(br, &mut qinfo.intra_mat, true)?;
        }
        if br.read_bool()? {
            read_quant_mat(br, &mut qinfo.inter_mat, false)?;
        }
        if shape == Shape::Grayscale {
            return Err(DecoderError::NotImplemented);
        }
    }
    Ok(())
}

fn read_quant_mat(br: &mut BitReader, mat: &mut [u8; 64], intra: bool) -> DecoderResult<()> {
    let mut prev_val = 0;
    let mut end = false;
    for &idx in ZIGZAG.iter() {
        let nval = if !end { br.read(8)? as u8 } else { prev_val };
        if intra && idx == 0 {
            validate!(nval == 8);
        }
        if nval == 0 {
            validate!(prev_val != 0);
            mat[idx] = prev_val;
            end = true;
        } else {
            mat[idx] = nval;
            prev_val = nval;
        }
    }
    Ok(())
}

fn read_vop_complexity_estimation_header(br: &mut BitReader) -> DecoderResult<()> {
    let estimation_method           = br.read(2)? as u8;
    if estimation_method == 0b00 || estimation_method == 0b01 {
        let shape_cplx_est          = !br.read_bool()?;
        if shape_cplx_est {
            let _opaque             = br.read_bool()?;
            let _transparent        = br.read_bool()?;
            let _intra_cae          = br.read_bool()?;
            let _inter_cae          = br.read_bool()?;
            let _no_update          = br.read_bool()?;
            let _upsampling         = br.read_bool()?;
        }
        let tex_cplx_est_set_1      = !br.read_bool()?;
        if tex_cplx_est_set_1 {
            let _intra_blocks       = br.read_bool()?;
            let _inter_blocks       = br.read_bool()?;
            let _inter4v_blocks     = br.read_bool()?;
            let _not_coded_blocks   = br.read_bool()?;
        }
                                    br.read_marker()?;
        let tex_cplx_est_set_2      = !br.read_bool()?;
        if tex_cplx_est_set_2 {
            let _dct_coeffs         = br.read_bool()?;
            let _dct_lines          = br.read_bool()?;
            let _vlc_symbols        = br.read_bool()?;
            let _vlc_bits           = br.read_bool()?;
        }
        let mc_cplx_est             = !br.read_bool()?;
        if mc_cplx_est {
            let _apm                = br.read_bool()?;
            let _npm                = br.read_bool()?;
            let _interpolate_mc_q   = br.read_bool()?;
            let _forw_back_mc_q     = br.read_bool()?;
            let _halfpel2           = br.read_bool()?;
            let _halfpel4           = br.read_bool()?;
        }
                                    br.read_marker()?;
        if estimation_method == 0b01 {
            let ver2_cplx_est       = !br.read_bool()?;
            if ver2_cplx_est {
                let _sadct          = br.read_bool()?;
                let _quarterpel     = br.read_bool()?;
            }
        }
    }
    Ok(())
}

struct Tables {
    intra_mcbpc_cb:     Codebook<u8>,
    inter_mcbpc_cb:     Codebook<u8>,
    cbpy_cb:            Codebook<u8>,
    intra_rl_cb:        Codebook<H263RLSym>,
    inter_rl_cb:        Codebook<H263RLSym>,
    mv_cb:              Codebook<u8>,
    luma_dc_len_cb:     Codebook<u8>,
    chroma_dc_len_cb:   Codebook<u8>,
}

fn map_idx(idx: usize) -> u8 { idx as u8 }

impl Tables {
    fn new() -> Self {
        let mut coderead = H263ShortCodeReader::new(H263_INTRA_MCBPC);
        let intra_mcbpc_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
        let mut coderead = H263ShortCodeReader::new(H263_INTER_MCBPC);
        let inter_mcbpc_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
        let mut coderead = H263ShortCodeReader::new(H263_CBPY);
        let cbpy_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
        let mut coderead = H263RLCodeReader::new(INTRA_RL_CODES);
        let intra_rl_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
        let mut coderead = H263RLCodeReader::new(H263_RL_CODES);
        let inter_rl_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
//        let mut coderead = H263RLCodeReader::new(H263_RL_CODES_AIC);
//        let aic_rl_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
        let mut coderead = H263ShortCodeReader::new(H263_MV);
        let mv_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
        let mut coderead = TableCodebookDescReader::new(LUMA_DC_LEN_CODES, LUMA_DC_LEN_BITS, map_idx);
        let luma_dc_len_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();
        let mut coderead = TableCodebookDescReader::new(CHROMA_DC_LEN_CODES, CHROMA_DC_LEN_BITS, map_idx);
        let chroma_dc_len_cb = Codebook::new(&mut coderead, CodebookMode::MSB).unwrap();

        Self {
            intra_mcbpc_cb,
            inter_mcbpc_cb,
            cbpy_cb,
            intra_rl_cb,
            inter_rl_cb,
//            aic_rl_cb,
            mv_cb,
            luma_dc_len_cb, chroma_dc_len_cb,
        }
    }
}

pub struct FrameDecoder {
    short_header:           bool,
    mb_w:                   usize,
    mb_h:                   usize,
    mb_addr:                usize,
    slice_start:            usize,
    bitpos:                 usize,
    pic_type:               FrameType,
    mb_num_bits:            u8,
    quant:                  u8,
    intra_dc_vlc_thr:       u8,
    fcode_forward:          u8,
    fcode_backward:         u8,
    shape:                  Shape,
    sprite_info:            SpriteInfo,
    cbs:                    Tables,
}

#[allow(clippy::needless_late_init)]
impl FrameDecoder {
    pub fn new() -> Self {
        Self {
            short_header:           true,
            mb_w:                   0,
            mb_h:                   0,
            mb_addr:                0,
            slice_start:            0,
            bitpos:                 0,
            pic_type:               FrameType::Other,
            mb_num_bits:            0,
            quant:                  0,
            intra_dc_vlc_thr:       0,
            fcode_forward:          0,
            fcode_backward:         0,
            shape:                  Shape::Rect,
            sprite_info:            SpriteInfo::default(),
            cbs:                    Tables::new(),
        }
    }
    pub fn init(&mut self, width: usize, height: usize) {
        if (width + 15) / 16 == self.mb_w && (height + 15) / 16 == self.mb_h { return; }
        self.mb_w = (width  + 15) / 16;
        self.mb_h = (height + 15) / 16;
        self.bitpos = 0;
    }
    pub fn get_dimensions(&self) -> (usize, usize) { (self.mb_w, self.mb_h) }
    //pub fn is_short_header(&self) -> bool { self.short_header }
    pub fn parse_vop_header(&mut self, src: &[u8], vol: &VideoObjectLayer) -> DecoderResult<PicInfo> {
        let mut br = BitReader::new(src, BitReaderMode::BE);
        br.skip(8)?;
        const PIC_TYPES: [FrameType; 4] = [FrameType::I, FrameType::P, FrameType::B, FrameType::Other];
        let pic_type                = PIC_TYPES[br.read(2)? as usize];
        let modulo_time_base        = br.read_code(UintCodeType::UnaryOnes)?;
                                    br.read_marker()?;
        let mut res_bits = 0;
        while (1 << res_bits) < (vol.time_res - 1) {
            res_bits += 1;
        }
        let vop_time_increment      = br.read(res_bits)?;
                                    br.read_marker()?;
        if !br.read_bool()? {
            return Ok(PicInfo { pic_type: FrameType::Skip, vop_time_increment, modulo_time_base, fcode_f: 1, fcode_b: 1, rounding: false })
        }

        if vol.newpred_enable {
            //let vop_id = br.read(ilog2(vop_time_increment).min(15))?;
            //let vop_id_for_pred = br.read_bool()?;
            //if vop_id_for_pred {
            //    let vop_id_for_prediction = br.read(ilog2(vop_time_increment).min(15))?
            //    br.read_marker()?;
            //}
            return Err(DecoderError::NotImplemented);
        }

        let vop_rounding_type;
        if vol.shape != Shape::BinaryOnly && (pic_type == FrameType::P || (pic_type == FrameType::Other && vol.sprite_info.sprite == Sprite::GMC)) {
            vop_rounding_type       = br.read_bool()?;
        } else {
            vop_rounding_type = false;
        }
        /*if vol.reduced_res && vol.shape == Shape::Rect && (pic_type == FrameType::P || pic_type == FrameType::I) {
            let _vop_reduced_res    = br.read_bool()?;
        }*/
        if vol.shape != Shape::Rect {
            //if vol.sprite_info.sprite == Sprite::Static && pic_type == FrameType::I {
            //    read sprite w, h, x, y
            //}
            //read various fields
            return Err(DecoderError::NotImplemented);
        }

        if vol.shape != Shape::BinaryOnly {
            if vol.complexity_estimation {
//read bytes depending on frame size and flags in cplx_est header set
unimplemented!();
            }

            const DC_VLC_THRESHOLDS: [u8; 8] = [64, 13, 15, 17, 19, 21, 23, 0];
            self.intra_dc_vlc_thr    = DC_VLC_THRESHOLDS[br.read(3)? as usize];
            /*if vol.interlaced {
                let _top_field_first = br.read_bool()?;
                let _alt_vert_scan   = br.read_bool()?;
            }*/
        }

        if matches!(vol.sprite_info.sprite, Sprite::Static | Sprite::GMC) && pic_type == FrameType::Other {
            if vol.sprite_info.num_warp_points > 0 {
                for _ in 0..vol.sprite_info.num_warp_points {
                    let _du = warping_mv_code(&mut br)?;
                    let _dv = warping_mv_code(&mut br)?;
                }
            }
            if vol.sprite_info.brightness_change {
                let len             = br.read_code(UintCodeType::LimitedOnes(4))? as u8;
                let _change         = br.read(len + 5)?;
                // 0    ->   -16..-1 | 1..16
                // 10   ->  -48..-17 | 17..48
                // 110  -> -112..-49 | 49..112
                // 1110 -> 113..624
                // 1111 -> 625..1648
            }
            if vol.sprite_info.sprite == Sprite::Static {
                // sprite piece transmission mode
                return Err(DecoderError::NotImplemented);
            }
        }

        if vol.shape != Shape::BinaryOnly {
            self.quant              = br.read(5)? as u8; // or quant_precision for high-bitdepth
            validate!(self.quant != 0);
            if vol.shape == Shape::Grayscale {
                // alpha quants
                return Err(DecoderError::NotImplemented);
            }
            if pic_type != FrameType::I {
                self.fcode_forward  = br.read(3)? as u8;
                validate!(self.fcode_forward != 0);
            }
            if pic_type == FrameType::B {
                self.fcode_backward = br.read(3)? as u8;
                validate!(self.fcode_backward != 0);
            }
            /* if !vol.scalability */ {
                if vol.shape != Shape::Rect && pic_type != FrameType::I {
                    let _vop_shape_coding_type = br.read_bool()?;
                }
            } /* else {
                if enhancement_type { various forward/backward shape parameters }
                let ref_select_code = br.read(2)?;
            } */
        } else {
            return Err(DecoderError::NotImplemented);
        }

        self.bitpos = br.tell();
        self.short_header = false;
        self.pic_type = pic_type;
        self.mb_w = (vol.width  + 15) / 16;
        self.mb_h = (vol.height + 15) / 16;
        let num_mbs = self.mb_w * self.mb_h;
        self.mb_num_bits = 1;
        while (1 << self.mb_num_bits) < num_mbs {
            self.mb_num_bits += 1;
        }

        self.shape = vol.shape;
        self.sprite_info = vol.sprite_info;

        Ok(PicInfo { pic_type, vop_time_increment, modulo_time_base, fcode_f: self.fcode_forward, fcode_b: self.fcode_backward, rounding: vop_rounding_type })
    }
    pub fn parse_vop_short_header(&mut self, src: &[u8]) -> DecoderResult<PicInfo> {
        let mut br = BitReader::new(src, BitReaderMode::BE);
        validate!(br.read(22)? == 0x20);
        let _temp_ref               = br.read(8)?;
                                    br.read_marker()?;
        validate!(br.read(1)? == 0);
        let _split_screen           = br.read_bool()?;
        let _document_camera        = br.read_bool()?;
        let _full_pic_freeze_release = br.read_bool()?;
        let source_format           = br.read(3)?;
        let (width, height) = match source_format {
                1 => ( 128,   96),
                2 => ( 176,  144),
                3 => ( 352,  288),
                4 => ( 704,  576),
                5 => (1408, 1152),
                _ => return Err(DecoderError::NotImplemented),
            };
        let pic_type                = if br.read_bool()? { FrameType::P } else { FrameType::I };
        validate!(br.read(4)? == 0);
        let vop_quant               = br.read(5)? as u8;
        validate!(vop_quant != 0);
        validate!(br.read(1)? == 0);
        while br.read_bool()? { // PEI
            br.skip(8)?;
        }

        self.bitpos = br.tell();
        self.short_header = true;
        self.pic_type = pic_type;
        self.quant = vop_quant;
        self.mb_w = (width  + 15) / 16;
        self.mb_h = (height + 15) / 16;
        self.shape = Shape::Rect;
        self.sprite_info = SpriteInfo::default();
        self.fcode_forward = 1;
        self.fcode_backward = 1;
        self.intra_dc_vlc_thr = 64;
        Ok(PicInfo{ pic_type, vop_time_increment: 1, modulo_time_base: 0, fcode_f: 1, fcode_b: 1, rounding: false })
    }

    pub fn start_decoding(&mut self) {
        self.mb_addr = 0;
        self.slice_start = 0;
    }

    #[allow(clippy::collapsible_if)]
    #[allow(clippy::collapsible_else_if)]
    pub fn decode_mb(&mut self, src: &[u8], mb: &mut Macroblock, colo_not_coded: bool, mb_x: usize, pred: &mut PredState, qinfo: &QuantInfo) -> DecoderResult<()> {
        let mut br = BitReader::new(src, BitReaderMode::BE);
        br.skip(self.bitpos as u32)?;

        if !self.short_header && br.left() > 17 {
            let mut abits = br.peek(24);
            let cur_tail = 8 - (br.tell() & 7);

            let resync_marker = ((1 << cur_tail) - 1) >> 1;
            let possible_stuffing = abits >> (24 - cur_tail);
            if possible_stuffing == resync_marker {
                abits = (abits << cur_tail) & 0xFFFFFF;
            } else {
                abits = !0;
            }
            if (abits >> 8) == 0x0000 {
                br.skip(cur_tail as u32)?;
                while !br.read_bool()? {}
                // video packet header
                if self.shape != Shape::Rect { return Err(DecoderError::NotImplemented); }
                let mb_no = br.read(self.mb_num_bits)?;
                validate!(mb_no as usize == self.mb_addr);
                if self.shape != Shape::Binary {
                    let qscale = br.read(5)? as u8;
                    validate!(qscale != 0);
                    self.quant = qscale;
                }
                if self.shape == Shape::Rect && br.read_bool()? {
                    // extended header
                    unimplemented!();
                }
                self.slice_start = self.mb_addr;
                pred.reset();
            }
        }

        if self.shape != Shape::Rect { return Err(DecoderError::NotImplemented); }
        let mut dquant = 0;
        if self.pic_type != FrameType::B {
            /* ignore binary shape coding */
            /* ignore non-rectangular shape with sprite stuff */
            let cbpc;
            let has_dquant;
            {
                let (mcbpc_cb, stuffing) = if self.pic_type == FrameType::I {
                        (&self.cbs.intra_mcbpc_cb, 8)
                    } else {
                        (&self.cbs.inter_mcbpc_cb, 20)
                    };
                let mut mcbpc = stuffing;
                while mcbpc == stuffing {
                    if self.pic_type != FrameType::I /* and not piece static sprite */ {
                        if br.read_bool()? { // not_coded
                            mb.mb_type = MBType::Skip;
                            mb.cbp = 0;
                            mb.quant = self.quant;
                            mb.mvs[0] = ZERO_MV;
                            self.bitpos = br.tell();
                            self.mb_addr += 1;
                            return Ok(());
                        }
                    }
                    mcbpc = br.read_cb(mcbpc_cb)?;
                }
                cbpc = mcbpc & 3;
                if self.pic_type == FrameType::I {
                    has_dquant = (mcbpc & 4) != 0;
                    mb.mb_type = MBType::Intra;
                    mb.mvs[0] = ZERO_MV;
                } else {
                    has_dquant = (mcbpc & 8) != 0;
                    mb.mb_type = match (mcbpc & !8) >> 2 {
                            1 => MBType::Intra,
                            4 => MBType::Inter4MV,
                            _ => MBType::Inter,
                        };
                }
            }
            /* ignore sprite mcsel */
            if !self.short_header && mb.mb_type == MBType::Intra {
                mb.ac_pred = br.read_bool()?;
            }
            let mut cbpy = br.read_cb(&self.cbs.cbpy_cb)?;
            if mb.mb_type != MBType::Intra {
                cbpy ^= 0xF;
            }
            mb.cbp = (cbpy << 2) | cbpc;
            if has_dquant {
                const DQUANTS: [i8; 4] = [-1, -2, 1, 2];
                dquant = DQUANTS[br.read(2)? as usize];
            }
            /* interlaced information skipped */
            /* scalability and sprite checks skipped */
            if mb.mb_type == MBType::Inter {
                mb.mvs[0] = read_mv(&mut br, &self.cbs.mv_cb, self.fcode_forward)?;
            }
            if mb.mb_type == MBType::Inter4MV {
                for mv in mb.mvs.iter_mut() {
                    *mv = read_mv(&mut br, &self.cbs.mv_cb, self.fcode_forward)?;
                }
            }
        } else {
            if !colo_not_coded /* or some sprite and scalability checks */ {
                let modb = br.read_code(UintCodeType::LimitedZeroes(2))?; //none, mb_type, cbpb+mb_type
                if modb != 0 {
                    let mbtype = br.read_code(UintCodeType::UnaryZeroes)?;
                    validate!(mbtype < 4);
                    const B_MB_TYPES: [MBType; 4] = [MBType::Direct, MBType::Interpolate, MBType::Backward, MBType::Forward];
                    mb.mb_type = B_MB_TYPES[mbtype as usize];
                    mb.cbp = if modb == 2 { br.read(6)? as u8 } else { 0 };
                    /* scalability checks ignored */
                    if mb.mb_type != MBType::Direct && mb.cbp != 0 {
                        const DBQUANTS: [i8; 3] = [0, -2, 2];
                        dquant = DBQUANTS[br.read_code(UintCodeType::Unary012)? as usize];
                    }
                    /* interlaced information ignored */
                    match mb.mb_type {
                        MBType::Direct => {
                            mb.mvs[0] = read_mv(&mut br, &self.cbs.mv_cb, 0)?;
                        },
                        MBType::Interpolate => {
                            mb.mvs[0] = read_mv(&mut br, &self.cbs.mv_cb, self.fcode_forward)?;
                            mb.mvs[1] = read_mv(&mut br, &self.cbs.mv_cb, self.fcode_backward)?;
                        },
                        MBType::Backward => {
                            mb.mvs[0] = read_mv(&mut br, &self.cbs.mv_cb, self.fcode_backward)?;
                        },
                        MBType::Forward => {
                            mb.mvs[0] = read_mv(&mut br, &self.cbs.mv_cb, self.fcode_forward)?;
                        },
                        _ => unreachable!(),
                    }
                } else {
                    mb.mb_type = MBType::Direct;
                    mb.cbp = 0;
                    mb.mvs = [ZERO_MV; 4];
                }
            } else {
                mb.mb_type = MBType::Forward;
                mb.cbp = 0;
                mb.mvs = [ZERO_MV; 4];
            }
        }
        /* ignore alpha part for gray shape */

        let quant = self.quant as i8 + dquant;
        validate!((0..64).contains(&quant));
        let quant = quant as u8;
        mb.quant = quant;

        let mut cbp = mb.cbp;
        if cbp != 0 || mb.mb_type == MBType::Intra {
            mb.coeffs = [[0; 64]; 6];
        }
        if mb.mb_type == MBType::Intra {
            let switch_cbs = quant >= self.intra_dc_vlc_thr;

            pred.set_quant(mb_x, mb.quant);

            let dc_scale = super::dsp::get_dc_scales(mb.quant);

            for (i, block) in mb.coeffs.iter_mut().enumerate() {
                let dc_len_cb = if i < 4 { &self.cbs.luma_dc_len_cb } else { &self.cbs.chroma_dc_len_cb };
                let start = if self.short_header || !switch_cbs {
                        block[0] = decode_intra_dc(&mut br, dc_len_cb, self.short_header)?;
                        1
                    } else {
                        0
                    };
                let pred_dir;
                if !self.short_header {
                    let cur_scale = dc_scale[if i >= 4 { 1 } else { 0 }];
                    let (dc, dir) = pred.predict_dc(mb_x, block[0], cur_scale, i);
                    block[0] = dc;
                    pred_dir = dir;
                } else {
                    block[0] *= 8;
                    pred_dir = false;
                }
                if (cbp & 0x20) != 0 {
                    let scan = if !mb.ac_pred {
                            &ZIGZAG
                        } else if pred_dir {
                            &ALT_HOR_SCAN
                        } else {
                            &ALT_VERT_SCAN
                        };
                    decode_block_intra(&mut br, &self.cbs.intra_rl_cb, block, start, self.short_header, scan)?;
                    dequant_intra_block(block, mb.quant, qinfo);
                }
                if mb.ac_pred {
                    pred.predict_ac(block, mb_x, i, pred_dir);
                } else {
                    pred.save_ac(block, mb_x, i);
                }
                cbp <<= 1;
            }
        } else {
            for block in mb.coeffs.iter_mut() {
                if (cbp & 0x20) != 0 {
                    decode_block_inter(&mut br, &self.cbs.inter_rl_cb, block, self.short_header)?;
                }
                cbp <<= 1;
            }
            dequant_inter(mb, qinfo);
        }
        self.bitpos = br.tell();

        let nstate = if matches!(mb.mb_type, MBType::Inter | MBType::Inter4MV) {
                NState {
                    has_l:  mb_x > 0 && self.mb_addr > self.slice_start,
                    has_t:  self.mb_addr >= self.slice_start + self.mb_w,
                    has_tr: self.mb_addr + 1 >= self.slice_start + self.mb_w && (mb_x + 1) < self.mb_w,
                }
            } else { NState::default() };
        match mb.mb_type {
            MBType::Intra => {
                mb.mvs = [ZERO_MV; 4];
            },
            MBType::Inter => {
                mb.mvs[0] = pred.pred_mv(mb_x, &nstate, mb.mvs[0]);
                mb.mvs = [mb.mvs[0]; 4];
            },
            MBType::Inter4MV => {
                pred.pred_4mv(mb_x, &nstate, &mut mb.mvs);
            },
            MBType::Interpolate => {
                mb.mvs[0] = pred.pred_mv_f(mb.mvs[0]);
                mb.mvs[1] = pred.pred_mv_b(mb.mvs[1]);
            },
            MBType::Backward => {
                mb.mvs[0] = pred.pred_mv_b(mb.mvs[0]);
            },
            MBType::Forward => {
                mb.mvs[0] = pred.pred_mv_f(mb.mvs[0]);
            },
            _ => {},
        }

        self.mb_addr += 1;

        Ok(())
    }
}

fn read_mv(br: &mut BitReader, mv_cb: &Codebook<u8>, fcode: u8) -> DecoderResult<MV> {
    let mut x = i16::from(br.read_cb(mv_cb)?);
    let xsign = x != 0 && br.read_bool()?;
    if x != 0 && fcode > 1 {
        x = ((x - 1) << (fcode - 1)) + (br.read(fcode - 1)? as i16) + 1;
    }
    if xsign {
        x = -x;
    }
    let mut y = i16::from(br.read_cb(mv_cb)?);
    let ysign = y != 0 && br.read_bool()?;
    if y != 0 && fcode > 1 {
        y = ((y - 1) << (fcode - 1)) + (br.read(fcode - 1)? as i16) + 1;
    }
    if ysign {
        y = -y;
    }
    Ok(MV{ x, y })
}

fn warping_mv_code(br: &mut BitReader) -> DecoderResult<i32> {
    let mut val = 0;
    let dmv_length = br.read(2)?;
    if dmv_length > 0 {
        let nbits = match dmv_length {
                0b00 => unreachable!(),
                0b01 => br.read(1)? as u8 + 1,
                0b10 => br.read(1)? as u8 + 3,
                _ => {
                    let len = br.read_code(UintCodeType::UnaryOnes)?;
                    validate!(len <= 9);
                    len as u8 + 5
                },
            };
        val = br.read(nbits)? as i32;
        let hrange = 1 << (nbits - 1);
        if val < hrange {
            val -= (1 << nbits) - 1;
        }
    }
    br.read_marker()?;
    Ok(val)
}

fn decode_escape(br: &mut BitReader, cb: &Codebook<H263RLSym>, short_header: bool, intra: bool) -> DecoderResult<(u8, i16, bool)> {
    if !short_header {
        if !br.read_bool()? { // 0 - type 1
            let code = br.read_cb(cb)?;
            validate!(!code.is_escape());
            let run = code.get_run();
            let mut level = code.get_level();
            let last = code.is_last();
            let lmax = if intra {
                    match (last, run) { // Table B-19
                        (false, 0) => 27,
                        (false, 1) => 10,
                        (false, 2) => 5,
                        (false, 3) => 4,
                        (false, 4..=7) => 3,
                        (false, 8..=9) => 2,
                        (true, 0) => 8,
                        (true, 1) => 3,
                        (true, 2..=6) => 2,
                        _ => 1,
                    }
                } else {
                    match (last, run) { // Table B-20
                        (false, 0) => 12,
                        (false, 1) => 6,
                        (false, 2) => 4,
                        (false, 3..=6) => 3,
                        (false, 7..=10) => 2,
                        (true, 0) => 3,
                        (true, 1) => 2,
                        _ => 1,
                    }
                };
            level += lmax;
            if br.read_bool()? {
                level = -level;
            }
            Ok((run, level, last))
        } else if !br.read_bool()? { // 10 - type 2
            let code = br.read_cb(cb)?;
            validate!(!code.is_escape());
            let run = code.get_run();
            let mut level = code.get_level();
            let last = code.is_last();
            let rmax = if intra {
                    match (last, level) { // Table B-21
                        (false, 1) => 14,
                        (false, 2) => 9,
                        (false, 3) => 7,
                        (false, 4) => 3,
                        (false, 5) => 2,
                        (false, 6..=10) => 1,
                        (true, 1) => 20,
                        (true, 2) => 6,
                        (true, 3) => 1,
                        _ => 0,
                    }
                } else {
                    match (last, level) { // Table B-22
                        (false, 1) => 26,
                        (false, 2) => 10,
                        (false, 3) => 6,
                        (false, 4) => 2,
                        (false, 5..=6) => 1,
                        (true, 1) => 40,
                        (true, 2) => 1,
                        _ => 0,
                    }
                };
            if br.read_bool()? {
                level = -level;
            }
            Ok((run + rmax + 1, level, last))
        } else { // 11 - type 3
            let last  = br.read_bool()?;
            let run   = br.read(6)? as u8;
                        br.read_marker()?;
            let level = br.read_s(12)? as i16;
            validate!(level != -2048 && level != 0);
                        br.read_marker()?;
            Ok((run, level, last))
        }
    } else { // type 4
        let last  = br.read_bool()?;
        let run   = br.read(6)? as u8;
        let level = br.read_s(8)? as i16;
        validate!(level != 0 && level != -128);
        Ok((run, level, last))
    }
}

fn decode_intra_dc(br: &mut BitReader, dc_len_cb: &Codebook<u8>, short_header: bool) -> DecoderResult<i16> {
    if short_header {
        let mut val = br.read(8)? as i16;
        validate!(val != 0 && val != 128);
        if val == 0xFF {
            val = 0x80;
        }
        Ok(val)
    } else {
        let dc_len = br.read_cb(dc_len_cb)?;
        validate!(dc_len < 10); // higher lengths are for higher bitdepths
        if dc_len > 0 {
            let val = br.read(dc_len)? as i16;
            let hsize = 1 << (dc_len - 1);
            if val >= hsize {
                Ok(val)
            } else {
                Ok(val - (1 << dc_len) + 1)
            }
        } else {
            Ok(0)
        }
    }
}

fn decode_block_intra(br: &mut BitReader, cb: &Codebook<H263RLSym>, block: &mut [i16; 64], mut idx: usize, short_header: bool, scan: &[usize; 64]) -> DecoderResult<()> {
    loop {
        let code = br.read_cb(cb)?;
        let (run, level, last) = if !code.is_escape() {
                let mut level = code.get_level();
                if br.read_bool()? {
                    level = -level;
                }
                (code.get_run(), level, code.is_last())
            } else {
                decode_escape(br, cb, short_header, true)?
            };
        idx += usize::from(run);
        validate!(idx < 64);
        block[scan[idx]] = level;
        idx += 1;
        if last { break; }
    }

    Ok(())
}

fn decode_block_inter(br: &mut BitReader, cb: &Codebook<H263RLSym>, block: &mut [i16; 64], short_header: bool) -> DecoderResult<()> {
    let mut idx = 0;
    loop {
        let code = br.read_cb(cb)?;
        let (run, level, last) = if !code.is_escape() {
                let mut level = code.get_level();
                if br.read_bool()? {
                    level = -level;
                }
                (code.get_run(), level, code.is_last())
            } else {
                decode_escape(br, cb, short_header, false)?
            };
        idx += usize::from(run);
        validate!(idx < 64);
        block[ZIGZAG[idx]] = level;
        if last { break; }
        idx += 1;
    }
    Ok(())
}

macro_rules! rlcodes{
    ($(($c:expr, $b:expr, $r:expr, $l:expr)),*) => {
        &[$(H263RLCodeDesc{ code: $c, bits: $b, sym: H263RLSym{ run: $r, level: $l}}),*]
    }
}

const LUMA_DC_LEN_CODES: &[u8] = &[ 3, 3, 2, 2, 1, 1, 1, 1, 1, 1, 1,  1,  1 ];
const LUMA_DC_LEN_BITS:  &[u8] = &[ 3, 2, 2, 3, 3, 4, 5, 6, 7, 8, 9, 10, 11 ];

const CHROMA_DC_LEN_CODES: &[u8] = &[ 3, 2, 1, 1, 1, 1, 1, 1, 1, 1,  1,  1,  1 ];
const CHROMA_DC_LEN_BITS:  &[u8] = &[ 2, 2, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12 ];

const INTRA_RL_CODES: &[H263RLCodeDesc] = rlcodes!(
    (0x02,  2,  0,  1), (0x0F,  4,  0,  3), (0x15,  6,  0,  6), (0x17,  7,  0,  9),
    (0x1F,  8,  0, 10), (0x25,  9,  0, 13), (0x24,  9,  0, 14), (0x21, 10,  0, 17),
    (0x20, 10,  0, 18), (0x07, 11,  0, 21), (0x06, 11,  0, 22), (0x20, 11,  0, 23),
    (0x06,  3,  0,  2), (0x14,  6,  1,  2), (0x1E,  8,  0, 11), (0x0F, 10,  0, 19),
    (0x21, 11,  0, 24), (0x50, 12,  0, 25), (0x0E,  4,  1,  1), (0x1D,  8,  0, 12),
    (0x0E, 10,  0, 20), (0x51, 12,  0, 26), (0x0D,  5,  0,  4), (0x23,  9,  0, 15),
    (0x0D, 10,  1,  7), (0x0C,  5,  0,  5), (0x22,  9,  4,  2), (0x52, 12,  0, 27),
    (0x0B,  5,  2,  1), (0x0C, 10,  2,  4), (0x53, 12,  1,  9), (0x13,  6,  0,  7),
    (0x0B, 10,  3,  4), (0x54, 12,  6,  3), (0x12,  6,  0,  8), (0x0A, 10,  4,  3),
    (0x11,  6,  3,  1), (0x09, 10,  8,  2), (0x10,  6,  4,  1), (0x08, 10,  5,  3),
    (0x16,  7,  1,  3), (0x55, 12,  1, 10), (0x15,  7,  2,  2), (0x14,  7,  7,  1),
    (0x1C,  8,  1,  4), (0x1B,  8,  3,  2), (0x21,  9,  0, 16), (0x20,  9,  1,  5),
    (0x1F,  9,  1,  6), (0x1E,  9,  2,  3), (0x1D,  9,  3,  3), (0x1C,  9,  5,  2),
    (0x1B,  9,  6,  2), (0x1A,  9,  7,  2), (0x22, 11,  1,  8), (0x23, 11,  9,  2),
    (0x56, 12,  2,  5), (0x57, 12,  7,  3), (0x07,  4,  0, -1), (0x19,  9, 11,  1),
    (0x05, 11,  0, -6), (0x0F,  6,  1, -1), (0x04, 11,  0, -7), (0x0E,  6,  2, -1),
    (0x0D,  6,  5,  1), (0x0C,  6,  0, -2), (0x13,  7,  5, -1), (0x12,  7,  6,  1),
    (0x11,  7,  3, -1), (0x10,  7,  4, -1), (0x1A,  8,  9, -1), (0x19,  8,  8,  1),
    (0x18,  8,  9,  1), (0x17,  8, 10,  1), (0x16,  8,  0, -3), (0x15,  8,  6, -1),
    (0x14,  8,  7, -1), (0x13,  8,  8, -1), (0x18,  9, 12,  1), (0x17,  9,  0, -4),
    (0x16,  9,  1, -2), (0x15,  9, 10, -1), (0x14,  9, 11, -1), (0x13,  9, 12, -1),
    (0x12,  9, 13, -1), (0x11,  9, 14, -1), (0x07, 10, 13,  1), (0x06, 10,  0, -5),
    (0x05, 10,  1, -3), (0x04, 10,  2, -2), (0x24, 11,  3, -2), (0x25, 11,  4, -2),
    (0x26, 11, 15, -1), (0x27, 11, 16, -1), (0x58, 12, 14,  1), (0x59, 12,  0, -8),
    (0x5A, 12,  5, -2), (0x5B, 12,  6, -2), (0x5C, 12, 17, -1), (0x5D, 12, 18, -1),
    (0x5E, 12, 19, -1), (0x5F, 12, 20, -1), (0x03,  7,  0,  0)
);

const ALT_HOR_SCAN: [usize; 64] = [
     0,  1,  2,  3,  8,  9, 16, 17,
    10, 11,  4,  5,  6,  7, 15, 14,
    13, 12, 19, 18, 24, 25, 32, 33,
    26, 27, 20, 21, 22, 23, 28, 29,
    30, 31, 34, 35, 40, 41, 48, 49,
    42, 43, 36, 37, 38, 39, 44, 45,
    46, 47, 50, 51, 56, 57, 58, 59,
    52, 53, 54, 55, 60, 61, 62, 63
];

const ALT_VERT_SCAN: [usize; 64] = [
     0,  8, 16, 24,  1,  9,  2, 10,
    17, 25, 32, 40, 48, 56, 57, 49,
    41, 33, 26, 18,  3, 11,  4, 12,
    19, 27, 34, 42, 50, 58, 35, 43,
    51, 59, 20, 28,  5, 13,  6, 14,
    21, 29, 36, 44, 52, 60, 37, 45,
    53, 61, 22, 30,  7, 15, 23, 31,
    38, 46, 54, 62, 39, 47, 55, 63
];
