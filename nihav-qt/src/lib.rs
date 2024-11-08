//! Crate for providing support for various QuickTime formats.
extern crate nihav_core;
extern crate nihav_codec_support;

#[allow(clippy::single_match)]
#[allow(clippy::upper_case_acronyms)]
mod codecs;
pub use crate::codecs::qt_register_all_decoders;
