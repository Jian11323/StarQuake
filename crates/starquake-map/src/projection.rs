//! Web Mercator 投影（XYZ 瓦片坐标系；经度相对中心折到 ±180°，支持循环地图）。
//! 另提供平面 / 球体统一投影门面（球体见 [`crate::globe`]）。

use crate::globe::{self, GlobeCamera};
use egui::{Pos2, Rect, Vec2};
use starquake_util::EARTH_RADIUS_KM;

/// 地图投影模式：平面 Web Mercator / 立体球。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProjectionMode {
    #[default]
    Flat,
    Globe,
}

impl ProjectionMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "globe" | "sphere" | "3d" | "ball" => Self::Globe,
            _ => Self::Flat,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Globe => "globe",
        }
    }

    pub fn is_globe(self) -> bool {
        matches!(self, Self::Globe)
    }
}

pub const TILE_SIZE: f64 = 256.0;

/// 将纬度钳到 Web Mercator 有效范围（约 ±85.0511°）
pub fn clamp_lat(lat: f64) -> f64 {
    lat.clamp(-85.05112878, 85.05112878)
}

/// 球体镜头中心纬度：可接近极点，但避开精确 ±90°（方位角/经度奇点）。
pub fn clamp_lat_globe(lat: f64) -> f64 {
    lat.clamp(-89.9, 89.9)
}

/// 经度归一到 (-180, 180]，对应 Leaflet `worldCopyJump` 后的中心经度。
pub fn normalize_lon(lon: f64) -> f64 {
    let mut x = (lon + 180.0).rem_euclid(360.0) - 180.0;
    if x <= -180.0 {
        x = 180.0;
    }
    x
}

/// 将 `lon` 折到距 `center_lon` 最短弧，避免跨日界线拉出贯穿屏幕的线段。
pub fn wrap_lon_near(lon: f64, center_lon: f64) -> f64 {
    center_lon + normalize_lon(lon - center_lon)
}

/// 缩放级别 z 下世界像素边长
pub fn world_size(zoom: f64) -> f64 {
    TILE_SIZE * 2f64.powf(zoom)
}

/// 经纬 → 世界像素坐标（原点左上，y 向下）。`lon` 可超出 ±180（循环副本）。
pub fn lonlat_to_world(lon: f64, lat: f64, zoom: f64) -> (f64, f64) {
    let lat = clamp_lat(lat);
    let w = world_size(zoom);
    let x = (lon + 180.0) / 360.0 * w;
    let sin_lat = (lat * std::f64::consts::PI / 180.0).sin();
    let y = (0.5 - (0.25 * std::f64::consts::FRAC_1_PI) * ((1.0 + sin_lat) / (1.0 - sin_lat)).ln())
        * w;
    (x, y)
}

/// 世界像素 → 经纬（经度不强制折回 ±180，便于连续平移）
pub fn world_to_lonlat(wx: f64, wy: f64, zoom: f64) -> (f64, f64) {
    let w = world_size(zoom);
    let lon = wx / w * 360.0 - 180.0;
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * wy / w;
    let lat = 180.0 / std::f64::consts::PI * n.sinh().atan();
    (lon, lat)
}

/// 视口中心经纬 + 视口矩形 → 屏幕像素（经度相对中心最短弧）
pub fn lonlat_to_screen(
    lon: f64,
    lat: f64,
    center_lon: f64,
    center_lat: f64,
    zoom: f64,
    rect: Rect,
) -> Pos2 {
    let lon = wrap_lon_near(lon, center_lon);
    let (cx, cy) = lonlat_to_world(center_lon, center_lat, zoom);
    let (wx, wy) = lonlat_to_world(lon, lat, zoom);
    let dx = (wx - cx) as f32;
    let dy = (wy - cy) as f32;
    rect.center() + Vec2::new(dx, dy)
}

/// 屏幕像素偏移 → 经纬中心变化（拖拽用）；结束后将经度折回 (-180, 180]
pub fn pan_by_screen_delta(
    center_lon: f64,
    center_lat: f64,
    zoom: f64,
    dx_px: f32,
    dy_px: f32,
) -> (f64, f64) {
    let (cx, cy) = lonlat_to_world(center_lon, center_lat, zoom);
    // 屏幕右拖 = 地图左移 → 中心世界坐标减小 x
    let (lon, lat) = world_to_lonlat(cx - dx_px as f64, cy - dy_px as f64, zoom);
    (normalize_lon(lon), clamp_lat(lat))
}

/// 表面距离（km）→ 屏幕像素半径（以中心纬度的赤道近似）
pub fn km_to_pixels(km: f64, lat: f64, zoom: f64) -> f32 {
    let w = world_size(zoom);
    let meters_per_pixel =
        (clamp_lat(lat).to_radians().cos() * 2.0 * std::f64::consts::PI * EARTH_RADIUS_KM * 1000.0)
            / w;
    if meters_per_pixel <= 1e-9 {
        return 0.0;
    }
    (km * 1000.0 / meters_per_pixel) as f32
}

/// 可见瓦片 XYZ 范围（含边距）。`zoom` 可为小数；瓦片层级取 `floor(zoom)`。
/// `x` 允许越界（循环地图）；`y` 钳在 `[0, n)`。
pub fn visible_tiles(
    center_lon: f64,
    center_lat: f64,
    zoom: f64,
    rect: Rect,
    pad: i32,
) -> (u8, i32, i32, i32, i32) {
    let z = zoom.floor().clamp(0.0, 18.0) as u8;
    let zf = z as f64;
    let scale = 2f64.powf(zoom - zf);
    let n = 1i32 << z;
    let (cx, cy) = lonlat_to_world(center_lon, center_lat, zf);
    // 屏幕半宽对应的 z 级世界像素（已除以放大倍率）
    let half_w = rect.width() as f64 * 0.5 / scale;
    let half_h = rect.height() as f64 * 0.5 / scale;
    let x0 = ((cx - half_w) / TILE_SIZE).floor() as i32 - pad;
    let y0 = ((cy - half_h) / TILE_SIZE).floor() as i32 - pad;
    let x1 = ((cx + half_w) / TILE_SIZE).floor() as i32 + pad;
    let y1 = ((cy + half_h) / TILE_SIZE).floor() as i32 + pad;
    (z, x0, y0.clamp(0, n - 1), x1, y1.clamp(0, n - 1))
}

/// 将可能越界的瓦片列号折到 `[0, n)`（循环取图）
pub fn wrap_tile_x(x: i32, z: u8) -> u32 {
    let n = 1i32 << z;
    (((x % n) + n) % n) as u32
}

/// 瓦片左上角在该 z 级下的世界像素坐标
pub fn tile_world_origin(x: i32, y: i32, _z: u8) -> (f64, f64) {
    (x as f64 * TILE_SIZE, y as f64 * TILE_SIZE)
}

/// 单帧视口投影上下文（球体相机按帧缓存）。
#[derive(Clone, Copy)]
pub struct ViewCtx {
    pub mode: ProjectionMode,
    pub center_lon: f64,
    pub center_lat: f64,
    pub zoom: f64,
    pub rect: Rect,
    globe: Option<GlobeCamera>,
}

impl ViewCtx {
    pub fn new(
        mode: ProjectionMode,
        center_lon: f64,
        center_lat: f64,
        zoom: f64,
        rect: Rect,
    ) -> Self {
        let globe = if mode.is_globe() {
            Some(GlobeCamera::new(center_lon, center_lat, zoom, rect))
        } else {
            None
        };
        Self {
            mode,
            center_lon,
            center_lat,
            zoom,
            rect,
            globe,
        }
    }

    pub fn project(&self, lon: f64, lat: f64) -> Option<Pos2> {
        match self.mode {
            ProjectionMode::Flat => Some(lonlat_to_screen(
                lon,
                lat,
                self.center_lon,
                self.center_lat,
                self.zoom,
                self.rect,
            )),
            ProjectionMode::Globe => self.globe.and_then(|c| c.try_project(lon, lat, self.rect)),
        }
    }

    /// 陆地填色投影（球体放宽地平线）。
    pub fn project_fill(&self, lon: f64, lat: f64) -> Option<Pos2> {
        match self.mode {
            ProjectionMode::Flat => self.project(lon, lat),
            ProjectionMode::Globe => {
                self.globe.and_then(|c| c.try_project_fill(lon, lat, self.rect))
            }
        }
    }

    /// 陆地填色兜底：略出地平线也投影（配合圆形遮罩）。
    pub fn project_fill_raw(&self, lon: f64, lat: f64) -> Option<Pos2> {
        match self.mode {
            ProjectionMode::Flat => self.project(lon, lat),
            ProjectionMode::Globe => self.globe.map(|c| c.project_fill_raw(lon, lat, self.rect)),
        }
    }

    pub fn pan_by_delta(&self, dx_px: f32, dy_px: f32) -> (f64, f64) {
        match self.mode {
            ProjectionMode::Flat => {
                pan_by_screen_delta(self.center_lon, self.center_lat, self.zoom, dx_px, dy_px)
            }
            ProjectionMode::Globe => self
                .globe
                .map(|c| c.pan_by_delta(dx_px, dy_px))
                .unwrap_or((self.center_lon, self.center_lat)),
        }
    }

    pub fn screen_to_lonlat(&self, pos: Pos2) -> (f64, f64) {
        match self.mode {
            ProjectionMode::Flat => {
                let (cx, cy) = lonlat_to_world(self.center_lon, self.center_lat, self.zoom);
                let wx = cx + (pos.x - self.rect.center().x) as f64;
                let wy = cy + (pos.y - self.rect.center().y) as f64;
                world_to_lonlat(wx, wy, self.zoom)
            }
            ProjectionMode::Globe => self
                .globe
                .map(|c| c.screen_to_lonlat(pos, self.rect))
                .unwrap_or((self.center_lon, self.center_lat)),
        }
    }

    pub fn horizon_radius(&self) -> Option<f32> {
        self.globe.map(|c| c.horizon_dist as f32)
    }

    /// 球体可见半球半角（度）；平面返回 `None`。
    pub fn horizon_half_deg(&self) -> Option<f64> {
        self.globe.map(|c| c.horizon_half_deg)
    }

    /// 点是否在地平线以上（对齐 GQ `isAboveHorizon`）。
    pub fn is_above_horizon(&self, lon: f64, lat: f64) -> bool {
        match self.mode {
            ProjectionMode::Flat => true,
            ProjectionMode::Globe => self
                .globe
                .is_some_and(|c| c.is_above_horizon(lon, lat)),
        }
    }

    /// 视口中心每经度对应的屏幕像素（仅球体）。
    pub fn one_deg_px(&self) -> Option<f64> {
        self.globe.map(|c| c.one_deg_px)
    }

    /// 屏幕像素 → 地表 km（仅球体；平面返回 `None`）。
    pub fn px_to_km(&self, px: f32) -> Option<f64> {
        self.globe.map(|c| c.px_to_km(px))
    }

    /// 球体贴面方块四角（TL/TR/BR/BL）。对齐 GQ `createSquare`：大圆偏移后 ECEF 投影。
    /// 任一角低于地平线则 `None`（调用方可回退中心 billboard）。
    pub fn project_surface_square(
        &self,
        lon: f64,
        lat: f64,
        side_px: f32,
    ) -> Option<[Pos2; 4]> {
        let cam = self.globe?;
        if side_px < 1.0 {
            return None;
        }
        // 边长 side → 中心到角点 = side/2 * √2
        let half_km = cam.px_to_km(side_px * 0.5);
        let corner_km = half_km * std::f64::consts::SQRT_2;
        // 北向上：NW / NE / SE / SW → 屏幕 TL / TR / BR / BL
        const BEARINGS: [f64; 4] = [315.0, 45.0, 135.0, 225.0];
        let mut out = [Pos2::ZERO; 4];
        for (i, br) in BEARINGS.iter().enumerate() {
            let (clon, clat) = globe::move_on_globe(lon, lat, corner_km, *br);
            out[i] = cam.try_project(clon, clat, self.rect)?;
        }
        Some(out)
    }

    pub fn circle_segments(&self, lon: f64, lat: f64, radius_km: f64) -> Vec<Vec<Pos2>> {
        let Some(cam) = self.globe else {
            return Vec::new();
        };
        let step = if radius_km < 200.0 {
            4.0
        } else if radius_km < 1500.0 {
            3.0
        } else {
            2.0
        };
        globe::project_polyline_km(&cam, lon, lat, radius_km, self.rect, step)
    }
}

