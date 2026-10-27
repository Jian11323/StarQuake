//! 启动画面：底图就绪且最短展示时间过后淡出关闭。

use crate::brand::{logo_texture, APP_DISPLAY_NAME};
use crate::theme::ThemePalette;
use egui::{
    Align2, Color32, ColorImage, Context, FontId, Pos2, Sense, TextureHandle, TextureOptions, Vec2,
};
use starquake_map::BasemapLoadState;
use std::time::Instant;

const MIN_SECS: f64 = 5.0;
const FADE_SECS: f64 = 0.55;
const SPLASH_BG_PNG: &[u8] = include_bytes!("../../../icon/brand/splash_bg.png");

enum SplashPhase {
    Hold,
    Fade(Instant),
    Done,
}

pub struct SplashGate {
    phase: SplashPhase,
    born: Instant,
}

impl Default for SplashGate {
    fn default() -> Self {
        Self {
            phase: SplashPhase::Hold,
            born: Instant::now(),
        }
    }
}

impl SplashGate {
    /// 仍在遮罩（含淡出中），挡操作。
    pub fn active(&self) -> bool {
        !matches!(self.phase, SplashPhase::Done)
    }

    /// 当前不透明度 1→0（淡出中插值）。
    pub fn opacity(&self) -> f32 {
        match self.phase {
            SplashPhase::Hold => 1.0,
            SplashPhase::Fade(started) => {
                let t = (started.elapsed().as_secs_f64() / FADE_SECS).clamp(0.0, 1.0);
                // smoothstep，收尾更柔
                let e = t * t * (3.0 - 2.0 * t);
                (1.0 - e) as f32
            }
            SplashPhase::Done => 0.0,
        }
    }

    /// 返回是否仍需绘制。世界层可用（或失败/无矢量）且过最短时间后进入淡出。
    pub fn tick(&mut self, load: BasemapLoadState) -> bool {
        match &self.phase {
            SplashPhase::Done => false,
            SplashPhase::Fade(started) => {
                if started.elapsed().as_secs_f64() >= FADE_SECS {
                    self.phase = SplashPhase::Done;
                    false
                } else {
                    true
                }
            }
            SplashPhase::Hold => {
                let usable = matches!(
                    load,
                    BasemapLoadState::Ready
                        | BasemapLoadState::Failed
                        | BasemapLoadState::Empty
                        | BasemapLoadState::LoadingRegional
                );
                if usable && self.born.elapsed().as_secs_f64() >= MIN_SECS {
                    self.phase = SplashPhase::Fade(Instant::now());
                }
                true
            }
        }
    }
}

fn with_alpha(c: Color32, opacity: f32) -> Color32 {
    let a = (f32::from(c.a()) * opacity.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

fn splash_bg_texture(ctx: &Context) -> Option<TextureHandle> {
    let id = egui::Id::new("starquake/splash_bg_tex");
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(tex);
    }
    let img = image::load_from_memory(SPLASH_BG_PNG).ok()?.into_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    let tex = ctx.load_texture("starquake/splash_bg", color, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

pub fn draw_splash(
    ctx: &egui::Context,
    p: &ThemePalette,
    status: &str,
    load: BasemapLoadState,
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.001 {
        return;
    }
    let screen = ctx.screen_rect();
    // 启动遮罩故意挡操作；淡出结束后卸掉
    egui::Area::new(egui::Id::new("starquake_splash"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
            let painter = ui.painter();
            let tint = Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * opacity) as u8);

            if let Some(tex) = splash_bg_texture(ctx) {
                // 等比铺满（cover）
                let tw = tex.size()[0] as f32;
                let th = tex.size()[1] as f32;
                let scale = (rect.width() / tw).max(rect.height() / th);
                let dw = tw * scale;
                let dh = th * scale;
                let img_rect = egui::Rect::from_center_size(rect.center(), Vec2::new(dw, dh));
                painter.image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                );
                // 轻暗罩，保证前景字可读且背景轮廓仍可见
                painter.rect_filled(
                    rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, (70.0 * opacity) as u8),
                );
            } else {
                painter.rect_filled(rect, 0.0, with_alpha(p.map_bg, opacity));
            }

            let c = screen.center();
            if let Some(tex) = logo_texture(ctx) {
                let sz = 96.0_f32;
                let r = egui::Rect::from_center_size(c - Vec2::new(0.0, 48.0), Vec2::splat(sz));
                painter.image(
                    tex.id(),
                    r,
                    egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                );
            }

            painter.text(
                c + Vec2::new(0.0, 28.0),
                Align2::CENTER_CENTER,
                APP_DISPLAY_NAME,
                FontId::proportional(28.0),
                with_alpha(p.text, opacity),
            );
            painter.text(
                c + Vec2::new(0.0, 56.0),
                Align2::CENTER_CENTER,
                crate::i18n::t("版本 {}").replace("{}", crate::APP_VERSION_DISPLAY),
                FontId::proportional(14.0),
                with_alpha(p.text_muted, opacity),
            );

            let line = if !status.is_empty() {
                status
            } else {
                match load {
                    BasemapLoadState::LoadingWorld => crate::i18n::t("正在加载世界底图…"),
                    BasemapLoadState::LoadingRegional => crate::i18n::t("正在加载区划…"),
                    BasemapLoadState::Failed => crate::i18n::t("底图加载失败"),
                    _ => crate::i18n::t("正在启动…"),
                }
            };
            painter.text(
                c + Vec2::new(0.0, 92.0),
                Align2::CENTER_CENTER,
                line,
                FontId::proportional(13.0),
                with_alpha(p.text_dim, opacity),
            );
        });
}
