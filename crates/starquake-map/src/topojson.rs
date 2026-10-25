//! 量化 TopoJSON（`objects.region`）→ 多边形环，对齐要石 `topojson-client` 用法。

use serde::Deserialize;
use serde_json::Value;

type PolyRing = Vec<[f32; 2]>;
type PolyWithHoles = (PolyRing, Vec<PolyRing>);
type NamedPolyWithHoles = (String, PolyRing, Vec<PolyRing>);

#[derive(Deserialize)]
struct Topology {
    arcs: Vec<Vec<[i64; 2]>>,
    transform: Transform,
    objects: Objects,
}

#[derive(Deserialize)]
struct Transform {
    scale: [f64; 2],
    translate: [f64; 2],
}

#[derive(Deserialize)]
struct Objects {
    region: GeometryCollection,
}

#[derive(Deserialize)]
struct GeometryCollection {
    geometries: Vec<TopoGeometry>,
}

#[derive(Deserialize)]
struct TopoGeometry {
    #[serde(rename = "type")]
    geom_type: String,
    arcs: Option<Value>,
    #[serde(default)]
    properties: Option<TopoProps>,
}

#[derive(Deserialize)]
struct TopoProps {
    #[serde(default)]
    name: Option<String>,
}

/// 解析要石风格 TopoJSON，返回 `(exterior, holes)[]`，坐标为经纬。
pub fn load_region_polygons(
    text: &str,
) -> Result<Vec<PolyWithHoles>, String> {
    Ok(load_named_region_polygons(text)?
        .into_iter()
        .map(|(_, exterior, holes)| (exterior, holes))
        .collect())
}

/// 带 `properties.name` 的区划多边形（JMA EEW 预报区等）。
/// 返回 `(name, exterior, holes)`；无名几何 `name` 为空串。
pub fn load_named_region_polygons(
    text: &str,
) -> Result<Vec<NamedPolyWithHoles>, String> {
    let topo: Topology = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for geom in &topo.objects.region.geometries {
        let name = geom
            .properties
            .as_ref()
            .and_then(|p| p.name.as_ref())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let Some(arcs) = geom.arcs.as_ref() else {
            continue;
        };
        match geom.geom_type.as_str() {
            "Polygon" => {
                if let Some((exterior, holes)) = polygon_from_arcs(arcs, &topo) {
                    out.push((name, exterior, holes));
                }
            }
            "MultiPolygon" => {
                let Some(polys) = arcs.as_array() else {
                    continue;
                };
                for poly in polys {
                    if let Some((exterior, holes)) = polygon_from_arcs(poly, &topo) {
                        out.push((name.clone(), exterior, holes));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn polygon_from_arcs(
    arcs: &Value,
    topo: &Topology,
) -> Option<PolyWithHoles> {
    let rings = arcs.as_array()?;
    if rings.is_empty() {
        return None;
    }
    let exterior = ring_from_arc_indices(rings[0].as_array()?, topo)?;
    if exterior.len() < 3 {
        return None;
    }
    let mut holes = Vec::new();
    for hole in rings.iter().skip(1) {
        if let Some(h) = ring_from_arc_indices(hole.as_array()?, topo) {
            if h.len() >= 3 {
                holes.push(h);
            }
        }
    }
    Some((exterior, holes))
}

fn ring_from_arc_indices(indices: &[Value], topo: &Topology) -> Option<Vec<[f32; 2]>> {
    let mut ring: Vec<[f32; 2]> = Vec::new();
    for idx_v in indices {
        let idx = idx_v.as_i64()?;
        let mut pts = decode_arc_index(idx, topo)?;
        if !ring.is_empty() && !pts.is_empty() {
            pts.remove(0);
        }
        ring.extend(pts);
    }
    if ring.len() >= 2 {
        let a = ring[0];
        let b = ring[ring.len() - 1];
        if (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6 {
            ring.pop();
        }
    }
    Some(ring)
}

fn decode_arc_index(idx: i64, topo: &Topology) -> Option<Vec<[f32; 2]>> {
    let (arc_i, reverse) = if idx < 0 {
        ((!(idx as isize)) as usize, true)
    } else {
        (idx as usize, false)
    };
    let arc = topo.arcs.get(arc_i)?;
    let mut pts = decode_arc(arc, &topo.transform);
    if reverse {
        pts.reverse();
    }
    Some(pts)
}

fn decode_arc(arc: &[[i64; 2]], transform: &Transform) -> Vec<[f32; 2]> {
    let mut x = 0_i64;
    let mut y = 0_i64;
    let mut out = Vec::with_capacity(arc.len());
    for p in arc {
        x += p[0];
        y += p[1];
        let lon = (x as f64) * transform.scale[0] + transform.translate[0];
        let lat = (y as f64) * transform.scale[1] + transform.translate[1];
        out.push([lon as f32, lat as f32]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_tiny_topology() {
        let text = r#"{
          "type":"Topology",
          "transform":{"scale":[1,1],"translate":[0,0]},
          "arcs":[[[0,0],[1,0],[0,1],[-1,0],[0,-1]]],
          "objects":{"region":{"type":"GeometryCollection","geometries":[
            {"type":"Polygon","arcs":[[0]]}
          ]}}
        }"#;
        let polys = load_region_polygons(text).unwrap();
        assert_eq!(polys.len(), 1);
        assert!(polys[0].0.len() >= 3);
    }
}
