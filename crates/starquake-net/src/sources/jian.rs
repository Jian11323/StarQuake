//! Jian Project API（`api.sismotide.top`）：`/all` 预警情报 + `/kmoni` 测站。

use starquake_config::JianSourceConfig;
use starquake_core::HealthStatus;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::adapter::{ingest_jian_frame, ingest_kmoni_frame, KmoniState};
use crate::auth::exchange_access_token;
use crate::bus::{emit_health, NetTx};
use crate::sources::ws::{redact_url, run_ws_loop, WsLoopOpts};

fn has_credentials(cfg: &JianSourceConfig) -> bool {
    cfg.refresh_token
        .as_ref()
        .map(|s| s.trim().starts_with("rt_"))
        .unwrap_or(false)
        || cfg
            .access_token
            .as_ref()
            .map(|s| s.trim().starts_with("at_"))
            .unwrap_or(false)
}

/// 优先 `rt_→at_`（每次连接前应调用）；无 rt_ 时才用环境变量/配置里的短期 `at_`。
pub fn resolve_access_token(cfg: &JianSourceConfig) -> Result<String, String> {
    if let Some(rt) = cfg
        .refresh_token
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| s.starts_with("rt_"))
    {
        return exchange_access_token(rt).map(|ok| {
            info!("jian: 已用 rt_ 换取 at_");
            ok.token
        });
    }
    if let Some(at) = cfg
        .access_token
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| s.starts_with("at_"))
    {
        return Ok(at.to_string());
    }
    Err("无 at_/rt_（请在设置中用 lk_ 换票）".into())
}

/// 去掉已有 `key=` 后再追加，避免旧票卡死 URL。
pub fn url_with_key(base: &str, token: &str) -> String {
    let stripped = strip_key_param(base.trim());
    let sep = if stripped.contains('?') { '&' } else { '?' };
    format!("{stripped}{sep}key={token}")
}

/// `/all` 每次 connect 前换票拼 `key=`（测站中转无需鉴权，勿用于站网）。
pub fn make_resolve_url(base_url: String, cfg: JianSourceConfig) -> crate::sources::ws::ResolveUrl {
    Arc::new(move || {
        let rewritten = crate::jian_endpoint::rewrite_api_url(&base_url);
        let token = resolve_access_token(&cfg)?;
        Ok(url_with_key(&rewritten, &token))
    })
}

fn strip_key_param(url: &str) -> String {
    let (base, frag) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let Some((path, query)) = base.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("key="))
        .collect();
    let mut out = path.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}

pub fn spawn(tx: NetTx, cfg: JianSourceConfig) -> Vec<JoinHandle<()>> {
    let mut tasks = Vec::new();

    if cfg.enabled {
        if !has_credentials(&cfg) {
            warn!("jian: 无 at_/rt_（请在设置中用 lk_ 换票），跳过 /all");
            emit_health(&tx, "jian", HealthStatus::Abnormal);
            tasks.extend(spawn_kmoni_if(&tx, &cfg));
            return tasks;
        }

        // 速报列表始终拉取；是否展示由「信息事件震级过滤」决定
        let list_cmds = vec![
            "cenclist".into(),
            "cwalist".into(),
            "jmalist".into(),
            "hkolist".into(),
        ];
        let period = 60;

        let base_url = cfg.ws_url.clone();
        info!(url = %redact_url(&base_url), "jian: spawn /all");
        let tx_all = tx.clone();
        let cfg_all = cfg.clone();
        let resolve = make_resolve_url(base_url, cfg.clone());
        tasks.push(tokio::spawn(async move {
            run_ws_loop(
                tx_all,
                WsLoopOpts {
                    source: "jian",
                    url: cfg_all.ws_url.clone(),
                    resolve_url: Some(resolve),
                    // 官方仅 cenc/cwa/jma/hko 支持 *list；分别拉取后由 upsert 混合去重、新→旧
                    on_open_texts: list_cmds.clone(),
                    periodic_texts: list_cmds,
                    periodic_secs: period,
                    backoff_max_secs: 60,
                    stale_secs: 0,
                },
                move |tx, text| ingest_jian_frame(tx, text, Some(&cfg_all)),
            )
            .await;
        }));
    }

    if cfg.station_enabled {
        tasks.extend(spawn_kmoni_if(&tx, &cfg));
    }

    // 整源关闭时不标「异常」，由 UI 显示「关闭」
    tasks
}

fn spawn_kmoni_if(tx: &NetTx, cfg: &JianSourceConfig) -> Vec<JoinHandle<()>> {
    if !cfg.station_enabled {
        return Vec::new();
    }
    // 测站中转无需鉴权；hub 默认走 stations 模块，此路径仅兼容旧键。
    let url = cfg.station_ws_url.clone();
    info!(url = %redact_url(&url), "jian: spawn kmoni (no auth)");
    let tx = tx.clone();
    vec![tokio::spawn(async move {
        let mut state = KmoniState::default();
        run_ws_loop(
            tx,
            WsLoopOpts {
                source: "kmoni",
                url,
                resolve_url: None,
                on_open_texts: Vec::new(),
                periodic_texts: Vec::new(),
                periodic_secs: 0,
                backoff_max_secs: 60,
                stale_secs: 15,
            },
            move |tx, text| ingest_kmoni_frame(&mut state, tx, text),
        )
        .await;
    })]
}

#[cfg(test)]
mod tests {
    use super::{strip_key_param, url_with_key};

    #[test]
    fn strip_and_append_key() {
        assert_eq!(
            url_with_key("wss://api.sismotide.top/all", "at_new"),
            "wss://api.sismotide.top/all?key=at_new"
        );
        assert_eq!(
            url_with_key("wss://api.sismotide.top/all?key=at_old", "at_new"),
            "wss://api.sismotide.top/all?key=at_new"
        );
        assert_eq!(
            strip_key_param("wss://x/all?foo=1&key=at_old&bar=2"),
            "wss://x/all?foo=1&bar=2"
        );
    }
}
