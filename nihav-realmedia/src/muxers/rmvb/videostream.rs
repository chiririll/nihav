use nihav_core::frame::*;
use nihav_core::muxers::*;
use super::RMStreamWriter;

static VIDEO_CODEC_REGISTRY: &[(&[u8;4], &str)] = &[
    (b"RV10", "realvideo1"),
    (b"RV20", "realvideo2"),
    (b"RVTR", "realvideo2"),
    (b"RV30", "realvideo3"),
    (b"RV40", "realvideo4"),
    (b"RV60", "realvideo6"),
    (b"CLV1", "clearvideo_rm"),
];

pub struct DummyStreamWriter {}
impl RMStreamWriter for DummyStreamWriter {
    fn write_header(&mut self, _bw: &mut ByteWriter, _stream: &NAStream) -> MuxerResult<()> {
        Ok(())
    }
    fn queue_packet(&mut self, _pkt: NAPacket, _ms: u32) -> bool {
        true
    }
    fn get_packet(&mut self) -> Option<(Vec<u8>, u32, bool)> {
        None
    }
    fn flush(&mut self) { }
    fn finish(&mut self, _bw: &mut ByteWriter) -> MuxerResult<()> {
        Ok(())
    }
}

struct VideoStreamWriter {
    fcc:        [u8; 4],
    buf:        Vec<u8>,
    nslices:    usize,
    cur_slice:  usize,
    seq_no:     u8,
    time:       u32,
    mi_time:    u32,
}

impl RMStreamWriter for VideoStreamWriter {
    fn write_header(&mut self, bw: &mut ByteWriter, vstream: &NAStream) -> MuxerResult<()> {
        let info = vstream.get_info().get_properties().get_video_info().unwrap();
        let start = bw.tell();

        bw.write_u32be(0)?; // header size
        bw.write_buf(b"VIDO")?;
        bw.write_buf(&self.fcc)?;
        bw.write_u16be(info.width as u16)?;
        bw.write_u16be(info.height as u16)?;
        bw.write_u16be(12)?; // bpp
        bw.write_u16be(0)?; // aligned width
        bw.write_u16be(0)?; // aligned height
        let (tb_num, tb_den) = vstream.get_timebase();
        if tb_num != 0 && tb_den != 0 {
            bw.write_u16be((tb_den / tb_num) as u16)?;
            let mut fps_frac = tb_den % tb_num;
            let mut div = tb_num;
            while div >= 0x10000 {
                fps_frac >>= 1;
                div      >>= 1;
            }
            fps_frac = (fps_frac << 16) / div;
            bw.write_u16le(fps_frac as u16)?;
        } else {
            bw.write_u16be(0)?;
            bw.write_u16be(0)?;
        }

        if let Some(edata) = vstream.get_info().get_extradata() {
            bw.write_buf(&edata)?;
        }
        let end = bw.tell();
        bw.seek(SeekFrom::Start(start))?;
        bw.write_u32be((end - start) as u32)?;
        bw.seek(SeekFrom::Start(end))?;
        Ok(())
    }
    fn queue_packet(&mut self, pkt: NAPacket, ms: u32) -> bool {
        if self.nslices == 0 {
            let src = pkt.get_buffer();
            let nslices = usize::from(src[0]) + 1;
            if src.len() > nslices * 8 + 1 {
                self.nslices = nslices;
                self.cur_slice = 0;
                self.buf.resize(src.len(), 0);
                self.buf.copy_from_slice(&src);
                self.time = ms;
                if ms > 0 {
                    self.mi_time = ms.max(self.mi_time + 1);
                }
            }
            true
        } else {
            false
        }
    }
    fn get_packet(&mut self) -> Option<(Vec<u8>, u32, bool)> {
        if self.cur_slice < self.nslices {
            let first = self.cur_slice == 0;
            let hdr_size = self.nslices * 8 + 1;
            let cur_off = (read_u32be(&self.buf[self.cur_slice * 8 + 5..]).unwrap_or(0) as usize) + hdr_size;
            let next_off = if self.cur_slice + 1 < self.nslices {
                    (read_u32be(&self.buf[self.cur_slice * 8 + 13..]).unwrap_or(0) as usize) + hdr_size
                } else {
                    self.buf.len()
                };
            let next_off = next_off.max(cur_off);
            let src = &self.buf[cur_off..next_off];
            let ret = if self.nslices == 1 {
                    let mut dst = vec![0; src.len() + 2];
                    dst[0] = 0x40;
                    dst[1] = self.seq_no;
                    dst[2..].copy_from_slice(src);
                    dst
                } else {
                    let mut dst = Vec::with_capacity(src.len() + 11);
                    let mut gw = GrowableMemoryWriter::new_write(&mut dst);
                    let mut bw = ByteWriter::new(&mut gw);

                    let hdr = ((self.nslices as u16) << 7) | ((self.cur_slice + 1) as u16);
                    bw.write_u16be(hdr).unwrap();

                    let full_size = self.buf.len() - hdr_size;
                    if full_size < (1 << 14) {
                        bw.write_u16be(0xC000 | (full_size as u16)).unwrap();
                    } else {
                        bw.write_u32be(0x80000000 | (full_size as u32)).unwrap();
                    }
                    let coff = cur_off - hdr_size;
                    if coff < (1 << 14) {
                        bw.write_u16be(0x4000 | (coff as u16)).unwrap();
                    } else {
                        bw.write_u32be(coff as u32).unwrap();
                    }
                    bw.write_byte(self.seq_no).unwrap();
                    bw.write_buf(src).unwrap();
                    dst
                };
            self.cur_slice += 1;
            if self.cur_slice == self.nslices {
                self.nslices = 0;
                self.cur_slice = 0;
                self.seq_no = self.seq_no.wrapping_add(1);
            }
            Some((ret, self.mi_time, first))
        } else {
            None
        }
    }
    fn flush(&mut self) { }
    fn finish(&mut self, _bw: &mut ByteWriter) -> MuxerResult<()> {
        Ok(())
    }
}

pub fn create_video_stream(stream: &NAStream) -> MuxerResult<Box<dyn RMStreamWriter>> {
    let info = stream.get_info();
    let cname = info.get_name();

    for &(fcc, name) in VIDEO_CODEC_REGISTRY.iter() {
        if name == cname {
            return Ok(Box::new(VideoStreamWriter {
                    fcc:        *fcc,
                    buf:        Vec::new(),
                    nslices:    0,
                    cur_slice:  0,
                    seq_no:     0,
                    time:       0,
                    mi_time:    0,
                }));
        }
    }
    Err(MuxerError::UnsupportedFormat)
}
