//! 地图气象叠加：中国/日本/台湾雷达 PNG + 中国降水 24/48/72（GET 轮询）。

use starquake_config::MapWxSourceConfig;
use starquake_core::{
    HealthStatus, PrecipFrame, PrecipPoly, RadarFrame, RadarKind,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::bus::{emit_health, NetEvent, NetTx};

const UA: &str = concat!(
    "StarQuake/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Jian11323/StarQuake)"
);

pub fn spawn(tx: NetTx, cfg: MapWxSourceConfig) -> Vec<JoinHandle<()>> {
    let precip_h = normalize_precip_hours(cfg.precip_hours);
    if !cfg.enabled || (!cfg.any_radar() && precip_h == 0) {
        return Vec::new();
    }
    let poll = cfg.poll_secs.max(30);
    info!(
        radar_cn = cfg.radar_china,
        radar_jp = cfg.radar_japan,
        radar_tw = cfg.radar_taiwan,
        precip_h,
        poll_secs = poll,
        "map_wx: spawn http poll"
    );

    vec![tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(60))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!("map_wx client: {e}");
                emit_health(&tx, "map_wx", HealthStatus::Abnormal);
                return;
            }
        };

        // 错开与台风等同 IP GET 限流（缩短首包等待）
        sleep(Duration::from_millis(600)).await;

        loop {
            emit_health(&tx, "map_wx", HealthStatus::Fluctuating);
            let mut ok_any = false;
            let mut rate_limited = false;

            // 已开启的雷达并行拉取，避免串行 + 间隔导致首帧过慢
            let mut radar_jobs = Vec::new();
            for kind in RadarKind::ALL {
                let on = match kind {
                    RadarKind::China => cfg.radar_china,
                    RadarKind::Japan => cfg.radar_japan,
                    RadarKind::Taiwan => cfg.radar_taiwan,
                };
                if !on {
                    let _ = tx.send(NetEvent::Radar { kind, frame: None });
                    continue;
                }
                let url = crate::jian_endpoint::rewrite_api_url(&match kind {
                    RadarKind::China => cfg.radar_china_url.clone(),
                    RadarKind::Japan => cfg.radar_japan_url.clone(),
                    RadarKind::Taiwan => cfg.radar_taiwan_url.clone(),
                });
                let client = client.clone();
                radar_jobs.push(async move { (kind, fetch_radar(&client, &url, kind).await) });
            }
            let radar_results = futures_util::future::join_all(radar_jobs).await;
            for (kind, result) in radar_results {
                match result {
                    Ok(frame) => {
                        let _ = tx.send(NetEvent::Radar {
                            kind,
                            frame: Some(frame),
                        });
                        ok_any = true;
                    }
                    Err(FetchErr::RateLimited) => {
                        rate_limited = true;
                        warn!(?kind, "map_wx radar 429");
                    }
                    Err(FetchErr::Other(e)) => warn!(?kind, "map_wx radar: {e}"),
                }
            }

            if precip_h > 0 {
                let precip_base = crate::jian_endpoint::rewrite_api_url(&cfg.precip_url);
                let url = format!(
                    "{base}{sep}period={h}&_={ts}",
                    base = precip_base,
                    sep = if precip_base.contains('?') {
                        '&'
                    } else {
                        '?'
                    },
                    h = precip_h,
                    ts = chrono::Utc::now().timestamp_millis()
                );
                match fetch_precip(&client, &url, precip_h).await {
                    Ok(frame) => {
                        let _ = tx.send(NetEvent::Precip(Some(frame)));
                        ok_any = true;
                    }
                    Err(FetchErr::RateLimited) => {
                        rate_limited = true;
                        warn!("map_wx precip 429");
                    }
                    Err(FetchErr::Other(e)) => warn!("map_wx precip: {e}"),
                }
            } else {
                let _ = tx.send(NetEvent::Precip(None));
            }

            emit_health(
                &tx,
                "map_wx",
                if ok_any {
                    HealthStatus::Normal
                } else {
                    HealthStatus::Abnormal
                },
            );
            let wait = if rate_limited { poll.max(5) } else { poll };
            sleep(Duration::from_secs(wait)).await;
        }
    })]
}

fn normalize_precip_hours(h: u16) -> u16 {
    match h {
        24 | 48 | 72 => h,
        _ => 0,
    }
}

enum FetchErr {
    RateLimited,
    Other(String),
}

async fn fetch_radar(
    client: &reqwest::Client,
    url: &str,
    kind: RadarKind,
) -> Result<RadarFrame, FetchErr> {
    let fetch_url = format!(
        "{url}{sep}_={ts}",
        sep = if url.contains('?') { '&' } else { '?' },
        ts = chrono::Utc::now().timestamp_millis()
    );
    let resp = client
        .get(&fetch_url)
        .send()
        .await
        .map_err(|e| FetchErr::Other(e.to_string()))?;
    if resp.status().as_u16() == 429 {
        return Err(FetchErr::RateLimited);
    }
    if !resp.status().is_success() {
        return Err(FetchErr::Other(format!("HTTP {}", resp.status())));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| FetchErr::Other(e.to_string()))?;
    // PNG 解码放到阻塞池，避免拖垮 tokio 调度
    let kind_copy = kind;
    let parsed = tokio::task::spawn_blocking(move || parse_radar_json(&body, kind_copy))
        .await
        .map_err(|e| FetchErr::Other(format!("join: {e}")))?
        .map_err(FetchErr::Other)?;
    Ok(parsed)
}

fn parse_radar_json(body: &[u8], kind: RadarKind) -> Result<RadarFrame, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    if v.get("status").and_then(|s| s.as_str()) == Some("error") {
        return Err(v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("radar error")
            .into());
    }
    let bounds = v
        .get("bounds")
        .ok_or_else(|| "missing bounds".to_string())?;
    let sw = bounds
        .get("sw")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "missing sw".to_string())?;
    let ne = bounds
        .get("ne")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "missing ne".to_string())?;
    if sw.len() < 2 || ne.len() < 2 {
        return Err("bounds too short".into());
    }
    // API: [纬度, 经度]
    let sw_lat = sw[0].as_f64().ok_or("sw lat")?;
    let sw_lon = sw[1].as_f64().ok_or("sw lon")?;
    let ne_lat = ne[0].as_f64().ok_or("ne lat")?;
    let ne_lon = ne[1].as_f64().ok_or("ne lon")?;
    let time = v
        .get("time")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let image = v
        .get("image")
        .and_then(|i| i.as_str())
        .ok_or_else(|| "missing image".to_string())?;
    let png = decode_data_url_png(image)?;
    if png.is_empty() {
        return Err("empty png".into());
    }
    let img = image::load_from_memory(&png)
        .map_err(|e| format!("png decode: {e}"))?
        .to_rgba8();
    let width = img.width();
    let height = img.height();
    let rgba = Arc::new(img.into_raw());
    Ok(RadarFrame {
        kind,
        time,
        sw_lat,
        sw_lon,
        ne_lat,
        ne_lon,
        width,
        height,
        rgba,
    })
}

fn decode_data_url_png(s: &str) -> Result<Vec<u8>, String> {
    let b64 = if let Some(rest) = s.strip_prefix("data:") {
        rest.split_once(',')
            .map(|(_, b)| b)
            .ok_or_else(|| "bad data url".to_string())?
    } else {
        s
    };
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64.trim()))
        .map_err(|e| format!("base64: {e}"))
}

async fn fetch_precip(
    client: &reqwest::Client,
    url: &str,
    hours: u16,
) -> Result<PrecipFrame, FetchErr> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| FetchErr::Other(e.to_string()))?;
    if resp.status().as_u16() == 429 {
        return Err(FetchErr::RateLimited);
    }
    if !resp.status().is_success() {
        return Err(FetchErr::Other(format!("HTTP {}", resp.status())));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| FetchErr::Other(e.to_string()))?;
    parse_precip_json(&body, hours).map_err(FetchErr::Other)
}

fn parse_precip_json(body: &[u8], hours: u16) -> Result<PrecipFrame, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let time = v
        .pointer("/properties/time")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let features = v
        .get("features")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let mut polys = Vec::with_capacity(features.len());
    for f in features {
        let props = f.get("properties").cloned().unwrap_or(Value::Null);
        let level = props
            .get("level")
            .and_then(|l| l.as_str())
            .unwrap_or("")
            .to_string();
        let fill = parse_rgba(
            props
                .get("fillColor")
                .or_else(|| props.get("color"))
                .and_then(|c| c.as_str())
                .unwrap_or("rgba(100,180,255,0.55)"),
        );
        let geom = f.get("geometry").cloned().unwrap_or(Value::Null);
        let typ = geom.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let coords = geom.get("coordinates").cloned().unwrap_or(Value::Null);
        match typ {
            "Polygon" => {
                if let Some(ring) = coords.as_array().and_then(|a| a.first()) {
                    if let Some(pts) = ring_lonlat(ring) {
                        if pts.len() >= 3 {
                            polys.push(PrecipPoly {
                                level: level.clone(),
                                fill,
                                ring: pts,
                            });
                        }
                    }
                }
            }
            "MultiPolygon" => {
                if let Some(arr) = coords.as_array() {
                    for poly in arr {
                        if let Some(ring) = poly.as_array().and_then(|a| a.first()) {
                            if let Some(pts) = ring_lonlat(ring) {
                                if pts.len() >= 3 {
                                    polys.push(PrecipPoly {
                                        level: level.clone(),
                                        fill,
                                        ring: pts,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(PrecipFrame {
        hours,
        time,
        polys,
    })
}

fn ring_lonlat(v: &Value) -> Option<Vec<[f64; 2]>> {
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for p in arr {
        let xy = p.as_array()?;
        if xy.len() < 2 {
            continue;
        }
        let lon = xy[0].as_f64()?;
        let lat = xy[1].as_f64()?;
        out.push([lon, lat]);
    }
    Some(out)
}

fn parse_rgba(s: &str) -> [u8; 4] {
    let s = s.trim();
    if let Some(inner) = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))
        .and_then(|x| x.strip_suffix(')'))
    {
        let parts: Vec<_> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() >= 3 {
            let r = parts[0].parse::<f32>().unwrap_or(100.0) as u8;
            let g = parts[1].parse::<f32>().unwrap_or(180.0) as u8;
            let b = parts[2].parse::<f32>().unwrap_or(255.0) as u8;
            let a = if parts.len() >= 4 {
                let v = parts[3].parse::<f32>().unwrap_or(0.55);
                if v <= 1.0 {
                    (v * 255.0) as u8
                } else {
                    v as u8
                }
            } else {
                140
            };
            return [r, g, b, a.max(40)];
        }
    }
    [100, 180, 255, 140]
}
