use nihav_core::demuxers::*;


#[cfg(debug_assertions)]
#[allow(unused_macros)]
macro_rules! validate {
    ($a:expr) => { if !$a { println!("check failed at {}:{}", file!(), line!()); return Err(DemuxerError::InvalidData); } };
}
#[cfg(not(debug_assertions))]
#[allow(unused_macros)]
macro_rules! validate {
    ($a:expr) => { if !$a { return Err(DemuxerError::InvalidData); } };
}

#[cfg(feature="demuxer_warhol")]
mod warhol;

const DEMUXERS: &[&dyn DemuxerCreator] = &[
#[cfg(feature="demuxer_warhol")]
    &warhol::WarholDemuxerCreator {},
];

/// Registers all available demuxers provided by this crate.
pub fn qt_register_all_demuxers(rd: &mut RegisteredDemuxers) {
    for demuxer in DEMUXERS.iter() {
        rd.add_demuxer(*demuxer);
    }
}
