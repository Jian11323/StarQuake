//! JMA 海啸沿岸线段：按预报区名称匹配描边并闪烁。

use crate::projection::{self, ViewCtx};
use egui::{Color32, Stroke, Ui};
use starquake_core::{TsunamiArea, tsunami_official_rgb};
use serde::Deserialize;
use std::path::Path;
use tracing::{info, warn};

#[derive(Clone)]
struct CoastSeg {
    name: String,
    lines: Vec<Vec<[f32; 2]>>,
}

#[derive(Default)]
pub struct TsunamiCoast {
    segs: Vec<CoastSeg>,
    loaded: bool,
}

#[derive(Deserialize)]
struct Fc {
    features: Vec<Feat>,
}

#[derive(Deserialize)]
struct Feat {
    #[serde(default)]
    geometry: Option<Geom>,
    properties: Props,
}

#[derive(Deserialize)]
struct Props {
    name: String,
}

#[derive(Deserialize)]
struct Geom {
    #[serde(rename = "type")]
    typ: String,
    coordinates: serde_json::Value,
}

impl TsunamiCoast {
    pub fn load_dir(&mut self, dir: &Path) {
        if self.loaded {
            return;
        }
        let path = dir.join("japan_tsunami_coast.geojson");
        if !path.is_file() {
            warn!(path = %path.display(), "缺少海啸沿岸 GeoJSON");
            self.loaded = true;
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Fc>(&s) {
                Ok(fc) => {
                    self.segs = fc
                        .features
                        .into_iter()
                        .filter_map(|f| {
                            let geom = f.geometry.as_ref()?;
                            let lines = parse_lines(geom)?;
                            if lines.is_empty() {
                                return None;
                            }
                            Some(CoastSeg {
                                name: f.properties.name,
                                lines,
                            })
                        })
                        .collect();
                    info!(n = self.segs.len(), "loaded japan tsunami coast");
                    self.loaded = true;
                }
                Err(e) => {
                    warn!(error = %e, "海啸沿岸 GeoJSON 解析失败");
                    self.loaded = true;
                }
            },
            Err(e) => {
                warn!(error = %e, "读取海啸沿岸 GeoJSON 失败");
                self.loaded = true;
            }
        }
    }

    /// 绘制 Forecast / Watch / Warning / MajorWarning 沿岸区。
    /// 闪烁：每 5 秒隐藏 1 秒（对齐要石）。
    pub fn paint(
        &self,
        ui: &mut Ui,
        view: &ViewCtx,
        areas: &[TsunamiArea],
        now_ms: i64,
    ) {
        if self.segs.is_empty() || areas.is_empty() {
            return;
        }
        // 隐藏帧：整秒 % 5 == 0；只预约到下一秒边界，勿每帧抢 200ms 盖过空闲降频
        let sec = now_ms / 1000;
        let ms_into_sec = (now_ms.rem_euclid(1000)) as u64;
        let until_next_sec = (1000 - ms_into_sec).max(50);
        if sec.rem_euclid(5) == 0 {
            crate::request_repaint_after_capped(ui.ctx(), until_next_sec);
            return;
        }
        crate::request_repaint_after_capped(ui.ctx(), until_next_sec.min(1000));

        let mut by_grade: Vec<(String, Color32)> = Vec::new();
        for a in areas {
            let g = a.grade.as_str();
            if matches!(g, "Cancel" | "") {
                continue;
            }
            let (r, gch, b) = tsunami_official_rgb(g);
            let color = Color32::from_rgba_unmultiplied(r, gch, b, 255);
            by_grade.push((a.name.clone(), color));
        }
        if by_grade.is_empty() {
            return;
        }

        let painter = ui.painter_at(view.rect);
        let mut shapes = Vec::new();
        let width = 3.5_f32;
        for (name, color) in &by_grade {
            let stroke = Stroke::new(width, *color);
            for seg in &self.segs {
                if !name_match(&seg.name, name) {
                    continue;
                }
                for line in &seg.lines {
                    paint_line(&mut shapes, line, view, stroke);
                }
            }
        }
        painter.extend(shapes);
    }

}

fn name_match(seg: &str, area: &str) -> bool {
    if seg == area {
        return true;
    }
    // 兼容简称 / 后缀差异
    if seg.contains(area) || area.contains(seg) {
        return true;
    }
    false
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

fn paint_line(
    shapes: &mut Vec<egui::Shape>,
    line: &[[f32; 2]],
    view: &ViewCtx,
    stroke: Stroke,
) {
    let mut pts = Vec::with_capacity(line.len());
    let mut last: Option<egui::Pos2> = None;
    let max_jump = if view.mode.is_globe() {
        view.horizon_radius().unwrap_or(80.0) * 0.35
    } else {
        projection::world_size(view.zoom) as f32 * (80.0 / 360.0)
    };
    for p in line {
        let Some(sp) = view.project(p[0] as f64, p[1] as f64) else {
            if pts.len() >= 2 {
                shapes.push(
                    egui::epaint::PathShape {
                        points: std::mem::take(&mut pts),
                        closed: false,
                        fill: Color32::TRANSPARENT,
                        stroke: egui::epaint::PathStroke::new(stroke.width, stroke.color),
                    }
                    .into(),
                );
            } else {
                pts.clear();
            }
            last = None;
            continue;
        };
        if let Some(prev) = last {
            let dx = sp.x - prev.x;
            let dy = sp.y - prev.y;
            if dx * dx + dy * dy > max_jump * max_jump {
                if pts.len() >= 2 {
                    shapes.push(
                        egui::epaint::PathShape {
                            points: std::mem::take(&mut pts),
                            closed: false,
                            fill: Color32::TRANSPARENT,
                            stroke: egui::epaint::PathStroke::new(stroke.width, stroke.color),
                        }
                        .into(),
                    );
                } else {
                    pts.clear();
                }
            }
        }
        pts.push(sp);
        last = Some(sp);
    }
    if pts.len() >= 2 {
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
}
