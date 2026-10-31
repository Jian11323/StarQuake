//! Jian Project API 主服 / 备服选择与自动切换。
//!
//! - 主服：`api.sismotide.top`
//! - 备服：`api.jianproject.cloud`
//! - 鉴权签发仍固定走 `auth.sismotide.top`（不随本模块切换）
//!
//! `endpoint_mode`：`auto` | `primary` | `backup`

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::info;

pub const PRIMARY_HOST: &str = "api.sismotide.top";
pub const BACKUP_HOST: &str = "api.jianproject.cloud";

const MODE_AUTO: u8 = 0;
const MODE_PRIMARY: u8 = 1;
const MODE_BACKUP: u8 = 2;

const HOST_PRIMARY: u8 = 0;
const HOST_BACKUP: u8 = 1;

/// 连续连接失败多少次后（auto）切到另一端。
const FAIL_FLIP_THRESHOLD: u32 = 3;
/// 在备服上每隔多久试探主服一次。
const PRIMARY_PROBE_INTERVAL: Duration = Duration::from_secs(10 * 60);

static MODE: AtomicU8 = AtomicU8::new(MODE_AUTO);
static ACTIVE: AtomicU8 = AtomicU8::new(HOST_PRIMARY);
static FAIL_STREAK: AtomicU32 = AtomicU32::new(0);
static LAST_PRIMARY_PROBE: Mutex<Option<Instant>> = Mutex::new(None);

pub fn normalize_mode(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "primary" | "main" | "master" => "primary",
        "backup" | "bak" | "secondary" => "backup",
        _ => "auto",
    }
}

fn mode_code(mode: &str) -> u8 {
    match normalize_mode(mode) {
        "primary" => MODE_PRIMARY,
        "backup" => MODE_BACKUP,
        _ => MODE_AUTO,
    }
}

/// 设置页保存 / 热切换源任务时调用。
pub fn set_mode(mode: &str) {
    let code = mode_code(mode);
    MODE.store(code, Ordering::SeqCst);
    FAIL_STREAK.store(0, Ordering::SeqCst);
    match code {
        MODE_PRIMARY => {
            ACTIVE.store(HOST_PRIMARY, Ordering::SeqCst);
            info!(host = PRIMARY_HOST, "jian endpoint: forced primary");
        }
        MODE_BACKUP => {
            ACTIVE.store(HOST_BACKUP, Ordering::SeqCst);
            info!(host = BACKUP_HOST, "jian endpoint: forced backup");
        }
        _ => {
            // auto：保留当前 ACTIVE，默认仍从主服起步
            if ACTIVE.load(Ordering::SeqCst) > HOST_BACKUP {
                ACTIVE.store(HOST_PRIMARY, Ordering::SeqCst);
            }
            info!(
                host = active_host(),
                "jian endpoint: auto (start/keep active)"
            );
        }
    }
}

pub fn active_host() -> &'static str {
    match MODE.load(Ordering::SeqCst) {
        MODE_PRIMARY => PRIMARY_HOST,
        MODE_BACKUP => BACKUP_HOST,
        _ => {
            if ACTIVE.load(Ordering::SeqCst) == HOST_BACKUP {
                BACKUP_HOST
            } else {
                PRIMARY_HOST
            }
        }
    }
}

pub fn is_jian_api_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    h == PRIMARY_HOST || h == BACKUP_HOST
}

/// 将已知 Jian API 主机改写为当前活跃主机；其它主机原样返回。
pub fn rewrite_api_url(url: &str) -> String {
    let target = active_host();
    rewrite_api_url_to(url, target)
}

pub fn rewrite_api_url_to(url: &str, target_host: &str) -> String {
    let Some((scheme, rest)) = split_scheme(url) else {
        return url.to_string();
    };
    let (authority, path_q) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, String::new()),
    };
    let host_port = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .to_ascii_lowercase();
    let host = host_port.split(':').next().unwrap_or(&host_port);
    if !is_jian_api_host(host) {
        return url.to_string();
    }
    if host.eq_ignore_ascii_case(target_host) {
        return url.to_string();
    }
    // 保留原端口（通常无）；替换主机名
    let new_authority = if let Some((_, port)) = host_port.split_once(':') {
        format!("{target_host}:{port}")
    } else {
        target_host.to_string()
    };
    // 还原 userinfo（一般没有）
    let new_auth = if authority.contains('@') {
        let user = authority.rsplit_once('@').map(|(u, _)| u).unwrap_or("");
        format!("{user}@{new_authority}")
    } else {
        new_authority
    };
    format!("{scheme}{new_auth}{path_q}")
}

fn split_scheme(url: &str) -> Option<(&str, &str)> {
    for prefix in ["wss://", "ws://", "https://", "http://"] {
        if let Some(rest) = url.strip_prefix(prefix) {
            return Some((prefix, rest));
        }
    }
    None
}

/// WebSocket / HTTP 连接结果反馈（仅 auto 模式会翻转）。
pub fn note_connect_result(ok: bool) {
    if MODE.load(Ordering::SeqCst) != MODE_AUTO {
        if ok {
            FAIL_STREAK.store(0, Ordering::SeqCst);
        }
        return;
    }
    if ok {
        FAIL_STREAK.store(0, Ordering::SeqCst);
        // 已在备服且到了探测窗口：下次失败阈值降为 1，并尝试切回主服
        if ACTIVE.load(Ordering::SeqCst) == HOST_BACKUP {
            let mut last = LAST_PRIMARY_PROBE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let due = (*last)
                .map(|t| t.elapsed() >= PRIMARY_PROBE_INTERVAL)
                .unwrap_or(true);
            if due {
                *last = Some(Instant::now());
                ACTIVE.store(HOST_PRIMARY, Ordering::SeqCst);
                info!(host = PRIMARY_HOST, "jian endpoint: probe primary after backup OK");
            }
        }
        return;
    }
    let streak = FAIL_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
    if streak < FAIL_FLIP_THRESHOLD {
        return;
    }
    FAIL_STREAK.store(0, Ordering::SeqCst);
    let cur = ACTIVE.load(Ordering::SeqCst);
    let next = if cur == HOST_BACKUP {
        HOST_PRIMARY
    } else {
        HOST_BACKUP
    };
    ACTIVE.store(next, Ordering::SeqCst);
    info!(
        from = if cur == HOST_BACKUP {
            BACKUP_HOST
        } else {
            PRIMARY_HOST
        },
        to = if next == HOST_BACKUP {
            BACKUP_HOST
        } else {
            PRIMARY_HOST
        },
        "jian endpoint: auto failover"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_swaps_known_hosts() {
        set_mode("backup");
        assert_eq!(
            rewrite_api_url("wss://api.sismotide.top/all"),
            "wss://api.jianproject.cloud/all"
        );
        set_mode("primary");
        assert_eq!(
            rewrite_api_url("wss://api.jianproject.cloud/weather"),
            "wss://api.sismotide.top/weather"
        );
        assert_eq!(
            rewrite_api_url("https://example.com/x"),
            "https://example.com/x"
        );
    }

    #[test]
    fn normalize_modes() {
        assert_eq!(normalize_mode("AUTO"), "auto");
        assert_eq!(normalize_mode("primary"), "primary");
        assert_eq!(normalize_mode("backup"), "backup");
    }
}
