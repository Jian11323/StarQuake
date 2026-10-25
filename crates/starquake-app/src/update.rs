//! 从静态更新清单检查 / 下载 / 应用更新。
//!
//! 并行拉取更新清单；成功结果中取**版本号最新**的一份（同版本优先 Gitee）。
//! 镜像优先级固定为：**Gitee → GitHub → 自建服务器**（服务器仅兜底）。
//! 更新包按该优先级顺序下载，单地址失败可重试；校验 Content-Length 与 SHA256。

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;
use tracing::{info, warn};

const UA: &str = concat!(
    "StarQuake/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Jian11323/StarQuake; +https://gitee.com/jian0786/starquake)"
);

/// 清单地址优先级：Gitee → GitHub → 自建服务器（并行拉取；取最新版，同版本偏 Gitee）。
const MANIFEST_URLS: &[&str] = &[
    // 1) Gitee
    "https://gitee.com/jian0786/starquake/raw/master/update/manifest.json",
    "https://gitee.com/jian0786/starquake/raw/main/update/manifest.json",
    // 2) GitHub
    "https://raw.githubusercontent.com/Jian11323/StarQuake/main/update/manifest.json",
    "https://raw.githubusercontent.com/Jian11323/StarQuake/master/update/manifest.json",
    // 3) 自建服务器（兜底；有稳定出站后再取消注释）
    // "https://api.sismotide.top/starquake/update/manifest.json",
];

const GITEE_REPO_PREFIX: &str = "https://gitee.com/jian0786/starquake/";
const GITHUB_REPO_PREFIX: &str = "https://github.com/Jian11323/StarQuake/";
/// 自建服务器 Release 前缀（与 Gitee/GitHub 路径后缀对齐时可推导）。
const SERVER_REPO_PREFIX: &str = "https://api.sismotide.top/starquake/";
/// 服务器附件镜像未稳定前勿自动推导，避免每次更新空打 404。
const SERVER_ASSET_MIRROR_ENABLED: bool = false;

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

/// 自动化端到端：跳过弹窗确认、忽略「稍后」记忆，便于本地压测更新链路。
fn update_e2e() -> bool {
    env_truthy("STARQUAKE_UPDATE_E2E")
}

/// E2E：强制走安装包通道（不依赖真实安装路径）。
fn update_e2e_prefer_installer() -> bool {
    update_e2e() && env_truthy("STARQUAKE_UPDATE_PREFER_INSTALLER")
}

/// E2E：强制走便携 zip 通道。
fn update_e2e_prefer_portable() -> bool {
    update_e2e() && env_truthy("STARQUAKE_UPDATE_PREFER_PORTABLE")
}

/// E2E：setup `/S /D=` 目标目录（避免写进真实 Program Files；仅 E2E 生效）。
fn update_e2e_install_dir() -> Option<String> {
    if !update_e2e() {
        return None;
    }
    std::env::var("STARQUAKE_UPDATE_INSTALL_DIR")
        .ok()
        .map(|s| s.trim().trim_end_matches(['\\', '/']).to_string())
        .filter(|s| !s.is_empty())
}

/// 清单 URL 列表：可用 `STARQUAKE_UPDATE_MANIFEST_URL` 覆盖（`;` / 换行分隔多地址）。
fn manifest_urls() -> Vec<String> {
    if let Ok(raw) = std::env::var("STARQUAKE_UPDATE_MANIFEST_URL") {
        let urls: Vec<String> = raw
            .split([';', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !urls.is_empty() {
            return urls;
        }
    }
    MANIFEST_URLS.iter().map(|s| (*s).to_string()).collect()
}

#[derive(Debug, Clone)]
pub struct AvailableUpdate {
    pub version: String,
    pub name: String,
    pub body: String,
    pub html_url: String,
    pub asset_url: String,
    pub asset_name: String,
    /// 空则跳过校验
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
struct UpdateManifest {
    latest_version: String,
    #[serde(default)]
    min_supported_version: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    #[cfg_attr(not(windows), allow(dead_code))]
    installer: Option<ManifestAsset>,
    #[serde(default)]
    #[cfg_attr(not(windows), allow(dead_code))]
    zip: Option<ManifestAsset>,
    #[serde(default)]
    #[cfg_attr(windows, allow(dead_code))]
    linux: Option<ManifestAsset>,
    /// 更新日志正文（弹窗展示）
    #[serde(default)]
    notes: String,
    #[serde(default)]
    release_notes_url: String,
    #[serde(default)]
    html_url: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ManifestAsset {
    url: String,
    #[serde(default)]
    sha256: String,
}

/// 解析 `v2.0.1` / `2.0.1` / `StarQuake-2.0.1` → (major, minor, patch)。
pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let s = raw.trim().trim_start_matches('v').trim_start_matches('V');
    // 预发布后缀截断：`2.3.2-beta` / `2.3.2-beta.1` → `2.3.2`（勿用 rsplit 误吃成 `1`）
    let s = s.split(['-', '+', ' ']).next().unwrap_or(s);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts
        .next()
        .unwrap_or("0")
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    match (parse_semver(remote), parse_semver(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

fn http_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()?)
}

fn try_fetch_manifest_one(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<UpdateManifest> {
    // 绕过 CDN/代理对 raw 文件的陈旧缓存（Gitee main 曾长期停在旧版）。
    let bust = format!(
        "{}{}_={}",
        url,
        if url.contains('?') { '&' } else { '?' },
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let resp = client
        .get(&bust)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .header(reqwest::header::PRAGMA, "no-cache")
        .send()
        .with_context(|| format!("请求 {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let snippet: String = body.chars().take(160).collect();
        bail!("HTTP {status} {snippet}");
    }
    // 兼容带 BOM 的 UTF-8
    let bytes = resp.bytes().context("读取清单正文")?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    serde_json::from_str::<UpdateManifest>(text).context("解析 JSON")
}

fn mirror_rank(url: &str) -> u8 {
    let u = url.to_ascii_lowercase();
    if u.contains("gitee.com") {
        0
    } else if u.contains("github.com") || u.contains("githubusercontent.com") {
        1
    } else {
        2 // 自建服务器及其它
    }
}

/// 选最新 `latest_version`；同版本优先 Gitee，再 GitHub，服务器最后。
fn pick_newest_manifest(cands: &[(String, UpdateManifest)]) -> Option<(String, UpdateManifest)> {
    let mut best: Option<(String, UpdateManifest)> = None;
    for (url, m) in cands {
        let take = match &best {
            None => true,
            Some((best_url, cur)) => {
                if is_newer(&m.latest_version, &cur.latest_version) {
                    true
                } else if is_newer(&cur.latest_version, &m.latest_version) {
                    false
                } else {
                    mirror_rank(url) < mirror_rank(best_url)
                }
            }
        };
        if take {
            best = Some((url.clone(), m.clone()));
        }
    }
    best
}

fn fetch_manifest(client: &reqwest::blocking::Client) -> Result<UpdateManifest> {
    let urls = manifest_urls();
    let (tx, rx) = mpsc::channel();
    let n = urls.len();
    for url in urls {
        let tx = tx.clone();
        let client = client.clone();
        std::thread::spawn(move || {
            let result = try_fetch_manifest_one(&client, &url);
            let _ = tx.send((url, result));
        });
    }
    drop(tx);

    let mut errors = Vec::new();
    let mut ok: Vec<(String, UpdateManifest)> = Vec::new();
    let mut remaining = n;
    // 等齐全部镜像，再取最新版——禁止「先到先得」吃到过期分支。
    while remaining > 0 {
        match rx.recv() {
            Ok((url, Ok(m))) => {
                info!(url = %url, ver = %m.latest_version, "更新清单候选");
                ok.push((url, m));
                remaining -= 1;
            }
            Ok((url, Err(e))) => {
                errors.push(format!("{url} → {e:#}"));
                remaining -= 1;
            }
            Err(_) => break,
        }
    }
    if let Some((url, m)) = pick_newest_manifest(&ok) {
        if ok.len() > 1 {
            let vers: Vec<_> = ok
                .iter()
                .map(|(u, x)| format!("{u}={}", x.latest_version))
                .collect();
            info!(
                chosen = %url,
                ver = %m.latest_version,
                candidates = %vers.join(" | "),
                "更新清单已选用最新版"
            );
        } else {
            info!(url = %url, ver = %m.latest_version, "更新清单已加载");
        }
        return Ok(m);
    }
    bail!(
        "无法获取更新清单：{}",
        if errors.is_empty() {
            "无可用镜像".into()
        } else {
            errors.join("；")
        }
    )
}

/// Windows 更新通道：安装包版与便携版互斥，禁止串路。
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WinUpdateChannel {
    Installer,
    Portable,
}

/// 根据运行路径与 E2E 开关判定 Windows 更新通道。
#[cfg(windows)]
fn windows_update_channel() -> WinUpdateChannel {
    if update_e2e_prefer_installer() {
        return WinUpdateChannel::Installer;
    }
    if update_e2e_prefer_portable() {
        return WinUpdateChannel::Portable;
    }
    if looks_like_windows_installed_path() {
        WinUpdateChannel::Installer
    } else {
        WinUpdateChannel::Portable
    }
}

/// 路径启发式：NSIS 安装目录 / Program Files / `%LocalAppData%\Programs\…`。
#[cfg(windows)]
fn looks_like_windows_installed_path() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    if let Some(dir) = exe.parent() {
        if dir.join("Uninstall.exe").is_file() || dir.join("uninstall.exe").is_file() {
            return true;
        }
    }
    let path = exe.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    if path.contains("\\program files\\") || path.contains("\\program files (x86)\\") {
        return true;
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let prefix = PathBuf::from(local)
            .join("Programs")
            .to_string_lossy()
            .to_ascii_lowercase()
            .replace('/', "\\");
        let prefix = prefix.trim_end_matches('\\').to_string();
        if !prefix.is_empty() && path.starts_with(&prefix) {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn pick_platform_asset_for(
    m: &UpdateManifest,
    channel: WinUpdateChannel,
) -> Result<(ManifestAsset, String)> {
    match channel {
        WinUpdateChannel::Installer => {
            let Some(a) = m.installer.as_ref().filter(|a| !a.url.trim().is_empty()) else {
                bail!("当前为安装包版，但更新清单缺少 installer URL");
            };
            let name = url_file_name(&a.url).unwrap_or_else(|| "StarQuake_setup.exe".into());
            info!(asset = %name, "更新通道：Windows 安装包");
            Ok((a.clone(), name))
        }
        WinUpdateChannel::Portable => {
            let Some(a) = m.zip.as_ref().filter(|a| !a.url.trim().is_empty()) else {
                bail!("当前为便携版，但更新清单缺少 zip URL");
            };
            let name = url_file_name(&a.url).unwrap_or_else(|| "StarQuake_portable.zip".into());
            info!(asset = %name, "更新通道：Windows 便携包");
            Ok((a.clone(), name))
        }
    }
}

fn pick_platform_asset(m: &UpdateManifest) -> Result<(ManifestAsset, String)> {
    #[cfg(windows)]
    {
        pick_platform_asset_for(m, windows_update_channel())
    }
    #[cfg(not(windows))]
    {
        let Some(l) = m.linux.as_ref().filter(|a| !a.url.trim().is_empty()) else {
            bail!("更新清单缺少 Linux 包 URL");
        };
        let name = url_file_name(&l.url).unwrap_or_else(|| "StarQuake_linux.tar.gz".into());
        info!(asset = %name, "更新通道：Linux");
        Ok((l.clone(), name))
    }
}

fn url_file_name(url: &str) -> Option<String> {
    let path = url.split('?').next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// 由清单主 URL 推导各镜像，固定顺序：**Gitee → GitHub → 自建服务器**。
fn asset_mirror_urls(primary: &str) -> Vec<String> {
    let primary = primary.trim();
    let mut gitee: Option<String> = None;
    let mut github: Option<String> = None;
    let mut server: Option<String> = None;

    let classify = |u: &str| -> &'static str {
        let lower = u.to_ascii_lowercase();
        if lower.contains("gitee.com") {
            "gitee"
        } else if lower.contains("github.com") {
            "github"
        } else if lower.starts_with(&SERVER_REPO_PREFIX.to_ascii_lowercase())
            || lower.contains("sismotide.top/starquake")
        {
            "server"
        } else {
            "other"
        }
    };

    match classify(primary) {
        "gitee" => gitee = Some(primary.to_string()),
        "github" => github = Some(primary.to_string()),
        "server" => server = Some(primary.to_string()),
        _ => {}
    }

    if let Some(rest) = primary.strip_prefix(GITEE_REPO_PREFIX) {
        github.get_or_insert_with(|| format!("{GITHUB_REPO_PREFIX}{rest}"));
        if SERVER_ASSET_MIRROR_ENABLED {
            server.get_or_insert_with(|| format!("{SERVER_REPO_PREFIX}{rest}"));
        }
    }
    if let Some(rest) = primary.strip_prefix(GITHUB_REPO_PREFIX) {
        gitee.get_or_insert_with(|| format!("{GITEE_REPO_PREFIX}{rest}"));
        if SERVER_ASSET_MIRROR_ENABLED {
            server.get_or_insert_with(|| format!("{SERVER_REPO_PREFIX}{rest}"));
        }
    }
    if let Some(rest) = primary.strip_prefix(SERVER_REPO_PREFIX) {
        gitee.get_or_insert_with(|| format!("{GITEE_REPO_PREFIX}{rest}"));
        github.get_or_insert_with(|| format!("{GITHUB_REPO_PREFIX}{rest}"));
    }

    let mut out = Vec::new();
    let mut push_opt = |u: Option<String>| {
        if let Some(u) = u {
            if !u.trim().is_empty() && !out.iter().any(|x| x == &u) {
                out.push(u);
            }
        }
    };
    push_opt(gitee);
    push_opt(github);
    push_opt(server);
    if out.is_empty() && !primary.is_empty() {
        out.push(primary.to_string());
    } else if !primary.is_empty()
        && classify(primary) == "other"
        && !out.iter().any(|x| x == primary)
    {
        out.push(primary.to_string());
    }
    out
}

/// 查询最新版；若无更新返回 `Ok(None)`。
pub fn check_latest(local_version: &str) -> Result<Option<AvailableUpdate>> {
    let client = http_client()?;
    let m = fetch_manifest(&client)?;
    let ver = m
        .latest_version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .to_string();
    if ver.is_empty() {
        bail!("更新清单缺少 latest_version");
    }
    if !is_newer(&ver, local_version) {
        info!(
            local = local_version,
            remote = %ver,
            channel = %m.channel,
            "已是最新版"
        );
        let _ = m.min_supported_version;
        return Ok(None);
    }
    let (asset, asset_name) = pick_platform_asset(&m)?;
    let html = if !m.html_url.trim().is_empty() {
        m.html_url
    } else if !m.release_notes_url.trim().is_empty() {
        m.release_notes_url.clone()
    } else {
        format!("https://gitee.com/jian0786/starquake/releases/tag/v{ver}")
    };
    let body = if !m.notes.trim().is_empty() {
        m.notes.trim().to_string()
    } else if !m.release_notes_url.trim().is_empty() {
        format!("发行说明：{}", m.release_notes_url.trim())
    } else {
        String::new()
    };
    Ok(Some(AvailableUpdate {
        version: ver.clone(),
        name: format!("StarQuake {ver}"),
        body,
        html_url: html,
        asset_url: asset.url,
        asset_name,
        sha256: asset.sha256.trim().to_ascii_lowercase(),
    }))
}

/// 下载前重置暂存目录（应用阶段禁止再调用，以免删掉已解压内容）。
fn reset_staging() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("StarQuake_update");
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn sha256_hex(path: &Path) -> Result<String> {
    // 进程内计算，避免 certutil 输出带空格/本地化文案导致解析失败。
    let mut file = File::open(path).with_context(|| format!("打开 {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).context("读取更新包")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Download progress: (bytes_downloaded, content_length).
pub type DownloadProgressCb = dyn Fn(u64, Option<u64>) + Send;

fn download_one_url_once(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    expect_sha: &str,
    progress: Option<&DownloadProgressCb>,
) -> Result<()> {
    info!(url = %url, dest = %dest.display(), "下载更新包");
    let mut resp = client
        .get(url)
        .header(reqwest::header::CACHE_CONTROL, "no-cache")
        .send()
        .context("下载更新包")?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let expect_len = resp.content_length();
    let mut file = File::create(dest)?;
    let mut written: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf).context("读取更新包")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("写入更新包")?;
        written += n as u64;
        if let Some(cb) = progress {
            cb(written, expect_len);
        }
    }
    file.flush()?;
    drop(file);
    if let Some(n) = expect_len {
        if written != n {
            let _ = fs::remove_file(dest);
            bail!("下载不完整（期望 {n} 字节，实际 {written}）");
        }
    } else if written < 1024 {
        let _ = fs::remove_file(dest);
        bail!("下载异常过小（{written} 字节），可能不是更新包");
    }
    if !expect_sha.is_empty() {
        let got = sha256_hex(dest)?;
        if got != expect_sha {
            let _ = fs::remove_file(dest);
            bail!("校验失败（期望 {expect_sha}，实际 {got}）");
        }
        info!(url = %url, "更新包 SHA256 校验通过");
    }
    Ok(())
}

fn download_one_url(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    expect_sha: &str,
    progress: Option<&DownloadProgressCb>,
) -> Result<()> {
    const ATTEMPTS: u32 = 3;
    let mut last = None;
    for attempt in 1..=ATTEMPTS {
        if dest.exists() {
            let _ = fs::remove_file(dest);
        }
        match download_one_url_once(client, url, dest, expect_sha, progress) {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(
                    url = %url,
                    attempt,
                    err = %format!("{e:#}"),
                    "下载失败，将重试"
                );
                last = Some(e);
                if attempt < ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(400 * u64::from(attempt)));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("下载失败")))
}

/// 下载资源到暂存目录，返回本地文件路径。
/// 按主 URL → 镜像顺序尝试（每地址最多 3 次）；避免双镜像并行把弱网带宽打爆导致截断。
pub fn download_asset(info: &AvailableUpdate) -> Result<PathBuf> {
    download_asset_with_progress(info, None)
}

pub fn download_asset_with_progress(
    info: &AvailableUpdate,
    progress: Option<&DownloadProgressCb>,
) -> Result<PathBuf> {
    let dir = reset_staging()?;
    let dest = dir.join(&info.asset_name);
    let urls = asset_mirror_urls(&info.asset_url);
    if urls.is_empty() {
        bail!("更新包 URL 为空");
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent(UA)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()?;

    let mut errors = Vec::new();
    for (i, url) in urls.into_iter().enumerate() {
        let part = dir.join(format!("part_{i}_{}", info.asset_name));
        match download_one_url(&client, &url, &part, &info.sha256, progress) {
            Ok(()) => {
                if part != dest {
                    fs::rename(&part, &dest)
                        .or_else(|_| {
                            fs::copy(&part, &dest)?;
                            fs::remove_file(&part)?;
                            Ok::<(), io::Error>(())
                        })
                        .with_context(|| format!("落盘 {}", dest.display()))?;
                }
                info!(url = %url, dest = %dest.display(), "更新包下载完成");
                return Ok(dest);
            }
            Err(e) => {
                let _ = fs::remove_file(&part);
                errors.push(format!("{url} → {e:#}"));
            }
        }
    }
    bail!(
        "下载失败：{}",
        if errors.is_empty() {
            "无可用镜像".into()
        } else {
            errors.join("；")
        }
    )
}

/// 生成 setup.exe 静默安装 bat 正文（可单测）。
/// `e2e_install_dir` 非空时追加 NSIS `/D=`（须为最后参数、无引号），并优先从该目录拉起。
#[cfg(windows)]
fn installer_apply_script(setup: &str, e2e_install_dir: Option<&str>) -> String {
    let mut body = String::from(
        "@echo off\r\n\
chcp 65001 >nul\r\n\
title StarQuake 更新\r\n\
echo.\r\n\
echo   Updating StarQuake, please wait...\r\n\
echo.\r\n\
timeout /t 2 /nobreak >nul\r\n",
    );
    body.push_str(&format!("if not exist \"{setup}\" goto :eof\r\n"));
    if let Some(dir) = e2e_install_dir {
        // NSIS：/D= 必须最后且路径勿加引号
        body.push_str(&format!("\"{setup}\" /S /D={dir}\r\n"));
        body.push_str(&format!(
            "if exist \"{dir}\\starquake.exe\" (start \"\" \"{dir}\\starquake.exe\" & goto :done)\r\n"
        ));
    } else {
        body.push_str(&format!("\"{setup}\"\r\n"));
    }
    body.push_str(
        "set \"L1=%ProgramFiles(x86)%\\StarQuake\\starquake.exe\"\r\n\
set \"L2=%ProgramFiles%\\StarQuake\\starquake.exe\"\r\n\
set \"L3=%LocalAppData%\\Programs\\StarQuake\\starquake.exe\"\r\n\
if exist \"%L1%\" (start \"\" \"%L1%\" & goto :done)\r\n\
if exist \"%L2%\" (start \"\" \"%L2%\" & goto :done)\r\n\
if exist \"%L3%\" (start \"\" \"%L3%\" & goto :done)\r\n\
:done\r\n\
del \"%~f0\"\r\n",
    );
    body
}

/// 应用已下载的更新包并重启（Windows 便携 zip / setup.exe；Linux tar.gz）。
pub fn apply_and_relaunch(package: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("current_exe")?;
    let app_dir = exe
        .parent()
        .map(Path::to_path_buf)
        .context("exe parent")?;
    let name = package
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    #[cfg(windows)]
    if name.ends_with(".exe") {
        // 勿再调用会清空暂存的逻辑；bat 与 setup 同目录，先退出本进程再静默安装
        let stage = package
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::temp_dir().join("StarQuake_update"));
        let bat = stage.join("apply_installer.cmd");
        let setup = package.display().to_string();
        let body = installer_apply_script(&setup, update_e2e_install_dir().as_deref());
        fs::write(&bat, body)?;
        Command::new("cmd")
            .args(["/C", &bat.to_string_lossy()])
            .spawn()
            .context("启动安装脚本")?;
        std::process::exit(0);
    }

    let extract_dir = package
        .parent()
        .unwrap_or(Path::new("."))
        .join("extracted");
    if extract_dir.exists() {
        let _ = fs::remove_dir_all(&extract_dir);
    }
    fs::create_dir_all(&extract_dir)?;

    if name.ends_with(".zip") {
        extract_zip(package, &extract_dir)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(package, &extract_dir)?;
    } else {
        bail!("不支持的更新包格式: {name}");
    }

    let payload = find_payload_root(&extract_dir)?;
    #[cfg(windows)]
    {
        // 必须与 package 同级写 bat，禁止 reset 暂存（否则会删掉刚解压的 payload）
        let bat = package
            .parent()
            .unwrap_or(Path::new("."))
            .join("apply_update.cmd");
        let exe_name = exe
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("starquake.exe");
        let body = format!(
            "@echo off\r\n\
chcp 65001 >nul\r\n\
title StarQuake 更新\r\n\
echo.\r\n\
echo   Updating StarQuake, please wait...\r\n\
echo.\r\n\
timeout /t 2 /nobreak >nul\r\n\
xcopy /E /Y /I /Q \"{src}\\*\" \"{dst}\\\"\r\n\
if exist \"{dst}\\{exe}\" start \"\" \"{dst}\\{exe}\"\r\n\
del \"%~f0\"\r\n",
            src = payload.display(),
            dst = app_dir.display(),
            exe = exe_name,
        );
        fs::write(&bat, body)?;
        Command::new("cmd")
            .args(["/C", &bat.to_string_lossy()])
            .spawn()
            .context("启动更新脚本")?;
        std::process::exit(0);
    }
    #[cfg(not(windows))]
    {
        let _ = package;
        copy_dir_recursive(&payload, &app_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin = app_dir.join("starquake");
            if bin.is_file() {
                let mut perms = fs::metadata(&bin)?.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&bin, perms);
            }
        }
        Command::new(&exe).spawn().context("重启应用")?;
        std::process::exit(0);
    }
}

fn find_payload_root(extract_dir: &Path) -> Result<PathBuf> {
    for name in ["StarQuake", "EEWView", "starquake", "eewview"] {
        let p = extract_dir.join(name);
        if p.is_dir() {
            return Ok(p);
        }
    }
    let mut dirs = fs::read_dir(extract_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect::<Vec<_>>();
    if dirs.len() == 1 {
        return Ok(dirs.remove(0));
    }
    Ok(extract_dir.to_path_buf())
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).context("打开 zip")?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let out = match entry.enclosed_name() {
            Some(p) => dest.join(p),
            None => continue,
        };
        if entry.name().ends_with('/') {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut outfile = File::create(&out)?;
        io::copy(&mut entry, &mut outfile)?;
    }
    Ok(())
}

fn extract_tar_gz(path: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("tar")
        .args([
            "-xzf",
            &path.to_string_lossy(),
            "-C",
            &dest.to_string_lossy(),
        ])
        .status()
        .context("执行 tar")?;
    if !status.success() {
        bail!("tar 解压失败");
    }
    Ok(())
}

#[cfg(not(windows))]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for ent in fs::read_dir(src)? {
        let ent = ent?;
        let from = ent.path();
        let to = dst.join(ent.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub fn open_release_page(url: &str) {
    #[cfg(windows)]
    {
        if let Err(e) = Command::new("cmd").args(["/C", "start", "", url]).spawn() {
            warn!("无法打开发布页: {e}");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = Command::new("open").arg(url).spawn() {
            warn!("无法打开发布页: {e}");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Err(e) = Command::new("xdg-open").arg(url).spawn() {
            warn!("无法打开发布页: {e}");
        }
    }
}

/// 启动主窗口前的检查结果（安装成功时通常已 `process::exit`，不会回到调用方）。
#[derive(Debug)]
pub enum StartupUpdateOutcome {
    /// 继续进主界面；`checked=true` 表示本轮已查过，主界面内勿再自动查
    Continue { checked: bool },
    /// 用户已确认安装：进入主界面后显示进度并下载（勿在 MessageBox 后静默下载）
    PendingInstall(AvailableUpdate),
    /// 已触发安装并应退出（兜底；正常路径在 `apply_and_relaunch` 内 exit）
    Exit,
}

/// 启动主窗口前检查更新：复用 `check_latest` / `download_asset` / `apply_and_relaunch`。
///
/// 流程：有新版本则原生对话框询问；确认后进入主界面显示下载进度再安装（E2E 仍可启动前静默）。
/// `cfg.last_dismissed_update_version` 可能被改写；调用方应在 Continue 时按需 `save_user_overlay`。
pub fn run_startup_auto_update(
    cfg: &mut starquake_config::UpdateConfig,
    local_version: &str,
) -> StartupUpdateOutcome {
    if !cfg.check_on_startup && !update_e2e() {
        return StartupUpdateOutcome::Continue { checked: false };
    }
    if update_e2e() && !cfg.check_on_startup {
        info!("STARQUAKE_UPDATE_E2E：强制开启启动检查更新");
    }

    // 非 Windows 无 MessageBox 确认：留给主界面内弹窗
    #[cfg(not(windows))]
    {
        let _ = local_version;
        return StartupUpdateOutcome::Continue { checked: false };
    }

    #[cfg(windows)]
    {
        let avail = match check_latest(local_version) {
            Ok(Some(info)) => info,
            Ok(None) => return StartupUpdateOutcome::Continue { checked: true },
            Err(e) => {
                warn!("启动前检查更新失败（已跳过）: {e:#}");
                return StartupUpdateOutcome::Continue { checked: true };
            }
        };

        let dismissed = cfg.last_dismissed_update_version.trim();
        if !update_e2e()
            && !dismissed.is_empty()
            && dismissed == avail.version.trim()
        {
            info!(
                ver = %avail.version,
                "启动前检查更新：已跳过该版本，不再询问"
            );
            return StartupUpdateOutcome::Continue { checked: true };
        }

        let body = avail.body.trim();
        let ask = if body.is_empty() {
            format!(
                "发现新版本 v{}（当前 v{}）。\n是否下载并安装？\n\n安装完成后将自动启动新版本。",
                avail.version, local_version
            )
        } else {
            let notes = if body.chars().count() > 400 {
                let t: String = body.chars().take(400).collect();
                format!("{t}…")
            } else {
                body.to_string()
            };
            format!(
                "发现新版本 v{}（当前 v{}）。\n是否下载并安装？\n\n安装完成后将自动启动新版本。\n\n{}",
                avail.version, local_version, notes
            )
        };

        if !update_e2e() && !native_yes_no("发现新版本", &ask) {
            cfg.last_dismissed_update_version = avail.version.clone();
            info!(ver = %avail.version, "启动前检查更新：用户暂不更新");
            return StartupUpdateOutcome::Continue { checked: true };
        }
        if update_e2e() {
            info!(ver = %avail.version, "STARQUAKE_UPDATE_E2E：自动确认安装");
        }

        if !update_e2e() && !cfg.auto_install {
            open_release_page(&avail.html_url);
            return StartupUpdateOutcome::Continue { checked: true };
        }

        // E2E：仍在启动前静默下载安装；正式环境进主界面显示进度条
        if update_e2e() {
            info!(ver = %avail.version, "启动前检查更新：E2E 静默下载");
            match download_asset(&avail) {
                Ok(path) => {
                    if let Err(e) = apply_and_relaunch(&path) {
                        warn!("启动前应用更新失败: {e:#}");
                        native_info(
                            "更新失败",
                            &format!("应用更新失败：{e:#}\n将继续启动当前版本。"),
                        );
                        StartupUpdateOutcome::Continue { checked: true }
                    } else {
                        StartupUpdateOutcome::Exit
                    }
                }
                Err(e) => {
                    warn!("启动前下载更新失败: {e:#}");
                    native_info(
                        "更新失败",
                        &format!("下载失败：{e:#}\n将继续启动当前版本。"),
                    );
                    StartupUpdateOutcome::Continue { checked: true }
                }
            }
        } else {
            info!(ver = %avail.version, "启动前检查更新：交由主界面下载并显示进度");
            StartupUpdateOutcome::PendingInstall(avail)
        }
    }
}

#[cfg(windows)]
fn native_yes_no(title: &str, message: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONQUESTION, MB_YESNO, IDYES,
    };
    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    let text = wide(message);
    let caption = wide(title);
    let r = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    r == IDYES
}

#[cfg(windows)]
fn native_info(title: &str, message: &str) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONINFORMATION, MB_OK,
    };
    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    let text = wide(message);
    let caption = wide(title);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn semver_newer() {
        assert!(is_newer("2.0.1", "2.0.0"));
        assert!(is_newer("v2.1.0", "2.0.9"));
        assert!(!is_newer("2.0.0", "2.0.0"));
        assert!(!is_newer("1.9.9", "2.0.0"));
        assert_eq!(parse_semver("2.3.2-beta"), Some((2, 3, 2)));
        assert_eq!(parse_semver("2.3.2-beta.1"), Some((2, 3, 2)));
        assert!(is_newer("2.3.2-beta", "2.3.1"));
        assert!(is_newer("2.3.3-beta", "2.3.2-beta"));
    }

    #[test]
    fn parse_manifest_shape() {
        let raw = r#"{
          "latest_version": "2.0.1",
          "notes": "hello",
          "zip": { "url": "https://example.com/a.zip", "sha256": "" },
          "installer": { "url": "https://example.com/a.exe", "sha256": "" }
        }"#;
        let m: UpdateManifest = serde_json::from_str(raw).unwrap();
        assert_eq!(m.latest_version, "2.0.1");
        assert_eq!(m.notes, "hello");
        assert!(m.zip.unwrap().url.contains("a.zip"));
    }

    #[test]
    fn mirror_urls_gitee_first_then_github() {
        let gitee = "https://gitee.com/jian0786/starquake/releases/download/v2.2.2/a.zip";
        let urls = asset_mirror_urls(gitee);
        assert!(urls.len() >= 2, "{urls:?}");
        assert!(urls[0].contains("gitee.com/jian0786"), "{urls:?}");
        assert!(urls[1].contains("github.com/Jian11323"), "{urls:?}");

        let github = "https://github.com/Jian11323/StarQuake/releases/download/v2.2.2/a.zip";
        let urls2 = asset_mirror_urls(github);
        assert!(urls2.len() >= 2, "{urls2:?}");
        assert!(urls2[0].contains("gitee.com/jian0786"), "{urls2:?}");
        assert!(urls2[1].contains("github.com/Jian11323"), "{urls2:?}");
    }

    #[test]
    fn pick_newest_same_version_prefers_gitee() {
        let gh = UpdateManifest {
            latest_version: "2.3.1".into(),
            min_supported_version: String::new(),
            channel: "stable".into(),
            installer: None,
            zip: None,
            linux: None,
            notes: "from-github".into(),
            release_notes_url: String::new(),
            html_url: String::new(),
        };
        let mut gt = gh.clone();
        gt.notes = "from-gitee".into();
        let cands = vec![
            (
                "https://raw.githubusercontent.com/Jian11323/StarQuake/main/update/manifest.json"
                    .into(),
                gh,
            ),
            (
                "https://gitee.com/jian0786/starquake/raw/master/update/manifest.json".into(),
                gt,
            ),
        ];
        let (url, m) = pick_newest_manifest(&cands).unwrap();
        assert!(url.contains("gitee.com"), "{url}");
        assert_eq!(m.notes, "from-gitee");
    }

    #[test]
    fn reset_staging_then_keep_payload_paths() {
        // 回归：apply 阶段不得再 wipe 暂存，否则 bat 指向的解压目录会消失
        let dir = reset_staging().unwrap();
        let marker = dir.join("keep_me.txt");
        fs::write(&marker, b"ok").unwrap();
        assert!(marker.is_file());
        // 二次 create_dir_all 式「仅确保存在」不得删文件
        fs::create_dir_all(&dir).unwrap();
        assert!(marker.is_file(), "apply 写 bat 时不得清空暂存");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_urls_env_override() {
        // SAFETY: 单测串行；仅本测读写该变量
        std::env::set_var(
            "STARQUAKE_UPDATE_MANIFEST_URL",
            "http://127.0.0.1:9/manifest.json",
        );
        let urls = manifest_urls();
        std::env::remove_var("STARQUAKE_UPDATE_MANIFEST_URL");
        assert_eq!(urls, vec!["http://127.0.0.1:9/manifest.json".to_string()]);
    }

    #[test]
    fn zip_apply_bat_lives_beside_package_not_wiping_staging() {
        let stage = reset_staging().unwrap();
        let package = stage.join("StarQuake_portable.zip");
        // 最小 zip：内含 StarQuake/marker.txt
        {
            let file = File::create(&package).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("StarQuake/update_e2e_marker.txt", opts).unwrap();
            zip.write_all(b"2.2.3").unwrap();
            zip.finish().unwrap();
        }
        let extract_dir = stage.join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();
        extract_zip(&package, &extract_dir).unwrap();
        let payload = find_payload_root(&extract_dir).unwrap();
        assert!(payload.join("update_e2e_marker.txt").is_file());
        // 模拟 apply：bat 写在 package 同级，不得 reset_staging
        let bat = package.parent().unwrap().join("apply_update.cmd");
        fs::write(&bat, b"@echo off\r\n").unwrap();
        assert!(package.is_file(), "package must survive");
        assert!(payload.join("update_e2e_marker.txt").is_file(), "payload must survive");
        assert!(bat.is_file());
        let _ = fs::remove_dir_all(&stage);
    }

    #[test]
    #[cfg(windows)]
    fn installer_apply_script_silent_and_e2e_dir() {
        let prod = installer_apply_script(r"C:\Temp\setup.exe", None);
        assert!(prod.contains(r#""C:\Temp\setup.exe""#));
        assert!(!prod.contains(" /S"));
        assert!(!prod.contains("/D="));
        assert!(prod.contains(r"%ProgramFiles(x86)%\StarQuake\starquake.exe"));

        let e2e = installer_apply_script(
            r"C:\Temp\setup.exe",
            Some(r"C:\Users\Huawei\StarQuakeE2E\install"),
        );
        assert!(e2e.contains(
            r#""C:\Temp\setup.exe" /S /D=C:\Users\Huawei\StarQuakeE2E\install"#
        ));
        assert!(e2e.contains(
            r"C:\Users\Huawei\StarQuakeE2E\install\starquake.exe"
        ));
    }

    #[test]
    fn pick_newest_ignores_stale_mirror() {
        let stale = UpdateManifest {
            latest_version: "2.3.0".into(),
            min_supported_version: String::new(),
            channel: "stable".into(),
            installer: None,
            zip: Some(ManifestAsset {
                url: "https://example.com/old.zip".into(),
                sha256: "aa".into(),
            }),
            linux: None,
            notes: String::new(),
            release_notes_url: String::new(),
            html_url: String::new(),
        };
        let fresh = UpdateManifest {
            latest_version: "2.3.1".into(),
            min_supported_version: String::new(),
            channel: "stable".into(),
            installer: None,
            zip: Some(ManifestAsset {
                url: "https://example.com/new.zip".into(),
                sha256: "bb".into(),
            }),
            linux: None,
            notes: String::new(),
            release_notes_url: String::new(),
            html_url: String::new(),
        };
        let cands = vec![
            ("https://stale/main".into(), stale),
            ("https://fresh/master".into(), fresh),
        ];
        let (url, m) = pick_newest_manifest(&cands).unwrap();
        assert_eq!(url, "https://fresh/master");
        assert_eq!(m.latest_version, "2.3.1");
        assert_eq!(m.zip.unwrap().sha256, "bb");
    }

    #[test]
    #[cfg(windows)]
    fn pick_platform_installer_channel_no_zip_fallback() {
        let raw = r#"{
          "latest_version": "2.2.3",
          "zip": { "url": "https://example.com/a.zip", "sha256": "" },
          "installer": { "url": "https://example.com/a_setup.exe", "sha256": "" }
        }"#;
        let m: UpdateManifest = serde_json::from_str(raw).unwrap();
        let (asset, name) =
            pick_platform_asset_for(&m, WinUpdateChannel::Installer).unwrap();
        assert!(asset.url.contains("setup.exe"), "{asset:?}");
        assert!(name.to_ascii_lowercase().ends_with(".exe"));
        assert!(!asset.url.contains(".zip"));
    }

    #[test]
    #[cfg(windows)]
    fn pick_platform_portable_channel_no_installer_fallback() {
        let raw = r#"{
          "latest_version": "2.2.3",
          "zip": { "url": "https://example.com/a.zip", "sha256": "aa" },
          "installer": { "url": "https://example.com/a_setup.exe", "sha256": "bb" }
        }"#;
        let m: UpdateManifest = serde_json::from_str(raw).unwrap();
        let (asset, name) =
            pick_platform_asset_for(&m, WinUpdateChannel::Portable).unwrap();
        assert!(asset.url.ends_with("a.zip"), "{asset:?}");
        assert_eq!(asset.sha256, "aa");
        assert!(name.to_ascii_lowercase().ends_with(".zip"));
    }

    #[test]
    #[cfg(windows)]
    fn pick_platform_installer_missing_does_not_use_zip() {
        let raw = r#"{
          "latest_version": "2.2.3",
          "zip": { "url": "https://example.com/a.zip", "sha256": "" }
        }"#;
        let m: UpdateManifest = serde_json::from_str(raw).unwrap();
        let err = pick_platform_asset_for(&m, WinUpdateChannel::Installer)
            .unwrap_err()
            .to_string();
        assert!(err.contains("安装包版"), "{err}");
        assert!(!err.to_ascii_lowercase().contains("a.zip"));
    }

    #[test]
    #[cfg(windows)]
    fn pick_platform_portable_missing_does_not_use_installer() {
        let raw = r#"{
          "latest_version": "2.2.3",
          "installer": { "url": "https://example.com/a_setup.exe", "sha256": "" }
        }"#;
        let m: UpdateManifest = serde_json::from_str(raw).unwrap();
        let err = pick_platform_asset_for(&m, WinUpdateChannel::Portable)
            .unwrap_err()
            .to_string();
        assert!(err.contains("便携版"), "{err}");
        assert!(!err.contains("setup.exe"), "{err}");
    }

}
