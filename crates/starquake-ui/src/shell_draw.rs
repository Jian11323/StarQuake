//! 主界面绘制：信息头、侧栏、HUD、列表行等。

use std::sync::Arc;

use chrono::Local;
use egui::{Color32, FontFamily, FontId, Frame, Galley, Margin, RichText, Sense, Stroke, Ui, Vec2};
use crate::intensity_icons::IntensityIcons;
use starquake_core::palette::color_for;
use starquake_core::{
    agency_bracket, dedupe_same_eew_events, eew_sidebar_entries, eew_sidebar_group_key,
    eew_wave_active_spread, format_hm_minutes, is_wave_candidate, max_s_spread_km, place_with_agency,
    cn_intensity_display_unknown, prefer_intensity_scale, same_eew_event,
    sunrise_sunset_local_hm, AppSnapshot, EewReport,
    IntensityKind, ListSelection, OverlayMode, SidebarTab, WeatherAlert, cst, jst,
};
use weather_icons::{weather_level_color, WeatherIconCache};
use starquake_core::palette::{cn_intensity_rgb, jma_shindo_rgb};
use starquake_map::{BasemapLoadState, TyphoonHoverHit};
use starquake_travel::{s_wave_still_spreading_to, TravelEngine};

use crate::settings::MapPickKind;
use crate::theme::ThemePalette;
use crate::weather_icons;

/// 烈度图标绘制参数。
struct IntensityPaint<'a> {
    kind: IntensityKind,
    level: u8,
    text: &'a str,
    scale: &'a str,
    size: f32,
    instrumental: bool,
}

/// 倒计时 HUD 上下文。
pub(crate) struct CountdownHudCtx<'a> {
    pub snap: &'a AppSnapshot,
    pub show_nied: bool,
    pub map_rect: egui::Rect,
    pub local_lat: Option<f64>,
    pub local_lon: Option<f64>,
    pub zoom: f32,
}

/// 侧栏绘制上下文。
pub(crate) struct SidebarDrawCtx<'a> {
    pub snap: &'a mut AppSnapshot,
    pub scale: &'a str,
    pub icons: &'a IntensityIcons,
    pub weather_icons: &'a mut WeatherIconCache,
    pub show_volcano_icons: bool,
    pub source_enabled: [bool; 5],
    pub detached: bool,
    pub weather_display_mode: &'a mut String,
    pub weather_location_filter: &'a mut String,
    pub weather_cfg_dirty: &'a mut bool,
}

/// 地图 Central 区右上叠层锚点（用 panel 初段 `map_rect`，勿用 map 绘制后的 `ui.max_rect()`）。
#[derive(Clone, Copy)]
pub(crate) struct MapOverlayStack {
    pub map_rect: egui::Rect,
    pub stack_right: f32,
    pub zoom: f32,
}

const OVERLAY_STACK_COL_W: f32 = 280.0;
const OVERLAY_CARD_PREF_W: f32 = 260.0;
const OVERLAY_CARD_MIN_W: f32 = 200.0;

/// 浮窗/停靠侧栏压住地图右栏时，把卡片右缘左移（整列高度均生效）。
pub(crate) fn map_overlay_stack_right(
    map_rect: egui::Rect,
    zoom: f32,
    avoid_rect: Option<egui::Rect>,
) -> f32 {
    let margin = 12.0 * zoom;
    let mut right = map_rect.right() - margin;
    if let Some(block) = avoid_rect {
        let stack_col = egui::Rect::from_min_max(
            egui::pos2(map_rect.right() - OVERLAY_STACK_COL_W * zoom, map_rect.min.y),
            egui::pos2(map_rect.right(), map_rect.bottom()),
        );
        if block.intersects(stack_col) {
            right = right.min(block.left() - margin);
        }
    }
    right.clamp(map_rect.left() + margin, map_rect.right() - margin)
}

/// 右上叠层卡片 `(left, width)`：优先 300×zoom，随 stack 列与侧栏避让收缩。
pub(crate) fn overlay_card_rect(stack: MapOverlayStack) -> (f32, f32) {
    let z = stack.zoom;
    let margin = 12.0 * z;
    let col_left = stack.map_rect.right() - OVERLAY_STACK_COL_W * z;
    let avail = (stack.stack_right - col_left - margin).max(OVERLAY_CARD_MIN_W * z);
    let mut card_w = (OVERLAY_CARD_PREF_W * z).min(avail);
    let mut card_left = stack.stack_right - card_w;
    let min_left = stack.map_rect.left() + margin;
    if card_left < min_left {
        card_left = min_left;
        card_w = (stack.stack_right - card_left).max(OVERLAY_CARD_MIN_W * z);
    }
    card_w = card_w.min(stack.stack_right - card_left);
    (card_left, card_w)
}

/// 气象浮卡绘制参数。
pub(crate) struct WeatherCardDraw<'a> {
    pub alerts: &'a [WeatherAlert],
    pub filter: &'a str,
    pub card_index: &'a mut usize,
    pub weather_icons: &'a mut WeatherIconCache,
    pub anchor_top_y: f32,
    pub stack: MapOverlayStack,
}

struct VolcanoRowPaint<'a> {
    place: &'a str,
    sub: &'a str,
    title: &'a str,
    selected: bool,
    show_icon: bool,
    id: egui::Id,
    z: f32,
}

struct ListRowPaint<'a> {
    place: &'a str,
    sub: &'a str,
    mag: &'a str,
    kind: IntensityKind,
    level: u8,
    intensity_text: &'a str,
    scale: &'a str,
    accent: Color32,
    selected: bool,
    id: egui::Id,
    z: f32,
}


pub(crate) fn live_wave_eews_travel<'a>(
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

pub(crate) fn overlay_child<R>(
    ui: &mut Ui,
    id_salt: &str,
    rect: egui::Rect,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(id_salt)
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min))
            .sense(Sense::hover()),
    );
    add(&mut child)
}

pub(crate) fn round_coord(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

pub(crate) fn round_zoom(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub(crate) fn draw_map_pick_banner(ui: &mut Ui, p: &ThemePalette, kind: MapPickKind) {
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

pub(crate) fn format_header_time(origin_ms: i64) -> String {
    if origin_ms <= 0 {
        return "----/--/-- --:--:--".into();
    }
    let Some(dt) = chrono::DateTime::from_timestamp_millis(origin_ms) else {
        return "----/--/-- --:--:--".into();
    };
    let cst = cst();
    dt.with_timezone(&cst)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

pub(crate) fn format_list_time(origin_ms: i64) -> String {
    if origin_ms <= 0 {
        return "—".into();
    }
    let Some(dt) = chrono::DateTime::from_timestamp_millis(origin_ms) else {
        return "—".into();
    };
    let cst = cst();
    dt.with_timezone(&cst)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn rgb(c: starquake_core::palette::Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

#[allow(dead_code)]
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
fn paint_intensity(ui: &mut Ui, icons: &IntensityIcons, paint: IntensityPaint<'_>) {
    let IntensityPaint {
        kind,
        level,
        text,
        scale,
        size,
        instrumental,
    } = paint;
    let (k, lv, col, label) = intensity_chip(kind, level, text, scale);
    let (resp, painter) = ui.allocate_painter(Vec2::splat(size), Sense::hover());
    // 中国烈度 0：无数据显「—」；深源估算 <0.5 档时仍展示小数文案（如 0.4）
    let unknown_cn =
        k == IntensityKind::CnIntensity && lv == 0 && cn_intensity_display_unknown(&label);
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

pub(crate) fn draw_map_load_banner(ui: &mut Ui, p: &ThemePalette, state: BasemapLoadState, status: &str) {
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
pub(crate) fn eew_marker_display_num(
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
pub(crate) fn header_live_eews<'a>(
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
pub(crate) fn eew_panel_zoom(map_w: f32, map_h: f32) -> f32 {
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
pub(crate) fn draw_event_header(
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
                                IntensityPaint {
                                    kind: ev.intensity_kind,
                                    level: ev.intensity_level,
                                    text: &ev.max_intensity_text,
                                    scale,
                                    size: 72.0 * zoom,
                                    instrumental: false,
                                },
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

pub(crate) fn draw_tsunami_banner(
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
                                dt.with_timezone(&cst())
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


/// 台风实况详情卡片布局（紧贴测站卡片下方）。
pub(crate) struct TyphoonDetailLayout {
    pub bottom_y: f32,
    pub fly_to: Option<(f64, f64, f64)>,
}

/// 台风实况详情：固定在测站卡片下方，避免地图锚点浮层带尾巴。
/// 点击标题栏 → 飞向该台风中心。
pub(crate) fn draw_typhoon_detail_below_stations(
    ui: &mut Ui,
    p: &ThemePalette,
    hover: Option<&TyphoonHoverHit>,
    typhoons: &[starquake_core::TyphoonInfo],
    station_bottom_y: f32,
    stack: MapOverlayStack,
) -> TyphoonDetailLayout {
    let zoom = stack.zoom;
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
        return TyphoonDetailLayout {
            bottom_y: station_bottom_y,
            fly_to: None,
        };
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
        crate::i18n::t("编号: {}").replace("{}", &hit.id.to_string()).to_string(),
        format!(
            "中心位置: ({:.1}\u{00b0}N, {:.1}\u{00b0}E)",
            hit.latitude, hit.longitude
        ),
        format!("最大风速: {speed}"),
        format!("中心气压: {pressure}"),
        format!("移动速度: {mv}"),
    ];

    let (card_left, card_w) = overlay_card_rect(stack);
    let pad = 8.0 * zoom;
    let wrap_w = (card_w - 2.0 * pad).max(1.0);
    let header_font = FontId::proportional(13.0 * zoom);
    let body_font = FontId::proportional(12.0 * zoom);
    let header_galley =
        wrap_text_galley(ui, header.clone(), header_font, Color32::WHITE, wrap_w);
    let header_h = header_galley.size().y.max(26.0 * zoom);
    let line_galleys: Vec<Arc<Galley>> = lines
        .iter()
        .map(|line| wrap_text_galley(ui, line.clone(), body_font.clone(), p.text, wrap_w))
        .collect();
    let mut body_text_h = 0.0_f32;
    for (i, galley) in line_galleys.iter().enumerate() {
        if i > 0 {
            body_text_h += 4.0 * zoom;
        }
        body_text_h += galley.size().y;
    }
    let body_h = pad + body_text_h + pad;
    let total_h = header_h + 2.0 * zoom + body_h;
    let gap = 14.0 * zoom;
    let mut origin_y = station_bottom_y + gap;
    let max_bottom = stack.map_rect.bottom() - 12.0 * zoom;
    if origin_y + total_h > max_bottom {
        origin_y = (max_bottom - total_h).max(stack.map_rect.min.y + 8.0 * zoom);
    }
    let origin = egui::pos2(card_left, origin_y);
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
        painter.galley(
            egui::pos2(header_rect.min.x + pad, header_rect.min.y + pad * 0.4),
            header_galley.clone(),
            Color32::WHITE,
        );
        let mut y = body_rect.min.y + pad;
        for galley in &line_galleys {
            painter.galley(egui::pos2(body_rect.min.x + pad, y), galley.clone(), p.text);
            y += galley.size().y + 4.0 * zoom;
        }
    });
    let fly_to = if clicked
        && (hit.latitude.abs() > 0.01 || hit.longitude.abs() > 0.01)
    {
        Some((hit.longitude, hit.latitude, 9.0))
    } else {
        None
    };
    TyphoonDetailLayout {
        bottom_y: rect.bottom(),
        fly_to,
    }
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
pub(crate) struct StationCardLayout {
    pub(crate) clicked: Option<&'static str>,
    /// 卡片底边 Y；无卡片时为顶栏下方起点，供台风详情承接。
    pub(crate) bottom_y: f32,
}

/// 地图右上角实时震度/烈度卡片（对齐 EEWCN sourceMaxCard）。
/// 点击卡片返回台网 key，供跳转到该网最大测站。
pub(crate) fn draw_realtime_intensity_card(
    ui: &mut Ui,
    p: &ThemePalette,
    snap: &AppSnapshot,
    stack: MapOverlayStack,
) -> StationCardLayout {
    let zoom = stack.zoom;
    let top_y = stack.map_rect.min.y + OVERLAY_TOP_Y * zoom;
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
    let rect = egui::Rect::from_min_size(
        egui::pos2(stack.stack_right - total_w.max(72.0 * zoom), top_y),
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


pub(crate) fn draw_legend_bar(ui: &mut Ui, p: &ThemePalette, zoom: f32) {
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

        for (i, (label, level)) in SH_ROWS.iter().enumerate().take(SH_N) {
            let y0 = r.top() + row * i as f32;
            let cy = y0 + row * 0.5;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(sh_sw_x, y0 + pad),
                    egui::pos2(sh_sw_x + sw, y0 + row - pad),
                ),
                1.5 * zoom,
                rgb(jma_shindo_rgb(*level)),
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

pub(crate) fn draw_countdown_hud(ui: &mut Ui, p: &ThemePalette, ctx: CountdownHudCtx<'_>) {
    let CountdownHudCtx {
        snap,
        show_nied,
        map_rect,
        local_lat,
        local_lon,
        zoom,
    } = ctx;
    let local = Local::now();
    let clock = local.format("%Y/%m/%d %H:%M:%S").to_string();
    let jst_clock = if show_nied {
        let jst = jst();
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

pub(crate) enum SideAction {
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
pub(crate) fn draw_sidebar(
    ui: &mut Ui,
    p: &ThemePalette,
    ctx: SidebarDrawCtx<'_>,
) -> Option<SideAction> {
    let SidebarDrawCtx {
        snap,
        scale,
        icons,
        weather_icons,
        show_volcano_icons,
        source_enabled,
        detached,
        weather_display_mode,
        weather_location_filter,
        weather_cfg_dirty,
    } = ctx;
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
                        crate::i18n::t("{name}·关闭").replace("{name}", name).to_string()
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
                                        ListRowPaint {
                                            place: &place,
                                            sub: &sub,
                                            mag: "",
                                            kind: r.intensity_kind,
                                            level: r.intensity_level,
                                            intensity_text: &r.intensity_text,
                                            scale,
                                            accent: col,
                                            selected,
                                            id: egui::Id::new(("rec", i)),
                                            z,
                                        },
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
                                        ListRowPaint {
                                            place: &place,
                                            sub: &sub,
                                            mag: "",
                                            kind: e.intensity_kind,
                                            level: e.intensity_level,
                                            intensity_text: &e.max_intensity_text,
                                            scale,
                                            accent: col,
                                            selected,
                                            id: egui::Id::new(("eew", real_i)),
                                            z,
                                        },
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
                                        VolcanoRowPaint {
                                            place: &place,
                                            sub: &vol_time,
                                            title: desc,
                                            selected,
                                            show_icon: show_volcano_icons,
                                            id: egui::Id::new(("vol", i)),
                                            z,
                                        },
                                    ) {
                                        clicked = Some(SideAction::List(SidebarTab::Volcano, i));
                                    }
                                }
                            }
                            SidebarTab::Weather => {
                                draw_weather_sidebar_controls(
                                    ui,
                                    p,
                                    weather_display_mode,
                                    weather_location_filter,
                                    weather_cfg_dirty,
                                    z,
                                );
                                let filtered = weather_filtered_indices(
                                    snap.weather.as_slice(),
                                    weather_location_filter,
                                );
                                if snap.weather.is_empty() {
                                    empty_hint(
                                        ui,
                                        p,
                                        crate::i18n::t("暂无气象预警"),
                                        crate::i18n::t("启用气象源并登录 Jian 令牌后，中央台预警会出现在此列表。"),
                                        z,
                                    );
                                } else if filtered.is_empty() {
                                    empty_hint(
                                        ui,
                                        p,
                                        crate::i18n::t("无匹配预警"),
                                        crate::i18n::t("请调整地名筛选条件。"),
                                        z,
                                    );
                                } else if weather_is_card_mode(weather_display_mode) {
                                    ui.add_space(6.0 * z);
                                    ui.label(
                                        RichText::new(crate::i18n::t(
                                            "卡片模式：地图右上台风卡片下方显示气象悬浮卡片，每 30 秒自动轮播，也可点击序号切换。",
                                        ))
                                        .size(12.0 * z)
                                        .color(p.text_muted),
                                    );
                                } else {
                                    weather_icons.poll(ui.ctx());
                                    for (i, w) in snap.weather.iter().enumerate() {
                                        if !weather_matches_filter(w, weather_location_filter) {
                                            continue;
                                        }
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
                                            clicked =
                                                Some(SideAction::List(SidebarTab::Weather, i));
                                        }
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

fn wrap_text_galley(
    ui: &Ui,
    text: String,
    font: FontId,
    color: Color32,
    wrap_width: f32,
) -> Arc<Galley> {
    ui.fonts(|f| f.layout(text, font, color, wrap_width))
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

pub(crate) fn weather_is_card_mode(mode: &str) -> bool {
    !mode.trim().eq_ignore_ascii_case("list")
}

fn weather_matches_filter(alert: &WeatherAlert, filter: &str) -> bool {
    let q = filter.trim();
    if q.is_empty() {
        return true;
    }
    let hay = format!(
        "{} {} {} {} {} {}",
        alert.province,
        alert.city,
        alert.district,
        alert.title,
        alert.headline,
        alert.description
    );
    hay.contains(q)
}

pub(crate) const WEATHER_CARD_AUTO_SECS: u64 = 30;

fn weather_wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn weather_filtered_indices(alerts: &[WeatherAlert], filter: &str) -> Vec<usize> {
    let now = weather_wall_ms();
    alerts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.is_active_at(now) && weather_matches_filter(a, filter))
        .map(|(i, _)| i)
        .collect()
}

fn weather_level_label(icon_type: &str) -> &'static str {
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
        "001" => "红色预警",
        "002" => "橙色预警",
        "003" => "黄色预警",
        "004" => "蓝色预警",
        _ => "预警",
    }
}

fn weather_display_time(alert: &WeatherAlert) -> String {
    let t = alert.origin_time.trim();
    if !t.is_empty() {
        return t.replace('/', "-");
    }
    if alert.origin_ms <= 0 {
        return String::new();
    }
    use chrono::{TimeZone, Utc};
    let Some(dt) = Utc.timestamp_millis_opt(alert.origin_ms).single() else {
        return String::new();
    };
    let cst = dt.with_timezone(&cst());
    cst.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn weather_relieve_display(alert: &WeatherAlert) -> String {
    alert
        .relieve_time
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('/', "-"))
        .unwrap_or_default()
}

/// 卡片正文：按宽度换行，超出最大高度则截断。
fn weather_desc_card_galley(
    ui: &Ui,
    raw: &str,
    font: FontId,
    color: Color32,
    wrap_w: f32,
    max_h: f32,
) -> Arc<Galley> {
    let t = raw.trim();
    if t.is_empty() {
        return wrap_text_galley(ui, String::new(), font, color, wrap_w);
    }
    let total = t.chars().count();
    let mut end = total;
    loop {
        let mut s: String = t.chars().take(end).collect();
        if end < total {
            s.push('\u{2026}');
        }
        let g = wrap_text_galley(ui, s, font.clone(), color, wrap_w);
        if g.size().y <= max_h || end <= 48 {
            return g;
        }
        end = end.saturating_sub(24);
    }
}

fn weather_issuer_line(alert: &WeatherAlert) -> String {
    let region = weather_region_line(alert);
    if region.is_empty() {
        return String::new();
    }
    format!("{region}气象台")
}

/// 卡片标题：优先取「发布」后的预警类型短语（如寒潮蓝色预警信号）。
fn weather_card_brief_title(alert: &WeatherAlert) -> String {
    let h = weather_headline(alert);
    if let Some(pos) = h.find("发布") {
        let tail = h[pos + "发布".len()..].trim();
        if !tail.is_empty() {
            return tail.to_string();
        }
    }
    let t = alert.title.trim();
    if let Some(pos) = t.find("发布") {
        let tail = t[pos + "发布".len()..].trim();
        if !tail.is_empty() {
            return tail.to_string();
        }
    }
    let level = weather_level_label(&alert.icon_type);
    if level != "预警" {
        return format!("{level}信号");
    }
    weather_card_title(alert)
}

fn weather_card_title(alert: &WeatherAlert) -> String {
    let h = weather_headline(alert);
    if !h.is_empty() {
        return h;
    }
    alert.title.trim().to_string()
}

/// 气象悬浮卡片布局结果（位于台风卡片下方）。
pub(crate) struct WeatherCardLayout {
    pub(crate) fly_to: Option<(f64, f64, f64)>,
    pub(crate) select_index: Option<usize>,
}

fn draw_weather_sidebar_controls(
    ui: &mut Ui,
    p: &ThemePalette,
    display_mode: &mut String,
    location_filter: &mut String,
    cfg_dirty: &mut bool,
    z: f32,
) {
    ui.add_space(4.0 * z);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("展示"))
                .size(12.5 * z)
                .color(p.text_dim),
        );
        ui.add_space(6.0 * z);
        let card_mode = weather_is_card_mode(display_mode);
        if weather_mode_button(ui, p, crate::i18n::t("列表"), !card_mode, z).clicked() {
            *display_mode = "list".into();
            *cfg_dirty = true;
        }
        ui.add_space(4.0 * z);
        if weather_mode_button(ui, p, crate::i18n::t("卡片"), card_mode, z).clicked() {
            *display_mode = "card".into();
            *cfg_dirty = true;
        }
    });
    ui.add_space(6.0 * z);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t("地名筛选"))
                .size(12.5 * z)
                .color(p.text_dim),
        );
        ui.add_space(6.0 * z);
        let edit = egui::TextEdit::singleline(location_filter)
            .desired_width(ui.available_width().max(80.0 * z))
            .hint_text(crate::i18n::t("输入省市区或地名"))
            .margin(margin_xy(6.0 * z, 5.0 * z));
        let resp = ui.add(edit);
        if resp.changed() {
            *cfg_dirty = true;
        }
    });
    ui.add_space(4.0 * z);
    ui.separator();
}

fn weather_mode_button(
    ui: &mut Ui,
    p: &ThemePalette,
    label: &str,
    selected: bool,
    z: f32,
) -> egui::Response {
    let fill = if selected {
        p.accent_soft
    } else {
        p.chip_bg
    };
    let stroke = if selected {
        Stroke::new(1.0_f32 * z, p.accent)
    } else {
        Stroke::new(1.0_f32 * z, p.panel_border)
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(12.5 * z).color(p.text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(6.0 * z)
            .min_size(Vec2::new(44.0 * z, 26.0 * z)),
    )
}

pub(crate) fn draw_weather_floating_card(
    ui: &mut Ui,
    p: &ThemePalette,
    draw: WeatherCardDraw<'_>,
) -> WeatherCardLayout {
    let WeatherCardDraw {
        alerts,
        filter,
        card_index,
        weather_icons,
        anchor_top_y,
        stack,
    } = draw;
    let zoom = stack.zoom;
    let gap = 14.0 * zoom;
    let empty = WeatherCardLayout {
        fly_to: None,
        select_index: None,
    };
    let indices = weather_filtered_indices(alerts, filter);
    if indices.is_empty() {
        *card_index = 0;
        return empty;
    }
    if *card_index >= indices.len() {
        *card_index = 0;
    }

    let orig_idx = indices[*card_index];
    let alert = &alerts[orig_idx];
    let brief_title = weather_card_brief_title(alert);
    let full_title = weather_card_title(alert);
    let issuer = weather_issuer_line(alert);
    let time_val = weather_display_time(alert);
    let relieve_val = weather_relieve_display(alert);
    let region = weather_region_line(alert);
    let issuer_line = if issuer.is_empty() {
        format!("{}：—", crate::i18n::t("发布机构"))
    } else {
        format!("{}：{issuer}", crate::i18n::t("发布机构"))
    };
    let time_line = if time_val.is_empty() {
        format!("{}：—", crate::i18n::t("发布时间"))
    } else {
        format!("{}：{time_val}", crate::i18n::t("发布时间"))
    };
    let relieve_line = if relieve_val.is_empty() {
        None
    } else {
        Some(format!(
            "{}：{relieve_val}",
            crate::i18n::t("预计解除")
        ))
    };
    let region_line = if region.is_empty() {
        format!("{}：—", crate::i18n::t("影响地区"))
    } else {
        format!("{}：{region}", crate::i18n::t("影响地区"))
    };
    let source_line = crate::i18n::t("数据来源：中国气象局");

    let (card_left, card_w) = overlay_card_rect(stack);
    let pad = 12.0 * zoom;
    let body_pad = 8.0 * zoom;
    let icon_sz = 44.0 * zoom;
    let row_gap = 5.0 * zoom;
    let page_w = 52.0 * zoom;
    let text_x_off = pad;
    let content_w = (card_w - 2.0 * pad).max(1.0);
    let meta_x_off = text_x_off + icon_sz + 10.0 * zoom;
    let meta_w = (card_w - pad - meta_x_off).max(1.0);
    let header_title_font = FontId::proportional(14.0 * zoom);
    let meta_font = FontId::proportional(12.5 * zoom);
    let subtitle_font = FontId::proportional(13.0 * zoom);
    let desc_font = FontId::proportional(12.0 * zoom);
    let header_title_w = (card_w - 2.0 * pad - page_w).max(1.0);
    let header_title_galley = wrap_text_galley(
        ui,
        brief_title.clone(),
        header_title_font,
        Color32::WHITE,
        header_title_w,
    );
    let header_h = (header_title_galley.size().y + 10.0 * zoom).max(32.0 * zoom);
    let issuer_galley =
        wrap_text_galley(ui, issuer_line, meta_font.clone(), p.text_muted, meta_w);
    let time_galley = wrap_text_galley(ui, time_line, meta_font.clone(), p.text_muted, meta_w);
    let relieve_galley = relieve_line.as_ref().map(|line| {
        wrap_text_galley(ui, line.clone(), meta_font.clone(), p.text_muted, meta_w)
    });
    let region_galley = wrap_text_galley(ui, region_line, meta_font, p.text_muted, meta_w);
    let show_subtitle = !full_title.is_empty() && full_title != brief_title;
    let subtitle_galley = if show_subtitle {
        Some(wrap_text_galley(
            ui,
            full_title.clone(),
            subtitle_font.clone(),
            p.text,
            content_w,
        ))
    } else {
        None
    };
    let desc_max_h = 78.0 * zoom;
    let desc_galley = weather_desc_card_galley(
        ui,
        alert.description.trim(),
        desc_font.clone(),
        p.text_muted,
        content_w,
        desc_max_h,
    );
    let has_desc = !alert.description.trim().is_empty();
    let chip_font = FontId::proportional(11.0 * zoom);
    let chip_label_g = ui.fonts(|f| {
        f.layout_no_wrap(
            source_line.to_string(),
            chip_font.clone(),
            p.text_dim,
        )
    });
    let chip_h = 22.0 * zoom;
    let chip_w = chip_label_g.size().x + 16.0 * zoom;
    let chip_lead_gap = row_gap + 6.0 * zoom;
    let issuer_h = issuer_galley.size().y;
    let time_h = time_galley.size().y;
    let relieve_h = relieve_galley.as_ref().map(|g| g.size().y).unwrap_or(0.0);
    let region_h = region_galley.size().y;
    let mut text_block_h = issuer_h + row_gap + time_h;
    if relieve_galley.is_some() {
        text_block_h += row_gap + relieve_h;
    }
    text_block_h += row_gap + region_h;
    let detail_block_h = text_block_h.max(icon_sz);
    let subtitle_h = subtitle_galley
        .as_ref()
        .map(|g| g.size().y)
        .unwrap_or(0.0);
    let desc_h = if has_desc { desc_galley.size().y } else { 0.0 };
    let mut body_h = body_pad + detail_block_h;
    if show_subtitle {
        body_h += row_gap + subtitle_h;
    }
    if has_desc {
        body_h += row_gap + desc_h;
    }
    body_h += chip_lead_gap + chip_h + body_pad;
    let total_h = header_h + body_h;
    let mut origin_y = anchor_top_y + gap;
    let max_bottom = stack.map_rect.bottom() - 12.0 * zoom;
    if origin_y + total_h > max_bottom {
        origin_y = (max_bottom - total_h).max(stack.map_rect.min.y + 8.0 * zoom);
    }
    let origin = egui::pos2(card_left, origin_y);
    let rect = egui::Rect::from_min_size(origin, Vec2::new(card_w, total_h));

    let page_total = indices.len();
    let mut cycle_next = false;
    let mut fly_to = None;

    overlay_child(ui, "weather_floating_card", rect, |ui| {
        weather_icons.poll(ui.ctx());
        let icon_tex = weather_icons.get(ui.ctx(), &alert.icon_type);
        let level_color = weather_level_color(&alert.icon_type);
        let page = format!("{}/{}", *card_index + 1, page_total);
        let header_rect = egui::Rect::from_min_size(rect.min, Vec2::new(card_w, header_h));
        let body_rect = egui::Rect::from_min_size(
            egui::pos2(rect.min.x, header_rect.bottom()),
            Vec2::new(card_w, body_h),
        );
        let text_x = body_rect.min.x + text_x_off;
        let meta_x = body_rect.min.x + meta_x_off;

        let page_rect = egui::Rect::from_min_size(
            egui::pos2(header_rect.right() - pad - page_w, header_rect.min.y),
            Vec2::new(page_w, header_h),
        );
        let page_resp = ui.interact(page_rect, ui.id().with("wx_page"), Sense::click());
        if page_resp.clicked() {
            cycle_next = true;
        }
        if page_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let title_resp = ui.interact(
            egui::Rect::from_min_max(
                egui::pos2(header_rect.min.x + pad, header_rect.min.y),
                egui::pos2(page_rect.min.x - 2.0 * zoom, header_rect.max.y),
            ),
            ui.id().with("wx_title_click"),
            Sense::click(),
        );
        if title_resp.clicked() {
            if let (Some(lat), Some(lon)) = (alert.latitude, alert.longitude) {
                if lat.abs() > 0.01 || lon.abs() > 0.01 {
                    fly_to = Some((lon, lat, 9.0));
                }
            }
        }
        if title_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let painter = ui.painter();
        let radius = 8.0 * zoom;
        let light = p.text.r() < 128;
        let border = if light {
            Color32::from_rgb(0xC5, 0xCB, 0xD3)
        } else {
            p.panel_border
        };
        let body_fill = Color32::from_rgba_unmultiplied(
            p.panel_solid.r(),
            p.panel_solid.g(),
            p.panel_solid.b(),
            0xF0,
        );
        let shadow = Color32::from_rgba_unmultiplied(0, 0, 0, if light { 18 } else { 48 });
        painter.rect_filled(rect.expand(1.0 * zoom), radius + 1.0, shadow);
        painter.rect_filled(
            header_rect,
            egui::CornerRadius {
                nw: radius as u8,
                ne: radius as u8,
                sw: 0,
                se: 0,
            },
            level_color,
        );
        painter.rect_filled(
            body_rect,
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: radius as u8,
                se: radius as u8,
            },
            body_fill,
        );
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32 * zoom, border),
            egui::StrokeKind::Outside,
        );

        let hdr_text_y = header_rect.min.y + (header_h - header_title_galley.size().y) * 0.5;
        painter.galley(
            egui::pos2(header_rect.min.x + pad, hdr_text_y),
            header_title_galley.clone(),
            Color32::WHITE,
        );
        painter.text(
            page_rect.center(),
            egui::Align2::CENTER_CENTER,
            page,
            FontId::proportional(12.0 * zoom),
            Color32::WHITE,
        );

        let block_top = body_rect.min.y + body_pad;
        let icon_y = block_top + (detail_block_h - icon_sz) * 0.5;
        let icon_rect = egui::Rect::from_min_size(egui::pos2(text_x, icon_y), Vec2::splat(icon_sz));
        if let Some(tex) = icon_tex.as_ref() {
            painter.image(
                tex.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.rect_filled(icon_rect, 6.0 * zoom, level_color);
        }

        let mut ty = block_top;
        painter.galley(egui::pos2(meta_x, ty), issuer_galley.clone(), p.text_muted);
        ty += issuer_h + row_gap;
        painter.galley(egui::pos2(meta_x, ty), time_galley.clone(), p.text_muted);
        ty += time_h + row_gap;
        if let Some(ref rg) = relieve_galley {
            painter.galley(egui::pos2(meta_x, ty), rg.clone(), p.text_muted);
            ty += relieve_h + row_gap;
        }
        painter.galley(egui::pos2(meta_x, ty), region_galley.clone(), p.text_muted);

        let mut y = block_top + detail_block_h;
        if let Some(ref sub) = subtitle_galley {
            y += row_gap;
            painter.galley(egui::pos2(text_x, y), sub.clone(), p.text);
            y += subtitle_h;
        }
        if has_desc {
            y += row_gap;
            painter.galley(egui::pos2(text_x, y), desc_galley.clone(), p.text_muted);
            y += desc_h;
        }
        y += chip_lead_gap;

        let chip_rect = egui::Rect::from_min_size(
            egui::pos2(text_x, y),
            Vec2::new(chip_w, chip_h),
        );
        painter.rect_filled(chip_rect, chip_h * 0.5, p.row_selected);
        painter.galley(
            egui::pos2(
                chip_rect.min.x + (chip_rect.width() - chip_label_g.size().x) * 0.5,
                chip_rect.min.y + (chip_h - chip_label_g.size().y) * 0.5,
            ),
            chip_label_g.clone(),
            p.text_dim,
        );
    });

    let mut select_index = None;
    if cycle_next {
        *card_index = (*card_index + 1) % indices.len();
        select_index = Some(indices[*card_index]);
    }

    WeatherCardLayout {
        fly_to,
        select_index,
    }
}

fn volcano_list_row(ui: &mut Ui, p: &ThemePalette, row: VolcanoRowPaint<'_>) -> bool {
    let VolcanoRowPaint {
        place,
        sub,
        title,
        selected,
        show_icon,
        id,
        z,
    } = row;
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
    row: ListRowPaint<'_>,
) -> RowClick {
    let ListRowPaint {
        place,
        sub,
        mag,
        kind,
        level,
        intensity_text,
        scale,
        accent,
        selected,
        id,
        z: _z,
    } = row;
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
                        IntensityPaint {
                            kind,
                            level,
                            text: intensity_text,
                            scale,
                            size: chip,
                            instrumental: false,
                        },
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
