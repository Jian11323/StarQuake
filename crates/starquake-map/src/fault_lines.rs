//! 活动断层迹线：全球 GEM + 中国 CAFD（CN-faults），平面 / 球体同绘。
//!
//! 性能：
//! - 数据 LOD（lod0 长断层 / lod1 中等 / lod2 短线）按缩放选用
//! - 空间网格 + 预算/顶点上限
//! - 平面跟焦/拖拽：缓存 Mesh **缩放+平移**跟手；漂移过大再后台精确重投
//! - 交互中不取消进行中的重建，避免连取消导致闪烁与主线程卡顿

use crate::projection::{self, ViewCtx};
use crate::PaintQuality;
use egui::{Color32, Context, Mesh, Pos2, Ui, Vec2};
use serde::Deserialize;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use tracing::{info, warn};

const GRID_CELL_DEG: f32 = 5.0;
const MAX_PAINT_SEGS: usize = 4800;
const CHINA_RESERVE: usize = 700;
const MAX_MESH_VERTICES: usize = 32_000;

/// 与 `tools/prep_faults.py` 对齐
const LOD0_MIN_DIAG: f32 = 1.0;
const LOD1_MIN_DIAG: f32 = 0.28;
/// 高滑移（板缘）即便几何短，也进 lod0，避免日本海沟等在远距被丢掉
const LOD0_MIN_SLIP: f32 = 5.0;

/// 中国及邻区粗包围（度）；东界收至 135°，避免占掉日本 GEM 预算
const CHINA_BBOX: [f32; 4] = [70.0, 15.0, 135.0, 54.0];

/// 相对上次全量裁剪中心，平面平移超过该像素则后台重裁剪
const CULL_REBUILD_PX: f32 = 96.0;
/// 相对 build_zoom 超过该差值才排队精确重建（跟焦慢缩勿每帧换任务）
const CULL_REBUILD_ZOOM: f64 = 0.45;
/// 仿射跟手允许的最大 |dz|（相对 build）；更大则催后台，仍继续变换当前 mesh
const XFORM_MAX_ZOOM_DELTA: f64 = 1.15;

#[derive(Clone)]
struct FaultSeg {
    bbox: [f32; 4],
    diag: f32,
    /// GEM `net_slip_rate`（mm/yr）；缺失为 0，线宽回退到 diag。
    slip_rate: f32,
    line: Vec<[f32; 2]>,
}

#[derive(Default, Clone)]
struct SegLayer {
    segs: Vec<FaultSeg>,
    grid: HashMap<(i16, i16), Vec<u32>>,
}

/// 互斥三档：lod0 长 / lod1 中 / lod2 短
#[derive(Default, Clone)]
struct FaultLods {
    lod0: SegLayer,
    lod1: SegLayer,
    lod2: SegLayer,
}

struct PaintCache {
    key: u64,
    base_key: u64,
    /// 上次**全量重建**时的中心/缩放（用于判断是否需重裁剪）
    build_lon: f64,
    build_lat: f64,
    build_zoom: f64,
    /// 当前 Mesh 已对齐到的视角（缩放/平移变换后更新）
    mesh_lon: f64,
    mesh_lat: f64,
    mesh_zoom: f64,
    mesh: Arc<Mesh>,
}

struct RebuildJob {
    gen: u64,
    key: u64,
    base_key: u64,
    build_lon: f64,
    build_lat: f64,
    build_zoom: f64,
    mesh: Mesh,
}

#[derive(Default)]
pub struct FaultLines {
    global: Arc<FaultLods>,
    china: Arc<FaultLods>,
    loaded: bool,
    paint_cache: Option<PaintCache>,
    rebuild_gen: u64,
    rebuild_pending: bool,
    /// 正在后台重建的目标 paint_key（用于去重/作废）
    rebuild_target_key: Option<u64>,
    rebuild_rx: Option<Receiver<RebuildJob>>,
    /// 交互中跳过了换任务：当前任务完成后需再排一趟
    rebuild_wanted: bool,
    /// LOD 滞回：避免缩放跨阈值时小断层反复显隐
    lod1_on: bool,
    lod2_on: bool,
}

#[derive(Deserialize)]
struct Fc {
    features: Vec<Feat>,
}

#[derive(Deserialize)]
struct Feat {
    #[serde(default)]
    geometry: Option<Geom>,
    #[serde(default)]
    properties: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Geom {
    #[serde(rename = "type")]
    typ: String,
    coordinates: serde_json::Value,
}

impl FaultLines {
    /// 关闭断层时释放几何，避免常驻占用。
    pub fn clear(&mut self) {
        self.global = Arc::new(FaultLods::default());
        self.china = Arc::new(FaultLods::default());
        self.loaded = false;
        self.paint_cache = None;
        self.rebuild_gen = self.rebuild_gen.wrapping_add(1);
        self.rebuild_pending = false;
        self.rebuild_target_key = None;
        self.rebuild_rx = None;
        self.rebuild_wanted = false;
        self.lod1_on = false;
        self.lod2_on = false;
    }

    pub fn load_dir(&mut self, dir: &Path) {
        if self.loaded {
            return;
        }
        let faults = dir.join("faults");
        self.global = Arc::new(load_lods(
            &faults,
            "global_active_faults",
            "global faults",
        ));
        self.china = Arc::new(load_lods(
            &faults,
            "china_active_faults",
            "china faults",
        ));
        self.loaded = true;
        self.paint_cache = None;
        self.rebuild_gen = self.rebuild_gen.wrapping_add(1);
        self.rebuild_pending = false;
        self.rebuild_target_key = None;
        self.rebuild_rx = None;
        self.rebuild_wanted = false;
    }

    /// 飞跃落地：保留旧 mesh 跟手，只作废后台任务并标记需要精确重建（避免闪没+主线程卡顿）。
    pub fn invalidate_paint_cache(&mut self) {
        self.rebuild_gen = self.rebuild_gen.wrapping_add(1);
        self.rebuild_pending = false;
        self.rebuild_target_key = None;
        self.rebuild_rx = None;
        self.rebuild_wanted = true;
    }

    /// 静止帧可复用的断层 mesh（含降质临时图；满质后台完成后由 poll 清 still_static）。
    pub fn still_mesh(&self) -> Option<Arc<Mesh>> {
        self.paint_cache.as_ref().map(|c| c.mesh.clone())
    }

    /// 收取后台重建结果；有更新返回 true。
    pub fn poll(&mut self, ctx: &Context) -> bool {
        let Some(rx) = self.rebuild_rx.as_ref() else {
            return false;
        };
        let mut got = false;
        while let Ok(job) = rx.try_recv() {
            if job.gen != self.rebuild_gen {
                continue;
            }
            let mesh = Arc::new(job.mesh);
            self.paint_cache = Some(PaintCache {
                key: job.key,
                base_key: job.base_key,
                build_lon: job.build_lon,
                build_lat: job.build_lat,
                build_zoom: job.build_zoom,
                mesh_lon: job.build_lon,
                mesh_lat: job.build_lat,
                mesh_zoom: job.build_zoom,
                mesh,
            });
            self.rebuild_pending = false;
            self.rebuild_target_key = None;
            got = true;
        }
        if got {
            crate::request_repaint_after_capped(ctx, 33);
        }
        got
    }

    /// `interacting`：拖拽/滚轮/飞跃/S 波跟焦为 true。
    pub fn paint(
        &mut self,
        ui: &mut Ui,
        view: &ViewCtx,
        dark: bool,
        quality: PaintQuality,
        interacting: bool,
    ) {
        if self.is_empty() {
            return;
        }
        let _ = self.poll(ui.ctx());

        let ppp = ui.ctx().pixels_per_point();
        let globe = view.mode.is_globe();
        update_lod_sticky(view.zoom, &mut self.lod1_on, &mut self.lod2_on);
        let lod_mask = [true, self.lod1_on, self.lod2_on];
        let lookup_key = paint_key(view, dark, ppp, quality, interacting);
        let lookup_base = paint_base_key(view, dark, ppp, quality);

        // —— 球体：静止精确贴合；交互只画旧图+节流后台（不可仿射） ——
        if globe {
            if let Some(c) = self.paint_cache.as_ref() {
                if c.key == lookup_key {
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                    return;
                }
            }
            if interacting {
                if let Some(c) = self.paint_cache.as_ref() {
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                }
                self.request_rebuild(view, dark, quality, lookup_key, lookup_base, lod_mask, true);
                return;
            }
            // 落地后有缓存：先画旧图并后台重建，禁止同步全量卡住镜头
            if self.paint_cache.is_some() {
                if let Some(c) = self.paint_cache.as_ref() {
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                }
                self.request_rebuild(view, dark, quality, lookup_key, lookup_base, lod_mask, false);
                return;
            }
            let q = fault_interact_quality(quality);
            let key = paint_key(view, dark, ppp, q, false);
            let base_key = paint_base_key(view, dark, ppp, q);
            let mesh = Arc::new(build_fault_mesh(
                &self.global,
                &self.china,
                view,
                dark,
                q,
                /* interact_expand */ false,
                lod_mask,
            ));
            ui.painter_at(view.rect)
                .add(egui::Shape::Mesh(mesh.clone()));
            self.paint_cache = Some(PaintCache {
                key,
                base_key,
                build_lon: view.center_lon,
                build_lat: view.center_lat,
                build_zoom: view.zoom,
                mesh_lon: view.center_lon,
                mesh_lat: view.center_lat,
                mesh_zoom: view.zoom,
                mesh,
            });
            if q != quality {
                self.request_rebuild(view, dark, quality, lookup_key, lookup_base, lod_mask, false);
            }
            return;
        }

        // —— 平面：缩放+平移跟手（与矢量 still 同思路）；漂移过大再精确重投 ——
        let can_xform = self.paint_cache.as_ref().is_some_and(|c| {
            c.base_key == lookup_base
                && (c.build_zoom - view.zoom).abs() <= XFORM_MAX_ZOOM_DELTA
        });
        if can_xform {
            let mut ok = false;
            if let Some(c) = self.paint_cache.as_mut() {
                if apply_fault_mesh_view_delta(c, view) {
                    c.key = lookup_key;
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                    ok = true;
                } else {
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                }
            }
            if ok {
                let need_cull = self.paint_cache.as_ref().is_some_and(|c| {
                    fault_needs_cull_rebuild(c, view)
                }) || self.rebuild_wanted;
                if need_cull {
                    self.request_rebuild(
                        view,
                        dark,
                        quality,
                        lookup_key,
                        lookup_base,
                        lod_mask,
                        interacting,
                    );
                }
            } else {
                self.request_rebuild(
                    view,
                    dark,
                    quality,
                    lookup_key,
                    lookup_base,
                    lod_mask,
                    interacting,
                );
            }
            return;
        }

        if let Some(c) = self.paint_cache.as_ref() {
            if c.key == lookup_key {
                ui.painter_at(view.rect)
                    .add(egui::Shape::Mesh(c.mesh.clone()));
                return;
            }
        }

        // 漂移过大：继续仿射跟手，并催后台精确重投（禁止清空闪没）
        if let Some(c) = self.paint_cache.as_mut() {
            if apply_fault_mesh_view_delta(c, view) {
                c.key = lookup_key;
                ui.painter_at(view.rect)
                    .add(egui::Shape::Mesh(c.mesh.clone()));
                self.request_rebuild(
                    view,
                    dark,
                    quality,
                    lookup_key,
                    lookup_base,
                    lod_mask,
                    interacting,
                );
                return;
            }
            ui.painter_at(view.rect)
                .add(egui::Shape::Mesh(c.mesh.clone()));
            self.request_rebuild(
                view,
                dark,
                quality,
                lookup_key,
                lookup_base,
                lod_mask,
                interacting,
            );
            return;
        }

        // 无缓存：前台用「断层交互档」，再后台刷满质（仅冷启动）
        let q = fault_interact_quality(quality);
        let key = paint_key(view, dark, ppp, q, interacting);
        let base_key = paint_base_key(view, dark, ppp, q);
        let mesh = Arc::new(build_fault_mesh(
            &self.global,
            &self.china,
            view,
            dark,
            q,
            interacting,
            lod_mask,
        ));
        ui.painter_at(view.rect)
            .add(egui::Shape::Mesh(mesh.clone()));
        self.paint_cache = Some(PaintCache {
            key,
            base_key,
            build_lon: view.center_lon,
            build_lat: view.center_lat,
            build_zoom: view.zoom,
            mesh_lon: view.center_lon,
            mesh_lat: view.center_lat,
            mesh_zoom: view.zoom,
            mesh,
        });
        if !interacting && q != quality {
            self.request_rebuild(view, dark, quality, lookup_key, lookup_base, lod_mask, false);
        }
    }

    fn is_empty(&self) -> bool {
        self.global.total_segs() == 0 && self.china.total_segs() == 0
    }

    /// 排队精确重建。`coalesce`：已有任务则只记 wanted，不取消（跟焦关键）。
    fn request_rebuild(
        &mut self,
        view: &ViewCtx,
        dark: bool,
        quality: PaintQuality,
        key: u64,
        base_key: u64,
        lod_mask: [bool; 3],
        coalesce: bool,
    ) {
        if self.rebuild_pending && self.rebuild_target_key == Some(key) {
            self.rebuild_wanted = false;
            return;
        }
        if self.rebuild_pending && coalesce {
            self.rebuild_wanted = true;
            return;
        }
        self.rebuild_wanted = false;
        self.spawn_rebuild(view, dark, quality, key, base_key, lod_mask);
    }

    fn spawn_rebuild(
        &mut self,
        view: &ViewCtx,
        dark: bool,
        quality: PaintQuality,
        key: u64,
        base_key: u64,
        lod_mask: [bool; 3],
    ) {
        self.rebuild_gen = self.rebuild_gen.wrapping_add(1);
        let gen = self.rebuild_gen;
        let global = Arc::clone(&self.global);
        let china = Arc::clone(&self.china);
        let mode = view.mode;
        let lon = view.center_lon;
        let lat = view.center_lat;
        let zoom = view.zoom;
        let rect = view.rect;
        let (tx, rx) = mpsc::channel();
        match thread::Builder::new()
            .name("starquake-faults".into())
            .spawn(move || {
                let view = ViewCtx::new(mode, lon, lat, zoom, rect);
                let mesh = build_fault_mesh(
                    &global,
                    &china,
                    &view,
                    dark,
                    quality,
                    false,
                    lod_mask,
                );
                let _ = tx.send(RebuildJob {
                    gen,
                    key,
                    base_key,
                    build_lon: lon,
                    build_lat: lat,
                    build_zoom: zoom,
                    mesh,
                });
            }) {
            Ok(_) => {
                self.rebuild_rx = Some(rx);
                self.rebuild_pending = true;
                self.rebuild_target_key = Some(key);
            }
            Err(e) => {
                warn!(error = %e, "fault rebuild thread spawn failed");
                self.rebuild_pending = false;
                self.rebuild_target_key = None;
            }
        }
    }
}

impl FaultLods {
    fn total_segs(&self) -> usize {
        self.lod0.segs.len() + self.lod1.segs.len() + self.lod2.segs.len()
    }

    fn layers_for_zoom(&self) -> [&SegLayer; 3] {
        [&self.lod0, &self.lod1, &self.lod2]
    }
}

/// LOD 开关滞回：进入阈值高于退出，避免滚轮在阈值附近时小断层闪烁。
fn update_lod_sticky(zoom: f64, lod1_on: &mut bool, lod2_on: &mut bool) {
    if zoom >= 3.05 {
        *lod1_on = true;
    } else if zoom < 2.85 {
        *lod1_on = false;
    }
    if zoom >= 4.85 {
        *lod2_on = true;
    } else if zoom < 4.55 {
        *lod2_on = false;
    }
}

fn lod_mask_from_sticky(lod1_on: bool, lod2_on: bool) -> [bool; 3] {
    [true, lod1_on, lod2_on]
}

/// 测试用瞬时阈值（无滞回）。
fn lod_mask(zoom: f64) -> [bool; 3] {
    let mut a = false;
    let mut b = false;
    update_lod_sticky(zoom, &mut a, &mut b);
    // 用开启阈值的瞬时语义
    [true, zoom >= 3.05, zoom >= 4.85]
}

/// 断层交互档：比全局 `while_interacting` 更温和，避免国际短段被抽光。
fn fault_interact_quality(q: PaintQuality) -> PaintQuality {
    match q {
        PaintQuality::Eco => PaintQuality::Eco,
        PaintQuality::Normal => PaintQuality::Normal,
        PaintQuality::Extreme => PaintQuality::Normal,
    }
}

fn load_lods(dir: &Path, stem: &str, label: &str) -> FaultLods {
    let fine_path = dir.join(format!("{stem}.geojson"));
    let lod0_path = dir.join(format!("{stem}_lod0.geojson"));
    let lod1_path = dir.join(format!("{stem}_lod1.geojson"));

    if lod0_path.is_file() && lod1_path.is_file() && fine_path.is_file() {
        let lod0 = SegLayer::from_segs(load_file(&lod0_path, &format!("{label} lod0")));
        let lod1 = SegLayer::from_segs(load_file(&lod1_path, &format!("{label} lod1")));
        let fine_all = load_file(&fine_path, &format!("{label} fine"));
        // 与 partition_lods 的 lod2 一致：高滑移短段已在 lod0/lod1 文件，勿再塞进 lod2 重复绘制
        let short: Vec<FaultSeg> = fine_all
            .into_iter()
            .filter(|s| s.diag < LOD1_MIN_DIAG && s.slip_rate < 1.0)
            .collect();
        info!(
            lod0 = lod0.segs.len(),
            lod1 = lod1.segs.len(),
            lod2 = short.len(),
            %label,
            "fault lods from files"
        );
        return FaultLods {
            lod0,
            lod1,
            lod2: SegLayer::from_segs(short),
        };
    }

    let all = load_file(&fine_path, label);
    partition_lods(all, label)
}

fn partition_lods(all: Vec<FaultSeg>, label: &str) -> FaultLods {
    let mut lod0 = Vec::new();
    let mut lod1 = Vec::new();
    let mut lod2 = Vec::new();
    for s in all {
        if s.diag >= LOD0_MIN_DIAG || s.slip_rate >= LOD0_MIN_SLIP {
            lod0.push(s);
        } else if s.diag >= LOD1_MIN_DIAG || s.slip_rate >= 1.0 {
            lod1.push(s);
        } else {
            lod2.push(s);
        }
    }
    info!(
        lod0 = lod0.len(),
        lod1 = lod1.len(),
        lod2 = lod2.len(),
        %label,
        "fault lods partitioned"
    );
    FaultLods {
        lod0: SegLayer::from_segs(lod0),
        lod1: SegLayer::from_segs(lod1),
        lod2: SegLayer::from_segs(lod2),
    }
}

fn build_fault_mesh(
    global: &FaultLods,
    china: &FaultLods,
    view: &ViewCtx,
    dark: bool,
    quality: PaintQuality,
    interact_expand: bool,
    mask: [bool; 3],
) -> Mesh {
    let (min_lon, min_lat, max_lon, max_lat) = view_lonlat_bounds(view, interact_expand);
    let globe = view.mode.is_globe();
    // Eco：降细分/顶点数；禁止大 stride 抽段，否则拖转时「部分断层消失」
    let (step_bias, stride_mul, budget_scale) = match (quality, globe) {
        (PaintQuality::Eco, true) => (1usize, 1usize, 0.85_f32),
        (PaintQuality::Eco, false) => (1usize, 1usize, 0.80_f32),
        (PaintQuality::Normal, _) => (0, 1, 1.0),
        (PaintQuality::Extreme, _) => (0, 1, 1.15),
    };
    let zoom_budget = zoom_budget_scale(view.zoom);
    let globe_mul = if globe {
        if quality == PaintQuality::Eco {
            1.45_f32
        } else {
            1.7_f32
        }
    } else {
        1.0
    };
    let step = lod_step(view.zoom) + step_bias;
    // Eco 交互禁止按 index 抽段，否则拖转时可见断层会成片消失
    let stride_g = if quality == PaintQuality::Eco {
        1
    } else {
        (seg_stride(view.zoom, false, globe) * stride_mul).max(1)
    };
    let stride_c = if quality == PaintQuality::Eco {
        1
    } else {
        (seg_stride(view.zoom, true, globe) * stride_mul).max(1)
    };
    // 交互中关掉 min_diag，避免跨阈值时极短断层闪烁
    let min_diag = if interact_expand {
        0.0
    } else {
        min_diag_deg(view.zoom)
    };
    let (global_rgba, china_rgba, width) = stroke_style(dark, view.zoom);
    let max_segs =
        (((MAX_PAINT_SEGS as f32) * budget_scale * zoom_budget * globe_mul) as usize).max(80);
    let china_reserve =
        (((CHINA_RESERVE as f32) * budget_scale * zoom_budget * globe_mul) as usize).max(40);
    let vert_cap =
        (((MAX_MESH_VERTICES as f32) * budget_scale * globe_mul) as usize).max(2_000);
    // 为中国层预留顶点，避免全球层先画满导致 CAFD 整帧 0 顶点
    let china_vert_reserve = ((vert_cap as f32) * 0.28).round() as usize;
    let china_vert_reserve = china_vert_reserve.clamp(800, vert_cap.saturating_sub(400));

    let mut mesh = Mesh::default();
    let mut pts = Vec::new();
    let mut thin = Vec::new();
    let mut seen = Vec::new();
    let mut candidates = Vec::new();

    let global_budget = max_segs.saturating_sub(china_reserve);
    let global_vert_cap = vert_cap.saturating_sub(china_vert_reserve);
    let painted_g = paint_lods(
        &mut mesh,
        global,
        mask,
        view,
        min_lon,
        min_lat,
        max_lon,
        max_lat,
        step,
        stride_g,
        min_diag,
        width,
        global_rgba,
        global_budget,
        global_vert_cap,
        &mut pts,
        &mut thin,
        &mut seen,
        &mut candidates,
    );

    let china_budget = china_reserve + global_budget.saturating_sub(painted_g);
    if china_budget > 0 && bbox_overlap(&CHINA_BBOX, min_lon, min_lat, max_lon, max_lat) {
        let _ = paint_lods(
            &mut mesh,
            china,
            mask,
            view,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            if view.zoom < 4.0 { step.max(2) } else { step },
            stride_c,
            min_diag,
            width + 0.15,
            china_rgba,
            china_budget,
            vert_cap,
            &mut pts,
            &mut thin,
            &mut seen,
            &mut candidates,
        );
    }
    mesh
}

fn paint_lods(
    mesh: &mut Mesh,
    lods: &FaultLods,
    mask: [bool; 3],
    view: &ViewCtx,
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    step: usize,
    stride: usize,
    min_diag: f32,
    width: f32,
    color: Color32,
    budget: usize,
    vert_cap: usize,
    pts: &mut Vec<Pos2>,
    thin: &mut Vec<[f32; 2]>,
    seen: &mut Vec<bool>,
    candidates: &mut Vec<u32>,
) -> usize {
    if budget == 0 {
        return 0;
    }
    let layers = lods.layers_for_zoom();
    let mut painted = 0usize;
    let mut remain = budget;
    for (i, layer) in layers.iter().enumerate() {
        if !mask[i] || remain == 0 || mesh.vertices.len() >= vert_cap {
            break;
        }
        let n = paint_layer(
            mesh,
            layer,
            view,
            min_lon,
            min_lat,
            max_lon,
            max_lat,
            step,
            stride,
            min_diag,
            width,
            color,
            remain,
            vert_cap,
            pts,
            thin,
            seen,
            candidates,
        );
        painted += n;
        remain = remain.saturating_sub(n);
    }
    painted
}

fn paint_layer(
    mesh: &mut Mesh,
    layer: &SegLayer,
    view: &ViewCtx,
    min_lon: f64,
    min_lat: f64,
    max_lon: f64,
    max_lat: f64,
    step: usize,
    stride: usize,
    min_diag: f32,
    width: f32,
    color: Color32,
    budget: usize,
    vert_cap: usize,
    pts: &mut Vec<Pos2>,
    thin: &mut Vec<[f32; 2]>,
    seen: &mut Vec<bool>,
    candidates: &mut Vec<u32>,
) -> usize {
    if budget == 0 || layer.segs.is_empty() {
        return 0;
    }
    let step = step.max(1);
    let stride = stride.max(1);

    if seen.len() < layer.segs.len() {
        seen.resize(layer.segs.len(), false);
    } else {
        seen[..layer.segs.len()].fill(false);
    }
    layer.collect_candidates(min_lon, min_lat, max_lon, max_lat, seen, candidates);

    // 交互（min_diag=0）不按优先级截候选，避免短段在预算边界进进出出闪烁
    if min_diag > 0.0 {
        let keep = (budget.saturating_mul(3))
            .max(budget)
            .min(candidates.len());
        if keep < candidates.len() {
            candidates.select_nth_unstable_by(keep, |&a, &b| {
                let da = seg_priority(&layer.segs[a as usize]);
                let db = seg_priority(&layer.segs[b as usize]);
                db.total_cmp(&da).then_with(|| a.cmp(&b))
            });
            candidates.truncate(keep);
        }
    }
    candidates.sort_unstable_by(|&a, &b| {
        let da = seg_priority(&layer.segs[a as usize]);
        let db = seg_priority(&layer.segs[b as usize]);
        db.total_cmp(&da).then_with(|| a.cmp(&b))
    });

    let mut painted = 0usize;
    let keep_diag = (min_diag * 3.0).max(2.5);

    for &idx in candidates.iter() {
        if painted >= budget || mesh.vertices.len() >= vert_cap {
            break;
        }
        let seg = &layer.segs[idx as usize];
        if !bbox_overlap(&seg.bbox, min_lon, min_lat, max_lon, max_lat) {
            continue;
        }
        // 高滑移板缘（日本海沟等）几何短，勿被远距 min_diag 滤掉
        if min_diag > 0.0 && seg.diag < min_diag && seg.slip_rate < LOD0_MIN_SLIP {
            continue;
        }
        if stride > 1
            && seg.diag < keep_diag
            && seg.slip_rate < LOD0_MIN_SLIP
            && (idx as usize) % stride != 0
        {
            continue;
        }

        // GQ：越大/滑移越快线越粗；`width` 为缩放基准
        let seg_w = stroke_width_for_seg(seg, width);
        // step/stride>1 视为拖转轻量：球体大圆细分也降档
        let lite = step > 1 || stride > 1;
        if step == 1 {
            paint_line_mesh(mesh, &seg.line, view, seg_w, color, pts, lite);
        } else {
            thin.clear();
            thin.reserve(seg.line.len() / step + 2);
            for (i, p) in seg.line.iter().enumerate() {
                if i % step == 0 || i + 1 == seg.line.len() {
                    thin.push(*p);
                }
            }
            if thin.len() >= 2 {
                paint_line_mesh(mesh, thin, view, seg_w, color, pts, lite);
            }
        }
        painted += 1;
    }
    candidates.clear();
    painted
}

impl SegLayer {
    fn from_segs(segs: Vec<FaultSeg>) -> Self {
        let mut grid: HashMap<(i16, i16), Vec<u32>> = HashMap::new();
        for (i, seg) in segs.iter().enumerate() {
            let idx = i as u32;
            for cell in bbox_cells(&seg.bbox) {
                grid.entry(cell).or_default().push(idx);
            }
        }
        Self { segs, grid }
    }

    fn collect_candidates(
        &self,
        min_lon: f64,
        min_lat: f64,
        max_lon: f64,
        max_lat: f64,
        seen: &mut [bool],
        out: &mut Vec<u32>,
    ) {
        out.clear();
        let mut cells = view_cells(min_lon, min_lat, max_lon, max_lat);
        cells.sort_unstable();
        for cell in cells {
            let Some(list) = self.grid.get(&cell) else {
                continue;
            };
            for &idx in list {
                let i = idx as usize;
                if i < seen.len() && !seen[i] {
                    seen[i] = true;
                    out.push(idx);
                }
            }
        }
    }
}

fn load_file(path: &Path, label: &str) -> Vec<FaultSeg> {
    if !path.is_file() {
        warn!(path = %path.display(), "缺少断层 GeoJSON（{label}）");
        return Vec::new();
    }
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<Fc>(&s) {
            Ok(fc) => {
                let segs: Vec<FaultSeg> = fc
                    .features
                    .into_iter()
                    .flat_map(|f| {
                        let slip = parse_slip_rate(f.properties.as_ref());
                        let Some(geom) = f.geometry.as_ref() else {
                            return Vec::new();
                        };
                        let Some(lines) = parse_lines(geom) else {
                            return Vec::new();
                        };
                        lines
                            .into_iter()
                            .filter_map(|line| {
                                if line.len() < 2 {
                                    return None;
                                }
                                let bbox = line_bbox(&line);
                                let diag =
                                    ((bbox[2] - bbox[0]).hypot(bbox[3] - bbox[1])).max(0.0);
                                Some(FaultSeg {
                                    bbox,
                                    diag,
                                    slip_rate: slip,
                                    line,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                info!(n = segs.len(), path = %path.display(), %label, "loaded faults");
                segs
            }
            Err(e) => {
                warn!(error = %e, path = %path.display(), "断层 GeoJSON 解析失败");
                Vec::new()
            }
        },
        Err(e) => {
            warn!(error = %e, path = %path.display(), "读取断层 GeoJSON 失败");
            Vec::new()
        }
    }
}

fn line_bbox(line: &[[f32; 2]]) -> [f32; 4] {
    let mut min_lon = f32::MAX;
    let mut min_lat = f32::MAX;
    let mut max_lon = f32::MIN;
    let mut max_lat = f32::MIN;
    for p in line {
        min_lon = min_lon.min(p[0]);
        max_lon = max_lon.max(p[0]);
        min_lat = min_lat.min(p[1]);
        max_lat = max_lat.max(p[1]);
    }
    [min_lon, min_lat, max_lon, max_lat]
}

fn lon_cell(lon: f32) -> i16 {
    let x = ((lon + 180.0) / GRID_CELL_DEG).floor() as i32;
    x.clamp(0, (360.0 / GRID_CELL_DEG) as i32 - 1) as i16
}

fn lat_cell(lat: f32) -> i16 {
    let y = ((lat + 90.0) / GRID_CELL_DEG).floor() as i32;
    y.clamp(0, (180.0 / GRID_CELL_DEG) as i32 - 1) as i16
}

fn bbox_cells(b: &[f32; 4]) -> Vec<(i16, i16)> {
    let ix0 = lon_cell(b[0]);
    let ix1 = lon_cell(b[2]);
    let iy0 = lat_cell(b[1]);
    let iy1 = lat_cell(b[3]);
    let mut out = Vec::new();
    if ix0 <= ix1 {
        for ix in ix0..=ix1 {
            for iy in iy0..=iy1 {
                out.push((ix, iy));
            }
        }
    } else {
        let max_ix = lon_cell(180.0);
        for ix in ix0..=max_ix {
            for iy in iy0..=iy1 {
                out.push((ix, iy));
            }
        }
        for ix in 0..=ix1 {
            for iy in iy0..=iy1 {
                out.push((ix, iy));
            }
        }
    }
    out
}

fn view_cells(min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> Vec<(i16, i16)> {
    let iy0 = lat_cell(min_lat as f32);
    let iy1 = lat_cell(max_lat as f32);
    let mut out = Vec::new();
    if min_lon <= max_lon {
        let ix0 = lon_cell(min_lon as f32);
        let ix1 = lon_cell(max_lon as f32);
        for ix in ix0..=ix1 {
            for iy in iy0..=iy1 {
                out.push((ix, iy));
            }
        }
    } else {
        let ix0 = lon_cell(min_lon as f32);
        let ix1 = lon_cell(max_lon as f32);
        let max_ix = lon_cell(180.0);
        for ix in ix0..=max_ix {
            for iy in iy0..=iy1 {
                out.push((ix, iy));
            }
        }
        for ix in 0..=ix1 {
            for iy in iy0..=iy1 {
                out.push((ix, iy));
            }
        }
    }
    out
}

fn bbox_overlap(b: &[f32; 4], min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> bool {
    let overlaps = |a0: f64, a1: f64, b0: f64, b1: f64| a0 <= b1 && b0 <= a1;
    let lat_ok = overlaps(b[1] as f64, b[3] as f64, min_lat, max_lat);
    if !lat_ok {
        return false;
    }
    if min_lon <= max_lon {
        overlaps(b[0] as f64, b[2] as f64, min_lon, max_lon)
    } else {
        overlaps(b[0] as f64, b[2] as f64, min_lon, 180.0)
            || overlaps(b[0] as f64, b[2] as f64, -180.0, max_lon)
    }
}

fn view_lonlat_bounds(view: &ViewCtx, interact_expand: bool) -> (f64, f64, f64, f64) {
    if view.mode.is_globe() {
        // 用地平线半角（可见半球）；拖转时再外扩，避免球缘断层被裁没
        let pad = if interact_expand { 1.22 } else { 1.08 };
        let half = view
            .horizon_half_deg()
            .unwrap_or_else(|| {
                let scroll = crate::globe::zoom_to_scroll(view.zoom);
                let cam_r = (1.0 + scroll).max(1.05);
                cam_r.recip().acos().to_degrees()
            })
            .clamp(28.0, 95.0)
            * pad;
        let min_lat = (view.center_lat - half).max(-90.0);
        let max_lat = (view.center_lat + half).min(90.0);
        let cos_lat = view.center_lat.to_radians().cos().abs().max(0.22);
        let lon_half = (half / cos_lat).min(179.0);
        let mut min_lon = view.center_lon - lon_half;
        let mut max_lon = view.center_lon + lon_half;
        min_lon = projection::normalize_lon(min_lon);
        max_lon = projection::normalize_lon(max_lon);
        (min_lon, min_lat, max_lon, max_lat)
    } else {
        let corners = [
            view.screen_to_lonlat(view.rect.left_top()),
            view.screen_to_lonlat(view.rect.right_top()),
            view.screen_to_lonlat(view.rect.left_bottom()),
            view.screen_to_lonlat(view.rect.right_bottom()),
        ];
        let mut min_lon = f64::INFINITY;
        let mut max_lon = f64::NEG_INFINITY;
        let mut min_lat = f64::INFINITY;
        let mut max_lat = f64::NEG_INFINITY;
        for (lon, lat) in corners {
            min_lon = min_lon.min(lon);
            max_lon = max_lon.max(lon);
            min_lat = min_lat.min(lat);
            max_lat = max_lat.max(lat);
        }
        if max_lon - min_lon > 300.0 {
            let half_w =
                (view.rect.width() as f64) / projection::world_size(view.zoom) * 180.0 + 6.0;
            (
                projection::normalize_lon(view.center_lon - half_w),
                min_lat - 1.5,
                projection::normalize_lon(view.center_lon + half_w),
                max_lat + 1.5,
            )
        } else {
            // 平面裁剪外扩；交互中更大，拖/缩时边缘不立刻空白
            let base = ((max_lon - min_lon).abs().max(max_lat - min_lat).abs()
                * if interact_expand { 0.65 } else { 0.45 })
            .clamp(
                if interact_expand { 8.0 } else { 5.0 },
                if interact_expand { 42.0 } else { 32.0 },
            );
            (
                min_lon - base,
                min_lat - base,
                max_lon + base,
                max_lat + base,
            )
        }
    }
}

fn lod_step(zoom: f64) -> usize {
    if zoom < 2.8 {
        8
    } else if zoom < 3.8 {
        4
    } else if zoom < 5.2 {
        2
    } else {
        1
    }
}

fn seg_stride(zoom: f64, china: bool, globe: bool) -> usize {
    let base: usize = if zoom < 2.5 {
        if globe { 8 } else { 12 }
    } else if zoom < 3.2 {
        if globe { 5 } else { 7 }
    } else if zoom < 4.0 {
        if globe { 2 } else { 3 }
    } else if zoom < 5.5 {
        2
    } else {
        1
    };
    // 中国层略密：补齐 CAFD 短段，勿再加倍抽稀
    if china {
        base.saturating_sub(1).max(1)
    } else {
        base
    }
}

/// 选段优先级：高滑移短段（板缘）与长几何同等重要。
fn seg_priority(seg: &FaultSeg) -> f32 {
    seg.diag.max(seg.slip_rate * 0.12)
}

fn min_diag_deg(zoom: f64) -> f32 {
    // 阈值放宽：少滤短段，降低缩放时「突然消失」
    if zoom < 2.8 {
        0.85
    } else if zoom < 3.8 {
        0.40
    } else if zoom < 5.0 {
        0.14
    } else if zoom < 6.2 {
        0.05
    } else {
        0.0
    }
}

fn zoom_budget_scale(zoom: f64) -> f32 {
    if zoom < 2.5 {
        0.35
    } else if zoom < 3.5 {
        0.55
    } else if zoom < 5.0 {
        0.75
    } else {
        1.0
    }
}

fn stroke_style(dark: bool, zoom: f64) -> (Color32, Color32, f32) {
    // 基准线宽（像素）；单条再按 GQ 公式随尺度加粗
    let w = if zoom < 4.0 {
        0.95_f32
    } else if zoom < 6.5 {
        1.15_f32
    } else {
        1.35_f32
    };
    if dark {
        (
            Color32::from_rgba_unmultiplied(0xE0, 0x7A, 0x52, 210),
            Color32::from_rgba_unmultiplied(0xF0, 0x90, 0x60, 230),
            w,
        )
    } else {
        (
            Color32::from_rgba_unmultiplied(0xB0, 0x40, 0x28, 210),
            Color32::from_rgba_unmultiplied(0xC8, 0x4C, 0x30, 230),
            w,
        )
    }
}

/// 对齐新版 GQ：`(float)(Math.log(slipRate/5+1)+0.2)`。
/// 板缘高滑移（日本海沟等）即使几何短也明显加粗；无速率时用 diag 代理。
fn stroke_width_for_seg(seg: &FaultSeg, base: f32) -> f32 {
    let size = if seg.slip_rate > 1e-6 {
        seg.slip_rate as f64
    } else {
        (seg.diag as f64 * 10.0).max(0.05)
    };
    let gq = ((size / 5.0) + 1.0).ln() + 0.2;
    // 略放大视觉对比：海沟 ~90mm/yr → 明显粗于陆内短断层
    let w = (gq as f32) * (base / 0.75);
    w.clamp(0.6, 6.5)
}

fn parse_slip_rate(props: Option<&serde_json::Value>) -> f32 {
    let Some(p) = props else {
        return 0.0;
    };
    let raw = p
        .get("net_slip_rate")
        .or_else(|| p.get("slip_rate"))
        .or_else(|| p.get("slipRate"));
    let Some(v) = raw else {
        return 0.0;
    };
    if let Some(n) = v.as_f64() {
        return (n as f32).clamp(0.0, 100.0);
    }
    if let Some(s) = v.as_str() {
        // GQ：`(a, b)` 取首段；纯数字直接解析
        let s = s.trim();
        let s = s.strip_prefix('(').unwrap_or(s);
        let head = s.split([',', ' ', '–', '-']).next().unwrap_or(s).trim();
        if let Ok(n) = head.parse::<f32>() {
            return n.clamp(0.0, 100.0);
        }
    }
    0.0
}

fn paint_key(
    view: &ViewCtx,
    dark: bool,
    ppp: f32,
    quality: PaintQuality,
    interacting: bool,
) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    view.mode.hash(&mut h);
    if view.mode.is_globe() {
        if interacting {
            // 拖/滚轮：约 0.04° / 0.04 zoom 一格，显著减少同步重投次数
            ((view.center_lon * 25.0).round() as i64).hash(&mut h);
            ((view.center_lat * 25.0).round() as i64).hash(&mut h);
            ((view.zoom * 24.0).round() as i64).hash(&mut h);
        } else {
            // 静止：精确视角，与底图贴合
            view.center_lon.to_bits().hash(&mut h);
            view.center_lat.to_bits().hash(&mut h);
            view.zoom.to_bits().hash(&mut h);
        }
    } else if interacting {
        // 平面跟焦/拖动：更粗量化，少触发后台重建换 key
        ((view.center_lon * 40.0).round() as i64).hash(&mut h);
        ((view.center_lat * 40.0).round() as i64).hash(&mut h);
        ((view.zoom * 12.0).round() as i64).hash(&mut h);
    } else {
        ((view.center_lon * 100.0).round() as i64).hash(&mut h);
        ((view.center_lat * 100.0).round() as i64).hash(&mut h);
        ((view.zoom * 28.0).round() as i64).hash(&mut h);
    }
    ((view.rect.width() * 0.5).round() as i32).hash(&mut h);
    ((view.rect.height() * 0.5).round() as i32).hash(&mut h);
    ((ppp * 2.0).round() as i32).hash(&mut h);
    dark.hash(&mut h);
    quality.hash(&mut h);
    h.finish()
}

/// 不含 zoom/center：跟焦缩放时 base 不变，才能走 mesh 缩放复用。
fn paint_base_key(view: &ViewCtx, dark: bool, ppp: f32, quality: PaintQuality) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    view.mode.hash(&mut h);
    ((view.rect.width() * 0.25).round() as i32).hash(&mut h);
    ((view.rect.height() * 0.25).round() as i32).hash(&mut h);
    ((view.rect.min.x * 0.25).round() as i32).hash(&mut h);
    ((view.rect.min.y * 0.25).round() as i32).hash(&mut h);
    ((ppp * 2.0).round() as i32).hash(&mut h);
    dark.hash(&mut h);
    quality.hash(&mut h);
    h.finish()
}

fn fault_needs_cull_rebuild(c: &PaintCache, view: &ViewCtx) -> bool {
    if (c.build_zoom - view.zoom).abs() >= CULL_REBUILD_ZOOM {
        return true;
    }
    flat_pan_delta(c.build_lon, c.build_lat, view)
        .map(|(dx, dy)| dx * dx + dy * dy >= CULL_REBUILD_PX * CULL_REBUILD_PX)
        .unwrap_or(true)
}

/// 平面断层 mesh：按 zoom 差缩放 + 中心平移（跟焦/飞跃跟手）。
/// 大 |dz| 几何略偏，靠后台精确重投纠偏；禁止每帧同步重投。
fn apply_fault_mesh_view_delta(c: &mut PaintCache, view: &ViewCtx) -> bool {
    if view.mode.is_globe() {
        return false;
    }
    let dz = view.zoom - c.mesh_zoom;
    let scale = if dz.abs() > 1e-9 {
        let sc = 2f32.powf(dz as f32);
        if !sc.is_finite() || sc < 1e-4 || sc > 1e4 {
            return false;
        }
        sc
    } else {
        1.0
    };
    let (dx, dy) = match flat_pan_delta(c.mesh_lon, c.mesh_lat, view) {
        Some(d) => d,
        None => return false,
    };
    let need_s = (scale - 1.0).abs() > 1e-6;
    let need_t = dx.abs() > 0.05 || dy.abs() > 0.05;
    if need_s || need_t {
        let mesh = Arc::make_mut(&mut c.mesh);
        let origin = view.rect.center();
        if need_s && need_t {
            let t = Vec2::new(dx, dy);
            for v in &mut mesh.vertices {
                v.pos = origin + (v.pos - origin) * scale + t;
            }
        } else if need_s {
            for v in &mut mesh.vertices {
                v.pos = origin + (v.pos - origin) * scale;
            }
        } else {
            mesh.translate(Vec2::new(dx, dy));
        }
    }
    c.mesh_lon = view.center_lon;
    c.mesh_lat = view.center_lat;
    c.mesh_zoom = view.zoom;
    true
}


/// 平面相机平移对应的 mesh 像素增量。跨日界时用最短弧；异常大位移返回 `None`。
fn flat_pan_delta(build_lon: f64, build_lat: f64, view: &ViewCtx) -> Option<(f32, f32)> {
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

fn parse_lines(g: &Geom) -> Option<Vec<Vec<[f32; 2]>>> {
    match g.typ.as_str() {
        "LineString" => {
            let pts = coords_line(&g.coordinates)?;
            Some(vec![pts])
        }
        "MultiLineString" => {
            let arr = g.coordinates.as_array()?;
            let mut out = Vec::new();
            for line in arr {
                if let Some(pts) = coords_line(line) {
                    out.push(pts);
                }
            }
            Some(out)
        }
        _ => None,
    }
}

fn coords_line(v: &serde_json::Value) -> Option<Vec<[f32; 2]>> {
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
    if pts.len() < 2 {
        None
    } else {
        Some(pts)
    }
}

fn paint_line_mesh(
    mesh: &mut Mesh,
    line: &[[f32; 2]],
    view: &ViewCtx,
    width: f32,
    color: Color32,
    pts: &mut Vec<Pos2>,
    lite: bool,
) {
    if view.mode.is_globe() {
        paint_line_mesh_globe(mesh, line, view, width, color, lite);
        return;
    }
    pts.clear();
    let mut last: Option<Pos2> = None;
    let max_jump = projection::world_size(view.zoom) as f32 * (80.0 / 360.0);
    for p in line {
        let Some(sp) = view.project(p[0] as f64, p[1] as f64) else {
            stroke_poly(mesh, pts, width, color);
            last = None;
            continue;
        };
        if let Some(prev) = last {
            let dx = sp.x - prev.x;
            let dy = sp.y - prev.y;
            if dx * dx + dy * dy > max_jump * max_jump {
                stroke_poly(mesh, pts, width, color);
            }
        }
        pts.push(sp);
        last = Some(sp);
    }
    stroke_poly(mesh, pts, width, color);
}

/// 球体：大圆加密 + 左右地表偏移生成贴面带宽（对齐 GQ createSquare / move_on_globe）。
fn paint_line_mesh_globe(
    mesh: &mut Mesh,
    line: &[[f32; 2]],
    view: &ViewCtx,
    width_px: f32,
    color: Color32,
    lite: bool,
) {
    if line.len() < 2 {
        return;
    }
    let half_km = view
        .px_to_km((width_px * 0.5).max(0.6))
        .unwrap_or(2.5)
        .clamp(0.6, 14.0);
    // 先按视距加密折线，再逐段贴面
    let densified = densify_fault_line_globe(line, view, lite);
    for w in densified.windows(2) {
        let (lon0, lat0) = (w[0][0] as f64, w[0][1] as f64);
        let (lon1, lat1) = (w[1][0] as f64, w[1][1] as f64);
        let br = crate::globe::bearing_deg(lon0, lat0, lon1, lat1);
        let left_br = (br + 90.0).rem_euclid(360.0);
        let right_br = (br + 270.0).rem_euclid(360.0);
        let (l0lon, l0lat) = crate::globe::move_on_globe(lon0, lat0, half_km, left_br);
        let (r0lon, r0lat) = crate::globe::move_on_globe(lon0, lat0, half_km, right_br);
        let (l1lon, l1lat) = crate::globe::move_on_globe(lon1, lat1, half_km, left_br);
        let (r1lon, r1lat) = crate::globe::move_on_globe(lon1, lat1, half_km, right_br);
        // 用放宽地平线的 project_fill，避免球缘断层被硬裁成“断在半空”
        let corners = [
            view.project_fill(l0lon, l0lat),
            view.project_fill(r0lon, r0lat),
            view.project_fill(l1lon, l1lat),
            view.project_fill(r1lon, r1lat),
        ];
        match (corners[0], corners[1], corners[2], corners[3]) {
            (Some(l0), Some(r0), Some(l1), Some(r1)) => {
                let i0 = mesh.vertices.len() as u32;
                mesh.colored_vertex(l0, color);
                mesh.colored_vertex(r0, color);
                mesh.colored_vertex(l1, color);
                mesh.colored_vertex(r1, color);
                mesh.add_triangle(i0, i0 + 1, i0 + 2);
                mesh.add_triangle(i0 + 1, i0 + 3, i0 + 2);
            }
            _ => {
                let vis: Vec<Pos2> = corners.into_iter().flatten().collect();
                if vis.len() == 3 {
                    let i0 = mesh.vertices.len() as u32;
                    mesh.colored_vertex(vis[0], color);
                    mesh.colored_vertex(vis[1], color);
                    mesh.colored_vertex(vis[2], color);
                    mesh.add_triangle(i0, i0 + 1, i0 + 2);
                }
            }
        }
    }
}

fn densify_fault_line_globe(line: &[[f32; 2]], view: &ViewCtx, eco: bool) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(line.len() * 3);
    // Eco（拖转同步）：更大步长/更少细分；静止再加密贴球
    let step_km = if eco {
        view.px_to_km(14.0).unwrap_or(28.0).clamp(10.0, 55.0)
    } else {
        view.px_to_km(8.0).unwrap_or(16.0).clamp(4.0, 36.0)
    };
    let max_div = if eco {
        if view.zoom >= 7.0 {
            4
        } else if view.zoom >= 5.5 {
            8
        } else {
            12
        }
    } else if view.zoom >= 7.0 {
        10
    } else if view.zoom >= 5.5 {
        18
    } else {
        36
    };
    for w in line.windows(2) {
        let (lon0, lat0) = (w[0][0] as f64, w[0][1] as f64);
        let (lon1, lat1) = (w[1][0] as f64, w[1][1] as f64);
        if out.is_empty() {
            out.push(w[0]);
        }
        let dist = haversine_km(lon0, lat0, lon1, lat1);
        if dist > step_km * 1.25 {
            let n = ((dist / step_km).ceil() as usize).clamp(2, max_div);
            let br = crate::globe::bearing_deg(lon0, lat0, lon1, lat1);
            for i in 1..n {
                let d = dist * (i as f64) / (n as f64);
                let (lon, lat) = crate::globe::move_on_globe(lon0, lat0, d, br);
                out.push([lon as f32, lat as f32]);
            }
        }
        out.push(w[1]);
    }
    if out.is_empty() && !line.is_empty() {
        out.extend_from_slice(line);
    }
    out
}

fn haversine_km(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let r = 6371.0_f64;
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp * 0.5).sin().powi(2) + p1.cos() * p2.cos() * (dl * 0.5).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

fn stroke_poly(mesh: &mut Mesh, pts: &mut Vec<Pos2>, width: f32, color: Color32) {
    if pts.len() < 2 {
        pts.clear();
        return;
    }
    let half = (width * 0.5).max(0.35);
    for w in pts.windows(2) {
        let a = w[0];
        let b = w[1];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.4 {
            continue;
        }
        let nx = -dy / len * half;
        let ny = dx / len * half;
        let i0 = mesh.vertices.len() as u32;
        mesh.colored_vertex(Pos2::new(a.x + nx, a.y + ny), color);
        mesh.colored_vertex(Pos2::new(a.x - nx, a.y - ny), color);
        mesh.colored_vertex(Pos2::new(b.x + nx, b.y + ny), color);
        mesh.colored_vertex(Pos2::new(b.x - nx, b.y - ny), color);
        mesh.add_triangle(i0, i0 + 1, i0 + 2);
        mesh.add_triangle(i0 + 1, i0 + 3, i0 + 2);
    }
    pts.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::ProjectionMode;
    use egui::Rect;

    #[test]
    fn lod_mask_by_zoom() {
        assert_eq!(lod_mask(2.0), [true, false, false]);
        assert_eq!(lod_mask(4.0), [true, true, false]);
        assert_eq!(lod_mask(6.0), [true, true, true]);
    }

    #[test]
    fn lod_sticky_hysteresis() {
        let mut l1 = false;
        let mut l2 = false;
        update_lod_sticky(4.9, &mut l1, &mut l2);
        assert!(l1 && l2);
        update_lod_sticky(4.7, &mut l1, &mut l2);
        assert!(l2, "still on inside hysteresis band");
        update_lod_sticky(4.4, &mut l1, &mut l2);
        assert!(!l2);
        assert_eq!(lod_mask_from_sticky(true, false), [true, true, false]);
    }

    #[test]
    fn partition_exclusive_tiers() {
        let segs = vec![
            FaultSeg {
                bbox: [0.0, 0.0, 2.0, 2.0],
                diag: 2.8,
                slip_rate: 0.0,
                line: vec![[0.0, 0.0], [2.0, 2.0]],
            },
            FaultSeg {
                bbox: [0.0, 0.0, 0.5, 0.5],
                diag: 0.5,
                slip_rate: 0.0,
                line: vec![[0.0, 0.0], [0.5, 0.5]],
            },
            FaultSeg {
                bbox: [0.0, 0.0, 0.1, 0.1],
                diag: 0.1,
                slip_rate: 0.0,
                line: vec![[0.0, 0.0], [0.1, 0.1]],
            },
        ];
        let lods = partition_lods(segs, "test");
        assert_eq!(lods.lod0.segs.len(), 1);
        assert_eq!(lods.lod1.segs.len(), 1);
        assert_eq!(lods.lod2.segs.len(), 1);
        assert!(lods.lod0.segs[0].diag >= LOD0_MIN_DIAG);
        assert!(lods.lod1.segs[0].diag >= LOD1_MIN_DIAG);
        assert!(lods.lod2.segs[0].diag < LOD1_MIN_DIAG);
    }

    #[test]
    fn flat_pan_delta_moves_opposite_camera() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let view = ViewCtx::new(ProjectionMode::Flat, 120.0, 30.0, 5.0, rect);
        let (dx, dy) = flat_pan_delta(119.0, 30.0, &view).expect("delta");
        assert!(dx < 0.0, "dx={dx}");
        assert!(dy.abs() < 1.0, "dy={dy}");
    }

    #[test]
    fn flat_pan_delta_dateline_uses_short_arc() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let view = ViewCtx::new(ProjectionMode::Flat, -179.0, 0.0, 8.0, rect);
        let (dx, dy) = flat_pan_delta(179.0, 0.0, &view).expect("dateline delta");
        let one_deg = (projection::world_size(8.0) / 360.0) as f32;
        assert!(dx.abs() < one_deg * 5.0, "dx={dx} one_deg={one_deg}");
        assert!(dy.abs() < 1.0, "dy={dy}");
    }

    #[test]
    fn china_bbox_skips_atlantic() {
        assert!(!bbox_overlap(&CHINA_BBOX, -40.0, 20.0, -10.0, 50.0));
        assert!(bbox_overlap(&CHINA_BBOX, 100.0, 20.0, 130.0, 40.0));
    }

    #[test]
    fn gq_style_width_grows_with_size() {
        let thin = FaultSeg {
            bbox: [0.0; 4],
            diag: 0.2,
            slip_rate: 0.0,
            line: vec![[0.0, 0.0], [0.2, 0.0]],
        };
        let thick = FaultSeg {
            bbox: [0.0; 4],
            diag: 4.0,
            slip_rate: 0.0,
            line: vec![[0.0, 0.0], [4.0, 0.0]],
        };
        let wt = stroke_width_for_seg(&thin, 1.15);
        let wk = stroke_width_for_seg(&thick, 1.15);
        assert!(wk > wt + 0.3, "thin={wt} thick={wk}");
        // 有滑移速率时优先 GQ 公式
        let by_rate = FaultSeg {
            slip_rate: 40.0,
            ..thick
        };
        let wr = stroke_width_for_seg(&by_rate, 1.15);
        assert!(wr > wt);
    }

    #[test]
    fn globe_cull_covers_visible_hemisphere() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 800.0));
        let view = ViewCtx::new(ProjectionMode::Globe, 110.0, 32.0, 4.5, rect);
        let (min_lon, min_lat, max_lon, max_lat) = view_lonlat_bounds(&view, false);
        let lat_span = max_lat - min_lat;
        assert!(
            lat_span > 80.0,
            "globe cull lat span too narrow: {lat_span} ({min_lat}..{max_lat})"
        );
        // 日本附近应落入以中国为中心的球体可见范围
        assert!(bbox_overlap(&[130.0, 30.0, 145.0, 45.0], min_lon, min_lat, max_lon, max_lat));
    }

    #[test]
    fn paint_key_stable_for_same_quantized_view() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let a = ViewCtx::new(ProjectionMode::Flat, 120.001, 30.001, 5.0, rect);
        let b = ViewCtx::new(ProjectionMode::Flat, 120.004, 30.004, 5.0, rect);
        let ka = paint_key(&a, true, 1.0, PaintQuality::Normal, false);
        let kb = paint_key(&b, true, 1.0, PaintQuality::Normal, false);
        assert_eq!(ka, kb);
    }
}
