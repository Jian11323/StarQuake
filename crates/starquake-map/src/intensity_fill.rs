//! 预警烈度/震度分布：行政区填色（对齐要石 / RhythmQuake），非正式测定。
//!
//! - 国内 CEA / 省网：中国 EEW 区划 + 要石 CSIS 衰减
//! - 台湾 CWA：台湾县市区划 + JMA 計測衰减（县域填色，非同心圆）
//! - JMA：仅当 `warn_areas` 非空时，按预报区 name/code 填色

use crate::projection::{self, ViewCtx};
use crate::topojson::{self, NamedRegionPoly};
use egui::{Color32, Pos2, Stroke, Ui};
use starquake_core::agency::{agency_family, AgencyFamily};
use starquake_core::palette::color_for;
use starquake_core::{
    calc_csis_kanameishi, calc_jma_instrumental, cn_intensity_round, jma_from_instrumental,
    jma_shindo_level, EewReport, EewWarnArea, IntensityKind,
};
use starquake_util::haversine_km;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use tracing::{info, warn};

const FILL_ALPHA: u8 = 90;
const STROKE_ALPHA: u8 = 160;
const MIN_LEVEL: u8 = 1;

/// 台湾县市（cn.eew 简体名 + 繁体别名）
const TW_COUNTIES: &[&str] = &[
    "台北市",
    "台中市",
    "台南市",
    "高雄市",
    "新北市",
    "基隆市",
    "新竹市",
    "嘉义市",
    "桃园市",
    "新竹县",
    "苗栗县",
    "彰化县",
    "南投县",
    "云林县",
    "嘉义县",
    "屏东县",
    "宜兰县",
    "花莲县",
    "台东县",
    "澎湖县",
    "金门县",
    "连江县",
    "臺北市",
    "臺中市",
    "臺南市",
    "桃園市",
    "新竹縣",
    "苗栗縣",
    "彰化縣",
    "南投縣",
    "雲林縣",
    "嘉義縣",
    "屏東縣",
    "宜蘭縣",
    "花蓮縣",
    "臺東縣",
    "澎湖縣",
    "金門縣",
    "連江縣",
];

#[derive(Clone)]
struct Region {
    name: String,
    code: String,
    polys: Vec<(Vec<[f32; 2]>, Vec<Vec<[f32; 2]>>)>,
    center_lon: f64,
    center_lat: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CacheKind {
    #[default]
    None,
    Cn,
    Tw,
    Jp,
}

#[derive(Default)]
pub struct IntensityFill {
    cn: Vec<Region>,
    tw: Vec<Region>,
    jp: Vec<Region>,
    loaded: bool,
    cache_key: String,
    cache_kind: CacheKind,
    /// `(level, region_index)`，低档在前
    cached: Vec<(u8, usize)>,
}

impl IntensityFill {
    pub fn load_dir(&mut self, dir: &Path) {
        if self.loaded {
            return;
        }
        let kdir = dir.join("kanameishi");
        self.cn = load_regions(&kdir.join("cn.eew.topo.json"), RegionFilter::All);
        self.tw = load_regions(&kdir.join("cn.eew.topo.json"), RegionFilter::Taiwan);
        self.jp = load_regions(&kdir.join("jp.eew.topo.json"), RegionFilter::All);
        info!(
            cn = self.cn.len(),
            tw = self.tw.len(),
            jp = self.jp.len(),
            "loaded intensity fill regions"
        );
        self.loaded = true;
    }

    pub fn paint(&mut self, ui: &mut Ui, view: &ViewCtx, ev: &EewReport) {
        if !self.loaded || !intensity_map_eligible(&ev.agency.0) {
            return;
        }
        if ev.is_cancel || !ev.magnitude.is_finite() || ev.magnitude < 3.0 {
            return;
        }
        if ev.latitude.abs() < 0.01 && ev.longitude.abs() < 0.01 {
            return;
        }

        match agency_family(&ev.agency.0) {
            AgencyFamily::Jma => {
                if ev.warn_areas.is_empty() {
                    return;
                }
                refresh_jma_cache(self, ev);
                paint_cached(ui, view, &self.jp, &self.cached, IntensityKind::JmaShindo);
            }
            AgencyFamily::Cwa => {
                refresh_atten_cache(self, ev, CacheKind::Tw);
                paint_cached(ui, view, &self.tw, &self.cached, IntensityKind::JmaShindo);
            }
            AgencyFamily::Cea | AgencyFamily::CeaPr => {
                refresh_atten_cache(self, ev, CacheKind::Cn);
                paint_cached(ui, view, &self.cn, &self.cached, IntensityKind::CnIntensity);
            }
            _ => {}
        }
    }
}

/// 仅 cea / cea-pr / cwa-eew / jma-eew（含 Wolfx 镜像）参与烈度分布绘制。
pub fn intensity_map_eligible(agency: &str) -> bool {
    matches!(
        agency_family(agency),
        AgencyFamily::Cea | AgencyFamily::CeaPr | AgencyFamily::Cwa | AgencyFamily::Jma
    )
}

fn refresh_jma_cache(fill: &mut IntensityFill, ev: &EewReport) {
    let key = format!("jp|{}|{}|{}", ev.event_id, ev.serial, warn_sig(&ev.warn_areas));
    if fill.cache_key == key && fill.cache_kind == CacheKind::Jp {
        return;
    }
    let mut by_name: HashMap<&str, u8> = HashMap::new();
    let mut by_code: HashMap<&str, u8> = HashMap::new();
    for a in &ev.warn_areas {
        let level = warn_area_level(a);
        if level < MIN_LEVEL {
            continue;
        }
        if !a.name.is_empty() {
            let e = by_name.entry(a.name.as_str()).or_insert(0);
            *e = (*e).max(level);
        }
        if !a.code.is_empty() {
            let e = by_code.entry(a.code.as_str()).or_insert(0);
            *e = (*e).max(level);
        }
    }
    let mut cached = Vec::new();
    for (i, r) in fill.jp.iter().enumerate() {
        let mut level = 0_u8;
        if !r.code.is_empty() {
            if let Some(&lv) = by_code.get(r.code.as_str()) {
                level = level.max(lv);
            }
        }
        if !r.name.is_empty() {
            if let Some(&lv) = by_name.get(r.name.as_str()) {
                level = level.max(lv);
            } else {
                for (n, &lv) in &by_name {
                    if r.name.contains(n) || n.contains(r.name.as_str()) {
                        level = level.max(lv);
                    }
                }
            }
        }
        if level >= MIN_LEVEL {
            cached.push((level, i));
        }
    }
    cached.sort_by(|a, b| a.0.cmp(&b.0));
    fill.cache_key = key;
    fill.cache_kind = CacheKind::Jp;
    fill.cached = cached;
}

fn refresh_atten_cache(fill: &mut IntensityFill, ev: &EewReport, kind: CacheKind) {
    let key = format!(
        "{:?}|{}|{}|{:.4}|{:.4}|{:.2}|{:.1}",
        kind, ev.event_id, ev.serial, ev.latitude, ev.longitude, ev.magnitude, ev.depth_km
    );
    if fill.cache_key == key && fill.cache_kind == kind {
        return;
    }
    let regions: &[Region] = match kind {
        CacheKind::Cn => &fill.cn,
        CacheKind::Tw => &fill.tw,
        _ => &[],
    };
    let ikind = match kind {
        CacheKind::Cn => IntensityKind::CnIntensity,
        _ => IntensityKind::JmaShindo,
    };
    let mut cached = Vec::new();
    for (i, r) in regions.iter().enumerate() {
        let surface = if point_in_any_poly(ev.longitude, ev.latitude, &r.polys) {
            0.0
        } else {
            haversine_km(ev.longitude, ev.latitude, r.center_lon, r.center_lat)
        };
        let level = match ikind {
            IntensityKind::CnIntensity => {
                cn_intensity_round(calc_csis_kanameishi(ev.magnitude, ev.depth_km, surface))
            }
            IntensityKind::JmaShindo => {
                let inst = calc_jma_instrumental(ev.magnitude, ev.depth_km.max(0.0), surface);
                jma_from_instrumental(inst).0
            }
        };
        if level >= MIN_LEVEL {
            cached.push((level, i));
        }
    }
    cached.sort_by(|a, b| a.0.cmp(&b.0));
    fill.cache_key = key;
    fill.cache_kind = kind;
    fill.cached = cached;
}

fn paint_cached(
    ui: &mut Ui,
    view: &ViewCtx,
    regions: &[Region],
    cached: &[(u8, usize)],
    kind: IntensityKind,
) {
    if cached.is_empty() {
        return;
    }
    let painter = ui.painter_at(view.rect);
    let mut shapes = Vec::new();
    for &(level, idx) in cached {
        let Some(r) = regions.get(idx) else {
            continue;
        };
        let rgb = color_for(kind, level);
        let fill = Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, FILL_ALPHA);
        let stroke = Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(rgb.r, rgb.g, rgb.b, STROKE_ALPHA),
        );
        for (exterior, holes) in &r.polys {
            paint_poly(&mut shapes, view, exterior, holes, fill, stroke);
        }
    }
    painter.extend(shapes);
}

enum RegionFilter {
    All,
    Taiwan,
}

fn load_regions(path: &Path, filter: RegionFilter) -> Vec<Region> {
    if !path.is_file() {
        warn!(path = %path.display(), "缺少烈度区划 TopoJSON");
        return Vec::new();
    }
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, path = %path.display(), "读取烈度区划失败");
            return Vec::new();
        }
    };
    let polys = match topojson::load_named_region_polygons_ex(&text) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, path = %path.display(), "解析烈度区划失败");
            return Vec::new();
        }
    };
    let mut by_key: HashMap<String, Region> = HashMap::new();
    for NamedRegionPoly {
        name,
        code,
        exterior,
        holes,
    } in polys
    {
        if exterior.len() < 3 {
            continue;
        }
        let keep = match filter {
            RegionFilter::All => true,
            RegionFilter::Taiwan => is_taiwan_county(&name),
        };
        if !keep {
            continue;
        }
        let key = if !code.is_empty() {
            format!("c:{code}")
        } else {
            format!("n:{name}")
        };
        let e = by_key.entry(key).or_insert_with(|| Region {
            name: name.clone(),
            code: code.clone(),
            polys: Vec::new(),
            center_lon: 0.0,
            center_lat: 0.0,
        });
        e.polys.push((exterior, holes));
        let (alon, alat, n) = all_centroid(&e.polys);
        if n > 0 {
            e.center_lon = alon;
            e.center_lat = alat;
        }
    }
    by_key.into_values().collect()
}

fn is_taiwan_county(name: &str) -> bool {
    TW_COUNTIES.iter().any(|c| *c == name)
}

fn all_centroid(polys: &[(Vec<[f32; 2]>, Vec<Vec<[f32; 2]>>)]) -> (f64, f64, usize) {
    let mut sx = 0.0_f64;
    let mut sy = 0.0_f64;
    let mut n = 0usize;
    for (ext, _) in polys {
        for p in ext {
            sx += p[0] as f64;
            sy += p[1] as f64;
            n += 1;
        }
    }
    if n == 0 {
        (0.0, 0.0, 0)
    } else {
        (sx / n as f64, sy / n as f64, n)
    }
}

fn point_in_ring(lon: f64, lat: f64, ring: &[[f32; 2]]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let yi = ring[i][1] as f64;
        let xi = ring[i][0] as f64;
        let yj = ring[j][1] as f64;
        let xj = ring[j][0] as f64;
        if ((yi > lat) != (yj > lat))
            && (lon < (xj - xi) * (lat - yi) / ((yj - yi) + f64::EPSILON) + xi)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn point_in_any_poly(
    lon: f64,
    lat: f64,
    polys: &[(Vec<[f32; 2]>, Vec<Vec<[f32; 2]>>)],
) -> bool {
    for (ext, holes) in polys {
        if !point_in_ring(lon, lat, ext) {
            continue;
        }
        if !holes.iter().any(|h| point_in_ring(lon, lat, h)) {
            return true;
        }
    }
    false
}

fn warn_sig(areas: &[EewWarnArea]) -> u64 {
    let mut h = DefaultHasher::new();
    for a in areas {
        a.name.hash(&mut h);
        a.code.hash(&mut h);
        a.intensity_from.hash(&mut h);
        a.intensity_to.hash(&mut h);
    }
    h.finish()
}

fn warn_area_level(a: &EewWarnArea) -> u8 {
    let to = if a.intensity_to.is_empty() {
        0
    } else {
        jma_shindo_level(&a.intensity_to)
    };
    let from = if a.intensity_from.is_empty() {
        0
    } else {
        jma_shindo_level(&a.intensity_from)
    };
    to.max(from)
}

fn paint_poly(
    shapes: &mut Vec<egui::Shape>,
    view: &ViewCtx,
    exterior: &[[f32; 2]],
    holes: &[Vec<[f32; 2]>],
    fill: Color32,
    stroke: Stroke,
) {
    let Some(ext_pts) = project_ring(view, exterior) else {
        return;
    };
    if ext_pts.len() < 3 {
        return;
    }
    let mut flat: Vec<f64> = Vec::with_capacity(ext_pts.len() * 2);
    let mut hole_indices: Vec<usize> = Vec::new();
    for p in &ext_pts {
        flat.push(p.x as f64);
        flat.push(p.y as f64);
    }
    let mut hole_rings: Vec<Vec<Pos2>> = Vec::new();
    for h in holes {
        let Some(hp) = project_ring(view, h) else {
            continue;
        };
        if hp.len() < 3 {
            continue;
        }
        hole_indices.push(flat.len() / 2);
        for p in &hp {
            flat.push(p.x as f64);
            flat.push(p.y as f64);
        }
        hole_rings.push(hp);
    }
    if let Ok(idx) = earcutr::earcut(&flat, &hole_indices, 2) {
        let mut mesh = egui::epaint::Mesh::default();
        let n = flat.len() / 2;
        mesh.reserve_vertices(n);
        mesh.reserve_triangles(idx.len() / 3);
        for i in 0..n {
            mesh.colored_vertex(
                Pos2::new(flat[i * 2] as f32, flat[i * 2 + 1] as f32),
                fill,
            );
        }
        for tri in idx.chunks_exact(3) {
            mesh.add_triangle(tri[0] as u32, tri[1] as u32, tri[2] as u32);
        }
        if !mesh.indices.is_empty() {
            shapes.push(egui::Shape::mesh(mesh));
        }
    }
    let mut closed = ext_pts;
    if let Some(first) = closed.first().copied() {
        closed.push(first);
    }
    shapes.push(egui::Shape::line(closed, stroke));
    for mut hp in hole_rings {
        if let Some(first) = hp.first().copied() {
            hp.push(first);
        }
        shapes.push(egui::Shape::line(hp, stroke));
    }
}

fn project_ring(view: &ViewCtx, ring: &[[f32; 2]]) -> Option<Vec<Pos2>> {
    let mut pts = Vec::with_capacity(ring.len());
    let mut last: Option<Pos2> = None;
    for p in ring {
        let lon = projection::wrap_lon_near(p[0] as f64, view.center_lon);
        let Some(pos) = view.project(lon, p[1] as f64) else {
            if view.mode.is_globe() {
                return None;
            }
            continue;
        };
        if let Some(prev) = last {
            let dx = pos.x - prev.x;
            let dy = pos.y - prev.y;
            if dx * dx + dy * dy > 220.0_f32 * 220.0 {
                return None;
            }
        }
        pts.push(pos);
        last = Some(pos);
    }
    if pts.len() < 3 {
        None
    } else {
        Some(pts)
    }
}
