//! 常用固定时区（东八区 / 日本标准时）。

use chrono::FixedOffset;

/// 中国标准时间 UTC+8
pub fn cst() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8")
}

/// 日本标准时间 UTC+9
pub fn jst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).expect("UTC+9")
}
