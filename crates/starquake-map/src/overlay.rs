//! 震中点、测站点与 P/S 波阵面叠加。

use crate::globe;
use crate::marker_icons::MarkerIcons;
use crate::projection::{self, ViewCtx};
use egui::epaint::{Mesh, Vertex};
use egui::{Color32, Pos2, Rect, Stroke, TextureHandle, Ui, Vec2};
use starquake_core::palette::color_for;
use starquake_core::{
    cn_intensity_round, CencIntEvent, EewReport, EqRecord, StationSample, VolcanoNotice,
    WeatherAlert, LocalHypoEstimate,
};
use starquake_travel::{max_s_wave_spread_km, TravelEngine, Wave};
use std::sync::Once;

const MARKER_RED: Color32 = Color32::from_rgb(220, 40, 40);
const HOME_FALLBACK: Color32 = Color32::from_rgb(80, 220, 80);

/// 预警/速报 SVG 边长，对齐 eewcn：`(3+1.5*exp(0.25*intensity))*5`
/// `intensity` 优先用震度/烈度档；未知时用震级近似。
fn eewcn_mark_size(intensity: f64) -> f32 {
    let v = intensity.max(0.0);
    ((3.0 + 1.5 * (0.25 * v).exp()) * 5.0).clamp(16.0, 72.0) as f32
}

fn mark_size_from_event(intensity_level: u8, magnitude: f64) -> f32 {
    let intensity = if intensity_level > 0 {
        intensity_level as f64
    } else if magnitude.is_finite() && magnitude > 0.0 {
        magnitude
    } else {
        3.0
    };
    eewcn_mark_size(intensity)
}

/// 平面：轴对齐 billboard；球体：贴面方块（对齐 GQ `createSquare` + `project3D`）。
/// 球缘角点穿地平线时不回退 billboard（否则会再次「立起」），改画小圆点。
fn paint_tex(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    tex: &TextureHandle,
    p: Pos2,
    size: f32,
    tint: Color32,
) {
    if view.mode.is_globe() {
        if let Some([tl, tr, br, bl]) = view.project_surface_square(lon, lat, size) {
            let mut mesh = Mesh::with_texture(tex.id());
            mesh.vertices.extend_from_slice(&[
                Vertex {
                    pos: tl,
                    uv: Pos2::new(0.0, 0.0),
                    color: tint,
                },
                Vertex {
                    pos: tr,
                    uv: Pos2::new(1.0, 0.0),
                    color: tint,
                },
                Vertex {
                    pos: br,
                    uv: Pos2::new(1.0, 1.0),
                    color: tint,
                },
                Vertex {
                    pos: bl,
                    uv: Pos2::new(0.0, 1.0),
                    color: tint,
                },
            ]);
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            painter.add(egui::Shape::mesh(mesh));
        }
        // 角点穿地平线：不回退 billboard（会立起），与 GQ shouldDraw=false 一致
        return;
    }
    let rect = Rect::from_center_size(p, Vec2::splat(size));
    painter.image(
        tex.id(),
        rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        tint,
    );
}

fn paint_red_x_fallback(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    p: Pos2,
    half_px: f32,
) {
    let white = Stroke::new(half_px * 0.55 + 1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 200));
    let red = Stroke::new(half_px * 0.45 + 1.6, MARKER_RED);
    // 球体：大圆十字（对齐 GQ `createCross`）；平面：屏幕对角叉
    if view.mode.is_globe() {
        if let Some(r_km) = view.px_to_km(half_px * 2.0) {
            let mut arms: Vec<(Pos2, Pos2)> = Vec::with_capacity(2);
            for br0 in [45.0_f64, 135.0] {
                let (a_lon, a_lat) = globe::move_on_globe(lon, lat, r_km, br0);
                let (b_lon, b_lat) = globe::move_on_globe(lon, lat, r_km, br0 + 180.0);
                if let (Some(a), Some(b)) = (view.project(a_lon, a_lat), view.project(b_lon, b_lat)) {
                    arms.push((a, b));
                }
            }
            if arms.len() == 2 {
                for &(a, b) in &arms {
                    painter.line_segment([a, b], white);
                }
                for &(a, b) in &arms {
                    painter.line_segment([a, b], red);
                }
                return;
            }
        }
    }
    let a = p + Vec2::new(-half_px, -half_px);
    let b = p + Vec2::new(half_px, half_px);
    let c = p + Vec2::new(half_px, -half_px);
    let d = p + Vec2::new(-half_px, half_px);
    painter.line_segment([a, b], white);
    painter.line_segment([c, d], white);
    painter.line_segment([a, b], red);
    painter.line_segment([c, d], red);
}

fn paint_red_dot_fallback(painter: &egui::Painter, p: Pos2, r: f32, selected: bool) {
    painter.circle_filled(p, r, MARKER_RED);
    painter.circle_stroke(
        p,
        r,
        Stroke::new(
            if selected { 1.6_f32 } else { 1.0_f32 },
            if selected {
                Color32::WHITE
            } else {
                Color32::from_rgba_unmultiplied(255, 255, 255, 160)
            },
        ),
    );
}

/// 预警震中叉标闪烁相位（对齐 eewcn `eewMarksTimer` 500ms）。
pub fn eew_mark_blink_on(now_ms: i64) -> bool {
    if now_ms <= 0 {
        return true;
    }
    (now_ms / 500) % 2 == 0
}

fn paint_eew_marker(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    icons: Option<&MarkerIcons>,
    p: Pos2,
    intensity_level: u8,
    magnitude: f64,
) {
    let size = mark_size_from_event(intensity_level, magnitude);
    if let Some(tex) = icons.and_then(|i| i.cross()) {
        paint_tex(painter, view, lon, lat, tex, p, size, Color32::WHITE);
    } else {
        paint_red_x_fallback(painter, view, lon, lat, p, size * 0.28);
    }
}

fn paint_record_marker(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    icons: Option<&MarkerIcons>,
    p: Pos2,
    intensity_level: u8,
    magnitude: f64,
    selected: bool,
) {
    let size = mark_size_from_event(intensity_level, magnitude) * if selected { 1.12 } else { 1.0 };
    if let Some(tex) = icons.and_then(|i| i.red()) {
        let tint = if selected {
            Color32::WHITE
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 220)
        };
        paint_tex(painter, view, lon, lat, tex, p, size, tint);
    } else {
        paint_red_dot_fallback(painter, p, size * 0.35, selected);
    }
}

fn paint_home_marker(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    icons: Option<&MarkerIcons>,
    p: Pos2,
) {
    if let Some(tex) = icons.and_then(|i| i.green()) {
        paint_tex(painter, view, lon, lat, tex, p, 24.0, Color32::WHITE);
    } else {
        painter.circle_stroke(p, 7.0, Stroke::new(3.0_f32, Color32::WHITE));
        painter.circle_stroke(p, 7.0, Stroke::new(1.8_f32, HOME_FALLBACK));
    }
}

fn is_record_event(ev: &EewReport) -> bool {
    ev.event_id.starts_with("record:")
}

fn paint_epicenter_marker(
    painter: &egui::Painter,
    view: &ViewCtx,
    icons: Option<&MarkerIcons>,
    p: Pos2,
    ev: &EewReport,
    with_label: bool,
    show_cross: bool,
) {
    if is_record_event(ev) {
        paint_record_marker(
            painter,
            view,
            ev.longitude,
            ev.latitude,
            icons,
            p,
            ev.intensity_level,
            ev.magnitude,
            true,
        );
    } else if show_cross {
        // 有波圈时按相位显隐，实现红色叉标闪烁
        paint_eew_marker(
            painter,
            view,
            ev.longitude,
            ev.latitude,
            icons,
            p,
            ev.intensity_level,
            ev.magnitude,
        );
    }
    if with_label && show_cross {
        painter.text(
            p + Vec2::new(12.0, -20.0),
            egui::Align2::LEFT_BOTTOM,
            format!("M{:.1} {}", ev.magnitude, ev.place),
            egui::FontId::proportional(13.0),
            Color32::WHITE,
        );
    }
}


fn felt_level(s: &StationSample) -> u8 {
    match s.intensity_kind {
        starquake_core::IntensityKind::JmaShindo => {
            if s.intensity_level >= 1 {
                return s.intensity_level.min(9);
            }
            if s.instrumental >= 0.5 {
                return starquake_core::jma_from_instrumental(s.instrumental).0;
            }
            0
        }
        starquake_core::IntensityKind::CnIntensity => {
            if s.intensity_level >= 1 {
                s.intensity_level.min(12)
            } else {
                0
            }
        }
    }
}

/// 测站散点：震度 >=1 用 `shindo_inst`；其余計測色点。有预警时有感点始终显示。
/// 平面拖图时复用静默测站 mesh，避免每帧重投上千点导致「拖时消失、松手才回来」。
#[derive(Default)]
pub struct StationPaintCache {
    data_fp: u64,
    mesh_lon: f64,
    mesh_lat: f64,
    build_zoom: f64,
    mesh: Option<std::sync::Arc<Mesh>>,
}

pub fn paint_stations(
    ui: &mut Ui,
    view: &ViewCtx,
    stations: &[StationSample],
    selected_id: Option<&str>,
    icons: Option<&MarkerIcons>,
    eew_active: bool,
    quality: crate::PaintQuality,
    cache: &mut StationPaintCache,
) {
    if stations.is_empty() {
        cache.mesh = None;
        return;
    }
    let rect = view.rect;
    let painter = ui.painter_at(rect);
    let pad = 16.0_f32;
    let _ = eew_active;
    // Qt MapQuickItem 的 width 按屏像素量；egui 用点，须 / ppp 才与 eewcn 观感一致
    let ppp = ui.ctx().pixels_per_point().max(1.0);
    // 低缩放用地理网格抽稀；放宽格距与上限，避免缩小时大片消失
    let (grid_deg, quiet_cap) = match quality {
        crate::PaintQuality::Eco => {
            let g = if view.zoom < 3.5 {
                0.55_f64
            } else if view.zoom < 4.5 {
                0.35
            } else if view.zoom < 5.5 {
                0.20
            } else {
                0.0
            };
            (g, 900usize)
        }
        crate::PaintQuality::Normal => {
            let g = if view.zoom < 3.2 {
                0.40_f64
            } else if view.zoom < 4.2 {
                0.25
            } else if view.zoom < 5.2 {
                0.14
            } else {
                0.0
            };
            (g, 2200)
        }
        crate::PaintQuality::Extreme => {
            let g = if view.zoom < 2.8 { 0.22_f64 } else { 0.0 };
            (g, 5000)
        }
    };

    let data_fp = station_quiet_fp(stations, selected_id, quality, view, grid_deg, quiet_cap, ppp);
    let flat = !view.mode.is_globe();
    let can_shift = flat
        && cache.mesh.is_some()
        && cache.data_fp == data_fp
        && (cache.build_zoom - view.zoom).abs() < 1e-9;

    let mut felt_idx: Vec<usize> = Vec::new();
    let mut selected_quiet: Vec<usize> = Vec::new();
    for (idx, s) in stations.iter().enumerate() {
        let felt = match s.intensity_kind {
            starquake_core::IntensityKind::JmaShindo => {
                s.intensity_level >= 1 || s.instrumental >= 0.5
            }
            starquake_core::IntensityKind::CnIntensity => s.intensity_level >= 1,
        };
        if felt {
            felt_idx.push(idx);
        } else if selected_id == Some(s.id.as_str()) {
            selected_quiet.push(idx);
        }
    }

    if can_shift {
        if let Some(mesh) = cache.mesh.as_mut() {
            if let Some((dx, dy)) = station_flat_pan_delta(cache.mesh_lon, cache.mesh_lat, view) {
                if dx.abs() > 0.05 || dy.abs() > 0.05 {
                    let m = std::sync::Arc::make_mut(mesh);
                    m.translate(Vec2::new(dx, dy));
                    cache.mesh_lon = view.center_lon;
                    cache.mesh_lat = view.center_lat;
                }
            } else {
                // 跨日界等：放弃平移，强制重建
                cache.mesh = None;
            }
        }
    }

    if let Some(mesh) = cache.mesh.as_ref().filter(|_| {
        cache.data_fp == data_fp && (cache.build_zoom - view.zoom).abs() < 1e-9 && flat
    }) {
        painter.add(egui::Shape::Mesh(mesh.clone()));
    } else if flat {
        let mesh = build_quiet_station_mesh(
            view,
            stations,
            selected_id,
            grid_deg,
            quiet_cap,
            pad,
            ppp,
        );
        painter.add(egui::Shape::Mesh(mesh.clone()));
        cache.data_fp = data_fp;
        cache.mesh_lon = view.center_lon;
        cache.mesh_lat = view.center_lat;
        cache.build_zoom = view.zoom;
        cache.mesh = Some(mesh);
    } else {
        // 球体：逐点投影（背面剔除），不缓存平移
        cache.mesh = None;
        let mut quiet_drawn = 0usize;
        let mut occupied: std::collections::HashSet<(i32, i32)> =
            std::collections::HashSet::new();
        for s in stations {
            let felt = match s.intensity_kind {
                starquake_core::IntensityKind::JmaShindo => {
                    s.intensity_level >= 1 || s.instrumental >= 0.5
                }
                starquake_core::IntensityKind::CnIntensity => s.intensity_level >= 1,
            };
            if felt {
                continue;
            }
            let selected = selected_id == Some(s.id.as_str());
            let Some(p) = view.project(s.longitude, s.latitude) else {
                continue;
            };
            if p.x < rect.left() - pad
                || p.x > rect.right() + pad
                || p.y < rect.top() - pad
                || p.y > rect.bottom() + pad
            {
                continue;
            }
            if !selected {
                if quiet_drawn >= quiet_cap {
                    continue;
                }
                if grid_deg > 0.0 {
                    let gx = (s.longitude / grid_deg).floor() as i32;
                    let gy = (s.latitude / grid_deg).floor() as i32;
                    if !occupied.insert((gx, gy)) {
                        continue;
                    }
                }
                quiet_drawn += 1;
            }
            paint_station_dot(&painter, view, icons, s, p, selected, true, ppp);
        }
    }

    for idx in selected_quiet {
        let s = &stations[idx];
        let Some(p) = view.project(s.longitude, s.latitude) else {
            continue;
        };
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        paint_station_dot(&painter, view, icons, s, p, true, true, ppp);
    }

    felt_idx.sort_by(|&a, &b| {
        let sa = &stations[a];
        let sb = &stations[b];
        let la = felt_level(sa);
        let lb = felt_level(sb);
        la.cmp(&lb).then_with(|| {
            sa.instrumental
                .partial_cmp(&sb.instrumental)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    for idx in felt_idx {
        let s = &stations[idx];
        let Some(p) = view.project(s.longitude, s.latitude) else {
            continue;
        };
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        let selected = selected_id == Some(s.id.as_str());
        paint_station_dot(&painter, view, icons, s, p, selected, false, ppp);
    }
}

fn station_quiet_fp(
    stations: &[StationSample],
    selected_id: Option<&str>,
    quality: crate::PaintQuality,
    view: &ViewCtx,
    grid_deg: f64,
    quiet_cap: usize,
    ppp: f32,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    stations.len().hash(&mut h);
    quality.hash(&mut h);
    view.zoom.to_bits().hash(&mut h);
    (view.rect.width() as i32).hash(&mut h);
    (view.rect.height() as i32).hash(&mut h);
    grid_deg.to_bits().hash(&mut h);
    quiet_cap.hash(&mut h);
    selected_id.hash(&mut h);
    ((ppp * 100.0).round() as i32).hash(&mut h);
    std::mem::discriminant(&view.mode).hash(&mut h);
    for s in stations {
        s.id.hash(&mut h);
        s.instrumental.to_bits().hash(&mut h);
        s.intensity_level.hash(&mut h);
        std::mem::discriminant(&s.intensity_kind).hash(&mut h);
    }
    h.finish()
}

fn station_flat_pan_delta(build_lon: f64, build_lat: f64, view: &ViewCtx) -> Option<(f32, f32)> {
    let build_lon = projection::wrap_lon_near(build_lon, view.center_lon);
    let (x0, y0) = projection::lonlat_to_world(build_lon, build_lat, view.zoom);
    let (x1, y1) =
        projection::lonlat_to_world(view.center_lon, view.center_lat, view.zoom);
    let dx = (x0 - x1) as f32;
    let dy = (y0 - y1) as f32;
    let max = (projection::world_size(view.zoom) as f32) * 0.45;
    if !dx.is_finite() || !dy.is_finite() || dx.abs() > max || dy.abs() > max {
        return None;
    }
    Some((dx, dy))
}

fn build_quiet_station_mesh(
    view: &ViewCtx,
    stations: &[StationSample],
    selected_id: Option<&str>,
    grid_deg: f64,
    quiet_cap: usize,
    pad: f32,
    ppp: f32,
) -> std::sync::Arc<Mesh> {
    let rect = view.rect;
    let mut mesh = Mesh::default();
    let mut quiet_drawn = 0usize;
    let mut occupied: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
    for s in stations {
        let felt = match s.intensity_kind {
            starquake_core::IntensityKind::JmaShindo => {
                s.intensity_level >= 1 || s.instrumental >= 0.5
            }
            starquake_core::IntensityKind::CnIntensity => s.intensity_level >= 1,
        };
        if felt || selected_id == Some(s.id.as_str()) {
            continue;
        }
        let Some(p) = view.project(s.longitude, s.latitude) else {
            continue;
        };
        // 先视口裁剪，再占网格：避免屏外点抢格导致拖图时「空洞/整片消失」
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        if quiet_drawn >= quiet_cap {
            continue;
        }
        if grid_deg > 0.0 {
            let gx = (s.longitude / grid_deg).floor() as i32;
            let gy = (s.latitude / grid_deg).floor() as i32;
            if !occupied.insert((gx, gy)) {
                continue;
            }
        }
        quiet_drawn += 1;
        let rgb = match s.intensity_kind {
            starquake_core::IntensityKind::CnIntensity => starquake_core::palette::Rgb::new(0x66, 0x66, 0x66),
            starquake_core::IntensityKind::JmaShindo => {
                if s.instrumental >= -3.0 {
                    starquake_core::instrumental_rgb(s.instrumental)
                } else {
                    color_for(s.intensity_kind, s.intensity_level)
                }
            }
        };
        let fill = Color32::from_rgb(rgb.r, rgb.g, rgb.b);
        append_filled_circle(&mut mesh, p, station_dot_radius(view.zoom, false, ppp), fill);
    }
    std::sync::Arc::new(mesh)
}

/// eewcn 公式 `5+max(0,zoom-8)*5` 为参考；egui 实心圆+抗锯齿观感偏大，再乘缩放并 `/ppp`。
fn station_dot_diameter_pts(zoom: f64, ppp: f32) -> f32 {
    const VISUAL_SCALE: f64 = 0.45;
    let px = (5.0 + (zoom - 8.0).max(0.0) * 5.0) * VISUAL_SCALE;
    let ppp = f64::from(ppp).max(1.0);
    (px / ppp).clamp(1.2, 14.0) as f32
}

fn station_dot_radius(zoom: f64, selected: bool, ppp: f32) -> f32 {
    let r = station_dot_diameter_pts(zoom, ppp) * 0.5;
    if selected {
        r + 1.0 / ppp.max(1.0)
    } else {
        r
    }
}

/// 有感震度/烈度图标边长：相对 eewcn 点径放大。
fn station_icon_size(zoom: f64, base: f32, selected: bool, ppp: f32) -> f32 {
    let diam = station_dot_diameter_pts(zoom, ppp);
    let s = diam * (base / 5.0).clamp(1.8, 4.0);
    if selected {
        s + 4.0 / ppp.max(1.0)
    } else {
        s
    }
}

fn append_filled_circle(mesh: &mut Mesh, center: Pos2, radius: f32, color: Color32) {
    const N: usize = 8;
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex {
        pos: center,
        uv: Pos2::ZERO,
        color,
    });
    for i in 0..N {
        let a = (i as f32) * std::f32::consts::TAU / (N as f32);
        mesh.vertices.push(Vertex {
            pos: center + Vec2::new(a.cos(), a.sin()) * radius,
            uv: Pos2::ZERO,
            color,
        });
    }
    for i in 0..N {
        let i1 = base + 1 + i as u32;
        let i2 = base + 1 + ((i + 1) % N) as u32;
        mesh.indices.extend_from_slice(&[base, i1, i2]);
    }
}

fn paint_station_dot(
    painter: &egui::Painter,
    view: &ViewCtx,
    icons: Option<&MarkerIcons>,
    s: &StationSample,
    p: Pos2,
    selected: bool,
    quiet: bool,
    ppp: f32,
) {
    let jma_lv = if s.intensity_kind == starquake_core::IntensityKind::JmaShindo {
        if s.intensity_level >= 1 {
            s.intensity_level.min(9)
        } else if s.instrumental >= 0.5 {
            starquake_core::jma_from_instrumental(s.instrumental).0
        } else {
            0
        }
    } else {
        0
    };
    if jma_lv >= 1 {
        if let Some(tex) = icons.and_then(|i| i.shindo_inst(jma_lv)) {
            let base = match jma_lv {
                1 => 12.0,
                2 => 14.0,
                3 | 4 => 16.0,
                5 | 6 => 18.0,
                _ => 20.0,
            };
            let size = station_icon_size(view.zoom, base, selected, ppp);
            paint_tex(
                painter,
                view,
                s.longitude,
                s.latitude,
                tex,
                p,
                size,
                Color32::WHITE,
            );
            if selected {
                painter.text(
                    p + Vec2::new(size * 0.55, -size * 0.45),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{} {}", s.name, starquake_core::jma_shindo_text(jma_lv)),
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
            return;
        }
    }
    let is_kma = s.id.starts_with("kma:");
    if !is_kma
        && s.intensity_kind == starquake_core::IntensityKind::CnIntensity
        && s.intensity_level >= 1
    {
        let cn_lv = s.intensity_level.clamp(1, 12);
        if let Some(tex) = icons.and_then(|i| i.csis_inst(cn_lv)) {
            let base = match cn_lv {
                1..=3 => 12.0,
                4..=6 => 15.0,
                7..=9 => 18.0,
                _ => 20.0,
            };
            let size = station_icon_size(view.zoom, base, selected, ppp);
            paint_tex(
                painter,
                view,
                s.longitude,
                s.latitude,
                tex,
                p,
                size,
                Color32::WHITE,
            );
            if selected {
                painter.text(
                    p + Vec2::new(size * 0.55, -size * 0.45),
                    egui::Align2::LEFT_BOTTOM,
                    format!("{} {}", s.name, s.intensity_text),
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
            return;
        }
    }
    let felt = !quiet;
    let rgb = match s.intensity_kind {
        starquake_core::IntensityKind::CnIntensity => {
            if s.intensity_level >= 1 {
                color_for(s.intensity_kind, s.intensity_level)
            } else {
                starquake_core::palette::Rgb::new(0x66, 0x66, 0x66)
            }
        }
        starquake_core::IntensityKind::JmaShindo => {
            if s.instrumental >= -3.0 {
                starquake_core::instrumental_rgb(s.instrumental)
            } else {
                color_for(s.intensity_kind, s.intensity_level)
            }
        }
    };
    let fill = Color32::from_rgb(rgb.r, rgb.g, rgb.b);
    // 静默点：eewcn 屏像素换算；有感色点略放大
    let mut r = station_dot_radius(view.zoom, selected, ppp);
    if felt && !quiet {
        let bump_px = match s.intensity_level {
            0 => 0.0,
            1..=3 => 1.0,
            4..=6 => 2.0,
            _ => 3.0,
        };
        r += bump_px / ppp.max(1.0);
    }
    painter.circle_filled(p, r, fill);
    if selected {
        painter.circle_stroke(p, r, Stroke::new(1.5_f32, Color32::WHITE));
        painter.text(
            p + Vec2::new(8.0, -12.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{} {}", s.name, s.intensity_text),
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
}



/// 气象预警：有经纬时画色点（等级色取自图标码末三位）。

/// Local station hypo: purple cross (vs official red X).
pub fn paint_local_hypo(ui: &mut Ui, view: &ViewCtx, hypo: &LocalHypoEstimate) {
    let Some(p) = view.project(hypo.longitude, hypo.latitude) else {
        return;
    };
    let rect = view.rect;
    if p.x < rect.left() - 20.0
        || p.x > rect.right() + 20.0
        || p.y < rect.top() - 20.0
        || p.y > rect.bottom() + 20.0
    {
        return;
    }
    let painter = ui.painter_at(rect);
    let half = 10.0_f32;
    let purple = Color32::from_rgb(0x9B, 0x59, 0xB6);
    let white = Stroke::new(3.2_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 210));
    let ink = Stroke::new(2.0_f32, purple);
    let a = p + Vec2::new(-half, -half);
    let b = p + Vec2::new(half, half);
    let c = p + Vec2::new(half, -half);
    let d = p + Vec2::new(-half, half);
    painter.line_segment([a, b], white);
    painter.line_segment([c, d], white);
    painter.line_segment([a, b], ink);
    painter.line_segment([c, d], ink);
    painter.circle_stroke(p, 5.0, Stroke::new(1.5_f32, purple));
    let label = format!("{} {:.1}km  n={}", "推算", hypo.depth_km, hypo.used_stations);
    painter.text(
        p + Vec2::new(12.0, -8.0),
        egui::Align2::LEFT_BOTTOM,
        label,
        egui::FontId::proportional(12.0),
        purple,
    );
}

pub fn paint_weather_alerts(
    ui: &mut Ui,
    view: &ViewCtx,
    alerts: &[WeatherAlert],
    selected_idx: Option<usize>,
) {
    if alerts.is_empty() {
        return;
    }
    let rect = view.rect;
    let painter = ui.painter_at(rect);
    let pad = 20.0_f32;
    for (i, a) in alerts.iter().enumerate() {
        let (Some(lon), Some(lat)) = (a.longitude, a.latitude) else {
            continue;
        };
        if lon.abs() <= 0.01 && lat.abs() <= 0.01 {
            continue;
        }
        let Some(p) = view.project(lon, lat) else {
            continue;
        };
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        let selected = selected_idx == Some(i);
        let fill = weather_level_color(&a.icon_type);
        let r = if selected { 7.0_f32 } else { 5.0_f32 };
        painter.circle_filled(p, r, fill);
        painter.circle_stroke(
            p,
            r,
            Stroke::new(if selected { 2.0_f32 } else { 1.0_f32 }, Color32::WHITE),
        );
        if selected && !a.title.is_empty() {
            painter.text(
                p + Vec2::new(8.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                &a.title,
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
    }
}

fn weather_level_color(icon_type: &str) -> Color32 {
    // p00TTLLL：末三位 001红 / 002橙 / 003黄 / 004蓝
    let digits: String = icon_type
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    let code = if digits.len() >= 3 {
        &digits[digits.len() - 3..]
    } else {
        ""
    };
    match code {
        "001" => Color32::from_rgb(0xE6, 0x00, 0x12),
        "002" => Color32::from_rgb(0xFA, 0x8C, 0x16),
        "003" => Color32::from_rgb(0xFA, 0xDB, 0x14),
        "004" => Color32::from_rgb(0x18, 0x90, 0xFF),
        _ => Color32::from_rgb(0x8C, 0x8C, 0x8C),
    }
}

/// 烈度速报：震中 + 计测台站点（仅传入当前选中事件）。
pub fn paint_cenc_int_event(ui: &mut Ui, view: &ViewCtx, event: &CencIntEvent, icons: Option<&MarkerIcons>) {
    let rect = view.rect;
    let painter = ui.painter_at(rect);
    let pad = 20.0_f32;
    if let Some(epi) = view.project(event.longitude, event.latitude) {
        if epi.x >= rect.left() - pad
            && epi.x <= rect.right() + pad
            && epi.y >= rect.top() - pad
            && epi.y <= rect.bottom() + pad
        {
            if let Some(tex) = icons.and_then(|i| i.cross()) {
                let size = 22.0_f32;
                let r = Rect::from_center_size(epi, Vec2::splat(size));
                painter.image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            } else {
                painter.circle_filled(epi, 5.0, MARKER_RED);
                painter.circle_stroke(epi, 5.0, Stroke::new(1.5_f32, Color32::WHITE));
            }
        }
    }
    for s in &event.stations {
        let Some(p) = view.project(s.longitude, s.latitude) else {
            continue;
        };
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        let level = s.intensity.map(cn_intensity_round).unwrap_or(0);
        let rgb = if level >= 1 {
            starquake_core::palette::cn_intensity_rgb(level)
        } else {
            starquake_core::palette::Rgb::new(0x88, 0x88, 0x88)
        };
        let fill = Color32::from_rgb(rgb.r, rgb.g, rgb.b);
        let r = 3.2_f32;
        painter.circle_filled(p, r, fill);
    }
}

/// 火山：按经纬度画实心三角 ▲
pub fn paint_volcanoes(
    ui: &mut Ui,
    view: &ViewCtx,
    volcanoes: &[VolcanoNotice],
    selected_idx: Option<usize>,
) {
    if volcanoes.is_empty() {
        return;
    }
    let rect = view.rect;
    let painter = ui.painter_at(rect);
    let pad = 20.0_f32;
    let fill = Color32::from_rgb(0xF9, 0x73, 0x16);
    let fill_sel = Color32::from_rgb(0xFF, 0xB0, 0x40);
    for (i, v) in volcanoes.iter().enumerate() {
        if v.latitude.abs() <= 0.01 && v.longitude.abs() <= 0.01 {
            continue;
        }
        let Some(p) = view.project(v.longitude, v.latitude) else {
            continue;
        };
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        let selected = selected_idx == Some(i);
        let h = if selected { 14.0_f32 } else { 10.0_f32 };
        let half = h * 0.72;
        let pts = vec![
            egui::pos2(p.x, p.y - h),
            egui::pos2(p.x - half, p.y + h * 0.55),
            egui::pos2(p.x + half, p.y + h * 0.55),
        ];
        painter.add(egui::Shape::convex_polygon(
            pts,
            if selected { fill_sel } else { fill },
            Stroke::new(
                if selected { 1.6_f32 } else { 1.0_f32 },
                Color32::from_rgb(0x33, 0x1A, 0x00),
            ),
        ));
        if selected {
            painter.text(
                p + Vec2::new(h * 0.7, -h),
                egui::Align2::LEFT_BOTTOM,
                &v.name,
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
    }
}

/// 速报列表：`markers/red.svg`，尺寸随震级
pub fn paint_records(
    ui: &mut Ui,
    view: &ViewCtx,
    records: &[EqRecord],
    selected_place: Option<&str>,
    icons: Option<&MarkerIcons>,
) {
    if records.is_empty() {
        return;
    }
    let rect = view.rect;
    let painter = ui.painter_at(rect);
    let pad = 24.0_f32;
    for r in records.iter().rev() {
        if !r.has_map_coord() {
            continue;
        }
        let Some(p) = view.project(r.longitude, r.latitude) else {
            continue;
        };
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        let selected = selected_place == Some(r.place.as_str());
        paint_record_marker(
            &painter,
            view,
            r.longitude,
            r.latitude,
            icons,
            p,
            r.intensity_level,
            r.magnitude,
            selected,
        );
    }
}

/// 本机位置（`markers/green.svg` 蓝房子）
pub fn paint_home(
    ui: &mut Ui,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    icons: Option<&MarkerIcons>,
) {
    let Some(p) = view.project(lon, lat) else {
        return;
    };
    if !view.rect.expand(20.0).contains(p) {
        return;
    }
    let painter = ui.painter_at(view.rect);
    paint_home_marker(&painter, view, lon, lat, icons, p);
}

/// 仅震中标记（无波圈）
pub fn paint_marker_only(
    ui: &mut Ui,
    view: &ViewCtx,
    ev: &EewReport,
    icons: Option<&MarkerIcons>,
) {
    let Some(p) = view.project(ev.longitude, ev.latitude) else {
        return;
    };
    let painter = ui.painter_at(view.rect);
    paint_epicenter_marker(&painter, view, icons, p, ev, true, true);
}

fn stroke_polyline(painter: &egui::Painter, segs: &[Vec<Pos2>], stroke: Stroke) {
    for pts in segs {
        if pts.len() < 2 {
            continue;
        }
        painter.add(egui::Shape::line(pts.clone(), stroke));
    }
}

pub fn paint_wave_overlay(
    ui: &mut Ui,
    view: &ViewCtx,
    active: Option<&EewReport>,
    travel: Option<&TravelEngine>,
    elapsed_s: f64,
    icons: Option<&MarkerIcons>,
    selected: bool,
    show_cross: bool,
) {
    let Some(ev) = active else { return };
    let rect = view.rect;
    let painter = ui.painter_at(rect);

    let Some(epicenter) = view.project(ev.longitude, ev.latitude) else {
        return;
    };

    let max_km = max_s_wave_spread_km(ev.magnitude);
    let depth = ev.depth_km.max(0.0);
    let (p_km, s_km) = if let Some(eng) = travel {
        (
            eng.surface_distance_for_elapsed(Wave::P, ev.depth_km, elapsed_s),
            eng.surface_distance_for_elapsed(Wave::S, ev.depth_km, elapsed_s),
        )
    } else {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| tracing::warn!("无走时表，波圈使用直线速度兜底"));
        let surface = |v: f64| {
            let slant = elapsed_s * v;
            if slant <= depth {
                slant - depth
            } else {
                (slant * slant - depth * depth).sqrt()
            }
        };
        (surface(7.0), surface(5.0))
    };

    let (p_alpha, s_alpha, p_w, s_w) = if selected {
        (255_u8, 255_u8, 3.2_f32, 4.0_f32)
    } else {
        (170, 180, 2.4, 3.0)
    };

    let s_rgb = color_for(ev.intensity_kind, ev.intensity_level);
    // S 未出地表：画 eewcn 式竖条进度（P 绿 / S 烈度色），不出 S 圈
    if s_km < 0.0 && depth > 0.5 {
        paint_ps_progress_bar(&painter, epicenter, p_km, s_km, depth, s_rgb);
    }

    let max_r = rect.width().max(rect.height()) * 3.0;
    let p_draw = p_km.max(0.0);
    let s_draw = s_km.max(0.0);
    if s_km < max_km {
        let fade = (1.0 - (s_draw / max_km).clamp(0.0, 1.0)).mul_add(0.35, 0.65);
        let p_stroke = Stroke::new(
            p_w,
            Color32::from_rgba_unmultiplied(255, 255, 255, ((p_alpha as f64) * fade) as u8),
        );
        let s_stroke = Stroke::new(
            s_w,
            Color32::from_rgba_unmultiplied(
                s_rgb.r,
                s_rgb.g,
                s_rgb.b,
                ((s_alpha as f64) * fade) as u8,
            ),
        );
        if view.mode.is_globe() {
            if p_draw > 0.5 {
                stroke_polyline(
                    &painter,
                    &view.circle_segments(ev.longitude, ev.latitude, p_draw),
                    p_stroke,
                );
            }
            if s_draw > 0.5 {
                stroke_polyline(
                    &painter,
                    &view.circle_segments(ev.longitude, ev.latitude, s_draw),
                    s_stroke,
                );
            }
        } else {
            if p_draw > 0.5 {
                let p_r = projection::km_to_pixels(p_draw, ev.latitude, view.zoom).max(3.0);
                if p_r < max_r {
                    painter.circle_stroke(epicenter, p_r, p_stroke);
                }
            }
            if s_draw > 0.5 {
                let s_r = projection::km_to_pixels(s_draw, ev.latitude, view.zoom).max(3.0);
                if s_r < max_r {
                    painter.circle_stroke(epicenter, s_r, s_stroke);
                }
            }
        }
    }

    paint_epicenter_marker(
        &painter,
        view,
        icons,
        epicenter,
        ev,
        selected,
        show_cross,
    );
}

/// S 波尚在地下时的竖条进度（对齐 eewcn `barItems`：10×50，右偏约 50px）。
fn paint_ps_progress_bar(
    painter: &egui::Painter,
    epicenter: Pos2,
    p_km: f64,
    s_km: f64,
    depth_km: f64,
    s_rgb: starquake_core::palette::Rgb,
) {
    let depth = depth_km.max(1e-3);
    let bar_w = 10.0_f32;
    let bar_h = 50.0_f32;
    let border = 1.0_f32;
    // eewcn：锚点使条在震中右侧约 50px
    let left = epicenter.x + 50.0 - bar_w * 0.5;
    let top = epicenter.y - bar_h * 0.5;
    let outer = Rect::from_min_size(egui::pos2(left, top), Vec2::new(bar_w, bar_h));
    painter.rect_stroke(
        outer,
        0.0,
        Stroke::new(border, Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    let fill_h = (bar_h - border * 2.0).max(1.0);
    let inner_x = left + border;
    let inner_w = (bar_w - border * 2.0).max(1.0);
    let p_frac = ((1.0 + p_km / depth).clamp(0.0, 1.0)) as f32;
    let s_frac = ((1.0 + s_km / depth).clamp(0.0, 1.0)) as f32;
    let p_h = p_frac * fill_h;
    let s_h = s_frac * fill_h;
    let bottom = top + border + fill_h;
    // P：#A1E0BA；S：烈度色；自下而上填充
    if p_h > 0.5 {
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(inner_x, bottom - p_h),
                egui::pos2(inner_x + inner_w, bottom),
            ),
            0.0,
            Color32::from_rgb(0xA1, 0xE0, 0xBA),
        );
    }
    if s_h > 0.5 {
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(inner_x, bottom - s_h),
                egui::pos2(inner_x + inner_w, bottom),
            ),
            0.0,
            Color32::from_rgb(s_rgb.r, s_rgb.g, s_rgb.b),
        );
    }
}

/// ①②③… 序号（多震，对齐 eewcn `String.fromCharCode(9312+i)`）。
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

/// 多震序号相对震中的方位（对齐 eewcn index%4：下→上→左→右）。
fn circled_number_offset(index: usize) -> Vec2 {
    const D: f32 = 50.0;
    match index % 4 {
        0 => Vec2::new(0.0, D),
        1 => Vec2::new(0.0, -D),
        2 => Vec2::new(-D, 0.0),
        _ => Vec2::new(D, 0.0),
    }
}

/// 各数据源最新预警叉标（无波圈；选中项带标签）。
/// `show_cross`：有传播中波圈时按闪烁相位传入，对齐 eewcn。
/// `marker_nums`：与 `markers` 等长；`Some(n)` 为与左上预警条一致的 ①②…（1-based）；空切片则按枚举回退。
pub fn paint_eew_markers(
    ui: &mut Ui,
    view: &ViewCtx,
    markers: &[EewReport],
    marker_nums: &[Option<usize>],
    selected: Option<(&str, &str)>,
    icons: Option<&MarkerIcons>,
    show_cross: bool,
) {
    if markers.is_empty() {
        return;
    }
    let rect = view.rect;
    let painter = ui.painter_at(rect);
    let pad = 28.0_f32;
    let any_num = marker_nums.iter().any(|n| n.is_some_and(|v| v > 0))
        || (marker_nums.is_empty() && markers.len() > 1);
    for (i, ev) in markers.iter().enumerate() {
        let Some(p) = view.project(ev.longitude, ev.latitude) else {
            continue;
        };
        if p.x < rect.left() - pad
            || p.x > rect.right() + pad
            || p.y < rect.top() - pad
            || p.y > rect.bottom() + pad
        {
            continue;
        }
        let is_sel = selected == Some((ev.agency.0.as_str(), ev.event_id.as_str()));
        paint_epicenter_marker(&painter, view, icons, p, ev, is_sel, show_cross);
        // 多震序号：优先用与左上框对齐的编号
        let num = marker_nums
            .get(i)
            .copied()
            .flatten()
            .or_else(|| {
                if marker_nums.is_empty() && markers.len() > 1 {
                    Some(i + 1)
                } else {
                    None
                }
            });
        if any_num && show_cross {
            if let Some(n) = num.filter(|n| *n > 0) {
                let label = circled_index(n).to_string();
                let pos = p + circled_number_offset(n.saturating_sub(1));
                painter.text(
                    pos,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(22.0),
                    Color32::WHITE,
                );
            }
        }
    }
}

/// 台风悬停命中：中心或路径点，用于绘制实况详情。
#[derive(Debug, Clone)]
pub struct TyphoonHoverHit {
    pub anchor: Pos2,
    pub id: String,
    pub name: String,
    pub strong: String,
    pub power: i32,
    pub time_text: String,
    pub longitude: f64,
    pub latitude: f64,
    pub speed_ms: f64,
    pub pressure_hpa: f64,
    pub move_direction: String,
    pub move_speed_kmh: f64,
    /// 是否中心（否则为历史路径点）
    #[allow(dead_code)]
    pub is_center: bool,
}

/// 台风：实况路径、预报路径、风圈、中心徽章与悬停详情（对齐 eewcn MapTyphoonLogic）。
pub fn paint_typhoons(
    ui: &mut Ui,
    view: &ViewCtx,
    typhoons: &[starquake_core::TyphoonInfo],
    forecast_agency: &str,
    hover_pos: Option<Pos2>,
) -> Option<TyphoonHoverHit> {
    if typhoons.is_empty() {
        return None;
    }
    let rect = view.rect;
    let painter = ui.painter_at(rect);
    let agency_sel = forecast_agency.trim();
    let show_all = agency_sel.is_empty() || agency_sel == "全部";

    let mut best_hit: Option<(f32, TyphoonHoverHit)> = None;
    let hit_r2 = 14.0_f32 * 14.0;

    for tf in typhoons {
        let last = tf.points.last();
        // 风圈（7 / 10 / 12 级）— 先画，避免盖住路径
        if let Some(pt) = last {
            paint_wind_circles(
                &painter,
                view,
                pt.longitude,
                pt.latitude,
                pt.radius7,
                pt.radius10,
                pt.radius12,
            );
        }

        // 实况路径折线（段色按终点等级）
        for w in tf.points.windows(2) {
            let a = &w[0];
            let b = &w[1];
            let (r, g, bl) = starquake_core::typhoon_color_rgb(&b.strong, b.power);
            let stroke = Stroke::new(2.4_f32, Color32::from_rgb(r, g, bl));
            paint_geodesic_segment(
                &painter,
                view,
                a.longitude,
                a.latitude,
                b.longitude,
                b.latitude,
                stroke,
            );
        }

        // 预报路径（「全部」时线更细、透明度更低，减轻多源叠线杂乱）
        if let Some(pt) = last {
            let mut drawn = 0usize;
            for fc in &pt.forecasts {
                if !show_all {
                    let norm = normalize_typhoon_agency(&fc.agency);
                    if norm != agency_sel && fc.agency != agency_sel {
                        continue;
                    }
                }
                if fc.points.len() < 2 {
                    continue;
                }
                let (r, g, bl) = starquake_core::typhoon_forecast_agency_rgb(&fc.agency);
                let (w, a) = if show_all {
                    (1.35_f32, 150_u8)
                } else {
                    (2.0_f32, 210_u8)
                };
                let stroke = Stroke::new(w, Color32::from_rgba_unmultiplied(r, g, bl, a));
                for seg in fc.points.windows(2) {
                    paint_geodesic_segment(
                        &painter,
                        view,
                        seg[0].longitude,
                        seg[0].latitude,
                        seg[1].longitude,
                        seg[1].latitude,
                        stroke,
                    );
                }
                drawn += 1;
                if !show_all || drawn >= 5 {
                    break;
                }
            }
        }

        // 历史点 + 悬停
        let n = tf.points.len();
        for (i, pt) in tf.points.iter().enumerate() {
            let is_latest = i + 1 == n;
            let Some(p) = project_near(view, pt.longitude, pt.latitude) else {
                continue;
            };
            if !rect.expand(12.0).contains(p) {
                continue;
            }
            if !is_latest {
                let (r, g, bl) = starquake_core::typhoon_color_rgb(&pt.strong, pt.power);
                let rad = match starquake_core::typhoon_grade_level(&pt.strong, pt.power) {
                    0 => 7.0,
                    1 | 2 | 3 => 6.0,
                    4 => 5.0,
                    _ => 4.0,
                };
                painter.circle_filled(p, rad, Color32::from_rgb(r, g, bl));
                painter.circle_stroke(p, rad, Stroke::new(1.0_f32, Color32::WHITE));
            }
            if let Some(hp) = hover_pos {
                let d2 = (hp.x - p.x).powi(2) + (hp.y - p.y).powi(2);
                if d2 <= hit_r2 {
                    let hit = TyphoonHoverHit {
                        anchor: p,
                        id: tf.id.clone(),
                        name: tf.name.clone(),
                        strong: if pt.strong.is_empty() {
                            tf.strong.clone()
                        } else {
                            pt.strong.clone()
                        },
                        power: if pt.power > 0 { pt.power } else { tf.power },
                        time_text: pt.time_text.clone(),
                        longitude: pt.longitude,
                        latitude: pt.latitude,
                        speed_ms: if pt.speed_ms > 0.0 {
                            pt.speed_ms
                        } else {
                            tf.wind_speed_ms
                        },
                        pressure_hpa: if pt.pressure_hpa > 0.0 {
                            pt.pressure_hpa
                        } else {
                            tf.pressure_hpa
                        },
                        move_direction: if pt.move_direction.is_empty() {
                            tf.move_direction.clone()
                        } else {
                            pt.move_direction.clone()
                        },
                        move_speed_kmh: if pt.move_speed_kmh > 0.0 {
                            pt.move_speed_kmh
                        } else {
                            tf.move_speed_kmh
                        },
                        is_center: is_latest,
                    };
                    if best_hit.as_ref().is_none_or(|(bd, _)| d2 < *bd) {
                        best_hit = Some((d2, hit));
                    }
                }
            }
        }

        // 当前中心：仅圆环，状态条文案改由右上 HUD（不贴在地图上）
        let Some(p) = project_near(view, tf.longitude, tf.latitude) else {
            continue;
        };
        if !rect.expand(80.0).contains(p) {
            continue;
        }
        let (r, g, bl) = starquake_core::typhoon_color_rgb(&tf.strong, tf.power);
        painter.circle_stroke(p, 9.0, Stroke::new(2.2_f32, Color32::WHITE));
        painter.circle_stroke(p, 6.5, Stroke::new(2.0_f32, Color32::from_rgb(r, g, bl)));

        let strong = if tf.strong.is_empty() {
            last.map(|x| x.strong.as_str()).unwrap_or("")
        } else {
            tf.strong.as_str()
        };

        if let Some(hp) = hover_pos {
            let d2 = (hp.x - p.x).powi(2) + (hp.y - p.y).powi(2);
            if d2 <= hit_r2 {
                let hit = TyphoonHoverHit {
                    anchor: p,
                    id: tf.id.clone(),
                    name: tf.name.clone(),
                    strong: strong.to_string(),
                    power: tf.power,
                    time_text: last.map(|x| x.time_text.clone()).unwrap_or_default(),
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
                if best_hit.as_ref().is_none_or(|(bd, _)| d2 < *bd) {
                    best_hit = Some((d2, hit));
                }
            }
        }
    }

    // 详情改由 UI 画在测站卡片下方，避免地图上带尾巴的浮层
    best_hit.map(|(_, hit)| hit)
}

fn project_near(view: &ViewCtx, lon: f64, lat: f64) -> Option<Pos2> {
    let lon = projection::wrap_lon_near(lon, view.center_lon);
    view.project(lon, lat)
}

fn normalize_typhoon_agency(a: &str) -> String {
    match a.trim() {
        "台湾" | "中国台湾" => "中国台湾".into(),
        "香港" | "中国香港" => "中国香港".into(),
        other => other.to_string(),
    }
}

fn paint_wind_circles(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    r7: Option<starquake_core::TyphoonQuadrants>,
    r10: Option<starquake_core::TyphoonQuadrants>,
    r12: Option<starquake_core::TyphoonQuadrants>,
) {
    // 由外到内画：7 级最大 → 12 级最小，避免小圈被盖死
    if let Some(q) = r7 {
        paint_wind_quad(
            painter,
            view,
            lon,
            lat,
            q,
            Color32::from_rgba_unmultiplied(0x55, 0xFF, 0x55, 0x40),
            Color32::from_rgb(0x55, 0xFF, 0x55),
        );
    }
    if let Some(q) = r10 {
        paint_wind_quad(
            painter,
            view,
            lon,
            lat,
            q,
            Color32::from_rgba_unmultiplied(0xFF, 0xDD, 0x44, 0x48),
            Color32::from_rgb(0xFF, 0xDD, 0x44),
        );
    }
    if let Some(q) = r12 {
        paint_wind_quad(
            painter,
            view,
            lon,
            lat,
            q,
            Color32::from_rgba_unmultiplied(0xFF, 0x55, 0x55, 0x50),
            Color32::from_rgb(0xFF, 0x55, 0x55),
        );
    }
}

fn paint_wind_quad(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon: f64,
    lat: f64,
    q: starquake_core::TyphoonQuadrants,
    fill: Color32,
    border: Color32,
) {
    let Some(center) = project_near(view, lon, lat) else {
        return;
    };
    let quads = q.as_array();
    if quads.iter().all(|&r| r <= 0.0) {
        return;
    }
    // 象限边界平滑过渡；r=0 用邻象限非零半径，避免落到中心造成扇形缺口/斑纹
    let r_at = |az: f64| -> f64 {
        let a = az.rem_euclid(360.0);
        let sector = (a / 90.0).floor() as usize % 4;
        let local = a - sector as f64 * 90.0;
        let r0 = quads[sector];
        let r1 = quads[(sector + 1) % 4];
        let blend = 12.0_f64;
        let base = if r0 > 0.0 {
            r0
        } else if r1 > 0.0 {
            r1
        } else {
            quads
                .iter()
                .copied()
                .filter(|&x| x > 0.0)
                .fold(0.0_f64, f64::max)
        };
        if r0 > 0.0 && r1 > 0.0 && local > 90.0 - blend {
            let t = ((local - (90.0 - blend)) / blend).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            r0 * (1.0 - t) + r1 * t
        } else {
            base
        }
    };
    let step = 3.0_f64;
    let mut ring: Vec<Pos2> = Vec::with_capacity(130);
    let mut az = 0.0_f64;
    while az < 360.0 - 1e-6 {
        let r_km = r_at(az);
        if r_km > 0.0 {
            let (olon, olat) = crate::globe::move_on_globe(lon, lat, r_km, az);
            if let Some(p) = project_near(view, olon, olat) {
                ring.push(p);
            }
        }
        az += step;
    }
    if let Some(p0) = ring.first().copied() {
        ring.push(p0);
    }
    if ring.len() < 4 {
        return;
    }
    // 闭环耳切填色（均匀、无中心扇形缝）
    let mut flat: Vec<f64> = Vec::with_capacity((ring.len() - 1) * 2);
    for p in &ring[..ring.len() - 1] {
        flat.push(p.x as f64);
        flat.push(p.y as f64);
    }
    if let Ok(idx) = earcutr::earcut(&flat, &[], 2) {
        let mut mesh = egui::epaint::Mesh::default();
        let n = ring.len() - 1;
        mesh.reserve_vertices(n);
        mesh.reserve_triangles(idx.len() / 3);
        for p in &ring[..n] {
            mesh.colored_vertex(*p, fill);
        }
        for tri in idx.chunks_exact(3) {
            mesh.add_triangle(tri[0] as u32, tri[1] as u32, tri[2] as u32);
        }
        if !mesh.indices.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
        }
    } else {
        for w in ring.windows(2) {
            painter.add(egui::Shape::convex_polygon(
                vec![center, w[0], w[1]],
                fill,
                Stroke::NONE,
            ));
        }
    }
    painter.add(egui::Shape::line(
        ring,
        Stroke::new(1.8_f32, border),
    ));
}

fn paint_geodesic_segment(
    painter: &egui::Painter,
    view: &ViewCtx,
    lon0: f64,
    lat0: f64,
    lon1: f64,
    lat1: f64,
    stroke: Stroke,
) {
    // 分段插值；经度相对中心折绕，避免跨日界线拉直线
    let n = 8usize;
    let mut pts: Vec<Pos2> = Vec::with_capacity(n + 1);
    let lon0 = projection::wrap_lon_near(lon0, view.center_lon);
    let lon1 = projection::wrap_lon_near(lon1, view.center_lon);
    // 再折到相对 lon0 的最短弧
    let lon1 = lon0 + projection::normalize_lon(lon1 - lon0);
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let lon = lon0 + (lon1 - lon0) * t;
        let lat = lat0 + (lat1 - lat0) * t;
        if let Some(p) = project_near(view, lon, lat) {
            if let Some(prev) = pts.last() {
                let dx = p.x - prev.x;
                let dy = p.y - prev.y;
                if dx * dx + dy * dy > 180.0_f32 * 180.0 {
                    if pts.len() >= 2 {
                        painter.add(egui::Shape::line(pts.clone(), stroke));
                    }
                    pts.clear();
                }
            }
            pts.push(p);
        }
    }
    if pts.len() >= 2 {
        painter.add(egui::Shape::line(pts, stroke));
    }
}
