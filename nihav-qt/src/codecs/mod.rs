use nihav_core::codecs::*;

#[cfg(debug_assertions)]
macro_rules! validate {
    ($a:expr) => { if !$a { println!("check failed at {}:{}", file!(), line!()); return Err(DecoderError::InvalidData); } };
}
#[cfg(not(debug_assertions))]
macro_rules! validate {
    ($a:expr) => { if !$a { return Err(DecoderError::InvalidData); } };
}

#[cfg(feature="decoder_qdraw")]
mod qdraw;

#[cfg(feature="decoder_rle")]
mod rle;

#[cfg(feature="decoder_rpza")]
mod rpza;

#[cfg(feature="decoder_smc")]
mod smc;

#[cfg(feature="decoder_svq1")]
mod svq1;
#[cfg(feature="decoder_svq1")]
mod svq1data;

#[cfg(feature="decoder_svq3")]
#[allow(clippy::too_many_arguments)]
mod svq3;
#[cfg(feature="decoder_svq3")]
#[allow(clippy::collapsible_else_if)]
#[allow(clippy::erasing_op)]
#[allow(clippy::identity_op)]
#[allow(clippy::many_single_char_names)]
mod svq3dsp;

#[cfg(feature="decoder_rawvid")]
mod rawvid;

#[cfg(feature="decoder_alac")]
mod alac;

#[cfg(feature="decoder_ima_adpcm_qt")]
mod imaadpcm;

#[cfg(feature="decoder_mace")]
mod mace;

#[cfg(feature="decoder_qcelp")]
#[allow(clippy::excessive_precision)]
mod qcelp;

#[cfg(any(feature="decoder_qdm",feature="decoder_qdm2"))]
mod qdmcommon;
#[cfg(feature="decoder_qdm")]
mod qdmc;
#[cfg(feature="decoder_qdm2")]
#[allow(clippy::excessive_precision)]
mod qdm2fft;
#[cfg(feature="decoder_qdm2")]
#[allow(clippy::excessive_precision)]
mod qdm2qmf;
#[cfg(feature="decoder_qdm2")]
#[allow(clippy::excessive_precision)]
mod qdm2;

const QT_CODECS: &[DecoderInfo] = &[
#[cfg(feature="decoder_qdraw")]
    DecoderInfo { name: "qdraw", get_decoder: qdraw::get_decoder },
#[cfg(feature="decoder_rle")]
    DecoderInfo { name: "qt-rle", get_decoder: rle::get_decoder },
#[cfg(feature="decoder_rpza")]
    DecoderInfo { name: "apple-video", get_decoder: rpza::get_decoder },
#[cfg(feature="decoder_smc")]
    DecoderInfo { name: "qt-smc", get_decoder: smc::get_decoder },
#[cfg(feature="decoder_svq1")]
    DecoderInfo { name: "sorenson-video", get_decoder: svq1::get_decoder },
#[cfg(feature="decoder_svq3")]
    DecoderInfo { name: "sorenson-video3", get_decoder: svq3::get_decoder },
#[cfg(feature="decoder_rawvid")]
    DecoderInfo { name: "qt-yuv2", get_decoder: rawvid::get_decoder_yuv2 },
#[cfg(feature="decoder_rawvid")]
    DecoderInfo { name: "qt-yuv4", get_decoder: rawvid::get_decoder_yuv4 },
#[cfg(feature="decoder_alac")]
    DecoderInfo { name: "alac", get_decoder: alac::get_decoder },
#[cfg(feature="decoder_ima_adpcm_qt")]
    DecoderInfo { name: "ima-adpcm-qt", get_decoder: imaadpcm::get_decoder },
#[cfg(feature="decoder_mace")]
    DecoderInfo { name: "mace-3", get_decoder: mace::get_decoder_3 },
#[cfg(feature="decoder_mace")]
    DecoderInfo { name: "mace-6", get_decoder: mace::get_decoder_6 },
#[cfg(feature="decoder_qcelp")]
    DecoderInfo { name: "qualcomm-purevoice", get_decoder: qcelp::get_decoder },
#[cfg(feature="decoder_qdm")]
    DecoderInfo { name: "qdesign-music", get_decoder: qdmc::get_decoder },
#[cfg(feature="decoder_qdm2")]
    DecoderInfo { name: "qdesign-music2", get_decoder: qdm2::get_decoder },
];

/// Registers all available codecs provided by this crate.
pub fn qt_register_all_decoders(rd: &mut RegisteredDecoders) {
    for decoder in QT_CODECS.iter() {
        rd.add_decoder(*decoder);
    }
}
