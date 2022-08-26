use nihav_core::frame::*;
use nihav_codec_support::codecs::MV;
use nihav_codec_support::codecs::blockdsp::*;

#[cfg(not(debug_assertions))]
mod release;
#[cfg(not(debug_assertions))]
use release::*;
#[cfg(debug_assertions)]
mod debug;
#[cfg(debug_assertions)]
use debug::*;

fn clip_u8(val: i16) -> u8 { val.max(0).min(255) as u8 }

pub struct H264MC {
    avg_buf:    NAVideoBufferRef<u8>,
}

impl H264MC {
    pub fn new(avg_buf: NAVideoBufferRef<u8>) -> Self {
        Self {
            avg_buf
        }
    }
    pub fn do_mc(&mut self, frm: &mut NASimpleVideoFrame<u8>, refpic: NAVideoBufferRef<u8>, xpos: usize, ypos: usize, w: usize, h: usize, mv: MV) {
        let mut ebuf = [0u8; 22 * 22];
        let mvx = mv.x >> 2;
        let mvy = mv.y >> 2;
        let mode = ((mv.x & 3) + (mv.y & 3) * 4) as usize;
        let pre  = if mode != 0 { 2isize } else { 0 };
        let post = if mode != 0 { 3isize } else { 0 };
        let (yw, yh) = refpic.get_dimensions(0);
        let src = refpic.get_data();
        let systride = refpic.get_stride(0);
        let src_x = (xpos as isize) + (mvx as isize);
        let src_y = (ypos as isize) + (mvy as isize);
        let (ysrc, ystride) = if (src_x - pre < 0) || (src_x + (w as isize) + post > (yw as isize)) || (src_y - pre < 0) || (src_y + (h as isize) + post > (yh as isize)) {
                let add = (pre + post) as usize;
                edge_emu(&refpic, src_x - pre, src_y - pre, w + add, h + add, &mut ebuf, 22, 0, 0);
                (ebuf.as_slice(), 22)
            } else {
                (&src[refpic.get_offset(0) + ((src_x - pre) as usize) + ((src_y - pre) as usize) * systride..], systride)
            };
        (H264_LUMA_INTERP[mode])(&mut frm.data[frm.offset[0] + xpos + ypos * frm.stride[0]..], frm.stride[0], ysrc, ystride, w, h);

        let (cw, ch) = refpic.get_dimensions(1);
        let mvx = mv.x >> 3;
        let mvy = mv.y >> 3;
        let dx = (mv.x & 7) as u16;
        let dy = (mv.y & 7) as u16;
        let src_x = ((xpos >> 1) as isize) + (mvx as isize);
        let src_y = ((ypos >> 1) as isize) + (mvy as isize);
        let suoff = refpic.get_offset(1);
        let svoff = refpic.get_offset(2);
        let sustride = refpic.get_stride(1);
        let svstride = refpic.get_stride(2);
        let cbw = w / 2;
        let cbh = h / 2;
        let (csrc, cstride) = if (src_x < 0) || (src_x + (cbw as isize) + 1 > (cw as isize)) || (src_y < 0) || (src_y + (cbh as isize) + 1 > (ch as isize)) {
                edge_emu(&refpic, src_x, src_y, cbw+1, cbh+1, &mut ebuf,      18, 1, 4);
                edge_emu(&refpic, src_x, src_y, cbw+1, cbh+1, &mut ebuf[9..], 18, 2, 4);
                ([&ebuf, &ebuf[9..]], [18, 18])
            } else {
                ([&src[suoff + (src_x as usize) + (src_y as usize) * sustride..],
                 &src[svoff + (src_x as usize) + (src_y as usize) * svstride..]],
                 [sustride, svstride])
            };
        for chroma in 1..3 {
            let off = frm.offset[chroma] + xpos / 2 + (ypos / 2) * frm.stride[chroma];
            chroma_interp(&mut frm.data[off..], frm.stride[chroma], csrc[chroma - 1], cstride[chroma - 1], dx, dy, cbw, cbh);
        }
    }

    pub fn mc_blocks(&mut self, ydst: &mut [u8], udst: &mut [u8], vdst: &mut [u8], refpic: NAVideoBufferRef<u8>, xpos: usize, ypos: usize, w: usize, h: usize, mv: MV) {
        let mode = ((mv.x & 3) + (mv.y & 3) * 4) as usize;

        let pre  = if mode != 0 { 2 } else { 0 };
        let post = if mode != 0 { 3 } else { 0 };
        let (width, height) = refpic.get_dimensions(0);
        let sx = (xpos as isize) + ((mv.x >> 2) as isize);
        let sy = (ypos as isize) + ((mv.y >> 2) as isize);

        const EBUF_STRIDE: usize = 32;
        let mut ebuf = [0u8; EBUF_STRIDE * (16 + 2 + 3)];

        if (sx - pre < 0) || (sx + (w as isize) + post > (width as isize)) ||
           (sy - pre < 0) || (sy + (h as isize) + post > (height as isize)) {
            let edge = (pre + post) as usize;
            edge_emu(&refpic, sx - pre, sy - pre, w + edge, h + edge,
                     &mut ebuf, EBUF_STRIDE, 0, 0);
            (H264_LUMA_INTERP[mode])(ydst, 16, &ebuf, EBUF_STRIDE, w, h);
        } else {
            let sstride = refpic.get_stride(0);
            let soff    = refpic.get_offset(0);
            let sdta    = refpic.get_data();
            let sbuf: &[u8] = sdta.as_slice();
            let saddr = soff + ((sx - pre) as usize) + ((sy - pre) as usize) * sstride;
            (H264_LUMA_INTERP[mode])(ydst, 16, &sbuf[saddr..], sstride, w, h);
        }

        let (cw, ch) = refpic.get_dimensions(1);
        let mvx = mv.x >> 3;
        let mvy = mv.y >> 3;
        let dx = (mv.x & 7) as u16;
        let dy = (mv.y & 7) as u16;
        let src_x = ((xpos >> 1) as isize) + (mvx as isize);
        let src_y = ((ypos >> 1) as isize) + (mvy as isize);
        let suoff = refpic.get_offset(1);
        let svoff = refpic.get_offset(2);
        let sustride = refpic.get_stride(1);
        let svstride = refpic.get_stride(2);
        let src = refpic.get_data();
        let cbw = w / 2;
        let cbh = h / 2;
        let (csrc, cstride) = if (src_x < 0) || (src_x + (cbw as isize) + 1 > (cw as isize)) || (src_y < 0) || (src_y + (cbh as isize) + 1 > (ch as isize)) {
                edge_emu(&refpic, src_x, src_y, cbw+1, cbh+1, &mut ebuf,      18, 1, 4);
                edge_emu(&refpic, src_x, src_y, cbw+1, cbh+1, &mut ebuf[9..], 18, 2, 4);
                ([&ebuf, &ebuf[9..]], [18, 18])
            } else {
                ([&src[suoff + (src_x as usize) + (src_y as usize) * sustride..],
                 &src[svoff + (src_x as usize) + (src_y as usize) * svstride..]],
                 [sustride, svstride])
            };
        chroma_interp(udst, 16, csrc[0], cstride[0], dx, dy, cbw, cbh);
        chroma_interp(vdst, 16, csrc[1], cstride[1], dx, dy, cbw, cbh);
    }

    pub fn avg(&mut self, dst: &mut [u8], dstride: usize, bw: usize, bh: usize, comp: usize) {
        let afrm = NASimpleVideoFrame::from_video_buf(&mut self.avg_buf).unwrap();
        let src = &afrm.data[afrm.offset[comp]..];
        let sstride = afrm.stride[comp];
        for (dline, sline) in dst.chunks_mut(dstride).zip(src.chunks(sstride)).take(bh) {
            for (dst, src) in dline.iter_mut().zip(sline.iter()).take(bw) {
                *dst = ((u16::from(*dst) + u16::from(*src) + 1) >> 1) as u8;
            }
        }
    }

    pub fn do_mc_avg(&mut self, frm: &mut NASimpleVideoFrame<u8>, refpic: NAVideoBufferRef<u8>, xpos: usize, ypos: usize, w: usize, h: usize, mv: MV) {
        let mut abuf = self.avg_buf.clone();
        let mut afrm = NASimpleVideoFrame::from_video_buf(&mut abuf).unwrap();
        let amv = MV { x: mv.x + (xpos as i16) * 4, y: mv.y + (ypos as i16) * 4 };
        self.do_mc(&mut afrm, refpic, 0, 0, w, h, amv);
        for comp in 0..3 {
            let shift = if comp == 0 { 0 } else { 1 };
            self.avg(&mut frm.data[frm.offset[comp] + (xpos >> shift) + (ypos >> shift) * frm.stride[comp]..], frm.stride[comp], w >> shift, h >> shift, comp);
        }
    }

    pub fn put_block_weighted(&mut self, dst: &mut [u8], stride: usize, src: &[u8], w: usize, h: usize, wparams: [i8; 3]) {
        let weight = i16::from(wparams[0]);
        let offset = i16::from(wparams[1]);
        let wshift = wparams[2] as u8;
        let bias = (1 << wshift) >> 1;

        for (drow, srow) in dst.chunks_mut(stride).zip(src.chunks(16)).take(h) {
            for (dst, &src) in drow[..w].iter_mut().zip(srow.iter()) {
                *dst = clip_u8(((i16::from(src) * weight + bias) >> wshift) + offset);
            }
        }
    }

    pub fn put_block_weighted2(&mut self, dst: &mut [u8], stride: usize, src0: &[u8], src1: &[u8], w: usize, h: usize, wparams: [i8; 5]) {
        let weight0 = i16::from(wparams[0]);
        let offset0 = i16::from(wparams[1]);
        let weight1 = i16::from(wparams[2]);
        let offset1 = i16::from(wparams[3]);
        let wshift = (wparams[4] as u8) + 1;
        let offset = (offset0 + offset1 + 1) >> 1;
        let bias = (1 << wshift) >> 1;

        for (drow, (srow0, srow1)) in dst.chunks_mut(stride).zip(src0.chunks(16).zip(src1.chunks(16))).take(h) {
            for (dst, (&src0, &src1)) in drow[..w].iter_mut().zip(srow0.iter().zip(srow1.iter())) {
                *dst = clip_u8(((i16::from(src0) * weight0 + i16::from(src1) * weight1 + bias) >> wshift) + offset);
            }
        }
    }

    pub fn gray_block(&mut self, frm: &mut NASimpleVideoFrame<u8>, x: usize, y: usize, w: usize, h: usize) {
        let yoff = frm.offset[0] + x + y * frm.stride[0];
        let coff = [frm.offset[1] + x / 2 + y / 2 * frm.stride[1],
                    frm.offset[2] + x / 2 + y / 2 * frm.stride[2]];
        for row in frm.data[yoff..].chunks_mut(frm.stride[0]).take(h) {
            for el in row[..w].iter_mut() {
                *el = 128;
            }
        }
        for chroma in 0..2 {
            for row in frm.data[coff[chroma]..].chunks_mut(frm.stride[chroma + 1]).take(h / 2) {
                for el in row[..w / 2].iter_mut() {
                    *el = 128;
                }
            }
        }
    }
}
