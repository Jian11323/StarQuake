//! 地图镜头：抛物线飞跃与按波圈半径适配缩放。

use std::time::Instant;

use crate::projection::{self, clamp_lat, clamp_lat_globe, normalize_lon};

fn clamp_flight_lat(lat: f64, globe: bool) -> f64 {
    if globe {
        clamp_lat_globe(lat)
    } else {
        clamp_lat(lat)
    }
}

const ZOOM_MIN: f64 = 2.0;
const ZOOM_MAX: f64 = 12.0;

/// 抛物线镜头飞跃（中途拉远、两端落在震中）。
/// 进度只认墙钟秒，与目标帧率 / 每帧 `dt` / 重绘间隔无关：
/// 30/60/120fps 下同一段跳转墙钟时长一致（高帧率只更密采样，不会更快或更慢）。
#[derive(Debug, Clone)]
pub struct CameraFlight {
    start_lon: f64,
    start_lat: f64,
    start_zoom: f64,
    end_lon: f64,
    end_lat: f64,
    end_zoom: f64,
    /// 球体模式：中途对齐 GQ，拉到能看见完整地球。
    globe: bool,
    started: Instant,
    duration: f64,
}

impl CameraFlight {
    pub fn new(
        from: (f64, f64, f64),
        to: (f64, f64, f64),
        duration_s: f64,
        globe: bool,
    ) -> Self {
        let (start_lon, start_lat, start_zoom) = from;
        let (end_lon, end_lat, end_zoom) = to;
        Self {
            start_lon: normalize_lon(start_lon),
            start_lat: clamp_flight_lat(start_lat, globe),
            start_zoom: start_zoom.clamp(ZOOM_MIN, ZOOM_MAX),
            end_lon: normalize_lon(end_lon),
            end_lat: clamp_flight_lat(end_lat, globe),
            end_zoom: end_zoom.clamp(ZOOM_MIN, ZOOM_MAX),
            globe,
            started: Instant::now(),
            duration: duration_s.max(0.05),
        }
    }

    /// 墙钟总时长（秒），供测试与调试；与帧率无关。
    pub fn duration_s(&self) -> f64 {
        self.duration
    }

    /// 按墙钟推进；`_dt` 保留签名但不参与进度（禁止用帧间隔累加）。
    /// 位移：GQ 余弦缓动；缩放：线性时间抛物线（对齐 eewcn / GQ 中途全球视角）。
    pub fn tick(&mut self, _dt: f64) -> (f64, f64, f64, bool) {
        let t = (self.started.elapsed().as_secs_f64() / self.duration).clamp(0.0, 1.0);
        // GQ: t1 = -cos(t*PI)*0.5+0.5
        let t1 = -((t * std::f64::consts::PI).cos()) * 0.5 + 0.5;
        let lon = lerp_lon(self.start_lon, self.end_lon, t1);
        let lat = self.start_lat + (self.end_lat - self.start_lat) * t1;
        let dist_km = haversine_km(
            self.start_lat,
            self.start_lon,
            self.end_lat,
            self.end_lon,
        );
        let z0 = self.start_zoom;
        let z1 = self.end_zoom;
        let y_m = mid_flight_zoom(z0, z1, dist_km, self.globe);
        // eewcn: D=2*yM-avg; a=yA+yB-2D; b=yB-yA-a; zoom=a*t^2+b*t+yA（t 线性）
        let avg = (z0 + z1) * 0.5;
        let d = 2.0 * y_m - avg;
        let a = z0 + z1 - 2.0 * d;
        let b = z1 - z0 - a;
        let zoom = if t >= 1.0 - 1e-9 {
            self.end_zoom
        } else {
            (a * t * t + b * t + z0).clamp(ZOOM_MIN, ZOOM_MAX)
        };
        (
            if t >= 1.0 - 1e-9 {
                self.end_lon
            } else {
                normalize_lon(lon)
            },
            if t >= 1.0 - 1e-9 {
                self.end_lat
            } else {
                clamp_flight_lat(lat, self.globe)
            },
            zoom,
            t >= 1.0 - 1e-9,
        )
    }

    pub fn target(&self) -> (f64, f64, f64) {
        (self.end_lon, self.end_lat, self.end_zoom)
    }
}

/// 球体远距中途拉远目标（再低易缩成小点）。
const GLOBE_FILL_ZOOM: f64 = 7.0;

/// 中途目标 zoom。球体：近距轻拉、远距拉到整球占满窗口；平面适度拉远。
fn mid_flight_zoom(z0: f64, z1: f64, dist_km: f64, globe: bool) -> f64 {
    let avg = (z0 + z1) * 0.5;
    let lo = z0.min(z1);
    if globe {
        let ym = if dist_km < 350.0 {
            // 近距：略抬高，不贴全球地板
            (avg - 0.55).min(lo - 0.3).max(7.8)
        } else if dist_km < 1800.0 {
            // 中距：区域 → 略抬，不到远距地板
            let t = ((dist_km - 350.0) / 1450.0).clamp(0.0, 1.0);
            let target = 8.0 - t * (8.0 - (GLOBE_FILL_ZOOM + 0.25));
            target.min(lo - 0.55)
        } else {
            // 远距：整球占满窗口
            GLOBE_FILL_ZOOM.min(lo - 0.8)
        };
        return ym.clamp(GLOBE_FILL_ZOOM, ZOOM_MAX);
    }
    let pull = if dist_km < 400.0 {
        0.4
    } else if dist_km < 1500.0 {
        0.4 + (dist_km - 400.0) / 1100.0 * 0.7
    } else {
        1.1 + (dist_km / 14000.0).clamp(0.0, 1.0) * 0.9
    };
    let ym = (avg - pull).min(lo - 0.35);
    ym.clamp(3.8, ZOOM_MAX)
}

fn haversine_km(lat0: f64, lon0: f64, lat1: f64, lon1: f64) -> f64 {
    let r = 6371.0_f64;
    let dlat = (lat1 - lat0).to_radians();
    let dlon = (lon1 - lon0).to_radians();
    let a = (dlat * 0.5).sin().powi(2)
        + lat0.to_radians().cos() * lat1.to_radians().cos() * (dlon * 0.5).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

#[allow(dead_code)]
fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

fn lerp_lon(a: f64, b: f64, t: f64) -> f64 {
    let mut d = normalize_lon(b - a);
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    normalize_lon(a + d * t)
}

/// 球体构图：西太平洋震中略向东偏，减少左侧欧亚/俄罗斯占比、留出美洲侧。
/// `zoom` 越大越近；跟焦常用 9–13，故在 zoom < 11 即开始偏置。
/// 仅日本列岛及近海施加东偏；中国内陆、堪察加/千岛等高纬必须对准震中
/// （否则闲时跟焦堪察加速报会被偏到日界线，看起来像「默认北太平洋」）。
pub fn globe_framed_center(lon: f64, lat: f64, zoom: f64) -> (f64, f64) {
    let lon = normalize_lon(lon);
    let lat = clamp_lat_globe(lat);
    if zoom >= 11.0 {
        return (lon, lat);
    }
    let t = ((11.0 - zoom) / 8.0).clamp(0.0, 1.0);
    // 日本列岛及近海（约 125–150°E、24–46°N）；不含中国内陆与堪察加
    if (125.0..150.0).contains(&lon) && (24.0..46.0).contains(&lat) {
        let bias = 12.0 + 20.0 * t; // 约 12°–32°
        return (normalize_lon(lon + bias), lat);
    }
    // 加勒比海盆：对准震中，禁止西偏（否则波圈贴右侧栏、左侧大片空域）
    if (-90.0..-55.0).contains(&lon) && (5.0..28.0).contains(&lat) {
        return (lon, lat);
    }
    // 东太平洋 / 北美西岸：略西偏，避免右侧陆地贴边被裁
    if (-170.0..-50.0).contains(&lon) && (-10.0..70.0).contains(&lat) {
        let bias = 8.0 + 14.0 * t;
        return (normalize_lon(lon - bias), lat);
    }
    (lon, lat)
}

/// 使 `radius_km` 波圈大约落在视口短边的 `fill` 比例内，反推 zoom（平面 Web Mercator）。
pub fn zoom_to_fit_radius_km(lat: f64, radius_km: f64, viewport_min_px: f32, fill: f32) -> f64 {
    let target_px = (viewport_min_px * fill.clamp(0.10, 0.95)).max(32.0);
    if radius_km <= 1.0 {
        return 8.0;
    }
    let mut lo = ZOOM_MIN;
    let mut hi = ZOOM_MAX;
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        let px = projection::km_to_pixels(radius_km, lat, mid);
        if px > target_px {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    ((lo + hi) * 0.5).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// 球体：使震中处 `radius_km` 大圆半径约占视口短边 `fill`，反推 zoom。
/// 用实际投影量测（非 Mercator `km_to_pixels`），避免跟焦缩成「地球小点、S 波离边还很远」。
pub fn zoom_to_fit_radius_km_globe(
    lon: f64,
    lat: f64,
    radius_km: f64,
    viewport_w: f32,
    viewport_h: f32,
    fill: f32,
) -> f64 {
    use crate::globe::{move_on_globe, GlobeCamera};
    use egui::{Pos2, Rect, Vec2};

    let w = viewport_w.max(64.0);
    let h = viewport_h.max(64.0);
    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
    let target_px = (w.min(h) * fill.clamp(0.10, 0.95)).max(32.0) as f64;
    if radius_km <= 1.0 {
        return 11.0;
    }
    let lon = normalize_lon(lon);
    let lat = clamp_lat_globe(lat);
    let mut lo = ZOOM_MIN;
    let mut hi = ZOOM_MAX;
    for _ in 0..28 {
        let mid = (lo + hi) * 0.5;
        let cam = GlobeCamera::new(lon, lat, mid, rect);
        let (elon, elat) = move_on_globe(lon, lat, radius_km, 90.0);
        let px = match (
            cam.try_project(lon, lat, rect),
            cam.try_project(elon, elat, rect),
        ) {
            (Some(c), Some(e)) => c.distance(e) as f64,
            // 已出地平线：过近，需要缩小
            _ => target_px * 2.5,
        };
        if px > target_px {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    ((lo + hi) * 0.5).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// 按投影模式选择平面 / 球体贴边估 zoom。
pub fn zoom_to_fit_radius_for_mode(
    lon: f64,
    lat: f64,
    radius_km: f64,
    viewport_w: f32,
    viewport_h: f32,
    fill: f32,
    mode: projection::ProjectionMode,
) -> f64 {
    if mode.is_globe() {
        zoom_to_fit_radius_km_globe(lon, lat, radius_km, viewport_w, viewport_h, fill)
    } else {
        zoom_to_fit_radius_km(lat, radius_km, viewport_w.min(viewport_h), fill)
    }
}

/// 当前镜头下，震中处 `radius_km` 在屏幕上的像素半径。
/// 对齐 eewcn：用真实投影量测，再与视口短边比较判断是否「贴边」。
pub fn radius_km_screen_px(
    view_lon: f64,
    view_lat: f64,
    zoom: f64,
    epi_lon: f64,
    epi_lat: f64,
    radius_km: f64,
    viewport_w: f32,
    viewport_h: f32,
    mode: projection::ProjectionMode,
) -> f64 {
    if radius_km <= 0.0 {
        return 0.0;
    }
    if mode.is_globe() {
        use crate::globe::{move_on_globe, GlobeCamera};
        use egui::{Pos2, Rect, Vec2};
        let w = viewport_w.max(64.0);
        let h = viewport_h.max(64.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
        let cam = GlobeCamera::new(view_lon, view_lat, zoom, rect);
        let (elon, elat) = move_on_globe(epi_lon, epi_lat, radius_km, 90.0);
        match (
            cam.try_project(epi_lon, epi_lat, rect),
            cam.try_project(elon, elat, rect),
        ) {
            (Some(a), Some(b)) => a.distance(b) as f64,
            // 出地平线：视为已远超贴边
            _ => (w.min(h) as f64) * 0.65,
        }
    } else {
        projection::km_to_pixels(radius_km, epi_lat, zoom) as f64
    }
}

/// 以锚点为中心筛掉过远事件，避免全球多震把镜头拉到 ZOOM_MIN。
/// `max_sep_deg`：平面近似角距（度），约 30° ≈ 3300 km。
pub fn cluster_for_fit(
    points: &[(f64, f64, f64)],
    anchor: Option<(f64, f64)>,
    max_sep_deg: f64,
) -> Vec<(f64, f64, f64)> {
    if points.is_empty() {
        return Vec::new();
    }
    let (alon, alat) = anchor.unwrap_or((points[0].0, points[0].1));
    let max_sep = max_sep_deg.max(5.0);
    let mut out: Vec<(f64, f64, f64)> = points
        .iter()
        .copied()
        .filter(|(lon, lat, _)| {
            let mut dlon = (lon - alon).abs();
            if dlon > 180.0 {
                dlon = 360.0 - dlon;
            }
            let dlat = (lat - alat).abs();
            (dlon * dlon + dlat * dlat).sqrt() <= max_sep
        })
        .collect();
    if out.is_empty() {
        // 锚点本身不在列表时，至少保留最近的一点
        if let Some(p) = points.iter().min_by(|a, b| {
            let da = {
                let mut dlon = (a.0 - alon).abs();
                if dlon > 180.0 {
                    dlon = 360.0 - dlon;
                }
                let dlat = (a.1 - alat).abs();
                dlon * dlon + dlat * dlat
            };
            let db = {
                let mut dlon = (b.0 - alon).abs();
                if dlon > 180.0 {
                    dlon = 360.0 - dlon;
                }
                let dlat = (b.1 - alat).abs();
                dlon * dlon + dlat * dlat
            };
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            out.push(*p);
        }
    }
    out
}

/// 多震：用包围盒 + 各震 S 半径估一个合适缩放与中心。
pub fn fit_events(
    points: &[(f64, f64, f64)],
    viewport_w: f32,
    viewport_h: f32,
) -> Option<(f64, f64, f64)> {
    if points.is_empty() {
        return None;
    }
    if points.len() == 1 {
        let (lon, lat, r_km) = points[0];
        let z = zoom_to_fit_radius_km(lat, r_km.max(50.0), viewport_w.min(viewport_h), 0.52);
        return Some((normalize_lon(lon), clamp_lat(lat), z));
    }

    let mut lon_min = f64::INFINITY;
    let mut lon_max = f64::NEG_INFINITY;
    let mut lat_min = f64::INFINITY;
    let mut lat_max = f64::NEG_INFINITY;
    let mut max_r = 80.0_f64;
    let ref_lon = points[0].0;
    for &(lon, lat, r_km) in points {
        let lon = ref_lon + normalize_lon(lon - ref_lon);
        // 粗略：经度 1°≈111km·cos(lat)，纬度 1°≈111km；半径外扩
        let dlat = r_km / 111.0;
        let dlon = r_km / (111.0 * clamp_lat(lat).to_radians().cos().max(0.2));
        lon_min = lon_min.min(lon - dlon);
        lon_max = lon_max.max(lon + dlon);
        lat_min = lat_min.min(lat - dlat);
        lat_max = lat_max.max(lat + dlat);
        max_r = max_r.max(r_km);
    }
    let center_lon = normalize_lon((lon_min + lon_max) * 0.5);
    let center_lat = clamp_lat((lat_min + lat_max) * 0.5);
    let span_lat = (lat_max - lat_min).max(0.5);
    let span_lon = (lon_max - lon_min).max(0.5);
    let span_km = (span_lat * 111.0)
        .max(span_lon * 111.0 * center_lat.to_radians().cos().abs().max(0.2))
        .max(max_r * 2.0);
    let z = zoom_to_fit_radius_km(
        center_lat,
        span_km * 0.5,
        viewport_w.min(viewport_h),
        0.78,
    );
    Some((center_lon, center_lat, z))
}


#[cfg(test)]
mod flight_timing_tests {
    use super::CameraFlight;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn flight_progress_is_wall_clock_not_tick_count() {
        // 同一墙钟时长：tick 2 次与 tick 20 次都应接近同一进度（与「帧率」无关）
        let dur = 0.20_f64;
        let mut sparse = CameraFlight::new((0.0, 0.0, 6.0), (10.0, 5.0, 8.0), dur, true);
        let mut dense = CameraFlight::new((0.0, 0.0, 6.0), (10.0, 5.0, 8.0), dur, true);
        thread::sleep(Duration::from_millis(100));
        let (_, _, _, _) = sparse.tick(1.0 / 30.0);
        let (lon_s, lat_s, z_s, _) = sparse.tick(1.0 / 30.0);
        for _ in 0..20 {
            let _ = dense.tick(1.0 / 120.0);
        }
        let (lon_d, lat_d, z_d, _) = dense.tick(1.0 / 120.0);
        assert!((lon_s - lon_d).abs() < 0.35, "lon {lon_s} vs {lon_d}");
        assert!((lat_s - lat_d).abs() < 0.25, "lat {lat_s} vs {lat_d}");
        assert!((z_s - z_d).abs() < 0.35, "zoom {z_s} vs {z_d}");
        assert!((sparse.duration_s() - dur).abs() < 1e-9);
    }

    #[test]
    fn globe_mid_flight_near_vs_far() {
        // 远距：整球占满窗口，不得缩成小点
        let far = super::mid_flight_zoom(9.5, 10.0, 9000.0, true);
        assert!((far - super::GLOBE_FILL_ZOOM).abs() < 0.15, "far={far}");
        // 近距：保持较近，不进全球地板
        let near = super::mid_flight_zoom(9.0, 9.5, 180.0, true);
        assert!(near >= 7.8, "near={near}");
        assert!(near > far + 0.5, "near={near} far={far}");
    }
}

#[cfg(test)]
mod fit_radius_mode_tests {
    use super::{zoom_to_fit_radius_for_mode, zoom_to_fit_radius_km};
    use crate::projection::ProjectionMode;

    #[test]
    fn globe_fit_closer_than_mercator_for_mid_s() {
        // ~1200km S：球体贴边所需 zoom 应明显高于 Mercator（否则会缩成小地球）
        let r = 1200.0_f64;
        let flat = zoom_to_fit_radius_km(0.0, r, 700.0, 0.48);
        let globe = zoom_to_fit_radius_for_mode(
            160.0,
            -9.0,
            r,
            700.0,
            700.0,
            0.48,
            ProjectionMode::Globe,
        );
        assert!(
            globe > flat + 1.0,
            "globe={globe} flat={flat} (globe must stay closer)"
        );
        assert!(globe >= 8.0, "globe={globe}");
    }
}

#[cfg(test)]
mod framed_center_tests {
    use super::globe_framed_center;

    #[test]
    fn guyuan_stays_on_epicenter() {
        let (lon, lat) = globe_framed_center(106.2, 36.0, 8.5);
        assert!((lon - 106.2).abs() < 1e-9, "lon={lon}");
        assert!((lat - 36.0).abs() < 1e-9, "lat={lat}");
    }

    #[test]
    fn japan_gets_east_bias_when_zoomed_out() {
        let (lon, _) = globe_framed_center(140.0, 35.0, 8.5);
        assert!(lon > 140.0, "lon={lon}");
    }

    #[test]
    fn near_zoom_no_bias() {
        let (lon, _) = globe_framed_center(140.0, 35.0, 11.5);
        assert!((lon - 140.0).abs() < 1e-9);
    }

    #[test]
    fn caribbean_stays_on_epicenter() {
        // 莫纳海峡附近：不得被东太平洋西偏顶到右侧
        let (lon, lat) = globe_framed_center(-67.5, 18.0, 8.5);
        assert!((lon - (-67.5)).abs() < 1e-9, "lon={lon}");
        assert!((lat - 18.0).abs() < 1e-9, "lat={lat}");
    }

    #[test]
    fn kamchatka_stays_on_epicenter() {
        // 堪察加东岸近海：不得被日本构图东偏拉到日界线
        let (lon, lat) = globe_framed_center(160.3, 52.5, 7.5);
        assert!((lon - 160.3).abs() < 1e-9, "lon={lon}");
        assert!((lat - 52.5).abs() < 1e-9, "lat={lat}");
    }
}
