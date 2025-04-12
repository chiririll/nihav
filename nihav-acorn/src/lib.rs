//! Crate for providing support for various Acorn Replay Movie formats.
extern crate nihav_core;
extern crate nihav_codec_support;

mod codecs;

mod demuxers;

pub use crate::codecs::acorn_register_all_decoders;
pub use crate::codecs::acorn_register_all_packetisers;
pub use crate::demuxers::acorn_register_all_demuxers;
pub use crate::demuxers::acorn_register_all_raw_demuxers;