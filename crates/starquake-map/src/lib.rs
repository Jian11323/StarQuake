//! 地图视口：GeoJSON 矢量底图（默认）/ 可选 XYZ 瓦片 + 走时表驱动 P/S 叠加层。

mod camera;
mod fault_lines;
mod globe;
mod marker_icons;
mod overlay;
mod projection;
mod tile_cache;
mod weather_overlay;
mod weather_paint;
mod topojson;
mod tsunami_coast;
mod vector_basemap;

pub use camera::{
    cluster_for_fit, fit_events, globe_framed_center, radius_km_screen_px,
    zoom_to_fit_radius_for_mode, zoom_to_fit_radius_km, CameraFlight,
};
pub use overlay::{StationPaintCache, TyphoonHoverHit};
pub use projection::{
    clamp_lat, clamp_lat_globe, km_to_pixels, lonlat_to_screen, normalize_lon, ProjectionMode,
};
pub use tile_cache::TileCache;
pub use vector_basemap::{BasemapLoadState, BasemapMode, VectorBasemap, VectorStyle, OCEAN};

fn repaint_floor_id() -> egui::Id {
    egui::Id::new("starquake_repaint_floor_ms")
}

/// 本帧重绘下限（毫秒）。空闲时由应用层设为 1000+，避免地图内短间隔抢跑。
pub fn set_repaint_floor_ms(ctx: &egui::Context, ms: u64) {
    ctx.data_mut(|d| {
        d.insert_temp(repaint_floor_id(), ms);
    });
}

/// 预约重绘，但不会快于当前帧的 `set_repaint_floor_ms`。
pub fn request_repaint_after_capped(ctx: &egui::Context, ms: u64) {
    let floor = ctx.data(|d| d.get_temp::<u64>(repaint_floor_id()).unwrap_or(0));
    ctx.request_repaint_after(std::time::Duration::from_millis(ms.max(floor).max(1)));
}

/// 绘制质量：随性能档位降低测站密度 / 矢量细度，减轻老 GPU/CPU。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaintQuality {
    Eco,
    #[default]
    Normal,
    Extreme,
}

impl PaintQuality {
    pub fn from_profile(profile: &str) -> Self {
        match profile.to_ascii_lowercase().as_str() {
            "eco" => Self::Eco,
            "extreme" => Self::Extreme,
            _ => Self::Normal,
        }
    }

    /// 拖拽/飞跃时的绘制档：节能仍狠抽；正常略降；极致几乎不降，用满空闲算力换丝滑。
    pub fn while_interacting(self) -> Self {
        match self {
            Self::Eco => Self::Eco,
            Self::Normal => Self::Eco,
            Self::Extreme => Self::Normal,
        }
    }
}

/// 地图一帧交互结果。
#[derive(Clone)]
pub struct MapUiResult {
    pub click: Option<(f64, f64)>,
    pub typhoon_hover: Option<TyphoonHoverHit>,
    /// 地图命中区响应（侧栏关闭时挂右键菜单）
    pub map_response: Option<egui::Response>,
}

impl Default for MapUiResult {
    fn default() -> Self {
        Self {
            click: None,
            typhoon_hover: None,
            map_response: None,
        }
    }
}

use egui::{Color32, Sense, Stroke, Ui};
use starquake_core::{
    CencIntEvent, EewReport, EqRecord, PrecipFrame, RadarFrame, StationSample, TsunamiInfo,
    TyphoonInfo, VolcanoNotice, WeatherAlert, LocalHypoEstimate,
};
use starquake_travel::TravelEngine;
use marker_icons::MarkerIcons;
use projection::ViewCtx;
use fault_lines::FaultLines;
use weather_overlay::WeatherOverlay;
use tsunami_coast::TsunamiCoast;
use tile_cache::TileCache as Cache;
use std::sync::Arc;

const ZOOM_MIN: f64 = 2.0;
const ZOOM_MAX: f64 = 12.0;

/// 视角静止时复用的底图/断层 mesh，波圈等动态层仍每帧绘制。
/// S 波跟焦缩放时对已镶嵌层做平面缩放+平移，避免每帧全量重画导致顿挫。
struct StillStatic {
    key: u64,
    /// 不含中心/缩放的稳定键（质量、样式、是否断层等）
    base_key: u64,
    /// 上次全量重建锚点（漂移过大才重裁）
    anchor_lon: f64,
    anchor_lat: f64,
    anchor_zoom: f64,
    /// 当前 mesh 已对齐的视角
    mesh_lon: f64,
    mesh_lat: f64,
    mesh_zoom: f64,
    layers: Vec<Arc<egui::epaint::Mesh>>,
}

pub struct MapViewport {
    pub center_lon: f64,
    pub center_lat: f64,
    pub zoom: f64,
    pub basemap: BasemapMode,
    pub projection_mode: ProjectionMode,
    ocean_fill: Color32,
    vector_style: VectorStyle,
    cache: Cache,
    vector: VectorBasemap,
    tsunami_coast: TsunamiCoast,
    faults: FaultLines,
    weather_overlay: WeatherOverlay,
    markers: MarkerIcons,
    flight: Option<CameraFlight>,
    /// 飞跃刚结束后抑制波圈自动缩放，避免落地瞬间被二次改 zoom「弹一下」
    post_flight_until: Option<std::time::Instant>,
    /// 用户拖拽/滚轮后暂停自动跟焦缩放，直到下一次主动飞跃或空闲超时
    pub user_camera_override: bool,
    /// 最近一次用户改镜头时刻（用于空闲自动恢复跟焦）
    pub last_user_camera_at: Option<std::time::Instant>,
    /// 最近一帧视口短边（px），供跟焦时按波圈估 zoom
    pub last_viewport_min_px: f32,
    /// 性能档对应的绘制抽稀
    paint_quality: PaintQuality,
    /// 平面静默测站 mesh（拖图平移复用）
    station_cache: overlay::StationPaintCache,
    /// 非交互时合成底图层，避免波圈动画每帧再走一遍矢量路径
    still_static: Option<StillStatic>,
    show_terminator: bool,
    /// S 波跟焦等程序改镜头：下一帧按「软交互」画断层（量化/变换，勿每帧全量）
    follow_cam_anim: bool,
}

impl Default for MapViewport {
    fn default() -> Self {
        Self {
            center_lon: 135.0,
            center_lat: 35.0,
            zoom: 6.8,
            basemap: BasemapMode::Vector,
            projection_mode: ProjectionMode::Flat,
            ocean_fill: OCEAN,
            vector_style: VectorStyle::dark(),
            cache: Cache::new(
                "https://tilemap.sismotide.top".into(),
                "arcwob".into(),
            ),
            vector: VectorBasemap::default(),
            tsunami_coast: TsunamiCoast::default(),
            faults: FaultLines::default(),
            weather_overlay: WeatherOverlay::default(),
            markers: MarkerIcons::default(),
            flight: None,
            post_flight_until: None,
            user_camera_override: false,
            last_user_camera_at: None,
            last_viewport_min_px: 720.0,
            paint_quality: PaintQuality::Normal,
            station_cache: overlay::StationPaintCache::default(),
            still_static: None,
            show_terminator: false,
            follow_cam_anim: false,
        }
    }
}

impl MapViewport {
    pub fn set_projection_mode(&mut self, mode: ProjectionMode) {
        self.projection_mode = mode;
    }

    pub fn configure(
        &mut self,
        basemap: BasemapMode,
        tile_base: impl Into<String>,
        tile_source: impl Into<String>,
        geodata_dir: Option<&std::path::Path>,
        show_faults: bool,
    ) {
        self.basemap = basemap;
        self.cache
            .set_source(tile_base.into(), tile_source.into());
        if let Some(dir) = geodata_dir {
            if basemap.show_vector() || self.projection_mode.is_globe() {
                self.vector
                    .load_dir(dir, self.projection_mode.is_globe());
            }
            self.tsunami_coast.load_dir(dir);
            if show_faults {
                self.faults.load_dir(dir);
            } else {
                self.faults.clear();
            }
        } else if !show_faults {
            self.faults.clear();
        }
    }

    pub fn configure_tiles(&mut self, tile_base: impl Into<String>, tile_source: impl Into<String>) {
        self.cache
            .set_source(tile_base.into(), tile_source.into());
    }

    /// 一键清除内存瓦片缓存。
    pub fn clear_tile_cache(&mut self) {
        self.cache.clear();
    }

    /// 按本机内存五档调整瓦片张数上限（2G…大于16G → 64…800）。
    pub fn set_tile_capacity(&mut self, slots: usize) {
        self.cache.set_capacity(slots);
    }

    fn clamp_view_lat(&self, lat: f64) -> f64 {
        if self.projection_mode.is_globe() {
            projection::clamp_lat_globe(lat)
        } else {
            projection::clamp_lat(lat)
        }
    }

    /// 列表/图例联动：瞬时居中到经纬，可选改缩放
    pub fn center_on(&mut self, lon: f64, lat: f64, zoom: Option<f64>) {
        self.flight = None;
        self.post_flight_until =
            Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(1.2));
        self.center_lon = projection::normalize_lon(lon);
        self.center_lat = self.clamp_view_lat(lat);
        if let Some(z) = zoom {
            self.zoom = z.clamp(ZOOM_MIN, ZOOM_MAX);
        }
        self.user_camera_override = false;
        self.last_user_camera_at = None;
    }

    /// 用户拖拽/滚轮：锁镜头并刷新空闲计时。
    pub fn mark_user_camera(&mut self) {
        self.user_camera_override = true;
        self.last_user_camera_at = Some(std::time::Instant::now());
    }

    /// 空闲超过 `idle_secs` 秒则解除用户镜头锁；返回是否刚解除。
    pub fn clear_idle_user_camera(&mut self, idle_secs: f64) -> bool {
        if !self.user_camera_override {
            return false;
        }
        let Some(t) = self.last_user_camera_at else {
            // 无时间戳的旧锁：立即解除，避免永久卡住
            self.user_camera_override = false;
            return true;
        };
        if t.elapsed().as_secs_f64() >= idle_secs {
            self.user_camera_override = false;
            self.last_user_camera_at = None;
            true
        } else {
            false
        }
    }

    /// GQ 风格跳转：`smoothTransition` 抛物线飞到目标；近距则瞬时到位。
    /// 球体远距目标会做太平洋侧构图偏置。
    pub fn fly_to(&mut self, lon: f64, lat: f64, zoom: Option<f64>, duration_s: f64) {
        self.fly_to_inner(lon, lat, zoom, duration_s, true);
    }

    /// 列表/卡片点选：镜头对准震中经纬，不做球体构图偏置。
    pub fn fly_to_centered(&mut self, lon: f64, lat: f64, zoom: Option<f64>, duration_s: f64) {
        self.fly_to_inner(lon, lat, zoom, duration_s, false);
    }

    fn fly_to_inner(
        &mut self,
        lon: f64,
        lat: f64,
        zoom: Option<f64>,
        duration_s: f64,
        frame_globe: bool,
    ) {
        let end_zoom = zoom.unwrap_or(self.zoom).clamp(ZOOM_MIN, ZOOM_MAX);
        let (lon, lat) = if frame_globe && self.projection_mode.is_globe() {
            globe_framed_center(lon, lat, end_zoom)
        } else {
            (lon, lat)
        };
        let dist = haversine_approx_deg(self.center_lon, self.center_lat, lon, lat);
        if dist < 0.05 && (self.zoom - end_zoom).abs() < 0.12 {
            self.center_on(lon, lat, Some(end_zoom));
            return;
        }
        // 墙钟秒（与目标帧率无关）。球体远距略加时，给 GQ 式「整球中转」留出观感。
        let km = dist * 111.0;
        let globe = self.projection_mode.is_globe();
        let dur = if globe {
            (duration_s.max(0.85) + km / 9000.0).clamp(0.85, 3.2)
        } else {
            (duration_s.max(0.55) + km / 12000.0).clamp(0.55, 2.4)
        };
        self.flight = Some(CameraFlight::new(
            (self.center_lon, self.center_lat, self.zoom),
            (lon, lat, end_zoom),
            dur,
            globe,
        ));
        self.post_flight_until = None;
        // 不在此清 user_camera_override：列表点选会在 fly 后立刻锁镜头；
        // 自动跟焦路径本身不会带锁，由调用方决定。
    }

    fn post_flight_active(&self) -> bool {
        self.post_flight_until
            .is_some_and(|t| std::time::Instant::now() < t)
    }

    /// 推进镜头动画；返回是否仍在飞跃中。
    pub fn tick_flight(&mut self, dt: f64) -> bool {
        if let Some(until) = self.post_flight_until {
            if std::time::Instant::now() >= until {
                self.post_flight_until = None;
            }
        }
        let Some(flight) = self.flight.as_mut() else {
            return false;
        };
        let (lon, lat, zoom, done) = flight.tick(dt);
        self.center_lon = lon;
        self.center_lat = lat;
        self.zoom = zoom;
        if done {
            let (elon, elat, ez) = flight.target();
            self.center_lon = elon;
            self.center_lat = elat;
            self.zoom = ez;
            self.flight = None;
            // 落地：矢量/断层标记重建但保留旧 mesh，避免闪没与主线程同步全量
            self.vector.invalidate_paint_cache();
            self.faults.invalidate_paint_cache();
            self.still_static = None;
            // 短冷却，尽快把 S 波跟焦接上
            self.post_flight_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(0.28));
        }
        !done
    }

    pub fn is_flying(&self) -> bool {
        self.flight.is_some()
    }

    /// 飞跃或落地冷却中，需要按目标帧率连续重绘（由应用层调度，勿在此硬编码 16ms）。
    pub fn wants_anim_repaint(&self) -> bool {
        self.flight.is_some() || self.post_flight_active()
    }

    /// 预警期跟 S 波（调用方须先按 eewcn 规则判定已贴边才传入更低 target）。
    /// - 当前过远（target > zoom）：较快拉近
    /// - 当前过近（target < zoom）：按 `zoom_out_per_sec` 缓慢缩小跟随
    /// 镜头始终对准震中经纬（不做球体构图偏置）。
    pub fn apply_linear_zoom_out(
        &mut self,
        target_zoom: f64,
        zoom_out_per_sec: f64,
        dt: f64,
        focus_lon: Option<f64>,
        focus_lat: Option<f64>,
    ) {
        if self.flight.is_some() || self.user_camera_override || self.post_flight_active() {
            return;
        }
        let target = target_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        if (target - self.zoom).abs() < 0.02 && focus_lon.is_none() {
            return;
        }
        let z0 = self.zoom;
        let lon0 = self.center_lon;
        let lat0 = self.center_lat;
        let dt = dt.clamp(1e-4, 0.25);
        if target < self.zoom - 0.02 {
            let step = zoom_out_per_sec.clamp(0.05, 2.0) * dt;
            self.zoom = (self.zoom - step).max(target).clamp(ZOOM_MIN, ZOOM_MAX);
        } else if target > self.zoom + 0.02 {
            // 视角过远：拉近到「S 贴边」所需 zoom（略快于缩小，尽快回到图四构图）
            let step = (zoom_out_per_sec * 2.4).clamp(0.15, 3.0) * dt;
            self.zoom = (self.zoom + step).min(target).clamp(ZOOM_MIN, ZOOM_MAX);
        }
        if let (Some(flon), Some(flat)) = (focus_lon, focus_lat) {
            let tlon = projection::normalize_lon(flon);
            let tlat = self.clamp_view_lat(flat);
            let dlon = projection::normalize_lon(tlon - self.center_lon);
            let dlon = if dlon > 180.0 {
                dlon - 360.0
            } else if dlon < -180.0 {
                dlon + 360.0
            } else {
                dlon
            };
            let dlat = tlat - self.center_lat;
            if dlon.abs() > 0.05 || dlat.abs() > 0.05 {
                // 中心跟焦略快于缩放，避免波圈偏到一侧
                let kb = (1.0 - (1.0 - 0.18_f64).powf(dt * 30.0)).clamp(0.0, 1.0);
                self.center_lon = projection::normalize_lon(self.center_lon + dlon * kb);
                self.center_lat = self.clamp_view_lat(self.center_lat + dlat * kb);
            }
        }
        if (self.zoom - z0).abs() > 1e-6
            || (self.center_lon - lon0).abs() > 1e-6
            || (self.center_lat - lat0).abs() > 1e-6
        {
            self.follow_cam_anim = true;
        }
    }

    pub fn load_state(&self) -> BasemapLoadState {
        self.vector.load_state()
    }

    pub fn load_status(&self) -> &str {
        self.vector.status_text()
    }

    /// 底图仍在后台加载（空闲降频时应按软忙，允许较快收包刷新）。
    pub fn is_basemap_loading(&self) -> bool {
        matches!(
            self.vector.load_state(),
            BasemapLoadState::LoadingWorld | BasemapLoadState::LoadingRegional
        )
    }

    /// 与界面主题同步的海洋/缺瓦片底色。
    pub fn set_ocean_fill(&mut self, color: Color32) {
        self.ocean_fill = color;
    }

    /// 与界面主题同步的矢量陆地/边界色。
    pub fn set_vector_style(&mut self, style: VectorStyle) {
        self.vector_style = style;
    }

    /// 与性能档位同步的绘制抽稀（节能更狠）。
    pub fn set_paint_quality(&mut self, q: PaintQuality) {
        self.paint_quality = q;
    }

    pub fn set_show_terminator(&mut self, on: bool) {
        self.show_terminator = on;
    }

    /// 绘制地图交互；若单击（非拖拽）则返回点击处经纬度。
    /// `wave_events`：仍在传播的多震；`eew_markers`：预警叉标；`eew_marker_nums`：与左上框一致的 ①②…。
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        active: Option<&EewReport>,
        wave_events: &[EewReport],
        eew_markers: &[EewReport],
        eew_marker_nums: &[Option<usize>],
        travel: Option<&TravelEngine>,
        now_ms: i64,
        show_waves: bool,
        stations: &[StationSample],
        records: &[EqRecord],
        volcanoes: &[VolcanoNotice],
        selected_volcano: Option<usize>,
        weather: &[WeatherAlert],
        selected_weather: Option<usize>,
        local_hypo: Option<&LocalHypoEstimate>,
        cenc_int: Option<&CencIntEvent>,
        typhoons: &[TyphoonInfo],
        typhoon_forecast_agency: &str,
        home: Option<(f64, f64)>,
        tsunami: Option<&TsunamiInfo>,
        radars: &[&RadarFrame],
        precip: Option<&PrecipFrame>,
        show_faults: bool,
        radar_opacity: f32,
        precip_opacity: f32,
    ) -> MapUiResult {
        if self.vector.poll() {
            request_repaint_after_capped(ui.ctx(), 50);
        }
        if self.faults.poll(ui.ctx()) {
            // 后台断层刷完必须卸 still_static，否则会一直贴飞跃前的旧 mesh
            self.still_static = None;
            request_repaint_after_capped(ui.ctx(), 33);
        }
        self.markers.ensure(ui.ctx());

        // 飞跃进度走墙钟；跟焦缩放用 unstable_dt（勿用 heavily-smoothed stable_dt）
        let dt = ui.input(|i| i.unstable_dt as f64).clamp(1e-4, 0.25);
        let _ = self.tick_flight(dt);

        let (resp, _) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.last_viewport_min_px = rect.width().min(rect.height()).max(120.0);

        // 先应用拖/滚轮，再取 ViewCtx 绘制：底图 mesh 平移与测站投影同一帧对齐
        let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
        let dragging = resp.dragged() || (resp.hovered() && scroll_y.abs() > 0.0);
        let flying = self.flight.is_some();
        let globe = self.projection_mode.is_globe();
        {
            let view_in = ViewCtx::new(
                self.projection_mode,
                self.center_lon,
                self.center_lat,
                self.zoom,
                rect,
            );
            if resp.dragged() {
                self.flight = None;
                self.post_flight_until = None;
                self.mark_user_camera();
                let d = resp.drag_delta();
                let (lon, lat) = view_in.pan_by_delta(d.x, d.y);
                self.center_lon = lon;
                self.center_lat = self.clamp_view_lat(lat);
            }
            if resp.hovered() && scroll_y.abs() > 0.0 {
                self.flight = None;
                self.post_flight_until = None;
                self.mark_user_camera();
                let before = self.zoom;
                let after = (before + scroll_y as f64 * 0.002).clamp(ZOOM_MIN, ZOOM_MAX);
                if (after - before).abs() > 1e-9 {
                    if let Some(pos) = resp.hover_pos() {
                        if globe {
                            self.zoom = after;
                        } else {
                            let (lon, lat) = view_in.screen_to_lonlat(pos);
                            self.zoom = after;
                            let dx = (pos.x - rect.center().x) as f64;
                            let dy = (pos.y - rect.center().y) as f64;
                            let (wx, wy) = projection::lonlat_to_world(lon, lat, after);
                            let (nlon, nlat) =
                                projection::world_to_lonlat(wx - dx, wy - dy, after);
                            self.center_lon = projection::normalize_lon(nlon);
                            self.center_lat = self.clamp_view_lat(nlat);
                        }
                    } else {
                        self.zoom = after;
                    }
                }
            }
        }

        let view = ViewCtx::new(
            self.projection_mode,
            self.center_lon,
            self.center_lat,
            self.zoom,
            rect,
        );
        // 手动拖/滚轮；S 波跟焦算软交互（断层走变换/量化，勿每帧全量）
        let soft_anim = self.follow_cam_anim;
        self.follow_cam_anim = false;
        let interacting = dragging || flying;
        let fault_interact = interacting || soft_anim;
        let want_vector = globe || self.basemap.show_vector();
        let still_key = still_static_key(
            &view,
            self.paint_quality,
            self.ocean_fill,
            self.vector_style,
            want_vector,
            show_faults,
        );
        let still_base = still_static_base_key(
            &view,
            self.paint_quality,
            self.ocean_fill,
            self.vector_style,
            want_vector,
            show_faults,
        );
        // 平面：拖/滚轮/跟焦均可变换已缓存层（飞跃仍走完整路径）；漂移过大再重建
        let still_can_xform = !flying
            && !globe
            && self.still_static.as_ref().is_some_and(|s| {
                s.base_key == still_base
                    && !s.layers.is_empty()
                    && (s.key == still_key
                        || still_view_drift_ok(
                            s.anchor_lon,
                            s.anchor_lat,
                            s.anchor_zoom,
                            &view,
                        ))
            });
        let still_transformed = still_can_xform
            && self
                .still_static
                .as_mut()
                .is_some_and(|s| apply_still_static_view_delta(s, &view, still_key));
        let still_hit = still_transformed
            || (!interacting
                && self
                    .still_static
                    .as_ref()
                    .is_some_and(|s| s.key == still_key && !s.layers.is_empty()));

        ui.painter().rect_filled(rect, 0.0, self.ocean_fill);

        if globe {
            if let Some(r) = view.horizon_radius() {
                // 球体外遮罩：地平线外用略深色，勾出球轮廓
                ui.painter().circle_filled(rect.center(), r, self.ocean_fill);
                ui.painter().circle_stroke(
                    rect.center(),
                    r,
                    Stroke::new(1.2_f32, Color32::from_rgb(0x55, 0x58, 0x60)),
                );
            }
        }

        // 平面瓦片（tiles/both）
        if !globe && self.basemap.show_tiles() {
            self.cache
                .paint(ui, rect, self.center_lon, self.center_lat, self.zoom);
        }

        if still_hit {
            // 仅贴矢量静止层（可缩放平移）；断层不进 still，避免省界 LOD 切换时连带闪没
            let painter = ui.painter_at(rect);
            if let Some(s) = self.still_static.as_ref() {
                for mesh in &s.layers {
                    painter.add(egui::Shape::Mesh(mesh.clone()));
                }
            }
        } else {
            // 暗色矢量 / 球体：只描轮廓，不填陆地（对齐旧版纯黑轮廓图）
            if want_vector {
                self.vector.paint(
                    ui,
                    &view,
                    /* fill_land */ false,
                    self.vector_style,
                    /* skip_world_stroke */ false,
                    self.paint_quality,
                    /* lite */ flying,
                );
            }

            if flying {
                // 飞跃跨度大，丢弃静止层；普通拖/缩保留以便变换复用
                self.still_static = None;
            } else if !still_hit {
                let mut layers = Vec::with_capacity(1);
                if want_vector {
                    if let Some(m) = self.vector.still_mesh() {
                        layers.push(m);
                    }
                }
                self.still_static = if layers.is_empty() {
                    None
                } else {
                    Some(StillStatic {
                        key: still_key,
                        base_key: still_base,
                        anchor_lon: view.center_lon,
                        anchor_lat: view.center_lat,
                        anchor_zoom: view.zoom,
                        mesh_lon: view.center_lon,
                        mesh_lat: view.center_lat,
                        mesh_zoom: view.zoom,
                        layers,
                    })
                };
            }
        }

        // 断层始终单独绘制：不与省界/矢量 still 绑死；跟焦用 soft_anim 降重建频率
        if show_faults {
            let dark = self.ocean_fill.r() < 80;
            self.faults
                .paint(ui, &view, dark, self.paint_quality, fault_interact);
        }

        // JMA 海啸：按沿岸预报区描边闪烁（不对中日外轮廓整圈描边）
        if let Some(t) = tsunami {
            if !t.cancelled && !t.grade.eq_ignore_ascii_case("Cancel") {
                self.tsunami_coast.paint(ui, &view, &t.areas_list, now_ms);
            }
        }

        if self.show_terminator {
            weather_paint::paint_terminator(ui, &view, now_ms);
        }

        // 雷达 / 降水：底图之上、测站与事件之下
        self.weather_overlay
            .paint_radars(ui, &view, radars, radar_opacity);
        WeatherOverlay::paint_precip(ui, &view, precip, precip_opacity);

        let mut typhoon_hover = None;
        {
            let icons = Some(&self.markers);
            let selected_station = active.and_then(|e| e.event_id.strip_prefix("station:"));
            let selected_record = active.and_then(|e| {
                e.event_id
                    .strip_prefix("record:")
                    .map(|_| e.place.as_str())
            });
            let eew_active = active.is_some_and(|e| {
                !e.event_id.starts_with("station:")
                    && !e.event_id.starts_with("record:")
                    && !e.is_cancel
            });
            // 测站：仅飞跃降质抽稀；手动拖图保持满档，避免 NIED 点闪烁
            let flying = self.flight.is_some();
            let station_q = if flying {
                self.paint_quality.while_interacting()
            } else {
                self.paint_quality
            };
            overlay::paint_stations(
                ui,
                &view,
                stations,
                selected_station,
                icons,
                eew_active,
                station_q,
                &mut self.station_cache,
            );
            overlay::paint_records(ui, &view, records, selected_record, icons);
            // 台风跟焦期间也要画，否则飞跃落点常是「空洋面无标记」
            if !typhoons.is_empty() {
                let hover = if flying { None } else { resp.hover_pos() };
                typhoon_hover = overlay::paint_typhoons(
                    ui,
                    &view,
                    typhoons,
                    typhoon_forecast_agency,
                    hover,
                );
            }
            if !flying {
                overlay::paint_volcanoes(ui, &view, volcanoes, selected_volcano);
                overlay::paint_weather_alerts(ui, &view, weather, selected_weather);
                if let Some(ev) = cenc_int {
                    overlay::paint_cenc_int_event(ui, &view, ev, icons);
                }
            }

            if let Some((hlon, hlat)) = home {
                overlay::paint_home(ui, &view, hlon, hlat, icons);
            }
            if let Some(h) = local_hypo {
                overlay::paint_local_hypo(ui, &view, h);
            }

            let active_key = active.map(|e| (e.agency.0.as_str(), e.event_id.as_str()));
            // 有传播中波圈时叉标 500ms 闪烁（对齐 eewcn eewMarksTimer）
            let blink_active = wave_events.iter().any(|e| {
                !e.event_id.starts_with("station:") && !e.event_id.starts_with("record:")
            });
            let show_cross = !blink_active || overlay::eew_mark_blink_on(now_ms);

            overlay::paint_eew_markers(
                ui,
                &view,
                eew_markers,
                eew_marker_nums,
                active_key,
                icons,
                show_cross,
            );

            if show_waves {
                let mut painted_active = false;
                for ev in wave_events {
                    if ev.event_id.starts_with("station:") || ev.event_id.starts_with("record:") {
                        continue;
                    }
                    let elapsed = event_elapsed_s(ev, now_ms);
                    let selected = active_key
                        == Some((ev.agency.0.as_str(), ev.event_id.as_str()));
                    if selected {
                        painted_active = true;
                    }
                    overlay::paint_wave_overlay(
                        ui,
                        &view,
                        Some(ev),
                        travel,
                        elapsed,
                        icons,
                        selected,
                        show_cross,
                    );
                }
                if !painted_active {
                    if let Some(ev) = active.filter(|e| {
                        !e.event_id.starts_with("station:") && !e.event_id.starts_with("record:")
                    }) {
                        overlay::paint_wave_overlay(
                            ui,
                            &view,
                            Some(ev),
                            travel,
                            event_elapsed_s(ev, now_ms),
                            icons,
                            true,
                            show_cross,
                        );
                    }
                }
            } else if let Some(ev) = active.filter(|e| {
                !e.event_id.starts_with("station:") && !e.event_id.starts_with("record:")
            }) {
                let already = eew_markers.iter().any(|m| {
                    m.agency.0 == ev.agency.0 && m.event_id == ev.event_id
                });
                if !already {
                    overlay::paint_marker_only(ui, &view, ev, icons);
                }
            }
        }

        let mut click = None;
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let (lon, lat) = view.screen_to_lonlat(pos);
                click = Some((
                    projection::normalize_lon(lon),
                    self.clamp_view_lat(lat),
                ));
            }
        }
        MapUiResult {
            click,
            typhoon_hover,
            map_response: Some(resp),
        }
    }
}

fn still_static_base_key(
    view: &ViewCtx,
    quality: PaintQuality,
    ocean: Color32,
    style: VectorStyle,
    want_vector: bool,
    show_faults: bool,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (view.rect.width() as i32).hash(&mut h);
    (view.rect.height() as i32).hash(&mut h);
    (view.rect.min.x as i32).hash(&mut h);
    (view.rect.min.y as i32).hash(&mut h);
    std::mem::discriminant(&view.mode).hash(&mut h);
    quality.hash(&mut h);
    ocean.to_array().hash(&mut h);
    style.land_fill.to_array().hash(&mut h);
    style.world_border.to_array().hash(&mut h);
    style.admin_border.to_array().hash(&mut h);
    want_vector.hash(&mut h);
    show_faults.hash(&mut h);
    // 不把省界阈值写进 base：跟焦连续缩放过 7.5 时勿每帧打爆矢量重建（漂移够大仍会重裁）
    h.finish()
}

fn still_static_key(
    view: &ViewCtx,
    quality: PaintQuality,
    ocean: Color32,
    style: VectorStyle,
    want_vector: bool,
    show_faults: bool,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    still_static_base_key(view, quality, ocean, style, want_vector, show_faults).hash(&mut h);
    view.center_lon.to_bits().hash(&mut h);
    view.center_lat.to_bits().hash(&mut h);
    view.zoom.to_bits().hash(&mut h);
    h.finish()
}

/// 跟焦缩放允许的视角漂移：过大则需重裁剪，否则边缘会空白。
fn still_view_drift_ok(build_lon: f64, build_lat: f64, build_zoom: f64, view: &ViewCtx) -> bool {
    // S 波慢缩常跨 1 级以上；过紧会每帧打爆矢量重绘导致自动缩放卡顿
    if (view.zoom - build_zoom).abs() > 1.15 {
        return false;
    }
    let dlat = (view.center_lat - build_lat).abs();
    let mut dlon = (view.center_lon - build_lon).abs();
    if dlon > 180.0 {
        dlon = 360.0 - dlon;
    }
    dlat <= 3.5 && dlon <= 3.5
}

/// 平面仍图层：按 zoom 差缩放 + 中心平移，供 S 波线性跟焦复用。
fn apply_still_static_view_delta(s: &mut StillStatic, view: &ViewCtx, new_key: u64) -> bool {
    if s.key == new_key {
        return true;
    }
    if view.mode.is_globe() {
        return false;
    }
    let dz = view.zoom - s.mesh_zoom;
    let scale = if dz.abs() > 1e-9 {
        let sc = 2f32.powf(dz as f32);
        if !sc.is_finite() || sc < 1e-4 || sc > 1e4 {
            return false;
        }
        sc
    } else {
        1.0
    };
    let (dx, dy) = flat_still_pan_delta(s.mesh_lon, s.mesh_lat, view).unwrap_or((0.0, 0.0));
    let need_s = (scale - 1.0).abs() > 1e-6;
    let need_t = dx.abs() > 0.05 || dy.abs() > 0.05;
    if need_s || need_t {
        let origin = view.rect.center();
        for layer in &mut s.layers {
            let mesh = Arc::make_mut(layer);
            if need_s && need_t {
                let t = egui::vec2(dx, dy);
                for v in &mut mesh.vertices {
                    v.pos = origin + (v.pos - origin) * scale + t;
                }
            } else if need_s {
                for v in &mut mesh.vertices {
                    v.pos = origin + (v.pos - origin) * scale;
                }
            } else {
                mesh.translate(egui::vec2(dx, dy));
            }
        }
    }
    s.mesh_zoom = view.zoom;
    s.mesh_lon = view.center_lon;
    s.mesh_lat = view.center_lat;
    s.key = new_key;
    true
}

fn flat_still_pan_delta(build_lon: f64, build_lat: f64, view: &ViewCtx) -> Option<(f32, f32)> {
    let (x0, y0) = projection::lonlat_to_world(build_lon, build_lat, view.zoom);
    let (x1, y1) = projection::lonlat_to_world(view.center_lon, view.center_lat, view.zoom);
    let mut dx_w = x0 - x1;
    let world = projection::world_size(view.zoom);
    if dx_w > world * 0.5 {
        dx_w -= world;
    } else if dx_w < -world * 0.5 {
        dx_w += world;
    }
    let dy_w = y0 - y1;
    if dx_w.abs() > world * 0.45 {
        return None;
    }
    Some((dx_w as f32, dy_w as f32))
}

/// 粗略角距离（度），用于判断是否值得开飞跃动画。
fn haversine_approx_deg(lon0: f64, lat0: f64, lon1: f64, lat1: f64) -> f64 {
    let mut dlon = projection::normalize_lon(lon1 - lon0).abs();
    if dlon > 180.0 {
        dlon = 360.0 - dlon;
    }
    let dlat = (lat1 - lat0).abs();
    (dlon * dlon + dlat * dlat).sqrt()
}

fn event_elapsed_s(ev: &EewReport, now_ms: i64) -> f64 {
    if ev.origin_ms > 0 && now_ms > 0 {
        ((now_ms - ev.origin_ms) as f64 / 1000.0).max(0.0)
    } else {
        0.0
    }
}
