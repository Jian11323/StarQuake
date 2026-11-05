//! 融合历史 FE 地名修正：`fe_fix.txt` 1° 格网 + `fe_fix_region_data.json` 外包矩形。
//!
//! 规则对齐融合历史 `TranslateLocation`：JMA / CWA / HKO / CENC / 国内地方局保留原文；
//! 其余源在有效经纬时用格网（优先）再回退矩形。

use serde::Deserialize;
use std::path::Path;
use std::sync::{OnceLock, RwLock};
use tracing::{info, warn};

const MAX_FE_REGION_AREA: f64 = 8.0;
const FALLBACK_FE_REGION_AREA: f64 = 80.0;

static FE: OnceLock<RwLock<FeGeo>> = OnceLock::new();

#[derive(Default)]
struct FeGeo {
    grid: Vec<Vec<i32>>,
    names: Vec<String>,
    regions: Vec<FeRegion>,
}

#[derive(Clone, Deserialize)]
struct FeRegion {
    name: String,
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
}

#[derive(Deserialize)]
struct RegionFile {
    regions: Vec<FeRegion>,
}

fn geo() -> &'static RwLock<FeGeo> {
    FE.get_or_init(|| RwLock::new(FeGeo::default()))
}

/// 从 `media/place/`（或任意含 `fe_fix.txt` 的目录）加载格网。可重复调用。
pub fn load_from_dir(dir: impl AsRef<Path>) {
    let dir = dir.as_ref();
    let mut loaded = FeGeo::default();
    let txt = dir.join("fe_fix.txt");
    match std::fs::read_to_string(&txt) {
        Ok(s) => {
            if let Some((grid, names)) = parse_fe_txt(&s) {
                loaded.grid = grid;
                loaded.names = names;
            } else {
                warn!(path = %txt.display(), "fe_fix.txt 无法解析");
            }
        }
        Err(e) => warn!(path = %txt.display(), error = %e, "未读取 fe_fix.txt"),
    }
    let json = dir.join("fe_fix_region_data.json");
    match std::fs::read_to_string(&json) {
        Ok(s) => match serde_json::from_str::<RegionFile>(&s) {
            Ok(f) => loaded.regions = f.regions,
            Err(e) => warn!(path = %json.display(), error = %e, "fe_fix_region_data.json 无法解析"),
        },
        Err(e) => warn!(path = %json.display(), error = %e, "未读取 fe_fix_region_data.json"),
    }
    let rows = loaded.grid.len();
    let names = loaded.names.len();
    let regions = loaded.regions.len();
    *geo().write().expect("fe geo lock") = loaded;
    if rows > 0 && names > 0 {
        info!(rows, names, regions, "FE 地名格网已加载");
    }
}

/// 按融合历史规则修正地名。
pub fn fix_place(agency: &str, place: &str, lat: f64, lon: f64) -> String {
    let place = place.trim();
    let src = source_key(agency);
    if keeps_original(&src) {
        return if place.is_empty() {
            "未知地区".into()
        } else {
            place.to_string()
        };
    }
    if has_coords(lat, lon) {
        if let Some(name) = lookup(lat, lon) {
            return name;
        }
    }
    if place.is_empty() {
        "未知地区".into()
    } else {
        place.to_string()
    }
}

fn keeps_original(src: &str) -> bool {
    matches!(
        src,
        "JMA" | "CWA" | "HKO" | "BEIJING" | "YUNNAN" | "NINGXIA" | "CENC" | "CEA-PR"
    )
}

fn source_key(agency: &str) -> String {
    let a = agency.trim().to_ascii_uppercase().replace('_', "-");
    if a.contains("JMA") || a.contains("P2P") {
        return "JMA".into();
    }
    if a.contains("CWA") {
        return "CWA".into();
    }
    if a.contains("HKO") {
        return "HKO".into();
    }
    if a.starts_with("CEA-PR") {
        return "CEA-PR".into();
    }
    if a.contains("CENC") || a == "CEA" || a.starts_with("CEA-") || a.starts_with("CEA:") {
        return "CENC".into();
    }
    if a.contains("BEIJING") {
        return "BEIJING".into();
    }
    if a.contains("YUNNAN") {
        return "YUNNAN".into();
    }
    if a.contains("NINGXIA") {
        return "NINGXIA".into();
    }
    a
}

fn has_coords(lat: f64, lon: f64) -> bool {
    lat.abs() > 1e-9 || lon.abs() > 1e-9
}

fn lookup(lat: f64, lon: f64) -> Option<String> {
    let g = geo().read().ok()?;
    if let Some(name) = g.grid_lookup(lat, lon) {
        return Some(name);
    }
    if let Some(name) = g.region_lookup(lat, lon, MAX_FE_REGION_AREA) {
        return Some(name);
    }
    g.region_lookup(lat, lon, FALLBACK_FE_REGION_AREA)
}

impl FeGeo {
    fn grid_lookup(&self, lat: f64, lon: f64) -> Option<String> {
        if self.grid.is_empty() || self.names.is_empty() {
            return None;
        }
        let mut lat = lat.clamp(-90.0, 90.0);
        let mut lon = lon.clamp(-180.0, 180.0);
        if lat >= 90.0 {
            lat = 89.9999;
        }
        if lon >= 180.0 {
            lon = 179.9999;
        }
        let li = (lat + 90.0) as i32 as usize;
        let lj = (lon + 180.0) as i32 as usize;
        let row = self.grid.get(li)?;
        let id = *row.get(lj)?;
        if id < 1 {
            return None;
        }
        self.names
            .get((id as usize) - 1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn region_lookup(&self, lat: f64, lon: f64, max_area: f64) -> Option<String> {
        let mut best: Option<(&str, f64)> = None;
        for r in &self.regions {
            if lat < r.lat_min || lat > r.lat_max || lon < r.lon_min || lon > r.lon_max {
                continue;
            }
            let area = (r.lat_max - r.lat_min) * (r.lon_max - r.lon_min);
            if max_area > 0.0 && area > max_area {
                continue;
            }
            if best.map(|(_, a)| area < a).unwrap_or(true) {
                best = Some((r.name.as_str(), area));
            }
        }
        best.map(|(n, _)| n.to_string())
    }
}

fn parse_fe_txt(content: &str) -> Option<(Vec<Vec<i32>>, Vec<String>)> {
    let content = content.trim_start_matches('\u{feff}');
    let numbers_key = "const feNumbers = [";
    let names_key = "const feNames = [";
    let n0 = content.find(numbers_key)?;
    let n1 = content.find(names_key)?;
    if n1 <= n0 {
        return None;
    }
    let grid_body = &content[n0 + numbers_key.len()..n1];
    let names_body = content[n1 + names_key.len()..].trim_end();
    let names_body = names_body
        .strip_suffix(';')
        .unwrap_or(names_body)
        .strip_suffix(']')
        .unwrap_or(names_body);

    let mut grid = Vec::new();
    let mut rest = grid_body;
    while let Some(start) = rest.find('[') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find(']') else {
            break;
        };
        let row_s = &rest[..end];
        rest = &rest[end + 1..];
        let row: Vec<i32> = row_s
            .split(',')
            .filter_map(|t| {
                let t = t.trim();
                if t.is_empty() {
                    None
                } else {
                    t.parse().ok()
                }
            })
            .collect();
        if !row.is_empty() {
            grid.push(row);
        }
    }

    let mut names = Vec::new();
    let mut in_str = false;
    let mut cur = String::new();
    for ch in names_body.chars() {
        match ch {
            '"' if in_str => {
                names.push(std::mem::take(&mut cur));
                in_str = false;
            }
            '"' => in_str = true,
            c if in_str => cur.push(c),
            _ => {}
        }
    }
    if grid.len() < 180 || names.is_empty() {
        return None;
    }
    Some((grid, names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../media/place")
    }

    #[test]
    fn jma_keeps_original() {
        assert_eq!(fix_place("jma-eew", "浦河沖", 41.0, 144.0), "浦河沖");
        assert_eq!(fix_place("wolfx-cenc", "巴拿马", 7.5, -80.6), "巴拿马");
        assert_eq!(fix_place("p2pquake", "胆振地方中東部", 42.0, 142.0), "胆振地方中東部");
    }

    #[test]
    fn international_uses_fe_grid() {
        load_from_dir(fixture_dir());
        let name = fix_place("usgs", "Western Honshu", 35.0, 133.0);
        assert!(!name.is_empty());
        assert_ne!(name, "Western Honshu");
    }
}
