//! JMA2001 / ak135 走时表加载与插值。

use starquake_util::haversine_km;
use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TravelError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("表为空或网格无效")]
    InvalidTable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    P = 0,
    S = 1,
}

/// S 波最大传播距离（km）。震级越高越远；小震缩短以免远源小震长时间挂波圈。
pub fn max_s_wave_spread_km(magnitude: f64) -> f64 {
    if magnitude <= 2.0 {
        200.0
    } else if magnitude <= 2.5 {
        350.0
    } else if magnitude <= 3.0 {
        500.0
    } else if magnitude <= 3.5 {
        700.0
    } else if magnitude <= 4.0 {
        900.0
    } else if magnitude <= 4.5 {
        1200.0
    } else if magnitude <= 5.0 {
        1600.0
    } else if magnitude <= 5.5 {
        2100.0
    } else if magnitude <= 6.0 {
        2600.0
    } else if magnitude <= 6.5 {
        3400.0
    } else if magnitude <= 7.0 {
        4200.0
    } else if magnitude <= 7.5 {
        5200.0
    } else if magnitude <= 8.0 {
        6200.0
    } else {
        7500.0
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TravelTable {
    pub name: String,
    pub depths_km: Vec<f64>,
    pub distances_km: Vec<f64>,
    /// [depth_idx][dist_idx]
    pub p_times_s: Vec<Vec<f64>>,
    pub s_times_s: Vec<Vec<f64>>,
}

impl TravelTable {
    pub fn load_json(path: impl AsRef<Path>) -> Result<Self, TravelError> {
        let raw = std::fs::read_to_string(path)?;
        let t: Self = serde_json::from_str(&raw)?;
        if t.depths_km.is_empty() || t.distances_km.is_empty() {
            return Err(TravelError::InvalidTable);
        }
        Ok(t)
    }

    fn grid(&self, wave: Wave) -> &[Vec<f64>] {
        match wave {
            Wave::P => &self.p_times_s,
            Wave::S => &self.s_times_s,
        }
    }

    /// 双线性插值走时（秒）。
    /// 震中距超出本表最大距离时返回 `None`（禁止钳到表末，否则远源会被当成 ~2000km）。
    pub fn travel_time(&self, wave: Wave, depth_km: f64, distance_km: f64) -> Option<f64> {
        if self.distances_km.is_empty() {
            return None;
        }
        let d_max = *self.distances_km.last()?;
        if distance_km > d_max + 1e-6 {
            return None;
        }
        let di = bracket(&self.depths_km, depth_km)?;
        let dj = bracket(&self.distances_km, distance_km)?;
        let g = self.grid(wave);
        let (d0, d1) = (self.depths_km[di.0], self.depths_km[di.1]);
        let (x0, x1) = (self.distances_km[dj.0], self.distances_km[dj.1]);
        let td = if (d1 - d0).abs() < 1e-9 {
            0.0
        } else {
            (depth_km - d0) / (d1 - d0)
        };
        let tx = if (x1 - x0).abs() < 1e-9 {
            0.0
        } else {
            (distance_km - x0) / (x1 - x0)
        };
        let v00 = g[di.0][dj.0];
        let v01 = g[di.0][dj.1];
        let v10 = g[di.1][dj.0];
        let v11 = g[di.1][dj.1];
        let v0 = v00 + (v01 - v00) * tx;
        let v1 = v10 + (v11 - v10) * tx;
        Some(v0 + (v1 - v0) * td)
    }

    /// 给定走时求表面距离（km），在深度行上对距离一维插值。
    /// 波尚未出地表时返回负值（对齐 eewcn：`sec*depth/t0 - depth`），供 P/S 进度条使用。
    pub fn distance_for_time(&self, wave: Wave, depth_km: f64, sec: f64) -> Option<f64> {
        let di = bracket(&self.depths_km, depth_km)?;
        let g = self.grid(wave);
        let row = &g[di.0];
        let depth = depth_km.max(0.0);
        if sec <= row[0] {
            // eewcn TravelTime.qml：未达 0km 走时则按深度比例给负半径
            if row[0] <= 1e-9 {
                return Some(0.0);
            }
            return Some(sec * depth / row[0] - depth);
        }
        for i in 0..row.len().saturating_sub(1) {
            if row[i + 1] >= sec {
                let t0 = row[i];
                let t1 = row[i + 1];
                let x0 = self.distances_km[i];
                let x1 = self.distances_km[i + 1];
                if (t1 - t0).abs() < 1e-9 {
                    return Some(x0);
                }
                return Some(x0 + (sec - t0) / (t1 - t0) * (x1 - x0));
            }
        }
        None
    }
}

fn bracket(xs: &[f64], v: f64) -> Option<(usize, usize)> {
    if xs.is_empty() {
        return None;
    }
    if v <= xs[0] {
        return Some((0, 0));
    }
    if v >= xs[xs.len() - 1] {
        let i = xs.len() - 1;
        return Some((i, i));
    }
    for i in 0..xs.len() - 1 {
        if xs[i] <= v && v <= xs[i + 1] {
            return Some((i, i + 1));
        }
    }
    None
}

/// 走时引擎：近距 JMA2001，远距可选 ak135，最后直线兜底
pub struct TravelEngine {
    pub primary: TravelTable,
    pub fallback: Option<TravelTable>,
    pub linear_last_resort: bool,
}

fn resolve_table_path(dir: &Path, name: &str) -> std::path::PathBuf {
    let n = name.trim().to_ascii_lowercase();
    let stem = n
        .strip_suffix(".json")
        .unwrap_or(&n)
        .replace("_stub", "");
    let candidates = [
        dir.join(format!("{stem}.json")),
        dir.join(format!("{stem}_stub.json")),
        dir.join(format!("{n}.json")),
    ];
    for c in &candidates {
        if c.is_file() {
            return c.clone();
        }
    }
    candidates[0].clone()
}

impl TravelEngine {
    pub fn load_default(assets_travel: impl AsRef<Path>) -> Result<Self, TravelError> {
        Self::load_named(assets_travel, "jma2001", "ak135", true)
    }

    /// 按配置名加载：`jma2001` / `ak135`（文件可为 `ak135.json` 或 `ak135_stub.json`）。
    pub fn load_named(
        assets_travel: impl AsRef<Path>,
        primary: &str,
        fallback: &str,
        linear_last_resort: bool,
    ) -> Result<Self, TravelError> {
        let dir = assets_travel.as_ref();
        let primary = TravelTable::load_json(resolve_table_path(dir, primary))?;
        let fallback = TravelTable::load_json(resolve_table_path(dir, fallback)).ok();
        Ok(Self {
            primary,
            fallback,
            linear_last_resort,
        })
    }

    pub fn travel_time(&self, wave: Wave, depth_km: f64, distance_km: f64) -> f64 {
        if let Some(t) = self.primary.travel_time(wave, depth_km, distance_km) {
            return t;
        }
        if let Some(fb) = &self.fallback {
            if let Some(t) = fb.travel_time(wave, depth_km, distance_km) {
                return t;
            }
        }
        if !self.linear_last_resort {
            return f64::INFINITY;
        }
        // 最后兜底：P≈7 S≈5 km/s 斜距
        let v = match wave {
            Wave::P => 7.0,
            Wave::S => 5.0,
        };
        (depth_km.hypot(distance_km)) / v
    }

    pub fn eta_s_to_site(
        &self,
        lon: f64,
        lat: f64,
        depth_km: f64,
        site_lon: f64,
        site_lat: f64,
    ) -> f64 {
        let d = haversine_km(lon, lat, site_lon, site_lat);
        self.travel_time(Wave::S, depth_km, d)
    }

    /// 地表传播半径（km）。尚未出地表时为负（对齐 eewcn），画圈时请 `max(0, …)`。
    pub fn surface_distance_for_elapsed(&self, wave: Wave, depth_km: f64, elapsed_s: f64) -> f64 {
        if let Some(d) = self.primary.distance_for_time(wave, depth_km, elapsed_s) {
            return d;
        }
        if let Some(fb) = &self.fallback {
            if let Some(d) = fb.distance_for_time(wave, depth_km, elapsed_s) {
                return d;
            }
        }
        if !self.linear_last_resort {
            return 0.0;
        }
        // 斜距匀速：未出地表返回 r-depth（负），出地表后球面近似用直角三角形
        let v = match wave {
            Wave::P => 7.0,
            Wave::S => 5.0,
        };
        let slant = (elapsed_s * v).max(0.0);
        let depth = depth_km.max(0.0);
        if slant <= depth {
            slant - depth
        } else {
            (slant * slant - depth * depth).sqrt()
        }
    }
}

/// 与地图波圈一致：当前 S 地表半径是否仍小于上限（未结束）。
pub fn s_wave_still_spreading_to(
    eng: &TravelEngine,
    depth_km: f64,
    elapsed_s: f64,
    max_km: f64,
) -> bool {
    // 未出地表为负，仍算传播中；出地表后与上限比较
    eng.surface_distance_for_elapsed(Wave::S, depth_km, elapsed_s).max(0.0) < max_km
}

/// 与地图波圈一致：当前 S 地表半径是否仍小于震级上限（未结束）。
pub fn s_wave_still_spreading(
    eng: &TravelEngine,
    magnitude: f64,
    depth_km: f64,
    elapsed_s: f64,
) -> bool {
    s_wave_still_spreading_to(eng, depth_km, elapsed_s, max_s_wave_spread_km(magnitude))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn travel_time_rejects_beyond_table_max() {
        let t = TravelTable {
            name: "t".into(),
            depths_km: vec![0.0, 10.0],
            distances_km: vec![0.0, 1000.0, 2000.0],
            p_times_s: vec![vec![0.0, 100.0, 200.0], vec![1.0, 101.0, 201.0]],
            s_times_s: vec![vec![0.0, 180.0, 360.0], vec![2.0, 182.0, 362.0]],
        };
        assert!(t.travel_time(Wave::S, 5.0, 1500.0).is_some());
        assert!(t.travel_time(Wave::S, 5.0, 2000.0).is_some());
        assert!(t.travel_time(Wave::S, 5.0, 2000.1).is_none());
        assert!(t.travel_time(Wave::S, 5.0, 9000.0).is_none());
    }

    #[test]
    fn surface_distance_negative_before_emergence() {
        let eng = TravelEngine {
            primary: TravelTable {
                name: "t".into(),
                depths_km: vec![0.0, 10.0],
                distances_km: vec![0.0, 1000.0, 2000.0],
                // 深度 10km 处 0km 走时约 2s（S）
                p_times_s: vec![vec![0.0, 100.0, 200.0], vec![1.0, 101.0, 201.0]],
                s_times_s: vec![vec![0.0, 180.0, 360.0], vec![2.0, 182.0, 362.0]],
            },
            fallback: None,
            linear_last_resort: true,
        };
        let d = eng.surface_distance_for_elapsed(Wave::S, 10.0, 0.5);
        assert!(d < 0.0, "expected underground negative radius, got {d}");
        let d0 = eng.surface_distance_for_elapsed(Wave::S, 10.0, 2.0);
        assert!(d0 >= -1e-6, "at t0 should be near surface, got {d0}");
    }

    #[test]
    fn max_spread_scales_with_magnitude() {
        assert_eq!(max_s_wave_spread_km(2.0), 200.0);
        assert_eq!(max_s_wave_spread_km(2.5), 350.0);
        assert_eq!(max_s_wave_spread_km(3.0), 500.0);
        assert_eq!(max_s_wave_spread_km(4.0), 900.0);
        assert_eq!(max_s_wave_spread_km(5.5), 2100.0);
        assert_eq!(max_s_wave_spread_km(6.5), 3400.0);
        assert_eq!(max_s_wave_spread_km(7.5), 5200.0);
        assert_eq!(max_s_wave_spread_km(8.0), 6200.0);
        assert_eq!(max_s_wave_spread_km(8.5), 7500.0);
        assert!(max_s_wave_spread_km(3.0) < max_s_wave_spread_km(5.0));
        assert!(max_s_wave_spread_km(5.0) < max_s_wave_spread_km(7.0));
    }

    #[test]
    fn engine_falls_back_beyond_primary_max() {
        let primary = TravelTable {
            name: "jma".into(),
            depths_km: vec![0.0, 10.0],
            distances_km: vec![0.0, 1000.0, 2000.0],
            p_times_s: vec![vec![0.0, 100.0, 200.0], vec![1.0, 101.0, 201.0]],
            s_times_s: vec![vec![0.0, 180.0, 360.0], vec![2.0, 182.0, 362.0]],
        };
        let fallback = TravelTable {
            name: "ak".into(),
            depths_km: vec![0.0, 10.0],
            distances_km: vec![0.0, 2000.0, 5000.0],
            p_times_s: vec![vec![0.0, 200.0, 500.0], vec![1.0, 201.0, 501.0]],
            s_times_s: vec![vec![0.0, 360.0, 900.0], vec![2.0, 362.0, 902.0]],
        };
        let eng = TravelEngine {
            primary,
            fallback: Some(fallback),
            linear_last_resort: false,
        };
        let t = eng.travel_time(Wave::S, 5.0, 3000.0);
        assert!(t.is_finite() && t > 360.0 && t < 900.0);
    }
}
