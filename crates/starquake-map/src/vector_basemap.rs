//! 行政区矢量底图（暗色矢量 / 亮色浅填）。
//!
//! 世界层：`geodata/kanameishi/medium.global.modified.topo.json`（精细，本身不含中日国界）。
//! 区划层：中国优先 GQ（含藏南），日本优先要石/JMA（含北方四岛）；并由省级溶解出国级外轮廓。
//! 低于 zoom 7.5 只画中日国级；达到后叠省级。平面 / 球体按投影分别加载。
//! 加载在后台线程分两阶段（世界 → 区划），主线程 `poll` 合并。

use crate::globe::zoom_to_scroll;
use crate::projection::{self, ViewCtx};
use crate::topojson;
use crate::PaintQuality;
use egui::{Color32, Pos2, Rect, Stroke, Ui};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use tracing::{info, warn};

/// 海洋底色（暗色轮廓图：近黑）
pub const OCEAN: Color32 = Color32::from_rgb(0x20, 0x21, 0x24);

/// 矢量陆地 / 边界色（随界面主题切换）。
#[derive(Debug, Clone, Copy)]
pub struct VectorStyle {
    pub land_fill: Color32,
    pub world_border: Color32,
    pub admin_border: Color32,
}

impl VectorStyle {
    pub fn dark() -> Self {
        Self {
            // 轮廓图：陆地不填色；描边浅灰
            land_fill: Color32::from_rgb(0x4A, 0x4A, 0x4E),
            world_border: Color32::from_rgb(0xC8, 0xC8, 0xCC),
            admin_border: Color32::from_rgb(0xA0, 0xA0, 0xA4),
        }
    }

    pub fn light() -> Self {
        Self {
            land_fill: Color32::from_rgb(0xE8, 0xEC, 0xF0),
            world_border: Color32::from_rgb(0x6B, 0x72, 0x80),
            admin_border: Color32::from_rgb(0x9C, 0xA3, 0xAF),
        }
    }

    pub fn for_theme(theme: &str) -> Self {
        if theme.eq_ignore_ascii_case("light") {
            Self::light()
        } else {
            Self::dark()
        }
    }
}

impl Default for VectorStyle {
    fn default() -> Self {
        Self::dark()
    }
}

/// 中日省级描边起始缩放：低于此只画国级外轮廓，达到后叠省级。
const ZOOM_REGIONAL: f64 = 7.5;
const ZOOM_REGIONAL_MED: f64 = 8.2;
const ZOOM_REGIONAL_FINE: f64 = 9.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasemapMode {
    Vector,
    Tiles,
    Both,
}

impl BasemapMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "tiles" | "tile" | "raster" => Self::Tiles,
            "both" | "overlay" => Self::Both,
            _ => Self::Vector,
        }
    }

    /// `auto`：亮色→地形瓦片；暗色→矢量（GQ 风格：海洋底 + 陆地 Path 填色）。
    pub fn resolve(configured: &str, theme: &str) -> Self {
        match configured.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => {
                if theme.eq_ignore_ascii_case("light") {
                    Self::Tiles
                } else {
                    Self::Vector
                }
            }
            other => Self::parse(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vector => "vector",
            Self::Tiles => "tiles",
            Self::Both => "both",
        }
    }

    pub fn show_vector(self) -> bool {
        matches!(self, Self::Vector | Self::Both)
    }

    pub fn show_tiles(self) -> bool {
        matches!(self, Self::Tiles | Self::Both)
    }
}

#[derive(Clone)]
struct RingSet {
    exterior: Vec<[f32; 2]>,
    holes: Vec<Vec<[f32; 2]>>,
    min_lon: f32,
    min_lat: f32,
    max_lon: f32,
    max_lat: f32,
    /// 沿环解缠后的经度跨度；>180° 表示绕地过半，禁止填色。
    lon_span: f32,
    /// 经纬度平面耳切三角剖分（外环+孔洞顶点序）；空表示不可填。
    tris: Vec<u32>,
}

impl RingSet {
    fn from_parts(exterior: Vec<[f32; 2]>, holes: Vec<Vec<[f32; 2]>>) -> Option<Self> {
        if exterior.len() < 3 {
            return None;
        }
        let lon_span = unwrapped_lon_span(&exterior);
        let (min_lon, min_lat, max_lon, max_lat) = bbox_of(&exterior);
        // 当前底图只描轮廓（fill_land=false），不预计算耳切，显著省内存
        Some(Self {
            exterior,
            holes,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            lon_span,
            tris: Vec::new(),
        })
    }

    /// 绕地过半或跨日界的环只描边；其余用 earcut 预剖分后 Mesh 填色。
    fn may_fill(&self) -> bool {
        self.lon_span <= 180.0 + 1e-3 && !has_antimeridian_edge(&self.exterior)
    }
}

struct LayerLods {
    coarse: Vec<RingSet>,
    medium: Vec<RingSet>,
    fine: Vec<RingSet>,
}

fn empty_lods() -> LayerLods {
    LayerLods {
        coarse: Vec::new(),
        medium: Vec::new(),
        fine: Vec::new(),
    }
}

impl LayerLods {
    fn from_fine(fine: Vec<RingSet>, coarse_tol: f64, medium_tol: f64) -> Self {
        let medium: Vec<RingSet> = fine
            .iter()
            .flat_map(|r| simplify_ringsets(r, medium_tol))
            .collect();
        let coarse: Vec<RingSet> = fine
            .iter()
            .flat_map(|r| simplify_ringsets(r, coarse_tol))
            .collect();
        Self {
            coarse,
            medium,
            fine,
        }
    }

    fn pick(&self, zoom: f64, z_med: f64, z_fine: f64) -> &[RingSet] {
        // 区县量大时 coarse/medium 可能为空（跳过简化）；禁止回退到 fine，
        // 否则远距也会每帧描几千区划，亮色矢量会卡死/崩。
        if zoom < z_med {
            if !self.coarse.is_empty() {
                self.coarse.as_slice()
            } else {
                &[]
            }
        } else if zoom < z_fine {
            if !self.medium.is_empty() {
                self.medium.as_slice()
            } else if !self.coarse.is_empty() {
                self.coarse.as_slice()
            } else {
                &[]
            }
        } else if !self.fine.is_empty() {
            self.fine.as_slice()
        } else if !self.medium.is_empty() {
            self.medium.as_slice()
        } else {
            self.coarse.as_slice()
        }
    }
}

enum LoadPacket {
    World {
        lods: LayerLods,
        rings: usize,
        files: usize,
        skipped: usize,
        /// GlobalQuake Special 分层（有则球体按 scroll 绘制）
        gq: Option<GqLayers>,
    },
    Regional {
        lods: LayerLods,
        national: LayerLods,
        rings: usize,
        files: usize,
        skipped: usize,
    },
}

/// 旧 GQ 分层（世界层已改要石；保留结构以兼容 paint 分支）。
#[derive(Clone, Default)]
struct GqLayers {
    md: Vec<RingSet>,
    md_near: Vec<RingSet>,
    cn: Vec<RingSet>,
    jp: Vec<RingSet>,
}

/// 矢量底图加载阶段（供 HUD 文案）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasemapLoadState {
    Empty,
    LoadingWorld,
    LoadingRegional,
    Ready,
    Failed,
}

/// 视口不变时复用已镶嵌 Mesh（Arc），命中只增引用计数，避免每帧深拷 Path。
/// 同缩放平移时用 `mesh_lon/lat` 做像素平移；飞跃中用轻量 mesh 做 zoom 缩放。
struct VectorPaintCache {
    key: u64,
    /// 与 zoom/rect/style 相关、不含中心点的键，用于判定可平移
    base_key: u64,
    /// 不含 zoom/center/quality：飞跃整段可变换复用
    interact_key: u64,
    build_zoom: f64,
    mesh_lon: f64,
    mesh_lat: f64,
    /// 飞跃专用：仅世界远景 MD，顶点数少，按帧缩放才不卡
    fly_lite: bool,
    mesh: Arc<egui::epaint::Mesh>,
}

pub struct VectorBasemap {
    world: LayerLods,
    /// 中日省界 / 都道府县（近距）
    regional: LayerLods,
    /// 中日外轮廓（远距；由区划边界溶解得到）
    national: LayerLods,
    /// GlobalQuake Special 完整图层（优先）
    gq: Option<GqLayers>,
    world_ready: bool,
    regional_ready: bool,
    state: BasemapLoadState,
    status: String,
    gen: u64,
    rx: Option<Receiver<(u64, LoadPacket)>>,
    loaded_dir: Option<PathBuf>,
    /// 当前内存几何面向球体还是平面；切换投影时整包重载
    loaded_for_globe: Option<bool>,
    paint_cache: Option<VectorPaintCache>,
}

impl Default for VectorBasemap {
    fn default() -> Self {
        Self {
            world: LayerLods {
                coarse: Vec::new(),
                medium: Vec::new(),
                fine: Vec::new(),
            },
            regional: LayerLods {
                coarse: Vec::new(),
                medium: Vec::new(),
                fine: Vec::new(),
            },
            national: LayerLods {
                coarse: Vec::new(),
                medium: Vec::new(),
                fine: Vec::new(),
            },
            gq: None,
            world_ready: false,
            regional_ready: false,
            state: BasemapLoadState::Empty,
            status: String::new(),
            gen: 0,
            rx: None,
            loaded_dir: None,
            loaded_for_globe: None,
            paint_cache: None,
        }
    }
}

impl VectorBasemap {
    pub fn is_loaded(&self) -> bool {
        self.world_ready || self.regional_ready
    }

    pub fn load_state(&self) -> BasemapLoadState {
        self.state
    }

    pub fn status_text(&self) -> &str {
        &self.status
    }

    /// 后台分两阶段加载：先全球轮廓，再中日省级区划。
    /// `for_globe`：球体/平面各自只常驻一套几何；切换投影时整包重载。
    pub fn load_dir(&mut self, dir: impl AsRef<Path>, for_globe: bool) {
        let dir = dir.as_ref().to_path_buf();
        if !dir.is_dir() {
            warn!(path = %dir.display(), "geodata 目录不存在，矢量底图未加载");
            self.state = BasemapLoadState::Failed;
            self.status = format!("geodata 不存在：{}", dir.display());
            return;
        }
        if self.loaded_dir.as_ref() == Some(&dir)
            && self.loaded_for_globe == Some(for_globe)
            && self.state == BasemapLoadState::Ready
            && self.world_ready
        {
            return;
        }
        if self.loaded_dir.as_ref() == Some(&dir)
            && self.loaded_for_globe == Some(for_globe)
            && matches!(
                self.state,
                BasemapLoadState::LoadingWorld | BasemapLoadState::LoadingRegional
            )
        {
            return;
        }

        if self.loaded_for_globe != Some(for_globe) || self.loaded_dir.as_ref() != Some(&dir)
        {
            self.clear_geometry();
        }

        self.gen = self.gen.wrapping_add(1);
        let gen = self.gen;
        self.loaded_dir = Some(dir.clone());
        self.loaded_for_globe = Some(for_globe);
        self.state = BasemapLoadState::LoadingWorld;
        self.status = if for_globe {
            "正在加载球体底图…".into()
        } else {
            "正在加载平面底图…".into()
        };

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);

        let _ = thread::Builder::new()
            .name("starquake-geodata".into())
            .spawn(move || {
                let (world_lods, world_n, w_files, w_skip, gq) =
                    load_world_lods(&dir, for_globe);
                if tx
                    .send((
                        gen,
                        LoadPacket::World {
                            lods: world_lods,
                            rings: world_n,
                            files: w_files,
                            skipped: w_skip,
                            gq,
                        },
                    ))
                    .is_err()
                {
                    return;
                }

                // 平面/球体：省级 + 由省级溶解的中日国级外轮廓（始终常驻）
                let (mut regional_fine, r_files, r_skip) = load_regional_rings(&dir);
                let diaoyu = builtin_diaoyu_patches();
                let mut national_fine = dissolve_national_outlines(&regional_fine);
                national_fine.extend(diaoyu.iter().cloned());
                regional_fine.extend(diaoyu);
                let regional_n = regional_fine.len();
                // 份量不大：coarse/fine 共用一份，各 zoom pick 都能命中
                let regional_lods = LayerLods {
                    coarse: regional_fine.clone(),
                    medium: Vec::new(),
                    fine: regional_fine,
                };
                let national_lods = LayerLods {
                    coarse: national_fine.clone(),
                    medium: Vec::new(),
                    fine: national_fine,
                };

                let _ = tx.send((
                    gen,
                    LoadPacket::Regional {
                        lods: regional_lods,
                        national: national_lods,
                        rings: regional_n,
                        files: r_files,
                        skipped: r_skip,
                    },
                ));
            });
    }

    fn clear_geometry(&mut self) {
        self.world = empty_lods();
        self.regional = empty_lods();
        self.national = empty_lods();
        self.gq = None;
        self.world_ready = false;
        self.regional_ready = false;
        self.paint_cache = None;
    }

    /// 飞跃落地 / 大跨度平移后丢弃可复用 mesh，强制按当前视口重镶嵌。
    pub fn invalidate_paint_cache(&mut self) {
        self.paint_cache = None;
    }

    /// 主线程每帧拉取后台结果；有更新时返回 true。
    pub fn poll(&mut self) -> bool {
        let Some(rx) = self.rx.take() else {
            return false;
        };
        let mut changed = false;
        let mut keep = true;
        while let Ok((gen, packet)) = rx.try_recv() {
            if gen != self.gen {
                continue;
            }
            changed = true;
            self.paint_cache = None;
            match packet {
                LoadPacket::World {
                    lods,
                    rings,
                    files,
                    skipped,
                    gq,
                } => {
                    self.world = lods;
                    self.gq = gq;
                    self.world_ready = !self.world.coarse.is_empty()
                        || !self.world.medium.is_empty()
                        || !self.world.fine.is_empty()
                        || self.gq.as_ref().is_some_and(|g| !g.md.is_empty());
                    self.state = BasemapLoadState::LoadingRegional;
                    self.status = if self.world_ready {
                        format!("世界底图就绪（{rings} 环），正在加载区划…")
                    } else {
                        "世界底图为空，正在加载区划…".into()
                    };
                    info!(
                        rings,
                        files,
                        skipped,
                        "vector world layer ready"
                    );
                }
                LoadPacket::Regional {
                    lods,
                    national,
                    rings,
                    files,
                    skipped,
                } => {
                    self.regional = lods;
                    self.national = national;
                    self.regional_ready = !self.regional.coarse.is_empty() || !self.regional.medium.is_empty() || !self.regional.fine.is_empty()
                        || !self.national.coarse.is_empty() || !self.national.medium.is_empty() || !self.national.fine.is_empty();
                    self.state = if self.world_ready || self.regional_ready {
                        BasemapLoadState::Ready
                    } else {
                        BasemapLoadState::Failed
                    };
                    self.status = if self.state == BasemapLoadState::Ready {
                        String::new()
                    } else {
                        "矢量底图加载失败（无可用几何）".into()
                    };
                    info!(
                        rings,
                        files,
                        skipped,
                        world = self.world.fine.len(),
                        regional = self.regional.fine.len(),
                        national = self.national.fine.len(),
                        "vector basemap fully loaded"
                    );
                    keep = false;
                }
            }
        }
        if keep {
            self.rx = Some(rx);
        }
        changed
    }

    /// 静止帧可复用的完整底图 mesh（飞跃轻量图不算）。
    pub fn still_mesh(&self) -> Option<std::sync::Arc<egui::epaint::Mesh>> {
        self.paint_cache
            .as_ref()
            .filter(|c| !c.fly_lite)
            .map(|c| c.mesh.clone())
    }

    /// `lite`：拖拽/滚轮轻量绘制；飞跃应传 `false`，否则会只剩粗国界像「没加载地图」。
    pub fn paint(
        &mut self,
        ui: &mut Ui,
        view: &ViewCtx,
        fill_land: bool,
        style: VectorStyle,
        skip_world_stroke: bool,
        quality: PaintQuality,
        lite: bool,
    ) {
        if !self.world_ready && !self.regional_ready {
            return;
        }

        let rect = view.rect;
        let zoom = view.zoom;
        let painter = ui.painter_at(rect);
        let ppp = ui.ctx().pixels_per_point();
        let interacting = lite;
        let q = if interacting {
            quality.while_interacting()
        } else {
            quality
        };
        let cache_key =
            paint_cache_key(view, fill_land, style, self.gen, skip_world_stroke, q, ppp);
        let base_key =
            paint_base_key(view, fill_land, style, self.gen, skip_world_stroke, q, ppp);
        let interact_key =
            paint_interact_key(view, fill_land, style, self.gen, skip_world_stroke, ppp);

        if let Some(c) = self.paint_cache.as_ref() {
            if c.key == cache_key && !c.fly_lite {
                painter.add(egui::Shape::Mesh(c.mesh.clone()));
                return;
            }
        }

        // 飞跃/拖拽：只用轻量 MD mesh 做缩放+平移。两端 |dz/dt| 大，全量 mesh 按帧缩放必卡；
        // 中间 zoom 几乎不变所以以前「只平移」显得流畅。
        if interacting && !view.mode.is_globe() {
            let lite_ok = self.paint_cache.as_ref().is_some_and(|c| {
                c.fly_lite && c.interact_key == interact_key
            });
            if lite_ok {
                if let Some(c) = self.paint_cache.as_mut() {
                    if apply_flat_mesh_view_delta(c, view) {
                        c.key = cache_key;
                        c.base_key = base_key;
                        painter.add(egui::Shape::Mesh(c.mesh.clone()));
                        return;
                    }
                }
            }
            // 无轻量缓存：下面强制只镶嵌 world.coarse，并标 fly_lite
        } else if !interacting && self.paint_cache.as_ref().is_some_and(|c| c.fly_lite) {
            // 落地后丢掉飞跃轻量图，走完整重建（勿在飞跃中每帧清空）
            self.paint_cache = None;
        }

        // 平面同缩放：小幅平移可复用 mesh；超过视口约 28% 必须重建，否则新露出岸线（如海南）进不了 mesh
        let can_shift = self.paint_cache.as_ref().is_some_and(|c| {
            !view.mode.is_globe()
                && !c.fly_lite
                && c.base_key == base_key
                && (c.build_zoom - view.zoom).abs() < 1e-9
        });
        if can_shift {
            if let Some(c) = self.paint_cache.as_mut() {
                if let Some((dx, dy)) = vector_flat_pan_delta(c.mesh_lon, c.mesh_lat, view) {
                    let shift_limit = rect.width().max(rect.height()) * 0.28;
                    if dx.abs() > shift_limit || dy.abs() > shift_limit {
                        // 落入下方完整重建
                    } else {
                        if dx.abs() > 0.05 || dy.abs() > 0.05 {
                            Arc::make_mut(&mut c.mesh).translate(egui::Vec2::new(dx, dy));
                            c.mesh_lon = view.center_lon;
                            c.mesh_lat = view.center_lat;
                        }
                        c.key = cache_key;
                        painter.add(egui::Shape::Mesh(c.mesh.clone()));
                        return;
                    }
                }
            }
        }

        let mut shapes = Vec::with_capacity(512);
        let seg_bias = match q {
            PaintQuality::Eco => 1.6_f32,
            PaintQuality::Normal => 1.0,
            PaintQuality::Extreme => 0.85,
        };
        let (z_world_med, z_world_fine) = match q {
            PaintQuality::Eco => (3.2_f64, 5.2_f64),
            PaintQuality::Normal => (4.0, 6.0),
            PaintQuality::Extreme => (4.0, 6.0),
        };
        let regional_zoom = match q {
            PaintQuality::Eco => ZOOM_REGIONAL + 0.6,
            _ => ZOOM_REGIONAL,
        };

        if view.mode.is_globe() {
            // 仅拖拽/滚轮用粗轮廓抽稀；飞跃走完整 GQ 分层，避免跳转时像没底图
            if interacting {
                paint_globe_fly_lite(&mut shapes, self, view, style);
            } else {
                paint_globe_gq_style(&mut shapes, self, view, fill_land, style);
            }
            let mesh = bake_shapes_mesh_lite(ppp, shapes);
            painter.add(egui::Shape::Mesh(mesh.clone()));
            self.paint_cache = Some(VectorPaintCache {
                key: cache_key,
                base_key,
                interact_key,
                build_zoom: view.zoom,
                mesh_lon: view.center_lon,
                mesh_lat: view.center_lat,
                fly_lite: interacting,
                mesh,
            });
            return;
        }

        let world_px = projection::world_size(zoom) as f32;
        let full_wrap = rect.width() >= world_px * 0.92;
        let (min_lon, min_lat, max_lon, max_lat) = if full_wrap {
            (-180.0, -85.0, 180.0, 85.0)
        } else {
            viewport_bounds(view.center_lon, view.center_lat, zoom, rect)
        };
        let max_jump_px = world_px * (80.0 / 360.0);
        // 宽窗需要 ±360 副本；飞跃中至少保留 1 份副本，避免两侧空白
        let half_copies = {
            let need = ((rect.width() * 0.5 / world_px.max(1.0)).ceil() as i32 + 1).clamp(1, 6);
            if interacting {
                if full_wrap || rect.width() > world_px * 0.65 {
                    need.min(2)
                } else {
                    0
                }
            } else {
                need
            }
        };

        for k in -half_copies..=half_copies {
            let lon_shift = f64::from(k) * 360.0;

            if !self.world.coarse.is_empty()
                || !self.world.medium.is_empty()
                || !self.world.fine.is_empty()
            {
                // 要石世界层本身无中日国界；近景直接叠省级即可
                let rings = if !self.world.coarse.is_empty() {
                    self.world.coarse.as_slice()
                } else {
                    self.world.pick(zoom, z_world_med, z_world_fine)
                };
                let min_seg = if interacting {
                    3.0_f32 * seg_bias // 飞跃轻度抽稀折线
                } else if zoom < z_world_med {
                    1.2_f32 * seg_bias
                } else {
                    0.8_f32 * seg_bias
                };
                let stroke = Stroke::new(0.9_f32, style.world_border);
                // 世界层不画藏南/麦克马洪线一带，缺口由中国国级/省级补上
                paint_rings(
                    &mut shapes,
                    rings,
                    view,
                    min_lon + lon_shift,
                    min_lat,
                    max_lon + lon_shift,
                    max_lat,
                    min_seg,
                    max_jump_px,
                    stroke,
                    fill_land,
                    false,
                    style.land_fill,
                    lon_shift,
                    true,
                );
            }

            // 要石世界层无中日国界：
            // zoom < 7.5 → 只画国级外轮廓；zoom ≥ 7.5 → 叠省级（外轮廓由省界承担，避免双描）。
            paint_cn_jp_admin_flat(
                &mut shapes,
                self,
                view,
                zoom,
                regional_zoom,
                interacting,
                seg_bias,
                q,
                fill_land,
                style,
                min_lon + lon_shift,
                min_lat,
                max_lon + lon_shift,
                max_lat,
                max_jump_px,
                lon_shift,
            );
        }

        let mesh = if interacting {
            bake_shapes_mesh_lite(ppp, shapes)
        } else {
            bake_shapes_mesh(ppp, shapes)
        };
        painter.add(egui::Shape::Mesh(mesh.clone()));
        self.paint_cache = Some(VectorPaintCache {
            key: cache_key,
            base_key,
            interact_key,
            build_zoom: view.zoom,
            mesh_lon: view.center_lon,
            mesh_lat: view.center_lat,
            fly_lite: interacting,
            mesh,
        });
    }
}

fn paint_cache_key(
    view: &ViewCtx,
    fill_land: bool,
    style: VectorStyle,
    gen: u64,
    skip_world_stroke: bool,
    quality: PaintQuality,
    ppp: f32,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    gen.hash(&mut h);
    view.center_lon.to_bits().hash(&mut h);
    view.center_lat.to_bits().hash(&mut h);
    view.zoom.to_bits().hash(&mut h);
    (view.rect.width() as i32).hash(&mut h);
    (view.rect.height() as i32).hash(&mut h);
    (view.rect.min.x as i32).hash(&mut h);
    (view.rect.min.y as i32).hash(&mut h);
    fill_land.hash(&mut h);
    skip_world_stroke.hash(&mut h);
    style.land_fill.to_array().hash(&mut h);
    style.world_border.to_array().hash(&mut h);
    style.admin_border.to_array().hash(&mut h);
    std::mem::discriminant(&view.mode).hash(&mut h);
    quality.hash(&mut h);
    ((ppp * 100.0).round() as i32).hash(&mut h);
    h.finish()
}

/// 不含相机中心：同缩放平移可复用 mesh。
fn paint_base_key(
    view: &ViewCtx,
    fill_land: bool,
    style: VectorStyle,
    gen: u64,
    skip_world_stroke: bool,
    quality: PaintQuality,
    ppp: f32,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    gen.hash(&mut h);
    view.zoom.to_bits().hash(&mut h);
    (view.rect.width() as i32).hash(&mut h);
    (view.rect.height() as i32).hash(&mut h);
    (view.rect.min.x as i32).hash(&mut h);
    (view.rect.min.y as i32).hash(&mut h);
    fill_land.hash(&mut h);
    skip_world_stroke.hash(&mut h);
    style.land_fill.to_array().hash(&mut h);
    style.world_border.to_array().hash(&mut h);
    style.admin_border.to_array().hash(&mut h);
    std::mem::discriminant(&view.mode).hash(&mut h);
    quality.hash(&mut h);
    ((ppp * 100.0).round() as i32).hash(&mut h);
    h.finish()
}

/// 飞跃复用键：忽略 zoom/center/quality，仅窗口与样式变化时失效。
fn paint_interact_key(
    view: &ViewCtx,
    fill_land: bool,
    style: VectorStyle,
    gen: u64,
    skip_world_stroke: bool,
    ppp: f32,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    gen.hash(&mut h);
    (view.rect.width() as i32).hash(&mut h);
    (view.rect.height() as i32).hash(&mut h);
    (view.rect.min.x as i32).hash(&mut h);
    (view.rect.min.y as i32).hash(&mut h);
    fill_land.hash(&mut h);
    skip_world_stroke.hash(&mut h);
    style.land_fill.to_array().hash(&mut h);
    style.world_border.to_array().hash(&mut h);
    style.admin_border.to_array().hash(&mut h);
    std::mem::discriminant(&view.mode).hash(&mut h);
    ((ppp * 100.0).round() as i32).hash(&mut h);
    h.finish()
}

fn vector_flat_pan_delta(build_lon: f64, build_lat: f64, view: &ViewCtx) -> Option<(f32, f32)> {
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

/// 将已镶嵌平面 mesh 从 `(mesh_lon/lat, build_zoom)` 变到当前视口：缩放+平移合并为一次顶点遍历。
fn apply_flat_mesh_view_delta(c: &mut VectorPaintCache, view: &ViewCtx) -> bool {
    let dz = view.zoom - c.build_zoom;
    let s = if dz.abs() > 1e-9 {
        let s = 2f32.powf(dz as f32);
        if !s.is_finite() || s < 1e-4 || s > 1e4 {
            return false;
        }
        s
    } else {
        1.0
    };
    let (dx, dy) = match vector_flat_pan_delta(c.mesh_lon, c.mesh_lat, view) {
        Some(d) => d,
        None => (0.0, 0.0), // 跨日界：本帧只缩放，静止后重建
    };
    let need_s = (s - 1.0).abs() > 1e-6;
    let need_t = dx.abs() > 0.05 || dy.abs() > 0.05;
    if !need_s && !need_t {
        return true;
    }
    let origin = view.rect.center();
    let mesh = Arc::make_mut(&mut c.mesh);
    if need_s && need_t {
        let t = egui::vec2(dx, dy);
        for v in &mut mesh.vertices {
            v.pos = origin + (v.pos - origin) * s + t;
        }
    } else if need_s {
        for v in &mut mesh.vertices {
            v.pos = origin + (v.pos - origin) * s;
        }
    } else {
        mesh.translate(egui::Vec2::new(dx, dy));
    }
    if need_s {
        c.build_zoom = view.zoom;
    }
    if need_t {
        c.mesh_lon = view.center_lon;
        c.mesh_lat = view.center_lat;
    }
    true
}

/// 将 Path 等 Shape 预镶嵌为 Arc\<Mesh\>，缓存命中时 O(1) 克隆。
fn bake_shapes_mesh(ppp: f32, shapes: Vec<egui::Shape>) -> Arc<egui::epaint::Mesh> {
    bake_shapes_mesh_opts(ppp, shapes, true)
}

/// 飞跃轻量镶嵌：关 feathering，顶点更少，按帧缩放更稳。
fn bake_shapes_mesh_lite(ppp: f32, shapes: Vec<egui::Shape>) -> Arc<egui::epaint::Mesh> {
    bake_shapes_mesh_opts(ppp, shapes, false)
}

fn bake_shapes_mesh_opts(
    ppp: f32,
    shapes: Vec<egui::Shape>,
    feathering: bool,
) -> Arc<egui::epaint::Mesh> {
    let opts = egui::epaint::TessellationOptions {
        feathering,
        ..Default::default()
    };
    let mut tess = egui::epaint::Tessellator::new(ppp, opts, [1024, 1024], Vec::new());
    tess.set_clip_rect(egui::Rect::EVERYTHING);
    let mut mesh = egui::epaint::Mesh::default();
    for shape in shapes {
        tess.tessellate_shape(shape, &mut mesh);
    }
    Arc::new(mesh)
}


fn layer_has_rings(lods: &LayerLods) -> bool {
    !lods.coarse.is_empty() || !lods.medium.is_empty() || !lods.fine.is_empty()
}

/// 平面：中日国级始终补画（填世界层藏南缺口）；省级仅 zoom ≥ regional_zoom。
fn paint_cn_jp_admin_flat(
    shapes: &mut Vec<egui::Shape>,
    basemap: &VectorBasemap,
    view: &ViewCtx,
    zoom: f64,
    regional_zoom: f64,
    interacting: bool,
    seg_bias: f32,
    q: PaintQuality,
    fill_land: bool,
    style: VectorStyle,
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    max_jump_px: f32,
    lon_shift: f64,
) {
    // 国级始终画：世界层跳过麦克马洪线后，靠它补藏南中国主张边界
    if layer_has_rings(&basemap.national) {
        let rings = basemap.national.pick(zoom, 4.0, 6.0);
        let min_seg = if interacting {
            2.8_f32 * seg_bias
        } else {
            1.4_f32 * seg_bias
        };
        paint_rings(
            shapes,
            rings,
            view,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            min_seg,
            max_jump_px,
            Stroke::new(0.9_f32, style.world_border),
            fill_land,
            false,
            style.land_fill,
            lon_shift,
            false,
        );
    } else if let Some(gq) = basemap.gq.as_ref() {
        let stroke_admin = Stroke::new(0.85_f32, style.admin_border);
        for rings in [gq.cn.as_slice(), gq.jp.as_slice()] {
            if rings.is_empty() {
                continue;
            }
            paint_rings(
                shapes,
                rings,
                view,
                min_lon,
                min_lat,
                max_lon,
                max_lat,
                1.6_f32 * seg_bias,
                max_jump_px,
                stroke_admin,
                fill_land,
                false,
                style.land_fill,
                lon_shift,
                false,
            );
        }
    }

    let show_provincial = zoom >= regional_zoom && layer_has_rings(&basemap.regional);
    if !show_provincial {
        return;
    }
    let rings = basemap
        .regional
        .pick(zoom, ZOOM_REGIONAL_MED, ZOOM_REGIONAL_FINE);
    let min_seg = if interacting {
        3.2_f32 * seg_bias
    } else if zoom < ZOOM_REGIONAL_MED {
        2.0_f32 * seg_bias
    } else if zoom < ZOOM_REGIONAL_FINE {
        1.4_f32 * seg_bias
    } else {
        1.0_f32 * seg_bias
    };
    let stroke = Stroke::new(
        if interacting || zoom < ZOOM_REGIONAL_MED {
            0.7_f32
        } else {
            0.95_f32
        },
        style.admin_border,
    );
    paint_rings(
        shapes,
        rings,
        view,
        min_lon,
        min_lat,
        max_lon,
        max_lat,
        min_seg,
        max_jump_px,
        stroke,
        fill_land,
        !interacting && q != PaintQuality::Eco && zoom >= ZOOM_REGIONAL_FINE,
        style.land_fill,
        lon_shift,
        false,
    );
}

/// 球体：中日国级始终补藏南；省级仅 zoom ≥ ZOOM_REGIONAL。
fn paint_cn_jp_admin_globe(
    shapes: &mut Vec<egui::Shape>,
    basemap: &VectorBasemap,
    view: &ViewCtx,
    style: VectorStyle,
    stride: usize,
) {
    let (min_lon, min_lat, max_lon, max_lat) = (-180.0, -90.0, 180.0, 90.0);
    let stroke_admin = Stroke::new(0.85_f32, style.admin_border);
    let stroke_nation = Stroke::new(0.9_f32, style.world_border);

    if layer_has_rings(&basemap.national) {
        let rings = basemap.national.pick(view.zoom, 4.0, 6.0);
        paint_rings_globe(
            shapes,
            rings,
            view,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            stroke_nation,
            stride,
            false,
        );
    } else if let Some(gq) = basemap.gq.as_ref() {
        for rings in [&gq.cn, &gq.jp] {
            if rings.is_empty() {
                continue;
            }
            paint_rings_globe(
                shapes,
                rings,
                view,
                min_lon,
                min_lat,
                max_lon,
                max_lat,
                stroke_admin,
                stride,
                false,
            );
        }
    }

    if view.zoom >= ZOOM_REGIONAL && layer_has_rings(&basemap.regional) {
        let rings = basemap
            .regional
            .pick(view.zoom, ZOOM_REGIONAL_MED, ZOOM_REGIONAL_FINE);
        paint_rings_globe(
            shapes,
            rings,
            view,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            stroke_admin,
            stride,
            false,
        );
    }
}

/// 对齐 GQ `GlobePanel`：世界轮廓 + 中日国级/省级分档。
fn paint_globe_gq_style(
    shapes: &mut Vec<egui::Shape>,
    basemap: &VectorBasemap,
    view: &ViewCtx,
    _fill_land: bool,
    style: VectorStyle,
) {
    // 球体固定只描轮廓（不做陆地填色 / 地平线弓弧），避免穿海弦线
    let (min_lon, min_lat, max_lon, max_lat) = (-180.0, -90.0, 180.0, 90.0);
    let stroke_w = Stroke::new(0.9_f32, style.world_border);
    let scroll = zoom_to_scroll(view.zoom);

    if let Some(gq) = basemap.gq.as_ref() {
        // 远景完整 MD；近景用去掉中日的 MD + 本侧国级/省级
        if scroll >= 0.5 {
            paint_rings_globe(
                shapes,
                &gq.md,
                view,
                min_lon,
                min_lat,
                max_lon,
                max_lat,
                stroke_w,
                1,
                true,
            );
        } else {
            let world = if !gq.md_near.is_empty() {
                gq.md_near.as_slice()
            } else {
                gq.md.as_slice()
            };
            paint_rings_globe(
                shapes,
                world,
                view,
                min_lon,
                min_lat,
                max_lon,
                max_lat,
                stroke_w,
                1,
                true,
            );
            paint_cn_jp_admin_globe(shapes, basemap, view, style, 1);
        }
        return;
    }

    let rings = basemap.world.pick(view.zoom, 4.0, 6.0);
    paint_rings_globe(
        shapes,
        rings,
        view,
        min_lon,
        min_lat,
        max_lon,
        max_lat,
        stroke_w,
        1,
        true,
    );
    paint_cn_jp_admin_globe(shapes, basemap, view, style, 1);
}

/// 球体飞跃/拖拽：世界粗轮廓 + 中日国级（省级仅近景），抽稀保帧率。
fn paint_globe_fly_lite(
    shapes: &mut Vec<egui::Shape>,
    basemap: &VectorBasemap,
    view: &ViewCtx,
    style: VectorStyle,
) {
    let (min_lon, min_lat, max_lon, max_lat) = (-180.0, -90.0, 180.0, 90.0);
    let stroke_w = Stroke::new(1.0_f32, style.world_border);
    let rings: &[RingSet] = if let Some(gq) = basemap.gq.as_ref() {
        if !gq.md.is_empty() {
            gq.md.as_slice()
        } else {
            &[]
        }
    } else if !basemap.world.coarse.is_empty() {
        basemap.world.coarse.as_slice()
    } else {
        basemap.world.pick(view.zoom, 4.0, 6.0)
    };
    // stride=3：投影量约 1/3，飞跃时轮廓仍可读
    paint_rings_globe(
        shapes,
        rings,
        view,
        min_lon,
        min_lat,
        max_lon,
        max_lat,
        stroke_w,
        3,
        true,
    );
    // 要石世界层不含中日；GQ 远景 md 已含完整国界时不再叠，以免双描
    let gq_far_md = basemap.gq.as_ref().is_some_and(|g| !g.md.is_empty())
        && zoom_to_scroll(view.zoom) >= 0.5;
    if !gq_far_md {
        paint_cn_jp_admin_globe(shapes, basemap, view, style, 3);
    }
}

/// 球体：只描可见轮廓段（穿地平线断开，不闭合、不填色、不加弓弧）。
fn paint_rings_globe(
    shapes: &mut Vec<egui::Shape>,
    rings: &[RingSet],
    view: &ViewCtx,
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    stroke: Stroke,
    stride: usize,
    omit_zangnan: bool,
) {
    let stride = stride.max(1);
    for ring in rings {
        if !ring_visible(ring, min_lon, min_lat, max_lon, max_lat) {
            continue;
        }
        for seg in project_globe_outline_segments(&ring.exterior, view, stride, omit_zangnan) {
            if seg.len() < 2 {
                continue;
            }
            shapes.push(
                egui::epaint::PathShape {
                    points: seg,
                    closed: false,
                    fill: Color32::TRANSPARENT,
                    stroke: egui::epaint::PathStroke::new(stroke.width, stroke.color),
                }
                .into(),
            );
        }
    }
}

/// 将多边形外环按地平线可见性拆成多段折线；段内不做穿海连接。
fn project_globe_outline_segments(
    exterior: &[[f32; 2]],
    view: &ViewCtx,
    stride: usize,
    omit_zangnan: bool,
) -> Vec<Vec<Pos2>> {
    if exterior.len() < 2 || !view.mode.is_globe() {
        return Vec::new();
    }
    let hr = match view.horizon_radius() {
        Some(r) => r,
        None => return Vec::new(),
    };
    let max_jump = (hr * 0.55).max(48.0);
    let stride = stride.max(1);
    let n = exterior.len();
    let mut segs: Vec<Vec<Pos2>> = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    let mut prev_ll: Option<[f32; 2]> = None;

    // 仅当首尾很近（真闭合环）时才绕回起点；开折线绕回会拉出「神秘直线」
    let close_dist = {
        let a = exterior[0];
        let b = exterior[n - 1];
        ((a[0] - b[0]).hypot(a[1] - b[1])) as f64
    };
    let do_close = close_dist < 0.35;
    let steps = (n + stride - 1) / stride;
    let k_end = if do_close { steps } else { steps.saturating_sub(1) };

    for k in 0..=k_end {
        let i = if do_close && k == steps {
            0
        } else {
            (k * stride) % n
        };
        let ll = exterior[i];
        if omit_zangnan {
            // 带内顶点整点跳过，避免断头残段；长弦穿越则只拆段
            if in_zangnan_omit_box(ll) {
                if cur.len() >= 2 {
                    segs.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                prev_ll = None;
                continue;
            }
            if let Some(prev) = prev_ll {
                if edge_hits_zangnan(prev, ll) {
                    if cur.len() >= 2 {
                        segs.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                }
            }
        }
        let lon = ll[0] as f64;
        let lat = ll[1] as f64;
        match view.project(lon, lat) {
            Some(sp) => {
                if let Some(prev) = cur.last().copied() {
                    if (prev - sp).length() > max_jump {
                        if cur.len() >= 2 {
                            segs.push(std::mem::take(&mut cur));
                        } else {
                            cur.clear();
                        }
                    }
                }
                if cur.last().map(|p| (*p - sp).length() > 0.35).unwrap_or(true) {
                    cur.push(sp);
                }
                prev_ll = Some(ll);
            }
            None => {
                if cur.len() >= 2 {
                    segs.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                prev_ll = Some(ll);
            }
        }
    }
    if cur.len() >= 2 {
        segs.push(cur);
    }
    segs
}

fn paint_rings(
    shapes: &mut Vec<egui::Shape>,
    rings: &[RingSet],
    view: &ViewCtx,
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    min_seg_px: f32,
    max_jump_px: f32,
    stroke: Stroke,
    fill: bool,
    draw_holes: bool,
    land_fill: Color32,
    lon_shift: f64,
    omit_zangnan: bool,
) {
    // 先填后描。平面 / 球体均用屏幕 Path 实心填色（对齐 GQ）。
    if fill {
        for ring in rings {
            if !ring.may_fill() {
                continue;
            }
            if !ring_visible(
                ring,
                min_lon - lon_shift,
                min_lat,
                max_lon - lon_shift,
                max_lat,
            ) {
                continue;
            }
            if let Some(pts) =
                project_exterior_path_fill(ring, view, lon_shift, max_jump_px)
            {
                if pts.len() >= 3 {
                    shapes.push(egui::Shape::Path(egui::epaint::PathShape {
                        points: pts,
                        closed: true,
                        fill: land_fill,
                        stroke: egui::epaint::PathStroke::NONE,
                    }));
                }
            }
        }
    }

    for ring in rings {
        // 可见性按未偏移地理范围；副本仅改投影经度
        if !ring_visible(
            ring,
            min_lon - lon_shift,
            min_lat,
            max_lon - lon_shift,
            max_lat,
        ) {
            continue;
        }

        let segments = project_segments(
            &ring.exterior,
            view,
            min_seg_px,
            max_jump_px,
            lon_shift,
            omit_zangnan,
        );
        for pts in segments {
            if pts.len() < 2 {
                continue;
            }
            shapes.push(
                egui::epaint::PathShape {
                    points: pts,
                    closed: false,
                    fill: Color32::TRANSPARENT,
                    stroke: egui::epaint::PathStroke::new(stroke.width, stroke.color),
                }
                .into(),
            );
        }

        if draw_holes {
            for hole in &ring.holes {
                let hsegs = project_segments(
                    hole,
                    view,
                    min_seg_px,
                    max_jump_px,
                    lon_shift,
                    false,
                );
                for hpts in hsegs {
                    if hpts.len() < 2 {
                        continue;
                    }
                    shapes.push(
                        egui::epaint::PathShape {
                            points: hpts,
                            closed: false,
                            fill: Color32::TRANSPARENT,
                            stroke: egui::epaint::PathStroke::new(
                                stroke.width * 0.7,
                                stroke.color.gamma_multiply(0.75),
                            ),
                        }
                        .into(),
                    );
                }
            }
        }
    }
}

/// 经纬度耳切；顶点序 = 外环 + 各孔洞，与 `project_fill_vertices` 一致。
fn triangulate_ring(exterior: &[[f32; 2]], holes: &[Vec<[f32; 2]>]) -> Vec<u32> {
    let mut flat: Vec<f64> = Vec::with_capacity((exterior.len()
        + holes.iter().map(|h| h.len()).sum::<usize>())
        * 2);
    let mut hole_indices: Vec<usize> = Vec::with_capacity(holes.len());
    for p in exterior {
        flat.push(p[0] as f64);
        flat.push(p[1] as f64);
    }
    for hole in holes {
        if hole.len() < 3 {
            continue;
        }
        hole_indices.push(flat.len() / 2);
        for p in hole {
            flat.push(p[0] as f64);
            flat.push(p[1] as f64);
        }
    }
    match earcutr::earcut(&flat, &hole_indices, 2) {
        Ok(idx) => idx.into_iter().map(|i| i as u32).collect(),
        Err(_) => Vec::new(),
    }
}


/// 外环 → 屏幕闭合路径（GQ：投影后再 fill）。孔洞忽略（与 GQ 只取外环一致）。
fn project_exterior_path_fill(
    ring: &RingSet,
    view: &ViewCtx,
    lon_shift: f64,
    max_jump_px: f32,
) -> Option<Vec<Pos2>> {
    if ring.exterior.len() < 3 {
        return None;
    }
    if view.mode.is_globe() {
        // 球体：全部顶点需落在地平线内，且相邻/首尾无大跳，否则跳过填色（只留描边）
        let mut pts = Vec::with_capacity(ring.exterior.len());
        let mut prev: Option<Pos2> = None;
        for p in &ring.exterior {
            let lon = p[0] as f64 + lon_shift;
            let lat = p[1] as f64;
            // 填色用放宽地平线的投影，避免大陆边缘被整环丢弃
            let Some(sp) = view.project_fill(lon, lat) else {
                return None;
            };
            if let Some(q) = prev {
                if (sp - q).length() > max_jump_px {
                    return None;
                }
            }
            pts.push(sp);
            prev = Some(sp);
        }
        if pts.len() < 3 {
            return None;
        }
        if (pts[0] - pts[pts.len() - 1]).length() > max_jump_px {
            return None;
        }
        // 周长异常扭曲才弃填；正常大陆轮廓放行
        let bb = Rect::from_points(&pts);
        let peri: f32 = pts
            .windows(2)
            .map(|w| (w[1] - w[0]).length())
            .sum::<f32>()
            + (pts[0] - pts[pts.len() - 1]).length();
        let diag = bb.width().hypot(bb.height()).max(1.0);
        if peri > diag * 14.0 {
            return None;
        }
        Some(pts)
    } else {
        project_ring_points_fill(&ring.exterior, view, lon_shift)
    }
}

fn project_ring_points_fill(
    ring: &[[f32; 2]],
    view: &ViewCtx,
    lon_shift: f64,
) -> Option<Vec<Pos2>> {
    if ring.len() < 3 {
        return None;
    }

    // 先解缠成连续经度轨迹，再整体加世界副本偏移；禁止对单点 wrap_lon_near，
    // 否则大块陆地在中心移到环「背面」时会被撕成断环并丢弃填色。
    let unwrapped = unwrap_ring(ring);
    if unwrapped.len() != ring.len() {
        return None;
    }
    let zoom = view.zoom;
    let rect = view.rect;
    let (cx, cy) = projection::lonlat_to_world(view.center_lon, view.center_lat, zoom);
    let mut out = Vec::with_capacity(unwrapped.len());
    for p in &unwrapped {
        let lon = p[0] as f64 + lon_shift;
        let lat = p[1] as f64;
        let (wx, wy) = projection::lonlat_to_world(lon, lat, zoom);
        out.push(rect.center() + egui::Vec2::new((wx - cx) as f32, (wy - cy) as f32));
    }
    Some(out)
}

fn ring_visible(ring: &RingSet, min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> bool {
    if (ring.max_lat as f64) < min_lat || (ring.min_lat as f64) > max_lat {
        return false;
    }
    lon_ranges_overlap(
        ring.min_lon as f64,
        ring.max_lon as f64,
        min_lon,
        max_lon,
    )
}

fn lon_ranges_overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> bool {
    for shift in [-360.0, 0.0, 360.0] {
        let lo = a0 + shift;
        let hi = a1 + shift;
        if hi >= b0 && lo <= b1 {
            return true;
        }
    }
    false
}


fn kanameishi_dir(dir: &Path) -> PathBuf {
    dir.join("kanameishi")
}

fn globalquake_dir(dir: &Path) -> PathBuf {
    dir.join("globalquake")
}

fn first_existing(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    for name in names {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn try_load_rings(path: &Path, files: &mut usize, skipped: &mut usize, label: &str) -> Option<Vec<RingSet>> {
    if !path.is_file() {
        return None;
    }
    *files += 1;
    match load_file(path) {
        Ok(rings) => {
            info!(
                path = %path.display(),
                rings = rings.len(),
                label,
                "loaded basemap layer"
            );
            Some(rings)
        }
        Err(e) => {
            *skipped += 1;
            warn!(path = %path.display(), error = %e, label, "底图加载失败");
            None
        }
    }
}

/// World layer: kanameishi fine world (or root GeoJSON fallback).
fn load_world_lods(
    dir: &Path,
    _for_globe: bool,
) -> (LayerLods, usize, usize, usize, Option<GqLayers>) {
    let (world_fine, files, skipped) = load_world_rings_fallback(dir);
    let rings = world_fine.len();
    let lods = LayerLods {
        coarse: world_fine,
        medium: Vec::new(),
        fine: Vec::new(),
    };
    (lods, rings, files, skipped, None)
}

/// 非 GQ 回退：根目录精细 GeoJSON → 要石 Topo。
fn load_world_rings_fallback(dir: &Path) -> (Vec<RingSet>, usize, usize) {
    let mut world_fine = Vec::new();
    let mut files = 0usize;
    let mut skipped = 0usize;

    if let Some(world) = first_existing(
        dir,
        &[
            "medium.global.modified.geo.json",
            "medium.global.modified.geojson",
            "medium.global.geo.json",
        ],
    ) {
        files += 1;
        match load_any_map(&world) {
            Ok(rings) => {
                info!(
                    path = %world.display(),
                    rings = rings.len(),
                    "loaded fine world basemap GeoJSON"
                );
                world_fine.extend(rings);
                append_cn_boundary_patches(dir, &mut world_fine, &mut files, &mut skipped);
                world_fine = strip_zangnan_corridor_from_world(world_fine);
                return (world_fine, files, skipped);
            }
            Err(e) => {
                skipped += 1;
                warn!(
                    path = %world.display(),
                    error = %e,
                    "精细世界底图加载失败，回退要石"
                );
            }
        }
    }

    let kdir = kanameishi_dir(dir);
    if let Some(world) = first_existing(
        &kdir,
        &[
            "medium.global.modified.topo.json",
            "world.geo.json",
            "world.geojson",
        ],
    ) {
        files += 1;
        match load_any_map(&world) {
            Ok(rings) => {
                info!(
                    path = %world.display(),
                    rings = rings.len(),
                    "loaded kanameishi world basemap"
                );
                world_fine.extend(rings);
                append_cn_boundary_patches(dir, &mut world_fine, &mut files, &mut skipped);
                world_fine = strip_zangnan_corridor_from_world(world_fine);
                return (world_fine, files, skipped);
            }
            Err(e) => {
                skipped += 1;
                warn!(path = %world.display(), error = %e, "要石世界底图加载失败");
            }
        }
    }

    world_fine.extend(builtin_diaoyu_patches());
    world_fine = strip_zangnan_corridor_from_world(world_fine);

    (world_fine, files, skipped)
}

/// 世界层只叠钓鱼岛（点状）。藏南不画世界层边，由中国省界/国级补上。
fn append_cn_boundary_patches(
    dir: &Path,
    world_fine: &mut Vec<RingSet>,
    files: &mut usize,
    skipped: &mut usize,
) {
    let _ = dir;
    let diaoyu = builtin_diaoyu_patches();
    if diaoyu.is_empty() {
        *skipped += 1;
        return;
    }
    *files += 1;
    world_fine.extend(diaoyu);
}

/// 仅麦克马洪线山脊一带（偏北）。过宽会切断不丹/尼泊尔边，并留下断头残段。
const ZANGNAN_OMIT_LON0: f32 = 91.4;
const ZANGNAN_OMIT_LON1: f32 = 97.3;
const ZANGNAN_OMIT_LAT0: f32 = 27.55;
const ZANGNAN_OMIT_LAT1: f32 = 29.35;

#[inline]
fn in_zangnan_omit_box(p: [f32; 2]) -> bool {
    p[0] >= ZANGNAN_OMIT_LON0
        && p[0] <= ZANGNAN_OMIT_LON1
        && p[1] >= ZANGNAN_OMIT_LAT0
        && p[1] <= ZANGNAN_OMIT_LAT1
}

/// Liang-Barsky：线段与矩形是否有重叠区间（端点均在盒外时用）。
fn segment_crosses_rect(
    a: [f32; 2],
    b: [f32; 2],
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
) -> bool {
    let mut t0 = 0.0_f32;
    let mut t1 = 1.0_f32;
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let mut clip = |p: f32, q: f32| -> bool {
        if p == 0.0 {
            return q >= 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return false;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return false;
            }
            if r < t1 {
                t1 = r;
            }
        }
        true
    };
    clip(-dx, a[0] - x0)
        && clip(dx, x1 - a[0])
        && clip(-dy, a[1] - y0)
        && clip(dy, y1 - a[1])
        && t1 > t0
}

/// 世界层应跳过的边：端点落入或线段真正穿越麦克马洪盒（不用 AABB+长度误伤邻边）。
fn edge_hits_zangnan(a: [f32; 2], b: [f32; 2]) -> bool {
    if in_zangnan_omit_box(a) || in_zangnan_omit_box(b) {
        return true;
    }
    segment_crosses_rect(
        a,
        b,
        ZANGNAN_OMIT_LON0,
        ZANGNAN_OMIT_LON1,
        ZANGNAN_OMIT_LAT0,
        ZANGNAN_OMIT_LAT1,
    )
}

/// 加载时剥掉世界层麦克马洪走廊边；短残段直接丢弃，缺口由中国国级/省级补。
fn strip_zangnan_corridor_from_world(rings: Vec<RingSet>) -> Vec<RingSet> {
    let mut out = Vec::with_capacity(rings.len());
    let mut cut_rings = 0usize;
    for ring in rings {
        let parts = split_ring_omitting_zangnan(&ring.exterior);
        let unchanged = parts.len() == 1 && parts[0].len() == ring.exterior.len();
        if unchanged {
            out.push(ring);
            continue;
        }
        cut_rings += 1;
        for part in parts {
            if let Some(r) = RingSet::from_parts(part, Vec::new()) {
                out.push(r);
            }
        }
    }
    if cut_rings > 0 {
        info!(
            cut_rings,
            out = out.len(),
            "stripped zangnan/McMahon corridor from world basemap"
        );
    }
    out
}

fn split_ring_omitting_zangnan(ring: &[[f32; 2]]) -> Vec<Vec<[f32; 2]>> {
    if ring.len() < 2 {
        return Vec::new();
    }
    let n = ring.len();
    let closed = {
        let a = ring[0];
        let b = ring[n - 1];
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    };
    let pts: &[[f32; 2]] = if closed && n >= 2 {
        &ring[..n - 1]
    } else {
        ring
    };
    if pts.len() < 2 {
        return Vec::new();
    }

    let mut parts: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();

    let push_pt = |cur: &mut Vec<[f32; 2]>, p: [f32; 2]| {
        if cur
            .last()
            .map(|q| (q[0] - p[0]).abs() > 1e-7 || (q[1] - p[1]).abs() > 1e-7)
            .unwrap_or(true)
        {
            cur.push(p);
        }
    };
    // 断头残段（线头）：少于 3 点不保
    let flush = |parts: &mut Vec<Vec<[f32; 2]>>, cur: &mut Vec<[f32; 2]>| {
        if cur.len() >= 3 {
            parts.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };

    let edge_count = if closed { pts.len() } else { pts.len() - 1 };
    for i in 0..edge_count {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        if in_zangnan_omit_box(a) {
            flush(&mut parts, &mut cur);
            continue;
        }
        push_pt(&mut cur, a);
        if edge_hits_zangnan(a, b) {
            flush(&mut parts, &mut cur);
        }
    }
    if !closed {
        let last = pts[pts.len() - 1];
        if !in_zangnan_omit_box(last) {
            push_pt(&mut cur, last);
        }
    }
    flush(&mut parts, &mut cur);
    parts
}

/// 区划层：仅中日省级（GQ → 要石回退）。
fn load_regional_rings(dir: &Path) -> (Vec<RingSet>, usize, usize) {
    let mut regional_fine = Vec::new();
    let mut files = 0usize;
    let mut skipped = 0usize;

    let gq_cn = globalquake_dir(dir).join("cn-prefectures.geojson");
    if let Some(rings) = try_load_rings(
        &gq_cn,
        &mut files,
        &mut skipped,
        "GlobalQuake cn-prefectures",
    ) {
        regional_fine.extend(rings);
    } else {
        let kdir = kanameishi_dir(dir);
        if let Some(path) = first_existing(
            &kdir,
            &[
                "cn.province.topo.json",
                "cn_province.geo.json",
                "cn_province.geojson",
            ],
        ) {
            files += 1;
            match load_any_map(&path) {
                Ok(rings) => {
                    info!(
                        path = %path.display(),
                        rings = rings.len(),
                        "kanameishi cn province"
                    );
                    regional_fine.extend(rings);
                }
                Err(e) => {
                    skipped += 1;
                    warn!(path = %path.display(), error = %e, "要石中国省界加载失败");
                }
            }
        }
    }

    let jp = load_jp_pref_rings(dir, &mut files, &mut skipped);
    regional_fine.extend(jp);

    info!(
        total = regional_fine.len(),
        files,
        skipped,
        "regional admin rings loaded (provincial only)"
    );
    (regional_fine, files, skipped)
}

/// 远距 national 溶解源：中国优先 GQ（含藏南），日本优先要石/JMA（含北方四岛）。
fn load_coarse_admin_for_dissolve(dir: &Path) -> Vec<RingSet> {
    let mut out = Vec::new();
    let mut files = 0usize;
    let mut skipped = 0usize;
    let gq = globalquake_dir(dir);
    let kdir = kanameishi_dir(dir);

    if let Some(rings) = try_load_rings(
        &gq.join("cn-prefectures.geojson"),
        &mut files,
        &mut skipped,
        "coarse dissolve cn",
    ) {
        out.extend(rings);
    } else if let Some(path) = first_existing(
        &kdir,
        &[
            "cn.province.topo.json",
            "cn_province.geo.json",
            "cn_province.geojson",
        ],
    ) {
        if let Ok(rings) = load_any_map(&path) {
            out.extend(rings);
        }
    }

    // 与 load_jp_pref_rings 同序：要石含国後/択捉/色丹，GQ 日本包没有
    out.extend(load_jp_pref_rings(dir, &mut files, &mut skipped));
    out
}

/// 日本都道府县：要石/JMA → `日本/` → GQ。加载后剥除钓鱼岛范围小岛环。
/// 要石优先：含北方四岛；GQ `jp-prefectures` 缺国後/択捉/色丹，作回退。
fn load_jp_pref_rings(dir: &Path, files: &mut usize, skipped: &mut usize) -> Vec<RingSet> {
    let mut out = Vec::new();

    let kdir = kanameishi_dir(dir);
    if let Some(path) =
        first_existing(&kdir, &["jp.pref.topo.json", "jp_pref.geo.json", "jp_pref.geojson"])
    {
        *files += 1;
        match load_any_map(&path) {
            Ok(rings) => {
                let rings = strip_jp_diaoyu_rings(rings);
                info!(path = %path.display(), rings = rings.len(), "loaded kanameishi jp pref");
                return rings;
            }
            Err(e) => {
                *skipped += 1;
                warn!(path = %path.display(), error = %e, "要石日本区划加载失败");
            }
        }
    }

    let jp_dir = dir.join("日本");
    if jp_dir.is_dir() {
        for entry in walk_geojson(&jp_dir) {
            *files += 1;
            match load_file(&entry) {
                Ok(rings) => out.extend(rings),
                Err(e) => {
                    *skipped += 1;
                    warn!(path = %entry.display(), error = %e, "跳过日本 GeoJSON");
                }
            }
        }
        if !out.is_empty() {
            let out = strip_jp_diaoyu_rings(out);
            info!(rings = out.len(), "loaded jp prefs from 日本/");
            return out;
        }
    }

    let gq_jp = globalquake_dir(dir).join("jp-prefectures.geojson");
    if let Some(rings) = try_load_rings(
        &gq_jp,
        files,
        skipped,
        "loaded GlobalQuake jp-prefectures (fallback)",
    ) {
        return strip_jp_diaoyu_rings(rings);
    }
    out
}

/// 剔除日本区划中落在钓鱼岛附近的小岛环（由中国台湾省界 + 内置点状表达）。
fn strip_jp_diaoyu_rings(rings: Vec<RingSet>) -> Vec<RingSet> {
    // 约 123.2–124.8°E、25.5–26.1°N；只删跨度很小的岛环，不动冲绳本岛等大轮廓
    const LON0: f32 = 123.2;
    const LON1: f32 = 124.8;
    const LAT0: f32 = 25.5;
    const LAT1: f32 = 26.1;
    let before = rings.len();
    let out: Vec<RingSet> = rings
        .into_iter()
        .filter(|r| {
            let cx = (r.min_lon + r.max_lon) * 0.5;
            let cy = (r.min_lat + r.max_lat) * 0.5;
            let small = (r.max_lon - r.min_lon) < 0.5 && (r.max_lat - r.min_lat) < 0.5;
            !(small && cx >= LON0 && cx <= LON1 && cy >= LAT0 && cy <= LAT1)
        })
        .collect();
    let removed = before.saturating_sub(out.len());
    if removed > 0 {
        info!(removed, "stripped Diaoyu islets from Japan prefecture rings");
    }
    out
}




/// 从省界/都道府县溶解出国家外轮廓：只保留出现一次的边（内部省界成对抵消）。
fn dissolve_national_outlines(regions: &[RingSet]) -> Vec<RingSet> {
    if regions.is_empty() {
        return Vec::new();
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    struct QPt(i32, i32);

    fn qpt(p: [f32; 2]) -> QPt {
        QPt(
            (p[0] as f64 * 2000.0).round() as i32,
            (p[1] as f64 * 2000.0).round() as i32,
        )
    }

    // edge_key -> (count, representative endpoints in lon/lat)
    let mut edge_count: HashMap<(QPt, QPt), u32> = HashMap::new();
    let mut edge_geom: HashMap<(QPt, QPt), ([f32; 2], [f32; 2])> = HashMap::new();

    for ring in regions {
        let pts = &ring.exterior;
        if pts.len() < 3 {
            continue;
        }
        let n = pts.len();
        for i in 0..n {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let qa = qpt(a);
            let qb = qpt(b);
            if qa == qb {
                continue;
            }
            let key = if qa <= qb { (qa, qb) } else { (qb, qa) };
            *edge_count.entry(key).or_insert(0) += 1;
            edge_geom.entry(key).or_insert((a, b));
        }
    }

    // 邻接表：只保留 count==1 的外边界边
    let mut adj: HashMap<QPt, Vec<(QPt, [f32; 2], [f32; 2])>> = HashMap::new();
    for (key, &cnt) in &edge_count {
        if cnt != 1 {
            continue;
        }
        let (a, b) = edge_geom[key];
        let qa = key.0;
        let qb = key.1;
        // 按量化点方向存实际坐标
        let (a_pt, b_pt) = if qpt(a) == qa { (a, b) } else { (b, a) };
        adj.entry(qa).or_default().push((qb, a_pt, b_pt));
        adj.entry(qb).or_default().push((qa, b_pt, a_pt));
    }

    let mut used: HashMap<(QPt, QPt), bool> = HashMap::new();
    let mut outlines: Vec<RingSet> = Vec::new();

    let starts: Vec<QPt> = adj.keys().copied().collect();
    for start in starts {
        // 找一条未用边
        let Some(neigh) = adj.get(&start) else {
            continue;
        };
        let mut begin = None;
        for (to, _, _) in neigh {
            let e = if start <= *to {
                (start, *to)
            } else {
                (*to, start)
            };
            if !used.get(&e).copied().unwrap_or(false) {
                begin = Some(*to);
                break;
            }
        }
        let Some(first_to) = begin else {
            continue;
        };

        let mut ring_pts: Vec<[f32; 2]> = Vec::new();
        let mut cur = start;
        let mut next = first_to;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 200_000 {
                break;
            }
            let e = if cur <= next { (cur, next) } else { (next, cur) };
            if used.insert(e, true).is_some() {
                break;
            }
            // 取 cur→next 的实际坐标
            let mut from_lonlat = None;
            let mut to_lonlat = None;
            if let Some(list) = adj.get(&cur) {
                for (to, a, b) in list {
                    if *to == next {
                        from_lonlat = Some(*a);
                        to_lonlat = Some(*b);
                        break;
                    }
                }
            }
            if let Some(a) = from_lonlat {
                if ring_pts.last().map(|p| *p != a).unwrap_or(true) {
                    ring_pts.push(a);
                }
            }
            if let Some(b) = to_lonlat {
                ring_pts.push(b);
            }

            if next == start {
                break;
            }
            // 从 next 找另一条未用边
            let Some(list) = adj.get(&next) else {
                break;
            };
            let mut found = None;
            for (to, _, _) in list {
                let e2 = if next <= *to {
                    (next, *to)
                } else {
                    (*to, next)
                };
                if !used.get(&e2).copied().unwrap_or(false) {
                    found = Some(*to);
                    break;
                }
            }
            let Some(n2) = found else {
                break;
            };
            cur = next;
            next = n2;
        }

        if ring_pts.len() >= 3 {
            // 去闭合重复点
            if ring_pts.len() >= 2 {
                let a = ring_pts[0];
                let b = *ring_pts.last().unwrap();
                if (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5 {
                    ring_pts.pop();
                }
            }
            outlines.extend(ringsets_from_polygon(ring_pts, Vec::new()));
        }
    }

    info!(rings = outlines.len(), "dissolved national outlines from regional");
    outlines
}

fn load_any_map(path: &Path) -> Result<Vec<RingSet>, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.ends_with(".topo.json") || name.ends_with(".topojson") {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let polys = topojson::load_region_polygons(&text)?;
        let mut out = Vec::with_capacity(polys.len());
        for (exterior, holes) in polys {
            out.extend(ringsets_from_polygon(exterior, holes));
        }
        return Ok(out);
    }
    load_file(path)
}

fn walk_geojson(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_geojson_path(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn is_geojson_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let l = name.to_ascii_lowercase();
    l.ends_with(".geo.json")
        || l.ends_with(".geojson")
        || (l.ends_with(".json") && !l.ends_with(".geo.json"))
}

/// 钓鱼岛（点状，可描边）。
fn builtin_diaoyu_patches() -> Vec<RingSet> {
    let diaoyu: &[&[[f32; 2]]] = &[
        &[
            [123.45, 25.73],
            [123.50, 25.73],
            [123.50, 25.76],
            [123.45, 25.76],
            [123.45, 25.73],
        ],
        &[
            [123.67, 25.92],
            [123.70, 25.92],
            [123.70, 25.94],
            [123.67, 25.94],
            [123.67, 25.92],
        ],
        &[
            [124.55, 25.91],
            [124.57, 25.91],
            [124.57, 25.93],
            [124.55, 25.93],
            [124.55, 25.91],
        ],
    ];
    let mut out = Vec::new();
    for island in diaoyu {
        if let Some(r) = ringset_from_lonlat(island) {
            out.push(r);
        }
    }
    out
}

fn ringset_from_lonlat(pts: &[[f32; 2]]) -> Option<RingSet> {
    if pts.len() < 3 {
        return None;
    }
    let mut exterior: Vec<[f32; 2]> = pts.to_vec();
    if exterior.len() >= 2 {
        let a = exterior[0];
        let b = exterior[exterior.len() - 1];
        if (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6 {
            exterior.pop();
        }
    }
    ringsets_from_polygon(exterior, Vec::new()).into_iter().next()
}

#[derive(Deserialize)]
struct FeatureCollection {
    features: Vec<Feature>,
}

#[derive(Deserialize)]
struct Feature {
    #[serde(default)]
    properties: Option<serde_json::Value>,
    geometry: Option<Geometry>,
}

#[derive(Deserialize)]
struct Geometry {
    #[serde(rename = "type")]
    geom_type: String,
    coordinates: serde_json::Value,
}

fn feature_name(props: &Option<serde_json::Value>) -> Option<String> {
    let props = props.as_ref()?;
    for key in [
        "name_long",
        "name",
        "NAME_2",
        "NAME_1",
        "NAME",
        "name_l",
        "admin",
    ] {
        if let Some(s) = props.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn load_file(path: &Path) -> Result<Vec<RingSet>, String> {
    load_file_excluding(path, &[])
}

/// 对齐 GQ `parseGeoJson(..., remove)`：按名称跳过要素（如近景过滤 China/Japan）。
fn load_file_excluding(path: &Path, exclude: &[&str]) -> Result<Vec<RingSet>, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let fc: FeatureCollection = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for feat in fc.features {
        if !exclude.is_empty() {
            if let Some(name) = feature_name(&feat.properties) {
                if exclude
                    .iter()
                    .any(|e| name.eq_ignore_ascii_case(e) || name == *e)
                {
                    continue;
                }
            }
        }
        let Some(geom) = feat.geometry else {
            continue;
        };
        match geom.geom_type.as_str() {
            "Polygon" => out.extend(polygon_ringsets(&geom.coordinates)),
            "MultiPolygon" => {
                let Some(arr) = geom.coordinates.as_array() else {
                    continue;
                };
                for poly in arr {
                    out.extend(polygon_ringsets(poly));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn polygon_ringsets(coords: &serde_json::Value) -> Vec<RingSet> {
    let Some(rings) = coords.as_array() else {
        return Vec::new();
    };
    if rings.is_empty() {
        return Vec::new();
    }
    let Some(exterior) = parse_ring(&rings[0]) else {
        return Vec::new();
    };
    if exterior.len() < 3 {
        return Vec::new();
    }
    let mut holes = Vec::new();
    for hole in rings.iter().skip(1) {
        if let Some(h) = parse_ring(hole) {
            if h.len() >= 3 {
                holes.push(h);
            }
        }
    }
    ringsets_from_polygon(exterior, holes)
}

/// 将多边形沿日界线切开，得到可安全投影/填色的若干环。
fn ringsets_from_polygon(exterior: Vec<[f32; 2]>, holes: Vec<Vec<[f32; 2]>>) -> Vec<RingSet> {
    let ext_parts = split_closed_ring(&exterior);
    if ext_parts.is_empty() {
        return Vec::new();
    }
    let hole_parts: Vec<Vec<[f32; 2]>> = holes
        .iter()
        .flat_map(|h| split_closed_ring(h))
        .filter(|h| h.len() >= 3 && unwrapped_lon_span(h) <= 180.0 + 1e-3)
        .collect();

    if ext_parts.len() == 1 {
        return RingSet::from_parts(ext_parts.into_iter().next().unwrap(), hole_parts)
            .into_iter()
            .collect();
    }

    // 多片外环：孔洞不做归属（避免错挂），各自独立
    ext_parts
        .into_iter()
        .filter_map(|e| RingSet::from_parts(e, Vec::new()))
        .collect()
}

fn parse_ring(v: &serde_json::Value) -> Option<Vec<[f32; 2]>> {
    let arr = v.as_array()?;
    let mut pts = Vec::with_capacity(arr.len());
    for p in arr {
        let xy = p.as_array()?;
        if xy.len() < 2 {
            continue;
        }
        let lon = xy[0].as_f64()? as f32;
        let lat = xy[1].as_f64()? as f32;
        pts.push([lon, lat]);
    }
    if pts.len() >= 2 {
        let a = pts[0];
        let b = pts[pts.len() - 1];
        if (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6 {
            pts.pop();
        }
    }
    Some(pts)
}

fn bbox_of(pts: &[[f32; 2]]) -> (f32, f32, f32, f32) {
    let mut min_lon = f32::MAX;
    let mut min_lat = f32::MAX;
    let mut max_lon = f32::MIN;
    let mut max_lat = f32::MIN;
    for p in pts {
        min_lon = min_lon.min(p[0]);
        min_lat = min_lat.min(p[1]);
        max_lon = max_lon.max(p[0]);
        max_lat = max_lat.max(p[1]);
    }
    (min_lon, min_lat, max_lon, max_lat)
}

fn simplify_ringsets(src: &RingSet, tol_deg: f64) -> Vec<RingSet> {
    let exterior = douglas_peucker(&src.exterior, tol_deg);
    let holes: Vec<Vec<[f32; 2]>> = src
        .holes
        .iter()
        .map(|h| douglas_peucker(h, tol_deg))
        .filter(|h| h.len() >= 3)
        .collect();
    // 简化可能拉出跨日界弦，再切一次
    ringsets_from_polygon(exterior, holes)
}

fn douglas_peucker(pts: &[[f32; 2]], tol_deg: f64) -> Vec<[f32; 2]> {
    if pts.len() < 3 || tol_deg <= 0.0 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    dp_recurse(pts, 0, pts.len() - 1, tol_deg * tol_deg, &mut keep);
    pts.iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, p)| *p)
        .collect()
}

fn dp_recurse(pts: &[[f32; 2]], i0: usize, i1: usize, tol2: f64, keep: &mut [bool]) {
    if i1 <= i0 + 1 {
        return;
    }
    let a = pts[i0];
    let b = pts[i1];
    let mut max_d2 = 0.0_f64;
    let mut max_i = i0;
    for i in (i0 + 1)..i1 {
        let d2 = dist2_point_seg(pts[i], a, b);
        if d2 > max_d2 {
            max_d2 = d2;
            max_i = i;
        }
    }
    if max_d2 > tol2 {
        keep[max_i] = true;
        dp_recurse(pts, i0, max_i, tol2, keep);
        dp_recurse(pts, max_i, i1, tol2, keep);
    }
}

fn dist2_point_seg(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f64 {
    let px = p[0] as f64;
    let py = p[1] as f64;
    let ax = a[0] as f64;
    let ay = a[1] as f64;
    let bx = b[0] as f64;
    let by = b[1] as f64;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-18 {
        0.0
    } else {
        ((px - ax) * dx + (py - ay) * dy) / len2
    }
    .clamp(0.0, 1.0);
    let qx = ax + t * dx;
    let qy = ay + t * dy;
    let ex = px - qx;
    let ey = py - qy;
    ex * ex + ey * ey
}

fn viewport_bounds(center_lon: f64, center_lat: f64, zoom: f64, rect: Rect) -> (f64, f64, f64, f64) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
    ];
    let mut min_lon = f64::MAX;
    let mut min_lat = f64::MAX;
    let mut max_lon = f64::MIN;
    let mut max_lat = f64::MIN;
    let (cx, cy) = projection::lonlat_to_world(center_lon, center_lat, zoom);
    for c in corners {
        let wx = cx + (c.x - rect.center().x) as f64;
        let wy = cy + (c.y - rect.center().y) as f64;
        let (lon, lat) = projection::world_to_lonlat(wx, wy, zoom);
        min_lon = min_lon.min(lon);
        min_lat = min_lat.min(lat);
        max_lon = max_lon.max(lon);
        max_lat = max_lat.max(lat);
    }
    // 裁剪边距加宽：平移复用 mesh 时边缘新露出的省级岸线仍在缓存里
    let pad_lon = (max_lon - min_lon) * 0.45;
    let pad_lat = (max_lat - min_lat) * 0.45;
    (
        min_lon - pad_lon,
        min_lat - pad_lat,
        max_lon + pad_lon,
        max_lat + pad_lat,
    )
}

/// 相对视口中心连续解缠后投影；经度或屏幕跳变时拆段，禁止跨日界连线。
/// `lon_shift`：世界副本偏移（±360°…），用于低缩放横向铺满；球体忽略。
/// `omit_zangnan`：世界层跳过藏南/麦克马洪线走廊，由中国轮廓补画。
fn project_segments(
    ring: &[[f32; 2]],
    view: &ViewCtx,
    min_seg_px: f32,
    max_jump_px: f32,
    lon_shift: f64,
    omit_zangnan: bool,
) -> Vec<Vec<Pos2>> {
    let min2 = min_seg_px * min_seg_px;
    let jump2 = max_jump_px * max_jump_px;
    let mut segments: Vec<Vec<Pos2>> = Vec::new();
    let mut cur: Vec<Pos2> = Vec::with_capacity(ring.len().min(256));
    let mut prev_ll: Option<[f32; 2]> = None;

    let flush = |segments: &mut Vec<Vec<Pos2>>, cur: &mut Vec<Pos2>| {
        if cur.len() >= 2 {
            segments.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };

    if view.mode.is_globe() {
        for &p in ring {
            if omit_zangnan {
                if in_zangnan_omit_box(p) {
                    flush(&mut segments, &mut cur);
                    prev_ll = None;
                    continue;
                }
                if let Some(prev) = prev_ll {
                    if edge_hits_zangnan(prev, p) {
                        flush(&mut segments, &mut cur);
                    }
                }
            }
            match view.project(p[0] as f64, p[1] as f64) {
                Some(sp) => {
                    if let Some(last) = cur.last() {
                        let dx = sp.x - last.x;
                        let dy = sp.y - last.y;
                        let d2 = dx * dx + dy * dy;
                        if d2 < min2 {
                            prev_ll = Some(p);
                            continue;
                        }
                        if d2 > jump2 {
                            flush(&mut segments, &mut cur);
                        }
                    }
                    cur.push(sp);
                    prev_ll = Some(p);
                }
                None => {
                    flush(&mut segments, &mut cur);
                    prev_ll = Some(p);
                }
            }
        }
        if cur.len() >= 2 {
            segments.push(cur);
        }
        return segments;
    }

    let center_lon = view.center_lon;
    let center_lat = view.center_lat;
    let zoom = view.zoom;
    let rect = view.rect;
    let (cx, cy) = projection::lonlat_to_world(center_lon, center_lat, zoom);
    let mut prev_lon: Option<f64> = None;

    for &p in ring {
        if omit_zangnan {
            if in_zangnan_omit_box(p) {
                flush(&mut segments, &mut cur);
                prev_ll = None;
                prev_lon = None;
                continue;
            }
            if let Some(prev) = prev_ll {
                if edge_hits_zangnan(prev, p) {
                    flush(&mut segments, &mut cur);
                    prev_lon = None;
                }
            }
        }
        let lat = p[1] as f64;
        let mut lon = projection::wrap_lon_near(p[0] as f64, center_lon) + lon_shift;
        if let Some(prev) = prev_lon {
            while lon - prev > 180.0 {
                lon -= 360.0;
            }
            while prev - lon > 180.0 {
                lon += 360.0;
            }
            if (lon - prev).abs() > 180.0 {
                flush(&mut segments, &mut cur);
                lon = projection::wrap_lon_near(p[0] as f64, center_lon) + lon_shift;
            }
        }

        let (wx, wy) = projection::lonlat_to_world(lon, lat, zoom);
        let sp = rect.center()
            + egui::Vec2::new((wx - cx) as f32, (wy - cy) as f32);

        if let Some(last) = cur.last() {
            let dx = sp.x - last.x;
            let dy = sp.y - last.y;
            let d2 = dx * dx + dy * dy;
            if d2 < min2 {
                prev_lon = Some(lon);
                prev_ll = Some(p);
                continue;
            }
            if d2 > jump2 {
                flush(&mut segments, &mut cur);
            }
        }
        cur.push(sp);
        prev_lon = Some(lon);
        prev_ll = Some(p);
    }
    if cur.len() >= 2 {
        segments.push(cur);
    }
    segments
}

/// 经度归一到 (-180, 180]。
fn norm_lon_f32(lon: f32) -> f32 {
    let mut x = (lon as f64 + 180.0).rem_euclid(360.0) - 180.0;
    if x <= -180.0 {
        x = 180.0;
    }
    x as f32
}

/// 是否存在跨日界边。用原始经度差；±180 之间的缝边不算跨越。
fn has_antimeridian_edge(pts: &[[f32; 2]]) -> bool {
    if pts.len() < 2 {
        return false;
    }
    let n = pts.len();
    for i in 0..n {
        let a = pts[i][0];
        let b = pts[(i + 1) % n][0];
        if a.abs() >= 179.999 && b.abs() >= 179.999 {
            continue;
        }
        if (a as f64 - b as f64).abs() > 180.0 {
            return true;
        }
    }
    false
}

/// 将环上经度解缠为连续轨迹（相邻点 |Δlon| ≤ 180°）。
fn unwrap_ring(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if pts.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(pts.len());
    let mut prev = norm_lon_f32(pts[0][0]);
    out.push([prev, pts[0][1]]);
    for p in pts.iter().skip(1) {
        let mut lon = norm_lon_f32(p[0]);
        while (lon as f64) - (prev as f64) > 180.0 {
            lon -= 360.0;
        }
        while (prev as f64) - (lon as f64) > 180.0 {
            lon += 360.0;
        }
        out.push([lon, p[1]]);
        prev = lon;
    }
    out
}

fn unwrapped_lon_span(pts: &[[f32; 2]]) -> f32 {
    if pts.is_empty() {
        return 0.0;
    }
    let u = unwrap_ring(pts);
    let mut min_lon = f32::MAX;
    let mut max_lon = f32::MIN;
    for p in &u {
        min_lon = min_lon.min(p[0]);
        max_lon = max_lon.max(p[0]);
    }
    max_lon - min_lon
}

/// 在跨日界边上插入 (180,lat)/(-180,lat) 对并拆环；绕地过半的环直接丢弃。
fn split_closed_ring(pts: &[[f32; 2]]) -> Vec<Vec<[f32; 2]>> {
    if pts.len() < 3 {
        return Vec::new();
    }

    let crosses = has_antimeridian_edge(pts);
    let out_norm: Vec<[f32; 2]> = pts.iter().map(|p| [norm_lon_f32(p[0]), p[1]]).collect();

    // 无跨日界边，或绕地大环（南极式）：整环保留，paint 侧拆段描边
    if !crosses || unwrapped_lon_span(pts) > 180.0 + 1e-3 {
        return vec![out_norm];
    }


    let n = pts.len();
    let mut pieces: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();

    for i in 0..n {
        let a = [norm_lon_f32(pts[i][0]), pts[i][1]];
        let b = [norm_lon_f32(pts[(i + 1) % n][0]), pts[(i + 1) % n][1]];
        if cur.is_empty() {
            cur.push(a);
        }
        if (a[0] as f64 - b[0] as f64).abs() > 180.0 {
            let (cut_a, cut_b) = dateline_pair(a, b);
            cur.push(cut_a);
            if cur.len() >= 3 {
                pieces.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
            cur.push(cut_b);
            if i + 1 < n {
                cur.push(b);
            }
        } else if i + 1 < n {
            cur.push(b);
        }
    }
    if cur.len() >= 3 {
        pieces.push(cur);
    }

    // 环起点落在某一侧中部时，首尾两段同属一侧，可安全拼回
    if pieces.len() >= 2 {
        let mut merged = pieces[pieces.len() - 1].clone();
        let head = pieces[0].clone();
        merged.extend(head.iter().skip(1).copied());
        if merged.len() >= 3
            && unwrapped_lon_span(&merged) <= 180.0 + 1e-3
            && !has_antimeridian_edge(&merged)
        {
            pieces.pop();
            pieces.remove(0);
            pieces.insert(0, merged);
        }
    }

    pieces
        .into_iter()
        .filter(|p| p.len() >= 3)
        .filter(|p| unwrapped_lon_span(p) <= 180.0 + 1e-3)
        .filter(|p| !has_antimeridian_edge(p))
        .collect()
}

/// 跨日界边的两端缝点：西侧片用 +180，东侧片用 -180，保证片内 |Δlon|≤180。
fn dateline_pair(a: [f32; 2], b: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    // 在解缠空间插值纬度
    let mut a_lon = a[0];
    let mut b_lon = b[0];
    if a_lon - b_lon > 180.0 {
        b_lon += 360.0;
    } else if b_lon - a_lon > 180.0 {
        a_lon += 360.0;
    }
    // 目标缝：落在 a_lon→b_lon 之间的 ±180+360k
    let seam = if a_lon < b_lon {
        (a_lon / 360.0).ceil() * 360.0 - 180.0
    } else {
        (b_lon / 360.0).ceil() * 360.0 - 180.0
    };
    let t = if (b_lon - a_lon).abs() < 1e-9 {
        0.5
    } else {
        ((seam - a_lon) / (b_lon - a_lon)).clamp(0.0, 1.0)
    };
    let lat = a[1] + t * (b[1] - a[1]);
    if a[0] > 0.0 {
        ([180.0, lat], [-180.0, lat])
    } else {
        ([-180.0, lat], [180.0, lat])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basemap_mode_parse() {
        assert_eq!(BasemapMode::parse("vector"), BasemapMode::Vector);
        assert_eq!(BasemapMode::parse("TILES"), BasemapMode::Tiles);
    }

    #[test]
    fn zangnan_omit_detects_corridor_and_long_chord() {
        assert!(in_zangnan_omit_box([94.0, 28.0]));
        assert!(!in_zangnan_omit_box([90.0, 28.0]));
        // 中国主张南界附近不应整带省略
        assert!(!in_zangnan_omit_box([93.0, 26.9]));
        assert!(edge_hits_zangnan([91.5, 27.6], [97.0, 29.0]));
        assert!(edge_hits_zangnan([90.5, 27.0], [98.5, 29.5]));
        // 走廊南侧的长边不应被 AABB 误伤
        assert!(!edge_hits_zangnan([91.0, 26.5], [97.0, 26.8]));
        assert!(!edge_hits_zangnan([120.0, 30.0], [121.0, 31.0]));
    }

    #[test]
    fn strip_zangnan_drops_corridor_and_stubs() {
        let ring = [
            [90.0, 28.0],
            [91.0, 28.0],
            [92.0, 28.2], // in corridor
            [94.0, 28.5], // in corridor
            [96.0, 28.0], // in corridor
            [98.0, 28.0],
            [98.0, 30.0],
            [90.0, 30.0],
            [90.0, 28.0],
        ];
        let parts = split_ring_omitting_zangnan(&ring);
        assert!(!parts.is_empty());
        for p in &parts {
            assert!(p.iter().all(|pt| !in_zangnan_omit_box(*pt)));
            assert!(p.len() >= 3);
        }
    }

    #[test]
    fn strip_jp_diaoyu_removes_small_islet_keeps_okinawa() {
        let diaoyu = RingSet::from_parts(
            vec![
                [123.46, 25.73],
                [123.48, 25.73],
                [123.48, 25.75],
                [123.46, 25.75],
            ],
            Vec::new(),
        )
        .expect("diaoyu");
        let okinawa = RingSet::from_parts(
            vec![
                [127.5, 26.0],
                [128.5, 26.0],
                [128.5, 27.0],
                [127.5, 27.0],
            ],
            Vec::new(),
        )
        .expect("okinawa");
        let out = strip_jp_diaoyu_rings(vec![diaoyu, okinawa]);
        assert_eq!(out.len(), 1);
        assert!((out[0].min_lon - 127.5).abs() < 0.01);
    }

    #[test]
    fn split_fiji_like_ring_across_dateline() {
        // 跨日界线的小环（类似斐济）：应切成两片，且均可填色
        let ring = [
            [179.0, -16.0],
            [179.5, -16.0],
            [179.5, -17.0],
            [-179.5, -17.0],
            [-179.5, -16.0],
            [-179.0, -16.0],
            [179.0, -16.0],
        ];
        let mut pts = ring.to_vec();
        pts.pop(); // 去闭合点
        assert!(has_antimeridian_edge(&pts), "fixture must cross dateline");
        let parts = split_closed_ring(&pts);
        assert!(
            parts.len() >= 2,
            "expected split, got {} parts: {:?}",
            parts.len(),
            parts
        );
        for p in &parts {
            assert!(p.len() >= 3);
            assert!(unwrapped_lon_span(p) <= 180.0 + 1e-2);
            assert!(!has_antimeridian_edge(p));
        }
        let sets = ringsets_from_polygon(pts, Vec::new());
        assert!(!sets.is_empty());
        assert!(sets.iter().all(|s| s.may_fill()));
    }

    #[test]
    fn keep_full_world_spanning_ring_for_stroke() {
        // 跨度 >180° 但无跨日界边（南极式）→ 保留供描边
        let mut pts = Vec::new();
        for i in 0..36 {
            let lon = -180.0 + i as f32 * 10.0;
            pts.push([lon, -80.0]);
        }
        let parts = split_closed_ring(&pts);
        assert_eq!(parts.len(), 1);
        assert!(RingSet::from_parts(parts[0].clone(), Vec::new()).is_some());
    }

    #[test]
    fn plain_ring_unchanged() {
        let pts = vec![
            [100.0, 20.0],
            [110.0, 20.0],
            [110.0, 30.0],
            [100.0, 30.0],
        ];
        let parts = split_closed_ring(&pts);
        assert_eq!(parts.len(), 1);
        let set = RingSet::from_parts(parts[0].clone(), Vec::new()).unwrap();
        assert!(set.may_fill());
        // 运行时不再预剖分；耳切由 triangulate_ring 单测覆盖
        let tris = triangulate_ring(&set.exterior, &set.holes);
        assert_eq!(tris.len(), 6); // 四边形 → 2 个三角形
    }

    #[test]
    fn triangulate_ring_with_hole() {
        let exterior = vec![
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
        ];
        let hole = vec![
            [2.0, 2.0],
            [2.0, 8.0],
            [8.0, 8.0],
            [8.0, 2.0],
        ];
        let tris = triangulate_ring(&exterior, &[hole]);
        assert!(tris.len() >= 12); // 外环+孔至少若干三角形
        assert!(tris.len() % 3 == 0);
        assert!(tris.iter().all(|&i| i < 8));
    }
}
