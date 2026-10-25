//! Windows TTS 朗读文案：机构读法 + 预警/速报脚本。

use crate::agency::{agency_family, cea_pr_province, AgencyFamily};
use crate::{EewReport, EqRecord, IntensityKind};

/// TTS 专用机构读法（与 UI 括号标签分离）。
pub fn agency_tts_name(agency: &str, is_eew: bool, is_warn: bool) -> String {
    if let Some(prov) = cea_pr_province(agency) {
        if prov.is_empty() {
            return "地方地震局".into();
        }
        if prov.ends_with("地震局") {
            return prov.to_string();
        }
        return format!("{prov}地震局");
    }
    match agency_family(agency) {
        AgencyFamily::Cea => "中国预警网".into(),
        AgencyFamily::Jma if is_eew => {
            if is_warn {
                "日本气象厅警报".into()
            } else {
                "日本气象厅预报".into()
            }
        }
        AgencyFamily::Jma => "日本气象厅".into(),
        AgencyFamily::Sa => "美国ShakeAlert预警".into(),
        AgencyFamily::Kma => "韩国气象厅".into(),
        AgencyFamily::Gq => "GlobalQuake预警".into(),
        AgencyFamily::EarlyEst => "意大利Early-est".into(),
        AgencyFamily::Cwa => "台湾中央气象署".into(),
        AgencyFamily::Cenc => "中国地震台网中心".into(),
        AgencyFamily::Ningxia => "宁夏地震局".into(),
        AgencyFamily::Beijing => "北京地震局".into(),
        AgencyFamily::Yunnan => "云南地震局".into(),
        AgencyFamily::Shanxi => "山西地震局".into(),
        AgencyFamily::Hko => "香港天文台".into(),
        AgencyFamily::Tmd => "泰国气象局".into(),
        AgencyFamily::Bmkg => "印尼气象气候与地球物理局".into(),
        AgencyFamily::Geonet => "新西兰地质灾害监测网".into(),
        AgencyFamily::Usgs => "美国地质调查局".into(),
        AgencyFamily::Emsc => "欧洲地中海地震中心".into(),
        AgencyFamily::Gfz => "德国地学研究中心".into(),
        AgencyFamily::Bcsf => "法国国家地震局".into(),
        AgencyFamily::Ingv => "意大利国家地球物理与火山研究所".into(),
        AgencyFamily::Usp => "巴西圣保罗大学".into(),
        AgencyFamily::Nrcan => "加拿大国家地震研究中心".into(),
        AgencyFamily::Afad => "土耳其地震研究中心".into(),
        AgencyFamily::Ipma => "葡萄牙气象研究所".into(),
        AgencyFamily::Noa => "希腊国家天文台".into(),
        AgencyFamily::Sed => "瑞士地震服务中心".into(),
        AgencyFamily::Scsn => "南加州地震数据中心".into(),
        AgencyFamily::Ipgp => "巴黎地球物理研究所".into(),
        AgencyFamily::Infp => "罗马尼亚国家地球物理研究所".into(),
        AgencyFamily::Isc => "国际地震中心".into(),
        AgencyFamily::Knmi => "荷兰皇家气象研究所".into(),
        AgencyFamily::Ncedc => "北加州地震数据中心".into(),
        AgencyFamily::Lmu => "慕尼黑大学地震服务中心".into(),
        AgencyFamily::Koeri => "土耳其坎迪利天文台".into(),
        AgencyFamily::Gsras => "俄罗斯科学院地球物理局".into(),
        AgencyFamily::Csn => "智利国家地震中心".into(),
        AgencyFamily::Phivolcs => "菲律宾火山地震研究所".into(),
        AgencyFamily::Ssn => "墨西哥地震服务中心".into(),
        AgencyFamily::Ga => "澳大利亚地震局".into(),
        AgencyFamily::Igepn => "厄瓜多尔国家地球物理研究所".into(),
        AgencyFamily::Peru => "秘鲁地震物理研究所".into(),
        AgencyFamily::Mmd => "马来西亚气象局".into(),
        AgencyFamily::Fssn => "Fan Studio地震信息".into(),
        AgencyFamily::Other => "地震情报".into(),
    }
}

fn sanitize_place(place: &str) -> String {
    let t = place
        .replace(['【', '】', '[', ']', '（', '）', '(', ')', '|', '｜'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if t.is_empty() {
        "未知地点".into()
    } else {
        t
    }
}

fn format_origin_for_tts(origin_ms: i64) -> String {
    if origin_ms <= 0 {
        return String::new();
    }
    // 按东八区读出，便于国内听感
    let sec = origin_ms / 1000 + 8 * 3600;
    let days = sec.div_euclid(86400);
    let tod = sec.rem_euclid(86400);
    // 1970-01-01 起算的简易公历（足够 TTS；极端历史可不精确）
    let (y, m, d) = civil_from_days(days);
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    format!("{y}年{m}月{d}日{hh}时{mm}分{ss}秒")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Howard Hinnant civil_from_days
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as i64, d as i64)
}

/// 烈度/震度朗读：优先阿拉伯数字（罗马数字 SAPI 常读不出）。
fn intensity_speak_token(kind: IntensityKind, text: &str, level: u8) -> String {
    let t = text.trim();
    match kind {
        IntensityKind::JmaShindo => {
            if t.contains('弱') || t.contains('强') {
                return t.to_string();
            }
            if !t.is_empty() && t != "—" && t != "-" && t != "?" {
                return t.to_string();
            }
            if level > 0 {
                return level.to_string();
            }
            String::new()
        }
        IntensityKind::CnIntensity => {
            if level > 0 {
                return level.to_string();
            }
            // 罗马数字等 → 尽量解析为阿拉伯数字
            let lv = crate::cn_intensity_level(t);
            if lv > 0 && !t.is_empty() && t != "—" {
                return lv.to_string();
            }
            String::new()
        }
    }
}

fn intensity_phrase(kind: IntensityKind, text: &str, level: u8, eew_style: bool) -> String {
    let token = intensity_speak_token(kind, text, level);
    if token.is_empty() {
        return String::new();
    }
    match kind {
        IntensityKind::CnIntensity => {
            if eew_style {
                format!("预估最大烈度{token}")
            } else {
                format!("烈度{token}")
            }
        }
        IntensityKind::JmaShindo => {
            if eew_style {
                format!("推定最大震度{token}")
            } else {
                format!("最大震度{token}")
            }
        }
    }
}

/// 对齐 eewcn：同槽实质未变则不重读。
/// 同 event+报数时忽略地名抖动（format_place 重算），避免连环朗读。
pub fn should_play_eew_tts(last: Option<&EewReport>, data: &EewReport) -> bool {
    let Some(last) = last else {
        return true;
    };
    if last.event_id != data.event_id {
        return true;
    }
    if last.serial != data.serial {
        return true;
    }
    // 同报：仅震级/深度/烈度实质变化才再读
    if (last.magnitude - data.magnitude).abs() >= 0.1 {
        return true;
    }
    if (last.depth_km as i32) != (data.depth_km as i32) {
        return true;
    }
    if last.intensity_level != data.intensity_level {
        return true;
    }
    if last.max_intensity_text != data.max_intensity_text {
        return true;
    }
    false
}

/// 预警 TTS：`source[预警]第N报，place发生M级地震。预估最大烈度/推定最大震度…`
pub fn build_eew_tts_script(ev: &EewReport) -> String {
    let src = agency_tts_name(&ev.agency.0, true, ev.is_warn);
    let is_jma_eew = agency_family(&ev.agency.0) == AgencyFamily::Jma;
    let org = if is_jma_eew || src.contains("预警") {
        src
    } else {
        format!("{src}预警")
    };
    let updates = if ev.is_final && is_jma_eew {
        "最终报".to_string()
    } else if ev.serial > 0 {
        format!("第{}报", ev.serial)
    } else {
        String::new()
    };
    let place = sanitize_place(&ev.place);
    let body = if ev.magnitude > 0.0 {
        format!("{place}发生{:.1}级地震", ev.magnitude)
    } else {
        format!("{place}发生地震")
    };
    let inten = intensity_phrase(ev.intensity_kind, &ev.max_intensity_text, ev.intensity_level, true);
    let mut out = if updates.is_empty() {
        format!("{org}，{body}")
    } else {
        format!("{org}{updates}，{body}")
    };
    if !inten.is_empty() {
        out.push('。');
        out.push_str(&inten);
    }
    out
}

/// 速报 TTS：
/// `source地震速报，时间，place发生M级地震，震源深度…千米。预估最大烈度/推定最大震度…`
/// CENC：`中国地震台网中心自动测定/正式测定`（不加「地震速报」）。
pub fn build_record_tts_script(rec: &EqRecord) -> String {
    let src = agency_tts_name(&rec.agency.0, false, false);
    let is_cenc = agency_family(&rec.agency.0) == AgencyFamily::Cenc;
    let org = if is_cenc {
        if rec.is_auto {
            "中国地震台网中心自动测定".into()
        } else {
            "中国地震台网中心正式测定".into()
        }
    } else if src.contains("地震速报") || src.contains("地震信息") {
        src
    } else {
        format!("{src}地震速报")
    };
    let time = format_origin_for_tts(rec.origin_ms);
    let place = sanitize_place(&rec.place);
    let mut body = if rec.magnitude_unknown {
        format!("{place}发生地震，震级调查中")
    } else if rec.magnitude > 0.0 {
        format!("{place}发生{:.1}级地震", rec.magnitude)
    } else {
        format!("{place}发生地震")
    };
    if rec.hypocenter_unknown {
        body.push_str("，震源调查中");
    } else if agency_family(&rec.agency.0) == AgencyFamily::Jma && rec.depth_km.abs() < 1e-9 {
        body.push_str("，震源极浅");
    } else if rec.depth_km >= 0.0 {
        body.push_str(&format!("，震源深度{:.0}千米", rec.depth_km));
    }
    // 速报也用「预估最大烈度 / 推定最大震度」+ 阿拉伯数字
    let inten = intensity_phrase(rec.intensity_kind, &rec.intensity_text, rec.intensity_level, true);
    let mut out = if time.is_empty() {
        format!("{org}，{body}")
    } else {
        format!("{org}，{time}，{body}")
    };
    if !inten.is_empty() {
        out.push('。');
        out.push_str(&inten);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgencyId;

    #[test]
    fn should_play_skips_identical_update() {
        let ev = EewReport {
            agency: AgencyId("gq".into()),
            event_id: "gq-1".into(),
            serial: 6,
            place: "阿拉斯加州南部".into(),
            magnitude: 2.1,
            depth_km: 9.0,
            latitude: 60.0,
            longitude: -150.0,
            origin_ms: 1_700_000_000_000,
            max_intensity_text: "1".into(),
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: 1,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: false,
            final_at_ms: 0,
        };
        assert!(!should_play_eew_tts(Some(&ev), &ev));
        let mut place_jitter = ev.clone();
        place_jitter.place = "别处".into();
        assert!(
            !should_play_eew_tts(Some(&ev), &place_jitter),
            "同报地名抖动不应重读"
        );
        let mut next = ev.clone();
        next.serial = 7;
        assert!(should_play_eew_tts(Some(&ev), &next));
    }

    #[test]
    fn jma_eew_no_extra_yujing() {
        let ev = EewReport {
            agency: AgencyId("jma-eew".into()),
            event_id: "1".into(),
            serial: 3,
            place: "茨城县北部".into(),
            magnitude: 5.1,
            depth_km: 10.0,
            latitude: 36.0,
            longitude: 140.0,
            origin_ms: 0,
            max_intensity_text: "4".into(),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: 4,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: false,
            final_at_ms: 0,
        };
        let s = build_eew_tts_script(&ev);
        assert!(s.contains("日本气象厅预报"));
        assert!(!s.contains("预报预警"));
        assert!(s.contains("第3报"));
        assert!(s.contains("推定最大震度4"));
    }

    #[test]
    fn sa_tts_name() {
        assert_eq!(
            agency_tts_name("sa", true, false),
            "美国ShakeAlert预警"
        );
    }

    #[test]
    fn cenc_record_no_sudubao_suffix() {
        let rec = EqRecord {
            agency: AgencyId("cenc".into()),
            place: "四川汶川".into(),
            time_text: "".into(),
            magnitude: 5.2,
            intensity_text: "Ⅵ".into(),
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: 6,
            latitude: 31.0,
            longitude: 103.0,
            depth_km: 10.0,
            origin_ms: 0,
            is_auto: true,
            magnitude_unknown: false,
            hypocenter_unknown: false,
        };
        let s = build_record_tts_script(&rec);
        assert!(s.starts_with("中国地震台网中心自动测定"));
        assert!(!s.contains("地震速报"));
        assert!(s.contains("预估最大烈度6"));
    }

    #[test]
    fn record_tts_uses_estimated_intensity_words() {
        let rec = EqRecord {
            agency: AgencyId("usgs".into()),
            place: "California".into(),
            time_text: "".into(),
            magnitude: 4.0,
            intensity_text: "4".into(),
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: 4,
            latitude: 34.0,
            longitude: -118.0,
            depth_km: 8.0,
            origin_ms: 0,
            is_auto: false,
            magnitude_unknown: false,
            hypocenter_unknown: false,
        };
        let s = build_record_tts_script(&rec);
        assert!(s.contains("美国地质调查局地震速报"));
        assert!(s.contains("预估最大烈度4"));
        assert!(s.contains("震源深度8千米"));
    }
}
