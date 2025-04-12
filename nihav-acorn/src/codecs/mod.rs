use nihav_core::codecs::*;

#[cfg(debug_assertions)]
macro_rules! validate {
    ($a:expr) => { if !$a { println!("check failed at {}:{}", file!(), line!()); return Err(DecoderError::InvalidData); } };
}
#[cfg(not(debug_assertions))]
macro_rules! validate {
    ($a:expr) => { if !$a { return Err(DecoderError::InvalidData); } };
}

#[allow(dead_code)]
const RGB555_FORMAT: NAPixelFormaton = NAPixelFormaton { model: ColorModel::RGB(RGBSubmodel::RGB), components: 3,
                                        comp_info: [
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  0, comp_offs: 0, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift:  5, comp_offs: 0, next_elem: 2 }),
                                            Some(NAPixelChromaton{ h_ss: 0, v_ss: 0, packed: true, depth: 5, shift: 10, comp_offs: 0, next_elem: 2 }),
                                            None, None],
                                        elem_size: 2, be: false, alpha: false, palette: false };

#[allow(dead_code)]
mod yuvtab;

#[cfg(feature="decoder_movinglines")]
mod movinglines;
#[cfg(feature="decoder_movingblocks")]
mod movingblocks;
#[cfg(feature="decoder_movingblockshq")]
mod movingblockshq;
#[cfg(feature="decoder_supermovingblocks")]
mod supermovingblocks;
#[cfg(feature="decoder_rawvideo")]
mod rawvideo;

#[cfg(feature="decoder_linepack")]
mod linepack;

#[cfg(feature="decoder_escape")]
mod escape;

#[cfg(feature="decoder_euclid")]
mod euclid;

#[cfg(feature="decoder_rawaudio")]
mod rawaudio;

const ACORN_CODECS: &[DecoderInfo] = &[
#[cfg(feature="decoder_rawvideo")]
    DecoderInfo { name: "arm_rawvideo", get_decoder: rawvideo::get_decoder },
#[cfg(feature="decoder_movinglines")]
    DecoderInfo { name: "movinglines", get_decoder: movinglines::get_decoder },
#[cfg(feature="decoder_movingblocks")]
    DecoderInfo { name: "movingblocks", get_decoder: movingblocks::get_decoder },
#[cfg(feature="decoder_movingblockshq")]
    DecoderInfo { name: "movingblockshq", get_decoder: movingblockshq::get_decoder },
#[cfg(feature="decoder_supermovingblocks")]
    DecoderInfo { name: "supermovingblocks", get_decoder: supermovingblocks::get_decoder },

#[cfg(feature="decoder_linepack")]
    DecoderInfo { name: "linepack", get_decoder: linepack::get_decoder },

#[cfg(feature="decoder_escape")]
    DecoderInfo { name: "escape122", get_decoder: escape::get_decoder122 },
#[cfg(feature="decoder_escape")]
    DecoderInfo { name: "escape124", get_decoder: escape::get_decoder124 },
#[cfg(feature="decoder_escape")]
    DecoderInfo { name: "escape130", get_decoder: escape::get_decoder130 },
#[cfg(feature="decoder_escape")]
    DecoderInfo { name: "escape-adpcm", get_decoder: escape::get_decoder_audio },

#[cfg(feature="decoder_euclid")]
    DecoderInfo { name: "euclid", get_decoder: euclid::get_decoder },

#[cfg(feature="decoder_rawaudio")]
    DecoderInfo { name: "arm_rawaudio", get_decoder: rawaudio::get_decoder },
];

/// Registers all available codecs provided by this crate.
pub fn acorn_register_all_decoders(rd: &mut RegisteredDecoders) {
    for decoder in ACORN_CODECS.iter() {
        rd.add_decoder(*decoder);
    }
}

#[cfg(any(feature="packetiser_cinepak", feature="packetiser_msvideo1"))]
mod wss_packetisers;

const ACORN_PACKETISERS: &[PacketiserInfo] = &[
#[cfg(feature="decoder_rawvideo")]
    PacketiserInfo { name: "arm_rawvideo", get_packetiser: rawvideo::get_packetiser },
#[cfg(feature="decoder_movinglines")]
    PacketiserInfo { name: "movinglines", get_packetiser: movinglines::get_packetiser },
#[cfg(feature="decoder_movingblocks")]
    PacketiserInfo { name: "movingblocks", get_packetiser: movingblocks::get_packetiser },
#[cfg(feature="decoder_movingblockshq")]
    PacketiserInfo { name: "movingblockshq", get_packetiser: movingblockshq::get_packetiser },
#[cfg(feature="decoder_supermovingblocks")]
    PacketiserInfo { name: "supermovingblocks", get_packetiser: supermovingblocks::get_packetiser },

#[cfg(feature="decoder_linepack")]
    PacketiserInfo { name: "linepack", get_packetiser: linepack::get_packetiser },

#[cfg(feature="decoder_escape")]
    PacketiserInfo { name: "escape122", get_packetiser: escape::get_packetiser },
#[cfg(feature="decoder_escape")]
    PacketiserInfo { name: "escape124", get_packetiser: escape::get_packetiser },
#[cfg(feature="decoder_escape")]
    PacketiserInfo { name: "escape130", get_packetiser: escape::get_packetiser },
#[cfg(feature="decoder_escape")]
    PacketiserInfo { name: "escape-adpcm", get_packetiser: rawaudio::get_packetiser },

#[cfg(feature="decoder_rawaudio")]
    PacketiserInfo { name: "arm_rawaudio", get_packetiser: rawaudio::get_packetiser },

#[cfg(feature="packetiser_cinepak")]
    PacketiserInfo { name: "cinepak", get_packetiser: wss_packetisers::get_packetiser_cinepak },
#[cfg(feature="packetiser_msvideo1")]
    PacketiserInfo { name: "msvideo1", get_packetiser: wss_packetisers::get_packetiser_msvideo1 },
];

/// Registers all available packetisers provided by this crate.
pub fn acorn_register_all_packetisers(rp: &mut RegisteredPacketisers) {
    for pkt in ACORN_PACKETISERS.iter() {
        rp.add_packetiser(*pkt);
    }
}
