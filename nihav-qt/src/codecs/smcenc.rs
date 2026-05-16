use std::convert::TryInto;
use nihav_core::codecs::*;
use nihav_core::io::byteio::*;
use nihav_codec_support::vq::*;

#[derive(Clone,Copy,Default,PartialEq)]
enum Strategy {
    #[default]
    Lossless,
    Max2Clr,
    Max4Clr,
    Max8Clr,
}

#[derive(Clone,PartialEq)]
enum Opcode {
    None,
    Skip(usize),
    Repeat(usize),
    RepeatTwo(usize),
    Fill(usize, u8),
    TwoColours(usize, [u8; 2], [u16; 16]),
    FourColours(usize, [u8; 4], [u32; 16]),
    EightColours(usize, [u8; 8], [[u16; 3]; 16]),
    Raw(usize),
}

impl Opcode {
    fn is_skip(&self) -> bool { matches!(*self, Opcode::Skip(_)) }
}

struct EncoderHelper {
    pairs:      [[u8; 2]; 256],
    ppos:       usize,
    pcount:     usize,
    quads:      [[u8; 4]; 256],
    qpos:       usize,
    qcount:     usize,
    octets:     [[u8; 8]; 256],
    opos:       usize,
    ocount:     usize,
}

impl Default for EncoderHelper {
    fn default() -> Self {
        let tmp = std::mem::MaybeUninit::<Self>::zeroed();
        unsafe { tmp.assume_init() }
    }
}

impl EncoderHelper {
    fn reset(&mut self) {
        *self = EncoderHelper::default();
    }
    fn write_token(&mut self, dbuf: &mut Vec<u8>, token: &Opcode, blocks: &[[u8; 16]], pos: usize) {
        match *token {
            Opcode::None => {},
            Opcode::Skip(skip_len) => {
                let mut to_write = skip_len;
                while to_write > 16 {
                    let len = to_write.min(256);
                    dbuf.push(0x10);
                    dbuf.push((len - 1) as u8);
                    to_write -= len;
                }
                if to_write > 0 {
                    dbuf.push((to_write - 1) as u8);
                }
            },
            Opcode::Repeat(repeat_len) => {
                let mut to_write = repeat_len;
                while to_write > 16 {
                    let len = to_write.min(256);
                    dbuf.push(0x30);
                    dbuf.push((len - 1) as u8);
                    to_write -= len;
                }
                if to_write > 0 {
                    dbuf.push(0x20 | ((to_write - 1) as u8));
                }
            },
            Opcode::RepeatTwo(repeat_len) => {
                let mut to_write = repeat_len;
                while to_write > 16 {
                    let len = to_write.min(256);
                    dbuf.push(0x50);
                    dbuf.push((len - 1) as u8);
                    to_write -= len;
                }
                if to_write > 0 {
                    dbuf.push(0x40 | ((to_write - 1) as u8));
                }
            },
            Opcode::Fill(run_len, clr) => {
                let mut to_write = run_len;
                while to_write > 16 {
                    let len = to_write.min(256);
                    dbuf.push(0x70);
                    dbuf.push((len - 1) as u8);
                    dbuf.push(clr);
                    to_write -= len;
                }
                if to_write > 0 {
                    dbuf.push(0x60 | ((to_write - 1) as u8));
                    dbuf.push(clr);
                }
            },
            Opcode::TwoColours(count, pair, flags) => {
                if let Some(idx) = self.pairs.iter().take(self.pcount).position(|pp| pp == &pair) {
                    dbuf.push(0x90 | ((count - 1) as u8));
                    dbuf.push(idx as u8);
                } else {
                    self.pairs[self.ppos] = pair;
                    self.ppos = (self.ppos + 1) & 0xFF;
                    if self.pcount < 256 {
                        self.pcount += 1;
                    }
                    dbuf.push(0x80 | ((count - 1) as u8));
                    dbuf.extend_from_slice(&pair);
                }
                let mut tmp = [0; 2];
                for &flg in flags[..count].iter() {
                    let _ = write_u16be(&mut tmp, flg);
                    dbuf.extend_from_slice(&tmp);
                }
            },
            Opcode::FourColours(count, quad, flags) => {
                if let Some(idx) = self.quads.iter().take(self.qcount).position(|qq| qq == &quad) {
                    dbuf.push(0xB0 | ((count - 1) as u8));
                    dbuf.push(idx as u8);
                } else {
                    self.quads[self.qpos] = quad;
                    self.qpos = (self.qpos + 1) & 0xFF;
                    if self.qcount < 256 {
                        self.qcount += 1;
                    }
                    dbuf.push(0xA0 | ((count - 1) as u8));
                    dbuf.extend_from_slice(&quad);
                }
                let mut tmp = [0; 4];
                for &flg in flags[..count].iter() {
                    let _ = write_u32be(&mut tmp, flg);
                    dbuf.extend_from_slice(&tmp);
                }
            },
            Opcode::EightColours(count, oct, flags) => {
                if let Some(idx) = self.octets.iter().take(self.ocount).position(|oo| oo == &oct) {
                    dbuf.push(0xD0 | ((count - 1) as u8));
                    dbuf.push(idx as u8);
                } else {
                    self.octets[self.opos] = oct;
                    self.opos = (self.opos + 1) & 0xFF;
                    if self.ocount < 256 {
                        self.ocount += 1;
                    }
                    dbuf.push(0xC0 | ((count - 1) as u8));
                    dbuf.extend_from_slice(&oct);
                }
                let mut tmp = [0; 6];
                for flg in flags[..count].iter() {
                    let _ = write_u16be(&mut tmp[..2], flg[0]);
                    let _ = write_u16be(&mut tmp[2..], flg[1]);
                    let _ = write_u16be(&mut tmp[4..], flg[2]);
                    dbuf.extend_from_slice(&tmp);
                }
            },
            Opcode::Raw(count) => {
                let mut to_write = count;
                let mut src_pos = pos - count;
                while to_write > 0 {
                    let len = to_write.min(16);
                    dbuf.push(0xE0 | ((len - 1) as u8));
                    for blk in blocks[src_pos..][..len].iter() {
                        dbuf.extend_from_slice(blk);
                    }
                    to_write -= len;
                    src_pos += len;
                }
            },
        }
    }
}

fn blk_2clr(block: &[u8; 16], clrs: &[u8; 2]) -> u16 {
    let mut flags = 0;
    for &pix in block.iter() {
        flags <<= 1;
        if pix == clrs[1] {
            flags |= 1;
        }
    }
    flags
}

fn blk_4clr(block: &[u8; 16], clrs: &[u8; 4]) -> u32 {
    let mut flags = 0;
    for pix in block.iter() {
        flags <<= 2;
        flags |= clrs.iter().position(|c| c == pix).unwrap_or(0) as u32;
    }
    flags
}

fn blk_8clr(block: &[u8; 16], clrs: &[u8; 8]) -> [u16; 3] {
    let mut flags = [0; 4];
    for (flag, row) in flags.iter_mut().zip(block.chunks_exact(4)) {
        for pix in row.iter() {
            *flag <<= 3;
            *flag |= clrs.iter().position(|c| c == pix).unwrap_or(0) as u16;
        }
    }
    [(flags[0] << 4) | (flags[3] >> 8),
     (flags[1] << 4) | ((flags[3] >> 4) & 0xF),
     (flags[2] << 4) | (flags[3] & 0xF)]
}

fn calc_clrs(clrs: &mut [u8; 24], mut ccount: usize, blk: &[u8; 16]) -> usize {
    for clr in blk.iter() {
        if !clrs[..ccount].contains(clr) {
            clrs[ccount] = *clr;
            ccount += 1;
        }
    }
    if (2..=8).contains(&ccount) {
        clrs[..ccount].sort();
    }
    ccount
}

#[derive(Clone,Copy,Default,PartialEq)]
struct Colour([u8; 3]);

impl VQElement for Colour {
    fn dist(&self, rval: Self) -> u32 {
        let rd = u32::from(self.0[0].abs_diff(rval.0[0]));
        let gd = u32::from(self.0[1].abs_diff(rval.0[1]));
        let bd = u32::from(self.0[2].abs_diff(rval.0[2]));
        rd * rd + gd * gd + bd * bd
    }
    fn min_cw() -> Self { Colour([0x00; 3]) }
    fn max_cw() -> Self { Colour([0xFF; 3]) }
    fn min(&self, rval: Self) -> Self {
        Colour([self.0[0].min(rval.0[0]),
                self.0[1].min(rval.0[1]),
                self.0[2].min(rval.0[2])])
    }
    fn max(&self, rval: Self) -> Self {
        Colour([self.0[0].max(rval.0[0]),
                self.0[1].max(rval.0[1]),
                self.0[2].max(rval.0[2])])
    }
    fn num_components() -> usize { 3 }
    fn sort_by_component(arr: &mut [Self], component: usize) {
        arr.sort_unstable_by(|a, b| a.0[component].cmp(&b.0[component]));
    }
    fn max_dist_component(min: &Self, max: &Self) -> usize {
        let rd = u32::from(min.0[0].abs_diff(max.0[0]));
        let gd = u32::from(min.0[1].abs_diff(max.0[1]));
        let bd = u32::from(min.0[2].abs_diff(max.0[2]));
        if gd >= rd && gd >= bd {
            1
        } else if rd >= gd && rd >= bd {
            0
        } else if bd >= rd && bd >= gd {
            2
        } else {
            1
        }
    }
}

#[derive(Default)]
struct ColourSum {
    clr:    [u16; 3],
    tot:    u16,
}

impl VQElementSum<Colour> for ColourSum {
    fn zero() -> Self { Self::default() }
    fn add(&mut self, rval: Colour, count: u64) {
        for (dst, &src) in self.clr.iter_mut().zip(rval.0.iter()) {
            *dst += u16::from(src) * (count as u16);
        }
        self.tot += count as u16;
    }
    fn get_centroid(&self) -> Colour {
        if self.tot > 0 {
            Colour([(self.clr[0] / self.tot) as u8,
                    (self.clr[1] / self.tot) as u8,
                    (self.clr[2] / self.tot) as u8])
        } else {
            Colour::default()
        }
    }
}

fn pixel_dist(pix: &Colour, pal: &[u8]) -> u32 {
    pal.iter().zip(pix.0.iter()).fold(0u32,
        |acc, (&a, &b)| acc + u32::from(a.abs_diff(b)) * u32::from(a.abs_diff(b)))
}

fn lossy_block(blk: &[u8; 16], pal: &[u8; 1024], nclrs: usize, dst_clrs: &mut [u8; 8], dst_idx: &mut [u8; 16]) {
    let mut pixels = [Colour::default(); 16];
    for (dst, &src) in pixels.iter_mut().zip(blk.iter()) {
        let clr = &pal[usize::from(src) * 4..][..3];
        *dst = Colour([clr[0], clr[1], clr[2]]);
    }
    let mut ppal = [Colour::default(); 8];
    quantise_median_cut::<Colour, ColourSum>(&pixels, &mut ppal[..nclrs]);
    for (dst, src) in dst_clrs.iter_mut().zip(ppal.iter_mut()) {
        let mut best_dist = u32::MAX;
        let mut best_idx = 0;
        for (i, palclr) in pal.chunks_exact(4).enumerate() {
            let dist = pixel_dist(src, palclr);
            if dist < best_dist {
                best_dist = dist;
                best_idx = i;
                if dist == 0 {
                    break;
                }
            }
        }
        *dst = best_idx as u8;
        *src = Colour([pal[best_idx * 4], pal[best_idx * 4 + 1], pal[best_idx * 4 + 2]]);
    }
    for i in 0..nclrs {
        for j in i + 1..nclrs {
            if dst_clrs[i] > dst_clrs[j] {
                dst_clrs.swap(i, j);
                ppal.swap(i, j);
            }
        }
    }

    for (dst, pix) in dst_idx.iter_mut().zip(pixels.iter()) {
        let mut best_dist = u32::MAX;
        let mut best_idx = 0;
        for (i, refclr) in ppal.iter().take(nclrs).enumerate() {
            let dist = pixel_dist(pix, &refclr.0);
            if dist < best_dist {
                best_dist = dist;
                best_idx = i;
                if dist == 0 {
                    break;
                }
            }
        }
        *dst = best_idx as u8;
    }
}

struct SeanEncoder {
    stream:     Option<NAStreamRef>,
    pkt:        Option<NAPacket>,
    key_int:    u8,
    frm_no:     u8,
    lpal:       Arc<[u8; 1024]>,
    last_frm:   Vec<[u8; 16]>,
    cur_frm:    Vec<[u8; 16]>,
    vinfo:      NAVideoInfo,
    mode:       Strategy,
    helper:     EncoderHelper,
}

impl SeanEncoder {
    fn new() -> Self {
        Self {
            stream:     None,
            pkt:        None,
            key_int:    25,
            frm_no:     254,
            lpal:       Arc::new([0; 1024]),
            last_frm:   Vec::new(),
            cur_frm:    Vec::new(),
            vinfo:      NAVideoInfo::new(0, 0, false, PAL8_FORMAT),
            mode:       Strategy::default(),
            helper:     EncoderHelper::default(),
        }
    }
    fn load_frame(&mut self, buf: NAVideoBufferRef<u8>) {
        let (w, h) = buf.get_dimensions(0);
        let stride = buf.get_stride(0);
        let src = buf.get_data();

        self.cur_frm.clear();
        let mut blk = [0; 16];
        for strip in src.chunks_exact(stride * 4).take(h / 4) {
            for x in (0..w).step_by(4) {
                for (row, line) in blk.chunks_exact_mut(4).zip(strip.chunks_exact(stride)) {
                    row.copy_from_slice(&line[x..][..4]);
                }
                self.cur_frm.push(blk);
            }
        }
    }
    fn encode_frame(&mut self, dbuf: &mut Vec<u8>, force_intra: bool) -> bool {
        let mut is_intra = true;

        let mut opcode = Opcode::None;
        self.helper.reset();
        let mut clrs = [0; 24];
        let mut clrs2 = [0; 24];
        let mut lookahead = false;
        let mut lossy_clrs = [0; 8];
        let mut lossy_idx = [0; 16];
        for (blk_no, (block, pblock)) in self.cur_frm.iter().zip(self.last_frm.iter_mut()).enumerate() {
            if lookahead {
                lookahead = false;
                pblock.copy_from_slice(block);
                continue;
            }

            if !force_intra && block == pblock {
                if let Opcode::Skip(ref mut scount) = opcode {
                    *scount += 1;
                } else {
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    opcode = Opcode::Skip(1);
                }
                is_intra = false;
                continue;
            }

            pblock.copy_from_slice(block);

            let has_next_block = blk_no < self.cur_frm.len() - 1;

            if let Opcode::Fill(ref mut count, fillclr) = opcode {
                if block == &[fillclr; 16] {
                    *count += 1;
                    continue;
                }
            }

            if !opcode.is_skip() && blk_no > 0 && block == &self.cur_frm[blk_no - 1] {
                if let Opcode::Repeat(ref mut count) = opcode {
                    *count += 1;
                    continue;
                }
                self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                opcode = Opcode::Repeat(1);
                continue;
            }

            if !opcode.is_skip() && blk_no > 1 && has_next_block && block == &self.cur_frm[blk_no - 2] && self.cur_frm[blk_no - 1] == self.cur_frm[blk_no + 1] {
                if let Opcode::RepeatTwo(ref mut count) = opcode {
                    lookahead = true;
                    *count += 1;
                    continue;
                }
                lookahead = true;
                self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                opcode = Opcode::RepeatTwo(1);
                continue;
            }

            let ccount = calc_clrs(&mut clrs, 0, block);
            match ccount {
                1 => {
                    if let Opcode::Fill(ref mut count, ref clr) = opcode {
                        if *clr == clrs[0] {
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    opcode = Opcode::Fill(1, clrs[0]);
                },
                2 => {
                    if let Opcode::TwoColours(ref mut count, ref twoclrs, ref mut flags) = opcode {
                        if *count < 16 && twoclrs == &clrs[..2] {
                            flags[*count] = blk_2clr(block, twoclrs);
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [0; 16];
                    let twoclrs = [clrs[0], clrs[1]];
                    flags[0] = blk_2clr(block, &twoclrs);
                    opcode = Opcode::TwoColours(1, twoclrs, flags);
                },
                _ if self.mode == Strategy::Max2Clr => {
                    lossy_block(block, &self.lpal, 2, &mut lossy_clrs, &mut lossy_idx);
                    let mut blk_flags = 0;
                    for (dst, &idx) in pblock.iter_mut().zip(lossy_idx.iter()) {
                        *dst = lossy_clrs[usize::from(idx)];
                        blk_flags = (blk_flags << 1) | u16::from(idx);
                    }
                    if let Opcode::TwoColours(ref mut count, ref twoclrs, ref mut flags) = opcode {
                        if *count < 16 && twoclrs == &lossy_clrs[..2] {
                            flags[*count] = blk_flags;
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [0; 16];
                    let twoclrs = [lossy_clrs[0], lossy_clrs[1]];
                    flags[0] = blk_flags;
                    opcode = Opcode::TwoColours(1, twoclrs, flags);
                },
                3 => {
                    if let Opcode::FourColours(ref mut count, ref fourclrs, ref mut flags) = opcode {
                        if *count < 16 && fourclrs.contains(&clrs[0]) && fourclrs.contains(&clrs[1]) && fourclrs.contains(&clrs[2]) {
                            flags[*count] = blk_4clr(block, fourclrs);
                            *count += 1;
                            continue;
                        }
                    }
                    // check next blocks if possible to see if we can use merged 4-colour set
                    let mut fourclrs = clrs[..4].try_into().unwrap();
                    clrs2[..ccount].copy_from_slice(&clrs[..ccount]);
                    let mut ccount2 = ccount;
                    for next_blk in self.cur_frm.iter().skip(blk_no + 1) {
                        ccount2 = calc_clrs(&mut clrs2, ccount2, next_blk);
                        if ccount2 <= 4 {
                            fourclrs = clrs2[..4].try_into().unwrap();
                        } else {
                            break;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [0; 16];
                    flags[0] = blk_4clr(block, &fourclrs);
                    opcode = Opcode::FourColours(1, fourclrs, flags);
                },
                4 => {
                    if let Opcode::FourColours(ref mut count, ref fourclrs, ref mut flags) = opcode {
                        if *count < 16 && fourclrs == &clrs[..4] {
                            flags[*count] = blk_4clr(block, fourclrs);
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [0; 16];
                    let fourclrs = [clrs[0], clrs[1], clrs[2], clrs[3]];
                    flags[0] = blk_4clr(block, &fourclrs);
                    opcode = Opcode::FourColours(1, fourclrs, flags);
                },
                _ if self.mode == Strategy::Max4Clr => {
                    lossy_block(block, &self.lpal, 4, &mut lossy_clrs, &mut lossy_idx);
                    let mut blk_flags = 0;
                    for (dst, &idx) in pblock.iter_mut().zip(lossy_idx.iter()) {
                        *dst = lossy_clrs[usize::from(idx)];
                        blk_flags = (blk_flags << 2) | u32::from(idx);
                    }
                    if let Opcode::FourColours(ref mut count, ref fourclrs, ref mut flags) = opcode {
                        if *count < 16 && fourclrs == &lossy_clrs[..4] {
                            flags[*count] = blk_flags;
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [0; 16];
                    let fourclrs = lossy_clrs[..4].try_into().unwrap();
                    flags[0] = blk_flags;
                    opcode = Opcode::FourColours(1, fourclrs, flags);
                },
                5..=7 => {
                    if let Opcode::EightColours(ref mut count, ref eightclrs, ref mut flags) = opcode {
                        let mut all_in = true;
                        for clr in clrs[..ccount].iter() {
                            if !eightclrs.contains(clr) {
                                all_in = false;
                                break;
                            }
                        }
                        if *count < 16 && all_in {
                            flags[*count] = blk_8clr(block, eightclrs);
                            *count += 1;
                            continue;
                        }
                    }
                    // check next block if possible to see if we can use merged 8-colour set
                    let mut eightclrs = clrs[..8].try_into().unwrap();
                    clrs2[..ccount].copy_from_slice(&clrs[..ccount]);
                    let mut ccount2 = ccount;
                    for next_blk in self.cur_frm.iter().skip(blk_no + 1) {
                        ccount2 = calc_clrs(&mut clrs2, ccount2, next_blk);
                        if ccount2 <= 8 {
                            eightclrs = clrs2[..8].try_into().unwrap();
                        } else {
                            break;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [[0; 3]; 16];
                    flags[0] = blk_8clr(block, &eightclrs);
                    opcode = Opcode::EightColours(1, eightclrs, flags);
                },
                8 => {
                    if let Opcode::EightColours(ref mut count, ref eightclrs, ref mut flags) = opcode {
                        if *count < 16 && eightclrs == &clrs[..8] {
                            flags[*count] = blk_8clr(block, eightclrs);
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [[0; 3]; 16];
                    let eightclrs = clrs[..8].try_into().unwrap();
                    flags[0] = blk_8clr(block, &eightclrs);
                    opcode = Opcode::EightColours(1, eightclrs, flags);
                },
                _ if self.mode == Strategy::Max8Clr => {
                    lossy_block(block, &self.lpal, 8, &mut lossy_clrs, &mut lossy_idx);
                    let mut row_flags = [0; 4];
                    for (dst, &idx) in pblock.iter_mut().zip(lossy_idx.iter()) {
                        *dst = lossy_clrs[usize::from(idx)];
                    }
                    for (flg, row) in row_flags.iter_mut().zip(lossy_idx.chunks_exact(4)) {
                        for &idx in row.iter() {
                            *flg = (*flg << 3) | u16::from(idx);
                        }
                    }
                    let blk_flags = [(row_flags[0] << 4) | (row_flags[3] >> 8),
                                     (row_flags[1] << 4) | ((row_flags[3] >> 4) & 0xF),
                                     (row_flags[2] << 4) | (row_flags[3] & 0xF)];

                    if let Opcode::EightColours(ref mut count, ref eightclrs, ref mut flags) = opcode {
                        if *count < 16 && eightclrs == &lossy_clrs {
                            flags[*count] = blk_flags;
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    let mut flags = [[0; 3]; 16];
                    let eightclrs = lossy_clrs;
                    flags[0] = blk_flags;
                    opcode = Opcode::EightColours(1, eightclrs, flags);
                },
                _ => {
                    if let Opcode::Raw(ref mut count) = opcode {
                        if *count < 16 {
                            *count += 1;
                            continue;
                        }
                    }
                    self.helper.write_token(dbuf, &opcode, &self.cur_frm, blk_no);
                    opcode = Opcode::Raw(1);
                },
            }
        }
        self.helper.write_token(dbuf, &opcode, &self.cur_frm, self.cur_frm.len());

        is_intra
    }
    fn encode_skip_frame(&mut self, dbuf: &mut Vec<u8>) {
        self.helper.write_token(dbuf, &Opcode::Skip(self.cur_frm.len()), &[], 0);
    }
}

impl NAEncoder for SeanEncoder {
    fn negotiate_format(&self, encinfo: &EncodeParameters) -> EncoderResult<EncodeParameters> {
        match encinfo.format {
            NACodecTypeInfo::None => {
                Ok(EncodeParameters {
                    format: NACodecTypeInfo::Video(NAVideoInfo::new(0, 0, false, PAL8_FORMAT)),
                    ..Default::default()
                })
            },
            NACodecTypeInfo::Video(_vinfo) => {
                let mut info = *encinfo;
                if let NACodecTypeInfo::Video(ref mut vinfo) = info.format {
                    vinfo.format  = PAL8_FORMAT;
                    vinfo.width   = (vinfo.width  + 3) & !3;
                    vinfo.height  = (vinfo.height + 3) & !3;
                    vinfo.bits    = 8;
                    vinfo.flipped = false;
                } else {
                    return Err(EncoderError::FormatError);
                }
                Ok(info)
            },
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
        }
    }
    fn get_capabilities(&self) -> u64 { ENC_CAPS_SKIPFRAME }
    fn init(&mut self, stream_id: u32, encinfo: EncodeParameters) -> EncoderResult<NAStreamRef> {
        match encinfo.format {
            NACodecTypeInfo::None => Err(EncoderError::FormatError),
            NACodecTypeInfo::Audio(_) => Err(EncoderError::FormatError),
            NACodecTypeInfo::Video(vinfo) => {
                self.vinfo = vinfo;
                if vinfo.format != PAL8_FORMAT || ((vinfo.width | vinfo.height) & 3) != 0 {
                    return Err(EncoderError::FormatError);
                }
                let info = NACodecInfo::new("qt-smc", encinfo.format, None);
                let mut stream = NAStream::new(StreamType::Video, stream_id, info, encinfo.tb_num, encinfo.tb_den, 0);
                stream.set_num(stream_id as usize);
                let stream = stream.into_ref();
                self.stream = Some(stream.clone());
                Ok(stream)
            }
        }
    }
    fn encode(&mut self, frm: &NAFrame) -> EncoderResult<()> {
        let buf = frm.get_buffer();
        if let Some(vinfo) = buf.get_video_info() {
            if vinfo != self.vinfo {
                println!("Input format differs from the initial one");
                return Err(EncoderError::FormatError);
            }
        }
        self.frm_no += 1;
        let force_intra = self.frm_no >= self.key_int;

        let side_data = if let Some(vbuf) = buf.get_vbuf() {
                let mut npal = [0; 1024];
                let src = vbuf.get_data();
                for (dst, src) in npal.chunks_exact_mut(4).zip(src[vbuf.get_offset(1)..].chunks_exact(3)) {
                    dst[..3].copy_from_slice(src);
                }
                let new_pal = npal != *self.lpal;
                if new_pal {
                    self.lpal = Arc::new(npal);
                }
                Some(NASideData::Palette(new_pal, Arc::clone(&self.lpal)))
            } else { None };

        let mut dbuf = vec![0x80, 0, 0, 0]; // ID and 24-bit frame size
        let is_intra = if let Some(vbuf) = buf.get_vbuf() {
                self.load_frame(vbuf);
                if self.last_frm.is_empty() {
                    self.last_frm.extend_from_slice(&self.cur_frm);
                }

                self.encode_frame(&mut dbuf, force_intra)
            } else if matches!(buf, NABufferType::None) {
                self.cur_frm.clear();
                self.cur_frm.extend_from_slice(&self.last_frm);
                if force_intra {
                    if self.cur_frm.is_empty() {
                        return Err(EncoderError::InvalidParameters);
                    }
                    self.encode_frame(&mut dbuf, force_intra);
                    true
                } else {
                    self.encode_skip_frame(&mut dbuf);
                    false
                }
            } else {
                return Err(EncoderError::FormatError);
            };
        std::mem::swap(&mut self.last_frm, &mut self.cur_frm);

        if is_intra {
            self.frm_no = 0;
        }

        let size = dbuf.len() as u32;
        let _ = write_u24be(&mut dbuf[1..], size);

        let mut pkt = NAPacket::new(self.stream.clone().unwrap(), frm.ts, is_intra, dbuf);
        if let Some(sdata) = side_data {
            pkt.add_side_data(sdata);
        }

        self.pkt = Some(pkt);
        Ok(())
    }
    fn get_packet(&mut self) -> EncoderResult<Option<NAPacket>> {
        let mut npkt = None;
        std::mem::swap(&mut self.pkt, &mut npkt);
        Ok(npkt)
    }
    fn flush(&mut self) -> EncoderResult<()> {
        Ok(())
    }
}

const MODE_OPTION: &str = "mode";

const ENCODER_OPTS: &[NAOptionDefinition] = &[
    NAOptionDefinition {
        name: KEYFRAME_OPTION, description: KEYFRAME_OPTION_DESC,
        opt_type: NAOptionDefinitionType::Int(Some(0), Some(128)) },
    NAOptionDefinition {
        name: MODE_OPTION, description: "encoding strategy",
        opt_type: NAOptionDefinitionType::String(Some(&["lossless", "2clr", "4clr", "8clr"])) },
];

impl NAOptionHandler for SeanEncoder {
    fn get_supported_options(&self) -> &[NAOptionDefinition] { ENCODER_OPTS }
    fn set_options(&mut self, options: &[NAOption]) {
        for option in options.iter() {
            for opt_def in ENCODER_OPTS.iter() {
                if opt_def.check(option).is_ok() {
                    match option.name {
                        KEYFRAME_OPTION => {
                            if let NAValue::Int(intval) = option.value {
                                self.key_int = intval as u8;
                            }
                        },
                        MODE_OPTION => {
                            if let NAValue::String(ref strval) = option.value {
                                match strval.as_str() {
                                    "lossless" => { self.mode = Strategy::Lossless; },
                                    "2clr"     => { self.mode = Strategy::Max2Clr; },
                                    "4clr"     => { self.mode = Strategy::Max4Clr; },
                                    "8clr"     => { self.mode = Strategy::Max8Clr; },
                                    _ => {},
                                }
                            }
                        },
                        _ => {},
                    }
                }
            }
        }
    }
    fn query_option_value(&self, name: &str) -> Option<NAValue> {
        match name {
            KEYFRAME_OPTION => Some(NAValue::Int(i64::from(self.key_int))),
            MODE_OPTION => {
                match self.mode {
                    Strategy::Lossless => Some(NAValue::String("lossless".to_string())),
                    Strategy::Max2Clr  => Some(NAValue::String("2clr".to_string())),
                    Strategy::Max4Clr  => Some(NAValue::String("4clr".to_string())),
                    Strategy::Max8Clr  => Some(NAValue::String("8clr".to_string())),
                }
            },
            _ => None,
        }
    }
}

pub fn get_encoder() -> Box<dyn NAEncoder + Send> {
    Box::new(SeanEncoder::new())
}

#[cfg(test)]
mod test {
    use nihav_core::codecs::*;
    use nihav_core::demuxers::*;
    use nihav_core::muxers::*;
    use nihav_commonfmt::*;
    use nihav_codec_support::test::enc_video::*;
    use crate::*;

    // samples from https://samples.mplayerhq.hu/V-codecs/QTRLE
    fn test_core(enc_options: &[NAOption], hash: &[u32; 4]) {
        let mut dmx_reg = RegisteredDemuxers::new();
        generic_register_all_demuxers(&mut dmx_reg);
        let mut dec_reg = RegisteredDecoders::new();
        qt_register_all_decoders(&mut dec_reg);
        let mut mux_reg = RegisteredMuxers::new();
        generic_register_all_muxers(&mut mux_reg);
        let mut enc_reg = RegisteredEncoders::new();
        qt_register_all_encoders(&mut enc_reg);

        let dec_config = DecoderTestParams {
                demuxer:        "mov",
                in_name:        "assets/QT/Animation-256Greys.mov",
                stream_type:    StreamType::Video,
                limit:          Some(2),
                dmx_reg, dec_reg,
            };
        let enc_config = EncoderTestParams {
                muxer:          "mov",
                enc_name:       "qt-smc",
                out_name:       "qt_smc.mov",
                mux_reg, enc_reg,
            };
        let dst_vinfo = NAVideoInfo {
                width:   0,
                height:  0,
                format:  PAL8_FORMAT,
                flipped: false,
                bits:    8,
            };
        let enc_params = EncodeParameters {
                format:  NACodecTypeInfo::Video(dst_vinfo),
                quality: 0,
                bitrate: 0,
                tb_num:  0,
                tb_den:  0,
                flags:   0,
            };
        //test_encoding_to_file(&dec_config, &enc_config, enc_params, enc_options);
        test_encoding_md5(&dec_config, &enc_config, enc_params, enc_options, hash);
    }

    #[test]
    fn test_smc() {
        test_core(&[],
                  &[0xf53a5201, 0x7549e986, 0x680f929a, 0x92467b9e]);
    }
    #[test]
    fn test_smc_2clrs() {
        test_core(&[NAOption{ name: "mode", value: NAValue::String("2clr".to_string())}],
                  &[0xbf7be071, 0x62d1e567, 0x3ae99dc3, 0xac0bbc82]);
    }
    #[test]
    fn test_smc_4clrs() {
        test_core(&[NAOption{ name: "mode", value: NAValue::String("4clr".to_string())}],
                  &[0xc4b244ca, 0x1a4eebfd, 0x37b04213, 0x352a0105]);
    }
    #[test]
    fn test_smc_8clrs() {
        test_core(&[NAOption{ name: "mode", value: NAValue::String("8clr".to_string())}],
                  &[0x2f9a1663, 0x09601374, 0x1a014ab8, 0xfc341a92]);
    }
}
