//! PancakesAPI（GQ 速报、JMA 火山等）。

use serde_json::Value;
use starquake_core::{fill_intensity, AgencyId, EewReport, IntensityKind};

use crate::bus::{NetEvent, NetTx};
use crate::time_parse::{format_time_text, parse_time_str};

use super::fields::{field_depth_km, field_f64, json_intensity_raw};

/// PancakesAPI RealtimeEvent：`{ source, type, action, timestampMs, payload }`。
pub fn ingest_pancakes_event(tx: &NetTx, text: &str, cfg: &starquake_config::PancakesSourceConfig) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let source = v.get("source").and_then(|x| x.as_str()).unwrap_or("");
    if source.is_empty() {
        return;
    }
    if !pancakes_source_enabled(cfg, source) {
        return;
    }
    let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("update");
    let payload = v.get("payload").cloned().unwrap_or(v.clone());

    match source {
        "gq" => ingest_pancakes_gq(tx, action, &payload, cfg.gq_min_magnitude),
        "jma_volcano" => ingest_pancakes_volcano(tx, &payload),
        _ => {}
    }
}

fn pancakes_source_enabled(cfg: &starquake_config::PancakesSourceConfig, source: &str) -> bool {
    match source {
        "gq" => cfg.gq,
        "jma_volcano" => cfg.jma_volcano,
        _ => false,
    }
}



fn pancakes_id(payload: &Value, keys: &[&str]) -> String {
    let mut weak_numeric: Option<String> = None;
    for k in keys {
        let Some(v) = payload.get(*k) else {
            continue;
        };
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                return s.to_string();
            }
        }
        // 小数/报数级整数常是 revision，不能当 event 主键
        if let Some(n) = v.as_u64() {
            if n >= 100_000 {
                return n.to_string();
            }
            weak_numeric.get_or_insert_with(|| n.to_string());
            continue;
        }
        if let Some(n) = v.as_i64() {
            if n >= 100_000 {
                return n.to_string();
            }
            if n > 0 {
                weak_numeric.get_or_insert_with(|| n.to_string());
            }
            continue;
        }
        if let Some(n) = v.as_f64() {
            if n.is_finite() && n.abs() >= 100_000.0 {
                return format!("{n:.0}");
            }
        }
    }
    // 勿回落成固定 "unknown"，否则同源多震会互相顶掉
    let lat = field_f64(payload, &["latitude", "Latitude"]).unwrap_or(0.0);
    let lon = field_f64(payload, &["longitude", "Longitude"]).unwrap_or(0.0);
    let t = pancakes_origin_ms(payload);
    if lat.abs() > 0.01 || lon.abs() > 0.01 || t > 0 {
        return format!("anon:{t}:{lat:.3}:{lon:.3}");
    }
    weak_numeric.unwrap_or_else(|| "unknown".into())
}

fn pancakes_origin_ms(payload: &Value) -> i64 {
    if let Some(n) = payload
        .get("originTimeMs")
        .and_then(|x| x.as_i64())
        .or_else(|| payload.get("originTimeMs").and_then(|x| x.as_u64()).map(|u| u as i64))
    {
        return n;
    }
    if let Some(s) = payload
        .get("originTimeIso")
        .or_else(|| payload.get("originTime"))
        .or_else(|| payload.get("OriginTime"))
        .or_else(|| payload.get("time"))
        .or_else(|| payload.get("Time"))
        .or_else(|| payload.get("issuedAt"))
        .or_else(|| payload.get("issueTime"))
        .or_else(|| payload.get("effective"))
        .and_then(|x| x.as_str())
    {
        if let Some(ms) = parse_time_str(s) {
            return ms;
        }
    }
    if let Some(n) = payload.get("originTime").and_then(|x| x.as_i64()) {
        return n;
    }
    0
}

fn ingest_pancakes_gq(tx: &NetTx, action: &str, payload: &Value, min_mag: f64) {
    // 优先 eventId：上游 id 常为报数/修订号，作主键会把同源多震互相顶掉
    let id = pancakes_id(payload, &["eventId", "EventID", "id"]);
    if action == "cancelled" || action == "canceled" {
        let _ = tx.send(NetEvent::Eew(EewReport {
            agency: AgencyId("gq".into()),
            event_id: id,
            serial: u32::MAX,
            place: "—".into(),
            magnitude: 0.0,
            depth_km: 0.0,
            latitude: 0.0,
            longitude: 0.0,
            origin_ms: 0,
            max_intensity_text: "—".into(),
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: 0,
            is_final: true,
            is_cancel: true,
            is_assumption: false,
            is_warn: false,
            final_at_ms: 0,
        }));
        return;
    }
    let Some(magnitude) = field_f64(payload, &["magnitude"]) else {
        return;
    };
    if min_mag > 0.0 && magnitude + 1e-9 < min_mag {
        return;
    }
    let Some(latitude) = field_f64(payload, &["latitude"]) else {
        return;
    };
    let Some(longitude) = field_f64(payload, &["longitude"]) else {
        return;
    };
    let Some(depth_km) = field_depth_km(payload, &["depth", "depthKm"]) else {
        return;
    };
    let place = payload
        .get("region")
        .or_else(|| payload.get("place"))
        .and_then(|x| x.as_str())
        .unwrap_or("GlobalQuake")
        .to_string();
    // GQ 报数 = revisionId（修订版本号）；updates / reportNum 作兼容回退
    let serial = payload
        .get("revisionId")
        .or_else(|| payload.get("updates"))
        .or_else(|| payload.get("reportNum"))
        .or_else(|| payload.get("revision"))
        .and_then(|x| x.as_u64().or_else(|| x.as_i64().map(|i| i as u64)))
        .map(|n| n as u32)
        .filter(|&n| n > 0 && n < 10_000)
        .unwrap_or(1);
    let raw = json_intensity_raw(payload, &["intensity", "maxIntensity"]);
    // GQ 无上游烈度时不估算（避免 M1 级被估成烈度 2）
    let (max_intensity_text, intensity_level) = match raw.as_deref() {
        Some(s) if !s.is_empty() && s != "—" && s != "-" && s != "null" => {
            fill_intensity(IntensityKind::CnIntensity, magnitude, depth_km, Some(s))
        }
        _ => ("—".into(), 0),
    };
    let archived = action == "archived"
        || payload
            .get("status")
            .and_then(|x| x.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("archived"));
    let _ = tx.send(NetEvent::Eew(EewReport {
        agency: AgencyId("gq".into()),
        event_id: id,
        serial,
        place: crate::place::fix_place("gq", &place, latitude, longitude),
        magnitude,
        depth_km,
        latitude,
        longitude,
        origin_ms: pancakes_origin_ms(payload),
        max_intensity_text,
        intensity_kind: IntensityKind::CnIntensity,
        intensity_level,
        is_final: archived,
        is_cancel: false,
        is_assumption: false,
        is_warn: false,
        final_at_ms: 0,
    }));
}

fn ingest_pancakes_volcano(tx: &NetTx, payload: &Value) {
    let title = strip_jma_volcano_name_label(
        payload
            .get("title")
            .or_else(|| payload.get("headline"))
            .and_then(|x| x.as_str())
            .unwrap_or("\u{706b}\u{5c71}\u{60c5}\u{62a5}"),
    );
    let issued_ms = payload
        .get("announcedTime")
        .or_else(|| payload.get("issuedAt"))
        .and_then(|x| x.as_str())
        .and_then(parse_time_str)
        .unwrap_or_else(|| pancakes_origin_ms(payload));
    let name = payload
        .get("volcanoName")
        .or_else(|| payload.get("name"))
        .and_then(|x| x.as_str())
        .map(strip_jma_volcano_name_label)
        .or_else(|| {
            payload
                .get("areas")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("name"))
                .and_then(|x| x.as_str())
                .map(strip_jma_volcano_name_label)
        })
        .unwrap_or_else(|| title.clone());
    let mut latitude = field_f64(payload, &["latitude", "Latitude", "lat"]).unwrap_or(0.0);
    let mut longitude = field_f64(payload, &["longitude", "Longitude", "lon", "lng"]).unwrap_or(0.0);
    if latitude.abs() <= 0.01 && longitude.abs() <= 0.01 {
        if let Some(arr) = payload.get("areas").and_then(|a| a.as_array()) {
            for a in arr {
                let la = field_f64(a, &["latitude", "Latitude", "lat"]).unwrap_or(0.0);
                let lo = field_f64(a, &["longitude", "Longitude", "lon", "lng"]).unwrap_or(0.0);
                if la.abs() > 0.01 || lo.abs() > 0.01 {
                    latitude = la;
                    longitude = lo;
                    break;
                }
            }
        }
    }
    if latitude.abs() <= 0.01 && longitude.abs() <= 0.01 {
        if let Some((la, lo)) = jma_volcano_coords(&name).or_else(|| jma_volcano_coords(&title)) {
            latitude = la;
            longitude = lo;
        }
    }
    let _ = tx.send(NetEvent::Volcano(starquake_core::VolcanoNotice {
        agency: AgencyId("jma-volcano".into()),
        name,
        title,
        time_text: format_time_text(issued_ms),
        issued_ms,
        latitude,
        longitude,
    }));
}

/// JMA Atom 标题常带字段标签「火山名」；列表只展示火山名与情报类型。
pub(crate) fn strip_jma_volcano_name_label(s: &str) -> String {
    let t = s.trim();
    // 「火山名」
    const LABEL: &str = "\u{706b}\u{5c71}\u{540d}";
    let rest = match t.strip_prefix(LABEL) {
        Some(r) => r,
        None => return t.to_string(),
    };
    rest.trim_start_matches(|c: char| {
        c.is_whitespace()
            || c == '\u{3000}'
            || c == '\u{00a0}'
            || c == '\u{ff1a}'
            || c == ':'
            || c == '\u{30fb}'
            || c == '\u{00b7}'
    })
    .to_string()
}

fn jma_volcano_coords(name: &str) -> Option<(f64, f64)> {
    const TABLE: &[(&str, f64, f64)] = &[
        ("桜島", 31.593, 130.657),
        ("薩摩硫黄島", 30.793, 130.305),
        ("口永良部島", 30.447, 130.217),
        ("諏訪之瀬島", 29.638, 129.716),
        ("阿蘇山", 32.884, 131.104),
        ("霧島山", 31.934, 130.862),
        ("新燃岳", 31.909, 130.886),
        ("十勝岳", 43.418, 142.686),
        ("樽前山", 42.690, 141.380),
        ("俱多楽", 42.488, 141.157),
        ("北海道駒ヶ岳", 42.063, 140.677),
        ("有珠山", 42.157, 140.412),
        ("白山", 36.135, 138.523),
        ("吾妻山", 37.647, 140.081),
        ("草津白根山", 36.650, 139.233),
        ("那須岳", 36.864, 138.114),
        ("富士山", 35.363, 138.731),
        ("伊豆大島", 34.724, 139.004),
        ("三宅島", 34.736, 139.395),
        ("大島", 34.722, 139.398),
        ("三原山", 34.217, 139.150),
        ("硫黄島", 30.793, 130.305),
        ("桜島", 31.593, 130.657),
    ];
    let n = name.trim();
    for (k, la, lo) in TABLE {
        if n.contains(k) {
            return Some((*la, *lo));
        }
    }
    None
}

