use nihav_core::formats;
use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use std::io::SeekFrom;
use std::mem;
use super::indeo3data::*;

#[derive(Clone, Copy)]
struct MV {
    x: i8,
    y: i8
}

struct Buffers {
    width:      usize,
    height:     usize,
    cw:         usize,
    ch:         usize,
    sbuf:       Vec<u8>,
    dbuf:       Vec<u8>,
}

const DEFAULT_PIXEL: u8 = 0x40;

impl Buffers {
    fn new() -> Self { Buffers { width: 0, height: 0, cw: 0, ch: 0, sbuf: Vec::new(), dbuf: Vec::new() } }
    fn reset(&mut self) {
        self.width  = 0;
        self.height = 0;
        self.sbuf.clear();
        self.dbuf.clear();
    }
    fn alloc(&mut self, w: usize, h: usize) {
        self.width  = w;
        self.height = h;
        self.cw = ((w >> 2) + 3) & !3;
        self.ch = ((h >> 2) + 3) & !3;
        self.sbuf.resize(w * h + self.cw * self.ch * 2, DEFAULT_PIXEL);
        self.dbuf.resize(w * h + self.cw * self.ch * 2, DEFAULT_PIXEL);
    }
    fn flip(&mut self) { std::mem::swap(&mut self.sbuf, &mut self.dbuf); }
    fn get_stride(&mut self, planeno: usize) -> usize {
        if planeno == 0 { self.width } else { self.cw }
    }
    fn get_offset(&mut self, planeno: usize) -> usize {
        match planeno {
            1 => self.width * self.height,
            2 => self.width * self.height + self.cw * self.ch,
            _ => 0,
        }
    }
    fn fill_framebuf(&mut self, fbuf: &mut NAVideoBuffer<u8>) {
        for planeno in 0..3 {
            let mut soff = self.get_offset(planeno);
            let mut doff = fbuf.get_offset(planeno);
            let sstride = self.get_stride(planeno);
            let dstride = fbuf.get_stride(planeno);
            let width  = if planeno == 0 { self.width }  else { self.width >> 2 };
            let height = if planeno == 0 { self.height } else { self.height >> 2 };
            let src = self.dbuf.as_slice();
            let dst = fbuf.get_data_mut().unwrap();
            for _ in 0..height {
                for x in 0..width {
                    dst[doff + x] = src[soff + x] * 2;
                }
                soff += sstride;
                doff += dstride;
            }
        }
    }
    fn copy_block(&mut self, doff: usize, soff: usize, stride: usize, w: usize, h: usize) {
        let mut sidx = soff;
        let mut didx = doff;
        for _ in 0..h {
            self.dbuf[didx..][..w].copy_from_slice(&self.sbuf[sidx..][..w]);
            sidx += stride;
            didx += stride;
        }
    }
    fn fill_block(&mut self, doff: usize, stride: usize, w: usize, h: usize, topline: bool) {
        let mut didx = doff;
        let mut buf: [u8; 8] = [0; 8];
        if topline {
            for _ in 0..h {
                for i in 0..w { self.dbuf[didx + i] = DEFAULT_PIXEL; }
                didx += stride;
            }
        } else {
            for i in 0..w { buf[i] = self.dbuf[didx - stride + i]; }
            for _ in 0..h {
                self.dbuf[didx..][..w].copy_from_slice(&buf[..w]);
                didx += stride;
            }
        }
    }
}

#[allow(unused_variables)]
fn apply_delta4x4(bufs: &mut Buffers, off: usize, stride: usize,
                  deltas: &[u8], topline: bool, first_line: bool) {
    let dst = &mut bufs.dbuf[off..][..4];
    for i in 0..4 { dst[i] = dst[i].wrapping_add(deltas[i]) & 0x7F; }
}

#[allow(unused_variables)]
fn apply_delta4x8(bufs: &mut Buffers, off: usize, stride: usize,
                  deltas: &[u8], topline: bool, first_line: bool) {
    let dst = &mut bufs.dbuf[off..][..stride + 4];
    for i in 0..4 { dst[i + stride] = dst[i].wrapping_add(deltas[i]) & 0x7F; }
    if !topline {
        for i in 0..4 { dst[i] = (dst[i + stride] + dst[i]) >> 1; }
    } else {
        for i in 0..4 { dst[i] =  dst[i + stride]; }
    }
}

#[allow(unused_variables)]
fn apply_delta4x8m11(bufs: &mut Buffers, off: usize, stride: usize,
                     deltas: &[u8], topline: bool, first_line: bool) {
    let dst = &mut bufs.dbuf[off..][..stride + 4];
    for i in 0..4 { dst[i]          = dst[i]         .wrapping_add(deltas[i]) & 0x7F; }
    for i in 0..4 { dst[i + stride] = dst[i + stride].wrapping_add(deltas[i]) & 0x7F; }
}

#[allow(unused_variables)]
fn apply_delta8x8p(bufs: &mut Buffers, off: usize, stride: usize,
                   deltas: &[u8], topline: bool, first_line: bool) {
    let dst = &mut bufs.dbuf[off..][..stride + 8];
    for i in 0..8 { dst[i]          = dst[i]         .wrapping_add(deltas[i >> 1]) & 0x7F; }
    for i in 0..8 { dst[i + stride] = dst[i + stride].wrapping_add(deltas[i >> 1]) & 0x7F; }
}

fn apply_delta8x8i(bufs: &mut Buffers, off: usize, stride: usize,
                   deltas: &[u8], topline: bool, firstline: bool) {
    let dst = &mut bufs.dbuf[off..][..stride + 8];
    if !firstline {
        for i in 0..8 { dst[i + stride] = dst[i     ].wrapping_add(deltas[i >> 1]) & 0x7F; }
    } else {
        for i in 0..8 { dst[i + stride] = dst[i & !1].wrapping_add(deltas[i >> 1]) & 0x7F; }
    }
    if !topline {
        for i in 0..8 { dst[i] = (dst[i + stride] + dst[i]) >> 1; }
    } else {
        for i in 0..8 { dst[i] =  dst[i + stride]; }
    }
}

fn copy_line_top(bufs: &mut Buffers, off: usize, stride: usize, bw: usize, topline: bool) {
    let mut buf: [u8; 8] = [0; 8];
    if !topline {
        let src = &bufs.dbuf[(off - stride)..(off - stride + bw)];
        buf[..bw].copy_from_slice(&src[..bw]);
    } else {
        for i in 0..bw { buf[i] = DEFAULT_PIXEL; }
    }
    let dst = &mut bufs.dbuf[off..][..bw];
    dst.copy_from_slice(&buf[..bw]);
}

fn copy_line_top4x4(bufs: &mut Buffers, off: usize, stride: usize, topline: bool) {
    copy_line_top(bufs, off, stride, 4, topline);
}

fn copy_line_top4x8(bufs: &mut Buffers, off: usize, stride: usize, topline: bool) {
    copy_line_top(bufs, off,          stride, 4, topline);
    copy_line_top(bufs, off + stride, stride, 4, false);
}

fn copy_line_top8x8(bufs: &mut Buffers, off: usize, stride: usize, topline: bool) {
    let mut buf: [u8; 8] = [0; 8];
    if !topline {
        let src = &bufs.dbuf[(off - stride)..(off - stride + 8)];
        for i in 0..8 { buf[i] = src[i & !1]; }
    } else {
        for i in 0..8 { buf[i] = DEFAULT_PIXEL; }
    }
    let dst = &mut bufs.dbuf[off..][..8];
    dst.copy_from_slice(&buf[..8]);
}

fn fill_block8x8(bufs: &mut Buffers, doff: usize, stride: usize, h: usize, topline: bool, firstline: bool) {
    let mut didx = doff;
    let mut buf: [u8; 8] = [0; 8];
    if firstline {
        for i in 0..8 { buf[i] = DEFAULT_PIXEL; }
    } else {
        for i in 0..8 { buf[i] = bufs.dbuf[doff - stride + i]; }
    }
    if topline && !firstline {
        for i in 0..4 { buf[i * 2 + 1] = buf[i * 2]; }
        for i in 0..8 { bufs.dbuf[doff + i] = (bufs.dbuf[doff - stride + i] + buf[i]) >> 1; }
    }

    let start = if !topline { 0 } else { 1 };
    if topline {
        didx += stride;
    }
    for _ in start..h {
        bufs.dbuf[didx..][..8].copy_from_slice(&buf[..8]);
        didx += stride;
    }
}

struct Indeo3Decoder {
    info:       NACodecInfoRef,
    bpos:       u8,
    bbuf:       u8,
    width:      u16,
    height:     u16,
    mvs:        Vec<MV>,
    altquant:   [u8; 16],
    vq_offset:  u8,
    bufs:       Buffers,
    requant_tab: [[u8; 128]; 8],
}

#[derive(Clone,Copy)]
struct IV3Cell {
    x:      u16,
    y:      u16,
    w:      u16,
    h:      u16,
    d:      u8,
    vqt:    bool,
    mv:     Option<MV>,
}

impl IV3Cell {
    fn new(w: u16, h: u16) -> Self {
        IV3Cell { x: 0, y: 0, w, h, d: 20, vqt: false, mv: None }
    }
    fn split_h(&self) -> (Self, Self) {
        let h1 = if self.h > 2 { ((self.h + 2) >> 2) << 1 } else { 1 };
        let h2 = self.h - h1;
        let mut cell1 = *self;
        cell1.h  = h1;
        cell1.d -= 1;
        let mut cell2 = *self;
        cell2.y += h1;
        cell2.h  = h2;
        cell2.d -= 1;
        (cell1, cell2)
    }
    fn split_w(&self, stripw: u16) -> (Self, Self) {
        let w1 = if self.w > stripw {
                if self.w > stripw * 2 { stripw * 2 } else { stripw }
            } else {
                if self.w > 2 { ((self.w + 2) >> 2) << 1 } else { 1 }
            };
        let w2 = self.w - w1;
        let mut cell1 = *self;
        cell1.w  = w1;
        cell1.d -= 1;
        let mut cell2 = *self;
        cell2.x += w1;
        cell2.w  = w2;
        cell2.d -= 1;
        (cell1, cell2)
    }
    fn no_mv(&self) -> bool { self.mv.is_none() }
}

struct CellDecParams {
    tab:    [usize; 2],
    bw:     u16,
    bh:     u16,
    swap_q: [bool; 2],
    hq:     bool,
    apply_delta:   fn (&mut Buffers, usize, usize, &[u8], bool, bool),
    copy_line_top: fn (&mut Buffers, usize, usize, bool),
}

const FRMH_TAG: u32 = ((b'F' as u32) << 24) | ((b'R' as u32) << 16)
                     | ((b'M' as u32) << 8) | (b'H' as u32);

const H_SPLIT: u8 = 0;
const V_SPLIT: u8 = 1;
const SKIP_OR_TREE: u8 = 2;

impl Indeo3Decoder {
    fn new() -> Self {
        const REQUANT_OFF: [i32; 8] = [ 0, 1, 0, 4, 4, 1, 0, 1 ];

        let dummy_info = NACodecInfo::new_dummy();

        let mut requant_tab = [[0u8; 128]; 8];
        for i in 0..8 {
            let step = (i as i32) + 2;
            let start = if (i == 3) || (i == 4) { -3 } else { step / 2 };
            let mut last = 0;
            for j in 0..128 {
                requant_tab[i][j] = (((j as i32) + start) / step * step + REQUANT_OFF[i]) as u8;
                if requant_tab[i][j] < 128 {
                    last = requant_tab[i][j];
                } else {
                    requant_tab[i][j] = last;
                }
            }
        }
        requant_tab[1][7]   =  10;
        requant_tab[1][119] = 118;
        requant_tab[1][120] = 118;
        requant_tab[4][8]   =  10;

        Indeo3Decoder { info: dummy_info, bpos: 0, bbuf: 0, width: 0, height: 0,
                        mvs: Vec::new(), altquant: [0; 16],
                        vq_offset: 0, bufs: Buffers::new(), requant_tab }
    }

    fn br_reset(&mut self) {
        self.bpos = 0;
        self.bbuf = 0;
    }

    fn get_2bits(&mut self, br: &mut ByteReader) -> DecoderResult<u8> {
        if self.bpos == 0 {
            self.bbuf = br.read_byte()?;
            self.bpos = 8;
        }
        self.bpos -= 2;
        Ok((self.bbuf >> self.bpos) & 0x3)
    }

    #[allow(clippy::cognitive_complexity)]
    fn decode_cell_data(&mut self, br: &mut ByteReader, cell: IV3Cell,
                        off: usize, stride: usize, params: CellDecParams, vq_idx: u8) -> DecoderResult<()> {
        let blk_w = cell.w * 4 / params.bw;
        let blk_h = cell.h * 4 / params.bh;
        let scale: usize = if params.bh == 4 { 1 } else { 2 };

        validate!((((cell.w * 4) % params.bw) == 0) && (((cell.h * 4) % params.bh) == 0));

        let mut run_blocks = 0;
        let mut run_skip   = false;

        let mut didx: usize = ((cell.x*4) as usize) + ((cell.y * 4) as usize) * stride + off;
        let mut sidx: usize;

        if cell.no_mv() {
            sidx = 0;
        } else {
            let mv = cell.mv.unwrap();
            let mx = i16::from(mv.x);
            let my = i16::from(mv.y);
            let l = (cell.x as i16) * 4 + mx;
            let t = (cell.y as i16) * 4 + my;
            let r = ((cell.x + cell.w) as i16) * 4 + mx;
            let b = ((cell.y + cell.h) as i16) * 4 + my;
            validate!(l >= 0);
            validate!(t >= 0);
            validate!(r <= (self.width as i16));
            validate!(b <= (self.height as i16));
            sidx = (l as usize) + (t as usize) * stride + off;
        }
        if vq_idx >= 8 {
            let requant_tab = &self.requant_tab[(vq_idx & 7) as usize];
            if cell.no_mv() {
                if cell.y > 0 {
                    for x in 0..(cell.w as usize) * 4 {
                        self.bufs.dbuf[didx + x - stride] = requant_tab[self.bufs.dbuf[didx + x - stride] as usize];
                    }
                }
            } else {
                for x in 0..(cell.w as usize) * 4 {
                    self.bufs.sbuf[sidx + x] = requant_tab[self.bufs.sbuf[sidx + x] as usize];
                }
            }
        }
        for y in 0..blk_h {
            let mut xoff: usize = 0;
            for _ in 0..blk_w {
                if run_blocks > 0 {
                    if !run_skip || !cell.no_mv() {
                        if !(params.bw == 8 && cell.no_mv()) {
                            if !cell.no_mv() {
                                self.bufs.copy_block(didx + xoff, sidx + xoff, stride,
                                                     params.bw as usize, params.bh as usize);
                            } else {
                                self.bufs.fill_block(didx + xoff, stride,
                                                     params.bw as usize, params.bh as usize,
                                                     (cell.y == 0) && (y == 0));
                            }
                        } else {
                            fill_block8x8(&mut self.bufs,
                                          didx + xoff, stride, 8,
                                          y == 0, (cell.y == 0) && (y == 0));
                        }
                    }
                    run_blocks -= 1;
                } else {
                    let mut line: usize = 0;
                    while line < 4 {
                        let c = br.read_byte()?;
                        if c < 0xF8 {
                            let delta_tab = if params.hq {
                                                IVI3_DELTA_CBS[params.tab[line & 1]]
                                            } else {
                                                IVI3_DELTA_CBS[params.tab[1]]
                                            };
                            let mut idx1;
                            let mut idx2;
                            if (c as usize) < delta_tab.data.len()/2 {
                                idx1 = br.read_byte()? as usize;
                                validate!(idx1 < delta_tab.data.len() / 2);
                                idx2 = c as usize;
                            } else {
                                let tmp = (c as usize) - delta_tab.data.len()/2;
                                idx1 = tmp / (delta_tab.quad_radix as usize);
                                idx2 = tmp % (delta_tab.quad_radix as usize);
                                if params.swap_q[line & 1] {
                                    mem::swap(&mut idx1, &mut idx2);
                                }
                            }
                            let deltas: [u8; 4] = [delta_tab.data[idx1 * 2]     as u8,
                                                   delta_tab.data[idx1 * 2 + 1] as u8,
                                                   delta_tab.data[idx2 * 2 + 0]     as u8,
                                                   delta_tab.data[idx2 * 2 + 1] as u8];
                            let topline = (cell.y == 0) && (y == 0) && (line == 0);
                            let first_line = (y == 0) && (line == 0);
                            if cell.no_mv() {
                                (params.copy_line_top)(&mut self.bufs,
                                                       didx + xoff + line * scale * stride,
                                                       stride, topline);
                            } else {
                                self.bufs.copy_block(didx + xoff + line * scale * stride,
                                                     sidx + xoff + line * scale * stride,
                                                     stride, params.bw as usize, scale);
                            }
                            (params.apply_delta)(&mut self.bufs,
                                                 didx + xoff + line * scale * stride,
                                                 stride, &deltas, topline, first_line);
                            line += 1;
                        } else {
                            let mut tocopy: usize = 0;
                            let mut do_copy = true;
                            if c == 0xF8 { return Err(DecoderError::InvalidData); }
                            if c == 0xF9 {
                                run_blocks = 1;
                                run_skip   = true;
                                validate!(line == 0);
                                tocopy = 4;
                                do_copy = !cell.no_mv();
                            }
                            if c == 0xFA {
                                validate!(line == 0);
                                tocopy = 4;
                                do_copy = !cell.no_mv();
                            }
                            if c == 0xFB {
                                let c = br.read_byte()?;
                                validate!((c < 64) && ((c & 0x1F) != 0));
                                run_blocks = (c & 0x1F) - 1;
                                run_skip   = (c & 0x20) != 0;
                                tocopy = 4 - line;
                                if params.bw == 4 && cell.no_mv() && run_skip {
                                    do_copy = false;
                                }
                            }
                            if c == 0xFC {
                                run_skip = false;
                                run_blocks = 1;
                                tocopy = 4 - line;
                            }
                            if c >= 0xFD {
                                let nl = 257 - i16::from(c) - (line as i16);
                                validate!(nl > 0);
                                tocopy = nl as usize;
                            }
                            if do_copy {
                                if !(params.bh == 8 && cell.no_mv()) {
                                    if !cell.no_mv() {
                                        self.bufs.copy_block(didx + xoff + line * scale * stride,
                                                             sidx + xoff + line * scale * stride,
                                                             stride, params.bw as usize,
                                                             tocopy * scale);
                                    } else {
                                        self.bufs.fill_block(didx + xoff + line * scale * stride,
                                                             stride, params.bw as usize,
                                                             tocopy * scale,
                                                             (cell.y == 0) && (y == 0) && (line == 0));
                                    }
                                } else {
                                    fill_block8x8(&mut self.bufs,
                                                  didx + xoff + line * 2 * stride,
                                                  stride, tocopy * 2,
                                                  (y == 0) && (line == 0),
                                                  (cell.y == 0) && (y == 0) && (line == 0));
                                }
                            }
                            line += tocopy;
                        }
                    }
                }
                xoff += params.bw as usize;
            }
            didx += stride * (params.bh as usize);
            sidx += stride * (params.bh as usize);
        }
        Ok(())
    }

    fn copy_cell(&mut self, cell: IV3Cell, off: usize, stride: usize) -> DecoderResult<()> {
        if cell.no_mv() { return Err(DecoderError::InvalidData); }
        let mv = cell.mv.unwrap();
        let mx = i16::from(mv.x);
        let my = i16::from(mv.y);
        let l = (cell.x as i16) * 4 + mx;
        let t = (cell.y as i16) * 4 + my;
        let r = ((cell.x + cell.w) as i16) * 4 + mx;
        let b = ((cell.y + cell.h) as i16) * 4 + my;
        validate!(l >= 0);
        validate!(t >= 0);
        validate!(r <= (self.width as i16));
        validate!(b <= (self.height as i16));
        let sidx: usize = off + (l as usize) + (t as usize) * stride;
        let didx: usize = off + ((cell.x * 4) as usize) + ((cell.y * 4) as usize) * stride;
        self.bufs.copy_block(didx, sidx, stride, (cell.w * 4) as usize, (cell.h * 4) as usize);
        Ok(())
    }

    fn decode_cell(&mut self, br: &mut ByteReader, cell: IV3Cell, off: usize,
                   stride: usize, intra: bool) -> DecoderResult<()> {
        let code = br.read_byte()?;
        let mode   = code >> 4;
        let vq_idx = code & 0xF;

        let mut idx1: usize = vq_idx as usize;
        let mut idx2: usize = vq_idx as usize;
        if (mode == 1) || (mode == 4) {
            let c = self.altquant[vq_idx as usize];
            idx1 = (c >> 4) as usize;
            idx2 = (c & 0xF) as usize;
        } else {
            idx1 += self.vq_offset as usize;
            idx2 += self.vq_offset as usize;
        }
        validate!((idx1 < 24) && (idx2 < 24));

        let mut cp = CellDecParams {
                         tab: [idx2, idx1],
                         bw: 0, bh: 0,
                         swap_q: [idx2 >= 16, idx1 >= 16],
                         hq: false,
                         apply_delta:   apply_delta4x4,
                         copy_line_top: copy_line_top4x4,
                     };
        if (mode == 0) || (mode == 1) {
            cp.bw = 4;
            cp.bh = 4;
            cp.hq = true;
        } else if (mode == 3) || (mode == 4) {
            if !cell.no_mv() { return Err(DecoderError::InvalidData); }
            cp.bw = 4;
            cp.bh = 8;
            cp.hq = true;
            cp.apply_delta   = apply_delta4x8;
            cp.copy_line_top = copy_line_top4x8;
        } else if mode == 10 {
            if !cell.no_mv() {
                validate!(!intra);
                cp.apply_delta = apply_delta8x8p;
            } else {
                cp.apply_delta = apply_delta8x8i;
            }
            cp.bw = 8;
            cp.bh = 8;
            cp.copy_line_top = copy_line_top8x8;
        } else if mode == 11 {
            if cell.no_mv() { return Err(DecoderError::InvalidData); }
            validate!(!intra);
            cp.bw = 4;
            cp.bh = 8;
            cp.apply_delta = apply_delta4x8m11;
            cp.copy_line_top = copy_line_top4x8;
        } else {
            return Err(DecoderError::InvalidData);
        }
        self.decode_cell_data(br, cell, off, stride, cp, vq_idx)
    }

    fn parse_tree(&mut self, br: &mut ByteReader, cell: IV3Cell, off: usize,
                  stride: usize, stripw: u16, intra: bool) -> DecoderResult<()> {
        let op = self.get_2bits(br)?;
        if op == H_SPLIT {
            validate!(cell.h > 1);
            validate!(cell.d > 0);
            let (cell1, cell2) = cell.split_h();
            self.parse_tree(br, cell1, off, stride, stripw, intra)?;
            self.parse_tree(br, cell2, off, stride, stripw, intra)?;
            Ok(())
        } else if op == V_SPLIT {
            validate!(cell.w > 1);
            validate!(cell.d > 0);
            let (cell1, cell2) = cell.split_w(stripw);
            self.parse_tree(br, cell1, off, stride, stripw, intra)?;
            self.parse_tree(br, cell2, off, stride, stripw, intra)?;
            Ok(())
        } else if op == SKIP_OR_TREE {
            if !cell.vqt {
                let mut newcell = cell;
                newcell.vqt = true;
                newcell.d   -= 1;
                self.parse_tree(br, newcell, off, stride, stripw, intra)
            } else {
                validate!(!intra);
                let code = self.get_2bits(br)?;
                validate!(code < 2);
                if code == 1 { return Err(DecoderError::NotImplemented); }
                self.copy_cell(cell, off, stride)
            }
        } else {
            if !cell.vqt {
                let mut newcell = cell;
                newcell.vqt = true;
                newcell.d  -= 1;
                let mv_idx = br.read_byte()? as usize;
                validate!(mv_idx < self.mvs.len());
                newcell.mv = Some(self.mvs[mv_idx]);
                self.parse_tree(br, newcell, off, stride, stripw, intra)
            } else {
                self.decode_cell(br, cell, off, stride, intra)
            }
        }
    }

    fn decode_plane_intra(&mut self, br: &mut ByteReader, planeno: usize,
                          start: u64, end: u64) -> DecoderResult<()> {
        let offs   = self.bufs.get_offset(planeno);
        let stride = self.bufs.get_stride(planeno);
        br.seek(SeekFrom::Start(start))?;

        let nvec = br.read_u32le()?;
        validate!(nvec == 0); // for intra there should be no mc_vecs
        self.mvs.clear();
        for _ in 0..nvec {
            let x = br.read_byte()? as i8;
            let y = br.read_byte()? as i8;
            self.mvs.push(MV{ x, y });
        }

        let (cellwidth, cellheight) = if planeno == 0 {
                (self.bufs.width >> 2, self.bufs.height >> 2)
            } else {
                (((self.bufs.width >> 2) + 3) >> 2, ((self.bufs.height >> 2) + 3) >> 2)
            };
        let cell = IV3Cell::new(cellwidth as u16, cellheight as u16);
        self.br_reset();
        self.parse_tree(br, cell, offs, stride, if planeno > 0 { 10 } else { 40 }, true)?;
        validate!(br.tell() <= end);
        Ok(())
    }

    fn decode_plane_inter(&mut self, br: &mut ByteReader, planeno: usize,
                          start: u64, end: u64) -> DecoderResult<()> {
        let offs   = self.bufs.get_offset(planeno);
        let stride = self.bufs.get_stride(planeno);
        br.seek(SeekFrom::Start(start))?;

        let nvec = br.read_u32le()?;
        validate!(nvec <= 256); // for intra there should be no mc_vecs
        self.mvs.clear();
        for _ in 0..nvec {
            let y = br.read_byte()? as i8;
            let x = br.read_byte()? as i8;
            self.mvs.push(MV{ x, y });
        }

        let (cellwidth, cellheight) = if planeno == 0 {
                (self.bufs.width >> 2, self.bufs.height >> 2)
            } else {
                (((self.bufs.width >> 2) + 3) >> 2, ((self.bufs.height >> 2) + 3) >> 2)
            };
        let cell = IV3Cell::new(cellwidth as u16, cellheight as u16);
        self.br_reset();
        self.parse_tree(br, cell, offs, stride, if planeno > 0 { 10 } else { 40 }, false)?;
        validate!(br.tell() <= end);
        Ok(())
    }
}

const FLAG_KEYFRAME: u16 = 1 << 2;
const FLAG_NONREF:   u16 = 1 << 8;

impl NADecoder for Indeo3Decoder {
    fn init(&mut self, _supp: &mut NADecoderSupport, info: NACodecInfoRef) -> DecoderResult<()> {
        if let NACodecTypeInfo::Video(vinfo) = info.get_properties() {
            let w = vinfo.get_width();
            let h = vinfo.get_height();
            let fmt = formats::YUV410_FORMAT;
            let myinfo = NACodecTypeInfo::Video(NAVideoInfo::new(w, h, false, fmt));
            self.info = NACodecInfo::new_ref(info.get_name(), myinfo, info.get_extradata()).into_ref();
            self.bufs.reset();
            Ok(())
        } else {
            Err(DecoderError::InvalidData)
        }
    }
    fn decode(&mut self, _supp: &mut NADecoderSupport, pkt: &NAPacket) -> DecoderResult<NAFrameRef> {
        let src = pkt.get_buffer();
        let mut mr = MemoryReader::new_read(&src);
        let mut br = ByteReader::new(&mut mr);
        let frameno = br.read_u32le()?;
        let hdr_2   = br.read_u32le()?;
        let check   = br.read_u32le()?;
        let size    = br.read_u32le()?;

        let data_start = br.tell();

        if (frameno ^ hdr_2 ^ size ^ FRMH_TAG) != check {
            return Err(DecoderError::InvalidData);
        }
        if i64::from(size) > br.left() { return Err(DecoderError::InvalidData); }
        let ver     = br.read_u16le()?;
        if ver != 32 { return Err(DecoderError::NotImplemented); }
        let flags   = br.read_u16le()?;
        let size2   = br.read_u32le()?;
        if size2 == 0x80 {
            let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), NABufferType::None);
            frm.set_keyframe(false);
            frm.set_frame_type(FrameType::Skip);
            return Ok(frm.into_ref());
        }
        validate!(((size2 + 7) >> 3) <= size);
        let cb      = br.read_byte()?;
        self.vq_offset = cb;
        br.read_skip(3)?;
        let height  = br.read_u16le()?;
        let width   = br.read_u16le()?;
        validate!((width  >= 16) && (width  <= 640));
        validate!((height >= 16) && (height <= 640));
        validate!(((width & 3) == 0) && ((height & 3) == 0));
        let vinfo;
        if (self.bufs.width != (width as usize)) || (self.bufs.height != (height as usize)) {
            self.bufs.alloc(width as usize, height as usize);
            vinfo = NAVideoInfo::new(width as usize, height as usize, false, formats::YUV410_FORMAT);
        } else {
            vinfo = self.info.get_properties().get_video_info().unwrap();
        }
        self.width  = width;
        self.height = height;

        let yoff    = br.read_u32le()?;
        let uoff    = br.read_u32le()?;
        let voff    = br.read_u32le()?;
        if yoff > size { return Err(DecoderError::InvalidData); }
        if uoff > size { return Err(DecoderError::InvalidData); }
        if voff > size { return Err(DecoderError::InvalidData); }

        br.read_skip(4)?;
        br.read_buf(&mut self.altquant)?;

        let mut yend = src.len() as u32;//size;
        if (uoff < yend) && (uoff > yoff) { yend = uoff; }
        if (voff < yend) && (voff > yoff) { yend = voff; }
        let mut uend = size;
        if (yoff < uend) && (yoff > uoff) { uend = yoff; }
        if (voff < uend) && (voff > uoff) { uend = voff; }
        let mut vend = size;
        if (yoff < vend) && (yoff > voff) { vend = yoff; }
        if (uoff < vend) && (uoff > voff) { vend = uoff; }

        let intraframe = (flags & FLAG_KEYFRAME) != 0;
        let bufinfo = alloc_video_buffer(vinfo, 4)?;
        let mut buf = bufinfo.get_vbuf().unwrap();
        let ystart  = data_start + u64::from(yoff);
        let ustart  = data_start + u64::from(uoff);
        let vstart  = data_start + u64::from(voff);
        let yendpos = data_start + u64::from(yend);
        let uendpos = data_start + u64::from(uend);
        let vendpos = data_start + u64::from(vend);
        if intraframe {
            self.decode_plane_intra(&mut br, 0, ystart, yendpos)?;
            self.decode_plane_intra(&mut br, 1, vstart, vendpos)?;
            self.decode_plane_intra(&mut br, 2, ustart, uendpos)?;
        } else {
            self.decode_plane_inter(&mut br, 0, ystart, yendpos)?;
            self.decode_plane_inter(&mut br, 1, vstart, vendpos)?;
            self.decode_plane_inter(&mut br, 2, ustart, uendpos)?;
        }
        self.bufs.fill_framebuf(&mut buf);
        if (flags & FLAG_NONREF) == 0 { self.bufs.flip(); }
        let mut frm = NAFrame::new_from_pkt(pkt, self.info.clone(), bufinfo);
        frm.set_keyframe(intraframe);
        frm.set_frame_type(if intraframe { FrameType::I } else { FrameType::P });
        Ok(frm.into_ref())
    }
    fn flush(&mut self) {
        self.bufs.reset();
    }
}

impl NAOptionHandler for Indeo3Decoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { &[] }
    fn set_options(&mut self, _options: &[NAOption]) { }
    fn query_option_value(&self, _name: &str) -> Option<NAValue> { None }
}

pub fn get_decoder() -> Box<dyn NADecoder + Send> {
    Box::new(Indeo3Decoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::RegisteredDecoders;
    use nihav_core::demuxers::RegisteredDemuxers;
    use nihav_codec_support::test::dec_video::*;
    use crate::indeo_register_all_decoders;
    use nihav_commonfmt::generic_register_all_demuxers;
    #[test]
    fn test_indeo3() {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        indeo_register_all_decoders(&mut dec_reg);

        // sample: https://samples.mplayerhq.hu/V-codecs/IV32/iv32_example.avi
        test_decoding("avi", "indeo3", "assets/Indeo/iv32_example.avi", Some(10),
                      &dmx_reg, &dec_reg, ExpectedTestResult::MD5Frames(vec![
                            [0x90be698e, 0x326db071, 0x08e8c6a5, 0x39349acc],
                            [0x25d677fc, 0x63f96aaa, 0xd412ca98, 0x61416313],
                            [0xc4368250, 0x63e7b6bc, 0xffcff950, 0x11f13239],
                            [0x7e869758, 0x027abc2e, 0x25204bca, 0x93fbaa03],
                            [0x5a1e822c, 0x2b1a4cd5, 0x72059843, 0xe5689ad1],
                            [0x3a971cce, 0x5ec22135, 0x1a45f802, 0x0f5f9264],
                            [0x0a65f782, 0xd8767cf3, 0x878b4b8d, 0xfc94c88b],
                            [0x4ac70139, 0x3300eac1, 0xba84b068, 0x47f5ff29],
                            [0x3e8c8ec4, 0x9421b38c, 0x580abbbd, 0x92792d19],
                            [0x9096ee9b, 0x8dd9fb14, 0x981e31e3, 0x3ffd7d29],
                            [0x22dc71ec, 0x3d8f6f7e, 0x1a198982, 0x41d17ecc]]));
    }
}
