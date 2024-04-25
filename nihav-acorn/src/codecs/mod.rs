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

const ACORN_CODECS: &[DecoderInfo] = &[
#[cfg(feature="decoder_movinglines")]
    DecoderInfo { name: "movinglines", get_decoder: movinglines::get_decoder },
#[cfg(feature="decoder_movingblocks")]
    DecoderInfo { name: "movingblocks", get_decoder: movingblocks::get_decoder },
];

/// Registers all available codecs provided by this crate.
pub fn acorn_register_all_decoders(rd: &mut RegisteredDecoders) {
    for decoder in ACORN_CODECS.iter() {
        rd.add_decoder(*decoder);
    }
}

const ACORN_PACKETISERS: &[PacketiserInfo] = &[
#[cfg(feature="decoder_movinglines")]
    PacketiserInfo { name: "movinglines", get_packetiser: movinglines::get_packetiser },
#[cfg(feature="decoder_movingblocks")]
    PacketiserInfo { name: "movingblocks", get_packetiser: movingblocks::get_packetiser },
];

/// Registers all available packetisers provided by this crate.
pub fn acorn_register_all_packetisers(rp: &mut RegisteredPacketisers) {
    for pkt in ACORN_PACKETISERS.iter() {
        rp.add_packetiser(*pkt);
    }
}
