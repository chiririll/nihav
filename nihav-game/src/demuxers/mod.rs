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

#[cfg(any(feature="demuxer_bmv",feature="demuxer_bmv3"))]
mod bmv;
#[cfg(any(feature="demuxer_fst",feature="demuxer_fcmp"))]
mod fst;
#[cfg(feature="demuxer_gdv")]
mod gdv;
#[cfg(feature="demuxer_q")]
mod q;
#[cfg(feature="demuxer_rbt")]
mod rbt;
#[cfg(feature="demuxer_seq")]
mod seq;
#[cfg(feature="demuxer_sga")]
mod sga;
#[cfg(feature="demuxer_siff")]
mod siff;
#[cfg(feature="demuxer_smush")]
mod smush;
#[cfg(feature="demuxer_vmd")]
mod vmd;
#[cfg(feature="demuxer_vx")]
mod vx;

const GAME_DEMUXERS: &[&dyn DemuxerCreator] = &[
#[cfg(feature="demuxer_bmv")]
    &bmv::BMVDemuxerCreator {},
#[cfg(feature="demuxer_bmv3")]
    &bmv::BMV3DemuxerCreator {},
#[cfg(feature="demuxer_fcmp")]
    &fst::FCMPDemuxerCreator {},
#[cfg(feature="demuxer_fst")]
    &fst::FSTDemuxerCreator {},
#[cfg(feature="demuxer_gdv")]
    &gdv::GDVDemuxerCreator {},
#[cfg(feature="demuxer_q")]
    &q::QDemuxerCreator {},
#[cfg(feature="demuxer_rbt")]
    &rbt::RobotDemuxerCreator {},
#[cfg(feature="demuxer_seq")]
    &seq::SequenceDemuxerCreator {},
#[cfg(feature="demuxer_sga")]
    &sga::SGADemuxerCreator {},
#[cfg(feature="demuxer_siff")]
    &siff::SIFFDemuxerCreator {},
#[cfg(feature="demuxer_smush")]
    &smush::SmushDemuxerCreator {},
#[cfg(feature="demuxer_smush")]
    &smush::MCMPDemuxerCreator {},
#[cfg(feature="demuxer_vmd")]
    &vmd::VMDDemuxerCreator {},
#[cfg(feature="demuxer_vx")]
    &vx::VXDemuxerCreator {},
];

/// Registers all available demuxers provided by this crate.
pub fn game_register_all_demuxers(rd: &mut RegisteredDemuxers) {
    for demuxer in GAME_DEMUXERS.iter() {
        rd.add_demuxer(*demuxer);
    }
}
