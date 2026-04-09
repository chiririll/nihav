//! Crate for providing support for various formats that cannot be grouped elsewhere.
extern crate nihav_core;
extern crate nihav_codec_support;

#[cfg(feature="decoders")]
mod codecs;

#[cfg(feature="demuxers")]
mod demuxers;

#[cfg(feature="decoders")]
pub use crate::codecs::misc_register_all_decoders;

#[cfg(feature="encoders")]
pub use crate::codecs::misc_register_all_encoders;

#[cfg(feature="demuxers")]
pub use crate::demuxers::misc_register_all_demuxers;
