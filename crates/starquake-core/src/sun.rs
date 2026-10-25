//! 太阳位置、晨昏线与本机日出日落估算。

use std::f64::consts::PI;

/// 用户提示（鉴权 / 连接限制等），供 UI 弹窗。
#[derive(Debug, Clone)]
pub struct UserNotice {
    pub kind: UserNoticeKind,
    pub title: String,
    pub body: String,
    /// 服务器数字码；无则 0
    pub code: i32,
    /// 去重键（如 `conn_limit` / `4006`）
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserNoticeKind {
    Auth,
    ConnLimit,
    Banned,
    Info,
}

/// 太阳几何：赤纬（弧度）与格林尼治时角（弧度）。
pub fn sun_geometry(unix_ms: i64) -> (f64, f64) {
    let jd = unix_ms as f64 / 86_400_000.0 + 2_440_587.5;
    let n = jd - 2_451_545.0;
    let l = (280.460 + 0.9856474 * n).rem_euclid(360.0);
    let g = ((357.528 + 0.9856003 * n).rem_euclid(360.0)).to_radians();
    let lambda = (l + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    let eps = (23.439 - 0.0000004 * n).to_radians();
    // δ = arcsin(sin ε · sin λ)；α = atan2(sin λ · cos ε, cos λ)
    let dec = (eps.sin() * lambda.sin()).asin();
    let gmst_deg =
        (280.160_041_4 + 360.985_647_366 * (jd - 2_451_545.0)).rem_euclid(360.0);
    let ra = (lambda.sin() * eps.cos()).atan2(lambda.cos());
    let ha = (gmst_deg.to_radians() - ra).rem_euclid(2.0 * PI);
    (dec, ha)
}

/// 太阳高度角（度）。`dec_rad` / `ha0_rad` 来自 [`sun_geometry`]。
pub fn solar_altitude_deg(lon_deg: f64, lat_deg: f64, dec_rad: f64, ha0_rad: f64) -> f64 {
    let lat = lat_deg.to_radians();
    let ha = ha0_rad + lon_deg.to_radians();
    let sin_alt = lat.sin() * dec_rad.sin() + lat.cos() * dec_rad.cos() * ha.cos();
    sin_alt.clamp(-1.0, 1.0).asin().to_degrees()
}

/// 给定经纬度是否为夜侧（太阳高度角 < 0）。
pub fn is_night(lon_deg: f64, lat_deg: f64, unix_ms: i64) -> bool {
    let (dec, ha0) = sun_geometry(unix_ms);
    solar_altitude_deg(lon_deg, lat_deg, dec, ha0) < 0.0
}

/// 晨昏线上纬度对应的经度（度）；极昼/极夜返回 None。
pub fn terminator_lon_at_lat(lat_deg: f64, unix_ms: i64) -> Option<f64> {
    let (dec, ha0) = sun_geometry(unix_ms);
    let lat = lat_deg.to_radians();
    let cos_ha = -(lat.tan() * dec.tan());
    if cos_ha.abs() > 1.0 {
        return None;
    }
    let ha = cos_ha.acos();
    // 两条解：取「日落」一侧作折线，夜侧在太阳对面
    let lon = (ha - ha0).to_degrees();
    Some(((lon + 540.0) % 360.0) - 180.0)
}

/// 本机当日日出/日落本地时刻（分钟自当地 0 点）；失败返回 None。
pub fn sunrise_sunset_local_hm(lat_deg: f64, lon_deg: f64, unix_ms: i64) -> Option<(u32, u32)> {
    // NOAA 近似：用当地正午附近迭代太阳高度过零
    let offset_min = local_utc_offset_minutes();
    let day_start_utc_ms = {
        let local = unix_ms + offset_min as i64 * 60_000;
        let day = local.div_euclid(86_400_000);
        day * 86_400_000 - offset_min as i64 * 60_000
    };
    let mut rise: Option<i64> = None;
    let mut set: Option<i64> = None;
    let step = 60_000i64; // 1 min
    let mut prev_night = is_night(lon_deg, lat_deg, day_start_utc_ms);
    let mut t = day_start_utc_ms + step;
    let end = day_start_utc_ms + 86_400_000;
    while t <= end {
        let night = is_night(lon_deg, lat_deg, t);
        if prev_night && !night && rise.is_none() {
            rise = Some(t);
        }
        if !prev_night && night && set.is_none() {
            set = Some(t);
        }
        prev_night = night;
        t += step;
    }
    let rise_hm = rise.map(|ms| {
        let local = ms + offset_min as i64 * 60_000;
        
        ((local.rem_euclid(86_400_000)) / 60_000) as u32
    })?;
    let set_hm = set.map(|ms| {
        let local = ms + offset_min as i64 * 60_000;
        ((local.rem_euclid(86_400_000)) / 60_000) as u32
    })?;
    Some((rise_hm, set_hm))
}

fn local_utc_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

/// 格式化为 `HH:MM`。
pub fn format_hm_minutes(mins: u32) -> String {
    let h = (mins / 60) % 24;
    let m = mins % 60;
    format!("{h:02}:{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminator_midlat_ok() {
        let lon = terminator_lon_at_lat(0.0, 1_700_000_000_000);
        assert!(lon.is_some());
    }

    #[test]
    fn altitude_is_zero_on_terminator() {
        let t = 1_700_000_000_000i64;
        let (dec, ha) = sun_geometry(t);
        let lon = terminator_lon_at_lat(20.0, t).expect("lon");
        let alt = solar_altitude_deg(lon, 20.0, dec, ha);
        assert!(alt.abs() < 0.05, "alt {alt}");
    }

    #[test]
    fn polar_day_has_no_terminator_lon() {
        // 夏至附近北极应极昼，晨昏经度无解
        assert!(terminator_lon_at_lat(89.0, 1_718_841_600_000).is_none());
    }

    #[test]
    fn night_and_day_are_opposite() {
        let t = 1_700_000_000_000i64;
        let night = is_night(0.0, 0.0, t);
        assert_ne!(is_night(180.0, 0.0, t), night);
    }

    #[test]
    fn sunrise_sunset_are_ordered_for_mid_latitude() {
        // 2023-11-14 12:00 UTC 附近，东京应有日出早于日落
        let t = 1_700_000_000_000i64;
        let (rise, set) = sunrise_sunset_local_hm(35.68, 139.76, t).expect("sun times");
        assert!(rise < set, "rise {rise} set {set}");
        assert_eq!(format_hm_minutes(rise).len(), 5);
    }

    #[test]
    fn yinchuan_oct_day_length_reasonable() {
        // 2026-10-18 银川附近：日长约 11h，不应再出现 08:14/17:38 那种赤纬算错
        let t = 1_792_281_600_000i64; // 2026-10-18 00:00 UTC
        let (rise, set) = sunrise_sunset_local_hm(38.5959, 106.3325, t).expect("sun");
        let day_min = set.saturating_sub(rise);
        assert!(
            (10 * 60..12 * 60).contains(&day_min),
            "day_min={day_min} rise={} set={}",
            format_hm_minutes(rise),
            format_hm_minutes(set)
        );
        // CST 下大致落在 07:xx / 18:xx
        assert!((6 * 60 + 40..7 * 60 + 40).contains(&rise), "rise {}", format_hm_minutes(rise));
        assert!((17 * 60 + 40..18 * 60 + 40).contains(&set), "set {}", format_hm_minutes(set));
    }
}
