//! 日志脱敏：令牌 / 查询参数 / Bearer，避免写入 log.txt。

/// 对任意日志文本做敏感信息脱敏（可安全用于 URL、错误串、整行）。
pub fn redact_secrets(input: &str) -> String {
    let mut s = redact_query_values(
        input,
        &[
            "key",
            "token",
            "access_token",
            "refresh_token",
            "api_key",
            "apikey",
            "password",
            "secret",
            "authorization",
        ],
    );
    s = redact_bearer(&s);
    s = redact_prefixed_tokens(&s);
    s
}

/// 日志用：去掉 URL 中的密钥查询参数（兼容旧调用名）。
pub fn redact_url(url: &str) -> String {
    redact_secrets(url)
}

fn redact_query_values(input: &str, names: &[&str]) -> String {
    let mut out = input.to_string();
    for name in names {
        out = redact_named_values(&out, name);
    }
    out
}

/// 将 `name=VALUE`（可重复）中的 VALUE 替换为 `***`（VALUE 止于 `&` `#` 空白 `"'`）。
fn redact_named_values(input: &str, name: &str) -> String {
    let needle = format!("{name}=");
    let lower_needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while !rest.is_empty() {
        let lower_rest = rest.to_ascii_lowercase();
        let Some(rel) = lower_rest.find(&lower_needle) else {
            out.push_str(rest);
            break;
        };
        let val_start = rel + needle.len();
        out.push_str(&rest[..val_start]);
        let after = &rest[val_start..];
        let val_len = after
            .find(['&', '#', '?', '"', '\'', ' ', '\t', '\r', '\n'])
            .unwrap_or(after.len());
        out.push_str("***");
        rest = &after[val_len..];
    }
    out
}

fn redact_bearer(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(rel) = lower.find("bearer ") else {
            out.push_str(rest);
            break;
        };
        let start = rel;
        let val_start = start + "bearer ".len();
        out.push_str(&rest[..val_start]);
        let after = &rest[val_start..];
        let val_len = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';'))
            .unwrap_or(after.len());
        if val_len == 0 {
            out.push_str(&rest[val_start..val_start + 1.min(after.len())]);
            rest = &rest[val_start + 1.min(after.len())..];
            continue;
        }
        out.push_str("***");
        rest = &after[val_len..];
    }
    out
}

/// `at_` / `rt_` / `lk_` + 足够长的令牌体 → 保留前缀 + `***`。
fn redact_prefixed_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(prefix_len) = token_prefix_at(bytes, i) {
            let body_start = i + prefix_len;
            let body_len = token_body_len(&bytes[body_start..]);
            // 过短不像正式票（避免误伤普通英文）
            if body_len >= 8 {
                out.push_str(&input[i..body_start]);
                out.push_str("***");
                i = body_start + body_len;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn token_prefix_at(bytes: &[u8], i: usize) -> Option<usize> {
    // 前一字符若为字母数字或 `_`，不视为令牌起点（避免 `what_at_xxx`）
    if i > 0 {
        let prev = bytes[i - 1];
        if prev.is_ascii_alphanumeric() || prev == b'_' {
            return None;
        }
    }
    let rest = &bytes[i..];
    for (p, len) in [(b"at_" as &[u8], 3), (b"rt_", 3), (b"lk_", 3)] {
        if rest.len() >= len && rest[..len].eq_ignore_ascii_case(p) {
            return Some(len);
        }
    }
    None
}

fn token_body_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_key_query() {
        assert_eq!(
            redact_url("wss://api.example/all?key=at_secret_token_xx"),
            "wss://api.example/all?key=***"
        );
        assert_eq!(
            redact_url("wss://api.example/all?foo=1&key=at_secret_token_xx&bar=2"),
            "wss://api.example/all?foo=1&key=***&bar=2"
        );
    }

    #[test]
    fn redacts_bare_tokens_and_bearer() {
        let s = redact_secrets(
            "got at_Ueae-w95ShWWSfT-rO5Ql2J7aaoAKJ2JucxNh2T3aNE and Bearer rt_abcdefghijklmnop",
        );
        assert!(!s.contains("Ueae"));
        assert!(!s.contains("abcdefghijklmnop"));
        assert!(s.contains("at_***"));
        assert!(s.contains("Bearer ***"));
        let s2 = redact_secrets("refresh=rt_abcdefghijklmnopXX");
        assert!(s2.contains("rt_***"));
    }

    #[test]
    fn keeps_short_or_plain_text() {
        assert_eq!(redact_secrets("at_short"), "at_short");
        assert_eq!(redact_secrets("hello world"), "hello world");
        assert_eq!(redact_url("wss://api.example/kmoni"), "wss://api.example/kmoni");
    }
}
