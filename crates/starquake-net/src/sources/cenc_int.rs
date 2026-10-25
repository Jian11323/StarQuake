//! CENC 烈度速报：NowQuake 冷启动 GET `/list` + WebSocket（禁止持续轮询 GET）。

use starquake_config::CencIntSourceConfig;
use starquake_core::{CencIntEvent, CencIntStation, HealthStatus};
use serde_json::Value;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::bus::{emit_health, NetEvent, NetTx};
use crate::sources::ws::{redact_url, run_ws_loop, WsLoopOpts};
use crate::time_parse::parse_time_str;

pub fn spawn(tx: NetTx, cfg: CencIntSourceConfig) -> Vec<JoinHandle<()>> {
    if !cfg.enabled {
        return Vec::new();
    }
    let http_base = cfg.http_base.trim_end_matches('/').to_string();
    let ws_url = cfg.ws_url.clone();
    let page_size = cfg.list_page_size.clamp(1, 200);
    info!(
        http = %http_base,
        ws = %redact_url(&ws_url),
        page_size,
        "cenc_int: spawn NowQuake"
    );

    vec![tokio::spawn(async move {
        // 冷启动：仅一次 GET /list
        let client = match reqwest::Client::builder()
            .user_agent(concat!(
                "StarQuake/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/Jian11323/StarQuake)"
            ))
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!("cenc_int client: {e}");
                emit_health(&tx, "cenc_int", HealthStatus::Abnormal);
                return;
            }
        };

        emit_health(&tx, "cenc_int", HealthStatus::Fluctuating);
        let list_url = format!("{http_base}/list?pageNo=0&pageSize={page_size}");
        match client.get(&list_url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(body) => match parse_list(&body) {
                    Ok(list) => {
                        if !list.is_empty() {
                            let _ = tx.send(NetEvent::CencIntList(list));
                        }
                        emit_health(&tx, "cenc_int", HealthStatus::Normal);
                    }
                    Err(e) => {
                        warn!("cenc_int list parse: {e}");
                        emit_health(&tx, "cenc_int", HealthStatus::Abnormal);
                    }
                },
                Err(e) => {
                    warn!("cenc_int list body: {e}");
                    emit_health(&tx, "cenc_int", HealthStatus::Abnormal);
                }
            },
            Ok(resp) => {
                warn!(status = %resp.status(), "cenc_int list http");
                emit_health(&tx, "cenc_int", HealthStatus::Abnormal);
            }
            Err(e) => {
                warn!("cenc_int list fetch: {e}");
                emit_health(&tx, "cenc_int", HealthStatus::Abnormal);
            }
        }

        // WebSocket：heartbeat + data（含 stations）
        run_ws_loop(
            tx,
            WsLoopOpts {
                source: "cenc_int",
                url: ws_url,
                resolve_url: None,
                on_open_texts: Vec::new(),
                periodic_texts: Vec::new(),
                periodic_secs: 0,
                backoff_max_secs: 60,
                // 服务端约 60s 心跳
                stale_secs: 150,
            },
            ingest_cenc_int_frame,
        )
        .await;
    })]
}

pub fn ingest_cenc_int_frame(tx: &NetTx, text: &str) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let typ = v
        .get("type")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if typ == "heartbeat" || typ == "pong" {
        return;
    }
    // WsData: type=data + EventDetail 字段同级
    let data = if typ == "data" || v.get("eq_id").is_some() {
        &v
    } else {
        return;
    };
    if let Some(ev) = parse_event(data) {
        let _ = tx.send(NetEvent::CencInt(ev));
    }
}

fn parse_list(body: &[u8]) -> Result<Vec<CencIntEvent>, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let arr = v
        .as_array()
        .ok_or_else(|| "list not array".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        if let Some(e) = parse_event(item) {
            out.push(e);
        }
    }
    Ok(out)
}

fn parse_event(v: &Value) -> Option<CencIntEvent> {
    let eq_id = v.get("eq_id")?.as_str()?.trim().to_string();
    if eq_id.is_empty() {
        return None;
    }
    let happen_time = v
        .get("happen_time")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let happen_ms = parse_time_str(&happen_time).unwrap_or(0);
    let lat = opt_f64_req(v, "latitude")?;
    let lon = opt_f64_req(v, "longitude")?;
    let magnitude = opt_f64_req(v, "magnitude").unwrap_or(0.0);
    let stations = v
        .get("stations")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(parse_station)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(CencIntEvent {
        eq_id,
        happen_time,
        update_time: v
            .get("update_time")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        hypocenter: v
            .get("hypocenter")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        magnitude,
        depth_km: opt_f64(v, "depth"),
        latitude: lat,
        longitude: lon,
        max_intensity: opt_f64(v, "maxintensity").or_else(|| opt_f64(v, "maxIntensity")),
        max_forecast_intensity: opt_f64(v, "maxforecastintensity")
            .or_else(|| opt_f64(v, "maxForecastIntensity")),
        info: v
            .get("info")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        stations,
        happen_ms,
    })
}

fn parse_station(v: &Value) -> Option<CencIntStation> {
    let lat = opt_f64_req(v, "latitude")?;
    let lon = opt_f64_req(v, "longitude")?;
    let name = v
        .get("name")
        .or_else(|| v.get("stationid"))
        .or_else(|| v.get("id"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let place = v
        .get("location_name")
        .and_then(|ln| {
            ln.get("placename")
                .or_else(|| ln.get("county"))
                .and_then(|x| x.as_str())
        })
        .or_else(|| v.get("place").and_then(|x| x.as_str()))
        .unwrap_or("")
        .trim()
        .to_string();
    // 计测烈度字段为 JSON `int`
    let intensity = opt_f64(v, "int").or_else(|| opt_f64(v, "intensity"));
    Some(CencIntStation {
        name,
        place,
        latitude: lat,
        longitude: lon,
        intensity,
        forecast_int: opt_f64(v, "forecast_int"),
        pga: opt_f64(v, "pga"),
        pgv: opt_f64(v, "pgv"),
    })
}

fn opt_f64(v: &Value, key: &str) -> Option<f64> {
    let x = v.get(key)?;
    let n = if let Some(f) = x.as_f64() {
        f
    } else {
        let s = x.as_str()?;
        s.trim().parse().ok()?
    };
    if n.is_finite() {
        Some(n)
    } else {
        None
    }
}

fn opt_f64_req(v: &Value, key: &str) -> Option<f64> {
    opt_f64(v, key)
}
