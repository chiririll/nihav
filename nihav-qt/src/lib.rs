//! Crate for providing support for various QuickTime formats.
extern crate nihav_core;
extern crate nihav_codec_support;

#[allow(clippy::single_match)]
#[allow(clippy::upper_case_acronyms)]
#[cfg(feature="decoders")]
mod codecs;
#[cfg(feature="decoders")]
pub use crate::codecs::qt_register_all_decoders;

#[cfg(feature="demuxers")]
mod demuxers;
#[cfg(feature="demuxers")]
pub use crate::demuxers::qt_register_all_demuxers;
