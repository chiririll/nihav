use nihav_core::codecs::*;

#[cfg(debug_assertions)]
macro_rules! validate {
    ($a:expr) => { if !$a { println!("check failed at {}:{}", file!(), line!()); return Err(DecoderError::InvalidData); } };
}
#[cfg(not(debug_assertions))]
macro_rules! validate {
    ($a:expr) => { if !$a { return Err(DecoderError::InvalidData); } };
}

#[cfg(feature="decoder_adorage")]
mod adorage;

#[cfg(feature="decoder_arbc")]
mod arbc;

#[cfg(feature="decoder_fif")]
mod fif;

#[cfg(feature="decoder_moviepak")]
mod moviepak;

#[cfg(feature="decoder_mvi")]
mod motionpixels;

#[cfg(feature="decoder_mwv1")]
mod mwv1;

#[cfg(feature="decoder_pgvv")]
mod pgvv;

#[cfg(feature="decoder_pivc")]
mod pivc;

#[cfg(feature="decoder_pixar")]
mod pixar;

#[cfg(feature="decoder_qpeg")]
mod qpeg;

#[cfg(feature="decoder_tealvid")]
mod tealvid;

#[cfg(feature="decoder_ultimotion")]
mod ultimotion;
#[cfg(any(feature="decoder_ultimotion", feature="encoder_ultimotion"))]
mod ultidata;

const DECODERS: &[DecoderInfo] = &[
#[cfg(feature="decoder_adorage")]
    DecoderInfo { name: "adorage", get_decoder: adorage::get_decoder },
#[cfg(feature="decoder_arbc")]
    DecoderInfo { name: "gryphon-arbc-vfw", get_decoder: arbc::get_decoder_vfw },
#[cfg(feature="decoder_arbc")]
    DecoderInfo { name: "gryphon-arbc-qt", get_decoder: arbc::get_decoder_qt },
#[cfg(feature="decoder_fif")]
    DecoderInfo { name: "fif", get_decoder: fif::get_decoder },
#[cfg(feature="decoder_moviepak")]
    DecoderInfo { name: "moviepak", get_decoder: moviepak::get_decoder },
#[cfg(feature="decoder_mvi")]
    DecoderInfo { name: "mvi0", get_decoder: motionpixels::get_decoder0 },
#[cfg(feature="decoder_mvi")]
    DecoderInfo { name: "mvi1", get_decoder: motionpixels::get_decoder1 },
#[cfg(feature="decoder_mvi")]
    DecoderInfo { name: "mvi2", get_decoder: motionpixels::get_decoder2 },
#[cfg(feature="decoder_mwv1")]
    DecoderInfo { name: "mwv1", get_decoder: mwv1::get_decoder },
#[cfg(feature="decoder_pgvv")]
    DecoderInfo { name: "pgvv", get_decoder: pgvv::get_decoder },
#[cfg(feature="decoder_pivc")]
    DecoderInfo { name: "pivideo", get_decoder: pivc::get_decoder },
#[cfg(feature="decoder_pixar")]
    DecoderInfo { name: "pixar-0", get_decoder: pixar::get_decoder_0 },
#[cfg(feature="decoder_pixar")]
    DecoderInfo { name: "pixar-3", get_decoder: pixar::get_decoder_3 },
#[cfg(feature="decoder_pixar")]
    DecoderInfo { name: "pixar-4", get_decoder: pixar::get_decoder_4 },
#[cfg(feature="decoder_pixar")]
    DecoderInfo { name: "pixar-5", get_decoder: pixar::get_decoder_5 },
//#[cfg(feature="decoder_pixar")]
//    DecoderInfo { name: "pixar-7", get_decoder: pixar::get_decoder_7 },
#[cfg(feature="decoder_pixar")]
    DecoderInfo { name: "pixar-a", get_decoder: pixar::get_decoder_a },
#[cfg(feature="decoder_qpeg")]
    DecoderInfo { name: "qpeg-dvc", get_decoder: qpeg::get_decoder_dvc },
#[cfg(feature="decoder_tealvid")]
    DecoderInfo { name: "teal-video", get_decoder: tealvid::get_decoder_video },
#[cfg(feature="decoder_tealvid")]
    DecoderInfo { name: "teal-audio", get_decoder: tealvid::get_decoder_audio },
#[cfg(feature="decoder_ultimotion")]
    DecoderInfo { name: "ultimotion", get_decoder: ultimotion::get_decoder },
];

/// Registers all available codecs provided by this crate.
pub fn misc_register_all_decoders(rd: &mut RegisteredDecoders) {
    for decoder in DECODERS.iter() {
        rd.add_decoder(*decoder);
    }
}

#[cfg(feature="encoder_ultimotion")]
mod ultienc;

#[cfg(feature="encoders")]
const ENCODERS: &[EncoderInfo] = &[
#[cfg(feature="encoder_ultimotion")]
    EncoderInfo { name: "ultimotion", get_encoder: ultienc::get_encoder },
];

/// Registers all available encoders provided by this crate.
#[cfg(feature="encoders")]
pub fn misc_register_all_encoders(re: &mut RegisteredEncoders) {
    for encoder in ENCODERS.iter() {
        re.add_encoder(*encoder);
    }
}
