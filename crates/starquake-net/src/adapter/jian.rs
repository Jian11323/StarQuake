//! Jian Project 统一 WS（预警 / 速报 / 海啸 / 列表）。

use serde_json::Value;
use starquake_core::{
    agency_family, cea_pr_agency_id, fill_intensity, intensity_kind_for_agency, AgencyId,
    EewReport, EqRecord, TsunamiArea, TsunamiInfo,
};
use tracing::debug;

use crate::bus::{NetEvent, NetTx};
use crate::time_parse::{format_time_text, parse_origin_ms, parse_time_str};

use super::fields::{
    eew_is_assumption, eew_is_warn, field_depth_km, field_f64, field_looks_negative,
    field_looks_unknown, fill_record_intensity, flag_true, is_investigating_text,
    json_id_string, json_intensity_raw,
    record_is_auto,
};

/// Jian `/all` 或单源信封：`{ type, Data, md5 }` 或 `source：xxx`
/// `cfg` 为 `None` 时不按源开关过滤（单测）。
pub fn ingest_jian_frame(tx: &NetTx, text: &str, cfg: Option<&starquake_config::JianSourceConfig>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if typ == "heartbeat" || typ == "pong" {
        return;
    }
    if typ == "error" {
        let msg = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("(no message)");
        tracing::warn!(
            message = %starquake_util::redact_secrets(msg),
            "jian: error 帧（常见原因：同令牌/同 IP 并发已满，白名单不增加并发额度）"
        );
        return;
    }

    // /all 快照：键形如 "source：cenc"（仅 type=all；勿用 source 前缀误伤其它帧）
    if v.get("type").and_then(|t| t.as_str()) == Some("all") {
        if let Some(obj) = v.as_object() {
            for (k, entry) in obj {
                if !k.starts_with("source") {
                    continue;
                }
                let agency = agency_from_source_key(k);
                let data = entry.get("Data").cloned().unwrap_or_else(|| entry.clone());
                dispatch_jian_data(tx, agency, &data, cfg);
            }
        }
        return;
    }

    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if typ.ends_with("list_response") {
        // strip_suffix：trim_end_matches 按字符集剥离，会把 early-est 裁成 early-
        let agency = agency_from_list_response(typ);
        if let Some(arr) = v.get("Data").and_then(|d| d.as_array()) {
            for item in arr {
                dispatch_jian_data(tx, agency, item, cfg);
            }
        }
        return;
    }

    let data = v.get("Data").cloned().unwrap_or(v.clone());
    dispatch_jian_data(tx, typ, &data, cfg);
}

fn agency_from_source_key(k: &str) -> &str {
    // source：jma-eew / source:jma-eew / source：cea-pr:四川
    if let Some(rest) = k.strip_prefix("source：").or_else(|| k.strip_prefix("source:")) {
        return rest;
    }
    k.split('：').nth(1).or_else(|| k.split(':').nth(1)).unwrap_or(k)
}

fn dispatch_jian_data(
    tx: &NetTx,
    agency: &str,
    data: &Value,
    cfg: Option<&starquake_config::JianSourceConfig>,
) {
    if data.is_null() || !data.is_object() {
        return;
    }
    let agency = agency.trim();
    if is_tsunami_agency(agency) {
        if let Some(ts) = parse_jian_tsunami(agency, data) {
            let _ = tx.send(NetEvent::Tsunami(ts));
        }
        return;
    }
    if is_eew_agency(agency) {
        if let Some(c) = cfg {
            if !jian_accepts_eew(c, agency) {
                return;
            }
        }
        if let Some(eew) = parse_jian_eew(agency, data) {
            let _ = tx.send(NetEvent::Eew(eew));
        } else {
            tracing::warn!(agency, "jian: EEW 帧未能归一化（缺 placeName/经纬/震级？）");
        }
    } else if let Some(rec) = parse_jian_record(agency, data) {
        let _ = tx.send(NetEvent::Record(rec));
    } else {
        debug!(agency, "jian: 未能归一化帧");
    }
}

fn jian_accepts_eew(cfg: &starquake_config::JianSourceConfig, agency: &str) -> bool {
    use starquake_core::AgencyFamily;
    match agency_family(agency) {
        AgencyFamily::Jma => cfg.jma_eew,
        AgencyFamily::Cea => cfg.cea_eew,
        AgencyFamily::Cwa => cfg.cwa_eew,
        AgencyFamily::Kma => cfg.kma_eew,
        AgencyFamily::Sa => cfg.sa_eew,
        AgencyFamily::EarlyEst => cfg.early_est_eew,
        _ => true,
    }
}

pub(crate) fn agency_from_list_response(typ: &str) -> &str {
    typ.strip_suffix("list_response")
        .map(|s| s.strip_suffix('_').unwrap_or(s))
        .unwrap_or(typ)
}

fn is_tsunami_agency(agency: &str) -> bool {
    // 仅 JMA 海啸进顶条+沿岸描边；NMEFC/CENC 等只是报文，不进海啸系统
    let a = agency.trim().to_ascii_lowercase();
    a == "jma-tsunami" || a == "jma_tsunami" || a.ends_with("jma-tsunami")
}

fn parse_jian_tsunami(agency: &str, data: &Value) -> Option<TsunamiInfo> {
    // 对齐 api/jma-tsunami.php：infoTypeName=発表/訂正/取消；cancel=整报取消；
    // cleared=至少一个 area[].cleared；area 为预报区对象数组。
    let info_type = data
        .get("infoTypeName")
        .or_else(|| data.get("infoType"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let title_raw = data
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let level = data.get("level").and_then(|v| v.as_str()).unwrap_or("");
    let cancel_flag = flag_true(data, &["cancel", "isCancel", "isCanceled", "isCancelled"])
        || info_type.contains("取消")
        || title_raw.contains("取消");
    let (areas_list, active_count) = parse_jian_tsunami_area_list(agency, data);
    let cleared_flag = flag_true(data, &["cleared"]);
    // 整报取消，或预报区全解除/失効 → 顶条清除
    let cancelled = cancel_flag
        || (cleared_flag && active_count == 0)
        || (!areas_list.is_empty() && active_count == 0)
        || level.contains("解除")
        || (title_raw.contains("解除") && active_count == 0);
    let grade = if cancelled {
        "Cancel".into()
    } else if !areas_list.is_empty() {
        jian_tsunami_grade_from_areas(&areas_list, level, &title_raw)
    } else {
        jian_tsunami_grade(agency, level, &title_raw)
    };
    let zh_title = if cancelled {
        "海啸情报解除".into()
    } else {
        match grade.as_str() {
            "MajorWarning" => "大海啸警报".into(),
            "Warning" => "海啸警报".into(),
            "Watch" => "海啸注意报".into(),
            _ => "海啸预报".into(),
        }
    };
    let areas_summary = if areas_list.is_empty() {
        "—".into()
    } else {
        areas_list
            .iter()
            .take(8)
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join("\u{3001}")
    };
    let issued_ms = data
        .get("originTime")
        .map(|v| {
            if let Some(n) = v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)) {
                normalize_epoch_ms(n)
            } else if let Some(n) = v.as_f64() {
                normalize_epoch_ms(n as i64)
            } else {
                parse_origin_ms(v)
            }
        })
        .unwrap_or(0);
    let expire_ms = parse_tsunami_expire_ms(data);
    Some(TsunamiInfo {
        title: zh_title,
        grade,
        areas: areas_summary,
        areas_list,
        cancelled,
        issued_ms,
        source: "jian".into(),
        source_priority: 3,
        expire_ms,
    })
}

fn normalize_epoch_ms(n: i64) -> i64 {
    if n <= 0 {
        0
    } else if n > 1_000_000_000_000 {
        n // already ms
    } else if n > 1_000_000_000 {
        n * 1000 // seconds
    } else {
        0
    }
}

fn parse_tsunami_expire_ms(data: &Value) -> i64 {
    for key in [
        "endTime",
        "expireTime",
        "expire_ms",
        "validUntil",
        "until",
        "cancelTime",
    ] {
        if let Some(v) = data.get(key) {
            if let Some(n) = v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)) {
                let ms = normalize_epoch_ms(n);
                if ms > 0 {
                    return ms;
                }
            }
            if let Some(n) = v.as_f64() {
                let ms = normalize_epoch_ms(n as i64);
                if ms > 0 {
                    return ms;
                }
            }
            if let Some(s) = v.as_str() {
                if let Some(ms) = parse_time_str(s) {
                    return ms;
                }
            }
        }
    }
    if let Some(desc) = data
        .get("description")
        .or_else(|| data.get("text"))
        .or_else(|| data.get("content"))
        .and_then(|x| x.as_str())
    {
        for marker in ["有效期至", "解除予定", "解除予定時刻", "まで有効"] {
            if let Some(idx) = desc.find(marker) {
                let digits: String = desc[idx..]
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .take(14)
                    .collect();
                if digits.len() >= 12 {
                    let padded = format!("{:0<14}", &digits[..digits.len().min(14)]);
                    if let Some(ms) = parse_time_str(&padded) {
                        return ms;
                    }
                }
            }
        }
    }
    0
}


fn jian_tsunami_grade(_agency: &str, level: &str, title: &str) -> String {
    tsunami_kind_to_grade(&format!("{level} {title}"))
}

fn jian_tsunami_grade_from_areas(areas: &[TsunamiArea], level: &str, title: &str) -> String {
    let mut best = 0_u8;
    let mut grade = "Forecast".to_string();
    for a in areas {
        let r = tsunami_grade_rank(&a.grade);
        if r > best {
            best = r;
            grade = a.grade.clone();
        }
    }
    if best == 0 {
        return jian_tsunami_grade("", level, title);
    }
    grade
}

fn tsunami_kind_to_grade(kind: &str) -> String {
    if kind.contains("大津波") || kind.contains("大海啸") {
        return "MajorWarning".into();
    }
    if kind.contains("津波警報")
        || kind.contains("海啸警报")
        || kind.contains("警報")
        || kind.contains("警报")
        || kind.contains("Warning")
    {
        return "Warning".into();
    }
    if kind.contains("注意") || kind.contains("Watch") || kind.contains("Advisory") {
        return "Watch".into();
    }
    "Forecast".into()
}

fn tsunami_grade_rank(grade: &str) -> u8 {
    match grade {
        "MajorWarning" | "Major" => 4,
        "Warning" => 3,
        "Watch" => 2,
        "Forecast" | "forecast" => 1,
        _ => 0,
    }
}

fn area_item_cleared(a: &Value) -> bool {
    if flag_true(a, &["cleared"]) {
        return true;
    }
    let kind = a.get("kind").and_then(|x| x.as_str()).unwrap_or("");
    let kind_code = a
        .get("kindCode")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    kind == "津波なし"
        || kind_code == "00"
        || kind.contains("解除")
        || kind.contains("切替")
        || kind.contains("失効")
}

/// 解析 `area[]` 对象数组（新格式）；兼容旧版字符串 `area`。
/// 返回 (未解除区列表, 未解除区数量)。
fn parse_jian_tsunami_area_list(agency: &str, data: &Value) -> (Vec<TsunamiArea>, usize) {
    let a = agency.trim().to_ascii_lowercase();
    if !(a.starts_with("jma") || a.contains("jma-tsunami") || a.contains("jma_tsunami")) {
        return (Vec::new(), 0);
    }
    if let Some(arr) = data.get("area").and_then(|v| v.as_array()) {
        let mut out: Vec<TsunamiArea> = Vec::new();
        let mut active = 0usize;
        for item in arr {
            let name = item
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.len() < 2 {
                continue;
            }
            if area_item_cleared(item) {
                continue;
            }
            let kind = item.get("kind").and_then(|x| x.as_str()).unwrap_or("");
            let grade = tsunami_kind_to_grade(kind);
            active += 1;
            if !out.iter().any(|x| x.name == name) {
                out.push(TsunamiArea { name, grade });
            }
        }
        return (out, active);
    }
    // 旧缓存：area / description 为顿号或分号分隔文本
    let area = data
        .get("area")
        .or_else(|| data.get("description"))
        .or_else(|| data.get("place"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if area.is_empty() || area == "—" {
        return (Vec::new(), 0);
    }
    let fallback = jian_tsunami_grade(
        agency,
        data.get("level").and_then(|v| v.as_str()).unwrap_or(""),
        data.get("title").and_then(|v| v.as_str()).unwrap_or(""),
    );
    let mut out: Vec<TsunamiArea> = Vec::new();
    for part in area.split(['；', ';', '\n']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let clean = part.trim_start_matches(['•', '・', '-', ' ']);
        let g = tsunami_kind_to_grade(clean);
        let grade = if tsunami_grade_rank(&g) > 0 {
            g
        } else {
            fallback.clone()
        };
        let name = clean
            .split(['[', ' ', '\t'])
            .next()
            .unwrap_or(clean)
            .trim()
            .to_string();
        if name.len() < 2 {
            continue;
        }
        if !out.iter().any(|a| a.name == name) {
            out.push(TsunamiArea {
                name,
                grade,
            });
        }
    }
    let n = out.len();
    (out, n)
}

fn is_eew_agency(agency: &str) -> bool {
    let a = agency.trim().to_ascii_lowercase().replace('_', "-");
    matches!(
        a.as_str(),
        "jma-eew"
            | "cea"
            | "cea-pr"
            | "cwa-eew"
            | "sa"
            | "kma-eew"
            | "early-est"
            | "earlyest"
    ) || a.ends_with("-eew")
        || a.starts_with("cea-pr:")
        || a.starts_with("cea-pr/")
}

fn normalize_jian_agency(agency: &str, data: &Value) -> String {
    let a = agency.trim();
    if a.eq_ignore_ascii_case("cea-pr") {
        let province = data.get("province").and_then(|v| v.as_str());
        return cea_pr_agency_id(province);
    }
    a.to_string()
}

fn parse_jian_eew(agency: &str, data: &Value) -> Option<EewReport> {
    // 对齐 api/jma-eew.php：isTraining 丢弃；isPLUM→假定震源；取消报允许缺经纬/震级。
    if flag_true(data, &["isTraining", "is_training", "training"]) {
        return None;
    }
    let agency = normalize_jian_agency(agency, data);
    let place = data
        .get("placeName")
        .or_else(|| data.get("Hypocenter"))
        .or_else(|| data.get("place"))
        .or_else(|| data.get("epicenter"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".into());
    let event_id = json_id_string(data, &["id", "EventID", "eventId"]).unwrap_or_else(|| {
        let lat = field_f64(data, &["latitude", "Latitude"]).unwrap_or(0.0);
        let lon = field_f64(data, &["longitude", "Longitude"]).unwrap_or(0.0);
        let t = data
            .get("originTime")
            .or_else(|| data.get("shockTime"))
            .map(parse_origin_ms)
            .unwrap_or(0);
        format!("anon:{t}:{lat:.3}:{lon:.3}")
    });
    let serial = data
        .get("serial")
        .or_else(|| data.get("number"))
        .or_else(|| data.get("Serial"))
        .or_else(|| data.get("updates"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(1) as u32;
    let is_cancel = flag_true(
        data,
        &["isCancel", "isCanceled", "isCancelled", "canceled", "cancelled", "is_cancel"],
    );
    let magnitude = match field_f64(data, &["magnitude", "Magunitude", "Magnitude"]) {
        Some(m) => m,
        None if is_cancel => 0.0,
        None => return None,
    };
    let depth_km = match field_depth_km(data, &["depth", "Depth"]) {
        Some(v) => v,
        None if is_cancel => 0.0,
        None => return None,
    };
    let latitude = match field_f64(data, &["latitude", "Latitude"]) {
        Some(v) => v,
        None if is_cancel => 0.0,
        None => return None,
    };
    let longitude = match field_f64(data, &["longitude", "Longitude"]) {
        Some(v) => v,
        None if is_cancel => 0.0,
        None => return None,
    };
    let origin_ms = data
        .get("originTime")
        .or_else(|| data.get("shockTime"))
        .map(parse_origin_ms)
        .unwrap_or(0);

    let kind = intensity_kind_for_agency(&agency);
    let raw = json_intensity_raw(
        data,
        &["intensity", "MaxIntensity", "maxIntensity", "epiIntensity"],
    );
    // lgIntensity：有则附在震度文案后（长周期阶级）
    let lg = data
        .get("lgIntensity")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "0" && s != "不明");
    let (mut max_intensity_text, intensity_level) =
        fill_intensity(kind, magnitude, depth_km, raw.as_deref());
    if let Some(lg) = lg {
        if max_intensity_text.is_empty() || max_intensity_text == "—" {
            max_intensity_text = format!("L{lg}");
        } else {
            max_intensity_text = format!("{max_intensity_text}/L{lg}");
        }
    }
    let is_assumption = eew_is_assumption(data, &place);
    let is_warn = eew_is_warn(data, intensity_level, kind);
    Some(EewReport {
        agency: AgencyId(agency.clone()),
        event_id,
        serial,
        place: crate::place::fix_place(&agency, &place, latitude, longitude),
        magnitude,
        depth_km,
        latitude,
        longitude,
        origin_ms,
        max_intensity_text,
        intensity_kind: kind,
        intensity_level,
        is_final: flag_true(data, &["isFinal", "is_final", "final", "isLast", "isLastInfo"]),
        is_cancel,
        is_assumption,
        is_warn,
        final_at_ms: 0,
    })
}





pub(crate) fn parse_jian_record(agency: &str, data: &Value) -> Option<EqRecord> {
    // 对齐 api/jma.php：infoType=取消 丢弃；震中/震级調査中时允许缺数（不补假值）。
    let info_type = data
        .get("infoType")
        .or_else(|| data.get("infoTypeName"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if info_type.contains("取消") {
        return None;
    }
    let place_raw = data
        .get("placeName")
        .or_else(|| data.get("location"))
        .or_else(|| data.get("place"))
        .or_else(|| data.get("Place"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let hypo_unknown = flag_true(data, &["hypocenterUnknown", "hypocenter_unknown"])
        || field_looks_unknown(data, &["latitude", "Latitude", "longitude", "Longitude"])
        || place_raw
            .as_deref()
            .is_some_and(|p| is_investigating_text(p) || p.contains("調査中") || p.contains("调查中"));
    let mag_unknown = flag_true(data, &["magnitudeUnknown", "magnitude_unknown"])
        || field_looks_unknown(data, &["magnitude", "Magnitude", "Magunitude"])
        || field_looks_negative(data, &["magnitude", "Magnitude", "Magunitude"]);
    let magnitude = match field_f64(data, &["magnitude", "Magnitude", "Magunitude"]) {
        Some(m) if m >= 0.0 && !mag_unknown => m,
        Some(_) if mag_unknown => 0.0,
        None if mag_unknown || hypo_unknown => 0.0,
        _ => return None,
    };
    let latitude = match field_f64(data, &["latitude", "Latitude"]) {
        Some(v) if !hypo_unknown => v,
        _ if hypo_unknown => 0.0,
        _ => return None,
    };
    let longitude = match field_f64(data, &["longitude", "Longitude"]) {
        Some(v) if !hypo_unknown => v,
        _ if hypo_unknown => 0.0,
        _ => return None,
    };
    let depth_km = match field_depth_km(data, &["depth", "Depth"]) {
        Some(v) if v >= 0.0 && !hypo_unknown => v,
        _ if hypo_unknown => 0.0,
        Some(v) => v,
        None => return None,
    };
    let place = match place_raw {
        Some(p) if !is_investigating_text(&p) => p,
        _ if hypo_unknown => "调查中".into(),
        Some(p) => p,
        None => return None,
    };
    let origin_ms = data
        .get("originTime")
        .or_else(|| data.get("shockTime"))
        .or_else(|| data.get("time"))
        .or_else(|| data.get("Time"))
        .or_else(|| data.get("reportTime"))
        .map(parse_origin_ms)
        .unwrap_or(0);
    let kind = intensity_kind_for_agency(agency);
    let raw = json_intensity_raw(data, &["maxIntensity", "intensity", "MaxIntensity"]);
    let (intensity_text, intensity_level) =
        fill_record_intensity(kind, magnitude, depth_km, raw.as_deref(), mag_unknown);
    let is_auto = record_is_auto(agency, data);

    Some(EqRecord {
        agency: AgencyId(agency.into()),
        place: crate::place::fix_place(agency, &place, latitude, longitude),
        time_text: format_time_text(origin_ms),
        magnitude,
        intensity_text,
        intensity_kind: kind,
        intensity_level,
        latitude,
        longitude,
        depth_km,
        origin_ms,
        is_auto,
        magnitude_unknown: mag_unknown,
        hypocenter_unknown: hypo_unknown,
    })
}


