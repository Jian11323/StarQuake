//! 震中最大震度/烈度估算。
//! 中国烈度连续值对齐 [SCEEW](https://github.com/TenkyuChimata/SCEEW)。

use crate::{cn_intensity_text, jma_from_instrumental, IntensityKind};
use starquake_util::EARTH_RADIUS_KM;
use std::f64::consts::LN_10;

/// 反解烈度环时的地表距搜索上限（km）。
const RING_SEARCH_MAX_KM: f64 = 8_000.0;
/// 仅当相邻档半径差小于此值（数值噪声）才跳过，避免重复画同一圈。
const RING_EPS_KM: f64 = 1e-3;
/// SCEEW 震源距近场下界（km）。
///
/// 公式含 `−3.49·log10(R)`：R→0 会发散。若仅钳到 0.1km，则「极浅」(depth=0)
/// 或深度≤1km 的低震级（M2–M4）会被估成烈度 9–12。对齐 JMA 近场习惯，
/// 取 5km 下限（与 `calc_jma_instrumental` 的 3km 同量级）。
const SCEEW_MIN_DIST_KM: f64 = 5.0;

/// 中国烈度（CSIS）连续值：震级 + 震源距（km）。
///
/// 对齐 SCEEW 衰减式：`max(1.92 + 1.63·M − 3.49·log10(R), 0)`。
/// R 取震源距 `hypot(地表震中距, 深度)`，并设近场下限，避免极浅/近零深度虚高。
pub fn calc_csis(magnitude: f64, depth_km: f64, surface_km: f64) -> f64 {
    if !magnitude.is_finite() || !surface_km.is_finite() || !depth_km.is_finite() {
        return 0.0;
    }
    if surface_km > 10_000.0 {
        return 0.0;
    }
    let dist = surface_km
        .max(0.0)
        .hypot(depth_km.max(0.0))
        .max(SCEEW_MIN_DIST_KM);
    (1.92 + 1.63 * magnitude - 3.49 * (dist.ln() / LN_10)).max(0.0)
}

/// 震源距（km）：深度 + 地表震中距。
pub fn hypocentral_km(depth_km: f64, surface_km: f64) -> f64 {
    let theta = surface_km / EARTH_RADIUS_KM;
    let a = EARTH_RADIUS_KM - depth_km.max(0.0);
    (a * a + EARTH_RADIUS_KM * EARTH_RADIUS_KM
        - 2.0 * a * EARTH_RADIUS_KM * theta.cos())
        .sqrt()
}

/// JMA 計測震度（仪器震度）估算，对齐 RhythmQuake `calcJmaShindo`（ARV=1）。
pub fn calc_jma_instrumental(
    mj: f64,
    depth_km: f64,
    surface_km: f64,
) -> f64 {
    if !mj.is_finite() || !depth_km.is_finite() || mj < 0.0 || depth_km < 0.0 {
        return -3.0;
    }
    let mw = mj - 0.171;
    let fault_half = 10f64.powf(0.5 * mw - 1.85) / 2.0;
    let line_dis = hypocentral_km(depth_km, surface_km.max(0.0));
    let hypo_dist = (line_dis - fault_half).max(3.0);
    let x = hypo_dist;
    let pgv600 = 10f64.powf(
        0.58 * mw + 0.0038 * depth_km - 1.29
            - ((x + 0.0028 * 10f64.powf(0.5 * mw)).ln() / LN_10)
            - 0.002 * x,
    );
    let pgv = pgv600 * 1.307; // * ARV(1.0)
    if pgv <= 0.0 {
        return -3.0;
    }
    2.68 + 1.72 * (pgv.ln() / LN_10)
}

/// 震中最大震度/烈度：无官方烈度字段时用震级+深度估算（地表距=0）。
pub fn estimate_epicentral(
    kind: IntensityKind,
    magnitude: f64,
    depth_km: f64,
) -> (u8, String) {
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return (0, "—".into());
    }
    match kind {
        IntensityKind::JmaShindo => {
            let inst = calc_jma_instrumental(magnitude, depth_km.max(0.0), 0.0);
            if inst <= -3.0 {
                return (0, "—".into());
            }
            let (level, text) = jma_from_instrumental(inst);
            (level, text.into())
        }
        IntensityKind::CnIntensity => cn_from_csis(calc_csis(magnitude, depth_km, 0.0)),
    }
}

fn cn_from_csis(v: f64) -> (u8, String) {
    if v < 0.5 {
        (0, "0".into())
    } else {
        let level = v.round().clamp(1.0, 12.0) as u8;
        // 文案保留一位小数（对齐 SCEEW 本地烈度展示）；档位仍用四舍五入供环/阈值。
        (level, format!("{:.1}", v.min(12.0)))
    }
}

/// 若上游文案为空/`—`，用估算值补齐。
pub fn fill_intensity(
    kind: IntensityKind,
    magnitude: f64,
    depth_km: f64,
    raw: Option<&str>,
) -> (String, u8) {
    let raw = raw.map(str::trim).unwrap_or("");
    // 调查中 / 不明：保留原文，不估算假震度
    if matches!(
        raw,
        "不明"
            | "?"
            | "調査中"
            | "调查中"
            | "なし"
            | "无"
            | "N/A"
            | "n/a"
    ) {
        return (raw.into(), 0);
    }
    if !raw.is_empty() && raw != "—" && raw != "-" && raw != "null" {
        match kind {
            IntensityKind::JmaShindo => {
                let level = crate::jma_shindo_level(raw);
                (raw.into(), level)
            }
            IntensityKind::CnIntensity => {
                let level = crate::cn_intensity_level(raw);
                (cn_intensity_text(level).into(), level)
            }
        }
    } else {
        let (level, text) = estimate_epicentral(kind, magnitude, depth_km);
        (text, level)
    }
}

/// 本地预估烈度/震度（本机位置相对震中）。
pub fn estimate_at_site(
    kind: IntensityKind,
    magnitude: f64,
    depth_km: f64,
    surface_km: f64,
) -> (u8, String) {
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return (0, "—".into());
    }
    match kind {
        IntensityKind::JmaShindo => {
            let inst = calc_jma_instrumental(magnitude, depth_km.max(0.0), surface_km.max(0.0));
            if inst <= -3.0 {
                return (0, "—".into());
            }
            let (level, text) = jma_from_instrumental(inst);
            (level, text.into())
        }
        IntensityKind::CnIntensity => {
            cn_from_csis(calc_csis(magnitude, depth_km, surface_km.max(0.0)))
        }
    }
}

/// 该档位是否参与画环（JMA 1–9 / 中国 1–12）。
fn ring_level_ok(kind: IntensityKind, level: u8) -> bool {
    match kind {
        IntensityKind::JmaShindo => (1..=9).contains(&level),
        IntensityKind::CnIntensity => (1..=12).contains(&level),
    }
}

/// 二分求「离散估档仍 ≥ level」的最大地表距（km）。
///
/// 判定直接走 `estimate_at_site`，与本机倒计时/侧栏同一套衰减→档位映射，
/// 避免连续阈值与 `round`/震度分界不一致。
fn radius_for_level(
    kind: IntensityKind,
    level: u8,
    magnitude: f64,
    depth_km: f64,
) -> Option<f64> {
    if !ring_level_ok(kind, level) {
        return None;
    }
    let at = |r: f64| estimate_at_site(kind, magnitude, depth_km, r).0;
    if at(0.0) < level {
        return None;
    }
    if at(RING_SEARCH_MAX_KM) >= level {
        return Some(RING_SEARCH_MAX_KM);
    }
    let mut lo = 0.0;
    let mut hi = RING_SEARCH_MAX_KM;
    // 56 次二分 → 分辨率约 8000/2^56 ≪ 1 mm，远小于地图像素误差
    for _ in 0..56 {
        let mid = 0.5 * (lo + hi);
        if at(mid) >= level {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if lo < 0.05 {
        None
    } else {
        Some(lo)
    }
}

/// 单档烈度/震度环外半径（地表 km）：该档及以上区域的最远震中距。
/// 震中达不到该档时返回 `None`。
pub fn intensity_radius_km(
    kind: IntensityKind,
    level: u8,
    magnitude: f64,
    depth_km: f64,
) -> Option<f64> {
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return None;
    }
    radius_for_level(kind, level, magnitude, depth_km.max(0.0))
}

/// 预警烈度估算环：`(档位, 外半径 km)`，由高到低。
///
/// 半径由衰减正算 + 离散档位反解；仅依赖震级与深度（各向同性圆）。
/// 每一圈表示「估档 ≥ 该档」的外缘，与 `estimate_at_site` 一致。
pub fn intensity_rings(
    kind: IntensityKind,
    magnitude: f64,
    depth_km: f64,
) -> Vec<(u8, f64)> {
    let (max_lv, _) = estimate_epicentral(kind, magnitude, depth_km);
    if max_lv == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut prev_r = f64::INFINITY;
    for level in (1..=max_lv).rev() {
        let Some(r) = intensity_radius_km(kind, level, magnitude, depth_km) else {
            continue;
        };
        if !r.is_finite() || r < 0.05 {
            continue;
        }
        // 仅滤掉与更高档数值重合的圈（浮点噪声），不合并真实不同半径
        if prev_r.is_finite() && (prev_r - r).abs() < RING_EPS_KM {
            continue;
        }
        out.push((level, r));
        prev_r = r;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csis_matches_sceew_sample() {
        // SCEEW: 1.92 + 1.63*M - 3.49*log10(R)；R=震源距 hypot(地表,深度)
        let v = calc_csis(5.0, 10.0, 100.0);
        let r = 100.0_f64.hypot(10.0);
        let expect = 1.92 + 1.63 * 5.0 - 3.49 * r.log10();
        assert!((v - expect).abs() < 1e-9, "got {v} expect {expect}");
        assert!(v > 2.0 && v < 6.0, "got {v}");
    }

    #[test]
    fn csis_near_field_finite() {
        let v = calc_csis(5.3, 10.0, 0.0);
        assert!(v.is_finite() && v > 5.0, "got {v}");
    }

    #[test]
    fn csis_small_quake_not_catastrophic() {
        // 旧实现用 R=0.1km → M3.2 估出约 10.6→11；用震源距+近场下限后应约 3~5
        let (lv, text) = estimate_epicentral(IntensityKind::CnIntensity, 3.2, 10.0);
        assert!(
            (2..=5).contains(&lv),
            "M3.2@10km should be mild, got lv={lv} text={text}"
        );
        let v = calc_csis(3.2, 10.0, 0.0);
        assert!(v > 2.5 && v < 5.5, "continuous I={v}");
    }

    #[test]
    fn csis_very_shallow_low_mag_not_catastrophic() {
        // 「极浅」depth=0 / 1km：旧钳 0.1km 会把 M2–M4 抬到 Ⅷ–Ⅺ
        for (m, d) in [(2.0, 0.0), (3.2, 0.0), (3.2, 1.0), (4.0, 0.0)] {
            let (lv, text) = estimate_epicentral(IntensityKind::CnIntensity, m, d);
            assert!(
                lv <= 7,
                "M{m}@{d}km must not explode, got lv={lv} text={text}"
            );
            let v = calc_csis(m, d, 0.0);
            assert!(
                v < 8.0,
                "M{m}@{d}km continuous I={v} still too high"
            );
        }
        // 近场下限生效：depth=0 与 depth=5 震中估应一致
        let a = calc_csis(3.0, 0.0, 0.0);
        let b = calc_csis(3.0, 5.0, 0.0);
        assert!((a - b).abs() < 1e-9, "floor@5km: {a} vs {b}");
    }

    /// 大网格：对照旧下限 0.1km，统计虚高比例；现行 5km 下限不得再爆。
    #[test]
    fn csis_large_grid_shallow_floor() {
        fn legacy_floor01(m: f64, d: f64, s: f64) -> f64 {
            let dist = s.max(0.0).hypot(d.max(0.0)).max(0.1);
            (1.92 + 1.63 * m - 3.49 * (dist.ln() / LN_10)).max(0.0)
        }

        let mut total = 0u32;
        let mut risk = 0u32; // M≤4.5 且震源距 hypot<5（旧下限会顶到 0.1）
        let mut legacy_hi = 0u32; // 旧式连续值 ≥8
        let mut now_hi = 0u32;
        let mut now_max = 0.0_f64;
        let mut legacy_max = 0.0_f64;
        let mut worst_now = (0.0, 0.0, 0.0, 0.0);
        let mut worst_legacy = (0.0, 0.0, 0.0, 0.0);

        // M 1.0–7.0 / 0.1；深度与地表距覆盖极浅→远场
        let depths = [
            0.0, 0.1, 0.5, 1.0, 2.0, 3.0, 5.0, 8.0, 10.0, 15.0, 20.0, 30.0, 50.0, 100.0,
        ];
        let surfaces = [
            0.0, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0,
        ];
        // 1.0..=7.0 步长 0.1 → 61 档，避免浮点累加漂移
        for mi in 10..=70 {
            let m = mi as f64 / 10.0;
            for &d in &depths {
                for &s in &surfaces {
                    total += 1;
                    let now = calc_csis(m, d, s);
                    let old = legacy_floor01(m, d, s);
                    if now > now_max {
                        now_max = now;
                        worst_now = (m, d, s, now);
                    }
                    if old > legacy_max {
                        legacy_max = old;
                        worst_legacy = (m, d, s, old);
                    }
                    let hypo = s.max(0.0).hypot(d.max(0.0));
                    if m <= 4.5 && hypo < 5.0 {
                        risk += 1;
                        if old >= 8.0 {
                            legacy_hi += 1;
                        }
                        if now >= 8.0 {
                            now_hi += 1;
                        }
                        assert!(
                            now < 8.0,
                            "grid explode M{m} d={d} s={s} I={now}"
                        );
                        if s == 0.0 {
                            let (lv, _) =
                                estimate_epicentral(IntensityKind::CnIntensity, m, d);
                            assert!(lv <= 7, "epi lv={lv} M{m}@{d}");
                        }
                    }
                }
            }
        }

        eprintln!(
            "csis_large_grid: total={total} risk_cells(M<=4.5,hypo<5)={risk} \
             legacy_I>=8: {legacy_hi}/{risk} now_I>=8: {now_hi}/{risk} \
             now_max={now_max:.3}@{worst_now:?} legacy_max={legacy_max:.3}@{worst_legacy:?}"
        );
        assert!(total >= 5_000, "expected large grid, got {total}");
        assert!(risk >= 200, "expected many near-field low-M cells, got {risk}");
        assert!(
            legacy_hi > 0,
            "legacy 0.1km floor should still explode on this grid"
        );
        assert_eq!(now_hi, 0, "current 5km floor must not explode");
    }

    #[test]
    fn jma_small_quake_low_shindo() {
        let (lv, text) = estimate_epicentral(IntensityKind::JmaShindo, 2.9, 10.0);
        assert!(lv <= 3, "lv={lv} text={text}");
    }

    #[test]
    fn cn_rings_monotonic_and_exact_boundary() {
        let m = 6.0;
        let h = 10.0;
        let kind = IntensityKind::CnIntensity;
        let rings = intensity_rings(kind, m, h);
        assert!(!rings.is_empty(), "expected rings");
        for w in rings.windows(2) {
            assert!(w[0].0 > w[1].0);
            assert!(
                w[0].1 < w[1].1,
                "radii should grow outward: L{}={:.6} vs L{}={:.6}",
                w[0].0,
                w[0].1,
                w[1].0,
                w[1].1
            );
        }
        for &(lv, r) in &rings {
            let (on, _) = estimate_at_site(kind, m, h, r);
            assert!(
                on >= lv,
                "on-ring R={r:.6} expected level>={lv} got {on}"
            );
            // 略外侧必须掉档（触顶搜索上限时除外）
            if r < RING_SEARCH_MAX_KM - 1.0 {
                let outside = (r + (r * 1e-6).max(0.02)).min(RING_SEARCH_MAX_KM);
                let (off, _) = estimate_at_site(kind, m, h, outside);
                assert!(
                    off < lv,
                    "outside R={outside:.6} expected level<{lv} got {off} (ring={r:.6})"
                );
            }
        }
    }

    #[test]
    fn jma_rings_exact_boundary() {
        let m = 6.5;
        let h = 10.0;
        let kind = IntensityKind::JmaShindo;
        let rings = intensity_rings(kind, m, h);
        assert!(!rings.is_empty());
        for w in rings.windows(2) {
            assert!(w[0].0 > w[1].0);
            assert!(w[0].1 < w[1].1);
        }
        for &(lv, r) in &rings {
            let (on, _) = estimate_at_site(kind, m, h, r);
            assert!(on >= lv, "on-ring expected>={lv} got {on} R={r}");
            if r < RING_SEARCH_MAX_KM - 1.0 {
                let outside = (r + (r * 1e-6).max(0.02)).min(RING_SEARCH_MAX_KM);
                let (off, _) = estimate_at_site(kind, m, h, outside);
                assert!(
                    off < lv,
                    "outside expected<{lv} got {off} R={outside} ring={r}"
                );
            }
        }
    }

    #[test]
    fn tiny_quake_rings_bounded() {
        // 近场用震源距；远场仍应迅速掉档
        let rings = intensity_rings(IntensityKind::CnIntensity, 2.0, 10.0);
        assert!(!rings.is_empty());
        let (lo, r) = *rings.last().unwrap();
        assert!(lo >= 1);
        let (off, _) = estimate_at_site(IntensityKind::CnIntensity, 2.0, 10.0, r + 50.0);
        assert!(off < lo || r >= RING_SEARCH_MAX_KM - 1.0);
    }

    #[test]
    fn radius_matches_estimate_at_site_level() {
        // 任意点：在 L 环内应 ≥L，在下一档环外应 <L
        let kind = IntensityKind::CnIntensity;
        let m = 5.5;
        let h = 12.0;
        let rings = intensity_rings(kind, m, h);
        assert!(rings.len() >= 2);
        let (lv_hi, r_hi) = rings[0];
        let (_lv_lo, r_lo) = *rings.last().unwrap();
        let mid = 0.5 * (r_hi + r_lo);
        let (got, _) = estimate_at_site(kind, m, h, mid);
        assert!(got >= 1);
        // 震中档位应等于最高环档位
        let (epi, _) = estimate_epicentral(kind, m, h);
        assert_eq!(epi, lv_hi);
    }
}
