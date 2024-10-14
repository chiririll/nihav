use nihav_core::codecs::DecoderResult;
use nihav_codec_support::codecs::MV;

pub trait MTHelper {
    fn wait_for_mb(&mut self, xpos: usize, ypos: usize, id: u32) -> DecoderResult<()>;
    fn is_colo_mb_not_coded(&mut self, mb_addr: usize, id: u32) -> DecoderResult<bool>;
    fn get_colo_mvs(&mut self, mb_addr: usize, id: u32) -> DecoderResult<[MV; 4]>;
    fn update_pos(&mut self, mb_addr: usize);
}
