//! P2PQuake 速报。

use serde_json::Value;
use starquake_core::{jma_shindo_level, AgencyId, EqRecord, IntensityKind};

use crate::bus::{NetEvent, NetTx};
use crate::time_parse::{format_time_text, parse_time_str};

use super::fields::{
    field_depth_km, field_f64, fill_record_intensity, is_investigating_text,
};

/// P2PQuake JSON API v2：仅 `code` 551 地震情报（海啸见 Jian `jma-tsunami`）。
pub fn ingest_p2p_items(tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let items: Vec<&Value> = if let Some(arr) = v.as_array() {
        arr.iter().collect()
    } else {
        vec![&v]
    };

    for item in items {
        let code = item.get("code").and_then(|c| c.as_u64()).unwrap_or(0);
        if code != 551 {
            continue;
        }
        let Some(eq) = item.get("earthquake") else {
            continue;
        };
        let Some(hypo) = eq.get("hypocenter") else {
            continue;
        };
        // P2P：震源缺测 lat/lon=-200、depth=-1、magnitude=-1（震度速报等）
        let lat_raw = field_f64(hypo, &["latitude", "Latitude"]).unwrap_or(-200.0);
        let lon_raw = field_f64(hypo, &["longitude", "Longitude"]).unwrap_or(-200.0);
        let mag_raw = field_f64(hypo, &["magnitude", "Magnitude"]).unwrap_or(-1.0);
        let depth_raw = field_depth_km(hypo, &["depth", "Depth"]).unwrap_or(-1.0);
        let hypo_unknown = lat_raw <= -180.0 || lon_raw <= -180.0;
        let mag_unknown = mag_raw < 0.0;
        let depth_unknown = depth_raw < 0.0;
        let latitude = if hypo_unknown { 0.0 } else { lat_raw };
        let longitude = if hypo_unknown { 0.0 } else { lon_raw };
        let magnitude = if mag_unknown { 0.0 } else { mag_raw };
        let depth_km = if hypo_unknown || depth_unknown {
            0.0
        } else {
            depth_raw
        };
        let place_raw = hypo
            .get("name")
            .and_then(|x| x.as_str())
            .map(|s| s.trim())
            .unwrap_or("");
        let place = if place_raw.is_empty() || is_investigating_text(place_raw) {
            if hypo_unknown {
                "调查中"
            } else if place_raw.is_empty() {
                "\u{2014}"
            } else {
                place_raw
            }
        } else {
            place_raw
        };
        let max_scale = eq.get("maxScale").and_then(|x| x.as_i64());
        // 无震源、无震级、无有效震度：丢弃
        if hypo_unknown && mag_unknown && max_scale.filter(|&s| s > 0).is_none() {
            continue;
        }
        let raw_intensity = max_scale.and_then(|scale| {
            if scale <= 0 {
                None
            } else {
                Some(starquake_core::jma_shindo_text(jma_shindo_level(&scale.to_string())).to_string())
            }
        });
        let (intensity_text, level) = fill_record_intensity(
            IntensityKind::JmaShindo,
            magnitude,
            depth_km,
            raw_intensity.as_deref(),
            mag_unknown,
        );
        let origin_ms = eq
            .get("time")
            .and_then(|x| x.as_str())
            .and_then(parse_time_str)
            .unwrap_or(0);
        let time_text = eq
            .get("time")
            .and_then(|x| x.as_str())
            .map(|s| {
                parse_time_str(s)
                    .map(format_time_text)
                    .unwrap_or_else(|| s.to_string())
            })
            .unwrap_or_else(|| "\u{2014}".into());

        let _ = tx.send(NetEvent::Record(EqRecord {
            agency: AgencyId("p2pquake".into()),
            place: crate::place::fix_place("p2pquake", place, latitude, longitude),
            time_text,
            magnitude,
            intensity_text,
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: level,
            latitude,
            longitude,
            depth_km,
            origin_ms,
            is_auto: false,
            magnitude_unknown: mag_unknown,
            hypocenter_unknown: hypo_unknown,
        }));
    }
}

