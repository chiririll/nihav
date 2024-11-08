//! Crate for providing support for various lossless audio formats.
extern crate nihav_core;
extern crate nihav_codec_support;

#[cfg(any(feature="decoders", feature="encoders"))]
mod codecs;
#[cfg(feature="demuxers")]
mod demuxers;
#[cfg(feature="muxers")]
mod muxers;
#[cfg(feature="decoders")]
pub use crate::codecs::llaudio_register_all_decoders;
pub use crate::demuxers::llaudio_register_all_demuxers;
#[cfg(feature="decoders")]
pub use crate::codecs::llaudio_register_all_packetisers;
#[cfg(feature="demuxers")]
pub use crate::demuxers::llaudio_register_all_raw_demuxers;
#[cfg(feature="encoders")]
pub use crate::codecs::llaudio_register_all_encoders;
#[cfg(feature="muxers")]
pub use crate::muxers::llaudio_register_all_muxers;
