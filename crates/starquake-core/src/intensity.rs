//! 震度/烈度文案 → 档位索引（与 palette 档位对齐）。

use crate::IntensityKind;

/// JMA 震度文案 → 0–9（0 / 1 / 2 / 3 / 4 / 5弱 / 5强 / 6弱 / 6强 / 7）
pub fn jma_shindo_level(text: &str) -> u8 {
    let t = text.trim();
    let t = t
        .replace('弱', "-")
        .replace(['強', '强'], "+");
    match t.as_str() {
        "0" | "０" => 0,
        "1" | "１" => 1,
        "2" | "２" => 2,
        "3" | "３" => 3,
        "4" | "４" => 4,
        "5-" | "5−" => 5,
        "5+" => 6,
        "6-" | "6−" => 7,
        "6+" => 8,
        "7" | "７" => 9,
        _ => {
            // P2PQuake maxScale：10/20/…/70
            if let Ok(n) = t.parse::<i32>() {
                return match n {
                    0 => 0,
                    10 => 1,
                    20 => 2,
                    30 => 3,
                    40 => 4,
                    45 => 5,
                    50 => 6,
                    55 => 7,
                    60 => 8,
                    70 => 9,
                    _ if (0..=9).contains(&n) => n as u8,
                    _ => 0,
                };
            }
            0
        }
    }
}

pub fn jma_shindo_text(level: u8) -> &'static str {
    match level {
        0 => "0",
        1 => "1",
        2 => "2",
        3 => "3",
        4 => "4",
        5 => "5弱",
        6 => "5强",
        7 => "6弱",
        8 => "6强",
        _ => "7",
    }
}

/// 計測震度（仪器震度）→ JMA 震度阶级档位 0–9 与展示文案。
///
/// 分界按气象厅震度階級の解説（0.5 / 1.5 / … / 6.5）。
pub fn jma_from_instrumental(v: f64) -> (u8, &'static str) {
    let level = if v < 0.5 {
        0
    } else if v < 1.5 {
        1
    } else if v < 2.5 {
        2
    } else if v < 3.5 {
        3
    } else if v < 4.5 {
        4
    } else if v < 5.0 {
        5
    } else if v < 5.5 {
        6
    } else if v < 6.0 {
        7
    } else if v < 6.5 {
        8
    } else {
        9
    };
    (level, jma_shindo_text(level))
}

/// 测站/地图用展示文案：低于 0.5 显示取整計測值（含 -3～0），否则用震度阶级。
/// 計測震度半档展示（-3…7 步长 0.5）。
pub fn instrumental_half_step_text(v: f64) -> String {
    if !v.is_finite() || v < -3.0 {
        return "-".into();
    }
    let stepped = (v.clamp(-3.0, 7.0) * 2.0).round() / 2.0;
    if stepped.fract().abs() < 1e-9 {
        format!("{}", stepped as i32)
    } else {
        format!("{:.1}", stepped)
    }
}

pub fn instrumental_display_text(v: f64) -> String {
    if !v.is_finite() || v < -3.0 {
        "—".into()
    } else if v < 0.5 {
        format!("{}", v.round() as i32)
    } else {
        jma_from_instrumental(v).1.into()
    }
}

/// 中国烈度罗马数字或阿拉伯数字 → 1–12（含 MMI 文案，大小写不敏感）
pub fn cn_intensity_level(text: &str) -> u8 {
    let t = text.trim();
    match t {
        "Ⅰ" => return 1,
        "Ⅱ" => return 2,
        "Ⅲ" => return 3,
        "Ⅳ" => return 4,
        "Ⅴ" => return 5,
        "Ⅵ" => return 6,
        "Ⅶ" => return 7,
        "Ⅷ" => return 8,
        "Ⅸ" => return 9,
        "Ⅹ" => return 10,
        "Ⅺ" => return 11,
        "Ⅻ" => return 12,
        _ => {}
    }
    match t.to_ascii_uppercase().as_str() {
        "1" | "I" => 1,
        "2" | "II" => 2,
        "3" | "III" => 3,
        "4" | "IV" => 4,
        "5" | "V" => 5,
        "6" | "VI" => 6,
        "7" | "VII" => 7,
        "8" | "VIII" => 8,
        "9" | "IX" => 9,
        "10" | "X" => 10,
        "11" | "XI" => 11,
        "12" | "XII" => 12,
        _ => {
            if let Ok(f) = t.parse::<f64>() {
                f.round().clamp(1.0, 12.0) as u8
            } else {
                1
            }
        }
    }
}

pub fn cn_intensity_text(level: u8) -> &'static str {
    match level.clamp(1, 12) {
        1 => "Ⅰ",
        2 => "Ⅱ",
        3 => "Ⅲ",
        4 => "Ⅳ",
        5 => "Ⅴ",
        6 => "Ⅵ",
        7 => "Ⅶ",
        8 => "Ⅷ",
        9 => "Ⅸ",
        10 => "Ⅹ",
        11 => "Ⅺ",
        _ => "Ⅻ",
    }
}

/// 中国烈度档位 0 且侧栏应显示「—」（无上游、无有效估算文案）。
pub fn cn_intensity_display_unknown(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t == "—" || t == "-" {
        return true;
    }
    matches!(
        t,
        "不明" | "?" | "調査中" | "调查中" | "なし" | "无" | "N/A" | "n/a" | "null" | "0" | "０"
    )
}

/// 按界面「烈度标度」偏好重映射展示（`auto` 保持原文；`jma` / `cn` 强制标度）。
///
/// JMA / CWA 官方震度（`JmaShindo`）在强制「中国烈度」时也**不改写**，避免侧栏出现烈度色块。
pub fn prefer_intensity_scale(
    kind: IntensityKind,
    level: u8,
    text: &str,
    scale: &str,
) -> (IntensityKind, u8, String) {
    let keep_label = |kind: IntensityKind, level: u8, text: &str| -> String {
        if text.trim().is_empty() {
            match kind {
                IntensityKind::JmaShindo => jma_shindo_text(level).into(),
                IntensityKind::CnIntensity => cn_intensity_text(level).into(),
            }
        } else {
            text.to_string()
        }
    };
    match scale.trim().to_ascii_lowercase().as_str() {
        "jma" => {
            let lv = match kind {
                IntensityKind::JmaShindo => level.min(9),
                IntensityKind::CnIntensity => cn_to_jma_level(level),
            };
            (IntensityKind::JmaShindo, lv, jma_shindo_text(lv).into())
        }
        "cn" => {
            // 气象厅震度保持原标度（含 CWA）
            if kind == IntensityKind::JmaShindo {
                return (kind, level.min(9), keep_label(kind, level, text));
            }
            // level=0：无烈度，勿钳成 1
            if level == 0 {
                let label = if text.trim().is_empty() {
                    "—".into()
                } else {
                    text.to_string()
                };
                return (IntensityKind::CnIntensity, 0, label);
            }
            let lv = level.clamp(1, 12);
            (IntensityKind::CnIntensity, lv, cn_intensity_text(lv).into())
        }
        _ => (kind, level, keep_label(kind, level, text)),
    }
}

fn cn_to_jma_level(cn: u8) -> u8 {
    match cn.clamp(1, 12) {
        1 => 0,
        2 => 1,
        3 => 2,
        4 => 3,
        5 => 4,
        6 => 5,
        7 => 6,
        8 => 7,
        9 => 8,
        _ => 9,
    }
}

/// 按机构选择震度/烈度色标
pub fn intensity_kind_for_agency(agency: &str) -> IntensityKind {
    use crate::agency::{agency_family, AgencyFamily};
    match agency_family(agency) {
        AgencyFamily::Jma | AgencyFamily::Cwa => IntensityKind::JmaShindo,
        // KMA / PEWS 使用 MMI 烈度，与 EEWCN 一致按中国烈度色展示
        AgencyFamily::Kma => IntensityKind::CnIntensity,
        _ => IntensityKind::CnIntensity,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cn_zero_stays_unknown_on_force_cn() {
        let (k, lv, label) = prefer_intensity_scale(IntensityKind::CnIntensity, 0, "—", "cn");
        assert_eq!(k, IntensityKind::CnIntensity);
        assert_eq!(lv, 0);
        assert_eq!(label, "—");
    }

    #[test]
    fn cn_zero_stays_on_auto() {
        let (k, lv, label) = prefer_intensity_scale(IntensityKind::CnIntensity, 0, "—", "auto");
        assert_eq!(k, IntensityKind::CnIntensity);
        assert_eq!(lv, 0);
        assert_eq!(label, "—");
    }
}
