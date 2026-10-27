//! 产品显示名与品牌图标（窗口 / 关于页）。

use egui::{ColorImage, Context, IconData, TextureHandle, TextureOptions};

/// 面向用户的软件名称。
pub const APP_DISPLAY_NAME: &str = "StarQuake";

/// 面向用户的版本号（`Beta 2.3.4`；Cargo 包版本为数字 `2.3.4`）。
pub const APP_VERSION_DISPLAY: &str = concat!("Beta ", env!("CARGO_PKG_VERSION"));

const APP_PNG: &[u8] = include_bytes!("../../../icon/brand/app.png");

/// 主窗口 / 设置窗的任务栏与标题栏图标。
pub fn app_icon_data() -> Option<IconData> {
    let img = image::load_from_memory(APP_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

/// 关于页 / 启动画面 logo（按 Context 缓存，避免每帧解码上传）。
pub fn logo_texture(ctx: &Context) -> Option<TextureHandle> {
    let id = egui::Id::new("starquake/brand_logo_tex");
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(tex);
    }
    let img = image::load_from_memory(APP_PNG).ok()?.into_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    let tex = ctx.load_texture("starquake/brand_logo", color, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}
