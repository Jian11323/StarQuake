//! Wolfx 多源 WS。

use serde_json::Value;
use starquake_core::{
    fill_intensity, intensity_kind_for_agency, AgencyId, EewReport, EqRecord, IntensityKind,
};

use crate::bus::{NetEvent, NetTx};
use crate::time_parse::{format_time_text, parse_origin_ms, parse_time_str};

use super::fields::{
    eew_is_assumption, eew_is_warn, field_depth_km, field_f64, field_looks_negative,
    field_looks_unknown, fill_record_intensity, flag_true, is_investigating_text,
    json_id_string, json_intensity_raw, record_is_auto,
};

/// Wolfx `all_eew` / 单源 WS：按 `type` 分发；各子源开关见 [`starquake_config::WolfxSourceConfig`]。
pub fn ingest_wolfx_frame(tx: &NetTx, text: &str, cfg: &starquake_config::WolfxSourceConfig) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match typ {
        "heartbeat" | "pong" => {}
        "jma_eew" if cfg.type_enabled("jma_eew") => emit_wolfx_eew(tx, &v, "wolfx-jma-eew"),
        "sc_eew" if cfg.type_enabled("sc_eew") => emit_wolfx_eew(tx, &v, "wolfx-cea-pr:四川"),
        "fj_eew" if cfg.type_enabled("fj_eew") => emit_wolfx_eew(tx, &v, "wolfx-cea-pr:福建"),
        "cq_eew" if cfg.type_enabled("cq_eew") => emit_wolfx_eew(tx, &v, "wolfx-cea-pr:重庆"),
        "cenc_eew" if cfg.type_enabled("cenc_eew") => emit_wolfx_eew(tx, &v, "wolfx-cea"),
        "cwa_eew" if cfg.type_enabled("cwa_eew") => emit_wolfx_eew(tx, &v, "wolfx-cwa-eew"),
        "cenc_eqlist" if cfg.type_enabled("cenc_eqlist") => ingest_wolfx_cenc_eqlist_value(tx, &v),
        "jma_eqlist" if cfg.type_enabled("jma_eqlist") => ingest_wolfx_jma_eqlist_value(tx, &v),
        // 专用 `cwa_eew` 通道可能无 type：HypoCenter + ReportNum（区别于 JMA 的 Hypocenter + Serial）
        "" if cfg.type_enabled("cwa_eew")
            && v.get("HypoCenter").is_some()
            && v.get("ReportNum").is_some() =>
        {
            emit_wolfx_eew(tx, &v, "wolfx-cwa-eew");
        }
        // 旧版单通道 jma_eew 可能省略 type
        "" if cfg.type_enabled("jma_eew")
            && v.get("Hypocenter").is_some()
            && v.get("Serial").is_some() =>
        {
            emit_wolfx_eew(tx, &v, "wolfx-jma-eew");
        }

        _ => {}
    }
}

/// 兼容旧调用：仅解析 `jma_eew`（或无 type 的 JMA 形扁平包）。
pub fn ingest_wolfx_jma_eew(tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if typ == "heartbeat" || typ == "pong" {
        return;
    }
    if typ == "jma_eew" || typ.is_empty() {
        emit_wolfx_eew(tx, &v, "wolfx-jma-eew");
    }
}

/// 专用 `cwa_eew` WebSocket 文本帧。
pub fn ingest_wolfx_cwa_eew(tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if typ == "heartbeat" || typ == "pong" {
        return;
    }
    emit_wolfx_eew(tx, &v, "wolfx-cwa-eew");
}

fn emit_wolfx_eew(tx: &NetTx, v: &Value, agency: &str) {
    // 心跳有时只有简短对象
    if v.get("EventID").is_none()
        && v.get("Hypocenter").is_none()
        && v.get("HypoCenter").is_none()
    {
        return;
    }
    let place = v
        .get("Hypocenter")
        .or_else(|| v.get("HypoCenter"))
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".into());
    let event_id =
        json_id_string(v, &["EventID", "ID", "id"]).unwrap_or_else(|| "wolfx-unknown".into());
    let serial = v
        .get("Serial")
        .or_else(|| v.get("ReportNum"))
        .and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(1) as u32;
    let is_cancel = flag_true(
        v,
        &["isCancel", "isCanceled", "isCancelled", "canceled", "cancelled", "is_cancel"],
    ) || v
        .get("Type")
        .and_then(|x| x.as_str())
        .is_some_and(|s| s.contains("取消"));
    // Magnitude 优先；Magunitude 仅为兼容旧拼写。取消报允许缺经纬/震级/深度（对齐 Jian）。
    let magnitude = match field_f64(v, &["Magnitude", "magnitude", "Magunitude"]) {
        Some(m) => m,
        None if is_cancel => 0.0,
        None => return,
    };
    let depth_km = match field_depth_km(v, &["Depth", "depth"]) {
        Some(d) => d,
        None if is_cancel => 0.0,
        None => return,
    };
    let latitude = match field_f64(v, &["Latitude", "latitude"]) {
        Some(x) => x,
        None if is_cancel => 0.0,
        None => return,
    };
    let longitude = match field_f64(v, &["Longitude", "longitude"]) {
        Some(x) => x,
        None if is_cancel => 0.0,
        None => return,
    };
    let origin_ms = v
        .get("OriginTime")
        .or_else(|| v.get("originTime"))
        .and_then(|x| match x {
            Value::String(s) => parse_time_str(s),
            other => {
                let ms = parse_origin_ms(other);
                if ms > 0 {
                    Some(ms)
                } else {
                    None
                }
            }
        })
        .unwrap_or(0);

    let kind = intensity_kind_for_agency(agency);
    let raw = json_intensity_raw(v, &["MaxIntensity", "maxIntensity", "intensity"]);
    let (max_intensity_text, intensity_level) =
        fill_intensity(kind, magnitude, depth_km, raw.as_deref());

    let is_final = !is_cancel
        && (flag_true(v, &["isFinal", "is_final", "Final"])
            || v.get("Type")
                .and_then(|x| x.as_str())
                .is_some_and(|s| s.contains("最終") || s.contains("最终")));

    let is_assumption = eew_is_assumption(v, &place);
    let is_warn = eew_is_warn(v, intensity_level, kind);
    let _ = tx.send(NetEvent::Eew(EewReport {
        agency: AgencyId(agency.into()),
        event_id,
        serial,
        place: crate::place::fix_place(agency, &place, latitude, longitude),
        magnitude,
        depth_km,
        latitude,
        longitude,
        origin_ms,
        max_intensity_text,
        intensity_kind: kind,
        intensity_level,
        is_final,
        is_cancel,
        is_assumption,
        is_warn,
        final_at_ms: 0,
    }));
}

/// Wolfx `cenc_eqlist`（HTTP JSON 或 WS）：对象键为 No1..NoN 或数组。
/// 注意：官方字段 `magnitude` / `latitude` / `longitude` / `depth` / `intensity` 多为**字符串**。
pub fn ingest_wolfx_eqlist(tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    ingest_wolfx_cenc_eqlist_value(tx, &v);
}

fn wolfx_eqlist_items(v: &Value) -> Vec<&Value> {
    if let Some(arr) = v.as_array() {
        return arr.iter().collect();
    }
    let Some(obj) = v.as_object() else {
        return Vec::new();
    };
    let mut pairs: Vec<_> = obj
        .iter()
        .filter(|(k, _)| k.starts_with("No") || k.starts_with("no"))
        .collect();
    // No10 须排在 No2 之后：按数字后缀排序
    pairs.sort_by(|(a, _), (b, _)| {
        let na = a
            .trim_start_matches(['N', 'n', 'O', 'o'])
            .parse::<u32>()
            .unwrap_or(0);
        let nb = b
            .trim_start_matches(['N', 'n', 'O', 'o'])
            .parse::<u32>()
            .unwrap_or(0);
        na.cmp(&nb)
    });
    pairs.into_iter().map(|(_, val)| val).collect()
}

fn ingest_wolfx_cenc_eqlist_value(tx: &NetTx, v: &Value) {
    for item in wolfx_eqlist_items(v).into_iter().take(50) {
        let place = item
            .get("placeName")
            .or_else(|| item.get("location"))
            .or_else(|| item.get("place"))
            .or_else(|| item.get("Place"))
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".into());
        let Some(magnitude) = field_f64(item, &["magnitude", "Magnitude"]) else {
            continue;
        };
        let Some(latitude) = field_f64(item, &["latitude", "Latitude"]) else {
            continue;
        };
        let Some(longitude) = field_f64(item, &["longitude", "Longitude"]) else {
            continue;
        };
        let Some(depth_km) = field_depth_km(item, &["depth", "Depth"]) else {
            continue;
        };
        let origin_ms = item
            .get("time")
            .or_else(|| item.get("Time"))
            .and_then(|x| x.as_str())
            .and_then(parse_time_str)
            .unwrap_or(0);
        let time_text = item
            .get("time")
            .or_else(|| item.get("Time"))
            .and_then(|x| x.as_str())
            .map(|s| {
                parse_time_str(s)
                    .map(format_time_text)
                    .unwrap_or_else(|| s.to_string())
            })
            .unwrap_or_else(|| "—".into());

        let raw = json_intensity_raw(item, &["intensity", "maxIntensity", "MaxIntensity"]);
        let (intensity_text, intensity_level) = fill_intensity(
            IntensityKind::CnIntensity,
            magnitude,
            depth_km,
            raw.as_deref(),
        );
        let is_auto = record_is_auto("wolfx-cenc", item);

        let _ = tx.send(NetEvent::Record(EqRecord {
            agency: AgencyId("wolfx-cenc".into()),
            place: crate::place::fix_place("wolfx-cenc", &place, latitude, longitude),
            time_text,
            magnitude,
            intensity_text,
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level,
            latitude,
            longitude,
            depth_km,
            origin_ms,
            is_auto,
            magnitude_unknown: false,
            hypocenter_unknown: false,
        }));
    }
}

fn ingest_wolfx_jma_eqlist_value(tx: &NetTx, v: &Value) {
    for item in wolfx_eqlist_items(v).into_iter().take(50) {
        let place_raw = item
            .get("location")
            .or_else(|| item.get("place"))
            .or_else(|| item.get("Hypocenter"))
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mag_unknown = field_looks_unknown(item, &["magnitude", "Magnitude"])
            || field_looks_negative(item, &["magnitude", "Magnitude"]);
        let hypo_unknown = field_looks_unknown(item, &["latitude", "Latitude", "longitude", "Longitude"])
            || place_raw
                .as_deref()
                .is_some_and(|p| is_investigating_text(p) || p.contains("調査中") || p.contains("调查中"));
        let magnitude = match field_f64(item, &["magnitude", "Magnitude"]) {
            Some(m) if m >= 0.0 && !mag_unknown => m,
            _ if mag_unknown || hypo_unknown => 0.0,
            _ => continue,
        };
        let latitude = match field_f64(item, &["latitude", "Latitude"]) {
            Some(v) if !hypo_unknown => v,
            _ if hypo_unknown => 0.0,
            _ => continue,
        };
        let longitude = match field_f64(item, &["longitude", "Longitude"]) {
            Some(v) if !hypo_unknown => v,
            _ if hypo_unknown => 0.0,
            _ => continue,
        };
        let depth_km = match field_depth_km(item, &["depth", "Depth"]) {
            Some(v) if v >= 0.0 && !hypo_unknown => v,
            _ if hypo_unknown => 0.0,
            Some(v) => v,
            None => continue,
        };
        let place = match place_raw {
            Some(p) if !is_investigating_text(&p) => p,
            _ if hypo_unknown => "调查中".into(),
            Some(p) => p,
            None => "\u{2014}".into(),
        };
        let origin_ms = item
            .get("time")
            .or_else(|| item.get("Time"))
            .and_then(|x| x.as_str())
            .and_then(parse_time_str)
            .unwrap_or(0);
        let time_text = item
            .get("time")
            .or_else(|| item.get("Time"))
            .and_then(|x| x.as_str())
            .map(|s| {
                parse_time_str(s)
                    .map(format_time_text)
                    .unwrap_or_else(|| s.to_string())
            })
            .unwrap_or_else(|| "\u{2014}".into());

        let raw = json_intensity_raw(item, &["shindo", "MaxIntensity", "intensity"]);
        // 无震度且震级/震源皆调查中：仍入库（震度速报占位），不丢弃
        if mag_unknown
            && hypo_unknown
            && raw
                .as_deref()
                .map(|s| s.trim().is_empty() || is_investigating_text(s))
                .unwrap_or(true)
        {
            // 至少要有发震时刻，否则无展示价值
            if origin_ms <= 0 {
                continue;
            }
        }
        let (intensity_text, intensity_level) = fill_record_intensity(
            IntensityKind::JmaShindo,
            magnitude,
            depth_km,
            raw.as_deref(),
            mag_unknown,
        );

        let _ = tx.send(NetEvent::Record(EqRecord {
            agency: AgencyId("wolfx-jma".into()),
            place: crate::place::fix_place("wolfx-jma", &place, latitude, longitude),
            time_text,
            magnitude,
            intensity_text,
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level,
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

