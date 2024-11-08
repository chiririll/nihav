//! Crate for providing support for Intel multimedia formats.
extern crate nihav_core;
extern crate nihav_codec_support;

#[allow(clippy::upper_case_acronyms)]
mod codecs;

pub use crate::codecs::indeo_register_all_decoders;
pub use crate::codecs::indeo_register_all_encoders;

mod demuxers;

pub use crate::demuxers::indeo_register_all_demuxers;

#[cfg(test)]
extern crate nihav_commonfmt;