//! 测站源：Jian 中转（kmoni / s-net / kma-station）+ NIED kmoni/lmoni + MSIL S-Net。
//!
//! Jian 测站三条 WS **无需鉴权**，连接后即可收帧；与 `/all`（需 key）相互独立。
//!
//! S-Net 语义：
//! - **资料**（`time` / `int[]`）约每分钟更新一次（MSIL validTime）
//! - **Jian 中转**另以约 1Hz steady_push 重推同一资料，只刷新信封 `timestamp` 对时；原版无此对时帧
//! - 僵死看门狗按「收包」计（秒级）；业务上送仅在资料指纹变化时

use starquake_config::StationsConfig;
use starquake_core::{instrumental_display_text, jma_from_instrumental, IntensityKind, StationSample};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::adapter::{
    ingest_kma_station_frame, ingest_kmoni_frame, ingest_snet_frame, KmoniState,
};
use crate::bus::{emit_health, NetEvent, NetTx};
use crate::sources::ws::{run_ws_loop, WsLoopOpts};

pub fn spawn(
    tx: NetTx,
    cfg: StationsConfig,
    assets_stations: Option<std::path::PathBuf>,
) -> Vec<JoinHandle<()>> {
    let mut tasks = Vec::new();

    if cfg.jma_enabled {
        match cfg.jma_source.trim().to_ascii_lowercase().as_str() {
            "kmoni" => {
                info!("stations: spawn original NIED kmoni");
                tasks.push(spawn_nied_image(
                    tx.clone(),
                    NiedKind::Kmoni,
                    assets_stations.clone(),
                ));
            }
            "lmoni" => {
                info!("stations: spawn original NIED lmoni");
                tasks.push(spawn_nied_image(
                    tx.clone(),
                    NiedKind::Lmoni,
                    assets_stations.clone(),
                ));
            }
            _ => {
                let url = cfg.jma_jian_ws.clone();
                info!(%url, "stations: spawn Jian /kmoni (no auth)");
                let tx_k = tx.clone();
                let resolve = jian_rewrite_resolve(url.clone());
                tasks.push(tokio::spawn(async move {
                    let mut state = KmoniState::default();
                    run_ws_loop(
                        tx_k,
                        WsLoopOpts {
                            source: "kmoni",
                            url,
                            resolve_url: Some(resolve),
                            on_open_texts: Vec::new(),
                            periodic_texts: Vec::new(),
                            periodic_secs: 0,
                            backoff_max_secs: 60,
                            // ~1s 帧
                            stale_secs: 15,
                        },
                        move |tx, text| ingest_kmoni_frame(&mut state, tx, text),
                    )
                    .await;
                }));
            }
        }
    }

    if cfg.snet_enabled {
        match cfg.snet_source.trim().to_ascii_lowercase().as_str() {
            "original" => {
                tasks.push(spawn_snet_original(
                    tx.clone(),
                    assets_stations.clone(),
                    cfg.snet_jian_ws.clone(),
                ));
            }
            _ => {
                tasks.push(spawn_snet_jian(tx.clone(), cfg.snet_jian_ws.clone()));
            }
        }
    }

    if cfg.kma_enabled {
        let url = cfg.kma_jian_ws.clone();
        info!(%url, "stations: spawn Jian /kma-station (no auth)");
        let tx_k = tx.clone();
        let resolve = jian_rewrite_resolve(url.clone());
        tasks.push(tokio::spawn(async move {
            let mut state = KmoniState::default();
            run_ws_loop(
                tx_k,
                WsLoopOpts {
                    source: "kma-station",
                    url,
                    resolve_url: Some(resolve),
                    on_open_texts: Vec::new(),
                    periodic_texts: Vec::new(),
                    periodic_secs: 0,
                    backoff_max_secs: 60,
                    stale_secs: 15,
                },
                move |tx, text| ingest_kma_station_frame(&mut state, tx, text),
            )
            .await;
        }));
    }

    tasks
}

fn jian_rewrite_resolve(base: String) -> crate::sources::ws::ResolveUrl {
    std::sync::Arc::new(move || Ok(crate::jian_endpoint::rewrite_api_url(&base)))
}

fn spawn_snet_jian(tx: NetTx, url: String) -> JoinHandle<()> {
    info!(%url, "stations: spawn Jian /s-net (no auth; 资料~1min，对时~1Hz)");
    let resolve = jian_rewrite_resolve(url.clone());
    tokio::spawn(async move {
        let mut state = KmoniState::default();
        run_ws_loop(
            tx,
            WsLoopOpts {
                source: "s-net",
                url,
                resolve_url: Some(resolve),
                on_open_texts: Vec::new(),
                periodic_texts: Vec::new(),
                periodic_secs: 0,
                backoff_max_secs: 60,
                // 中转每秒有对时帧，看门狗按收包；勿当成「资料每秒更新」放宽到 120
                stale_secs: 15,
            },
            move |tx, text| ingest_snet_frame(&mut state, tx, text),
        )
        .await;
    })
}

#[derive(Clone)]
struct SnetPoint {
    code: String,
    lat: f64,
    lon: f64,
    x: u32,
    y: u32,
}

/// 原版 MSIL S-Net：对齐 `Station/S-Net/SNet.py`（pixels.csv + z5 双切片纵向拼接取色）。
fn spawn_snet_original(
    tx: NetTx,
    assets_stations: Option<std::path::PathBuf>,
    fallback_jian_ws: String,
) -> JoinHandle<()> {
    let path = assets_stations
        .as_ref()
        .map(|d| d.join("pixels.csv"))
        .filter(|p| p.is_file());
    let Some(path) = path else {
        warn!("stations: 缺少 media/stations/pixels.csv，S-Net 原版回退 Jian 中转");
        return spawn_snet_jian(tx, fallback_jian_ws);
    };
    let points = match load_snet_pixels(&path) {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            warn!("stations: pixels.csv 为空，S-Net 原版回退 Jian 中转");
            return spawn_snet_jian(tx, fallback_jian_ws);
        }
        Err(e) => {
            warn!(error = %e, "stations: 读取 pixels.csv 失败，回退 Jian 中转");
            return spawn_snet_jian(tx, fallback_jian_ws);
        }
    };
    info!(n = points.len(), "stations: spawn original MSIL S-Net");
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .user_agent(concat!(
                "StarQuake/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/Jian11323/StarQuake)"
            ))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        // MSIL 在部分网络下会 403/超时；探测失败则回退 Jian /s-net
        if fetch_snet_latest_time(&client).await.is_none() {
            warn!("stations: MSIL S-Net 不可达，回退 Jian 中转");
            let mut state = KmoniState::default();
            run_ws_loop(
                tx,
                WsLoopOpts {
                    source: "s-net",
                    url: fallback_jian_ws,
                    resolve_url: None,
                    on_open_texts: Vec::new(),
                    periodic_texts: Vec::new(),
                    periodic_secs: 0,
                    backoff_max_secs: 60,
                    stale_secs: 15,
                },
                move |tx, text| ingest_snet_frame(&mut state, tx, text),
            )
            .await;
            return;
        }

        emit_health(&tx, "snet", starquake_core::HealthStatus::Normal);
        let mut last_key = String::new();
        let mut fail_streak = 0u32;
        loop {
            let latest = fetch_snet_latest_time(&client).await;
            if let Some(key) = latest {
                fail_streak = 0;
                if key != last_key {
                    match sample_snet_tiles(&client, &key, &points).await {
                        Some(list) => {
                            let _ = tx.send(NetEvent::Stations {
                                network: "snet",
                                list,
                            });
                            last_key = key;
                            emit_health(&tx, "snet", starquake_core::HealthStatus::Normal);
                        }
                        None => {
                            fail_streak = fail_streak.saturating_add(1);
                            emit_health(&tx, "snet", starquake_core::HealthStatus::Fluctuating);
                        }
                    }
                } else {
                    // 原版无秒级对时帧：validTime 未变也刷时钟，避免卡片 90s 误报断开
                    let _ = tx.send(NetEvent::StationClock {
                        network: "snet",
                        ts_ms: None,
                    });
                }
            } else {
                fail_streak = fail_streak.saturating_add(1);
                emit_health(&tx, "snet", starquake_core::HealthStatus::Fluctuating);
            }
            if fail_streak >= 3 {
                warn!("stations: MSIL S-Net 连续失败，回退 Jian 中转");
                let mut state = KmoniState::default();
                run_ws_loop(
                    tx,
                    WsLoopOpts {
                        source: "s-net",
                        url: fallback_jian_ws,
                        resolve_url: None,
                        on_open_texts: Vec::new(),
                        periodic_texts: Vec::new(),
                        periodic_secs: 0,
                        backoff_max_secs: 60,
                        stale_secs: 15,
                    },
                    move |tx, text| ingest_snet_frame(&mut state, tx, text),
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    })
}

fn load_snet_pixels(path: &std::path::Path) -> Result<Vec<SnetPoint>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if i == 0 && line.to_ascii_lowercase().starts_with("x,") {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 6 {
            continue;
        }
        let x: u32 = cols[0].trim().parse().unwrap_or(0);
        let y: u32 = cols[1].trim().parse().unwrap_or(0);
        let code = cols[3].trim().to_string();
        if code.is_empty() {
            continue;
        }
        let lon: f64 = cols[4].trim().parse().unwrap_or(0.0);
        let lat: f64 = cols[5].trim().parse().unwrap_or(0.0);
        out.push(SnetPoint {
            code,
            lat,
            lon,
            x,
            y,
        });
    }
    Ok(out)
}

async fn fetch_snet_latest_time(client: &reqwest::Client) -> Option<String> {
    let url = "https://www.msil.go.jp/data/tiles/smoni/targetTimes.json";
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    // 上游偶发 UTF-8 BOM
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let arr = v.as_array()?;
    let last = arr.last()?;
    let t = last.get("validtime")?.as_str()?;
    if t.len() >= 12 {
        Some(t.to_string())
    } else {
        None
    }
}

async fn sample_snet_tiles(
    client: &reqwest::Client,
    key: &str,
    points: &[SnetPoint],
) -> Option<Vec<StationSample>> {
    let base = "https://www.msil.go.jp/data/tiles/smoni/tileimage";
    let url1 = format!("{base}/{key}/{key}/5/28/11.png");
    let url2 = format!("{base}/{key}/{key}/5/28/12.png");
    let b1 = client.get(&url1).send().await.ok()?.bytes().await.ok()?;
    let b2 = client.get(&url2).send().await.ok()?.bytes().await.ok()?;
    let img1 = image::load_from_memory(&b1).ok()?.to_rgb8();
    let img2 = image::load_from_memory(&b2).ok()?.to_rgb8();
    let (w1, h1) = img1.dimensions();
    let (w2, h2) = img2.dimensions();
    let w = w1.max(w2);
    let h = h1 + h2;
    let mut merged = image::RgbImage::new(w, h);
    image::imageops::replace(&mut merged, &img1, 0, 0);
    image::imageops::replace(&mut merged, &img2, 0, h1 as i64);

    let mut out = Vec::new();
    for pt in points {
        let (r, g, b) = if pt.x < w && pt.y < h {
            let p = merged.get_pixel(pt.x, pt.y).0;
            (p[0], p[1], p[2])
        } else {
            (0, 0, 0)
        };
        let shindo = match color_to_position_snet(r, g, b) {
            Some(pos) => {
                let v = 10.0 * pos - 3.0;
                if v >= 7.0 {
                    99.0
                } else {
                    (v * 10.0).round() / 10.0
                }
            }
            None => -3.0,
        };
        if shindo < -3.0 {
            continue;
        }
        let instrumental = if shindo >= 99.0 { 7.0 } else { shindo };
        let (level, _) = jma_from_instrumental(instrumental);
        out.push(StationSample {
            id: pt.code.clone(),
            name: pt.code.clone(),
            latitude: pt.lat,
            longitude: pt.lon,
            intensity_text: if shindo >= 99.0 {
                "7".into()
            } else {
                instrumental_display_text(shindo)
            },
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: level,
            instrumental,
        });
    }
    Some(out)
}

/// 对齐 Station/S-Net `hsv_to_position_p`。
fn color_to_position_snet(r: u8, g: u8, b: u8) -> Option<f64> {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    if !(v > 0.1 && s > 0.75) {
        return None;
    }
    let p = if h > 0.1476 {
        280.31 * h.powi(6) - 916.05 * h.powi(5) + 1142.6 * h.powi(4) - 709.95 * h.powi(3)
            + 234.65 * h.powi(2)
            - 40.27 * h
            + 3.2217
    } else if h > 0.001 && h <= 0.1476 {
        151.4 * h.powi(4) - 49.32 * h.powi(3) + 6.753 * h.powi(2) - 2.481 * h + 0.9033
    } else {
        -0.005171 * v.powi(2) - 0.3282 * v + 1.221
    };
    let p = p.clamp(0.0, 1.0);
    if p <= 0.0 {
        None
    } else {
        Some(p)
    }
}

#[derive(Clone, Copy)]
enum NiedKind {
    Kmoni,
    Lmoni,
}

impl NiedKind {
    /// 計測震度图：对齐 Station/NIED `jma_s`（lmoni 主机无 jma_s，改走 kmoni）。
    fn shindo_base(self) -> &'static str {
        "http://www.kmoni.bosai.go.jp/data/map_img/RealTimeImg"
    }
    fn shindo_prefix(self) -> &'static str {
        "jma_s"
    }
}

#[derive(Clone)]
struct NiedPoint {
    code: String,
    name: String,
    lat: f64,
    lon: f64,
    x: u32,
    y: u32,
    stop: bool,
}

fn spawn_nied_image(
    tx: NetTx,
    kind: NiedKind,
    assets_stations: Option<std::path::PathBuf>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let points_path = assets_stations
            .as_ref()
            .map(|d| d.join("nied_point.csv"))
            .filter(|p| p.is_file());
        let Some(path) = points_path else {
            warn!("stations: 缺少 media/stations/nied_point.csv，原版 NIED 无法启动");
            emit_health(&tx, "kmoni", starquake_core::HealthStatus::Abnormal);
            return;
        };
        let points = match load_nied_points(&path) {
            Ok(p) if !p.is_empty() => p,
            Ok(_) => {
                warn!("stations: nied_point.csv 为空");
                return;
            }
            Err(e) => {
                warn!(error = %e, "stations: 读取 nied_point.csv 失败");
                return;
            }
        };
        info!(n = points.len(), kind = kind_label(kind), "stations: NIED points loaded");
        emit_health(&tx, "kmoni", starquake_core::HealthStatus::Normal);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let mut last_key = String::new();
        loop {
            let latest = fetch_latest_time(&client).await;
            if let Some(key) = latest {
                if key != last_key {
                    if let Some(list) = sample_nied_gif(&client, kind, &key, &points).await {
                        let _ = tx.send(NetEvent::Stations {
                            network: "kmoni",
                            list,
                        });
                        last_key = key;
                        emit_health(&tx, "kmoni", starquake_core::HealthStatus::Normal);
                    } else {
                        emit_health(&tx, "kmoni", starquake_core::HealthStatus::Fluctuating);
                    }
                }
            } else {
                emit_health(&tx, "kmoni", starquake_core::HealthStatus::Fluctuating);
            }
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        }
    })
}

fn kind_label(kind: NiedKind) -> &'static str {
    match kind {
        NiedKind::Kmoni => "kmoni",
        NiedKind::Lmoni => "lmoni",
    }
}

fn load_nied_points(path: &std::path::Path) -> Result<Vec<NiedPoint>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 && (line.contains("Type") || line.starts_with("Type")) {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 9 {
            continue;
        }
        // 对齐 Station/NIED：X 列为表头时跳过
        if cols[7].trim() == "X" {
            continue;
        }
        let x: u32 = cols[7].trim().parse().unwrap_or(0);
        let y: u32 = cols[8].trim().parse().unwrap_or(0);
        let lat: f64 = cols[5].trim().parse().unwrap_or(0.0);
        let lon: f64 = cols[6].trim().parse().unwrap_or(0.0);
        let code = cols[1].trim().to_string();
        if code.is_empty() {
            continue;
        }
        out.push(NiedPoint {
            code: code.clone(),
            name: cols[3].trim().to_string(),
            lat,
            lon,
            x,
            y,
            stop: cols[2].trim().eq_ignore_ascii_case("true"),
        });
    }
    Ok(out)
}

async fn fetch_latest_time(client: &reqwest::Client) -> Option<String> {
    let url = "http://www.kmoni.bosai.go.jp/webservice/server/pros/latest.json";
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    let t = v.get("latest_time")?.as_str()?;
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 14 {
        Some(digits[..14].to_string())
    } else {
        None
    }
}

async fn sample_nied_gif(
    client: &reqwest::Client,
    kind: NiedKind,
    key: &str,
    points: &[NiedPoint],
) -> Option<Vec<StationSample>> {
    let day = &key[..8];
    let pref = kind.shindo_prefix();
    let url = format!(
        "{}/{}/{}/{}.{}.gif",
        kind.shindo_base(),
        pref,
        day,
        key,
        pref
    );
    let resp = client.get(&url).send().await.ok()?;
    if resp.status().as_u16() == 404 {
        return None;
    }
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgb8();
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    for pt in points {
        if pt.stop {
            continue;
        }
        let (r, g, b) = if pt.x < w && pt.y < h {
            let p = img.get_pixel(pt.x, pt.y).0;
            (p[0], p[1], p[2])
        } else {
            (0, 0, 0)
        };
        // 对齐 Station/NIED：无感色 → int=-3.0 仍上图
        let shindo = match color_to_position(r, g, b) {
            Some(pos) => ((10.0 * pos - 3.0) * 100.0).round() / 100.0,
            None => -3.0,
        };
        if shindo < -3.0 {
            continue;
        }
        let (level, _) = jma_from_instrumental(shindo);
        out.push(StationSample {
            id: pt.code.clone(),
            name: if pt.name.is_empty() {
                pt.code.clone()
            } else {
                pt.name.clone()
            },
            latitude: pt.lat,
            longitude: pt.lon,
            intensity_text: instrumental_display_text(shindo),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: level,
            instrumental: shindo,
        });
    }
    Some(out)
}

/// 对齐 Station/NIED `color2position`（HSV → 色带位置 0–1）。
fn color_to_position(r: u8, g: u8, b: u8) -> Option<f64> {
    let (h, s, v) = rgb_to_hsv(r, g, b);
    if !(v > 0.05 && s > 0.75) {
        return None;
    }
    let p = if h > 0.1476 {
        280.31 * h.powi(6) - 916.05 * h.powi(5) + 1142.6 * h.powi(4) - 709.95 * h.powi(3)
            + 234.65 * h.powi(2)
            - 40.27 * h
            + 3.2217
    } else if h > 0.001 && h <= 0.1476 {
        151.4 * h.powi(4) - 49.32 * h.powi(3) + 6.753 * h.powi(2) - 2.481 * h + 0.9033
    } else {
        -0.005171 * v.powi(2) - 0.3282 * v + 1.2236
    };
    Some(p.clamp(0.0, 1.0))
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let r = r as f64 / 255.0;
    let g = g as f64 / 255.0;
    let b = b as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max <= 1e-12 { 0.0 } else { d / max };
    let h = if d <= 1e-12 {
        0.0
    } else if max == r {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h, s, v)
}
