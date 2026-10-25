//! 主界面分区：地图视口 + 浮层信息头 / 图例 / 倒计时 + 右栏列表。

mod brand;
mod i18n;
mod intensity_icons;
mod settings;
mod splash;
mod theme;
mod weather_icons;

pub use brand::{app_icon_data, logo_texture, APP_DISPLAY_NAME};
pub use i18n::{set_lang_from_config, Lang};
pub use splash::{draw_splash, SplashGate};

use chrono::{FixedOffset, Local};
use egui::{Color32, FontFamily, FontId, Frame, Margin, RichText, Sense, Stroke, Ui, Vec2};
use intensity_icons::IntensityIcons;
use starquake_core::palette::color_for;
use starquake_core::{
    agency_bracket, dedupe_same_eew_events, eew_sidebar_entries, eew_sidebar_group_key,
    eew_wave_active_spread, format_hm_minutes, is_wave_candidate, max_s_spread_km, place_with_agency,
    prefer_intensity_scale, same_eew_event, sunrise_sunset_local_hm, AppSnapshot, EewReport,
    IntensityKind, ListSelection, OverlayMode, SidebarTab, UserNotice, WeatherAlert,
};
use weather_icons::{weather_level_color, WeatherIconCache};
use starquake_core::palette::{cn_intensity_rgb, jma_shindo_rgb};
use starquake_map::{
    zoom_to_fit_radius_km, BasemapLoadState, BasemapMode, MapUiResult, MapViewport,
    ProjectionMode, TyphoonHoverHit, VectorStyle, OCEAN,
};
use starquake_travel::{max_s_wave_spread_km, s_wave_still_spreading_to, TravelEngine, Wave};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};
use theme::{ThemePalette, UiTheme};

pub use settings::{
    settings_tab_count, settings_tab_title, MapPickKind, OfficialNoticeView, SettingsAction,
    SettingsSession, SourceHealthView,
};
pub use theme::UiTheme as AppUiTheme;

/// 左上预警条 / 波圈 / 叉标共用：有走时表时按表半径判定，与倒计时卸除条件一致。
fn live_wave_eews_travel<'a>(
    list: &'a [EewReport],
    now_ms: i64,
    spread: &[(f64, f64, f64)],
    travel: Option<&TravelEngine>,
) -> Vec<&'a EewReport> {
    list.iter()
        .filter(|e| {
            if let Some(eng) = travel {
                if !is_wave_candidate(e) {
                    return false;
                }
                if e.origin_ms <= 0 {
                    return true;
                }
                let age_ms = now_ms - e.origin_ms;
                if age_ms < -60_000 {
                    return false;
                }
                let elapsed = (age_ms as f64 / 1000.0).max(0.0);
                let max_km = max_s_spread_km(e.magnitude, spread);
                s_wave_still_spreading_to(eng, e.depth_km, elapsed, max_km)
            } else {
                eew_wave_active_spread(e, now_ms, spread)
            }
        })
        .collect()
}

fn overlay_child<R>(
    ui: &mut Ui,
    id_salt: &str,
    rect: egui::Rect,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    // 与 CentralPanel 同层绘制，避免 Foreground Area 的 Sense::hover 挡住 Background 命中
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min))
            .sense(Sense::hover()),
    );
    add(&mut child)
}

pub struct MainShell {
    pub map: MapViewport,
    /// 地图波圈用经过时间（秒）
    pub map_elapsed: f64,
    /// Home / 复位用默认视口
    pub home_lon: f64,
    pub home_lat: f64,
    pub home_zoom: f64,
    /// 本机经纬（图例 Home 与地图标记）
    pub local_lon: Option<f64>,
    pub local_lat: Option<f64>,
    pub show_nied_clock: bool,
    pub intensity_scale: String,
    pub settings: SettingsSession,
    intensity_icons: IntensityIcons,
    weather_icons: WeatherIconCache,
    /// 待确认的服务器提示
    notices: Vec<UserNotice>,
    /// 同类提示冷却，避免重连刷屏
    notice_seen: HashMap<String, Instant>,
    /// 分离侧栏几何节流落盘
    sidebar_geom_dirty_at: Option<Instant>,
    /// 刚分离后短暂禁止自动嵌回，避免一拖出就吸回去
    sidebar_detach_guard_until: Option<Instant>,
    /// 浮窗几何是否已下发过一次（之后勿每帧 with_position/size，否则高频抖动）
    sidebar_float_geom_applied: bool,
}

impl Default for MainShell {
    fn default() -> Self {
        Self {
            map: MapViewport::default(),
            map_elapsed: 0.0,
            home_lon: 135.0,
            home_lat: 35.0,
            home_zoom: 5.0,
            local_lon: None,
            local_lat: None,
            show_nied_clock: false,
            intensity_scale: "auto".into(),
            settings: SettingsSession::default(),
            intensity_icons: IntensityIcons::default(),
            weather_icons: WeatherIconCache::default(),
            notices: Vec::new(),
            notice_seen: HashMap::new(),
            sidebar_geom_dirty_at: None,
            sidebar_detach_guard_until: None,
            sidebar_float_geom_applied: false,
        }
    }
}

impl MainShell {
    pub fn configure_map(
        &mut self,
        basemap: BasemapMode,
        tile_base: impl Into<String>,
        tile_source: impl Into<String>,
        geodata_dir: Option<&Path>,
        lon: f64,
        lat: f64,
        zoom: f64,
        projection: ProjectionMode,
        show_faults: bool,
    ) {
        self.map.set_projection_mode(projection);
        self.map
            .configure(basemap, tile_base, tile_source, geodata_dir, show_faults);
        self.map.center_lon = lon;
        self.map.center_lat = lat;
        self.map.zoom = zoom;
        self.home_lon = lon;
        self.home_lat = lat;
        self.home_zoom = zoom;
    }

    /// 仅换底图/瓦片/投影，不改当前视口中心。
    pub fn reload_basemap(
        &mut self,
        basemap: BasemapMode,
        tile_base: impl Into<String>,
        tile_source: impl Into<String>,
        geodata_dir: Option<&Path>,
        projection: ProjectionMode,
        show_faults: bool,
    ) {
        let lon = self.map.center_lon;
        let lat = self.map.center_lat;
        let zoom = self.map.zoom;
        self.map.set_projection_mode(projection);
        self.map
            .configure(basemap, tile_base, tile_source, geodata_dir, show_faults);
        self.map.center_lon = lon;
        self.map.center_lat = lat;
        self.map.zoom = zoom;
    }

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        snap: &mut AppSnapshot,
        travel: Option<&TravelEngine>,
        cfg: &mut starquake_config::AppConfig,
    ) -> (Option<SettingsAction>, bool) {
        crate::i18n::set_lang_from_config(&cfg.ui.language);
        self.absorb_notices(snap);
        let ui_theme = UiTheme::from_config(&cfg.ui.theme);
        ui_theme.apply_egui(ctx);
        let p = ui_theme.palette();
        // 暗色：要石海洋 #202124；亮色：主题 map_bg（避免纯黑闪底）
        self.map.set_ocean_fill(if ui_theme.is_light() { p.map_bg } else { OCEAN });
        self.map
            .set_vector_style(VectorStyle::for_theme(&cfg.ui.theme));
        self.map
            .set_paint_quality(starquake_map::PaintQuality::from_profile(&cfg.performance.profile));

        snap.prune_finished_eews();
        self.intensity_icons.ensure(ctx);
        self.map_elapsed += ctx.input(|i| i.stable_dt) as f64;
        let now_ms = chrono::Utc::now().timestamp_millis();
        snap.show_wave_rings = cfg.ui.show_wave_rings;
        // 多震：只要仍有未取消的传播中事件就画波圈（不因当前选中项取消而全灭）
        let show_waves = snap.overlay_mode == OverlayMode::Wave && snap.show_wave_rings;
        let scale = self.intensity_scale.clone();

        let live_refs =
            live_wave_eews_travel(&snap.eew_list, now_ms, &snap.wave_spread_km, travel);
        let mut wave_owned: Vec<_> = live_refs.into_iter().cloned().collect();
        // 无有效发震时刻时用 map_elapsed 驱动波圈（上游 origin 缺测）
        let sample_elapsed = self.map_elapsed % 120.0;
        let mut map_ui = MapUiResult::default();
        let mut persist_cfg = false;

        // 波圈动画按档位降频，避免 uncapped request_repaint 拉高 CPU
        if show_waves && !wave_owned.is_empty() {
            let ms = cfg.performance.wave_repaint_ms();
            ctx.request_repaint_after(std::time::Duration::from_millis(ms));
        }

        // 侧栏：可拖宽关闭 / 竖屏底栏 / 可分离为独立 OS 窗口（对齐 eewcn QDock）
        let screen_w = ctx.screen_rect().width();
        let screen_h = ctx.screen_rect().height();
        const MAP_MIN_W: f32 = 360.0;
        const MAP_MIN_H: f32 = 240.0;
        /// 对齐 eewcn `resizeDocks(..., 260)`；DPI 由 egui pixels_per_point 处理
        const SIDE_IDEAL: f32 = 260.0;
        const SIDE_MIN: f32 = 176.0;
        /// 允许拖到比内容更窄，便于继续向右/向下拖到关闭
        const SIDE_DRAG_MIN: f32 = 40.0;
        /// 目标宽度（或指针意图宽度）低于此则关闭
        const SIDE_CLOSE_W: f32 = 110.0;
        const SIDE_CLOSE_H: f32 = 110.0;
        const SIDE_GRIP: f32 = 12.0;
        let sidebar_w = if screen_w < MAP_MIN_W + SIDE_MIN {
            (screen_w * 0.36).clamp(140.0, SIDE_MIN)
        } else {
            let cap = (screen_w - MAP_MIN_W).min(SIDE_IDEAL);
            cap.min(screen_w * 0.27).max(SIDE_MIN)
        };
        let sidebar_h = if screen_h < MAP_MIN_H + SIDE_MIN {
            (screen_h * 0.36).clamp(140.0, SIDE_MIN)
        } else {
            let cap = (screen_h - MAP_MIN_H).min(SIDE_IDEAL + 40.0);
            cap.min(screen_h * 0.34).max(SIDE_MIN)
        };
        let mut side_action = None;
        let show_sidebar = cfg.ui.show_sidebar;
        let sidebar_detached = cfg.ui.sidebar_detached;
        let portrait = screen_h > screen_w;
        let sidebar_id = egui::Id::new("eew_sidebar");
        let source_enabled = [
            cfg.sources.jian.enabled,
            cfg.sources.wolfx.enabled,
            cfg.sources.p2pquake.enabled,
            cfg.sources.pancakes.enabled,
            cfg.sources.fssn.enabled,
        ];
        let show_volcanoes = cfg.ui.show_volcanoes;

        if show_sidebar && sidebar_detached {
            let vp_id = egui::ViewportId::from_hash_of("starquake_sidebar");
            let sw = &cfg.ui.sidebar_window;
            let main_outer = ctx.input(|i| i.viewport().outer_rect);
            // 只设标题/最小尺寸；位置与客户区大小仅首次（或重新分离时）下发，
            // 否则每帧 with_position/with_inner_size 会与系统拖拽打架导致高频抖动。
            let mut builder = egui::ViewportBuilder::default()
                .with_title(crate::i18n::t("地震情报实况"))
                .with_min_inner_size([SIDE_MIN, 240.0])
                .with_close_button(true);
            if !self.sidebar_float_geom_applied {
                builder = builder.with_inner_size([
                    sw.width.clamp(SIDE_MIN, 720.0),
                    sw.height.clamp(240.0, 2400.0),
                ]);
                if let (Some(x), Some(y)) = (sw.x, sw.y) {
                    builder = builder.with_position([x, y]);
                }
            }
            let mut detached_geom: Option<(f32, f32, Option<f32>, Option<f32>)> = None;
            let mut close_requested = false;
            let mut float_pointer_down = false;
            ctx.show_viewport_immediate(vp_id, builder, |vp_ctx, _class| {
                egui::CentralPanel::default()
                    .frame(Frame::NONE.fill(p.sidebar_bg))
                    .show(vp_ctx, |ui| {
                        side_action = draw_sidebar(
                            ui,
                            &p,
                            snap,
                            scale.as_str(),
                            &self.intensity_icons,
                            &mut self.weather_icons,
                            show_volcanoes,
                            source_enabled,
                            true,
                        );
                    });
                let (w, h, x, y) = vp_ctx.input(|i| {
                    float_pointer_down = i.pointer.any_down();
                    let v = i.viewport();
                    let inner = v.inner_rect.unwrap_or(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 640.0),
                    ));
                    let (x, y) = v
                        .outer_rect
                        .map(|r| (Some(r.min.x.round()), Some(r.min.y.round())))
                        .unwrap_or((None, None));
                    (inner.width().round(), inner.height().round(), x, y)
                });
                if w >= 160.0 && h >= 200.0 {
                    detached_geom = Some((w, h, x, y));
                }
                if vp_ctx.input(|i| i.viewport().close_requested()) {
                    close_requested = true;
                }
            });
            if detached_geom.is_some() {
                self.sidebar_float_geom_applied = true;
            }
            if let Some((w, h, x, y)) = detached_geom {
                let sw = &mut cfg.ui.sidebar_window;
                // 阈值加大，避免亚像素/圆整来回写配置
                let changed = (sw.width - w).abs() > 2.0
                    || (sw.height - h).abs() > 2.0
                    || sw.x.map(|ox| x.map(|nx| (ox - nx).abs() > 2.0).unwrap_or(true)).unwrap_or(true)
                    || sw.y.map(|oy| y.map(|ny| (oy - ny).abs() > 2.0).unwrap_or(true)).unwrap_or(true);
                if changed {
                    sw.width = w;
                    sw.height = h;
                    sw.x = x;
                    sw.y = y;
                    self.sidebar_geom_dirty_at = Some(Instant::now());
                }
                // 近似 QDock：浮窗左缘贴近主窗右缘、松手后嵌回（刚分离有短保护）
                let guard_ok = self
                    .sidebar_detach_guard_until
                    .map(|t| Instant::now() >= t)
                    .unwrap_or(true);
                if side_action.is_none()
                    && guard_ok
                    && !float_pointer_down
                    && !ctx.input(|i| i.pointer.any_down())
                {
                    if let (Some(main), Some(fx), Some(fy)) = (main_outer, x, y) {
                        let float_bottom = fy + h;
                        let vert_ok = fy < main.bottom() && float_bottom > main.top();
                        let near_dock =
                            fx >= main.right() - 160.0 && fx <= main.right() + 48.0;
                        if vert_ok && near_dock {
                            side_action = Some(SideAction::AttachSidebar);
                        }
                    }
                }
            }
            if let Some(at) = self.sidebar_geom_dirty_at {
                if at.elapsed() >= Duration::from_millis(600) {
                    self.sidebar_geom_dirty_at = None;
                    persist_cfg = true;
                }
                // 勿在拖拽落盘节流期强制高频 repaint，否则浮窗更易抖
            }
            if close_requested && side_action.is_none() {
                // 关浮窗：嵌回主窗口侧栏（比直接关掉更符合拖出再拖回）
                side_action = Some(SideAction::AttachSidebar);
            }
        } else if show_sidebar && portrait {
            let panel_resp = egui::TopBottomPanel::bottom(sidebar_id)
                .default_height(sidebar_h)
                .min_height(SIDE_DRAG_MIN)
                .max_height((screen_h - MAP_MIN_H).clamp(SIDE_MIN, SIDE_IDEAL + 120.0))
                .resizable(true)
                .frame(Frame::NONE.fill(p.sidebar_bg))
                .show(ctx, |ui| {
                    side_action = draw_sidebar(
                        ui,
                        &p,
                        snap,
                        scale.as_str(),
                        &self.intensity_icons,
                        &mut self.weather_icons,
                        show_volcanoes,
                        source_enabled,
                        false,
                    );
                });
            let cur_h = panel_resp.response.rect.height();
            let resize_id = sidebar_id.with("__resize");
            let mut want_close = false;
            if let Some(resize_resp) = ctx.read_response(resize_id) {
                if resize_resp.dragged() || resize_resp.drag_stopped() {
                    if let Some(pointer) = resize_resp.interact_pointer_pos() {
                        let intent_h = (ctx.screen_rect().bottom() - pointer.y).max(0.0);
                        if intent_h <= SIDE_CLOSE_H {
                            want_close = true;
                        }
                    }
                    if resize_resp.drag_stopped() && cur_h <= SIDE_CLOSE_H {
                        want_close = true;
                    }
                }
            }
            if want_close && !matches!(side_action, Some(SideAction::CloseSidebar)) {
                side_action = Some(SideAction::CloseSidebar);
            }
        } else if show_sidebar {
            let panel_resp = egui::SidePanel::right(sidebar_id)
                .default_width(sidebar_w)
                .min_width(SIDE_DRAG_MIN)
                .max_width((screen_w - MAP_MIN_W).clamp(SIDE_MIN, SIDE_IDEAL + 72.0))
                .resizable(true)
                .frame(Frame::NONE.fill(p.sidebar_bg))
                .show(ctx, |ui| {
                    side_action = draw_sidebar(
                        ui,
                        &p,
                        snap,
                        scale.as_str(),
                        &self.intensity_icons,
                        &mut self.weather_icons,
                        show_volcanoes,
                        source_enabled,
                        false,
                    );
                });
            // 向右拖窄：拖动中指针意图宽度过低，或松手时实际宽度过低 → 关闭
            let cur_w = panel_resp.response.rect.width();
            let resize_id = sidebar_id.with("__resize");
            let mut want_close = false;
            if let Some(resize_resp) = ctx.read_response(resize_id) {
                if resize_resp.dragged() || resize_resp.drag_stopped() {
                    if let Some(pointer) = resize_resp.interact_pointer_pos() {
                        let intent_w = (ctx.screen_rect().right() - pointer.x).max(0.0);
                        if intent_w <= SIDE_CLOSE_W {
                            want_close = true;
                        }
                    }
                    if resize_resp.drag_stopped() && cur_w <= SIDE_CLOSE_W {
                        want_close = true;
                    }
                }
            }
            if want_close && !matches!(side_action, Some(SideAction::CloseSidebar)) {
                side_action = Some(SideAction::CloseSidebar);
            }
        } else {
            egui::SidePanel::right("eew_sidebar_grip")
                .exact_width(SIDE_GRIP)
                .resizable(false)
                .frame(Frame::NONE.fill(p.sidebar_bg))
                .show(ctx, |ui| {
                    let (rect, resp) =
                        ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                    let painter = ui.painter();
                    let c = rect.center();
                    for dy in [-6.0_f32, 0.0, 6.0] {
                        painter.circle_filled(
                            egui::pos2(c.x, c.y + dy),
                            1.6,
                            p.text_dim,
                        );
                    }
                    let open = resp.clicked()
                        || (resp.dragged() && resp.drag_delta().x < -1.5);
                    if open {
                        side_action = Some(SideAction::OpenSidebar);
                    }
                    resp.on_hover_text(crate::i18n::t("打开侧栏"));
                });
        }

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.map_bg))
            .show(ctx, |ui| {
                let map_rect = ui.available_rect_before_wrap();
                let home = match (self.local_lon, self.local_lat) {
                    (Some(lon), Some(lat)) => Some((lon, lat)),
                    _ => None,
                };
                for ev in &mut wave_owned {
                    if ev.origin_ms <= 0 {
                        ev.origin_ms = now_ms - (sample_elapsed * 1000.0) as i64;
                    }
                }
                let selected_volcano = snap.selection.and_then(|s| {
                    if s.tab == SidebarTab::Volcano {
                        Some(s.index)
                    } else {
                        None
                    }
                });
                let selected_weather = snap.selection.and_then(|s| {
                    if s.tab == SidebarTab::Weather {
                        Some(s.index)
                    } else {
                        None
                    }
                });
                let volcanoes_for_map = if cfg.ui.show_volcanoes {
                    snap.volcanoes.as_slice()
                } else {
                    &[]
                };
                let weather_for_map = if cfg.ui.show_weather {
                    snap.weather.as_slice()
                } else {
                    &[]
                };
                let show_typhoons_map =
                    cfg.ui.show_typhoons && cfg.sources.typhoon.enabled;
                let radars: Vec<_> = snap.active_radars().collect();
                self.map.set_show_terminator(cfg.ui.show_terminator);
                // 地图 ①②③ 与左上预警条同源槽编号对齐（勿按 eew_list 裸序）
                let header_cards = header_live_eews(snap, travel);
                let marker_nums: Vec<Option<usize>> = wave_owned
                    .iter()
                    .map(|ev| eew_marker_display_num(ev, snap, travel, &header_cards))
                    .collect();
                map_ui = self.map.ui(
                    ui,
                    snap.active.as_ref(),
                    &wave_owned,
                    &wave_owned,
                    &marker_nums,
                    travel,
                    now_ms,
                    show_waves,
                    &snap.stations,
                    &snap.records,
                    volcanoes_for_map,
                    selected_volcano,
                    weather_for_map,
                    selected_weather,
                    None,
                    if show_typhoons_map {
                        snap.typhoons.as_slice()
                    } else {
                        &[]
                    },
                    &cfg.ui.typhoon_forecast_agency,
                    home,
                    snap.tsunami.as_ref(),
                    &radars,
                    snap.precip.as_ref(),
                    cfg.ui.show_faults,
                    cfg.sources.map_wx.radar_opacity,
                    cfg.sources.map_wx.precip_opacity,
                );
                if cfg.ui.auto_zoom_waves
                    && show_waves
                    && !self.map.is_flying()
                    && !self.map.user_camera_override
                {
                    let focus = snap.active.as_ref().filter(|e| {
                        !e.is_cancel
                            && !e.event_id.starts_with("record:")
                            && !e.event_id.starts_with("station:")
                            && (e.latitude.abs() > 0.01 || e.longitude.abs() > 0.01)
                    });
                    if let Some(ev) = focus {
                        let elapsed = if ev.origin_ms > 0 {
                            ((now_ms - ev.origin_ms) as f64 / 1000.0).max(0.0)
                        } else {
                            sample_elapsed
                        };
                        let s_km = if let Some(eng) = travel {
                            eng.surface_distance_for_elapsed(Wave::S, ev.depth_km, elapsed)
                                .max(0.0)
                        } else {
                            elapsed * 5.0
                        };
                        let r = s_km
                            .min(max_s_wave_spread_km(ev.magnitude))
                            .max(8.0);
                        // 目标：S 半径约占视口短边 46%（直径≈92%，留边不裁切）
                        // 区间 [7, 11]：落地 Z11 起只缩小，地板 z7 禁止砸到半球远景
                        let z = zoom_to_fit_radius_km(
                            ev.latitude,
                            r,
                            map_rect.width().min(map_rect.height()),
                            0.46,
                        )
                        .clamp(7.0, 11.0);
                        let dt = ctx.input(|i| i.unstable_dt as f64).clamp(1e-4, 0.25);
                        // ~0.28 级/秒：Z11→Z7 约 14 秒，缓慢线性
                        self.map.apply_linear_zoom_out(
                            z,
                            0.28,
                            dt,
                            Some(ev.longitude),
                            Some(ev.latitude),
                        );
                        ctx.request_repaint_after(std::time::Duration::from_millis(
                            cfg.performance.wave_repaint_ms(),
                        ));
                    }
                }
                // 预警条/海啸/右上测站与台风：小视口缩小，大屏不放大（RhythmQuake 式）
                // 图例与右下时钟保持设计稿原尺寸
                let panel_z = eew_panel_zoom(map_rect.width(), map_rect.height());
                draw_map_load_banner(ui, &p, self.map.load_state(), self.map.load_status());
                if let Some(kind) = self.settings.map_pick {
                    draw_map_pick_banner(ui, &p, kind);
                }
                if let Some((lon, lat, z)) = draw_event_header(
                    ui,
                    &p,
                    snap,
                    scale.as_str(),
                    &self.intensity_icons,
                    travel,
                    panel_z,
                ) {
                    // 左上框点切换多震：跟焦并解除拖拽锁，便于 cinema 继续轮播
                    self.map.fly_to(lon, lat, Some(z), 0.9);
                    self.map.user_camera_override = false;
                    self.map.last_user_camera_at = None;
                }
                draw_tsunami_banner(ui, &p, snap, travel, panel_z);
                let station_card_bottom = draw_realtime_intensity_card(ui, &p, snap, panel_z);
                let typhoon_fly = draw_typhoon_detail_below_stations(
                    ui,
                    &p,
                    map_ui.typhoon_hover.as_ref(),
                    if show_typhoons_map {
                        snap.typhoons.as_slice()
                    } else {
                        &[]
                    },
                    station_card_bottom.bottom_y,
                    panel_z,
                );
                if let Some(net) = station_card_bottom.clicked {
                    if let Some((lon, lat, zoom)) = snap.focus_max_station(net) {
                        self.map.fly_to_centered(lon, lat, Some(zoom.clamp(7.5, 11.0)), 0.4);
                        self.map.mark_user_camera();
                    }
                }
                if let Some((lon, lat, z)) = typhoon_fly {
                    self.map.user_camera_override = false;
                    self.map.last_user_camera_at = None;
                    self.map.fly_to_centered(lon, lat, Some(z), 1.0);
                }
                if cfg.ui.show_legend {
                    draw_legend_bar(ui, &p, 1.0);
                }
                // 本地未设经纬度时，用当前地图中心估算日出日落
                let (sun_lat, sun_lon) = match (self.local_lat, self.local_lon) {
                    (Some(lat), Some(lon)) => (Some(lat), Some(lon)),
                    _ => (Some(self.map.center_lat), Some(self.map.center_lon)),
                };
                draw_countdown_hud(
                    ui,
                    &p,
                    snap,
                    self.show_nied_clock,
                    map_rect,
                    sun_lat,
                    sun_lon,
                    1.0,
                );

                // 侧栏关闭或已分离：地图右键菜单（内嵌侧栏打开时不挂，避免挡列表）
                let allow_ctx = (!show_sidebar || sidebar_detached)
                    && !self.settings.open
                    && self.settings.map_pick.is_none();
                if allow_ctx {
                    if let Some(resp) = map_ui.map_response.as_ref() {
                        let mut jump_local = false;
                        let mut open_settings = false;
                        let mut open_about = false;
                        let mut local_pick = false;
                        let mut toggle_legend = false;
                        let mut open_sidebar = false;
                        let has_local = self.local_lon.is_some() && self.local_lat.is_some();
                        resp.context_menu(|ui| {
                            if ui.button(crate::i18n::t("设置")).clicked() {
                                open_settings = true;
                                ui.close_menu();
                            }
                            if !show_sidebar {
                                if ui.button(crate::i18n::t("打开侧栏")).clicked() {
                                    open_sidebar = true;
                                    ui.close_menu();
                                }
                            }
                            ui.add_enabled_ui(has_local, |ui| {
                                if ui.button(crate::i18n::t("跳转所在地")).clicked() {
                                    jump_local = true;
                                    ui.close_menu();
                                }
                            });
                            let legend_label = if cfg.ui.show_legend {
                                crate::i18n::t("隐藏图例")
                            } else {
                                crate::i18n::t("显示图例")
                            };
                            if ui.button(legend_label).clicked() {
                                toggle_legend = true;
                                ui.close_menu();
                            }
                            if ui.button(crate::i18n::t("设置所在地")).clicked() {
                                local_pick = true;
                                ui.close_menu();
                            }
                            if ui.button(crate::i18n::t("关于")).clicked() {
                                open_about = true;
                                ui.close_menu();
                            }
                        });
                        if open_settings {
                            self.settings.open_with(cfg);
                        }
                        if open_about {
                            self.settings.open_about(cfg);
                        }
                        if local_pick {
                            self.settings.begin_local_pick(cfg);
                        }
                        if toggle_legend {
                            cfg.ui.show_legend = !cfg.ui.show_legend;
                            persist_cfg = true;
                        }
                        if open_sidebar {
                            side_action = Some(SideAction::OpenSidebar);
                        }
                        if jump_local {
                            if let (Some(lon), Some(lat)) = (self.local_lon, self.local_lat) {
                                let z = self.home_zoom.clamp(7.5, 11.0);
                                self.map.fly_to(lon, lat, Some(z), 0.9);
                                self.map.mark_user_camera();
                            }
                        }
                    }
                }
            });

        if let Some(act) = side_action {
            match act {
                SideAction::List(tab, idx) => {
                    if let Some((lon, lat, zoom)) = snap.select_list_item(tab, idx) {
                        let vp = ctx.screen_rect();
                        let (map_w, map_h) = if sidebar_detached || !show_sidebar {
                            let used = if show_sidebar { 0.0 } else { SIDE_GRIP };
                            ((vp.width() - used).max(120.0), vp.height().max(120.0))
                        } else if portrait {
                            (vp.width().max(120.0), (vp.height() - sidebar_h).max(120.0))
                        } else {
                            (
                                (vp.width() - sidebar_w).max(120.0),
                                vp.height().max(120.0),
                            )
                        };
                        match tab {
                            SidebarTab::Eew | SidebarTab::Records => {
                                let r_km = if tab == SidebarTab::Eew {
                                    snap.eew_list.get(idx).map(|e| {
                                        let max_km =
                                            max_s_spread_km(e.magnitude, &snap.wave_spread_km);
                                        let elapsed = if e.origin_ms > 0 {
                                            ((now_ms - e.origin_ms) as f64 / 1000.0).max(0.0)
                                        } else {
                                            0.0
                                        };
                                        let live = if let Some(eng) = travel {
                                            s_wave_still_spreading_to(
                                                eng, e.depth_km, elapsed, max_km,
                                            )
                                        } else {
                                            eew_wave_active_spread(
                                                e,
                                                now_ms,
                                                &snap.wave_spread_km,
                                            )
                                        };
                                        if !live {
                                            return 90.0;
                                        }
                                        let s_km = if let Some(eng) = travel {
                                            eng.surface_distance_for_elapsed(
                                                Wave::S,
                                                e.depth_km,
                                                elapsed,
                                            )
                                            .max(0.0)
                                        } else {
                                            elapsed * 4.0
                                        };
                                        s_km.min(max_km).max(18.0)
                                    })
                                } else {
                                    Some(55.0)
                                }
                                .unwrap_or(55.0);
                                let end_z = zoom_to_fit_radius_km(
                                    lat,
                                    r_km,
                                    map_w.min(map_h),
                                    0.52,
                                )
                                .clamp(8.0, 12.5);
                                let dur = if tab == SidebarTab::Eew { 0.85 } else { 0.7 };
                                self.map.fly_to_centered(lon, lat, Some(end_z), dur);
                                // 手动点选：锁定镜头，避免影院轮播 / 跟焦抢走速报跳转
                                self.map.mark_user_camera();
                            }
                            SidebarTab::Volcano | SidebarTab::Weather => {
                                self.map.fly_to_centered(lon, lat, Some(zoom.max(9.0)), 0.85);
                                self.map.mark_user_camera();
                            }
                        }
                        if snap.overlay_mode == OverlayMode::Wave {
                            self.map_elapsed = 0.0;
                        }
                    }
                }
                SideAction::Settings => {
                    self.settings.open_with(cfg);
                }
                SideAction::CloseSidebar => {
                    cfg.ui.show_sidebar = false;
                    cfg.ui.sidebar_detached = false;
                    self.sidebar_geom_dirty_at = None;
                    self.sidebar_float_geom_applied = false;
                    self.sidebar_detach_guard_until = None;
                    persist_cfg = true;
                    // 清掉过窄宽度，下次打开用 default_width
                    ctx.data_mut(|d| {
                        d.remove::<egui::containers::panel::PanelState>(sidebar_id);
                    });
                }
                SideAction::OpenSidebar => {
                    cfg.ui.show_sidebar = true;
                    persist_cfg = true;
                    ctx.data_mut(|d| {
                        d.remove::<egui::containers::panel::PanelState>(sidebar_id);
                    });
                }
                SideAction::DetachSidebar => {
                    cfg.ui.show_sidebar = true;
                    cfg.ui.sidebar_detached = true;
                    self.sidebar_geom_dirty_at = None;
                    self.sidebar_float_geom_applied = false;
                    self.sidebar_detach_guard_until =
                        Some(Instant::now() + Duration::from_millis(900));
                    persist_cfg = true;
                    ctx.data_mut(|d| {
                        d.remove::<egui::containers::panel::PanelState>(sidebar_id);
                    });
                }
                SideAction::AttachSidebar => {
                    cfg.ui.show_sidebar = true;
                    cfg.ui.sidebar_detached = false;
                    self.sidebar_geom_dirty_at = None;
                    self.sidebar_float_geom_applied = false;
                    self.sidebar_detach_guard_until = None;
                    persist_cfg = true;
                    ctx.data_mut(|d| {
                        d.remove::<egui::containers::panel::PanelState>(sidebar_id);
                    });
                }
            }
        }

        if let Some((lon, lat)) = map_ui.click {
            self.apply_map_pick(lon, lat);
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.settings.map_pick = None;
        }
        if self.settings.capture_view {
            self.settings.apply_capture_view(
                round_coord(self.map.center_lon),
                round_coord(self.map.center_lat),
                round_zoom(self.map.zoom),
            );
        }

        let health = SourceHealthView {
            jian: snap.health_jian,
            wolfx: snap.health_wolfx,
            p2p: snap.health_p2p,
            pancakes: snap.health_pancakes,
            fssn: snap.health_fssn,
            weather: snap.health_weather,
            cenc_int: snap.health_cenc_int,
            jian_enabled: cfg.sources.jian.enabled,
            wolfx_enabled: cfg.sources.wolfx.enabled,
            p2p_enabled: cfg.sources.p2pquake.enabled,
            pancakes_enabled: cfg.sources.pancakes.enabled,
            fssn_enabled: cfg.sources.fssn.enabled,
            weather_enabled: cfg.sources.weather.enabled,
            cenc_int_enabled: cfg.sources.cenc_int.enabled,
        };
        self.draw_user_notice(ctx);
        let settings_action = self.settings.ui(ctx, cfg, &health);
        (settings_action, persist_cfg)
    }

    fn absorb_notices(&mut self, snap: &mut AppSnapshot) {
        let now = Instant::now();
        self.notice_seen
            .retain(|_, t| now.duration_since(*t) < Duration::from_secs(180));
        for n in snap.take_notices() {
            if self.notices.iter().any(|x| x.dedupe_key == n.dedupe_key) {
                continue;
            }
            if self
                .notice_seen
                .get(&n.dedupe_key)
                .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(90))
            {
                continue;
            }
            self.notices.push(n);
        }
    }

    fn draw_user_notice(&mut self, ctx: &egui::Context) {
        let Some(n) = self.notices.first().cloned() else {
            return;
        };
        let mut close = false;
        egui::Window::new(crate::i18n::t(&n.title))
            .id(egui::Id::new("server_notice"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(280.0);
                ui.label(n.body);
                ui.add_space(8.0);
                if ui.button(crate::i18n::t("知道了")).clicked() {
                    close = true;
                }
            });
        if close {
            if let Some(gone) = self.notices.first() {
                self.notice_seen
                    .insert(gone.dedupe_key.clone(), Instant::now());
            }
            self.notices.remove(0);
        }
    }

    fn apply_map_pick(&mut self, lon: f64, lat: f64) {
        let Some(kind) = self.settings.map_pick else {
            return;
        };
        self.settings.apply_map_pick_coords(
            kind,
            round_coord(lon),
            round_coord(lat),
            round_zoom(self.map.zoom),
        );
    }
}

fn round_coord(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn round_zoom(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn draw_map_pick_banner(ui: &mut Ui, p: &ThemePalette, kind: MapPickKind) {
    let text = match kind {
        MapPickKind::DefaultViewport => crate::i18n::t("点击地图设置默认视口（Esc 取消）"),
        MapPickKind::LocalLocation => crate::i18n::t("点击地图设置本机位置（Esc 取消）"),
    };
    let pos = ui.max_rect().center_top() + Vec2::new(-140.0, 44.0);
    let rect = egui::Rect::from_min_size(pos, Vec2::new(280.0, 40.0));
    overlay_child(ui, "map_pick_banner", rect, |ui| {
        Frame::NONE
            .fill(p.panel_glass_soft)
            .stroke(Stroke::new(1.0_f32, p.accent))
            .inner_margin(Margin::symmetric(12, 6))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.label(RichText::new(text).size(13.0).color(p.text));
            });
    });
}

fn format_header_time(origin_ms: i64) -> String {
    if origin_ms <= 0 {
        return "----/--/-- --:--:--".into();
    }
    let Some(dt) = chrono::DateTime::from_timestamp_millis(origin_ms) else {
        return "----/--/-- --:--:--".into();
    };
    let cst = FixedOffset::east_opt(8 * 3600).unwrap();
    dt.with_timezone(&cst)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn format_list_time(origin_ms: i64) -> String {
    if origin_ms <= 0 {
        return "—".into();
    }
    let Some(dt) = chrono::DateTime::from_timestamp_millis(origin_ms) else {
        return "—".into();
    };
    let cst = FixedOffset::east_opt(8 * 3600).unwrap();
    dt.with_timezone(&cst)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn rgb(c: starquake_core::palette::Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

#[allow(dead_code)]
fn glass_frame(p: &ThemePalette) -> Frame {
    Frame::NONE
        .fill(p.panel_glass)
        .stroke(Stroke::new(1.0_f32, p.panel_border))
        .inner_margin(Margin::symmetric(12, 10))
        .corner_radius(10.0)
}

fn intensity_chip(
    kind: IntensityKind,
    level: u8,
    text: &str,
    scale: &str,
) -> (IntensityKind, u8, Color32, String) {
    let (k, lv, label) = prefer_intensity_scale(kind, level, text, scale);
    (k, lv, rgb(color_for(k, lv)), label)
}

/// 绘制烈度/震度图标；无贴图时回退色块+文案。
fn paint_intensity(
    ui: &mut Ui,
    icons: &IntensityIcons,
    kind: IntensityKind,
    level: u8,
    text: &str,
    scale: &str,
    size: f32,
    instrumental: bool,
) {
    let (k, lv, col, label) = intensity_chip(kind, level, text, scale);
    let (resp, painter) = ui.allocate_painter(Vec2::splat(size), Sense::hover());
    // 中国烈度 0 = 无数据：显示「—」，勿用烈度1图标冒充
    let unknown_cn = k == IntensityKind::CnIntensity && lv == 0;
    if !unknown_cn {
        if let Some(tex) = icons.get_ex(k, lv, instrumental) {
            painter.image(
                tex.id(),
                resp.rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            return;
        }
    }
    let show = if unknown_cn {
        "—".into()
    } else {
        label
    };
    let bg = if unknown_cn {
        Color32::from_rgb(0x3A, 0x3A, 0x3E)
    } else {
        col
    };
    painter.rect_filled(resp.rect, 4.0, bg);
    painter.text(
        resp.rect.center(),
        egui::Align2::CENTER_CENTER,
        &show,
        FontId::proportional(if show.chars().count() > 2 {
            size * 0.28
        } else {
            size * 0.45
        }),
        if unknown_cn {
            Color32::from_rgb(0xB0, 0xB0, 0xB8)
        } else {
            Color32::BLACK
        },
    );
}

fn draw_map_load_banner(ui: &mut Ui, p: &ThemePalette, state: BasemapLoadState, status: &str) {
    let text = match state {
        BasemapLoadState::Empty => return,
        BasemapLoadState::Ready => return,
        BasemapLoadState::LoadingWorld | BasemapLoadState::LoadingRegional => {
            if status.is_empty() {
                crate::i18n::t("正在加载底图…")
            } else {
                status
            }
        }
        BasemapLoadState::Failed => {
            if status.is_empty() {
                crate::i18n::t("底图加载失败")
            } else {
                status
            }
        }
    };
    let color = if state == BasemapLoadState::Failed {
        p.danger
    } else {
        p.text_muted
    };
    let pos = ui.max_rect().center_top() + Vec2::new(-120.0, 10.0);
    let rect = egui::Rect::from_min_size(pos, Vec2::new(260.0, 36.0));
    overlay_child(ui, "map_load_banner", rect, |ui| {
        Frame::NONE
            .fill(p.panel_glass_soft)
            .stroke(Stroke::new(1.0_f32, p.panel_border))
            .inner_margin(Margin::symmetric(12, 6))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.label(RichText::new(text).size(12.0).color(color));
            });
    });
}

/// 左上预警框与右上实时震度卡共用顶边。
const OVERLAY_TOP_Y: f32 = 12.0;

/// 左上堆叠顺序：各源槽彼此独立，固定源序（不因焦点把某源顶到最上）。
fn header_source_rank(agency: &str) -> u8 {
    match eew_sidebar_group_key(agency).as_str() {
        "jma" => 0,
        "cea" => 1,
        "cea-pr" => 2,
        "cwa" => 3,
        "kma" => 4,
        "sa" => 5,
        "early_est" => 6,
        "gq" => 7,
        _ => 20,
    }
}

/// 地图叉标序号：跨源与左上 ①②③ 同源槽同号；单源多震用 peer 序。
fn eew_marker_display_num(
    ev: &EewReport,
    snap: &AppSnapshot,
    travel: Option<&TravelEngine>,
    header_cards: &[&EewReport],
) -> Option<usize> {
    if header_cards.len() > 1 {
        let key = eew_sidebar_group_key(&ev.agency.0);
        return header_cards
            .iter()
            .position(|c| eew_sidebar_group_key(&c.agency.0) == key)
            .map(|i| i + 1);
    }
    if header_cards.len() == 1 {
        let peers = header_slot_peers(snap, travel, &ev.agency.0);
        if peers.len() > 1 {
            return peers
                .iter()
                .position(|p| same_eew_event(p, ev))
                .map(|i| i + 1);
        }
    }
    None
}

/// 左上预警条：每源一框（JMA/CN/CWA/GQ…各自独立）；同槽多震 `n/N` + 点击/cinema 轮切。
fn header_live_eews<'a>(
    snap: &'a AppSnapshot,
    travel: Option<&TravelEngine>,
) -> Vec<&'a EewReport> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let live = live_wave_eews_travel(&snap.eew_list, now_ms, &snap.wave_spread_km, travel);
    let unique = dedupe_same_eew_events(&live);
    let mut best: Vec<&EewReport> = Vec::new();
    for e in unique {
        let key = eew_sidebar_group_key(&e.agency.0);
        if let Some(pos) = best
            .iter()
            .position(|b| eew_sidebar_group_key(&b.agency.0) == key)
        {
            let cur = best[pos];
            let prefer_focus = snap.active.as_ref().is_some_and(|a| same_eew_event(a, e));
            let cur_focus = snap.active.as_ref().is_some_and(|a| same_eew_event(a, cur));
            if prefer_focus
                || (!cur_focus
                    && (e.origin_ms > cur.origin_ms
                        || (e.origin_ms == cur.origin_ms && e.serial >= cur.serial)))
            {
                best[pos] = e;
            }
        } else {
            best.push(e);
        }
    }
    best.sort_by(|a, b| {
        header_source_rank(&a.agency.0)
            .cmp(&header_source_rank(&b.agency.0))
            .then_with(|| a.origin_ms.cmp(&b.origin_ms))
    });
    best
}

/// 同侧栏源槽内、仍在传播的独立地震（发震旧→新，与 cinema 队列一致）。
fn header_slot_peers<'a>(
    snap: &'a AppSnapshot,
    travel: Option<&TravelEngine>,
    agency: &str,
) -> Vec<&'a EewReport> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let live = live_wave_eews_travel(&snap.eew_list, now_ms, &snap.wave_spread_km, travel);
    let key = eew_sidebar_group_key(agency);
    let raw: Vec<&EewReport> = live
        .into_iter()
        .filter(|e| eew_sidebar_group_key(&e.agency.0) == key)
        .collect();
    dedupe_same_eew_events(&raw)
}

/// 地图 HUD 缩放：参考 RhythmQuake `UiScale.factor`——相对设计稿**只缩不放**。
/// （eewcn `getWindowZoom` 在大屏可到 2×，会把测站/台风卡撑爆并挤出视口。）
fn hud_window_zoom(map_w: f32, map_h: f32) -> f32 {
    (((map_w / 640.0) * (map_h / 480.0)).sqrt()).clamp(0.75, 1.0)
}

/// 预警条 / 海啸 / 右上测站与台风：同只缩不放。
fn eew_panel_zoom(map_w: f32, map_h: f32) -> f32 {
    hud_window_zoom(map_w, map_h)
}

/// 侧栏内容缩放：对齐 eewcn——**不按宽度改字号**。
/// eewcn 用 `ListHistoryWidget::ResizeItems` + `ExpandHeight` 按视口宽重排行高；
/// 本侧用固定字号 + 地名换行 / 烈度块先占位实现同等效果。
fn sidebar_content_zoom(_width: f32) -> f32 {
    1.0
}

fn margin_xy(x: f32, y: f32) -> Margin {
    Margin::symmetric(x.round().clamp(0.0, 127.0) as i8, y.round().clamp(0.0, 127.0) as i8)
}

/// 左上角预警条总高度（海啸条避让）。
fn eew_header_stack_height(snap: &AppSnapshot, travel: Option<&TravelEngine>, zoom: f32) -> f32 {
    let n = header_live_eews(snap, travel).len();
    if n == 0 {
        return 0.0;
    }
    // 单条约 96px + 条间距 6（再乘 HUD 缩放）
    (96.0 * n as f32 + 6.0 * (n.saturating_sub(1) as f32)) * zoom
}

/// 绘制左上预警条。同源多震时一框显示 `n/N`，点击切到下一条并返回跟焦目标。
fn draw_event_header(
    ui: &mut Ui,
    _p: &ThemePalette,
    snap: &mut AppSnapshot,
    scale: &str,
    icons: &IntensityIcons,
    travel: Option<&TravelEngine>,
    zoom: f32,
) -> Option<(f64, f64, f64)> {
    // 提前 clone，避免 overlay 闭包与 snap 可变借用冲突
    let cards: Vec<EewReport> = header_live_eews(snap, travel)
        .into_iter()
        .cloned()
        .collect();
    if cards.is_empty() {
        return None;
    }
    let peer_lists: Vec<Vec<EewReport>> = cards
        .iter()
        .map(|ev| {
            header_slot_peers(snap, travel, &ev.agency.0)
                .into_iter()
                .cloned()
                .collect()
        })
        .collect();
    let focused_flags: Vec<bool> = cards
        .iter()
        .map(|ev| snap.active.as_ref().is_some_and(|a| same_eew_event(a, ev)))
        .collect();

    let left_x = 12.0 * zoom;
    let left_w = 340.0 * zoom;
    let row_gap = 6.0 * zoom;
    let top_y = OVERLAY_TOP_Y * zoom;
    let pos = ui.max_rect().left_top() + Vec2::new(left_x, top_y);
    let rect = egui::Rect::from_min_size(
        pos,
        Vec2::new(left_w + 24.0 * zoom, ui.max_rect().height().max(120.0)),
    );
    let mut cycle_to: Option<EewReport> = None;
    overlay_child(ui, "event_header", rect, |ui| {
        ui.spacing_mut().item_spacing.y = row_gap;
        ui.vertical(|ui| {
            for (i, ev) in cards.iter().enumerate() {
                let agency = agency_bracket(&ev.agency.0);
                let peers = &peer_lists[i];
                let peer_i = peers
                    .iter()
                    .position(|p| same_eew_event(p, ev))
                    .unwrap_or(0);
                // 同源多震：GQ 式 `2/3`；跨源堆叠仍用 ①②③
                let idx_mark = if peers.len() > 1 {
                    format!("{}/{} ", peer_i + 1, peers.len())
                } else if cards.len() > 1 {
                    format!("{} ", circled_index(i + 1))
                } else {
                    String::new()
                };
                // 报数挂在机构后：`地名 (JMA)  #终`
                let title = if ev.is_cancel {
                    format!("{idx_mark}{} ({agency}) \u{00b7} 取消", ev.place)
                } else if let Some(mark) = ev.serial_mark() {
                    format!("{idx_mark}{} ({agency})  {mark}", ev.place)
                } else {
                    format!("{idx_mark}{} ({agency})", ev.place)
                };
                let time_line = format_header_time(ev.origin_ms);
                let mag_line = if ev.is_assumption && ev.magnitude <= 0.0 {
                    crate::i18n::t("调查中").to_string()
                } else {
                    format!("M{:.1}", ev.magnitude)
                };
                let depth_line = ev.depth_label();
                let stroke = if focused_flags[i] {
                    Stroke::new(1.5_f32, Color32::from_rgb(0x7D, 0xF9, 0xFF))
                } else {
                    Stroke::new(1.0_f32, Color32::from_rgb(0x55, 0x55, 0x66))
                };
                let can_cycle = peers.len() > 1;
                let inner = Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(28, 28, 32, 230))
                    .stroke(stroke)
                    .inner_margin(margin_xy(10.0 * zoom, 8.0 * zoom))
                    .corner_radius(6.0 * zoom)
                    .show(ui, |ui| {
                        ui.set_width(left_w);
                        ui.horizontal(|ui| {
                            paint_intensity(
                                ui,
                                icons,
                                ev.intensity_kind,
                                ev.intensity_level,
                                &ev.max_intensity_text,
                                scale,
                                72.0 * zoom,
                                false,
                            );
                            ui.add_space(10.0 * zoom);
                            ui.vertical(|ui| {
                                ui.set_min_width(180.0 * zoom);
                                ui.label(
                                    RichText::new(title)
                                        .color(Color32::WHITE)
                                        .strong()
                                        .size(16.0 * zoom),
                                );
                                ui.add_space(2.0 * zoom);
                                ui.label(
                                    RichText::new(time_line)
                                        .color(Color32::WHITE)
                                        .size(14.0 * zoom)
                                        .monospace(),
                                );
                                ui.add_space(2.0 * zoom);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(mag_line)
                                            .color(Color32::WHITE)
                                            .strong()
                                            .size(15.0 * zoom),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new(depth_line)
                                                    .color(Color32::from_rgb(0xDD, 0xDD, 0xEE))
                                                    .size(14.0 * zoom),
                                            );
                                        },
                                    );
                                });
                            });
                        });
                    });
                if can_cycle {
                    let resp = ui.interact(
                        inner.response.rect,
                        ui.id().with(("eew_hdr_cycle", i)),
                        Sense::click(),
                    );
                    if resp.clicked() {
                        cycle_to = Some(peers[(peer_i + 1) % peers.len()].clone());
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            }
        });
    });
    if let Some(next) = cycle_to {
        let fly = (next.longitude, next.latitude, 11.0);
        snap.active = Some(next);
        snap.tab = SidebarTab::Eew;
        snap.overlay_mode = OverlayMode::Wave;
        Some(fly)
    } else {
        None
    }
}

/// ①②③… 序号（多震堆叠，对齐 EEWCN）。
fn circled_index(n: usize) -> char {
    match n {
        1 => '\u{2460}',
        2 => '\u{2461}',
        3 => '\u{2462}',
        4 => '\u{2463}',
        5 => '\u{2464}',
        6 => '\u{2465}',
        7 => '\u{2466}',
        8 => '\u{2467}',
        9 => '\u{2468}',
        10 => '\u{2469}',
        _ => '\u{00b7}',
    }
}

fn draw_tsunami_banner(
    ui: &mut Ui,
    p: &ThemePalette,
    snap: &AppSnapshot,
    travel: Option<&TravelEngine>,
    zoom: f32,
) {
    let Some(t) = snap.tsunami.as_ref() else {
        return;
    };
    if t.cancelled {
        return;
    }
    // 仅 JMA：NMEFC/CENC 类「海啸信息」报文不进顶条
    if t.areas_list.is_empty() && t.title.contains("海啸信息") {
        return;
    }
    let (r, g, b) = starquake_core::tsunami_official_rgb(&t.grade);
    let accent = Color32::from_rgb(r, g, b);
    let title = if t.title.is_empty() {
        starquake_core::tsunami_grade_zh(&t.grade).to_string()
    } else {
        t.title.clone()
    };
    let desc = match t.grade.as_str() {
        "MajorWarning" | "Major" => crate::i18n::t("大海啸警报：请立即撤离到高处或坚固建筑。"),
        "Warning" => crate::i18n::t("海啸警报：海岸附近请立即撤离。"),
        "Watch" => crate::i18n::t("海啸注意报：注意海面异常，远离海岸。"),
        _ => crate::i18n::t("海面可能发生些许变动，但无需采取特殊防灾措施。"),
    };
    // 预报/注意报用深底+色框；警报级用官方色实底
    let (fill, border, title_c, body_c) = match t.grade.as_str() {
        "MajorWarning" | "Major" | "Warning" => (
            Color32::from_rgba_unmultiplied(r, g, b, 230),
            accent,
            Color32::WHITE,
            Color32::from_rgb(0xF8, 0xFA, 0xFC),
        ),
        "Watch" => (
            Color32::from_rgba_unmultiplied(20, 20, 24, 230),
            accent,
            Color32::from_rgb(0xFF, 0xFF, 0x66),
            Color32::from_rgb(0xE5, 0xE7, 0xEB),
        ),
        _ => (
            Color32::from_rgba_unmultiplied(12, 24, 40, 235),
            accent,
            Color32::from_rgb(0x7D, 0xF9, 0xFF),
            Color32::from_rgb(0xD1, 0xD5, 0xDB),
        ),
    };
    // 有预警信息头时下移（按堆叠条数）；无预警时贴到左上角
    let top_y = {
        let h = eew_header_stack_height(snap, travel, zoom);
        if h > 0.0 {
            OVERLAY_TOP_Y * zoom + h + 8.0 * zoom
        } else {
            OVERLAY_TOP_Y * zoom
        }
    };
    let left_x = 12.0 * zoom;
    let left_w = 340.0 * zoom;
    let pos = ui.max_rect().left_top() + Vec2::new(left_x, top_y);
    let rect = egui::Rect::from_min_size(pos, Vec2::new(left_w + 24.0 * zoom, 220.0 * zoom));
    overlay_child(ui, "tsunami_banner", rect, |ui| {
            Frame::NONE
                .fill(fill)
                .stroke(Stroke::new(2.0_f32 * zoom, border))
                .corner_radius(6.0 * zoom)
                .inner_margin(margin_xy(10.0 * zoom, 10.0 * zoom))
                .show(ui, |ui| {
                    ui.set_width(left_w);
                    ui.label(
                        RichText::new(format!("{title}生效中"))
                            .color(title_c)
                            .strong()
                            .size(16.0 * zoom),
                    );
                    ui.add_space(4.0 * zoom);
                    ui.label(RichText::new(desc).color(body_c).size(12.5 * zoom));
                    if t.expire_ms > 0 {
                        ui.add_space(2.0 * zoom);
                        let exp = chrono::DateTime::from_timestamp_millis(t.expire_ms)
                            .map(|dt| {
                                dt.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                                    .format("预计解除 %Y/%m/%d %H:%M")
                                    .to_string()
                            })
                            .unwrap_or_else(|| crate::i18n::t("预计解除 —").into());
                        ui.label(RichText::new(exp).color(body_c).size(11.5 * zoom));
                    }
                    ui.add_space(6.0 * zoom);
                    let line = ui.available_width();
                    let (resp, painter) = ui.allocate_painter(Vec2::new(line, 2.0), Sense::hover());
                    painter.rect_filled(resp.rect, 0.0, border);
                    ui.add_space(4.0);
                    let area_line = if t.areas.is_empty() || t.areas == "—" {
                        starquake_core::tsunami_grade_zh(&t.grade).to_string()
                    } else {
                        t.areas.clone()
                    };
                    ui.label(RichText::new(area_line).color(body_c).size(12.0 * zoom));
                });
            let _ = p;
        });
}


/// 台风实况详情：固定在测站卡片下方，避免地图锚点浮层带尾巴。
/// 点击标题栏名称/标题 → 飞向该台风；返回 `(lon, lat, zoom)`。
fn draw_typhoon_detail_below_stations(
    ui: &mut Ui,
    p: &ThemePalette,
    hover: Option<&TyphoonHoverHit>,
    typhoons: &[starquake_core::TyphoonInfo],
    station_bottom_y: f32,
    zoom: f32,
) -> Option<(f64, f64, f64)> {
    let hit_owned;
    let hit: Option<&TyphoonHoverHit> = if let Some(h) = hover {
        Some(h)
    } else if let Some(tf) = typhoons.first() {
        let last = tf.points.last();
        let time_text = last.map(|x| x.time_text.clone()).unwrap_or_default();
        let strong = if tf.strong.is_empty() {
            last.map(|x| x.strong.clone()).unwrap_or_default()
        } else {
            tf.strong.clone()
        };
        hit_owned = TyphoonHoverHit {
            anchor: egui::pos2(0.0, 0.0),
            id: tf.id.clone(),
            name: tf.name.clone(),
            strong,
            power: if tf.power > 0 {
                tf.power
            } else {
                last.map(|x| x.power).unwrap_or(0)
            },
            time_text,
            longitude: tf.longitude,
            latitude: tf.latitude,
            speed_ms: last
                .map(|x| x.speed_ms)
                .filter(|&v| v > 0.0)
                .unwrap_or(tf.wind_speed_ms),
            pressure_hpa: last
                .map(|x| x.pressure_hpa)
                .filter(|&v| v > 0.0)
                .unwrap_or(tf.pressure_hpa),
            move_direction: last
                .map(|x| x.move_direction.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| tf.move_direction.clone()),
            move_speed_kmh: last
                .map(|x| x.move_speed_kmh)
                .filter(|&v| v > 0.0)
                .unwrap_or(tf.move_speed_kmh),
            is_center: true,
        };
        Some(&hit_owned)
    } else {
        None
    };
    let Some(hit) = hit else {
        return None;
    };

    let time = format_typhoon_time_ui(&hit.time_text);
    let header = if time.is_empty() {
        format!("{} {}", hit.strong, hit.name)
    } else {
        format!("{} {}  {}", hit.strong, hit.name, time)
    };
    let power = if hit.power > 0 {
        format!("{}级", hit.power)
    } else {
        "\u{2014}".into()
    };
    let speed = if hit.speed_ms > 0.0 {
        format!("{:.0} m/s ({power})", hit.speed_ms)
    } else {
        format!("\u{2014} ({power})")
    };
    let pressure = if hit.pressure_hpa > 0.0 {
        format!("{:.0} hPa", hit.pressure_hpa)
    } else {
        "\u{2014}".into()
    };
    let mv = if hit.move_direction.is_empty() && hit.move_speed_kmh <= 0.0 {
        "\u{2014}".into()
    } else {
        format!(
            "{} ({:.0} KM/H)",
            if hit.move_direction.is_empty() {
                "\u{2014}"
            } else {
                hit.move_direction.as_str()
            },
            hit.move_speed_kmh
        )
    };
    let lines = [
        format!("{}", crate::i18n::t("编号: {}").replace("{}", &hit.id.to_string())),
        format!(
            "中心位置: ({:.1}\u{00b0}N, {:.1}\u{00b0}E)",
            hit.latitude, hit.longitude
        ),
        format!("最大风速: {speed}"),
        format!("中心气压: {pressure}"),
        format!("移动速度: {mv}"),
    ];

    // 与右上三测站卡片同宽对齐，并与卡片底边留足间距，避免贴地图
    let card_w = 240.0 * zoom; // 76*3 + 6*2
    let pad = 8.0 * zoom;
    let line_h = 16.0 * zoom;
    let header_h = 26.0 * zoom;
    let body_h = pad + line_h * lines.len() as f32 + 4.0 * zoom;
    let total_h = header_h + 2.0 * zoom + body_h;
    let right = ui.max_rect().right() - 12.0 * zoom;
    let origin = egui::pos2(right - card_w, station_bottom_y + 14.0 * zoom);
    let rect = egui::Rect::from_min_size(origin, Vec2::new(card_w, total_h));
    let (r, g, b) = starquake_core::typhoon_color_rgb(&hit.strong, hit.power);
    let mut clicked = false;
    overlay_child(ui, "typhoon_detail_card", rect, |ui| {
        let header_rect = egui::Rect::from_min_size(rect.min, Vec2::new(card_w, header_h));
        let body_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x, header_rect.bottom() + 2.0 * zoom),
            Vec2::new(card_w, body_h),
        );
        // 标题栏可点：跳转到台风中心（同层 interact，不抢整图拖拽）
        let hdr_resp = ui.interact(
            header_rect,
            ui.id().with("typhoon_hdr_click"),
            Sense::click(),
        );
        if hdr_resp.clicked() {
            clicked = true;
        }
        if hdr_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let painter = ui.painter();
        let light = p.text.r() < 128;
        let header_stroke = if light {
            Color32::from_rgb(0x6B, 0x72, 0x80)
        } else {
            Color32::from_rgb(0xAA, 0xAA, 0xBB)
        };
        let body_stroke = if light {
            Color32::from_rgb(0xC5, 0xCB, 0xD3)
        } else {
            Color32::from_rgb(0x44, 0x48, 0x58)
        };
        let radius = 4.0 * zoom;
        painter.rect_filled(
            header_rect,
            radius,
            Color32::from_rgb(r, g, b).gamma_multiply(0.85),
        );
        painter.rect_stroke(
            header_rect,
            radius,
            Stroke::new(1.0_f32 * zoom, header_stroke),
            egui::StrokeKind::Outside,
        );
        let body_fill = Color32::from_rgba_unmultiplied(
            p.panel_solid.r(),
            p.panel_solid.g(),
            p.panel_solid.b(),
            0xF0,
        );
        painter.rect_filled(body_rect, radius, body_fill);
        painter.rect_stroke(
            body_rect,
            radius,
            Stroke::new(1.0_f32 * zoom, body_stroke),
            egui::StrokeKind::Outside,
        );
        painter.text(
            egui::pos2(header_rect.min.x + pad, header_rect.center().y),
            egui::Align2::LEFT_CENTER,
            header,
            FontId::proportional(13.0 * zoom),
            Color32::WHITE,
        );
        let mut y = body_rect.min.y + pad * 0.6;
        for line in &lines {
            painter.text(
                egui::pos2(body_rect.min.x + pad, y),
                egui::Align2::LEFT_TOP,
                line,
                FontId::proportional(12.0 * zoom),
                p.text,
            );
            y += line_h;
        }
    });
    if !clicked {
        return None;
    }
    if hit.latitude.abs() <= 0.01 && hit.longitude.abs() <= 0.01 {
        return None;
    }
    // 点击标题飞向中心：固定 Z9（原先按风圈 fit 再 cap 到 7，偏远）
    Some((hit.longitude, hit.latitude, 9.0))
}

fn format_typhoon_time_ui(time_str: &str) -> String {
    let bytes = time_str.as_bytes();
    let mut i = 0usize;
    while i + 10 <= bytes.len() {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && (bytes[i + 4] == b'-' || bytes[i + 4] == b'/')
        {
            let day = std::str::from_utf8(&bytes[i + 8..i + 10])
                .ok()
                .and_then(|s| s.parse::<u32>().ok());
            let mut j = i + 10;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'T') {
                j += 1;
            }
            if let Some(day) = day {
                if j + 2 <= bytes.len()
                    && bytes[j].is_ascii_digit()
                    && bytes[j + 1].is_ascii_digit()
                {
                    if let Ok(hour) = std::str::from_utf8(&bytes[j..j + 2])
                        .unwrap_or("")
                        .parse::<u32>()
                    {
                        return format!("{day}日{hour}时");
                    }
                }
            }
        }
        i += 1;
    }
    time_str.to_string()
}

/// 测站网实时震度卡片布局结果。
struct StationCardLayout {
    clicked: Option<&'static str>,
    /// 卡片底边 Y；无卡片时为顶栏下方起点，供台风详情承接。
    bottom_y: f32,
}

/// 地图右上角实时震度/烈度卡片（对齐 EEWCN sourceMaxCard）。
/// 点击卡片返回台网 key，供跳转到该网最大测站。
fn draw_realtime_intensity_card(
    ui: &mut Ui,
    p: &ThemePalette,
    snap: &AppSnapshot,
    zoom: f32,
) -> StationCardLayout {
    let top_y = ui.max_rect().right_top().y + OVERLAY_TOP_Y * zoom;
    if snap.stations.is_empty() {
        return StationCardLayout {
            clicked: None,
            bottom_y: top_y,
        };
    }
    // (key, label, max_value, is_kma_mmi, ts)
    let mut cards: Vec<(&'static str, &'static str, f64, bool, i64)> = Vec::new();
    for (key, label) in [("kmoni", "Kmoni"), ("snet", "S-Net"), ("kma", "KMA")] {
        let mut max_v = -99.0_f64;
        let mut any = false;
        let is_kma = key == "kma";
        for s in &snap.stations {
            if !s.id.starts_with(&format!("{key}:")) {
                continue;
            }
            any = true;
            let v = if is_kma {
                s.intensity_level as f64
            } else {
                s.instrumental
            };
            if v > max_v {
                max_v = v;
            }
        }
        if any {
            let ts = snap.station_updated_ms.get(key).copied().unwrap_or(0);
            cards.push((key, label, max_v, is_kma, ts));
        }
    }
    if cards.is_empty() {
        let max_inst = snap
            .stations
            .iter()
            .map(|s| s.instrumental)
            .fold(-99.0, f64::max);
        let ts = snap
            .station_updated_ms
            .values()
            .copied()
            .max()
            .unwrap_or(0);
        cards.push(("all", "NIED", max_inst, false, ts));
    }

    let card_w = 76.0 * zoom;
    let card_h = 88.0 * zoom;
    let gap = 6.0 * zoom;
    let now_ms = chrono::Utc::now().timestamp_millis();
    const STALE_MS: i64 = 8_000;
    let total_w = card_w * cards.len() as f32 + gap * (cards.len().saturating_sub(1) as f32);
    let top_right = ui.max_rect().right_top() + Vec2::new(-12.0 * zoom, OVERLAY_TOP_Y * zoom);
    let rect = egui::Rect::from_min_size(
        egui::pos2(top_right.x - total_w.max(72.0 * zoom), top_right.y),
        Vec2::new(total_w.max(72.0 * zoom), card_h),
    );
    let mut clicked: Option<&'static str> = None;
    overlay_child(ui, "rt_intensity_card", rect, |ui| {
        ui.set_min_size(Vec2::new(total_w, card_h));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (k, label, val, is_kma, ts) in &cards {
                let light = p.text.r() < 128;
                let dash = if light {
                    p.text_dim
                } else {
                    Color32::from_rgb(0x66, 0x66, 0x66)
                };
                let (value, value_color) = if *is_kma {
                    if *val >= 1.0 {
                        let lv = (*val).round().clamp(1.0, 12.0) as u8;
                        let rgb = starquake_core::palette::cn_intensity_rgb(lv);
                        (format!("{lv}"), Color32::from_rgb(rgb.r, rgb.g, rgb.b))
                    } else {
                        ("-".into(), dash)
                    }
                } else if *val >= -3.0 {
                    let rgb = starquake_core::instrumental_rgb(*val);
                    (
                        starquake_core::instrumental_half_step_text(*val),
                        Color32::from_rgb(rgb.r, rgb.g, rgb.b),
                    )
                } else {
                    ("-".into(), dash)
                };
                let time_text = if *ts > 0 {
                    format_clock_hms(*ts)
                } else {
                    "--:--:--".into()
                };
                let stale = if *k == "snet" { 90_000_i64 } else { STALE_MS };
                let connected = *ts > 0 && (now_ms - *ts).abs() <= stale;
                let time_color = if connected {
                    if light { p.text_muted } else { Color32::WHITE }
                } else {
                    p.danger
                };
                let card_fill = if light {
                    p.panel_solid
                } else {
                    Color32::from_rgb(0x1A, 0x1A, 0x1A)
                };
                let head_fill = if light {
                    p.tab_idle
                } else {
                    Color32::from_rgb(0x2D, 0x2D, 0x2D)
                };
                let stroke_c = if light {
                    Color32::from_rgb(0xC5, 0xCB, 0xD3)
                } else {
                    Color32::from_rgb(0x40, 0x40, 0x40)
                };
                let title_color = if light { p.text } else { Color32::WHITE };

                let corner = 6.0 * zoom;
                let (r, resp) =
                    ui.allocate_exact_size(Vec2::new(card_w, card_h), Sense::click());
                if resp.clicked() {
                    clicked = Some(*k);
                }
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let painter = ui.painter();
                painter.rect_filled(r, corner, card_fill);
                let head = egui::Rect::from_min_size(r.min, Vec2::new(r.width(), 20.0 * zoom));
                painter.rect_filled(
                    head,
                    egui::CornerRadius {
                        nw: corner as u8,
                        ne: corner as u8,
                        sw: 0,
                        se: 0,
                    },
                    head_fill,
                );
                painter.rect_stroke(
                    r,
                    corner,
                    Stroke::new(1.0_f32 * zoom, stroke_c),
                    egui::StrokeKind::Inside,
                );
                painter.hline(
                    r.x_range(),
                    head.bottom(),
                    Stroke::new(1.0_f32 * zoom, stroke_c),
                );
                painter.text(
                    head.center(),
                    egui::Align2::CENTER_CENTER,
                    *label,
                    FontId::proportional(12.0 * zoom),
                    title_color,
                );
                let body = egui::Rect::from_min_max(
                    egui::pos2(r.left(), head.bottom()),
                    egui::pos2(r.right(), r.bottom() - 18.0 * zoom),
                );
                // 加粗：双层叠绘
                let value_font = FontId::new(28.0 * zoom, FontFamily::Proportional);
                painter.text(
                    body.center() + Vec2::new(0.35 * zoom, 0.0),
                    egui::Align2::CENTER_CENTER,
                    &value,
                    value_font.clone(),
                    value_color,
                );
                painter.text(
                    body.center(),
                    egui::Align2::CENTER_CENTER,
                    &value,
                    value_font,
                    value_color,
                );
                painter.text(
                    egui::pos2(r.center().x, r.bottom() - 10.0 * zoom),
                    egui::Align2::CENTER_CENTER,
                    time_text,
                    FontId::proportional(11.0 * zoom),
                    time_color,
                );
            }
        });
    });
    StationCardLayout {
        clicked,
        bottom_y: rect.bottom(),
    }
}



fn format_clock_hms(ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    let Some(dt) = Utc.timestamp_millis_opt(ms).single() else {
        return "--:--:--".into();
    };
    let local = dt.with_timezone(&Local);
    local.format("%H:%M:%S").to_string()
}


fn draw_legend_bar(ui: &mut Ui, p: &ThemePalette, zoom: f32) {
    // 样式：  图例
    //      12□ □7
    // 数字紧贴色块；左列个位右对齐；右列强弱用 +/-
    let row = 17.0 * zoom;
    let sw = 14.0 * zoom;
    let mid = 2.0 * zoom;
    let left_w = 18.0 * zoom;
    let right_w = 28.0 * zoom;
    const CN_N: usize = 12;
    // 右列：离散 JMA 震度阶级（与列表 shindo SVG / jma_shindo_rgb 一致）
    const SH_N: usize = 10;
    const SH_ROWS: [(&str, u8); SH_N] = [
        ("7", 9),
        ("6+", 8),
        ("6-", 7),
        ("5+", 6),
        ("5-", 5),
        ("4", 4),
        ("3", 3),
        ("2", 2),
        ("1", 1),
        ("0", 0),
    ];
    let width = left_w + sw + mid + sw + right_w;
    let rows = CN_N.max(SH_N);
    let body_h = row * rows as f32;
    let bottom_left = ui.max_rect().left_bottom() + Vec2::new(4.0 * zoom, -4.0 * zoom);
    let h = body_h + 22.0 * zoom;
    let rect = egui::Rect::from_min_max(
        egui::pos2(bottom_left.x, bottom_left.y - h),
        egui::pos2(bottom_left.x + width, bottom_left.y),
    );
    overlay_child(ui, "legend_bar", rect, |ui| {
        ui.spacing_mut().window_margin = Margin::ZERO;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.set_min_size(Vec2::new(width, h));
        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
            ui.set_width(width);
            ui.label(
                RichText::new(crate::i18n::t("图例"))
                    .color(p.text_muted)
                    .strong()
                    .size(15.0 * zoom),
            );
        });
        let (resp, painter) = ui.allocate_painter(Vec2::new(width, body_h), Sense::hover());
        let r = resp.rect;
        let cn_sw_x = r.left() + left_w;
        let sh_sw_x = cn_sw_x + sw + mid;
        let pad = 1.5 * zoom;
        let font_sz = 12.0 * zoom;

        for i in 0..CN_N {
            let lv = (12 - i) as u8;
            let y0 = r.top() + row * i as f32;
            let cy = y0 + row * 0.5;
            painter.text(
                egui::pos2(cn_sw_x, cy),
                egui::Align2::RIGHT_CENTER,
                format!("{lv}"),
                FontId::proportional(font_sz),
                p.text_dim,
            );
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(cn_sw_x, y0 + pad),
                    egui::pos2(cn_sw_x + sw, y0 + row - pad),
                ),
                1.5 * zoom,
                rgb(cn_intensity_rgb(lv)),
            );
        }

        for i in 0..SH_N {
            let (label, level) = SH_ROWS[i];
            let y0 = r.top() + row * i as f32;
            let cy = y0 + row * 0.5;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(sh_sw_x, y0 + pad),
                    egui::pos2(sh_sw_x + sw, y0 + row - pad),
                ),
                1.5 * zoom,
                rgb(jma_shindo_rgb(level)),
            );
            painter.text(
                egui::pos2(sh_sw_x + sw, cy),
                egui::Align2::LEFT_CENTER,
                label,
                FontId::proportional(font_sz),
                p.text_muted,
            );
        }
    });
}

fn draw_countdown_hud(
    ui: &mut Ui,
    p: &ThemePalette,
    snap: &AppSnapshot,
    show_nied: bool,
    map_rect: egui::Rect,
    local_lat: Option<f64>,
    local_lon: Option<f64>,
    zoom: f32,
) {
    let local = Local::now();
    let clock = local.format("%Y/%m/%d %H:%M:%S").to_string();
    let jst_clock = if show_nied {
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        Some(
            local
                .with_timezone(&jst)
                .format("%Y/%m/%d %H:%M:%S")
                .to_string(),
        )
    } else {
        None
    };

    // 贴地图视口右下角；时钟用绝对坐标绘制，避免 bottom_up 嵌套把标题/时间叠成重影
    let br = map_rect.right_bottom() + Vec2::new(-4.0 * zoom, -4.0 * zoom);
    let hud_w = 280.0 * zoom;
    let clock_block_h = 36.0 * zoom;
    let sun_line = match (local_lat, local_lon) {
        (Some(lat), Some(lon)) => sunrise_sunset_local_hm(lat, lon, local.timestamp_millis())
            .map(|(rise, set)| {
                format!(
                    "{} {}  {} {}",
                    crate::i18n::t("日出"),
                    format_hm_minutes(rise),
                    crate::i18n::t("日落"),
                    format_hm_minutes(set)
                )
            }),
        _ => None,
    };
    let n_clocks = 1 + usize::from(show_nied) + usize::from(sun_line.is_some());
    let countdown_h = if snap.countdown_s.is_some() {
        72.0 * zoom
    } else {
        0.0
    };
    let gap = 6.0 * zoom;
    let hud_h = countdown_h + clock_block_h * n_clocks as f32 + gap + 8.0 * zoom;
    let rect = egui::Rect::from_min_size(
        egui::pos2(br.x - hud_w, br.y - hud_h),
        Vec2::new(hud_w, hud_h),
    );
    overlay_child(ui, "countdown_hud", rect, |ui| {
        ui.spacing_mut().window_margin = Margin::ZERO;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let r = ui.max_rect();
        let mut y_bottom = r.bottom() - 2.0 * zoom;

        // 自下而上：东京 → 北京 → 日出日落 → 视觉自上而下为日出日落、北京、东京
        if let Some(jst) = jst_clock.as_ref() {
            y_bottom = paint_tz_clock_at(
                ui,
                r.right() - 2.0 * zoom,
                y_bottom,
                crate::i18n::t("东京时间"),
                jst,
                zoom,
            );
        }
        y_bottom = paint_tz_clock_at(
            ui,
            r.right() - 2.0 * zoom,
            y_bottom,
            crate::i18n::t("北京时间"),
            &clock,
            zoom,
        );
        if let Some(sun) = sun_line.as_ref() {
            y_bottom = paint_tz_clock_at(
                ui,
                r.right() - 2.0 * zoom,
                y_bottom,
                crate::i18n::t("所在地"),
                sun,
                zoom,
            );
        }

        if let Some(sec) = snap.countdown_s {
            let cd_top = (y_bottom - gap - countdown_h).max(r.top());
            let cd_rect = egui::Rect::from_min_max(
                egui::pos2(r.left(), cd_top),
                egui::pos2(r.right(), y_bottom - gap),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(cd_rect), |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    let title = if sec > 0 { "S波即将到达" } else { "S波已到达" };
                    outlined_label(ui, title, Color32::WHITE, 14.0 * zoom, false);
                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), 44.0 * zoom),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0 * zoom;
                            let mm = sec / 60;
                            let ss = sec % 60;
                            let time_txt = format!("{mm}:{ss:02}");
                            let time_color = if sec > 10 {
                                Color32::from_rgb(0xFF, 0xEB, 0x3B)
                            } else {
                                p.danger
                            };
                            Frame::NONE
                                .fill(Color32::from_rgba_unmultiplied(0x66, 0x66, 0x66, 0x80))
                                .inner_margin(margin_xy(6.0 * zoom, 2.0 * zoom))
                                .show(ui, |ui| {
                                    outlined_label(ui, &time_txt, time_color, 34.0 * zoom, false);
                                });
                            let lv = snap.countdown_local_level;
                            let label = if snap.countdown_local_text.is_empty() {
                                if lv == 0 {
                                    "0".into()
                                } else {
                                    lv.to_string()
                                }
                            } else {
                                snap.countdown_local_text.clone()
                            };
                            let bg = if lv == 0 {
                                Color32::from_rgb(0x2A, 0x2A, 0x2E)
                            } else {
                                rgb(color_for(snap.countdown_local_kind, lv))
                            };
                            let fg = intensity_fg(bg);
                            let border = if lv == 0 {
                                Stroke::new(1.0_f32 * zoom, Color32::WHITE)
                            } else {
                                Stroke::NONE
                            };
                            Frame::NONE
                                .fill(bg)
                                .stroke(border)
                                .inner_margin(margin_xy(8.0 * zoom, 2.0 * zoom))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(label)
                                            .color(fg)
                                            .strong()
                                            .size(34.0 * zoom),
                                    );
                                });
                        },
                    );
                });
            });
        }
    });
}

/// 在右下角绝对位置绘制一块时钟；返回下一块可用的 bottom y。
fn paint_tz_clock_at(
    ui: &mut Ui,
    right: f32,
    bottom: f32,
    title: &str,
    datetime: &str,
    zoom: f32,
) -> f32 {
    let h = 36.0 * zoom;
    let top = bottom - h;
    let x = right.floor();
    let painter = ui.painter();
    let title_font = FontId::new(13.0 * zoom, FontFamily::Proportional);
    let time_font = FontId::new(15.0 * zoom, FontFamily::Proportional);
    painter.text(
        egui::pos2(x, (top + 12.0 * zoom).floor()),
        egui::Align2::RIGHT_CENTER,
        title,
        title_font,
        Color32::from_rgb(0xCC, 0xCC, 0xCC),
    );
    painter.text(
        egui::pos2(x, (top + 28.0 * zoom).floor()),
        egui::Align2::RIGHT_CENTER,
        datetime,
        time_font,
        Color32::WHITE,
    );
    top - 2.0 * zoom
}

fn intensity_fg(bg: Color32) -> Color32 {
    let y = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if y > 160.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

fn outlined_label(ui: &mut Ui, text: &str, color: Color32, size: f32, monospace: bool) {
    let font = if monospace {
        FontId::monospace(size)
    } else {
        FontId::proportional(size)
    };
    let galley = ui.fonts(|f| f.layout_no_wrap(text.to_string(), font, color));
    let size_v = galley.size();
    let (resp, painter) = ui.allocate_painter(size_v, Sense::hover());
    let pos = resp.rect.min;
    // 仅四向细描边，避免八向叠绘把笔画撑成「加粗」
    let stroke = Color32::from_rgba_unmultiplied(0, 0, 0, 160);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        painter.galley(pos + Vec2::new(dx, dy), galley.clone(), stroke);
    }
    painter.galley(pos, galley, color);
}

enum SideAction {
    List(SidebarTab, usize),
    Settings,
    CloseSidebar,
    OpenSidebar,
    DetachSidebar,
    AttachSidebar,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowClick {
    None,
    Primary,
}

fn health_dot(p: &ThemePalette, status: starquake_core::HealthStatus) -> Color32 {
    match status {
        starquake_core::HealthStatus::Normal => p.ok,
        starquake_core::HealthStatus::Fluctuating => p.warn,
        starquake_core::HealthStatus::Abnormal => p.danger,
    }
}

/// 返回侧栏操作
fn draw_sidebar(
    ui: &mut Ui,
    p: &ThemePalette,
    snap: &mut AppSnapshot,
    scale: &str,
    icons: &IntensityIcons,
    weather_icons: &mut WeatherIconCache,
    show_volcano_icons: bool,
    source_enabled: [bool; 5],
    detached: bool,
) -> Option<SideAction> {
    let mut clicked = None;
    // eewcn 侧栏：固定字号；宽度变化只影响换行/行高，不缩放 UI
    let z = sidebar_content_zoom(ui.available_width());
    ui.set_min_width(ui.available_width());
    Frame::NONE
        .fill(p.sidebar_bg)
        .inner_margin(Margin::symmetric(10, 9))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let title_tip = if detached {
                    crate::i18n::t("双击标题嵌回；或拖近主窗口右侧松手；也可点「嵌回」")
                } else {
                    crate::i18n::t("拖动标题可分离为独立窗口")
                };
                let title_resp = ui
                    .add(
                        egui::Label::new(
                            RichText::new(crate::i18n::t("地震情报实况"))
                                .color(p.text)
                                .strong()
                                .size(15.5 * z),
                        )
                        .sense(Sense::click_and_drag()),
                    )
                    .on_hover_text(title_tip)
                    .on_hover_cursor(if detached {
                        egui::CursorIcon::PointingHand
                    } else {
                        egui::CursorIcon::Grab
                    });
                let drag_id = egui::Id::new("sidebar_title_undock_drag");
                if detached {
                    if title_resp.double_clicked() {
                        clicked = Some(SideAction::AttachSidebar);
                    }
                } else if title_resp.dragged() {
                    let delta = title_resp.drag_delta();
                    let should_detach = ui.ctx().data_mut(|d| {
                        let acc = d.get_temp_mut_or_default::<Vec2>(drag_id);
                        *acc += delta;
                        if acc.length() >= 40.0 {
                            *acc = Vec2::ZERO;
                            true
                        } else {
                            false
                        }
                    });
                    if should_detach {
                        clicked = Some(SideAction::DetachSidebar);
                    }
                } else if title_resp.drag_stopped() {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(drag_id, Vec2::ZERO);
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if detached {
                        // 分离态：关窗 = 嵌回；额外给显式「嵌回」按钮
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("\u{00d7}").size(16.0 * z).color(p.text_muted),
                                )
                                .fill(Color32::TRANSPARENT)
                                .corner_radius(6.0 * z)
                                .min_size(Vec2::new(26.0 * z, 26.0 * z)),
                            )
                            .on_hover_text(crate::i18n::t("嵌回主窗口侧栏"))
                            .clicked()
                        {
                            clicked = Some(SideAction::AttachSidebar);
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(crate::i18n::t("嵌回"))
                                        .size(13.0 * z)
                                        .color(p.text_muted),
                                )
                                .fill(p.tab_idle)
                                .corner_radius(6.0 * z)
                                .min_size(Vec2::new(52.0 * z, 26.0 * z)),
                            )
                            .on_hover_text(crate::i18n::t("嵌回主窗口侧栏"))
                            .clicked()
                        {
                            clicked = Some(SideAction::AttachSidebar);
                        }
                    } else if ui
                        .add(
                            egui::Button::new(
                                RichText::new("\u{00d7}").size(16.0 * z).color(p.text_muted),
                            )
                            .fill(Color32::TRANSPARENT)
                            .corner_radius(6.0 * z)
                            .min_size(Vec2::new(26.0 * z, 26.0 * z)),
                        )
                        .on_hover_text(crate::i18n::t("关闭侧栏"))
                        .clicked()
                    {
                        clicked = Some(SideAction::CloseSidebar);
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(crate::i18n::t("设置")).size(13.0 * z).color(p.text_muted),
                            )
                            .fill(p.tab_idle)
                            .corner_radius(6.0 * z)
                            .min_size(Vec2::new(52.0 * z, 26.0 * z)),
                        )
                        .clicked()
                    {
                        clicked = Some(SideAction::Settings);
                    }
                });
            });

            ui.add_space(6.0 * z);
            ui.horizontal(|ui| {
                for (i, (name, st)) in [
                    ("Jian", snap.health_jian),
                    ("Wolfx", snap.health_wolfx),
                    ("P2P", snap.health_p2p),
                    ("Pancakes", snap.health_pancakes),
                    ("FSSN", snap.health_fssn),
                ]
                .into_iter()
                .enumerate()
                {
                    let on = source_enabled[i];
                    let tip = if on {
                        format!("{name}\u{00b7}{}", crate::i18n::t(st.as_zh()))
                    } else {
                        format!("{}", crate::i18n::t("{name}·关闭").replace("{name}", name))
                    };
                    let dot = if on {
                        health_dot(p, st)
                    } else {
                        p.text_dim
                    };
                    let resp = ui
                        .horizontal(|ui| {
                            let (r, painter) =
                                ui.allocate_painter(Vec2::splat(9.0 * z), Sense::hover());
                            painter.circle_filled(r.rect.center(), 3.8 * z, dot);
                            ui.label(RichText::new(name).size(12.5 * z).color(p.text_dim));
                        })
                        .response;
                    resp.on_hover_text(tip);
                    ui.add_space(4.0 * z);
                }
            });

            ui.add_space(8.0 * z);
            draw_side_tabs(ui, p, &mut snap.tab, z);
            ui.add_space(6.0 * z);

            let tab = snap.tab;
            let sel = snap.selection;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    match tab {
                            SidebarTab::Records => {
                                if snap.records.is_empty() {
                                    empty_hint(
                                        ui,
                                        p,
                                        crate::i18n::t("暂无速报"),
                                        crate::i18n::t("已启用数据源时将自动填入；也可在设置中检查连接状态。"),
                                        z,
                                    );
                                }
                                for (i, r) in snap.records.iter().enumerate() {
                                    let selected = sel
                                        == Some(ListSelection {
                                            tab: SidebarTab::Records,
                                            index: i,
                                        });
                                    let mut place = place_with_agency(&r.place, &r.agency.0);
                                    if r.is_auto {
                                        place.push_str(" [自动测定]");
                                    }
                                    let (_, _, col, _) = intensity_chip(
                                        r.intensity_kind,
                                        r.intensity_level,
                                        &r.intensity_text,
                                        scale,
                                    );
                                    // 单行：时间 震级 深度，避免分控件叠字
                                    let time = if r.origin_ms > 0 {
                                        format_list_time(r.origin_ms)
                                    } else {
                                        r.time_text.clone()
                                    };
                                    let sub = format!(
                                        "{}  {}  {}",
                                        time,
                                        r.magnitude_label(),
                                        r.depth_label()
                                    );
                                    if list_row(
                                        ui,
                                        p,
                                        icons,
                                        &place,
                                        &sub,
                                        "",
                                        r.intensity_kind,
                                        r.intensity_level,
                                        &r.intensity_text,
                                        scale,
                                        col,
                                        selected,
                                        egui::Id::new(("rec", i)),
                                        z,
                                    ) == RowClick::Primary
                                    {
                                        clicked = Some(SideAction::List(SidebarTab::Records, i));
                                    }
                                }
                            }
                            SidebarTab::Eew => {
                                let sidebar = eew_sidebar_entries(&snap.eew_list);
                                if sidebar.is_empty() {
                                    empty_hint(
                                        ui,
                                        p,
                                        crate::i18n::t("暂无预警"),
                                        crate::i18n::t("连接正常后，紧急地震速报会出现在此列表。"),
                                        z,
                                    );
                                }
                                for (real_i, e) in sidebar {
                                    let selected = sel
                                        == Some(ListSelection {
                                            tab: SidebarTab::Eew,
                                            index: real_i,
                                        });
                                    let mut place = place_with_agency(&e.place, &e.agency.0);
                                    if let Some(mark) = e.serial_mark() {
                                        place.push_str("  ");
                                        place.push_str(&mark);
                                    }
                                    let time = format_list_time(e.origin_ms);
                                    let mag = format!("M{:.1}", e.magnitude);
                                    let depth = e.depth_label();
                                    // 第二行仅时间/震级/深度；报数已挂在机构后
                                    let sub = format!("{time}  {mag}  {depth}");
                                    let (_, _, col, _) = intensity_chip(
                                        e.intensity_kind,
                                        e.intensity_level,
                                        &e.max_intensity_text,
                                        scale,
                                    );
                                    if list_row(
                                        ui,
                                        p,
                                        icons,
                                        &place,
                                        &sub,
                                        "",
                                        e.intensity_kind,
                                        e.intensity_level,
                                        &e.max_intensity_text,
                                        scale,
                                        col,
                                        selected,
                                        egui::Id::new(("eew", real_i)),
                                        z,
                                    ) == RowClick::Primary
                                    {
                                        clicked = Some(SideAction::List(SidebarTab::Eew, real_i));
                                    }
                                }
                            }
                            SidebarTab::Volcano => {
                                if snap.volcanoes.is_empty() {
                                    empty_hint(
                                        ui,
                                        p,
                                        crate::i18n::t("暂无火山情报"),
                                        crate::i18n::t("启用 Pancakes JMA-Atom 后，降灰预报等会出现在此列表。"),
                                        z,
                                    );
                                }
                                for (i, v) in snap.volcanoes.iter().enumerate() {
                                    let selected = sel
                                        == Some(ListSelection {
                                            tab: SidebarTab::Volcano,
                                            index: i,
                                        });
                                    // 首行：火山名 + 情报类型；次行发布时间；末行描述
                                    let headline = volcano_list_headline(&v.name, &v.title);
                                    let place = place_with_agency(&headline, &v.agency.0);
                                    let desc = if v.title.is_empty() {
                                        v.name.as_str()
                                    } else {
                                        v.title.as_str()
                                    };
                                    let vol_time = if v.issued_ms > 0 {
                                        format_list_time(v.issued_ms)
                                    } else {
                                        v.time_text.clone()
                                    };
                                    if volcano_list_row(
                                        ui,
                                        p,
                                        &place,
                                        &vol_time,
                                        desc,
                                        selected,
                                        show_volcano_icons,
                                        egui::Id::new(("vol", i)),
                                        z,
                                    ) {
                                        clicked = Some(SideAction::List(SidebarTab::Volcano, i));
                                    }
                                }
                            }
                            SidebarTab::Weather => {
                                if snap.weather.is_empty() {
                                    empty_hint(
                                        ui,
                                        p,
                                        crate::i18n::t("暂无气象预警"),
                                        crate::i18n::t("启用气象源并登录 Jian 令牌后，中央台预警会出现在此列表。"),
                                        z,
                                    );
                                }
                                weather_icons.poll(ui.ctx());
                                for (i, w) in snap.weather.iter().enumerate() {
                                    let selected = sel
                                        == Some(ListSelection {
                                            tab: SidebarTab::Weather,
                                            index: i,
                                        });
                                    let icon = weather_icons.get(ui.ctx(), &w.icon_type);
                                    if weather_list_row(
                                        ui,
                                        p,
                                        w,
                                        icon.as_ref(),
                                        selected,
                                        egui::Id::new(("wx", i)),
                                        z,
                                    ) {
                                        clicked = Some(SideAction::List(SidebarTab::Weather, i));
                                    }
                                }
                            }
                    }
                });
        });
    clicked
}

fn empty_hint(ui: &mut Ui, p: &ThemePalette, title: &str, detail: &str, z: f32) {
    ui.add_space(20.0 * z);
    Frame::NONE
        .fill(p.chip_bg)
        .stroke(Stroke::new(1.0_f32 * z, p.panel_border))
        .inner_margin(margin_xy(12.0 * z, 14.0 * z))
        .corner_radius(10.0 * z)
        .show(ui, |ui| {
            ui.label(RichText::new(title).color(p.text_muted).strong().size(14.0 * z));
            ui.add_space(4.0 * z);
            ui.label(RichText::new(detail).color(p.text_dim).size(12.5 * z));
        });
}

fn draw_side_tabs(ui: &mut Ui, p: &ThemePalette, cur: &mut SidebarTab, z: f32) {
    ui.horizontal(|ui| {
        for (tab, label) in [
            (SidebarTab::Records, crate::i18n::t("速报")),
            (SidebarTab::Eew, crate::i18n::t("预警")),
            (SidebarTab::Volcano, crate::i18n::t("火山")),
            (SidebarTab::Weather, crate::i18n::t("气象")),
        ] {
            let selected = *cur == tab;
            let text = if selected {
                RichText::new(label).strong().size(13.5 * z).color(p.text)
            } else {
                RichText::new(label).size(13.5 * z).color(p.text_dim)
            };
            let resp = ui.add(
                egui::Button::new(text)
                    .fill(if selected {
                        p.accent_soft
                    } else {
                        Color32::TRANSPARENT
                    })
                    .stroke(Stroke::NONE)
                    .corner_radius(6.0 * z)
                    .min_size(Vec2::new(52.0 * z, 28.0 * z)),
            );
            if selected {
                let r = resp.rect;
                ui.painter().hline(
                    r.left() + 8.0 * z..=r.right() - 8.0 * z,
                    r.bottom() - 1.0 * z,
                    Stroke::new(2.0_f32 * z, p.accent),
                );
            }
            if resp.clicked() {
                *cur = tab;
            }
        }
    });
}

/// 火山列表首行：优先完整标题；若 name/title 分立则拼成「名　类型」。
fn volcano_list_headline(name: &str, title: &str) -> String {
    let name = name.trim();
    let title = title.trim();
    if name.is_empty() {
        return title.to_string();
    }
    if title.is_empty() {
        return name.to_string();
    }
    if title == name || title.starts_with(name) || name.starts_with(title) || name.contains(title)
    {
        if title.len() >= name.len() {
            title.to_string()
        } else {
            name.to_string()
        }
    } else {
        format!("{name}\u{3000}{title}")
    }
}

/// 气象侧栏行：左图标，右三行 headline / 省市区 / description。
fn weather_list_row(
    ui: &mut Ui,
    p: &ThemePalette,
    alert: &WeatherAlert,
    icon: Option<&egui::TextureHandle>,
    selected: bool,
    id: egui::Id,
    z: f32,
) -> bool {
    let mut clicked = false;
    let bg = if selected {
        p.row_selected
    } else {
        Color32::TRANSPARENT
    };
    let headline = weather_headline(alert);
    let region = weather_region_line(alert);
    let desc = weather_desc_preview(alert.description.trim());
    let icon_sz = Vec2::splat(42.0 * z);
    let level = weather_level_color(&alert.icon_type);

    let resp = Frame::NONE
        .fill(bg)
        .inner_margin(margin_xy(8.0 * z, 7.0 * z))
        .corner_radius(8.0 * z)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                // 左：中央台图标；未就绪时用等级色块占位
                let (icon_r, painter) = ui.allocate_painter(icon_sz, Sense::hover());
                if let Some(tex) = icon {
                    painter.image(
                        tex.id(),
                        icon_r.rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    painter.rect_filled(icon_r.rect, 6.0 * z, level);
                    painter.rect_stroke(
                        icon_r.rect,
                        6.0 * z,
                        Stroke::new(1.0_f32 * z, p.panel_border),
                        egui::StrokeKind::Inside,
                    );
                }

                ui.add_space(8.0 * z);
                ui.vertical(|ui| {
                    let max_w = (ui.available_width() - 4.0 * z).max(40.0 * z);
                    ui.set_max_width(max_w);
                    if !headline.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&headline).strong().size(13.5 * z).color(p.text),
                            )
                            .truncate(),
                        );
                    }
                    if !region.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&region).size(12.5 * z).color(p.text_dim),
                            )
                            .truncate(),
                        );
                    }
                    if !desc.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&desc).size(12.0 * z).color(p.text_muted),
                            )
                            .truncate(),
                        );
                    }
                });
            });
        })
        .response;

    let full = ui
        .interact(resp.rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if full.hovered() && !selected {
        ui.painter().rect_stroke(
            resp.rect,
            8.0 * z,
            Stroke::new(1.0_f32 * z, p.panel_border),
            egui::StrokeKind::Outside,
        );
    }
    if full.clicked() {
        clicked = true;
    }
    ui.add_space(3.0 * z);
    clicked
}

fn weather_headline(alert: &WeatherAlert) -> String {
    let h = alert.headline.trim();
    if !h.is_empty() {
        return h.to_string();
    }
    alert.title.trim().to_string()
}

fn weather_region_line(alert: &WeatherAlert) -> String {
    [
        alert.province.trim(),
        alert.city.trim(),
        alert.district.trim(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\u{ff0c}")
}

fn weather_desc_preview(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    // 侧栏单行预览，过长截断
    const MAX_CHARS: usize = 72;
    let count = t.chars().count();
    if count <= MAX_CHARS {
        t.to_string()
    } else {
        let mut s: String = t.chars().take(MAX_CHARS).collect();
        s.push('\u{2026}');
        s
    }
}

fn volcano_list_row(
    ui: &mut Ui,
    p: &ThemePalette,
    place: &str,
    sub: &str,
    title: &str,
    selected: bool,
    show_icon: bool,
    id: egui::Id,
    z: f32,
) -> bool {
    let mut clicked = false;
    let bg = if selected {
        p.row_selected
    } else {
        Color32::TRANSPARENT
    };
    let accent = Color32::from_rgb(0xF9, 0x73, 0x16);

    let resp = Frame::NONE
        .fill(bg)
        .inner_margin(margin_xy(8.0 * z, 7.0 * z))
        .corner_radius(8.0 * z)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (stripe, painter) =
                    ui.allocate_painter(Vec2::new(4.0 * z, 50.0 * z), Sense::hover());
                painter.rect_filled(stripe.rect, 2.0 * z, accent);

                ui.add_space(8.0 * z);
                if show_icon {
                    ui.label(
                        RichText::new("\u{25b2}")
                            .color(accent)
                            .size(16.0 * z)
                            .strong(),
                    );
                    ui.add_space(4.0 * z);
                }
                ui.vertical(|ui| {
                    let max_w = (ui.available_width() - 4.0 * z).max(40.0 * z);
                    ui.set_max_width(max_w);
                    ui.add(
                        egui::Label::new(RichText::new(place).color(p.text).size(15.0 * z))
                            .truncate(),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(sub).color(p.text_dim).size(13.0 * z))
                            .truncate(),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(title).color(p.text_muted).size(12.5 * z))
                            .wrap(),
                    );
                });
            });
        })
        .response;

    let full = ui
        .interact(resp.rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if full.hovered() && !selected {
        ui.painter().rect_stroke(
            resp.rect,
            8.0 * z,
            Stroke::new(1.0_f32 * z, p.panel_border),
            egui::StrokeKind::Outside,
        );
    }
    if full.clicked() {
        clicked = true;
    }
    ui.add_space(3.0 * z);
    clicked
}

fn list_row(
    ui: &mut Ui,
    p: &ThemePalette,
    icons: &IntensityIcons,
    place: &str,
    sub: &str,
    mag: &str,
    kind: IntensityKind,
    level: u8,
    intensity_text: &str,
    scale: &str,
    accent: Color32,
    selected: bool,
    id: egui::Id,
    _z: f32,
) -> RowClick {
    let bg = if selected {
        p.row_selected
    } else {
        Color32::TRANSPARENT
    };

    // 对齐 eewcn EarthquakeListEntryForm：烈度块固定约 40px，地名随宽换行（ExpandHeight）
    let chip = 40.0_f32;
    let resp = Frame::NONE
        .fill(bg)
        .inner_margin(Margin::symmetric(8, 6))
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (stripe, painter) =
                    ui.allocate_painter(Vec2::new(3.0, chip), Sense::hover());
                painter.rect_filled(stripe.rect, 1.5, accent);
                ui.add_space(6.0);

                // 先占烈度块（右），剩余宽度给文字——等同 eewcn ExpandHeight 扣掉 intensity 宽
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    paint_intensity(
                        ui,
                        icons,
                        kind,
                        level,
                        intensity_text,
                        scale,
                        chip,
                        false,
                    );
                    ui.add_space(6.0);
                    let text_w = ui.available_width().max(48.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(text_w, ui.available_height().max(chip)),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.set_max_width(text_w);
                            // 地名：窄侧栏换行增高（eewcn labelLocation wordWrap）
                            ui.add(
                                egui::Label::new(RichText::new(place).color(p.text).size(14.0))
                                    .wrap(),
                            );
                            ui.add(
                                egui::Label::new(RichText::new(sub).color(p.text_dim).size(12.0))
                                    .wrap(),
                            );
                            if !mag.is_empty() {
                                ui.label(RichText::new(mag).color(p.text_muted).size(12.0));
                            }
                        },
                    );
                });
            });
        })
        .response;

    let full = ui
        .interact(resp.rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if full.hovered() && !selected {
        ui.painter().rect_stroke(
            resp.rect,
            6.0,
            Stroke::new(1.0_f32, p.panel_border),
            egui::StrokeKind::Outside,
        );
    }
    ui.add_space(2.0);
    if full.clicked() {
        RowClick::Primary
    } else {
        RowClick::None
    }
}
