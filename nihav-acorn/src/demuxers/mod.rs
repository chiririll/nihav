use nihav_core::demuxers::*;


#[allow(unused_macros)]
#[cfg(debug_assertions)]
macro_rules! validate {
    ($a:expr) => { if !$a { println!("check failed at {}:{}", file!(), line!()); return Err(DemuxerError::InvalidData); } };
}
#[cfg(not(debug_assertions))]
macro_rules! validate {
    ($a:expr) => { if !$a { return Err(DemuxerError::InvalidData); } };
}

#[cfg(feature="demuxer_armovie")]
mod armovie;

const RAW_DEMUXERS: &[&dyn RawDemuxerCreator] = &[
#[cfg(feature="demuxer_armovie")]
    &armovie::ARMovieDemuxerCreator {},
];

/// Registers all available demuxers provided by this crate.
pub fn acorn_register_all_raw_demuxers(rd: &mut RegisteredRawDemuxers) {
    for demuxer in RAW_DEMUXERS.iter() {
        rd.add_demuxer(*demuxer);
    }
}
