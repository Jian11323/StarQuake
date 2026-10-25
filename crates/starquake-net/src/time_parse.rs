//! 各源时间字符串 → unix ms。

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};

/// 尽力解析常见上游时间；失败返回 0（调用方勿据此伪造倒计时）
pub fn parse_origin_ms(value: &serde_json::Value) -> i64 {
    match value {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        serde_json::Value::String(s) => parse_time_str(s).unwrap_or(0),
        _ => 0,
    }
}

pub fn parse_time_str(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(ms) = s.parse::<i64>() {
        return Some(ms);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    // 2026-08-20T08:38:21+09:00 已由 rfc3339 覆盖；再试无时区 ISO → 按 JST
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        let jst = FixedOffset::east_opt(9 * 3600)?;
        return Some(jst.from_local_datetime(&ndt).single()?.timestamp_millis());
    }
    // Wolfx JMA / P2P: 2026/10/08 13:41:55 → JST
    for fmt in ["%Y/%m/%d %H:%M:%S", "%Y/%m/%d %H:%M:%S%.f"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            let jst = FixedOffset::east_opt(9 * 3600)?;
            return Some(jst.from_local_datetime(&ndt).single()?.timestamp_millis());
        }
    }
    // CENC / 国内常用: 2026-10-11 20:13:23 → CST（固定 +8，避免 Local 时区导致 origin_ms 错/沉底）
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            let cst = FixedOffset::east_opt(8 * 3600)?;
            return Some(cst.from_local_datetime(&ndt).single()?.timestamp_millis());
        }
    }
    None
}

pub fn format_time_text(origin_ms: i64) -> String {
    if origin_ms <= 0 {
        return "—".into();
    }
    let Some(dt) = DateTime::from_timestamp_millis(origin_ms) else {
        return "—".into();
    };
    let cst = FixedOffset::east_opt(8 * 3600).unwrap();
    dt.with_timezone(&cst)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
