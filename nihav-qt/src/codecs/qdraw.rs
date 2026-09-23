use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_core::scale::*;
use nihav_codec_support::codecs::HAMShuffler;

const ARGB_FORMAT: NAPixelFormaton = NAPixelFormaton {
        model: ColourModel::RGB(RGBSubmodel::RGB), components: 4,
        comp_info: [
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 1, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 2, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 3, next_elem: 4 }),
            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 8, shift: 0, comp_offs: 0, next_elem: 4 }),
            None ],
        elem_size: 4, be: false, alpha: true, palette: false };

trait ReadQDTypes {
    fn read_colour(&mut self) -> DecoderResult<[u8; 3]>;
    fn read_pattern(&mut self) -> DecoderResult<[u8; 8]>;
}

impl<T: ?Sized + ByteIO> ReadQDTypes for T {
    fn read_colour(&mut self) -> DecoderResult<[u8; 3]> {
        let r       = self.read_byte()?;
                      self.read_byte()?;
        let g       = self.read_byte()?;
                      self.read_byte()?;
        let b       = self.read_byte()?;
                      self.read_byte()?;
        Ok([r, g, b])
    }
    fn read_pattern(&mut self) -> DecoderResult<[u8; 8]> {
        let mut pat = [0; 8];
                      self.read_buf(&mut pat)?;
        Ok(pat)
    }
}

#[derive(Default)]
struct QDrawDecoder {
    info:       NACodecInfoRef,
    hams:       HAMShuffler<u8>,
    width:      usize,
    height:     usize,
    decs:       RegisteredDecoders,
}

impl QDrawDecoder {
    fn new() -> Self {
        let mut decs = RegisteredDecoders::new();
        crate::qt_register_all_decoders(&mut decs);
        Self{ decs, ..Default::default() }
    }
}

fn decode_bits(br: &mut dyn ByteIO, frm: &mut NASimpleVideoFrame<u8>, width: usize, height: usize, region: bool, direct: bool) -> DecoderResult<()> {
    if direct {
        let handle                      = br.read_u32be()?;
        validate!(handle == 0xFF);
    }
    let row_bytes                       = br.read_u16be()?;
    validate!(row_bytes & 0x8000 != 0);
    let cur_y                           = usize::from(br.read_u16be()?);
    let cur_x                           = usize::from(br.read_u16be()?);
    let cur_h                           = usize::from(br.read_u16be()?);
    let cur_w                           = usize::from(br.read_u16be()?);
    validate!(cur_x < cur_w && cur_w > 0 && cur_w <= width);
    validate!(cur_y < cur_h && cur_h > 0 && cur_h <= height);
    let version                         = br.read_u16be()?;
    validate!(version == 0);
    let mut pack_type                   = br.read_u16be()?;
    if !direct {
        if pack_type != 0 {
            return Err(DecoderError::NotImplemented);
        }
        if (row_bytes & 0x7FFF) < 8 {
            pack_type = 1;
        }
    } else {
        validate!((0..=4).contains(&pack_type));
    }
    let _pack_size                      = br.read_u32be()?;
    let _h_res                          = br.read_u32be()?;
    let _v_res                          = br.read_u32be()?;
    let pixel_type                      = br.read_u16be()?;
    let pixel_size                      = br.read_u16be()?;
    let cmp_count                       = usize::from(br.read_u16be()?);
    let cmp_size                        = br.read_u16be()?;
    if !direct && (pixel_type != 0 || pixel_size != 8 || cmp_count != 1 || cmp_size != 8) {
        return Err(DecoderError::NotImplemented);
    }
    let _plane_bytes                    = br.read_u32be()?;
    let _pm_table                       = br.read_u32be()?;
    let _pm_reserved                    = br.read_u32be()?;

    let mut pal = [0; 768];
    if !direct {
        let _ctseed                     = br.read_u32be()?;
        let _trans_index                = br.read_u16be()?;
        let ct_size                     = usize::from(br.read_u16be()?);
        for _ in 0..=ct_size {
            let idx                     = usize::from(br.read_u16be()?);
            validate!(idx < 256);
            let clr                     = br.read_colour()?;
            pal[idx * 3..][..3].copy_from_slice(&clr);
        }
    }

                                          br.read_skip(8)?; // src rect
                                          br.read_skip(8)?; // dst rect
    let _mode                           = br.read_u16be()?;
    if region {
        let mask_rgn_size               = usize::from(br.read_u16be()?);
        validate!(mask_rgn_size > 2);
                                          br.read_skip(mask_rgn_size - 2)?;
    }

    match pack_type {
        0 => {
            let line_end = cur_x + cur_w;
            for dline in frm.data[frm.offset[0]..].chunks_exact_mut(frm.stride[0])
                    .skip(cur_y).take(cur_h) {
                let size = if (row_bytes & 0x7FFF) > 250 { br.read_u16be()? } else { u16::from(br.read_byte()?) };

                let end = br.tell() + u64::from(size);

                let mut pos = cur_x;
                while br.tell() < end {
                    let op                  = br.read_byte()?;
                    if (op & 0x80) != 0 {
                        let pix             = br.read_byte()?;
                        let len = 257 - usize::from(op);
                        validate!(pos + len <= line_end);
                        for dst in dline[pos * 3..].chunks_exact_mut(3).take(len) {
                            dst.copy_from_slice(&pal[usize::from(pix) * 3..][..3]);
                        }
                        pos += len;
                    } else {
                        let len = usize::from(op) + 1;
                        validate!(pos + len <= line_end);
                        for dst in dline[pos * 3..].chunks_exact_mut(3).take(len) {
                            let pix         = br.read_byte()?;
                            dst.copy_from_slice(&pal[usize::from(pix) * 3..][..3]);
                        }
                        pos += len;
                    }
                }
            }
        },
        1 => {
            match pixel_size {
                8 => {
                    for dline in frm.data[frm.offset[0]..].chunks_exact_mut(frm.stride[0])
                            .skip(cur_y).take(cur_h) {
                        for pix in dline[cur_x * 3..].chunks_exact_mut(3).take(cur_w) {
                            let idx = usize::from(br.read_byte()?);
                            pix.copy_from_slice(&pal[idx * 3..][..3]);
                        }
                    }
                },
                24 => {
                    for dline in frm.data[frm.offset[0]..].chunks_exact_mut(frm.stride[0])
                            .skip(cur_y).take(cur_h) {
                        br.read_buf(&mut dline[cur_x * 3..][..cur_w * 3])?;
                    }
                },
                32 => {
                    for dline in frm.data[frm.offset[0]..].chunks_exact_mut(frm.stride[0])
                            .skip(cur_y).take(cur_h) {
                        for pix in dline[cur_x * 3..].chunks_exact_mut(3).take(cur_w) {
                            br.read_buf(pix)?;
                            br.read_byte()?;
                        }
                    }
                },
                _ => {
                    println!("raw {pixel_size}-bit data is not supported");
                    return Err(DecoderError::NotImplemented);
                }
            }
        },
        2 => {
            validate!(pixel_size == 24 || pixel_size == 32);
            for dline in frm.data[frm.offset[0]..].chunks_exact_mut(frm.stride[0])
                    .skip(cur_y).take(cur_h) {
                br.read_buf(&mut dline[cur_x * 3..][..cur_w * 3])?;
            }
        },
        3 => {
            validate!(pixel_size == 16);
            println!("16-bit DirectBits RLE");
            return Err(DecoderError::NotImplemented);
        },
        4 => {
            validate!(pixel_size == 32 && (3..=4).contains(&cmp_count));
            let line_end = cur_x + cur_w;
            let short_len = (row_bytes & 0x7FFF) < 250;
            for dline in frm.data[frm.offset[0]..].chunks_exact_mut(frm.stride[0])
                    .skip(cur_y).take(cur_h) {
                let mut pos = cur_x * 3;
                let mut to_dec = line_end * cmp_count;
                let data_size   = if !short_len { br.read_u16be()? } else { u16::from(br.read_byte()?) };
                let line_data_end = br.tell() + u64::from(data_size);
                let mut planes = 0;
                'linedec: while br.tell() < line_data_end {
                    let op = usize::from(br.read_byte()?);
                    if op < 0x80 {
                        let len = op + 1;
                        validate!(len <= to_dec);
                        for _ in 0..len {
                            dline[pos] = br.read_byte()?;
                            pos += 3;
                            if pos >= line_end * 3 {
                                pos -= line_end * 3 - 1;
                                planes += 1;
                                if planes == cmp_count.min(3) {
                                    break 'linedec;
                                }
                            }
                        }
                        to_dec -= len;
                    } else {
                        let len = 257 - op;
                        validate!(len <= to_dec);
                        let clr = br.read_byte()?;
                        for _ in 0..len {
                            dline[pos] = clr;
                            pos += 3;
                            if pos >= line_end * 3 {
                                pos -= line_end * 3 - 1;
                                planes += 1;
                                if planes == cmp_count.min(3) {
                                    break 'linedec;
                                }
                            }
                        }
                        to_dec -= len;
                    }
                }
                br.seek(SeekFrom::Start(line_data_end))?;
            }
        },
        _ => unreachable!(),
    }
    Ok(())
}

impl NADecoder for QDrawDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            self.width  = vinfo.get_width();
            self.height = vinfo.get_height();
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(self.width, self.height, false, RGB24_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();

            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    #[allow(unused_assignments)]
    #[allow(unused_variables)]
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() >= 2);
        let mut br = MemoryReader::new_read(src.as_slice());
        if br.peek_u16le()? == 0 {
                                          br.read_skip(0x200)?;
        }
                                          br.read_u16be()?; // low 16 bits of size
        let cur_y                       = usize::from(br.read_u16be()?);
        let cur_x                       = usize::from(br.read_u16be()?);
        let cur_h                       = usize::from(br.read_u16be()?);
        let cur_w                       = usize::from(br.read_u16be()?);
        validate!(cur_x < cur_w && cur_w - cur_x <= self.width);
        validate!(cur_y < cur_h && cur_h - cur_y <= self.height);
        let version_op                  = br.read_u16be()?;
        validate!(version_op == 0x0011);
        let version                     = br.read_u16be()?;
        validate!(version == 0x2FF);
        let hdr_op                      = br.read_u16be()?;
        validate!(hdr_op == 0x0C00);
        let _size                       = br.read_u32be()?;
                                          br.read_skip(16)?; // bounding box
                                          br.read_skip(4)?; // reserved

        let bufret = self.hams.clone_ref();
        let mut buf;
        if let Some(bbuf) = bufret {
            buf = bbuf;
        } else {
            let bufinfo = alloc_video_buffer(self.info.get_properties().get_video_info().unwrap(), 0)?;
            buf = bufinfo.get_vbuf().unwrap();
            self.hams.add_frame(buf);
            buf = self.hams.get_output_frame().unwrap();
        }
        let mut frm = NASimpleVideoFrame::from_video_buf(&mut buf).unwrap();

        let mut fg_clr = [0; 3];
        let mut bg_clr = [0; 3];
        let mut fill_clr = [0; 3];
        while br.left() >= 2 {
            let op                      = br.read_u16be()?;
            match op {
                0x0000 => {}, // nop
                0x0001 => { // clip
                    let size            = usize::from(br.read_u16be()?);
                    validate!(size >= 2);
                                          br.read_skip(size - 2)?;
                },
                0x0002 => { // bk pattern
                                          br.read_pattern()?;
                },
                0x0003 => { // text font
                                          br.read_u16be()?;
                },
                0x0004 => { // text face
                                          br.read_byte()?;
                },
                0x0005 => { // text mode
                                          br.read_u16be()?;
                },
                0x0007 => { // pen size
                    let size            = br.read_u32be()?;
                },
                0x0008 => { // pen mode
                                          br.read_u16be()?;
                },
                0x0009 => { // pen pattern
                                          br.read_pattern()?;
                },
                0x000A => { // fill pattern
                                          br.read_pattern()?;
                },
                0x000B => { // oval size
                    let xpos            = br.read_u16be()?;
                    let ypos            = br.read_u16be()?;
                },
                0x000C => { // origin
                    let xpos            = br.read_u16be()?;
                    let ypos            = br.read_u16be()?;
                },
                0x000D => { // text size
                                          br.read_skip(2)?;
                },
                0x0010 => { // text ratio
                                          br.read_skip(8)?;
                },
                0x0011 => {
                    println!("duplicate version opcode!");
                    return Err(DecoderError::InvalidData);
                },
                0x001A => { // fore color
                    fg_clr              = br.read_colour()?;
                },
                0x001B => { // back color
                    bg_clr              = br.read_colour()?;
                },
                0x001E => {}, // def hilite
                0x001F => { // fill color
                    fill_clr            = br.read_colour()?;
                },
                0x0020 => { // line
                    let sx              = br.read_u16be()?;
                    let sy              = br.read_u16be()?;
                    let dx              = br.read_u16be()?;
                    let dy              = br.read_u16be()?;
                },
                0x0021 => { // line from
                    let dx              = br.read_u16be()?;
                    let dy              = br.read_u16be()?;
                },
                0x0022 => { // short line
                    let xpos            = br.read_u16be()?;
                    let ypos            = br.read_u16be()?;
                    let dx              = br.read_byte()? as i8;
                    let dy              = br.read_byte()? as i8;
                },
                0x0023 => { // short line from
                    let dx              = br.read_byte()? as i8;
                    let dy              = br.read_byte()? as i8;
                },
                0x0028 => { // long text
                    let _xpos           = br.read_u16be()?;
                    let _ypos           = br.read_u16be()?;
                    let count           = usize::from(br.read_byte()?);
                                          br.read_skip(count)?;
                },
                0x0031 => { // paint rect
                    let sy              = usize::from(br.read_u16be()?);
                    let sx              = usize::from(br.read_u16be()?);
                    let dy              = usize::from(br.read_u16be()?);
                    let dx              = usize::from(br.read_u16be()?);
                    validate!(sy < dy && sx < dx);
                    for dline in frm.data[frm.offset[0]..].chunks_exact_mut(frm.stride[0])
                            .take(dy.min(self.height)).skip(sy) {
                        for pix in dline.chunks_exact_mut(3).take(dx.min(self.width)).skip(sx) {
                            pix.copy_from_slice(&fg_clr);
                        }
                    }
                },
                0x0098 => { // PackBits rect
                    decode_bits(&mut br, &mut frm, self.width, self.height, false, false)?;
                },
                0x0099 => { // PackBits region
                    decode_bits(&mut br, &mut frm, self.width, self.height, true,  false)?;
                },
                0x009A => { // DirectBits rect
                    decode_bits(&mut br, &mut frm, self.width, self.height, false, true)?;
                },
                0x009B => { // DirectBits region
                    decode_bits(&mut br, &mut frm, self.width, self.height, true,  true)?;
                },
                0x00A0 => { // short comment
                                          br.read_u16be()?;
                },
                0x00A1 => { // long comment
                                          br.read_u16be()?;
                    let size            = usize::from(br.read_u16be()?);
                                          br.read_skip(size)?;
                },
                0x00FF => break,
                0x0C00 => {
                    println!("duplicate header opcode!");
                    return Err(DecoderError::InvalidData);
                },
                0x8200 => {
                    let size            = br.read_u32be()? as usize;
                    validate!(size > 0x9A && (br.left() as usize) >= size);
                    let end = br.tell() + (size as u64);
                                          br.read_skip(0x44)?;
                    let desc_size       = br.read_u32be()? as usize;
                    let desc_end = br.tell() + (desc_size as u64) - 4;
                    validate!(desc_size >= 0x56 && desc_end < end);
                    let fcc             = br.read_tag()?;
                                          br.read_skip(12)?;
                                          br.read_tag()?; // vendor = "appl"
                                          br.read_skip(8)?; // quality
                    let qwidth          = usize::from(br.read_u16be()?);
                    let qheight         = usize::from(br.read_u16be()?);
                                          br.read_skip(8)?; // dpi
                    let data_size       = br.read_u32be()? as usize;
                    validate!(data_size <= size - desc_size - 0x44);
                                          br.read_u16be()?; // frame count
                                          br.read_skip(32)?; // codec name
                    let depth           = br.read_u16be()?;
                    let ctable          = br.read_u16be()?;

                                          br.seek(SeekFrom::Start(desc_end))?;
                    validate!(br.tell() + (data_size as u64) <= end);
                    let mut data = vec![0; data_size];
                                          br.read_buf(&mut data)?;
                                          br.seek(SeekFrom::Start(end))?;

                    let cname = match &fcc {
                            b"8BPS" => "8bps",
                            b"cvid" => "cinepak",
                            b"jpeg" => "jpeg",
                            b"smc " => "qt-smc",
                            b"rle " => "qt-rle",
                            b"rpza" => "apple-video",
                            b"kpcd" => "kodak-photocd",
                            b"mjpa" => "mjpeg-a",
                            b"mjpb" => "mjpeg-b",
                            b"SVQ1" | b"svqi" => "sorenson-video",
                            b"SVQ3" | b"svq3" => "sorenson-video3",
                            _ => "unknown",
                        };
                    let decfunc = if let Some(df) = self.decs.find_decoder(cname) {
                            df
                        } else {
                            println!("can't find {cname} decoder");
                            return Err(DecoderError::NotImplemented);
                        };
                    let mut dec = (decfunc)();
                    let mut dsupp = Box::new(NADecoderSupport::new());
                    let fmt = match depth {
                            1..=8 | 33..=40 => PAL8_FORMAT,
                            15 | 16 => NAPixelFormaton::make_rgb16_fmt(5, 5, 5, true, true),
                            24 => RGB24_FORMAT,
                            32 => ARGB_FORMAT,
                            _ => {
                                println!("don't know what to do with depth {depth}");
                                return Err(DecoderError::NotImplemented);
                            },
                        };
                    let mut vinfo = NAVideoInfo::new(qwidth, qheight, false, fmt);
                    vinfo.bits = (if depth > 32 { depth - 32 } else { depth }) as u8;
                    let info = NACodecInfo::new(cname, NACodecTypeInfo::Video(vinfo), None);
                    let stream = NAStream::new(StreamType::Video, 0, info, 1, 1, 0).into_ref();
                    if let Err(err) = dec.init(&mut dsupp, stream.get_info()) {
                        println!("Sub-decoder {cname} init error");
                        return Err(err);
                    }
                    let fake_pkt = NAPacket::new(stream, NATimeInfo::new(None, None, None, 1, 1), true, data);
                    let qtfrm = if let Ok(qf) = dec.decode(&mut dsupp, &fake_pkt) {
                            qf
                        } else {
                            println!("Sub-decoder {cname} decode error");
                            return Err(DecoderError::InvalidData);
                        };

                    let same = if let Some(vinfo) = qtfrm.get_buffer().get_video_info() {
                            vinfo.width == self.width && vinfo.height == self.height// && vinfo.format == RGB24_FORMAT
                        } else { false };
                    let dbuf = if same {
                            qtfrm.get_buffer()
                        } else {
                            let src_buf = qtfrm.get_buffer();
                            let dst_vinfo = self.info.get_properties().get_video_info().unwrap();
                            let mut out_pic = alloc_video_buffer(dst_vinfo, 0)?;
                            let dec_fmt = get_scale_fmt_from_pic(&src_buf);
                            let tgt_fmt = get_scale_fmt_from_pic(&out_pic);

                            let mut scaler = NAScale::new(dec_fmt, tgt_fmt).map_err(|_| DecoderError::Bug)?;
                            scaler.convert(&src_buf, &mut out_pic).map_err(|_| DecoderError::Bug)?;
                            out_pic
                        };
                    let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), dbuf);
                    frm.set_frame_type(FrameType::I);
                    return Ok(frm.into_ref());
                },
                0x8000..=0x80FF => {
                    println!("unknown opcode {op:04X}");
                },
                0x8100..=0xFFFF => {
                    println!("unknown opcode {op:04X}");
                    let size            = br.read_u32be()? as usize;
                    validate!(size >= 4);
                                          br.read_skip(size)?;
                },
                _ => return Err(DecoderError::NotImplemented),
            }
            if (br.tell() & 1) != 0 {
                                          br.read_skip(1)?;
            }
        }

        let buftype = NABufferType::Video(buf);

        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), buftype);
        frm.set_frame_type(if pkt.is_keyframe() { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        self.hams.clear();
    }
}

impl NAOptionHandler for QDrawDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(QDrawDecoder::new())
}

pub fn get_qdraw_decoder_with_external_decreg(ref_decs: &RegisteredDecoders) -> Box<dyn NADecoder + Send> {
    let mut decs = RegisteredDecoders::new();
    for dec in ref_decs.iter() {
        decs.add_decoder(*dec);
    }
    Box::new(QDrawDecoder{ decs, ..Default::default() })
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::qt_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_qdraw() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);

        // sample: https://samples.mplayerhq.hu/V-codecs/QT-qdrw/Airplane.mov
        test_decoding("mov", "qdraw", "assets/QT/Airplane.mov", None, &dmx_reg, &dec_reg,
                      ExpectedTestResult::MD5Frames(vec![
                            [0x9aced174, 0x4c9f58ca, 0xeacace05, 0x0d11e351],
                            [0xf7459501, 0xfbf81fd2, 0xcc436363, 0x95748ed6]]));
    }
}
