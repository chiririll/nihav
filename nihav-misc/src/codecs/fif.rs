use nihav_core::io::byteio::read_u16le;
use nihav_core::io::bitreader::*;
use nihav_core::codecs::*;

const WIDTH:  usize = 320;
const HEIGHT: usize = 200;

const LUMA_BLOCKS: usize = WIDTH * HEIGHT / 16;

struct FractalFrame {
    luma:       Vec<u8>,
    chroma:     Vec<u8>,
}

impl FractalFrame {
    fn new() -> Self {
        FractalFrame {
            luma:   vec![0xAB; WIDTH * HEIGHT],
            chroma: vec![0xAB; WIDTH * HEIGHT],
        }
    }
    fn affine_tx_4x4(&mut self, prev_frm: &Self, addr: &Addresses, blk_idx: usize, src_idx: usize, mode: u32, scale: &[u8; 256]) {
        let mut src_off = addr.src_addr[src_idx];
        let dst_off = addr.dst_addr[blk_idx];

        let is_mirrored = (mode == 1) || (mode == 3);
        let is_flipped  = (mode == 2) || (mode == 3);

        if is_flipped {
            src_off += WIDTH * 2 * 3;
        }

        let (dest, source) = if blk_idx < LUMA_BLOCKS {
                (&mut self.luma, &prev_frm.luma)
            } else {
                (&mut self.chroma, &prev_frm.chroma)
            };

        for line in dest[dst_off..].chunks_mut(WIDTH).take(4) {
            if !is_mirrored {
                for (dst, src) in line[..4].iter_mut()
                        .zip(source[src_off..].chunks(2)) {
                    *dst = scale[usize::from(src[0])];
                }
            } else {
                for (dst, src) in line[..4].iter_mut()
                        .zip(source[src_off..][..8].chunks(2).rev()) {
                    *dst = scale[usize::from(src[0])];
                }
            }
            if !is_flipped {
                src_off += WIDTH * 2;
            } else {
                src_off = src_off.wrapping_sub(WIDTH * 2);
            }
        }
    }
    fn affine_tx_8x8(&mut self, prev_frm: &Self, addr: &Addresses, blk_idx: usize, src_idx: usize, emode: u32, scale: &[u8; 256]) {
        let (dest, source) = if blk_idx < LUMA_BLOCKS {
                (&mut self.luma, &prev_frm.luma)
            } else {
                (&mut self.chroma, &prev_frm.chroma)
            };
        let mut src_off = addr.src_addr[src_idx];
        let dst_off = addr.dst_addr[blk_idx];

        if matches!(emode, 2 | 3) {
            src_off += WIDTH * 2 * 8;
        }
        if matches!(emode, 5 | 7) {
            src_off += 8 * 2;
        }
        for line in dest[dst_off..].chunks_mut(WIDTH).take(8) {
            match emode {
                0 => { // normal
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..][..16].chunks(2)) {
                        *dst = scale[usize::from(src[0])];
                    }
                    src_off += WIDTH * 2;
                },
                1 => { // mirrored
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..][..16].chunks(2).rev()) {
                        *dst = scale[usize::from(src[0])];
                    }
                    src_off += WIDTH * 2;
                },
                2 => { // flipped
                    src_off -= WIDTH * 2;
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..][..16].chunks(2)) {
                        *dst = scale[usize::from(src[0])];
                    }
                },
                3 => { // flipped+mirrored
                    src_off -= WIDTH * 2;
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..][..16].chunks(2).rev()) {
                        *dst = scale[usize::from(src[0])];
                    }
                },
                4 => { // transposed
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..].chunks(WIDTH * 2)) {
                        *dst = scale[usize::from(src[0])];
                    }
                    src_off += 2;
                },
                5 => { // transposed+mirrored
                    src_off -= 2;
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..].chunks(WIDTH * 2)) {
                        *dst = scale[usize::from(src[0])];
                    }
                },
                6 => { // transposed and flipped
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..].chunks(WIDTH * 2)
                                .take(8).rev()) {
                        *dst = scale[usize::from(src[0])];
                    }
                    src_off += 2;
                },
                7 => { // transposed, flipped and mirrored
                    src_off -= 2;
                    for (dst, src) in line[..8].iter_mut()
                            .zip(source[src_off..].chunks(WIDTH * 2)
                                .take(8).rev()) {
                        *dst = scale[usize::from(src[0])];
                    }
                },
                _ => {},
            }
        }
    }
    fn copy_block(&mut self, prev_frm: &Self, addr: &Addresses, blk_idx: usize) {
        let off = addr.dst_addr[blk_idx];
        let (dest, source) = if blk_idx < LUMA_BLOCKS {
                (&mut self.luma, &prev_frm.luma)
            } else {
                (&mut self.chroma, &prev_frm.chroma)
            };
        for (dline, sline) in dest[off..].chunks_mut(WIDTH)
                .zip(source[off..].chunks(WIDTH)).take(4) {
            dline[..4].copy_from_slice(&sline[..4]);
        }
    }
    fn mv_block(&mut self, prev_frm: &Self, addr: &Addresses, blk_idx: usize, mv_off: i16) -> DecoderResult<()> {
        let (dest, source) = if blk_idx < LUMA_BLOCKS {
                (&mut self.luma, &prev_frm.luma)
            } else {
                (&mut self.chroma, &prev_frm.chroma)
            };
        let dst_off = addr.dst_addr[blk_idx];
        let src_off = (dst_off as isize) + isize::from(mv_off);
        validate!(src_off >= 0);
        let src_off = src_off as usize;
        validate!(src_off + WIDTH * 3 + 4 <= source.len());
        for (dline, sline) in dest[dst_off..].chunks_mut(WIDTH)
                .zip(source[src_off..].chunks(WIDTH)).take(4) {
            dline[..4].copy_from_slice(&sline[..4]);
        }
        Ok(())
    }
    fn read_raw_4x4(&mut self, addr: &Addresses, br: &mut BitReader, blk_idx: usize) -> DecoderResult<()> {
        let dest = if blk_idx < LUMA_BLOCKS { &mut self.luma } else { &mut self.chroma };
        let dst_off = addr.dst_addr[blk_idx];
        for line in dest[dst_off..].chunks_mut(WIDTH).take(4) {
            for el in line[..4].iter_mut() {
                *el = br.read(8)? as u8;
            }
        }
        Ok(())
    }
    fn read_raw_2x2(&mut self, br: &mut BitReader, blk_idx: usize) -> DecoderResult<()> {
        let dest = if blk_idx < LUMA_BLOCKS { &mut self.luma } else { &mut self.chroma };
        let dst_off = br.read(16)? as usize;
        for line in dest[dst_off..].chunks_mut(WIDTH).take(2) {
            for el in line[..2].iter_mut() {
                *el = br.read(8)? as u8;
            }
        }
        Ok(())
    }
}

struct Addresses {
    src_addr:   [usize; WIDTH * HEIGHT / 4],
    dst_addr:   [usize; WIDTH * HEIGHT / 8],
    state_addr: [usize; WIDTH * HEIGHT / 8],
}

impl Addresses {
    fn fill_dst_addr(dst: &mut [usize], base: usize, line: usize, tile_w: usize, tile_h: usize, chroma: bool) -> usize {
        const SCAN: [(u8, u8); 16] = [
            (0, 0), (4, 0), (0,  4), (4,  4), (8, 0), (12, 0), (8,  4), (12,  4),
            (0, 8), (4, 8), (0, 12), (4, 12), (8, 8), (12, 8), (8, 12), (12, 12)
        ];
        let num_x = if !chroma { WIDTH / tile_w } else { WIDTH / 2 / tile_w };
        for (i, chunk) in dst.chunks_exact_mut(tile_h).take(num_x).enumerate() {
            for (doff, &(scan_x, scan_y)) in chunk.iter_mut().zip(SCAN.iter()) {
                *doff = base + i * tile_w + usize::from(scan_x) + (usize::from(scan_y) + line) * WIDTH;
            }
        }
        num_x * tile_h
    }
    fn fill_state_addr(dst: &mut [usize], base: usize, line: usize, tile_w: usize, tile_h: usize, chroma: bool) -> usize {
        const SCAN: [(u8, u8); 16] = [
            (0, 0), (1, 0), (0, 1), (1, 1), (2, 0), (3, 0), (2, 1), (3, 1),
            (0, 2), (1, 2), (0, 3), (1, 3), (2, 2), (3, 2), (2, 3), (3, 3)
        ];
        let num_x = if !chroma { WIDTH / 4 / tile_w } else { WIDTH / 8 / tile_w };
        for (i, chunk) in dst.chunks_exact_mut(tile_h).take(num_x).enumerate() {
            for (doff, &(scan_x, scan_y)) in chunk.iter_mut().zip(SCAN.iter()) {
                *doff = base + i * tile_w + usize::from(scan_x) + (usize::from(scan_y) + line / 4) * WIDTH / 4;
            }
        }
        num_x * tile_h
    }

    fn new() -> Self {
        let mut dst_addr = [0; WIDTH * HEIGHT / 8];
        let mut off = 0;
        for y in (0..192).step_by(16) {
            off += Self::fill_dst_addr(&mut dst_addr[off..], 0, y, 16, 16, false);
        }
        off += Self::fill_dst_addr(&mut dst_addr[off..], 0, 192, 16, 8, false);
        for y in (0..96).step_by(8) {
            off += Self::fill_dst_addr(&mut dst_addr[off..], 0, y, 16, 8, true);
        }
        off += Self::fill_dst_addr(&mut dst_addr[off..], 0, 96, 8, 2, true);
        for y in (0..96).step_by(8) {
            off += Self::fill_dst_addr(&mut dst_addr[off..], WIDTH / 2, y, 16, 8, true);
        }
        off += Self::fill_dst_addr(&mut dst_addr[off..], WIDTH / 2, 96, 8, 2, true);
        let _ = off; // just to suppress a warning

        let mut state_addr = [0; WIDTH * HEIGHT / 8];
        let mut off = 0;
        for y in (0..192).step_by(16) {
            off += Self::fill_state_addr(&mut state_addr[off..], 0, y, 4, 16, false);
        }
        off += Self::fill_state_addr(&mut state_addr[off..], 0, 192, 4, 8, false);
        for y in (200..296).step_by(8) {
            off += Self::fill_state_addr(&mut state_addr[off..], 0, y, 4, 8, true);
        }
        off += Self::fill_state_addr(&mut state_addr[off..], 0, 296, 2, 2, true);
        for y in (200..296).step_by(8) {
            off += Self::fill_state_addr(&mut state_addr[off..], WIDTH / 8, y, 4, 8, true);
        }
        off += Self::fill_state_addr(&mut state_addr[off..], WIDTH / 8, 296, 2, 2, true);
        let _ = off; // just to suppress a warning

        Self {
            src_addr:   std::array::from_fn(|i| { let x = i % (WIDTH / 2); let y = i / (WIDTH / 2); x * 2 + y * WIDTH * 2 }),
            dst_addr, state_addr,
        }
    }
}

struct FractalDecoder {
    cur_frm:    FractalFrame,
    prev_frm:   FractalFrame,
    blk_state:  [u8; WIDTH * HEIGHT / 4],
    addr:       Box<Addresses>, // just to evade stack overflow on tests

    scale:      [[u8; 256]; 128],
    mv_offs:    Vec<i16>,
    def_state:  u8,

    rgbfrm:     Vec<u8>,
    cur_src:    bool,
}


impl FractalDecoder {
    fn new() -> Self {
        Self {
            cur_frm:    FractalFrame::new(),
            prev_frm:   FractalFrame::new(),
            blk_state:  [0; WIDTH * HEIGHT / 4],
            addr:       Box::new(Addresses::new()),

            scale:      std::array::from_fn(|i| std::array::from_fn(|j| ((i as i32 - 64) * 4 + (((j as i32) * 3 + 2) >> 2)).clamp(0, 255) as u8)),
            mv_offs:    Vec::new(),
            def_state:  2,

            rgbfrm:     vec![0; WIDTH * HEIGHT * 3],
            cur_src:    true,
        }
    }

    fn decode_normal_block(&mut self, mode: u32, br: &mut BitReader, blk_idx: usize) -> DecoderResult<()> {
        let scale_idx = br.read(7)? as usize;
        let scale = &self.scale[scale_idx];
        let src_idx = br.read(14)? as usize;
        validate!(src_idx < self.addr.src_addr.len());

        self.cur_frm.affine_tx_4x4(&self.prev_frm, &self.addr, blk_idx, src_idx, mode, scale);
        self.blk_state[self.addr.state_addr[blk_idx]] = 0;

        Ok(())
    }
    fn raw_block(&mut self, br: &mut BitReader, blk_idx: usize) -> DecoderResult<()> {
        br.align();
        self.cur_frm.read_raw_4x4(&self.addr, br, blk_idx)?;
        self.blk_state[self.addr.state_addr[blk_idx]] = 0;
        Ok(())
    }

    fn decode_frame(&mut self, src: &[u8], flags: u16) -> DecoderResult<()> {
        if (flags & 8) != 0 {
            self.def_state = 2;
            return Ok(());
        }
        self.decode_frame_internal(src, flags)?;
        if (flags & 1) != 0 {
            for _ in 0..15 {
                self.decode_frame_internal(src, flags)?;
            }
        }
        self.cur_src = (flags & 4) == 0;
        self.def_state = if (flags & 4) == 0 { 2 } else { 3 };
        Ok(())
    }
    fn decode_frame_internal(&mut self, src: &[u8], flags: u16) -> DecoderResult<()> {
        const TOTAL_BLOCKS: usize = WIDTH * HEIGHT * 3 / 32;
        let mut br = BitReader::new(src, BitReaderMode::LE);
        let mut blk_idx = 0;
        let num_offs = br.read(16)? as usize;
        validate!(num_offs <= 16);
        self.mv_offs.clear();
        for _ in 0..num_offs {
            let mv_off = br.read(16)? as i16;
            self.mv_offs.push(mv_off);
        }
        while blk_idx < TOTAL_BLOCKS {
            let mode = br.read(3)?;
            match mode {
                0..=3 => {
                    self.decode_normal_block(mode, &mut br, blk_idx)?;
                    blk_idx += 1;
                },
                4 => {
                    let count = br.read(5)? as usize;
                    validate!(blk_idx + count < TOTAL_BLOCKS);
                    for _ in 0..=count {
                        let blk_state = &mut self.blk_state[self.addr.state_addr[blk_idx]];
                        if (*blk_state & 2) != 0 {
                            *blk_state = 3;
                        } else {
                            *blk_state = self.def_state;
                            self.cur_frm.copy_block(&self.prev_frm, &self.addr, blk_idx);
                        }
                        blk_idx += 1;
                    }
                },
                5 => {
                    let flag = br.read_bool()?;
                    let mv_idx = br.read(4)? as usize;
                    validate!(mv_idx < self.mv_offs.len());
                    let count = if flag { br.read(8)? as usize + 1 } else { 0 };
                    let mv_off = self.mv_offs[mv_idx];
                    validate!(blk_idx + count < TOTAL_BLOCKS);
                    for _ in 0..=count {
                        self.cur_frm.mv_block(&self.prev_frm, &self.addr, blk_idx, mv_off)?;
                        self.blk_state[self.addr.state_addr[blk_idx]] = 0;
                        blk_idx += 1;
                    }
                },
                6 => {
                    self.raw_block(&mut br, blk_idx)?;
                    blk_idx += 1;
                },
                7 => {
                    let emode = br.read(4)?;
                    match emode {
                        0..=7 => {
                            br.align();
                            let scale_idx = br.read(7)? as usize;
                            let scale = &self.scale[scale_idx];
                            let has_refinement = br.read_bool()?;
                            let src_idx = br.read(14)? as usize;
                            validate!(src_idx < self.addr.src_addr.len());
                            let subblk_off = br.read(2)? as usize;
                            self.cur_frm.affine_tx_8x8(&self.prev_frm, &self.addr, blk_idx, src_idx, emode, scale);

                            self.blk_state[self.addr.state_addr[blk_idx]]                 = 0;
                            self.blk_state[self.addr.state_addr[blk_idx]             + 1] = 0;
                            self.blk_state[self.addr.state_addr[blk_idx] + WIDTH / 4]     = 0;
                            self.blk_state[self.addr.state_addr[blk_idx] + WIDTH / 4 + 1] = 0;

                            if has_refinement {
                                let rf_blk_idx = blk_idx + subblk_off;

                                let rf_mode = br.read(3)?;
                                match rf_mode {
                                    0..=3 => {
                                        self.decode_normal_block(rf_mode, &mut br, rf_blk_idx)?;
                                    },
                                    4 => {
                                        br.align();
                                        let blk_state = &mut self.blk_state[self.addr.state_addr[rf_blk_idx]];
                                        *blk_state &= 1;
                                        if *blk_state == 0 {
                                            *blk_state = 1;
                                            self.cur_frm.copy_block(&self.prev_frm, &self.addr, rf_blk_idx);
                                        }
                                    },
                                    5 => {
                                        let marker = br.read_bool()?;
                                        validate!(!marker);
                                        let mv_idx = br.read(4)? as usize;
                                        validate!(mv_idx < self.mv_offs.len());
                                        let mv_off = self.mv_offs[mv_idx];
                                        self.cur_frm.mv_block(&self.prev_frm, &self.addr, rf_blk_idx, mv_off)?;
                                    },
                                    6 => {
                                        self.raw_block(&mut br, rf_blk_idx)?;
                                    },
                                    _ => return Err(DecoderError::InvalidData),
                                }
                            }

                            blk_idx += 4;
                        },
                        12 => {
                            let flag = br.read_bool()?;
                            let count = if !flag { br.read(8)? as usize + 0x20 } else { br.read(16)? as usize + 0x120 };
                            validate!(blk_idx + count < TOTAL_BLOCKS);
                            for _ in 0..=count {
                                let blk_state = &mut self.blk_state[self.addr.state_addr[blk_idx]];
                                if (*blk_state & 2) != 0 {
                                    *blk_state = 3;
                                } else {
                                    *blk_state = self.def_state;
                                    self.cur_frm.copy_block(&self.prev_frm, &self.addr, blk_idx);
                                }
                                blk_idx += 1;
                            }
                        },
                        15 => {
                            br.align();
                            self.cur_frm.read_raw_2x2(&mut br, blk_idx)?;
                        },
                        _ => return Err(DecoderError::InvalidData),
                    }
                },
                _ => unreachable!(),
            }
        }

        if (flags & 4) != 0 {
            std::mem::swap(&mut self.cur_frm, &mut self.prev_frm);
        }

        let (ystate, cstate) = self.blk_state.split_at_mut(LUMA_BLOCKS);
        for (ystrip, cstrip) in ystate.chunks_exact_mut(WIDTH / 2).zip(cstate.chunks_exact(WIDTH / 4)) {
            let (ustate, vstate) = cstrip.split_at(WIDTH / 8);
            for (x, (&us, &vs)) in ustate.iter().zip(vstate.iter()).enumerate() {
                ystrip[x * 2]                 |= (us & vs) << 2;
                ystrip[x * 2 + 1]             |= (us & vs) << 2;
                ystrip[x * 2     + WIDTH / 4] |= (us & vs) << 2;
                ystrip[x * 2 + 1 + WIDTH / 4] |= (us & vs) << 2;
            }
        }

        Ok(())
    }

    fn get_frame(&mut self) -> &[u8] {
        let frm = if self.cur_src { &self.cur_frm } else { &self.prev_frm };
        for ((dstrip, ystate), (ystrip, cstrip)) in
                self.rgbfrm.chunks_exact_mut(WIDTH * 3 * 4).zip(self.blk_state.chunks_exact(WIDTH / 4))
                    .zip(frm.luma.chunks_exact(WIDTH * 4).zip(frm.chroma.chunks_exact(WIDTH * 2))) {
            for (x, &blk_state) in ystate.iter().enumerate() {
                if blk_state == 0xF {
                    continue;
                }
                let dst = &mut dstrip[x * 4 * 3..];
                let ysrc = &ystrip[x * 4..];
                let csrc = &cstrip[x * 2..];
                for (dlines, (ylines, cline)) in dst.chunks_mut(WIDTH * 3 * 2)
                        .zip(ysrc.chunks(WIDTH * 2).zip(csrc.chunks(WIDTH))) {
                    let (dline0, dline1) = dlines.split_at_mut(WIDTH * 3);
                    let (yline0, yline1) = ylines.split_at(WIDTH);
                    let (uline, vline) = cline.split_at(WIDTH / 2);

                    for (dst, (ypair, (&u, &v))) in dline0.chunks_exact_mut(6).take(4)
                            .zip(yline0.chunks_exact(2).zip(uline.iter().zip(vline.iter()))) {
                        dst[0] = u;
                        dst[1] = ypair[0];
                        dst[2] = v;
                        dst[3] = u;
                        dst[4] = ypair[1];
                        dst[5] = v;
                    }
                    for (dst, (ypair, (&u, &v))) in dline1.chunks_exact_mut(6).take(4)
                            .zip(yline1.chunks_exact(2).zip(uline.iter().zip(vline.iter()))) {
                        dst[0] = u;
                        dst[1] = ypair[0];
                        dst[2] = v;
                        dst[3] = u;
                        dst[4] = ypair[1];
                        dst[5] = v;
                    }
                }
            }
        }
        self.rgbfrm.as_slice()
    }
}

struct FIFDecoder {
    info:       NACodecInfoRef,
    frac:       FractalDecoder,
}

impl FIFDecoder {
    fn new() -> Self {
        Self {
            info:       NACodecInfo::new_dummy(),
            frac:       FractalDecoder::new(),
        }
    }
}

impl NADecoder for FIFDecoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            if w != WIDTH || h != HEIGHT {
                return Err(DecoderError::NotImplemented);
            }
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, RGB24_FORMAT));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        validate!(src.len() >= 8);

        let flags = read_u16le(&src[4..])?;
        self.frac.decode_frame(&src[6..], flags)?;

        let ffrm = self.frac.get_frame();

        let vinfo = self.info.get_properties().get_video_info().unwrap();
        let bufinfo = alloc_video_buffer(vinfo, 0)?;

        if let Some(mut buf) = bufinfo.get_vbuf() {
            let stride = buf.get_stride(0);
            let data = buf.get_data_mut().unwrap();

            for (dline, sline) in data.chunks_mut(stride).zip(ffrm.chunks_exact(WIDTH * 3)) {
                dline[..WIDTH * 3].copy_from_slice(sline);
            }
        } else { unreachable!(); }

        let frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
    }
}

impl NAOptionHandler for FIFDecoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(FIFDecoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::*;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_fif() {
        let mut dmx_reg = RegisteredDemuxers::new();
        misc_register_all_demuxers(&mut dmx_reg);
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        misc_register_all_decoders(&mut dec_reg);

        // sample from Images Incorporated 3.2
        test_decoding("avi", "fif", "assets/Misc/FERNLOGO.AVI", Some(7), &dmx_reg,
                     &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0xf2a41fde, 0xcbc0111a, 0xaf86f613, 0x0a79a6db],
                            [0x38c53237, 0x64ffa574, 0x31eb0c56, 0x0d8dbb22],
                            [0xd8863468, 0xc82c4b6f, 0xf6521e99, 0x03e82561],
                            [0xb14ade1d, 0x5f19d79a, 0xbe5f9035, 0xdc49e76f],
                            [0xe8c72a47, 0x4247793d, 0xe03a172c, 0xddb1ddd6],
                            [0x5f8482e0, 0x4c7cebd0, 0x0a4260c1, 0x9f890545],
                            [0x632232df, 0xa7e4e4a8, 0x33a4e031, 0x86404ffa],
                            [0x24ec0d53, 0xe3cba1f8, 0xa3c655bc, 0xfd70985e]]));
    }
}
