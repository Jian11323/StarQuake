//! Jian Project API 主服务器 / 备服务器选择与自动切换。
//!
//! - 主服务器：`api.sismotide.top`
//! - 备服务器：`api.jianproject.cloud`
//! - `lk_`→`rt_` 签发固定 `auth.sismotide.top`；`rt_`→`at_` 随当前 API 主机走对应鉴权口
//!
//! `endpoint_mode`：`auto` | `primary` | `backup`
//!
//! 自动切换对齐 CysTerra / 滚动字幕：
//! - 仅统计 Jian WebSocket 网络类失败（握手失败、异常断开、换票失败等）
//! - 60 秒窗口内累计 3 次后主↔备翻转，供下次连接改写主机
//! - `set_mode("auto")` **不**重置失败计数与当前主机（避免热重建/重连把计数清零）

use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU32, Ordering};
use tracing::info;

pub const PRIMARY_HOST: &str = "api.sismotide.top";
pub const BACKUP_HOST: &str = "api.jianproject.cloud";

const MODE_AUTO: u8 = 0;
const MODE_PRIMARY: u8 = 1;
const MODE_BACKUP: u8 = 2;

const HOST_PRIMARY: u8 = 0;
const HOST_BACKUP: u8 = 1;

/// 短时间窗口内累计多少次网络异常后切换。
const FAIL_FLIP_THRESHOLD: u32 = 3;
/// 网络异常计数窗口（毫秒）。
const FAIL_WINDOW_MS: i64 = 60_000;

static MODE: AtomicU8 = AtomicU8::new(MODE_AUTO);
static ACTIVE: AtomicU8 = AtomicU8::new(HOST_PRIMARY);
static FAIL_STREAK: AtomicU32 = AtomicU32::new(0);
/// 当前失败窗口起点（unix ms）；0 表示尚未开窗。
static FAIL_WINDOW_START_MS: AtomicI64 = AtomicI64::new(0);

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

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

fn clear_fail_counter() {
    FAIL_STREAK.store(0, Ordering::SeqCst);
    FAIL_WINDOW_START_MS.store(0, Ordering::SeqCst);
}

/// 设置页保存 / 热切换源任务时调用。
///
/// - `primary` / `backup`：强制主机并清空失败计数
/// - `auto`：**保留**当前主机与失败计数，避免每次 `spawn_hub` / 重连把自动切换计数清零
pub fn set_mode(mode: &str) {
    let code = mode_code(mode);
    MODE.store(code, Ordering::SeqCst);
    match code {
        MODE_PRIMARY => {
            clear_fail_counter();
            ACTIVE.store(HOST_PRIMARY, Ordering::SeqCst);
            info!(host = PRIMARY_HOST, "jian endpoint: forced primary");
        }
        MODE_BACKUP => {
            clear_fail_counter();
            ACTIVE.store(HOST_BACKUP, Ordering::SeqCst);
            info!(host = BACKUP_HOST, "jian endpoint: forced backup");
        }
        _ => {
            // auto：不碰 FAIL_STREAK / ACTIVE
            if ACTIVE.load(Ordering::SeqCst) > HOST_BACKUP {
                ACTIVE.store(HOST_PRIMARY, Ordering::SeqCst);
            }
            info!(
                host = active_host(),
                fail_streak = FAIL_STREAK.load(Ordering::SeqCst),
                "jian endpoint: auto (keep active + fail streak)"
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

/// 当前 auto 模式下的失败计数（调试 / 单测）。
#[cfg(test)]
pub fn fail_streak() -> u32 {
    FAIL_STREAK.load(Ordering::SeqCst)
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

/// Jian WebSocket 连接结果反馈（仅 auto 模式会翻转）。
///
/// - `ok=true`：清空窗口计数
/// - `ok=false`：记入 60s 窗口；满 3 次则主↔备翻转，供下次 `rewrite_api_url` 使用
pub fn note_connect_result(ok: bool) {
    if MODE.load(Ordering::SeqCst) != MODE_AUTO {
        if ok {
            clear_fail_counter();
        }
        return;
    }
    if ok {
        clear_fail_counter();
        return;
    }

    let now = now_ms();
    let window_start = FAIL_WINDOW_START_MS.load(Ordering::SeqCst);
    if window_start == 0 || now - window_start > FAIL_WINDOW_MS {
        FAIL_WINDOW_START_MS.store(now, Ordering::SeqCst);
        FAIL_STREAK.store(1, Ordering::SeqCst);
        info!(
            host = active_host(),
            streak = 1u32,
            "jian endpoint: network fail (new window)"
        );
        return;
    }

    let streak = FAIL_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
    if streak < FAIL_FLIP_THRESHOLD {
        info!(
            host = active_host(),
            streak,
            "jian endpoint: network fail"
        );
        return;
    }

    clear_fail_counter();
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
    use std::sync::Mutex;

    /// 全局原子状态：单测必须串行，避免并行互相踩计数器。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn force_auto_primary() {
        MODE.store(MODE_AUTO, Ordering::SeqCst);
        ACTIVE.store(HOST_PRIMARY, Ordering::SeqCst);
        clear_fail_counter();
    }

    #[test]
    fn rewrite_swaps_known_hosts() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn set_mode_auto_keeps_active_and_streak() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force_auto_primary();
        note_connect_result(false);
        note_connect_result(false);
        assert_eq!(fail_streak(), 2);
        ACTIVE.store(HOST_BACKUP, Ordering::SeqCst);
        set_mode("auto");
        assert_eq!(active_host(), BACKUP_HOST);
        assert_eq!(fail_streak(), 2);
    }

    #[test]
    fn set_mode_primary_clears_and_forces() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force_auto_primary();
        ACTIVE.store(HOST_BACKUP, Ordering::SeqCst);
        note_connect_result(false);
        set_mode("primary");
        assert_eq!(active_host(), PRIMARY_HOST);
        assert_eq!(fail_streak(), 0);
    }

    #[test]
    fn three_fails_in_window_flip_host() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force_auto_primary();
        assert_eq!(active_host(), PRIMARY_HOST);
        note_connect_result(false);
        note_connect_result(false);
        assert_eq!(active_host(), PRIMARY_HOST);
        note_connect_result(false);
        assert_eq!(active_host(), BACKUP_HOST);
        assert_eq!(fail_streak(), 0);
    }

    #[test]
    fn success_clears_streak() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        force_auto_primary();
        note_connect_result(false);
        note_connect_result(false);
        note_connect_result(true);
        assert_eq!(fail_streak(), 0);
        note_connect_result(false);
        assert_eq!(fail_streak(), 1);
        assert_eq!(active_host(), PRIMARY_HOST);
    }
}
