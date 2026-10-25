//! WebSocket 重连循环（公共）。
//!
//! 对齐 EEWCN 测站链路：应用层看门狗、指数退避 + 抖动，僵死连接及时重建。

use futures_util::{SinkExt, StreamExt};
use starquake_core::HealthStatus;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

use crate::bus::{emit_health, NetEvent, NetTx};
use crate::notice::{
    is_auth_error_text, is_ban_text, is_conn_limit_text, notice_from_server, parse_server_error,
};

pub use starquake_util::{redact_secrets, redact_url};

fn emit_notice_from_text(tx: &NetTx, text: &str) {
    if let Some((code, error, message)) = parse_server_error(text) {
        let _ = tx.send(NetEvent::UserNotice(notice_from_server(code, &error, &message)));
        return;
    }
    if is_conn_limit_text(text) {
        let _ = tx.send(NetEvent::UserNotice(notice_from_server(
            4005,
            "conn_limit",
            "您已超过服务器连接限制",
        )));
    } else if is_ban_text(text) {
        let _ = tx.send(NetEvent::UserNotice(notice_from_server(
            0,
            "banned",
            "您的IP已被封禁，请联系管理员进行处理",
        )));
    }
}

/// 每次连接前解析 URL（例如用 rt_ 换新 at_ 再拼 key=）。
pub type ResolveUrl = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

pub struct WsLoopOpts {
    pub source: &'static str,
    pub url: String,
    /// 若设置，每次 `connect` 前调用（可阻塞换票）；成功则覆盖 `url`
    pub resolve_url: Option<ResolveUrl>,
    /// 连接成功后立刻发送的文本（鉴权 / query / list）
    pub on_open_texts: Vec<String>,
    /// 连接存活期间周期性重发（如 Jian cenclist）；空则不发
    pub periodic_texts: Vec<String>,
    /// 周期秒数；0 或 periodic_texts 空则禁用
    pub periodic_secs: u64,
    pub backoff_max_secs: u64,
    /// 无任何下行帧（含 ping/pong/heartbeat）超过该秒数则主动断开重连；0=关闭。
    /// kmoni/kma 建议 15；s-net 约每分钟一帧，建议 120。
    pub stale_secs: u64,
}

impl WsLoopOpts {
    pub fn with_stale(mut self, secs: u64) -> Self {
        self.stale_secs = secs;
        self
    }
}

/// 收到文本帧时调用
pub async fn run_ws_loop<F>(tx: NetTx, opts: WsLoopOpts, mut on_text: F)
where
    F: FnMut(&NetTx, &str) + Send + 'static,
{
    let mut backoff = 1u64;
    loop {
        emit_health(&tx, opts.source, HealthStatus::Fluctuating);

        let url = if let Some(ref resolve) = opts.resolve_url {
            let resolve = Arc::clone(resolve);
            match tokio::task::spawn_blocking(move || resolve()).await {
                Ok(Ok(u)) => u,
                Ok(Err(e)) => {
                    warn!(
                        source = opts.source,
                        error = %redact_secrets(&e),
                        "resolve url failed"
                    );
                    emit_health(&tx, opts.source, HealthStatus::Abnormal);
                    backoff = backoff.max(15);
                    let jitter_ms = backoff.saturating_mul(137) % 800;
                    sleep(Duration::from_secs(backoff) + Duration::from_millis(jitter_ms)).await;
                    backoff = (backoff * 2).min(opts.backoff_max_secs.max(60));
                    continue;
                }
                Err(e) => {
                    warn!(source = opts.source, "resolve url join: {e}");
                    emit_health(&tx, opts.source, HealthStatus::Abnormal);
                    sleep(Duration::from_secs(backoff.max(5))).await;
                    backoff = (backoff * 2).min(opts.backoff_max_secs.max(60));
                    continue;
                }
            }
        } else {
            opts.url.clone()
        };

        info!(
            source = opts.source,
            url = %redact_url(&url),
            "connecting"
        );

        match connect_async(&url).await {
            Ok((ws, _)) => {
                emit_health(&tx, opts.source, HealthStatus::Normal);
                backoff = 1;
                let (mut write, mut read) = ws.split();
                for text in &opts.on_open_texts {
                    if let Err(e) = write.send(Message::Text(text.clone().into())).await {
                        warn!(source = opts.source, "on_open send failed: {e}");
                    }
                }
                let mut rate_limited = false;
                let mut auth_failed = false;
                let use_periodic =
                    opts.periodic_secs > 0 && !opts.periodic_texts.is_empty();
                let mut tick = if use_periodic {
                    let mut t = interval(Duration::from_secs(opts.periodic_secs));
                    t.set_missed_tick_behavior(MissedTickBehavior::Delay);
                    // 跳过立刻触发的首 tick（on_open 已发过）
                    t.tick().await;
                    Some(t)
                } else {
                    None
                };
                let stale = if opts.stale_secs > 0 {
                    Duration::from_secs(opts.stale_secs)
                } else {
                    Duration::from_secs(u64::MAX / 4)
                };
                let mut last_msg = Instant::now();
                // 首包宽限：台账大帧可能较慢（对齐 EEWCN awaitFirstMessage）
                let first_grace = Duration::from_secs(opts.stale_secs.clamp(20, 90));
                let opened_at = Instant::now();
                let mut got_first = false;
                loop {
                    let watchdog =
                        sleep_until_deadline(last_msg, if got_first { stale } else { first_grace });
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(t))) => {
                                    last_msg = Instant::now();
                                    got_first = true;
                                    let text = t.as_str();
                                    if is_server_reject_frame(text) {
                                        emit_notice_from_text(&tx, text);
                                    }
                                    if is_conn_limit_error(text) {
                                        warn!(
                                            source = opts.source,
                                            "服务器连接数超限，请关闭其它占用同令牌或同 IP 的客户端后重试"
                                        );
                                        emit_health(&tx, opts.source, HealthStatus::Abnormal);
                                        rate_limited = true;
                                        break;
                                    }
                                    if is_ban_error(text) {
                                        warn!(
                                            source = opts.source,
                                            "IP 或账号封禁"
                                        );
                                        emit_health(&tx, opts.source, HealthStatus::Abnormal);
                                        rate_limited = true;
                                        break;
                                    }
                                    if is_auth_error(text) {
                                        let msg = error_message(text).unwrap_or("auth error");
                                        warn!(
                                            source = opts.source,
                                            message = %redact_secrets(msg),
                                            "鉴权失败，将换票后重连（避免无钥狂连触发 IP 封禁）"
                                        );
                                        emit_health(&tx, opts.source, HealthStatus::Abnormal);
                                        auth_failed = true;
                                        break;
                                    }
                                    if is_error_frame(text) {
                                        let msg = error_message(text).unwrap_or("error");
                                        warn!(
                                            source = opts.source,
                                            message = %redact_secrets(msg),
                                            "上游 error 帧"
                                        );
                                        emit_health(&tx, opts.source, HealthStatus::Abnormal);
                                    }
                                    on_text(&tx, text);
                                }
                                Some(Ok(Message::Ping(p))) => {
                                    last_msg = Instant::now();
                                    got_first = true;
                                    let _ = write.send(Message::Pong(p)).await;
                                }
                                Some(Ok(Message::Pong(_))) => {
                                    last_msg = Instant::now();
                                    got_first = true;
                                }
                                Some(Ok(Message::Close(_))) => {
                                    warn!(source = opts.source, "server closed");
                                    break;
                                }
                                Some(Err(e)) => {
                                    warn!(
                                        source = opts.source,
                                        "ws error: {}",
                                        redact_secrets(&e.to_string())
                                    );
                                    break;
                                }
                                None => break,
                                _ => {}
                            }
                        }
                        _ = async {
                            if let Some(t) = tick.as_mut() {
                                t.tick().await;
                            } else {
                                std::future::pending::<()>().await;
                            }
                        } => {
                            for text in &opts.periodic_texts {
                                if let Err(e) = write.send(Message::Text(text.clone().into())).await {
                                    warn!(source = opts.source, "periodic send failed: {e}");
                                    break;
                                }
                            }
                        }
                        _ = watchdog, if opts.stale_secs > 0 => {
                            let age = last_msg.elapsed().as_secs();
                            let open_age = opened_at.elapsed().as_secs();
                            warn!(
                                source = opts.source,
                                age_s = age,
                                open_s = open_age,
                                "watchdog stale → reconnect"
                            );
                            emit_health(&tx, opts.source, HealthStatus::Fluctuating);
                            break;
                        }
                    }
                }
                if rate_limited {
                    backoff = backoff.max(45);
                }
                if auth_failed {
                    // 官方：累计约 10 次无鉴权掐断会临时封 IP；鉴权失败后拉长退避并换票
                    backoff = backoff.max(20);
                }
                emit_health(&tx, opts.source, HealthStatus::Fluctuating);
            }
            Err(e) => {
                let es = e.to_string();
                warn!(
                    source = opts.source,
                    "connect failed: {}",
                    redact_secrets(&es)
                );
                if is_conn_limit_text(&es) || is_ban_text(&es) || es.contains("403") || es.contains("429")
                {
                    emit_notice_from_text(&tx, &es);
                    if is_conn_limit_text(&es) || es.contains("429") {
                        backoff = backoff.max(45);
                    } else {
                        backoff = backoff.max(60);
                    }
                }
                emit_health(&tx, opts.source, HealthStatus::Abnormal);
            }
        }

        let jitter_ms = backoff.saturating_mul(137) % 800;
        sleep(Duration::from_secs(backoff) + Duration::from_millis(jitter_ms)).await;
        backoff = (backoff * 2).min(opts.backoff_max_secs.max(60));
    }
}

async fn sleep_until_deadline(from: Instant, window: Duration) {
    let deadline = from + window;
    let now = Instant::now();
    if deadline > now {
        sleep(deadline - now).await;
    }
}

fn is_error_frame(text: &str) -> bool {
    text.contains("\"type\":\"error\"") || text.contains("\"type\": \"error\"")
}

fn is_server_reject_frame(text: &str) -> bool {
    is_error_frame(text)
        || (text.contains("\"ok\":false") || text.contains("\"ok\": false"))
            && (text.contains("\"code\"") || text.contains("\"error\""))
}

fn is_conn_limit_error(text: &str) -> bool {
    is_conn_limit_text(text)
        && (is_error_frame(text)
            || text.contains("\"ok\":false")
            || text.contains("\"ok\": false")
            || text.contains("连接限制")
            || text.contains("超过服务器连接"))
}

fn is_ban_error(text: &str) -> bool {
    is_ban_text(text)
        && (is_error_frame(text)
            || text.contains("\"ok\":false")
            || text.contains("\"ok\": false")
            || text.contains("已被封禁")
            || text.contains("黑名单"))
}

fn is_auth_error(text: &str) -> bool {
    if !(is_error_frame(text)
        || text.contains("\"ok\":false")
        || text.contains("\"ok\": false"))
    {
        return false;
    }
    is_auth_error_text(text)
}

fn error_message(text: &str) -> Option<&str> {
    let key = "\"message\"";
    let i = text.find(key)?;
    let rest = &text[i + key.len()..];
    let start = rest.find('"')? + 1;
    let rest = &rest[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::redact_url;

    #[test]
    fn redact_key_query() {
        assert_eq!(
            redact_url("wss://api.example/all?key=at_secret_token_xx"),
            "wss://api.example/all?key=***"
        );
        assert_eq!(
            redact_url("wss://api.example/all?foo=1&key=at_secret_token_xx&bar=2"),
            "wss://api.example/all?foo=1&key=***&bar=2"
        );
        assert_eq!(
            redact_url("wss://api.example/kmoni"),
            "wss://api.example/kmoni"
        );
    }
}
