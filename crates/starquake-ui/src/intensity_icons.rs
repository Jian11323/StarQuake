//! 烈度/震度图标：`icon/intensity/` 下各标度 SVG（观测 / 仪器）。
//!
//! JMA：level 0–9 → 0,1,2,3,4,5弱,5强,6弱,6强,7。
//! CN / MMI：1–12；LPGM：1–4。

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use starquake_core::IntensityKind;
use std::collections::HashMap;

macro_rules! svg {
    ($rel:expr) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../icon/intensity/",
            $rel
        ))
    };
}

const RASTER_PX: u32 = 128;

const SHINDO_SVG: [(&[u8], u8); 10] = [
    (svg!("shindo/Shindo 0.svg"), 0),
    (svg!("shindo/Shindo 1.svg"), 1),
    (svg!("shindo/Shindo 2.svg"), 2),
    (svg!("shindo/Shindo 3.svg"), 3),
    (svg!("shindo/Shindo 4.svg"), 4),
    (svg!("shindo/Shindo 5-.svg"), 5),
    (svg!("shindo/Shindo 5+.svg"), 6),
    (svg!("shindo/Shindo 6-.svg"), 7),
    (svg!("shindo/Shindo 6+.svg"), 8),
    (svg!("shindo/Shindo 7.svg"), 9),
];

const SHINDO_INST_SVG: [(&[u8], u8); 10] = [
    (svg!("shindo_inst/Shindo 0.svg"), 0),
    (svg!("shindo_inst/Shindo 1.svg"), 1),
    (svg!("shindo_inst/Shindo 2.svg"), 2),
    (svg!("shindo_inst/Shindo 3.svg"), 3),
    (svg!("shindo_inst/Shindo 4.svg"), 4),
    (svg!("shindo_inst/Shindo 5-.svg"), 5),
    (svg!("shindo_inst/Shindo 5+.svg"), 6),
    (svg!("shindo_inst/Shindo 6-.svg"), 7),
    (svg!("shindo_inst/Shindo 6+.svg"), 8),
    (svg!("shindo_inst/Shindo 7.svg"), 9),
];

const CSIS_SVG: [(&[u8], u8); 12] = [
    (svg!("csis/CSIS 1.svg"), 1),
    (svg!("csis/CSIS 2.svg"), 2),
    (svg!("csis/CSIS 3.svg"), 3),
    (svg!("csis/CSIS 4.svg"), 4),
    (svg!("csis/CSIS 5.svg"), 5),
    (svg!("csis/CSIS 6.svg"), 6),
    (svg!("csis/CSIS 7.svg"), 7),
    (svg!("csis/CSIS 8.svg"), 8),
    (svg!("csis/CSIS 9.svg"), 9),
    (svg!("csis/CSIS 10.svg"), 10),
    (svg!("csis/CSIS 11.svg"), 11),
    (svg!("csis/CSIS 12.svg"), 12),
];

const CSIS_INST_SVG: [(&[u8], u8); 12] = [
    (svg!("csis_inst/CSIS 1.svg"), 1),
    (svg!("csis_inst/CSIS 2.svg"), 2),
    (svg!("csis_inst/CSIS 3.svg"), 3),
    (svg!("csis_inst/CSIS 4.svg"), 4),
    (svg!("csis_inst/CSIS 5.svg"), 5),
    (svg!("csis_inst/CSIS 6.svg"), 6),
    (svg!("csis_inst/CSIS 7.svg"), 7),
    (svg!("csis_inst/CSIS 8.svg"), 8),
    (svg!("csis_inst/CSIS 9.svg"), 9),
    (svg!("csis_inst/CSIS 10.svg"), 10),
    (svg!("csis_inst/CSIS 11.svg"), 11),
    (svg!("csis_inst/CSIS 12.svg"), 12),
];

#[derive(Default)]
pub struct IntensityIcons {
    jma: HashMap<u8, TextureHandle>,
    jma_inst: HashMap<u8, TextureHandle>,
    csis: HashMap<u8, TextureHandle>,
    csis_inst: HashMap<u8, TextureHandle>,
    tried: bool,
}

impl IntensityIcons {
    pub fn ensure(&mut self, ctx: &Context) {
        if self.tried {
            return;
        }
        self.tried = true;
        // 启动只加载侧栏常用的 JMA / CSIS；MMI·LPGM 首次取用时再栅格化
        load_set(ctx, "starquake/shindo", &SHINDO_SVG, &mut self.jma);
        load_set(ctx, "starquake/shindo_inst", &SHINDO_INST_SVG, &mut self.jma_inst);
        load_set(ctx, "starquake/csis", &CSIS_SVG, &mut self.csis);
        load_set(ctx, "starquake/csis_inst", &CSIS_INST_SVG, &mut self.csis_inst);
        if self.jma.len() < 10 || self.csis.len() < 12 {
            tracing::warn!("部分烈度/震度 SVG 未能加载，将回退为色块文字");
        }
    }

    pub fn get_ex(&self, kind: IntensityKind, level: u8, instrumental: bool) -> Option<&TextureHandle> {
        match kind {
            IntensityKind::JmaShindo => {
                let lv = level.min(9);
                if instrumental {
                    self.jma_inst.get(&lv).or_else(|| self.jma.get(&lv))
                } else {
                    self.jma.get(&lv)
                }
            }
            IntensityKind::CnIntensity => {
                // 0 = 上游无烈度；禁止钳成 1 显示假「烈度1」图标
                if level == 0 {
                    return None;
                }
                let lv = level.clamp(1, 12);
                if instrumental {
                    self.csis_inst.get(&lv).or_else(|| self.csis.get(&lv))
                } else {
                    self.csis.get(&lv)
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn get_mmi(&self, _level: u8, _instrumental: bool) -> Option<&TextureHandle> {
        None
    }

    #[allow(dead_code)]
    pub fn get_lpgm(&self, _level: u8, _instrumental: bool) -> Option<&TextureHandle> {
        None
    }
}

fn load_set(
    ctx: &Context,
    prefix: &str,
    items: &[(&[u8], u8)],
    map: &mut HashMap<u8, TextureHandle>,
) {
    for (bytes, level) in items {
        if let Some(tex) = load_svg(ctx, &format!("{prefix}/{level}"), bytes) {
            map.insert(*level, tex);
        }
    }
}

fn load_svg(ctx: &Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let image = rasterize_svg(bytes, RASTER_PX)?;
    Some(ctx.load_texture(name, image, TextureOptions::LINEAR))
}

fn rasterize_svg(bytes: &[u8], max_px: u32) -> Option<ColorImage> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;
    let size = tree.size();
    let sw = size.width().max(1.0);
    let sh = size.height().max(1.0);
    let scale = max_px as f32 / sw.max(sh);
    let w = (sw * scale).round().max(1.0) as u32;
    let h = (sh * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(ColorImage::from_rgba_premultiplied(
        [w as usize, h as usize],
        pixmap.data(),
    ))
}
