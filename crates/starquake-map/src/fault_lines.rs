//! 活动断层迹线：全球 GEM + 中国 CAFD（CN-faults），平面 / 球体同绘。
//!
//! 性能：
//! - 数据 LOD（lod0 长断层 / lod1 中等 / lod2 短线）按缩放选用
//! - 空间网格 + 预算/顶点上限
//! - 平面拖拽：对缓存 Mesh **原地 translate**（Arc 唯一时零拷贝）
//! - 松手后后台线程重建 Mesh，主线程继续画旧图

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
const MAX_PAINT_SEGS: usize = 3600;
const CHINA_RESERVE: usize = 900;
const MAX_MESH_VERTICES: usize = 24_000;

/// 与 `tools/prep_faults.py` 对齐
const LOD0_MIN_DIAG: f32 = 1.0;
const LOD1_MIN_DIAG: f32 = 0.28;

/// 中国及邻区粗包围（度），视口不相交则跳过 CAFD
const CHINA_BBOX: [f32; 4] = [70.0, 15.0, 145.0, 55.0];

/// 相对上次全量裁剪中心，平面平移超过该像素则后台重裁剪
const CULL_REBUILD_PX: f32 = 120.0;

#[derive(Clone)]
struct FaultSeg {
    bbox: [f32; 4],
    diag: f32,
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
    /// 上次**全量重建**时的中心（用于判断是否需重裁剪）
    build_lon: f64,
    build_lat: f64,
    build_zoom: f64,
    /// 当前 Mesh 已对齐到的中心（原地平移后更新）
    mesh_lon: f64,
    mesh_lat: f64,
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
}

#[derive(Deserialize)]
struct Fc {
    features: Vec<Feat>,
}

#[derive(Deserialize)]
struct Feat {
    #[serde(default)]
    geometry: Option<Geom>,
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
    }

    /// 飞跃落地等场景：丢掉旧 mesh，迫使下一帧按新视角重建。
    pub fn invalidate_paint_cache(&mut self) {
        self.paint_cache = None;
        self.rebuild_gen = self.rebuild_gen.wrapping_add(1);
        self.rebuild_pending = false;
        self.rebuild_target_key = None;
        self.rebuild_rx = None;
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

    /// `interacting`：拖拽/滚轮/飞跃中为 true。
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
        let lookup_key = paint_key(view, dark, ppp, quality);
        let lookup_base = paint_base_key(view, dark, ppp, quality);

        // 平面同缩放：优先原地平移（含键未变的量化格内微移），避免粘滞 / 日界线炸 mesh
        let can_shift = self.paint_cache.as_ref().is_some_and(|c| {
            !view.mode.is_globe()
                && c.base_key == lookup_base
                && (c.build_zoom - view.zoom).abs() < 1e-9
        });
        if can_shift {
            let mut shift_ok = false;
            if let Some(c) = self.paint_cache.as_mut() {
                if let Some((dx, dy)) = flat_pan_delta(c.mesh_lon, c.mesh_lat, view) {
                    if dx.abs() > 0.05 || dy.abs() > 0.05 {
                        Arc::make_mut(&mut c.mesh).translate(Vec2::new(dx, dy));
                        c.mesh_lon = view.center_lon;
                        c.mesh_lat = view.center_lat;
                    }
                    c.key = lookup_key;
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                    shift_ok = true;
                } else {
                    // 跨日界等异常位移：先画旧图，下面强制重建
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                }
            }
            if shift_ok {
                let need_cull = self.paint_cache.as_ref().is_some_and(|c| {
                    flat_pan_delta(c.build_lon, c.build_lat, view)
                        .map(|(dx, dy)| dx * dx + dy * dy >= CULL_REBUILD_PX * CULL_REBUILD_PX)
                        .unwrap_or(true)
                });
                if !interacting && need_cull {
                    self.spawn_rebuild(view, dark, quality, lookup_key, lookup_base);
                }
            } else if !interacting {
                self.spawn_rebuild(view, dark, quality, lookup_key, lookup_base);
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

        // 平面缩放交互可短暂冻帧；球体交互中禁止冻旧屏空 mesh（否则断层不随球转）。
        if interacting && !view.mode.is_globe() {
            if let Some(c) = self.paint_cache.as_ref() {
                ui.painter_at(view.rect)
                    .add(egui::Shape::Mesh(c.mesh.clone()));
                return;
            }
        }

        // 静止且键变了：已有同视角后台任务则先画临时 mesh；微移后台重建；大跳转丢旧图快建
        if !interacting && self.paint_cache.is_some() {
            if self.rebuild_pending && self.rebuild_target_key == Some(lookup_key) {
                if let Some(c) = self.paint_cache.as_ref() {
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                }
                return;
            }
            let big_jump = self.paint_cache.as_ref().is_some_and(|c| {
                view_jump_too_far(c.build_lon, c.build_lat, c.build_zoom, view)
            });
            if !big_jump {
                if let Some(c) = self.paint_cache.as_ref() {
                    ui.painter_at(view.rect)
                        .add(egui::Shape::Mesh(c.mesh.clone()));
                }
                self.spawn_rebuild(view, dark, quality, lookup_key, lookup_base);
                return;
            }
            self.paint_cache = None;
        }

        // 无缓存：前台降质快建（落地/交互），再后台刷满质
        let q = quality.while_interacting();
        let key = paint_key(view, dark, ppp, q);
        let base_key = paint_base_key(view, dark, ppp, q);
        let mesh = build_fault_mesh(&self.global, &self.china, view, dark, q);
        let mesh = Arc::new(mesh);
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
            mesh,
        });
        if !interacting && q != quality {
            self.spawn_rebuild(view, dark, quality, lookup_key, lookup_base);
        }
    }

    fn is_empty(&self) -> bool {
        self.global.total_segs() == 0 && self.china.total_segs() == 0
    }

    fn spawn_rebuild(
        &mut self,
        view: &ViewCtx,
        dark: bool,
        quality: PaintQuality,
        key: u64,
        base_key: u64,
    ) {
        // 已有同目标键任务则跳过；视角已变则作废旧任务并重排
        if self.rebuild_pending && self.rebuild_target_key == Some(key) {
            return;
        }
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
                let mesh = build_fault_mesh(&global, &china, &view, dark, quality);
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

/// 相对缓存构建视角：平移/缩放是否大到不该继续复用旧 mesh。
fn view_jump_too_far(build_lon: f64, build_lat: f64, build_zoom: f64, view: &ViewCtx) -> bool {
    if (build_zoom - view.zoom).abs() > 0.35 {
        return true;
    }
    let dlat = (build_lat - view.center_lat).abs();
    let mut dlon = (build_lon - view.center_lon).abs();
    if dlon > 180.0 {
        dlon = 360.0 - dlon;
    }
    // 约 2.5° 或以上视为列表跳转/飞跃落地，需重裁
    dlat > 2.5 || dlon > 2.5
}

impl FaultLods {
    fn total_segs(&self) -> usize {
        self.lod0.segs.len() + self.lod1.segs.len() + self.lod2.segs.len()
    }

    fn layers_for_zoom(&self) -> [&SegLayer; 3] {
        [&self.lod0, &self.lod1, &self.lod2]
    }
}

fn lod_mask(zoom: f64) -> [bool; 3] {
    if zoom < 3.2 {
        [true, false, false]
    } else if zoom < 5.2 {
        [true, true, false]
    } else {
        [true, true, true]
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
        let short: Vec<FaultSeg> = fine_all
            .into_iter()
            .filter(|s| s.diag < LOD1_MIN_DIAG)
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
        if s.diag >= LOD0_MIN_DIAG {
            lod0.push(s);
        } else if s.diag >= LOD1_MIN_DIAG {
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
) -> Mesh {
    let (min_lon, min_lat, max_lon, max_lat) = view_lonlat_bounds(view);
    let (step_bias, stride_mul, budget_scale) = match quality {
        PaintQuality::Eco => (2usize, 3usize, 0.35_f32),
        PaintQuality::Normal => (0, 1, 1.0),
        PaintQuality::Extreme => (0, 1, 1.15),
    };
    let globe = view.mode.is_globe();
    let zoom_budget = zoom_budget_scale(view.zoom);
    // 球体可见半球更宽，略抬预算；交互档仍靠 quality 降载
    let globe_mul = if globe { 1.55_f32 } else { 1.0 };
    let step = lod_step(view.zoom) + step_bias;
    let stride_g = (seg_stride(view.zoom, false, globe) * stride_mul).max(1);
    let stride_c = (seg_stride(view.zoom, true, globe) * stride_mul).max(1);
    let min_diag = min_diag_deg(view.zoom);
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
    let mask = lod_mask(view.zoom);

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

    let keep = (budget.saturating_mul(3))
        .max(budget)
        .min(candidates.len());
    if keep < candidates.len() {
        candidates.select_nth_unstable_by(keep, |&a, &b| {
            let da = layer.segs[a as usize].diag;
            let db = layer.segs[b as usize].diag;
            db.total_cmp(&da).then_with(|| a.cmp(&b))
        });
        candidates.truncate(keep);
    }
    candidates.sort_unstable_by(|&a, &b| {
        let da = layer.segs[a as usize].diag;
        let db = layer.segs[b as usize].diag;
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
        if min_diag > 0.0 && seg.diag < min_diag {
            continue;
        }
        if stride > 1 && seg.diag < keep_diag && (idx as usize) % stride != 0 {
            continue;
        }

        if step == 1 {
            paint_line_mesh(mesh, &seg.line, view, width, color, pts);
        } else {
            thin.clear();
            thin.reserve(seg.line.len() / step + 2);
            for (i, p) in seg.line.iter().enumerate() {
                if i % step == 0 || i + 1 == seg.line.len() {
                    thin.push(*p);
                }
            }
            if thin.len() >= 2 {
                paint_line_mesh(mesh, thin, view, width, color, pts);
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
                    .filter_map(|f| {
                        let geom = f.geometry.as_ref()?;
                        parse_lines(geom)
                    })
                    .flatten()
                    .filter_map(|line| {
                        if line.len() < 2 {
                            return None;
                        }
                        let bbox = line_bbox(&line);
                        let diag = ((bbox[2] - bbox[0]).hypot(bbox[3] - bbox[1])).max(0.0);
                        Some(FaultSeg { bbox, diag, line })
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

fn view_lonlat_bounds(view: &ViewCtx) -> (f64, f64, f64, f64) {
    if view.mode.is_globe() {
        // 用地平线半角（可见半球），旧公式 (60/zoom) 钳在 18° 导致球上断层加载范围过窄
        let half = view
            .horizon_half_deg()
            .unwrap_or_else(|| {
                let scroll = crate::globe::zoom_to_scroll(view.zoom);
                let cam_r = (1.0 + scroll).max(1.05);
                cam_r.recip().acos().to_degrees()
            })
            .clamp(28.0, 95.0)
            * 1.08;
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
            (min_lon - 1.5, min_lat - 1.5, max_lon + 1.5, max_lat + 1.5)
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
    let base = if zoom < 2.5 {
        if globe { 10 } else { 18 }
    } else if zoom < 3.2 {
        if globe { 6 } else { 10 }
    } else if zoom < 4.0 {
        if globe { 3 } else { 5 }
    } else if zoom < 5.5 {
        2
    } else {
        1
    };
    if china && zoom < 5.0 && !globe {
        (base * 2).min(24)
    } else {
        base
    }
}

fn min_diag_deg(zoom: f64) -> f32 {
    if zoom < 2.8 {
        1.4
    } else if zoom < 3.8 {
        0.7
    } else if zoom < 5.0 {
        0.28
    } else if zoom < 6.5 {
        0.1
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
    let w = if zoom < 4.0 {
        1.05_f32
    } else if zoom < 6.5 {
        1.25_f32
    } else {
        1.45_f32
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

fn paint_key(view: &ViewCtx, dark: bool, ppp: f32, quality: PaintQuality) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    view.mode.hash(&mut h);
    // 量化收紧：球体无平移补偿，过粗会粘滞；平面仍靠 can_shift 吃微移
    ((view.center_lon * 100.0).round() as i64).hash(&mut h);
    ((view.center_lat * 100.0).round() as i64).hash(&mut h);
    ((view.zoom * 28.0).round() as i64).hash(&mut h);
    ((view.rect.width() * 0.5).round() as i32).hash(&mut h);
    ((view.rect.height() * 0.5).round() as i32).hash(&mut h);
    ((ppp * 2.0).round() as i32).hash(&mut h);
    dark.hash(&mut h);
    quality.hash(&mut h);
    h.finish()
}

fn paint_base_key(view: &ViewCtx, dark: bool, ppp: f32, quality: PaintQuality) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    view.mode.hash(&mut h);
    ((view.zoom * 16.0).round() as i64).hash(&mut h);
    ((view.rect.width() * 0.25).round() as i32).hash(&mut h);
    ((view.rect.height() * 0.25).round() as i32).hash(&mut h);
    ((view.rect.min.x * 0.25).round() as i32).hash(&mut h);
    ((view.rect.min.y * 0.25).round() as i32).hash(&mut h);
    ((ppp * 2.0).round() as i32).hash(&mut h);
    dark.hash(&mut h);
    quality.hash(&mut h);
    h.finish()
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
) {
    if view.mode.is_globe() {
        paint_line_mesh_globe(mesh, line, view, width, color);
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
) {
    if line.len() < 2 {
        return;
    }
    let half_km = view
        .px_to_km((width_px * 0.5).max(0.6))
        .unwrap_or(2.5)
        .clamp(0.6, 14.0);
    // 先按视距加密折线，再逐段贴面
    let densified = densify_fault_line_globe(line, view);
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

fn densify_fault_line_globe(line: &[[f32; 2]], view: &ViewCtx) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(line.len() * 3);
    // 更密的大圆细分；高缩放限制每段上限，避免跳转落地同步镶嵌过慢
    let step_km = view.px_to_km(8.0).unwrap_or(16.0).clamp(4.0, 36.0);
    let max_div = if view.zoom >= 7.0 {
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
    fn partition_exclusive_tiers() {
        let segs = vec![
            FaultSeg {
                bbox: [0.0, 0.0, 2.0, 2.0],
                diag: 2.8,
                line: vec![[0.0, 0.0], [2.0, 2.0]],
            },
            FaultSeg {
                bbox: [0.0, 0.0, 0.5, 0.5],
                diag: 0.5,
                line: vec![[0.0, 0.0], [0.5, 0.5]],
            },
            FaultSeg {
                bbox: [0.0, 0.0, 0.1, 0.1],
                diag: 0.1,
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
    fn globe_cull_covers_visible_hemisphere() {
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 800.0));
        let view = ViewCtx::new(ProjectionMode::Globe, 110.0, 32.0, 4.5, rect);
        let (min_lon, min_lat, max_lon, max_lat) = view_lonlat_bounds(&view);
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
        let ka = paint_key(&a, true, 1.0, PaintQuality::Normal);
        let kb = paint_key(&b, true, 1.0, PaintQuality::Normal);
        assert_eq!(ka, kb);
    }
}
