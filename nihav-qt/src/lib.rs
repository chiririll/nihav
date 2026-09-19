//! Crate for providing support for various QuickTime formats.
extern crate nihav_core;
extern crate nihav_codec_support;

#[allow(clippy::single_match)]
#[allow(clippy::upper_case_acronyms)]
#[cfg(feature="decoders")]
mod codecs;
#[cfg(feature="decoders")]
pub use crate::codecs::qt_register_all_decoders;
#[cfg(feature="encoders")]
pub use crate::codecs::qt_register_all_encoders;
///! Creates QuickDraw with externally provided list of other decoders.
///!
///! QuickDraw data may actually encode picture with some other QuickTime codec,
///! e.g. RoadPizza or Cinepak. So in order to decode that data it may need
///! to create another decoder, which may be implemented elsewhere.
///! Hence the need for this special function.
#[cfg(feature="decoder_qdraw")]
pub use crate::codecs::qdraw::get_qdraw_decoder_with_external_decreg;

#[cfg(feature="demuxers")]
mod demuxers;
#[cfg(feature="demuxers")]
pub use crate::demuxers::qt_register_all_demuxers;
