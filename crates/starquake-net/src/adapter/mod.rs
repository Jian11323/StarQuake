//! 原始 JSON → core 模型；一源一解析模块。

mod fields;
mod fssn;
mod jian;
mod kmoni;
mod p2p;
mod pancakes;
mod wolfx;

#[cfg(test)]
mod tests;

pub use fssn::ingest_fssn_frame;
pub use jian::ingest_jian_frame;
pub use kmoni::{ingest_kma_station_frame, ingest_kmoni_frame, ingest_snet_frame, KmoniState};
pub use p2p::ingest_p2p_items;
pub use pancakes::ingest_pancakes_event;
pub use wolfx::{
    ingest_wolfx_cwa_eew, ingest_wolfx_eqlist, ingest_wolfx_frame, ingest_wolfx_jma_eew,
};
