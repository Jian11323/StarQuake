//! 球体投影（对齐 GlobalQuake `GlobeRenderer`：ECEF + yaw/pitch + FOV 透视 + 地平线裁剪）。

use egui::{Pos2, Rect};
use starquake_util::EARTH_RADIUS_KM;

// 1 / tan(30°) ≈ √3；对齐 GlobalQuake FOV=60°
const PROJ_W: f64 = 1.7320508075688772;

/// 缩放 → 相机高度（地球半径倍数）。数值越小越近。
pub fn zoom_to_scroll(zoom: f64) -> f64 {
    2f64.powf(6.5 - zoom).clamp(0.05, 12.0)
}

#[derive(Debug, Clone, Copy)]
pub struct GlobeCamera {
    pub center_lon: f64,
    pub center_lat: f64,
    pub camera_altitude_km: f64,
    cos_yaw: f64,
    sin_yaw: f64,
    cos_pitch: f64,
    sin_pitch: f64,
    camera_x: f64,
    camera_y: f64,
    camera_z: f64,
    max_distance: f64,
    pub one_deg_px: f64,
    pub horizon_dist: f64,
    /// 相机到地平线的地心角（度），用于球体图层裁剪范围。
    pub horizon_half_deg: f64,
    width: f64,
    height: f64,
}

impl GlobeCamera {
    pub fn new(center_lon: f64, center_lat: f64, zoom: f64, rect: Rect) -> Self {
        let center_lon = crate::projection::normalize_lon(center_lon);
        let center_lat = crate::projection::clamp_lat_globe(center_lat);
        let scroll = zoom_to_scroll(zoom);
        let camera_altitude_km = EARTH_RADIUS_KM * scroll;
        let yaw = -center_lon.to_radians();
        let pitch = (180.0 - center_lat).to_radians();
        let cos_yaw = yaw.cos();
        let sin_yaw = yaw.sin();
        let cos_pitch = pitch.cos();
        let sin_pitch = pitch.sin();

        let (camera_x, camera_y, camera_z) =
            lonlat_to_ecef(center_lon, center_lat, camera_altitude_km);

        let mut cam = Self {
            center_lon,
            center_lat,
            camera_altitude_km,
            cos_yaw,
            sin_yaw,
            cos_pitch,
            sin_pitch,
            camera_x,
            camera_y,
            camera_z,
            max_distance: 0.0,
            one_deg_px: 1.0,
            horizon_dist: 1.0,
            horizon_half_deg: 90.0,
            width: rect.width() as f64,
            height: rect.height() as f64,
        };

        let center_to_camera =
            (camera_x * camera_x + camera_y * camera_y + camera_z * camera_z).sqrt();
        let max_angle = (EARTH_RADIUS_KM / center_to_camera.max(EARTH_RADIUS_KM + 1e-6)).acos();
        cam.horizon_half_deg = max_angle.to_degrees().clamp(20.0, 95.0);
        let arc_km =
            2.0 * std::f64::consts::PI * EARTH_RADIUS_KM * (max_angle / (2.0 * std::f64::consts::PI));
        let (h_lon, h_lat) = move_on_globe(center_lon, center_lat, arc_km, 0.0);
        let (hx, hy, hz) = lonlat_to_ecef(h_lon, h_lat, 0.0);
        cam.max_distance = dist3(cam.camera_x, cam.camera_y, cam.camera_z, hx, hy, hz);

        // 1° 弧长采样：朝最近极的反方向，避免贴极/Mercator 钳位把步长压成 0
        // （否则 one_deg_px→0 → px_to_km 爆炸，波圈/贴面标记瞬间放大）。
        let bearing = if center_lat >= 0.0 { 180.0 } else { 0.0 };
        let (m_lon, m_lat) = move_on_globe(center_lon, center_lat, deg_to_km(1.0), bearing);
        let p0 = cam.project_lonlat_raw(center_lon, center_lat);
        let p1 = cam.project_lonlat_raw(m_lon, m_lat);
        let measured = (p0.0 - p1.0).hypot(p0.1 - p1.1);

        let (hx_s, hy_s) = cam.project_xyz(hx, hy, hz);
        let cx = cam.width * 0.5;
        let cy = cam.height * 0.5;
        cam.horizon_dist = (hx_s - cx).hypot(hy_s - cy).max(1.0);
        // 地平线约 90° 弧；用其兜底，防止测量失败时比例尺塌缩
        let floor = (cam.horizon_dist / 120.0).max(1e-3);
        cam.one_deg_px = measured.max(floor);
        cam
    }

    fn project_xyz(&self, x: f64, y: f64, z: f64) -> (f64, f64) {
        let new_x = x * self.cos_yaw + z * self.sin_yaw;
        let new_z = z * self.cos_yaw - x * self.sin_yaw;
        let new_y = y;

        let tmp_y = new_y * self.cos_pitch - new_z * self.sin_pitch;
        let tmp_z = new_z * self.cos_pitch + new_y * self.sin_pitch;
        let mut new_z = tmp_z;
        let new_y = tmp_y;

        new_z -= EARTH_RADIUS_KM + self.camera_altitude_km;

        let w = PROJ_W * new_z;
        if w.abs() < 1e-12 {
            return (self.width * 0.5, self.height * 0.5);
        }
        let screen_x = (new_x / w + 1.0) * self.width - self.width * 0.5;
        let screen_y = (1.0 - new_y / w) * self.width - self.width + self.height * 0.5;
        (screen_x, screen_y)
    }

    fn project_lonlat_raw(&self, lon: f64, lat: f64) -> (f64, f64) {
        let (x, y, z) = lonlat_to_ecef(lon, lat, 0.0);
        self.project_xyz(x, y, z)
    }

    pub fn is_above_horizon(&self, lon: f64, lat: f64) -> bool {
        let (x, y, z) = lonlat_to_ecef(lon, lat, 0.0);
        let d = dist3(self.camera_x, self.camera_y, self.camera_z, x, y, z);
        d <= self.max_distance * 1.002
    }

    /// 屏幕像素 → 地表弧长（km）；比例取自视口中心 `one_deg_px`（对齐 GQ `pxToDeg`）。
    pub fn px_to_km(&self, px: f32) -> f64 {
        let km_per_deg = EARTH_RADIUS_KM * std::f64::consts::PI / 180.0;
        (px as f64) / self.one_deg_px.max(1e-6) * km_per_deg
    }

    /// 陆地填色用：放宽地平线，减少美洲/欧亚在球缘整块掉线。
    pub fn is_above_horizon_fill(&self, lon: f64, lat: f64) -> bool {
        let (x, y, z) = lonlat_to_ecef(lon, lat, 0.0);
        let d = dist3(self.camera_x, self.camera_y, self.camera_z, x, y, z);
        d <= self.max_distance * 1.14
    }

    /// 填色兜底投影：即使略低于地平线也投到屏幕（由圆形遮罩裁切）。
    pub fn project_fill_raw(&self, lon: f64, lat: f64, rect: Rect) -> Pos2 {
        let (sx, sy) = self.project_lonlat_raw(lon, lat);
        Pos2::new(rect.left() + sx as f32, rect.top() + sy as f32)
    }

    /// 投影到屏幕；背面返回 `None`。
    pub fn try_project(&self, lon: f64, lat: f64, rect: Rect) -> Option<Pos2> {
        if !self.is_above_horizon(lon, lat) {
            return None;
        }
        let (sx, sy) = self.project_lonlat_raw(lon, lat);
        Some(Pos2::new(rect.left() + sx as f32, rect.top() + sy as f32))
    }

    /// 填色投影：放宽地平线，仍拒绝真正背面以免穿球。
    pub fn try_project_fill(&self, lon: f64, lat: f64, rect: Rect) -> Option<Pos2> {
        if !self.is_above_horizon_fill(lon, lat) {
            return None;
        }
        let (sx, sy) = self.project_lonlat_raw(lon, lat);
        Some(Pos2::new(rect.left() + sx as f32, rect.top() + sy as f32))
    }

    pub fn pan_by_delta(&self, dx_px: f32, dy_px: f32) -> (f64, f64) {
        // 与平面一致：右拖中心西移；下拖中心北移
        let dlon = -(dx_px as f64) / self.one_deg_px;
        let dlat = (dy_px as f64) / self.one_deg_px;
        let lon = crate::projection::normalize_lon(self.center_lon + dlon);
        let lat = crate::projection::clamp_lat_globe(self.center_lat + dlat);
        (lon, lat)
    }

    pub fn screen_to_lonlat(&self, pos: Pos2, rect: Rect) -> (f64, f64) {
        let dx = (pos.x - rect.center().x) as f64;
        let dy = (pos.y - rect.center().y) as f64;
        let dlon = dx / self.one_deg_px;
        let dlat = -dy / self.one_deg_px;
        (
            crate::projection::normalize_lon(self.center_lon + dlon),
            crate::projection::clamp_lat_globe(self.center_lat + dlat),
        )
    }
}

/// 经纬 + 海拔(km) → ECEF（km），坐标约定对齐 GlobalQuake。
pub fn lonlat_to_ecef(lon: f64, lat: f64, alt_km: f64) -> (f64, f64, f64) {
    let r = EARTH_RADIUS_KM + alt_km;
    let lon_r = lon.to_radians();
    let lat_r = lat.to_radians();
    let x = -r * lon_r.sin() * lat_r.cos();
    let y = r * lat_r.sin();
    let z = -r * lon_r.cos() * lat_r.cos();
    (x, y, z)
}

fn dist3(ax: f64, ay: f64, az: f64, bx: f64, by: f64, bz: f64) -> f64 {
    (ax - bx).hypot(ay - by).hypot(az - bz)
}

fn deg_to_km(deg: f64) -> f64 {
    deg / 360.0 * 2.0 * std::f64::consts::PI * EARTH_RADIUS_KM
}

/// 两点间初始方位角（度，0=北，顺时针）。
pub fn bearing_deg(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dl = (lon2 - lon1).to_radians();
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

/// 从 (lon,lat) 沿方位角 `bearing_deg`（0=北）移动 `distance_km`。
/// 纬度按地理 ±90° 钳位（勿用 Web Mercator 的 ±85°，否则极区波圈被压扁）。
pub fn move_on_globe(lon: f64, lat: f64, distance_km: f64, bearing_deg: f64) -> (f64, f64) {
    let ang = distance_km / EARTH_RADIUS_KM;
    let br = bearing_deg.to_radians();
    let lat1 = lat.to_radians();
    let lon1 = lon.to_radians();
    let sin_lat1 = lat1.sin();
    let cos_lat1 = lat1.cos();
    let sin_ang = ang.sin();
    let cos_ang = ang.cos();
    let lat2 = (sin_lat1 * cos_ang + cos_lat1 * sin_ang * br.cos()).asin();
    let lon2 =
        lon1 + (br.sin() * sin_ang * cos_lat1).atan2(cos_ang - sin_lat1 * lat2.sin());
    (
        crate::projection::normalize_lon(lon2.to_degrees()),
        lat2.to_degrees().clamp(-90.0, 90.0),
    )
}

/// 将大圆折线按地平线可见性拆成多段。
pub fn project_polyline_km(
    cam: &GlobeCamera,
    lon: f64,
    lat: f64,
    radius_km: f64,
    rect: Rect,
    step_deg: f64,
) -> Vec<Vec<Pos2>> {
    if radius_km < 1e-3 {
        return Vec::new();
    }
    let mut segments: Vec<Vec<Pos2>> = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    let max_jump = (cam.one_deg_px * step_deg * 3.0).max(40.0) as f32;
    let max_jump2 = max_jump * max_jump;
    let mut ang = 0.0;
    while ang <= 360.0 + 1e-6 {
        let (plon, plat) = move_on_globe(lon, lat, radius_km, ang);
        match cam.try_project(plon, plat, rect) {
            Some(p) => {
                if let Some(prev) = cur.last() {
                    let dx = p.x - prev.x;
                    let dy = p.y - prev.y;
                    if dx * dx + dy * dy > max_jump2 {
                        if cur.len() >= 2 {
                            segments.push(std::mem::take(&mut cur));
                        } else {
                            cur.clear();
                        }
                    }
                }
                cur.push(p);
            }
            None => {
                if cur.len() >= 2 {
                    segments.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
        }
        ang += step_deg;
    }
    if cur.len() >= 2 {
        segments.push(cur);
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Rect;

    #[test]
    fn move_on_globe_can_reach_near_pole() {
        let (_lon, lat) = move_on_globe(160.0, 52.0, 4200.0, 0.0);
        assert!(
            lat > 85.1,
            "geographic poleward move must not be stuck at Mercator 85°, got {lat}"
        );
    }

    #[test]
    fn one_deg_px_stable_near_north_clamp() {
        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(900.0, 800.0));
        // 旧逻辑贴 Mercator 上限朝北采样会把 one_deg 压成 ~0
        let cam = GlobeCamera::new(160.0, 85.05112878, 7.0, rect);
        assert!(
            cam.one_deg_px > 1.0,
            "one_deg_px must stay usable at polar view, got {}",
            cam.one_deg_px
        );
        let cam2 = GlobeCamera::new(160.0, 89.9, 7.0, rect);
        assert!(
            cam2.one_deg_px > 1.0,
            "one_deg_px must stay usable at 89.9N, got {}",
            cam2.one_deg_px
        );
    }

    #[test]
    fn px_to_km_not_exploding_at_pole() {
        let rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(900.0, 800.0));
        let cam = GlobeCamera::new(0.0, 85.05112878, 7.0, rect);
        let km = cam.px_to_km(26.0);
        assert!(
            km < 5000.0,
            "26px must not map to planetary km at pole, got {km}"
        );
    }
}
