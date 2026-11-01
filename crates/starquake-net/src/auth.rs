//! Jian Project 鉴权：`lk_` → `rt_` → `at_`。
//!
//! - `lk_`→`rt_` 签发固定走主站 `auth.sismotide.top`
//! - `rt_`→`at_` 随当前 API 主机：主服务器 → `auth.sismotide.top`；
//!   备服务器 → `api.jianproject.cloud/api/access`（备服 auth-site replica；本地验票）

use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::jian_endpoint::{self, BACKUP_HOST};
use crate::notice::{notice_from_server, parse_server_error};

const REFRESH_URL: &str = "https://auth.sismotide.top/api/refresh";
/// 备服公网 `auth.jianproject.cloud` 尚无独立 HTTPS 证书时，换 at_ 挂在 API 域。
const BACKUP_ACCESS_URL: &str = "https://api.jianproject.cloud/api/access";
const PRIMARY_ACCESS_URL: &str = "https://auth.sismotide.top/api/access";
const UA: &str = "StarQuake/0.1";

fn access_url_for_active_api() -> &'static str {
    if jian_endpoint::active_host() == BACKUP_HOST {
        BACKUP_ACCESS_URL
    } else {
        PRIMARY_ACCESS_URL
    }
}

#[derive(Debug, Clone)]
pub struct TokenOk {
    pub token: String,
    /// 本响应令牌过期时刻（unix 秒）；未知则为 0
    pub expire_at: f64,
    /// 所用/签发的长期 Token（rt_）过期时刻（unix 秒）；未知则为 0
    pub refresh_expire_at: f64,
    /// 每把长期 Token 最大并发；未知为 0
    pub max_connections: u32,
    pub message: String,
}

#[derive(Deserialize)]
struct AuthBody {
    ok: Option<bool>,
    token: Option<String>,
    error: Option<String>,
    message: Option<String>,
    code: Option<serde_json::Value>,
    expires_after_sec: Option<i64>,
    expires_after_min: Option<i64>,
    refresh_expires_at: Option<i64>,
    refresh_expires_after_min: Option<i64>,
    max_connections: Option<u32>,
}

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn classify(value: &str) -> &str {
    let v = value.trim();
    if v.starts_with("lk_") {
        "login"
    } else if v.starts_with("rt_") {
        "refresh"
    } else if v.starts_with("at_") {
        "access"
    } else {
        "unknown"
    }
}

fn parse_refresh_expire(body: &AuthBody) -> f64 {
    if let Some(at) = body.refresh_expires_at {
        if at > 0 {
            return at as f64;
        }
    }
    if let Some(min) = body.refresh_expires_after_min {
        if min > 0 {
            return now_unix() + (min as f64) * 60.0;
        }
    }
    0.0
}

fn code_i32(code: &Option<serde_json::Value>) -> i32 {
    match code {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) as i32,
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn format_auth_err(error: Option<String>, message: Option<String>, code: Option<serde_json::Value>) -> String {
    let c = code_i32(&code);
    let err = error.unwrap_or_default();
    let msg = message.unwrap_or_default();
    let n = notice_from_server(c, &err, &msg);
    if c > 0 {
        format!("{}：{} (code={c})", n.title, n.body.lines().next().unwrap_or(""))
    } else if !msg.is_empty() {
        if !err.is_empty() {
            format!("{msg} ({err})")
        } else {
            msg
        }
    } else if !err.is_empty() {
        err
    } else {
        "鉴权失败".into()
    }
}

fn parse_body(body: AuthBody) -> Result<TokenOk, String> {
    if body.ok == Some(false) {
        return Err(format_auth_err(body.error, body.message, body.code));
    }
    let token = body.token.clone().unwrap_or_default().trim().to_string();
    if token.is_empty() {
        return Err(format_auth_err(body.error, body.message, body.code));
    }
    let refresh_expire_at = parse_refresh_expire(&body);
    let expire_at = if let Some(sec) = body.expires_after_sec {
        now_unix() + sec.max(0) as f64
    } else if let Some(min) = body.expires_after_min {
        now_unix() + (min.max(0) as f64) * 60.0
    } else if refresh_expire_at > 0.0 {
        refresh_expire_at
    } else {
        0.0
    };
    Ok(TokenOk {
        token,
        expire_at,
        refresh_expire_at,
        max_connections: body.max_connections.unwrap_or(0),
        message: "OK".into(),
    })
}

fn post_bearer(url: &str, token: &str) -> Result<TokenOk, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(UA)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", token.trim()))
        .header("Accept", "application/json")
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if let Ok(body) = serde_json::from_str::<AuthBody>(&text) {
        return parse_body(body);
    }
    if let Some((code, error, message)) = parse_server_error(&text) {
        return Err(format_auth_err(
            Some(error),
            Some(message),
            Some(serde_json::json!(code)),
        ));
    }
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    Err("鉴权响应无法解析".into())
}

/// 用邮件登录密钥 `lk_` 换取长期 Token `rt_`（约 180 天）。签发固定主站。
pub fn exchange_login_key(login_key: &str) -> Result<TokenOk, String> {
    let key = login_key.trim();
    if key.is_empty() {
        return Err("请先填写 Jian 登录密钥（邮件内 lk_…）".into());
    }
    if classify(key) != "login" {
        return Err("请填写邮件内 lk_ 开头的登录密钥（约 5 分钟有效、用后作废）".into());
    }
    let mut ok = post_bearer(REFRESH_URL, key)?;
    if !ok.token.starts_with("rt_") {
        return Err("期望 rt_ 长期 Token，收到非预期前缀".into());
    }
    if ok.refresh_expire_at <= 0.0 {
        ok.refresh_expire_at = ok.expire_at;
    }
    if ok.refresh_expire_at <= 0.0 {
        // 文档默认 180 天
        ok.refresh_expire_at = now_unix() + 259200.0 * 60.0;
    }
    ok.expire_at = ok.refresh_expire_at;
    ok.message = "鉴权成功，已保存长期 Token。".into();
    Ok(ok)
}

/// 用长期 Token `rt_` 换取访问令牌 `at_`（约 1 小时）。鉴权口随当前 API 主机。
pub fn exchange_access_token(refresh_token: &str) -> Result<TokenOk, String> {
    let token = refresh_token.trim();
    if token.is_empty() {
        return Err("长期 Token 为空".into());
    }
    if classify(token) != "refresh" {
        return Err("长期 Token 须为 rt_ 开头".into());
    }
    let url = access_url_for_active_api();
    let ok = post_bearer(url, token)?;
    if !ok.token.starts_with("at_") {
        return Err("期望 at_ 访问令牌，收到非预期前缀".into());
    }
    Ok(ok)
}

/// 验证已保存的 `rt_`（换一张 at_ 即视为有效）。
pub fn verify_refresh_token(refresh_token: &str) -> Result<TokenOk, String> {
    let ok = exchange_access_token(refresh_token)?;
    Ok(TokenOk {
        token: refresh_token.trim().to_string(),
        expire_at: ok.refresh_expire_at,
        refresh_expire_at: ok.refresh_expire_at,
        max_connections: ok.max_connections,
        message: "长期 Token 有效。".into(),
    })
}

/// 格式化 rt_ 过期日期（本地）。
pub fn format_expire_date(expire_at: f64) -> Option<String> {
    if expire_at <= 0.0 {
        return None;
    }
    let secs = expire_at as i64;
    chrono::DateTime::from_timestamp(secs, 0).map(|dt| {
        dt.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string()
    })
}
