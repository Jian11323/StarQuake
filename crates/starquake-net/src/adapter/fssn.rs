//! FSSN 国际速报。

use serde_json::Value;
use starquake_core::{fill_intensity, AgencyId, EqRecord, IntensityKind};

use crate::bus::{NetEvent, NetTx};
use crate::time_parse::{format_time_text, parse_time_str};

use super::fields::{field_depth_km, field_f64};

/// FSSN `/fssn`：`initial` / `update` / `query_response` 的 `Data` 对象，以及 `fssnlist_response` 数组。
/// 文档：<https://fssn-api.fanstudio.tech/#fssn>
pub fn ingest_fssn_frame(tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    match typ {
        "heartbeat" | "pong" | "error" => (),
        "fssnlist_response" => {
            if let Some(arr) = v.get("Data").and_then(|d| d.as_array()) {
                for item in arr.iter().take(80) {
                    ingest_fssn_item(tx, item);
                }
            }
        }
        _ => {
            let Some(data) = v.get("Data") else {
                return;
            };
            if data.is_object() {
                ingest_fssn_item(tx, data);
            }
        }
    }
}

fn ingest_fssn_item(tx: &NetTx, item: &Value) {
    let Some(magnitude) = field_f64(item, &["magnitude"]) else {
        return;
    };
    if !magnitude.is_finite() {
        return;
    }
    let Some(latitude) = field_f64(item, &["latitude"]) else {
        return;
    };
    let Some(longitude) = field_f64(item, &["longitude"]) else {
        return;
    };
    let Some(depth_km) = field_depth_km(item, &["depth"]) else {
        return;
    };
    let place = item
        .get("placeName_zh")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            item.get("placeName")
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("\u{2014}")
        .to_string();
    let origin_ms = item
        .get("shockTime")
        .and_then(|x| x.as_str())
        .and_then(parse_time_str)
        .unwrap_or(0);
    let time_text = if origin_ms > 0 {
        format_time_text(origin_ms)
    } else {
        item.get("shockTime")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "\u{2014}".into())
    };
    // FSSN 无上游烈度：震中 CSIS 估算（与其它速报 fill_intensity 一致）
    let (intensity_text, intensity_level) =
        fill_intensity(IntensityKind::CnIntensity, magnitude, depth_km, None);
    let _ = tx.send(NetEvent::Record(EqRecord {
        agency: AgencyId("fssn".into()),
        place: crate::place::fix_place("fssn", &place, latitude, longitude),
        time_text,
        magnitude,
        intensity_text,
        intensity_kind: IntensityKind::CnIntensity,
        intensity_level,
        latitude,
        longitude,
        depth_km,
        origin_ms,
        is_auto: false,
        magnitude_unknown: false,
        hypocenter_unknown: false,
    }));
}

