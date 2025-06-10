use nihav_core::codecs::*;

#[cfg(debug_assertions)]
macro_rules! validate {
    ($a:expr) => { if !$a { println!("check failed at {}:{}", file!(), line!()); return Err(DecoderError::InvalidData); } };
}
#[cfg(not(debug_assertions))]
macro_rules! validate {
    ($a:expr) => { if !$a { return Err(DecoderError::InvalidData); } };
}

#[cfg(feature="decoder_mwv1")]
mod mwv1;

#[cfg(feature="decoder_pgvv")]
mod pgvv;

#[cfg(feature="decoder_pivc")]
mod pivc;

#[cfg(feature="decoder_qpeg")]
mod qpeg;

#[cfg(feature="decoder_tealvid")]
mod tealvid;

const DECODERS: &[DecoderInfo] = &[
#[cfg(feature="decoder_mwv1")]
    DecoderInfo { name: "mwv1", get_decoder: mwv1::get_decoder },
#[cfg(feature="decoder_pgvv")]
    DecoderInfo { name: "pgvv", get_decoder: pgvv::get_decoder },
#[cfg(feature="decoder_pivc")]
    DecoderInfo { name: "pivideo", get_decoder: pivc::get_decoder },
#[cfg(feature="decoder_qpeg")]
    DecoderInfo { name: "qpeg-dvc", get_decoder: qpeg::get_decoder_dvc },
#[cfg(feature="decoder_tealvid")]
    DecoderInfo { name: "teal-video", get_decoder: tealvid::get_decoder_video },
#[cfg(feature="decoder_tealvid")]
    DecoderInfo { name: "teal-audio", get_decoder: tealvid::get_decoder_audio },
];

/// Registers all available codecs provided by this crate.
pub fn misc_register_all_decoders(rd: &mut RegisteredDecoders) {
    for decoder in DECODERS.iter() {
        rd.add_decoder(*decoder);
    }
}

