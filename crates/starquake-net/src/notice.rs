//! 服务器错误码 → 用户提示文案（对齐鉴权议程）。

use starquake_core::{UserNotice, UserNoticeKind};

/// 从 WS/HTTP JSON 文本解析 code / error / message。
pub fn parse_server_error(text: &str) -> Option<(i32, String, String)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let code = v
        .get("code")
        .and_then(|c| {
            c.as_i64()
                .or_else(|| c.as_u64().map(|u| u as i64))
                .or_else(|| c.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0) as i32;
    let error = v
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_string();
    let message = v
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    if code == 0 && error.is_empty() && message.is_empty() {
        return None;
    }
    Some((code, error, message))
}

pub fn notice_from_server(code: i32, error: &str, message: &str) -> UserNotice {
    let err = error.trim();
    let (kind, title, default_body, key) = classify(code, err);
    let body = if !message.trim().is_empty() {
        if code > 0 {
            format!("{}\n（code={code} / {err}）", message.trim())
        } else if !err.is_empty() {
            format!("{}\n（{err}）", message.trim())
        } else {
            message.trim().to_string()
        }
    } else if code > 0 {
        format!("{default_body}\n（code={code}）")
    } else {
        default_body.to_string()
    };
    UserNotice {
        kind,
        title: title.to_string(),
        body,
        code,
        dedupe_key: key,
    }
}

fn classify(code: i32, error: &str) -> (UserNoticeKind, &'static str, &'static str, String) {
    let e = error.to_ascii_lowercase();
    if code == 4005
        || e.contains("conn_limit")
        || e.contains("connection_limit")
        || e.contains("max_connections")
    {
        return (
            UserNoticeKind::ConnLimit,
            "连接超限",
            "同一长期 Token 或同 IP 并发连接已满。请关闭其它占用同令牌/同 IP 的客户端后重试。",
            "conn_limit".into(),
        );
    }
    if code == 4006
        || e.contains("account_banned")
        || e == "banned"
        || e.contains("temp_banned")
    {
        return (
            UserNoticeKind::Banned,
            "账号已封禁",
            "该邮箱账号已被临时或永久封禁，无法继续连接。请联系运营或等待解封。",
            "account_banned".into(),
        );
    }
    if code == 4403
        || e.contains("http_denied")
        || e.contains("forbidden")
        || e.contains("ip") && e.contains("ban")
    {
        return (
            UserNoticeKind::Banned,
            "IP 已封禁或拒绝访问",
            "当前 IP 被黑名单 / ACL 拒绝，或触发自动封禁。请稍后再试或更换网络。",
            "ip_banned".into(),
        );
    }
    let (kind, title, body) = match code {
        4001 => (
            UserNoticeKind::Auth,
            "缺少访问令牌",
            "未携带访问令牌（at_）。请在设置中用 lk_ 换票，或检查长期 Token。",
        ),
        4002 => (
            UserNoticeKind::Auth,
            "令牌格式无效",
            "访问令牌前缀或形态不正确。请重新换票。",
        ),
        4003 => (
            UserNoticeKind::Auth,
            "访问令牌无效",
            "访问令牌无效。请用长期 Token（rt_）重新换取 at_。",
        ),
        4004 => (
            UserNoticeKind::Auth,
            "访问令牌已过期",
            "访问令牌已过期。客户端将尝试用 rt_ 自动换票后重连。",
        ),
        4101 => (
            UserNoticeKind::Auth,
            "登录密钥无效",
            "邮件内的登录密钥（lk_）无效。请重新申请。",
        ),
        4102 => (
            UserNoticeKind::Auth,
            "登录密钥已过期",
            "登录密钥约 5 分钟有效且用后作废。请重新申请邮件密钥。",
        ),
        4103 => (
            UserNoticeKind::Auth,
            "长期 Token 已存在",
            "该邮箱已有长期 Token。可在申请页勾选吊销旧票后重签。",
        ),
        4201 => (
            UserNoticeKind::Auth,
            "长期 Token 无效",
            "长期 Token（rt_）无效。请用新的 lk_ 重新换票。",
        ),
        4202 => (
            UserNoticeKind::Auth,
            "长期 Token 已过期",
            "长期 Token 已过期（约 180 天）。请重新申请邮件密钥并换票。",
        ),
        4301 => (
            UserNoticeKind::Info,
            "发信冷却中",
            "申请邮件过于频繁，请稍后再试。",
        ),
        4302 => (
            UserNoticeKind::Auth,
            "仅支持 QQ 邮箱",
            "鉴权申请仅接受 QQ 邮箱。",
        ),
        4303 => (
            UserNoticeKind::Auth,
            "未同意条款",
            "请勾选说明后再申请。",
        ),
        4401 => (
            UserNoticeKind::Info,
            "请求过于频繁",
            "换票或接口请求过频，请稍后再试。",
        ),
        4404 => (
            UserNoticeKind::Info,
            "暂无数据",
            "上游暂无 latest 数据。",
        ),
        4405 => (
            UserNoticeKind::Info,
            "请求方法不正确",
            "换票接口请使用 GET 或 POST。",
        ),
        4399 => (
            UserNoticeKind::Info,
            "服务器错误",
            "鉴权服务暂时异常，请稍后再试。",
        ),
        _ if !e.is_empty() => (
            UserNoticeKind::Auth,
            "鉴权或连接失败",
            "服务器返回错误，请检查令牌与网络后重试。",
        ),
        _ => (
            UserNoticeKind::Info,
            "服务器提示",
            "收到服务器错误响应。",
        ),
    };
    let key = if code > 0 {
        format!("code_{code}")
    } else if !e.is_empty() {
        e.to_string()
    } else {
        "server_error".into()
    };
    (kind, title, body, key)
}

pub fn is_conn_limit_text(text: &str) -> bool {
    text.contains("连接限制")
        || text.contains("conn_limit")
        || text.contains("connection_limit")
        || text.contains("超过服务器连接")
        || text.contains("max_connections")
        || text.contains("\"code\":4005")
        || text.contains("\"code\": 4005")
}

pub fn is_ban_text(text: &str) -> bool {
    text.contains("account_banned")
        || text.contains("\"banned\"")
        || text.contains("已被封禁")
        || text.contains("\"code\":4006")
        || text.contains("\"code\": 4006")
        || text.contains("http_denied")
        || text.contains("\"code\":4403")
        || text.contains("\"code\": 4403")
        || text.contains("自动封禁")
        || text.contains("黑名单")
}

pub fn is_auth_error_text(text: &str) -> bool {
    text.contains("expired_access_token")
        || text.contains("invalid_api_key")
        || text.contains("missing_api_key")
        || text.contains("invalid_token_format")
        || text.contains("invalid_refresh_token")
        || text.contains("expired_refresh_token")
        || text.contains("\"code\":4001")
        || text.contains("\"code\":4002")
        || text.contains("\"code\":4003")
        || text.contains("\"code\":4004")
        || text.contains("\"code\":4201")
        || text.contains("\"code\":4202")
        || text.contains("\"code\": 4001")
        || text.contains("\"code\": 4002")
        || text.contains("\"code\": 4003")
        || text.contains("\"code\": 4004")
        || text.contains("\"code\": 4201")
        || text.contains("\"code\": 4202")
}

#[cfg(test)]
mod tests {
    use super::*;
    use starquake_core::UserNoticeKind;

    #[test]
    fn conn_limit_and_ban_titles() {
        let lim = notice_from_server(4005, "conn_limit", "您已超过服务器连接限制");
        assert_eq!(lim.kind, UserNoticeKind::ConnLimit);
        assert_eq!(lim.dedupe_key, "conn_limit");
        assert!(lim.title.contains("连接超限"));

        let ban = notice_from_server(4006, "account_banned", "账号已封禁");
        assert_eq!(ban.kind, UserNoticeKind::Banned);
        assert_eq!(ban.dedupe_key, "account_banned");

        let ip = notice_from_server(4403, "http_denied", "拒绝访问");
        assert_eq!(ip.kind, UserNoticeKind::Banned);
        assert_eq!(ip.dedupe_key, "ip_banned");
    }

    #[test]
    fn auth_codes_map() {
        for code in [4001, 4002, 4003, 4004, 4102, 4202] {
            let n = notice_from_server(code, "", "");
            assert_eq!(n.kind, UserNoticeKind::Auth, "{code}");
            assert!(n.body.contains(&code.to_string()));
        }
    }

    #[test]
    fn parse_error_json_and_plain_ban_text() {
        let (code, err, msg) = parse_server_error(
            r#"{"ok":false,"error":"expired_access_token","code":4004,"message":"访问令牌过期"}"#,
        )
        .expect("json");
        assert_eq!(code, 4004);
        assert_eq!(err, "expired_access_token");
        assert!(msg.contains("过期"));
        assert!(is_conn_limit_text(r#"{"type":"error","message":"您已超过服务器连接限制"}"#));
        assert!(is_ban_text(r#"{"type":"error","message":"您的IP已被封禁，请联系管理员进行处理"}"#));
        assert!(is_auth_error_text(r#"{"type":"error","code":4004,"error":"expired_access_token"}"#));
    }
}
