//! Crate for providing support for various game-related formats.
extern crate nihav_core;
extern crate nihav_codec_support;

#[allow(clippy::upper_case_acronyms)]
mod codecs;
pub use crate::codecs::game_register_all_decoders;
mod demuxers;
pub use crate::demuxers::game_register_all_demuxers;

mod muxers;
pub use crate::muxers::game_register_all_muxers;
