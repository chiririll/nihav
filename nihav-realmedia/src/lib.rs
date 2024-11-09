//! Crate for providing support for RealMedia formats.
extern crate nihav_core;
extern crate nihav_codec_support;

#[cfg(any(feature="decoders", feature="encoders"))]
#[allow(clippy::excessive_precision)]
#[allow(clippy::single_match)]
#[allow(clippy::upper_case_acronyms)]
mod codecs;
#[cfg(feature="decoders")]
pub use crate::codecs::realmedia_register_all_decoders;
#[cfg(feature="encoders")]
pub use crate::codecs::realmedia_register_all_encoders;

#[cfg(feature="demuxers")]
#[allow(clippy::upper_case_acronyms)]
mod demuxers;
#[cfg(feature="demuxers")]
pub use crate::demuxers::realmedia_register_all_demuxers;

#[cfg(feature="muxers")]
mod muxers;
#[cfg(feature="muxers")]
pub use crate::muxers::realmedia_register_all_muxers;
