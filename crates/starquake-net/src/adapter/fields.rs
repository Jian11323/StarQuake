//! 各源共用的 JSON 字段读取与烈度填充。

use serde_json::Value;
use starquake_core::{fill_intensity, IntensityKind};


/// JSON 数字或数字字符串 → f64（Wolfx CENC 的 magnitude/lat/lon/depth 常为字符串）。
pub(crate) fn json_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let s = s.trim().trim_end_matches("km").trim_end_matches("KM").trim();
            if s.is_empty() || s == "—" || s == "-" {
                return None;
            }
            s.parse().ok()
        }
        _ => None,
    }
}

pub(crate) fn json_intensity_raw(data: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = data.get(*k) {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.as_f64().map(|f| format!("{f}")).unwrap_or_default(),
                _ => continue,
            };
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

pub(crate) fn field_f64(data: &Value, keys: &[&str]) -> Option<f64> {
    for k in keys {
        if let Some(v) = data.get(*k) {
            if let Some(f) = json_f64(v) {
                return Some(f);
            }
        }
    }
    None
}

/// JMA：「极浅」/「ごく浅い」等文案（与 depth=0 同义）；展示统一走 depth_label→「极浅」。
pub(crate) fn is_very_shallow_depth_text(s: &str) -> bool {
    let t = s
        .trim()
        .trim_end_matches("km")
        .trim_end_matches("KM")
        .trim()
        .replace(['　', ' '], "");
    if t.eq_ignore_ascii_case("veryshallow") {
        return true;
    }
    // 含日文原文或中文「极浅/極浅」均视为极浅，禁止把原文直接展示出去
    t == "极浅"
        || t == "極浅"
        || t.contains("ごく浅い")
        || t.contains("ごく浅")
        || t.contains("極浅い")
        || t.contains("極浅")
        || t.contains("极浅")
}

/// 深度字段：数字，或 JMA 「极浅」文案→ `0.0`（展示仍用 `depth_label` 显「极浅」）。
pub(crate) fn field_depth_km(data: &Value, keys: &[&str]) -> Option<f64> {
    for k in keys {
        if let Some(v) = data.get(*k) {
            if let Value::String(s) = v {
                if is_very_shallow_depth_text(s) {
                    return Some(0.0);
                }
            }
            if let Some(f) = json_f64(v) {
                return Some(f);
            }
        }
    }
    None
}

/// 上游「调查中 / 不明」文案（JMA 震度速报等）。
pub(crate) fn is_investigating_text(s: &str) -> bool {
    let t = s.trim();
    matches!(
        t,
        "調査中"
            | "调查中"
            | "不明"
            | "?"
            | "なし"
            | "无"
            | "N/A"
            | "n/a"
            | "—"
            | "-"
            | "null"
    )
}

/// 字段值为调查中文案、Null，或无法解析为数字。
/// 不把负数当缺测：西经/南纬合法；震级哨兵用 [`field_looks_negative`]。
pub(crate) fn field_looks_unknown(data: &Value, keys: &[&str]) -> bool {
    for k in keys {
        if let Some(v) = data.get(*k) {
            match v {
                Value::String(s) if is_investigating_text(s) => return true,
                Value::String(s) if is_very_shallow_depth_text(s) => {}
                Value::String(s) => {
                    let s = s.trim().trim_end_matches("km").trim_end_matches("KM").trim();
                    if !s.is_empty() && json_f64(v).is_none() {
                        return true;
                    }
                }
                Value::Null => return true,
                _ => {}
            }
        }
    }
    false
}

/// 可解析且 `< 0`（震级/深度哨兵如 `-1`）。
pub(crate) fn field_looks_negative(data: &Value, keys: &[&str]) -> bool {
    for k in keys {
        if let Some(v) = data.get(*k) {
            if json_f64(v).is_some_and(|f| f < 0.0) {
                return true;
            }
        }
    }
    false
}

/// 有震度原文时解析；震级调查中且无震度时不估算假烈度。
pub(crate) fn fill_record_intensity(
    kind: IntensityKind,
    magnitude: f64,
    depth_km: f64,
    raw: Option<&str>,
    magnitude_unknown: bool,
) -> (String, u8) {
    let raw_trim = raw.map(str::trim).unwrap_or("");
    if magnitude_unknown {
        if raw_trim.is_empty() || is_investigating_text(raw_trim) {
            let text = if raw_trim.is_empty() || raw_trim == "調査中" {
                "调查中".into()
            } else {
                raw_trim.into()
            };
            return (text, 0);
        }
        // 震度速报：最大震度已知，震级仍调查中
        return fill_intensity(kind, magnitude, depth_km, Some(raw_trim));
    }
    fill_intensity(kind, magnitude, depth_km, raw)
}

/// Wolfx/JMA：仮定震源 / PLUM / 调查中。
pub(crate) fn eew_is_assumption(data: &Value, place: &str) -> bool {
    if flag_true(
        data,
        &[
            "isAssumption",
            "is_assumption",
            "assumption",
            "isAssumed",
            "isPLUM",
            "isPlum",
            "is_plum",
        ],
    ) {
        return true;
    }
    let hypo = data
        .get("Hypocenter")
        .or_else(|| data.get("hypocenter"))
        .or_else(|| data.get("placeName"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    [place, hypo]
        .iter()
        .any(|s| s.contains("調査中") || s.contains("调查中") || s.contains("仮定"))
}

/// 警报：显式 isWarn，或标题含「警報」，或 JMA 震度≥5弱。
pub(crate) fn eew_is_warn(data: &Value, intensity_level: u8, intensity_kind: IntensityKind) -> bool {
    if flag_true(data, &["isWarn", "is_warn", "warn"]) {
        return true;
    }
    let title = data
        .get("Title")
        .or_else(|| data.get("title"))
        .or_else(|| data.get("infoTypeName"))
        .or_else(|| data.get("alertflg"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if title.contains("警報") || title.eq_ignore_ascii_case("Warning") {
        return true;
    }
    intensity_kind == IntensityKind::JmaShindo && intensity_level >= 5
}

pub(crate) fn flag_true(data: &Value, keys: &[&str]) -> bool {
    for k in keys {
        if let Some(v) = data.get(*k) {
            if json_truthy(v) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn json_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        Value::String(s) => {
            let s = s.trim();
            matches!(
                s,
                "1" | "true"
                    | "True"
                    | "TRUE"
                    | "yes"
                    | "cancel"
                    | "canceled"
                    | "cancelled"
                    | "最終"
                    | "最终"
                    | "最終報"
                    | "最终报"
                    | "取消"
                    | "取消報"
            )
        }
        _ => false,
    }
}
/// 速报是否为自动测定：读 `type` / `Type` / `infoType` / `status`；
/// 含 automatic/auto/自动 → true；reviewed/manual/正式 → false。
/// 各源通用；无字段时返回 false（按正式展示，不标「自动测定」）。
pub(crate) fn record_is_auto(_agency: &str, data: &Value) -> bool {
    for key in ["type", "Type", "infoType", "status", "Status", "evaluationStatus"] {
        if let Some(t) = data.get(key).and_then(|v| v.as_str()) {
            let t = t.trim().to_ascii_lowercase();
            if t.is_empty() {
                continue;
            }
            if t == "automatic"
                || t == "auto"
                || t.contains("automatic")
                || t.contains("自动")
            {
                return true;
            }
            if t == "reviewed"
                || t == "manual"
                || t == "final"
                || t.contains("正式")
                || t.contains("人工")
            {
                return false;
            }
        }
    }
    false
}
pub(crate) fn json_id_string(data: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        let Some(v) = data.get(*k) else {
            continue;
        };
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(n) = v.as_u64() {
            return Some(n.to_string());
        }
        if let Some(n) = v.as_i64() {
            return Some(n.to_string());
        }
        if let Some(n) = v.as_f64() {
            if n.is_finite() {
                return Some(format!("{n:.0}"));
            }
        }
    }
    None
}
