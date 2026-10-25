//! 气象预警：侧栏列表/卡片切换、地名筛选、地图悬浮卡片与轮播。

use std::sync::Arc;

use egui::{Color32, FontId, Frame, Galley, Margin, RichText, Sense, Stroke, Ui, Vec2};
use starquake_core::{WeatherAlert, cst};
use crate::theme::ThemePalette;
use crate::weather_icons::{weather_level_color, WeatherIconCache};

fn overlay_child<R>(
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

fn margin_xy(x: f32, y: f32) -> Margin {
    Margin::symmetric(x.round().clamp(0.0, 127.0) as i8, y.round().clamp(0.0, 127.0) as i8)
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
/// 气象侧栏行：左图标，右三行 headline / 省市区 / description。
pub(crate) fn weather_list_row(
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

/// 气象浮卡绘制参数。
pub(crate) struct WeatherCardDraw<'a> {
    pub alerts: &'a [WeatherAlert],
    pub filter: &'a str,
    pub card_index: &'a mut usize,
    pub weather_icons: &'a mut WeatherIconCache,
    pub anchor_top_y: f32,
    pub stack: MapOverlayStack,
}

/// 气象悬浮卡片布局结果（位于台风卡片下方）。
pub(crate) struct WeatherCardLayout {
    pub(crate) fly_to: Option<(f64, f64, f64)>,
    pub(crate) select_index: Option<usize>,
}

pub(crate) fn draw_weather_sidebar_controls(
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

