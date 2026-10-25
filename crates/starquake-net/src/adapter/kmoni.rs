//! NIED 強震モニタ / S-Net / KMA 测站帧。

use serde_json::Value;
use starquake_core::{
    cn_intensity_text, instrumental_display_text, jma_from_instrumental, IntensityKind,
    StationSample,
};
use tracing::debug;

use crate::bus::{NetEvent, NetTx};

use super::fields::field_f64;

#[derive(Debug, Clone, Default)]
pub struct KmoniState {
    meta: Vec<KmoniMeta>,
    /// S-Net：上次已上送的资料指纹（`time` + int[]）。中转每秒对时帧指纹不变则不上送。
    last_snet_data_fp: u64,
}

#[derive(Debug, Clone)]
struct KmoniMeta {
    code: String,
    name: String,
    lat: f64,
    lon: f64,
}

/// Jian `/kmoni`：首包带 `stations` 元数据，后续 `int[]` 为計測震度（缺测 -3）。
pub fn ingest_kmoni_frame(state: &mut KmoniState, tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if typ == "heartbeat" || typ == "pong" || typ == "error" {
        return;
    }
    if typ != "kmoni" && v.get("int").is_none() && v.get("stations").is_none() {
        return;
    }

    if let Some(arr) = v.get("stations").and_then(|s| s.as_array()) {
        state.meta.clear();
        state.meta.reserve(arr.len());
        for s in arr {
            let code = s
                .get("code")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if code.is_empty() {
                continue;
            }
            let name = s
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or(code.as_str())
                .to_string();
            let lat = field_f64(s, &["lat", "latitude"]).unwrap_or(0.0);
            let lon = field_f64(s, &["lon", "lng", "longitude"]).unwrap_or(0.0);
            state.meta.push(KmoniMeta {
                code,
                name,
                lat,
                lon,
            });
        }
        debug!(n = state.meta.len(), "kmoni: stations meta");
    }

    let Some(ints) = v.get("int").and_then(|i| i.as_array()) else {
        return;
    };
    if state.meta.is_empty() {
        return;
    }

    let mut out = Vec::new();
    let n = ints.len().min(state.meta.len());
    for (i, intv) in ints.iter().enumerate().take(n) {
        let raw = intv.as_f64().unwrap_or(-99.0);
        // < -3 缺测；-3 起全部上图（要石色带最低档深蓝）
        if raw < -3.0 {
            continue;
        }
        let m = &state.meta[i];
        if m.lat.abs() < 1e-6 && m.lon.abs() < 1e-6 {
            continue;
        }
        let (level, _) = jma_from_instrumental(raw);
        out.push(StationSample {
            id: m.code.clone(),
            name: m.name.clone(),
            latitude: m.lat,
            longitude: m.lon,
            intensity_text: instrumental_display_text(raw),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: level,
            instrumental: raw,
        });
    }
    let _ = tx.send(NetEvent::Stations {
        network: "kmoni",
        list: out,
    });
}

/// Jian `/s-net`：首包 `stations[]`；其后中转约每秒重推（steady_push）。
///
/// - 上游 S-Net **资料**约每分钟更新一次（`time` 按分钟对齐，`int[]` 随资料变）
/// - 中转每秒只刷新信封 `timestamp` 作对时/保活；`time`/`int` 不变
/// - 原版 MSIL 无对时帧，仅在 validTime 变化时取色
///
/// 对时帧不上送 `NetEvent::Stations`，只发 `StationClock` 刷新卡片时钟；资料变化才上送列表。
pub fn ingest_snet_frame(state: &mut KmoniState, tx: &NetTx, text: &str) {
    ingest_snet_int_frame(state, tx, text);
}

/// Jian `/kma-station`：`stations` + `mmi[]`（韩国烈度，映射为 CN 档展示）。
pub fn ingest_kma_station_frame(state: &mut KmoniState, tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if typ == "heartbeat" || typ == "pong" || typ == "error" {
        return;
    }
    if let Some(arr) = v.get("stations").and_then(|s| s.as_array()) {
        load_station_meta(state, arr);
    }
    let Some(mmis) = v.get("mmi").and_then(|i| i.as_array()) else {
        return;
    };
    if state.meta.is_empty() {
        return;
    }
    let mut out = Vec::new();
    let n = mmis.len().min(state.meta.len());
    for (i, mmi) in mmis.iter().enumerate().take(n) {
        let raw = mmi.as_f64().unwrap_or(-3.0);
        if raw < 0.0 {
            continue;
        }
        let m = &state.meta[i];
        if m.lat.abs() < 1e-6 && m.lon.abs() < 1e-6 {
            continue;
        }
        let (level, text) = if raw < 1.0 {
            (0_u8, "—".into())
        } else {
            let level = (raw.round() as u8).clamp(1, 12);
            (level, cn_intensity_text(level).into())
        };
        out.push(StationSample {
            id: m.code.clone(),
            name: m.name.clone(),
            latitude: m.lat,
            longitude: m.lon,
            intensity_text: text,
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: level,
            instrumental: raw,
        });
    }
    let _ = tx.send(NetEvent::Stations {
        network: "kma",
        list: out,
    });
}

fn load_station_meta(state: &mut KmoniState, arr: &[Value]) {
    state.meta.clear();
    state.meta.reserve(arr.len());
    for (i, s) in arr.iter().enumerate() {
        let mut code = s
            .get("code")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if code.is_empty() {
            code = format!("{i}");
        }
        let name = s
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or(code.as_str())
            .to_string();
        let lat = field_f64(s, &["lat", "latitude"]).unwrap_or(0.0);
        let lon = field_f64(s, &["lon", "lng", "longitude"]).unwrap_or(0.0);
        state.meta.push(KmoniMeta {
            code,
            name,
            lat,
            lon,
        });
    }
}

fn ingest_snet_int_frame(state: &mut KmoniState, tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if typ == "heartbeat" || typ == "pong" || typ == "error" {
        return;
    }
    if typ != "s-net" && v.get("int").is_none() && v.get("stations").is_none() {
        return;
    }
    if let Some(arr) = v.get("stations").and_then(|s| s.as_array()) {
        load_station_meta(state, arr);
        debug!(n = state.meta.len(), "snet stations meta");
    }
    let Some(ints) = v.get("int").and_then(|i| i.as_array()) else {
        return;
    };
    if state.meta.is_empty() {
        return;
    }

    // 资料指纹：业务 time（分钟级）+ int[]；忽略每秒刷新的 timestamp
    let data_fp = snet_data_fingerprint(&v, ints);
    if state.last_snet_data_fp != 0 && state.last_snet_data_fp == data_fp {
        // 对时重推：不改列表，只刷新卡片时钟（信封 timestamp）
        let ts_ms = envelope_timestamp_ms(&v);
        let _ = tx.send(NetEvent::StationClock {
            network: "snet",
            ts_ms,
        });
        return;
    }

    let mut out = Vec::new();
    let n = ints.len().min(state.meta.len());
    for (i, intv) in ints.iter().enumerate().take(n) {
        let raw = intv.as_f64().unwrap_or(-99.0);
        if raw < -3.0 || raw >= 90.0 {
            continue;
        }
        let m = &state.meta[i];
        if m.lat.abs() < 1e-6 && m.lon.abs() < 1e-6 {
            continue;
        }
        let (level, _) = jma_from_instrumental(raw);
        out.push(StationSample {
            id: m.code.clone(),
            name: m.name.clone(),
            latitude: m.lat,
            longitude: m.lon,
            intensity_text: instrumental_display_text(raw),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: level,
            instrumental: raw,
        });
    }
    state.last_snet_data_fp = data_fp;
    let _ = tx.send(NetEvent::Stations {
        network: "snet",
        list: out,
    });
}

fn envelope_timestamp_ms(v: &Value) -> Option<i64> {
    match v.get("timestamp") {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
    .filter(|t| *t > 1_000_000_000_000) // 毫秒级
}

fn snet_data_fingerprint(v: &Value, ints: &[Value]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match v.get("time") {
        Some(Value::String(s)) => s.hash(&mut h),
        Some(Value::Number(n)) => n.to_string().hash(&mut h),
        _ => 0u8.hash(&mut h),
    }
    ints.len().hash(&mut h);
    for x in ints {
        let bits = x
            .as_f64()
            .map(f64::to_bits)
            .or_else(|| x.as_i64().map(|i| i as u64))
            .unwrap_or(0);
        bits.hash(&mut h);
    }
    h.finish()
}

