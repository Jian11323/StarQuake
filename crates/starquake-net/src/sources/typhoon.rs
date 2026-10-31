//! 活跃台风路径 / 风圈（HTTP 轮询，对齐 eewcn MapTyphoonLogic）。

use starquake_config::TyphoonSourceConfig;
use starquake_core::{
    HealthStatus, TyphoonForecast, TyphoonForecastPoint, TyphoonInfo, TyphoonQuadrants,
    TyphoonTrackPoint,
};
use serde_json::Value;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::bus::{emit_health, NetEvent, NetTx};

pub fn spawn(tx: NetTx, cfg: TyphoonSourceConfig) -> Vec<JoinHandle<()>> {
    if !cfg.enabled {
        return Vec::new();
    }
    let poll = cfg.poll_secs.max(10);
    let url_base = cfg.url.clone();
    info!(%url_base, poll_secs = poll, "typhoon: spawn http poll");

    vec![tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .user_agent(concat!(
                "StarQuake/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/Jian11323/StarQuake)"
            ))
            .timeout(Duration::from_secs(45))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!("typhoon client: {e}");
                emit_health(&tx, "typhoon", HealthStatus::Abnormal);
                return;
            }
        };

        // 错开启动瞬间与其它同 IP 的 GET（服务端 1s 限流）
        sleep(Duration::from_millis(500)).await;

        loop {
            emit_health(&tx, "typhoon", HealthStatus::Fluctuating);
            let url = crate::jian_endpoint::rewrite_api_url(&url_base);
            let fetch_url = format!(
                "{url}{sep}_={ts}",
                sep = if url.contains('?') { '&' } else { '?' },
                ts = chrono::Utc::now().timestamp_millis()
            );
            let mut ok = false;
            let mut rate_limited = false;
            match client.get(&fetch_url).send().await {
                Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                    Ok(body) => match parse_typhoon_list(&body) {
                        Ok(list) => {
                            let _ = tx.send(NetEvent::Typhoons(list));
                            ok = true;
                        }
                        Err(e) => warn!("typhoon parse: {e}"),
                    },
                    Err(e) => warn!("typhoon body: {e}"),
                },
                Ok(resp) => {
                    rate_limited = resp.status().as_u16() == 429;
                    warn!(status = %resp.status(), "typhoon http");
                }
                Err(e) => warn!("typhoon fetch: {e}"),
            }
            crate::jian_endpoint::note_connect_result(ok);
            emit_health(
                &tx,
                "typhoon",
                if ok {
                    HealthStatus::Normal
                } else {
                    HealthStatus::Abnormal
                },
            );
            let wait = if rate_limited {
                poll.max(3)
            } else {
                poll
            };
            sleep(Duration::from_secs(wait)).await;
        }
    })]
}

fn parse_typhoon_list(body: &[u8]) -> Result<Vec<TyphoonInfo>, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let arr = match v {
        Value::Array(a) => a,
        Value::Object(o) => o
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        if let Some(tf) = parse_one(&item) {
            out.push(tf);
        }
    }
    Ok(out)
}

fn parse_one(v: &Value) -> Option<TyphoonInfo> {
    // Fan Studio: isactive!="1" skip; 自建 API 无 isactive 视为活跃
    if let Some(a) = v.get("isactive") {
        let s = match a {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => if *b { "1".into() } else { "0".into() },
            _ => String::new(),
        };
        if !s.is_empty() && s != "1" {
            return None;
        }
    }
    let id = json_str(v, &["id", "tfid"]).unwrap_or_default();
    if id.is_empty() {
        return None;
    }
    let name = json_str(v, &["name"]).unwrap_or_else(|| id.clone());
    let name_en = json_str(v, &["name_en", "enname", "nameEn"]).unwrap_or_default();
    let mut points = Vec::new();
    if let Some(Value::Array(pts)) = v.get("points") {
        for (i, pt) in pts.iter().enumerate() {
            if let Some(mut p) = parse_point(pt) {
                // 仅保留末点预报，减负
                if i + 1 < pts.len() {
                    p.forecasts.clear();
                }
                points.push(p);
            }
        }
    }
    let last = points.last();
    let (lon, lat) = if let Some(p) = last {
        (p.longitude, p.latitude)
    } else {
        let lon = json_f64(v, &["longitude", "centerlng", "lng"])?;
        let lat = json_f64(v, &["latitude", "centerlat", "lat"])?;
        (lon, lat)
    };
    let strong = last
        .map(|p| p.strong.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| json_str(v, &["type", "strong"]))
        .unwrap_or_default();
    let power = last
        .map(|p| p.power)
        .filter(|&p| p > 0)
        .unwrap_or_else(|| json_i32(v, &["power"]).unwrap_or(0));
    Some(TyphoonInfo {
        id,
        name,
        name_en,
        strong,
        power,
        longitude: lon,
        latitude: lat,
        pressure_hpa: last
            .map(|p| p.pressure_hpa)
            .filter(|&p| p > 0.0)
            .unwrap_or_else(|| json_f64(v, &["pressure"]).unwrap_or(0.0)),
        wind_speed_ms: last
            .map(|p| p.speed_ms)
            .filter(|&p| p > 0.0)
            .unwrap_or_else(|| json_f64(v, &["windSpeed", "speed"]).unwrap_or(0.0)),
        move_direction: last
            .map(|p| p.move_direction.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| json_str(v, &["moveDirection", "movedirection"]))
            .unwrap_or_default(),
        move_speed_kmh: last
            .map(|p| p.move_speed_kmh)
            .filter(|&p| p > 0.0)
            .unwrap_or_else(|| json_f64(v, &["moveSpeed", "movespeed"]).unwrap_or(0.0)),
        update_text: json_str(v, &["updateTime", "endtime", "time"]).unwrap_or_default(),
        points,
    })
}

fn parse_point(v: &Value) -> Option<TyphoonTrackPoint> {
    let longitude = json_f64(v, &["lng", "longitude", "lon"])?;
    let latitude = json_f64(v, &["lat", "latitude"])?;
    let mut forecasts = Vec::new();
    if let Some(Value::Array(fcs)) = v.get("forecast") {
        for fc in fcs {
            let agency = json_str(fc, &["tm", "agency"]).unwrap_or_default();
            let mut fpts = Vec::new();
            if let Some(Value::Array(arr)) = fc.get("forecastpoints") {
                for fp in arr {
                    if let (Some(lng), Some(lat)) = (
                        json_f64(fp, &["lng", "longitude", "lon"]),
                        json_f64(fp, &["lat", "latitude"]),
                    ) {
                        fpts.push(TyphoonForecastPoint {
                            longitude: lng,
                            latitude: lat,
                            time_text: json_str(fp, &["time"]).unwrap_or_default(),
                            strong: json_str(fp, &["strong"]).unwrap_or_default(),
                            power: json_i32(fp, &["power"]).unwrap_or(0),
                        });
                    }
                }
            }
            if fpts.len() >= 2 {
                forecasts.push(TyphoonForecast {
                    agency,
                    points: fpts,
                });
            }
        }
    }
    Some(TyphoonTrackPoint {
        longitude,
        latitude,
        time_text: json_str(v, &["time"]).unwrap_or_default(),
        strong: json_str(v, &["strong"]).unwrap_or_default(),
        power: json_i32(v, &["power"]).unwrap_or(0),
        speed_ms: json_f64(v, &["speed"]).unwrap_or(0.0),
        pressure_hpa: json_f64(v, &["pressure"]).unwrap_or(0.0),
        move_direction: json_str(v, &["movedirection", "moveDirection"]).unwrap_or_default(),
        move_speed_kmh: json_f64(v, &["movespeed", "moveSpeed"]).unwrap_or(0.0),
        radius7: json_radius(v, "radius7"),
        radius10: json_radius(v, "radius10"),
        radius12: json_radius(v, "radius12"),
        forecasts,
    })
}

fn json_radius(v: &Value, key: &str) -> Option<TyphoonQuadrants> {
    match v.get(key)? {
        Value::String(s) => TyphoonQuadrants::from_pipe(s),
        Value::Number(n) => {
            let x = n.as_f64()?;
            if x > 0.0 {
                Some(TyphoonQuadrants {
                    ne: x,
                    se: x,
                    sw: x,
                    nw: x,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn json_str(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(x) = v.get(*k) {
            match x {
                Value::String(s) if !s.is_empty() => return Some(s.clone()),
                Value::Number(n) => return Some(n.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn json_f64(v: &Value, keys: &[&str]) -> Option<f64> {
    for k in keys {
        if let Some(x) = v.get(*k) {
            match x {
                Value::Number(n) => return n.as_f64(),
                Value::String(s) => {
                    if let Ok(f) = s.trim().parse::<f64>() {
                        return Some(f);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn json_i32(v: &Value, keys: &[&str]) -> Option<i32> {
    json_f64(v, keys).map(|f| f as i32)
}
