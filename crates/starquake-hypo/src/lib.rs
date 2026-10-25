//! 测站到时震源推算（SREV 式误差水平）。
//!
//! **不含检知**：调用方须自行提供已触发站的坐标与检出时刻。
//!
//! 流程概要：
//! 1. 多轮从候选池选入一部分站；
//! 2. 相对当前假震源做 P/S 识别；
//! 3. 网格爬山最小化加权发震时刻方差（着未着可选）；
//! 4. 按到时残差做 IQR / 最差比例剔除，再进入下一轮。

use serde::{Deserialize, Serialize};
use starquake_travel::{TravelEngine, Wave};
use starquake_util::haversine_km;

/// 已触发测站（检知结果由外部给出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrivalPick {
    /// 可选站码，仅用于诊断回传。
    #[serde(default)]
    pub id: String,
    pub latitude: f64,
    pub longitude: f64,
    /// 检出时刻（秒，任意统一时基）。
    pub detect_sec: f64,
}

/// 未触发站，仅用于着未着惩罚（可选）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuietStation {
    #[serde(default)]
    pub id: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Phase {
    P,
    S,
}

impl From<Phase> for Wave {
    fn from(p: Phase) -> Self {
        match p {
            Phase::P => Wave::P,
            Phase::S => Wave::S,
        }
    }
}

/// 推算参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateConfig {
    /// 最少触发站数。
    pub min_picks: usize,
    /// 假震源初始深度（km）。
    pub seed_depth_km: f64,
    /// 深度搜索上下限。
    pub depth_min_km: f64,
    pub depth_max_km: f64,
    /// 每轮最多选入站数；`0` 表示本轮用尽候选池。
    pub select_per_round: usize,
    /// 多轮上限。
    pub max_rounds: usize,
    /// 每轮按 |残差| 剔除比例（0~1），与 IQR 取并。
    pub cull_worst_fraction: f64,
    /// IQR 围栏系数（典型 1.5）；`<=0` 关闭 IQR。
    pub iqr_k: f64,
    /// 启用 IQR 所需最少站数。
    pub iqr_min_picks: usize,
    /// 着未着：早期秒数内启用。
    pub nyad_early_sec: f64,
    /// 着未着：触发站少于此数也启用。
    pub nyad_max_picks: usize,
    /// 着未着搜索半径 = 最大触发震中距 + 此值（km）。
    pub nyad_margin_km: f64,
    /// 着未着惩罚强度（加到均方误差上）。
    pub nyad_weight: f64,
    /// 浅源先验强度（对 (depth - seed)^2）。
    pub depth_prior: f64,
    /// 误差水平上限；超过则本轮解无效。
    pub max_error_level: f64,
    /// 爬山各阶段：`(d_lat_lon_deg, d_depth_km)`；`d_depth_km=0` 表示不搜深度。
    pub climb_steps: Vec<(f64, f64)>,
    /// 每阶段爬山最大步数。
    pub climb_guard: usize,
}

impl Default for LocateConfig {
    fn default() -> Self {
        Self {
            min_picks: 4,
            seed_depth_km: 10.0,
            depth_min_km: 5.0,
            depth_max_km: 100.0,
            select_per_round: 24,
            max_rounds: 4,
            cull_worst_fraction: 0.2,
            iqr_k: 1.5,
            iqr_min_picks: 6,
            nyad_early_sec: 3.0,
            nyad_max_picks: 30,
            nyad_margin_km: 30.0,
            nyad_weight: 2.5,
            depth_prior: 0.002,
            max_error_level: 12.0,
            climb_steps: vec![(0.5, 0.0), (0.1, 0.0), (0.1, 50.0), (0.1, 10.0)],
            climb_guard: 40,
        }
    }
}

/// 单站在终解上的诊断。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationFit {
    pub pick_index: usize,
    pub id: String,
    pub phase: Phase,
    pub residual_sec: f64,
    pub weight: f64,
    pub used: bool,
    pub rejected: bool,
}

/// 推算结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypocenterEstimate {
    pub latitude: f64,
    pub longitude: f64,
    pub depth_km: f64,
    pub origin_sec: f64,
    /// 加权均方发震时刻残差（秒²量级，含着未着/深度先验）。
    pub error_level: f64,
    pub rounds_run: usize,
    pub used_pick_indices: Vec<usize>,
    pub rejected_pick_indices: Vec<usize>,
    pub station_fits: Vec<StationFit>,
}

#[derive(Clone, Copy)]
struct Hypo {
    lat: f64,
    lon: f64,
    depth: f64,
    origin: f64,
}

/// 对已触发站做震源推算。`now_sec` 用于着未着（通常取当前墙钟同时间基）。
pub fn locate(
    travel: &TravelEngine,
    picks: &[ArrivalPick],
    quiet: &[QuietStation],
    now_sec: f64,
    cfg: &LocateConfig,
) -> Option<HypocenterEstimate> {
    if picks.len() < cfg.min_picks {
        return None;
    }

    let mut order: Vec<usize> = (0..picks.len()).collect();
    order.sort_by(|&a, &b| {
        picks[a]
            .detect_sec
            .partial_cmp(&picks[b].detect_sec)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut pool = order.clone();
    let mut rejected: Vec<usize> = Vec::new();
    type LocateBest = (Hypo, f64, Vec<Phase>, Vec<usize>, usize);
    let mut best: Option<LocateBest> = None;

    for round in 0..cfg.max_rounds {
        if pool.len() < cfg.min_picks {
            break;
        }
        let selected = select_subset(&pool, picks, cfg.select_per_round);
        if selected.len() < cfg.min_picks {
            break;
        }

        let Some((hypo0, err0, phases0)) =
            locate_once(travel, picks, quiet, now_sec, &selected, cfg)
        else {
            // 本轮选站失败：仍尝试剔最差时间离群后继续
            if let Some(culled) = cull_by_detect_spread(picks, &selected, cfg) {
                remove_from_pool(&mut pool, &culled);
                rejected.extend(culled);
                continue;
            }
            break;
        };

        // 剔站用纯 P 残差，避免误标 S 把野值残差抹平
        let residuals = station_residuals_p_only(travel, picks, &selected, hypo0);
        let culled = cull_outliers(&residuals, &selected, cfg);
        let culled_set: std::collections::HashSet<usize> = culled.iter().copied().collect();
        let mut kept: Vec<usize> = selected
            .iter()
            .copied()
            .filter(|i| !culled_set.contains(i))
            .collect();
        if kept.len() < cfg.min_picks {
            kept = selected.clone();
        }

        // 剔除后再算一解，避免离群站拉偏终解
        let (hypo, err, phases, used) = if kept.len() != selected.len() {
            if let Some((h, e, ph)) = locate_once(travel, picks, quiet, now_sec, &kept, cfg) {
                (h, e, ph, kept)
            } else {
                (hypo0, err0, phases0, selected.clone())
            }
        } else {
            (hypo0, err0, phases0, selected.clone())
        };

        remove_from_pool(&mut pool, &culled);
        rejected.extend(culled.iter().copied());

        let improve = best
            .as_ref()
            .map(|(_, e, _, _, _)| err < *e - 1e-9)
            .unwrap_or(true);
        if improve {
            best = Some((hypo, err, phases, used, round + 1));
        }

        if culled.is_empty() {
            break;
        }
    }

    let (hypo, err, _phases, used, rounds_run) = best?;
    if err.partial_cmp(&cfg.max_error_level) != Some(std::cmp::Ordering::Less) {
        return None;
    }

    // 终解上对全部输入站刷一次相位与残差（诊断用）
    let all_idx: Vec<usize> = (0..picks.len()).collect();
    let all_phases = classify_phases(travel, picks, &all_idx, hypo);
    let all_res = station_residuals(travel, picks, &all_idx, &all_phases, hypo);
    let rejected_set: std::collections::HashSet<usize> = rejected.iter().copied().collect();
    let used_set: std::collections::HashSet<usize> = used.iter().copied().collect();

    let mut station_fits = Vec::with_capacity(picks.len());
    for i in 0..picks.len() {
        let d = haversine_km(hypo.lon, hypo.lat, picks[i].longitude, picks[i].latitude);
        let w = distance_weight(d, first_distance(picks, &used, hypo));
        station_fits.push(StationFit {
            pick_index: i,
            id: picks[i].id.clone(),
            phase: all_phases[i],
            residual_sec: all_res[i],
            weight: w,
            used: used_set.contains(&i),
            rejected: rejected_set.contains(&i),
        });
    }

    let mut rejected_pick_indices = rejected;
    rejected_pick_indices.sort_unstable();
    rejected_pick_indices.dedup();

    Some(HypocenterEstimate {
        latitude: hypo.lat,
        longitude: hypo.lon,
        depth_km: (hypo.depth / 5.0).round() * 5.0,
        origin_sec: hypo.origin,
        error_level: err,
        rounds_run,
        used_pick_indices: used,
        rejected_pick_indices,
        station_fits,
    })
}

fn locate_once(
    travel: &TravelEngine,
    picks: &[ArrivalPick],
    quiet: &[QuietStation],
    now_sec: f64,
    selected: &[usize],
    cfg: &LocateConfig,
) -> Option<(Hypo, f64, Vec<Phase>)> {
    let first = *selected
        .iter()
        .min_by(|&&a, &&b| {
            picks[a]
                .detect_sec
                .partial_cmp(&picks[b].detect_sec)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
    let seed_lat = (picks[first].latitude * 100.0).round() / 100.0;
    let seed_lon = (picks[first].longitude * 100.0).round() / 100.0;
    let mut hypo = Hypo {
        lat: seed_lat,
        lon: seed_lon,
        depth: cfg.seed_depth_km,
        origin: picks[first].detect_sec - 2.0,
    };

    // 主搜索强制全 P，避免晚到野值被误标成 S 后把震源拉飞
    let mut phases = vec![Phase::P; picks.len()];
    refine_origin(travel, picks, selected, &phases, &mut hypo);

    for &(d_ll, d_dep) in &cfg.climb_steps {
        let mut moved = true;
        let mut guard = 0;
        while moved && guard < cfg.climb_guard {
            moved = false;
            guard += 1;
            let mut cands = vec![
                (hypo.lat, hypo.lon, hypo.depth),
                (hypo.lat + d_ll, hypo.lon, hypo.depth),
                (hypo.lat - d_ll, hypo.lon, hypo.depth),
                (hypo.lat, hypo.lon + d_ll, hypo.depth),
                (hypo.lat, hypo.lon - d_ll, hypo.depth),
            ];
            if d_dep > 0.0 {
                cands.push((hypo.lat, hypo.lon, hypo.depth + d_dep));
                cands.push((hypo.lat, hypo.lon, hypo.depth - d_dep));
            }
            let mut local_best =
                error_level(ErrorLevelCtx { travel, picks, quiet, now_sec, selected, phases: &phases, hypo, cfg });
            for (ci, &(la, lo, dep)) in cands.iter().enumerate() {
                if dep < cfg.depth_min_km || dep > cfg.depth_max_km {
                    continue;
                }
                if !(la.is_finite() && lo.is_finite()) {
                    continue;
                }
                let mut trial = Hypo {
                    lat: la,
                    lon: lo,
                    depth: dep,
                    origin: hypo.origin,
                };
                refine_origin(travel, picks, selected, &phases, &mut trial);
                let e = error_level(ErrorLevelCtx { travel, picks, quiet, now_sec, selected, phases: &phases, hypo: trial, cfg });
                if e < local_best {
                    local_best = e;
                    hypo = trial;
                    moved = ci != 0;
                }
            }
        }
        refine_origin(travel, picks, selected, &phases, &mut hypo);
    }

    // 爬山结束后再做 PS 识别，并微调发震时刻
    classify_phases_into(travel, picks, selected, hypo, &mut phases);
    refine_origin(travel, picks, selected, &phases, &mut hypo);

    let err = error_level(ErrorLevelCtx { travel, picks, quiet, now_sec, selected, phases: &phases, hypo, cfg });
    if !err.is_finite() {
        return None;
    }
    Some((hypo, err, phases))
}

fn select_subset(pool: &[usize], picks: &[ArrivalPick], cap: usize) -> Vec<usize> {
    let mut sorted = pool.to_vec();
    sorted.sort_by(|&a, &b| {
        picks[a]
            .detect_sec
            .partial_cmp(&picks[b].detect_sec)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if cap == 0 || sorted.len() <= cap {
        return sorted;
    }
    // 早到优先，再按方位扇区补点，避免全挤在首波一侧
    let mut out: Vec<usize> = Vec::with_capacity(cap);
    let first = sorted[0];
    out.push(first);
    let mut bins: [Vec<usize>; 8] = Default::default();
    for &i in sorted.iter().skip(1) {
        let az = azimuth_deg(
            picks[first].latitude,
            picks[first].longitude,
            picks[i].latitude,
            picks[i].longitude,
        );
        let b = ((az / 45.0).floor() as usize) % 8;
        bins[b].push(i);
    }
    // 轮转扇区取站
    let mut progress = true;
    while out.len() < cap && progress {
        progress = false;
        for bin in &mut bins {
            if out.len() >= cap {
                break;
            }
            if let Some(i) = bin.pop() {
                out.push(i);
                progress = true;
            }
        }
    }
    // 仍不足则按时间补
    for &i in &sorted {
        if out.len() >= cap {
            break;
        }
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

fn azimuth_deg(lat0: f64, lon0: f64, lat1: f64, lon1: f64) -> f64 {
    let lat0r = lat0.to_radians();
    let lat1r = lat1.to_radians();
    let d_lon = (lon1 - lon0).to_radians();
    let y = d_lon.sin() * lat1r.cos();
    let x = lat0r.cos() * lat1r.sin() - lat0r.sin() * lat1r.cos() * d_lon.cos();
    let mut az = y.atan2(x).to_degrees();
    if az < 0.0 {
        az += 360.0;
    }
    az
}

fn classify_phases(
    travel: &TravelEngine,
    picks: &[ArrivalPick],
    selected: &[usize],
    hypo: Hypo,
) -> Vec<Phase> {
    let mut phases = vec![Phase::P; picks.len()];
    classify_phases_into(travel, picks, selected, hypo, &mut phases);
    phases
}

fn classify_phases_into(
    travel: &TravelEngine,
    picks: &[ArrivalPick],
    selected: &[usize],
    hypo: Hypo,
    phases: &mut [Phase],
) {
    for &i in selected {
        let d = haversine_km(hypo.lon, hypo.lat, picks[i].longitude, picks[i].latitude);
        let tp = travel.travel_time(Wave::P, hypo.depth, d);
        let ts = travel.travel_time(Wave::S, hypo.depth, d);
        let rp = (picks[i].detect_sec - (hypo.origin + tp)).abs();
        let rs = (picks[i].detect_sec - (hypo.origin + ts)).abs();
        // S 须明显更好，且自身残差可接受；否则保持 P，交给 IQR 剔野值
        let accept_s = rs + 0.5 < rp && rs < 5.0_f64.max(0.25 * ts);
        phases[i] = if accept_s { Phase::S } else { Phase::P };
    }
}

fn refine_origin(
    travel: &TravelEngine,
    picks: &[ArrivalPick],
    selected: &[usize],
    phases: &[Phase],
    hypo: &mut Hypo,
) {
    let Some(&first) = selected.first() else {
        return;
    };
    let d0_raw = haversine_km(
        hypo.lon,
        hypo.lat,
        picks[first].longitude,
        picks[first].latitude,
    );
    let d0 = d0_raw.max(50.0);
    let mut sum = 0.0;
    let mut w_sum = 0.0;
    for &i in selected {
        let d = haversine_km(hypo.lon, hypo.lat, picks[i].longitude, picks[i].latitude);
        let tt = travel.travel_time(phases[i].into(), hypo.depth, d);
        let ori = picks[i].detect_sec - tt;
        let w = distance_weight(d, d0);
        sum += ori * w;
        w_sum += w;
    }
    if w_sum > 0.0 {
        hypo.origin = sum / w_sum;
    }
}

fn distance_weight(dist_km: f64, d0: f64) -> f64 {
    if dist_km <= 50.0 {
        1.0
    } else {
        (d0 / dist_km).clamp(0.05, 1.0)
    }
}

fn first_distance(picks: &[ArrivalPick], selected: &[usize], hypo: Hypo) -> f64 {
    let Some(&first) = selected
        .iter()
        .min_by(|&&a, &&b| {
            picks[a]
                .detect_sec
                .partial_cmp(&picks[b].detect_sec)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    else {
        return 50.0;
    };
    haversine_km(
        hypo.lon,
        hypo.lat,
        picks[first].longitude,
        picks[first].latitude,
    )
    .max(50.0)
}

struct ErrorLevelCtx<'a> {
    travel: &'a TravelEngine,
    picks: &'a [ArrivalPick],
    quiet: &'a [QuietStation],
    now_sec: f64,
    selected: &'a [usize],
    phases: &'a [Phase],
    hypo: Hypo,
    cfg: &'a LocateConfig,
}

fn error_level(ctx: ErrorLevelCtx<'_>) -> f64 {
    let ErrorLevelCtx {
        travel,
        picks,
        quiet,
        now_sec,
        selected,
        phases,
        hypo,
        cfg,
    } = ctx;
    let d0 = first_distance(picks, selected, hypo);
    let mut origins = Vec::with_capacity(selected.len());
    let mut weights = Vec::with_capacity(selected.len());
    let mut mean = 0.0;
    let mut w_sum = 0.0;
    for &i in selected {
        let d = haversine_km(hypo.lon, hypo.lat, picks[i].longitude, picks[i].latitude);
        let tt = travel.travel_time(phases[i].into(), hypo.depth, d);
        let ori = picks[i].detect_sec - tt;
        let w = distance_weight(d, d0);
        origins.push(ori);
        weights.push(w);
        mean += ori * w;
        w_sum += w;
    }
    if w_sum <= 0.0 {
        return f64::INFINITY;
    }
    mean /= w_sum;
    let mut err = 0.0;
    for (ori, w) in origins.iter().zip(weights.iter()) {
        let diff = *ori - mean;
        err += diff * diff * *w;
    }
    err /= w_sum;

    // 着未着
    let first_detect = selected
        .iter()
        .map(|&i| picks[i].detect_sec)
        .fold(f64::INFINITY, f64::min);
    let early = (now_sec - first_detect) <= cfg.nyad_early_sec;
    let sparse = selected.len() < cfg.nyad_max_picks;
    if !quiet.is_empty() && cfg.nyad_weight > 0.0 && (early || sparse) {
        let mut max_d = 0.0;
        for &i in selected {
            let d = haversine_km(hypo.lon, hypo.lat, picks[i].longitude, picks[i].latitude);
            if d > max_d {
                max_d = d;
            }
        }
        let lim = max_d + cfg.nyad_margin_km;
        let mut nyad = 0usize;
        let mut nyad_n = 0usize;
        for q in quiet {
            let d = haversine_km(hypo.lon, hypo.lat, q.longitude, q.latitude);
            if d > lim {
                continue;
            }
            nyad_n += 1;
            let tt = travel.travel_time(Wave::P, hypo.depth, d);
            if now_sec > hypo.origin + tt {
                nyad += 1;
            }
        }
        if nyad_n > 0 {
            err += (nyad as f64 / nyad_n as f64) * cfg.nyad_weight;
        }
    }

    let d_prior = hypo.depth - cfg.seed_depth_km;
    err += cfg.depth_prior * d_prior * d_prior;
    err
}

fn station_residuals(
    travel: &TravelEngine,
    picks: &[ArrivalPick],
    selected: &[usize],
    phases: &[Phase],
    hypo: Hypo,
) -> Vec<f64> {
    let mut out = vec![0.0; picks.len()];
    for &i in selected {
        let d = haversine_km(hypo.lon, hypo.lat, picks[i].longitude, picks[i].latitude);
        let tt = travel.travel_time(phases[i].into(), hypo.depth, d);
        out[i] = picks[i].detect_sec - (hypo.origin + tt);
    }
    out
}

fn station_residuals_p_only(
    travel: &TravelEngine,
    picks: &[ArrivalPick],
    selected: &[usize],
    hypo: Hypo,
) -> Vec<f64> {
    let mut out = vec![0.0; picks.len()];
    for &i in selected {
        let d = haversine_km(hypo.lon, hypo.lat, picks[i].longitude, picks[i].latitude);
        let tt = travel.travel_time(Wave::P, hypo.depth, d);
        out[i] = picks[i].detect_sec - (hypo.origin + tt);
    }
    out
}

fn cull_outliers(residuals: &[f64], selected: &[usize], cfg: &LocateConfig) -> Vec<usize> {
    if selected.len() <= cfg.min_picks {
        return Vec::new();
    }
    let mut vals: Vec<(usize, f64)> = selected
        .iter()
        .map(|&i| (i, residuals[i].abs()))
        .collect();
    vals.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut marked = std::collections::HashSet::new();
    let xs: Vec<f64> = vals.iter().map(|(_, r)| *r).collect();
    let med = percentile_sorted(&xs, 0.5).max(1e-3);

    // 相对中位残差的硬离群
    let hard = (3.0 * med).max(3.0);
    for &(i, r) in &vals {
        if r > hard {
            marked.insert(i);
        }
    }

    // IQR 围栏（对 |残差|）
    if cfg.iqr_k > 0.0 && selected.len() >= cfg.iqr_min_picks {
        let q1 = percentile_sorted(&xs, 0.25);
        let q3 = percentile_sorted(&xs, 0.75);
        let iqr = (q3 - q1).max(0.0);
        // 残差几乎全相等时 IQR≈0，改用硬阈，避免误伤
        let hi = if iqr < 1e-6 { hard } else { q3 + cfg.iqr_k * iqr };
        for &(i, r) in &vals {
            if r > hi {
                marked.insert(i);
            }
        }
    }

    // 最差比例：按 |残差| 从大到小
    let frac = cfg.cull_worst_fraction.clamp(0.0, 0.9);
    if frac > 0.0 {
        let n_cull = ((selected.len() as f64) * frac).ceil() as usize;
        let n_cull = n_cull.min(selected.len().saturating_sub(cfg.min_picks));
        for &(i, _) in vals.iter().rev().take(n_cull) {
            marked.insert(i);
        }
    }

    // 至少保留 min_picks：优先丢掉残差更大的
    let mut culled: Vec<usize> = marked.into_iter().collect();
    culled.sort_by(|&a, &b| {
        residuals[b]
            .abs()
            .partial_cmp(&residuals[a].abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    while selected.len() - culled.len() < cfg.min_picks && !culled.is_empty() {
        culled.pop();
    }
    culled.sort_unstable();
    culled
}

/// 选站失败时的兜底：按检出时刻离群粗剔。
fn cull_by_detect_spread(
    picks: &[ArrivalPick],
    selected: &[usize],
    cfg: &LocateConfig,
) -> Option<Vec<usize>> {
    if selected.len() <= cfg.min_picks {
        return None;
    }
    let mut times: Vec<(usize, f64)> = selected
        .iter()
        .map(|&i| (i, picks[i].detect_sec))
        .collect();
    times.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let xs: Vec<f64> = times.iter().map(|(_, t)| *t).collect();
    let med = percentile_sorted(&xs, 0.5);
    let mut scored: Vec<(usize, f64)> = times
        .iter()
        .map(|&(i, t)| (i, (t - med).abs()))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let n = ((selected.len() as f64) * cfg.cull_worst_fraction.clamp(0.1, 0.5)).ceil() as usize;
    let n = n.max(1).min(selected.len() - cfg.min_picks);
    Some(scored.into_iter().take(n).map(|(i, _)| i).collect())
}

fn percentile_sorted(sorted_asc: &[f64], p: f64) -> f64 {
    if sorted_asc.is_empty() {
        return 0.0;
    }
    if sorted_asc.len() == 1 {
        return sorted_asc[0];
    }
    let p = p.clamp(0.0, 1.0);
    let x = p * (sorted_asc.len() - 1) as f64;
    let i = x.floor() as usize;
    let j = (i + 1).min(sorted_asc.len() - 1);
    let t = x - i as f64;
    sorted_asc[i] * (1.0 - t) + sorted_asc[j] * t
}

fn remove_from_pool(pool: &mut Vec<usize>, remove: &[usize]) {
    if remove.is_empty() {
        return;
    }
    let set: std::collections::HashSet<usize> = remove.iter().copied().collect();
    pool.retain(|i| !set.contains(i));
}

#[cfg(test)]
mod tests {
    use super::*;
    use starquake_travel::TravelTable;
    use std::path::PathBuf;

    fn stub_engine() -> TravelEngine {
        // 近似常速：P 7 / S 5，斜距
        let depths = vec![0.0, 10.0, 50.0, 100.0];
        let dists: Vec<f64> = (0..=40).map(|i| i as f64 * 50.0).collect();
        let mut p = Vec::new();
        let mut s = Vec::new();
        for &dep in &depths {
            let mut prow = Vec::new();
            let mut srow = Vec::new();
            for &d in &dists {
                let slant = (dep * dep + d * d).sqrt();
                prow.push(slant / 7.0);
                srow.push(slant / 5.0);
            }
            p.push(prow);
            s.push(srow);
        }
        TravelEngine {
            primary: TravelTable {
                name: "stub".into(),
                depths_km: depths,
                distances_km: dists,
                p_times_s: p,
                s_times_s: s,
            },
            fallback: None,
            linear_last_resort: true,
        }
    }

    fn synth_picks(lat: f64, lon: f64, depth: f64, origin: f64, travel: &TravelEngine) -> Vec<ArrivalPick> {
        // 环绕假震源的若干站，理论 P 到时
        let offsets = [
            (0.3, 0.0),
            (-0.25, 0.15),
            (0.1, -0.35),
            (0.2, 0.25),
            (-0.3, -0.2),
            (0.05, 0.4),
            (-0.4, 0.05),
            (0.35, -0.15),
        ];
        offsets
            .iter()
            .enumerate()
            .map(|(i, &(dlat, dlon))| {
                let slat = lat + dlat;
                let slon = lon + dlon;
                let d = haversine_km(lon, lat, slon, slat);
                let tt = travel.travel_time(Wave::P, depth, d);
                ArrivalPick {
                    id: format!("S{i}"),
                    latitude: slat,
                    longitude: slon,
                    detect_sec: origin + tt,
                }
            })
            .collect()
    }

    #[test]
    fn locates_synthetic_cluster_near_truth() {
        let eng = stub_engine();
        let truth_lat = 35.5;
        let truth_lon = 139.5;
        let truth_dep = 10.0;
        let origin = 1000.0;
        let picks = synth_picks(truth_lat, truth_lon, truth_dep, origin, &eng);
        let now = picks.iter().map(|p| p.detect_sec).fold(0.0, f64::max) + 1.0;
        let est = locate(&eng, &picks, &[], now, &LocateConfig::default())
            .expect("should locate");
        let err_km = haversine_km(est.longitude, est.latitude, truth_lon, truth_lat);
        assert!(
            err_km < 40.0,
            "epicenter error {err_km} km, got ({}, {})",
            est.latitude,
            est.longitude
        );
        assert!(est.error_level < 5.0, "error_level {}", est.error_level);
        assert!(est.rounds_run >= 1);
    }

    #[test]
    fn iqr_rejects_outlier_pick() {
        let eng = stub_engine();
        let mut picks = synth_picks(36.0, 140.0, 10.0, 0.0, &eng);
        // 故意污染一站：检出时刻大幅偏移（远大于 P/S 时差）
        picks[3].detect_sec += 40.0;
        let now = picks.iter().map(|p| p.detect_sec).fold(0.0, f64::max) + 1.0;
        let cfg = LocateConfig {
            min_picks: 4,
            iqr_min_picks: 6,
            cull_worst_fraction: 0.125,
            max_rounds: 2,
            select_per_round: 0,
            ..Default::default()
        };
        let est = locate(&eng, &picks, &[], now, &cfg).expect("locate");
        assert!(
            est.rejected_pick_indices.contains(&3),
            "outlier station 3 should be rejected, rejected={:?}, fits={:?}",
            est.rejected_pick_indices,
            est.station_fits
                .iter()
                .map(|f| (f.pick_index, f.residual_sec, f.rejected))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn classifies_late_arrival_as_s() {
        let eng = stub_engine();
        let lat = 35.0;
        let lon = 139.0;
        let depth = 10.0;
        let origin = 0.0;
        let d = 50.0;
        // 构造一站：到时贴近 S
        let tt_s = eng.travel_time(Wave::S, depth, d);
        // 反推站位：正北约 d km ≈ 0.45°
        let slat = lat + d / 111.0;
        let pick = ArrivalPick {
            id: "late".into(),
            latitude: slat,
            longitude: lon,
            detect_sec: origin + tt_s,
        };
        let hypo = Hypo {
            lat,
            lon,
            depth,
            origin,
        };
        let mut phases = vec![Phase::P];
        classify_phases_into(&eng, &[pick], &[0], hypo, &mut phases);
        assert_eq!(phases[0], Phase::S);
    }

    #[test]
    fn refuses_too_few_picks() {
        let eng = stub_engine();
        let picks = vec![ArrivalPick {
            id: "a".into(),
            latitude: 35.0,
            longitude: 139.0,
            detect_sec: 1.0,
        }];
        assert!(locate(&eng, &picks, &[], 2.0, &LocateConfig::default()).is_none());
    }

    #[test]
    fn loads_real_jma2001_if_present() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop();
        path.pop();
        path.push("media/travel/jma2001.json");
        if !path.is_file() {
            return;
        }
        let eng = TravelEngine::load_named(path.parent().unwrap(), "jma2001", "ak135", true)
            .expect("load jma2001");
        let picks = synth_picks(37.5, 141.0, 20.0, 500.0, &eng);
        let now = picks.iter().map(|p| p.detect_sec).fold(0.0, f64::max) + 2.0;
        let est = locate(&eng, &picks, &[], now, &LocateConfig::default());
        assert!(est.is_some());
    }
}
