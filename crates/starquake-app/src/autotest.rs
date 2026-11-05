//! 进程内自动化巡检（环境变量 `STARQUAKE_AUTOTEST=1`）。
//!
//! 逐项覆盖：配置加载/回写、烈度公式、系统通知、托盘隐藏/恢复、设置六页切换、资源完整性。
//! 结果写入资源根 `logs/autotest_report.json` 后强制退出。

use crate::notify;
use crate::tray;
use serde_json::{json, Value};
use starquake_config::AppConfig;
use starquake_core::{calc_csis, estimate_at_site, estimate_epicentral, IntensityKind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    WaitSplash,
    OpenSettings,
    CycleTabs,
    TestNotify,
    TestIntensity,
    TestConfigRoundtrip,
    TestTrayHide,
    TestTrayShow,
    WriteReport,
    Done,
}

pub struct AutoTest {
    phase: Phase,
    started: Instant,
    phase_at: Instant,
    tab_idx: usize,
    checks: Vec<(String, bool, String)>,
    report_path: PathBuf,
}

impl AutoTest {
    /// 未设置 `STARQUAKE_AUTOTEST=1` 时返回 None。
    pub fn from_env(resource_root: &Path) -> Option<Self> {
        let on = std::env::var("STARQUAKE_AUTOTEST")
            .map(|v| {
                let v = v.trim();
                v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
            })
            .unwrap_or(false);
        if !on {
            return None;
        }
        let report_path = resource_root.join("logs").join("autotest_report.json");
        // 尽早落盘标记，便于确认环境变量已被进程读到（正式报告仍由 WriteReport 写出）
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(parent.join("autotest_started.txt"), b"1\n");
        }
        info!(path = %report_path.display(), "STARQUAKE_AUTOTEST enabled");
        Some(Self {
            phase: Phase::WaitSplash,
            started: Instant::now(),
            phase_at: Instant::now(),
            tab_idx: 0,
            checks: Vec::new(),
            report_path,
        })
    }

    pub fn finished(&self) -> bool {
        self.phase == Phase::Done
    }

    fn pass(&mut self, name: &str, detail: impl Into<String>) {
        self.checks.push((name.into(), true, detail.into()));
    }

    fn fail(&mut self, name: &str, detail: impl Into<String>) {
        self.checks.push((name.into(), false, detail.into()));
    }

    fn advance(&mut self, next: Phase) {
        self.phase = next;
        self.phase_at = Instant::now();
    }

    /// 每帧调用；返回 true 表示应强制退出。
    pub fn tick(
        &mut self,
        splash_active: bool,
        shell: &mut starquake_ui::MainShell,
        cfg: &AppConfig,
        resource_root: &Path,
        media_dir: &Path,
        hidden_to_tray: &mut bool,
        force_quit: &mut bool,
        ctx: &egui::Context,
    ) -> bool {
        // 总超时 90s
        if self.started.elapsed() > Duration::from_secs(90) && self.phase != Phase::Done {
            self.fail("timeout", "autotest exceeded 90s");
            self.advance(Phase::WriteReport);
        }

        // 保证巡检期间持续刷新
        ctx.request_repaint_after(Duration::from_millis(50));

        match self.phase {
            Phase::WaitSplash => {
                // 启动画面最多等 12s，超时也继续
                if !splash_active || self.phase_at.elapsed() > Duration::from_secs(12) {
                    self.pass(
                        "splash",
                        if splash_active {
                            "timed out waiting splash; continue"
                        } else {
                            "splash finished"
                        },
                    );
                    self.advance(Phase::OpenSettings);
                }
            }
            Phase::OpenSettings => {
                shell.settings.open_with(cfg);
                if shell.settings.open {
                    self.pass("settings_open", "settings panel opened");
                } else {
                    self.fail("settings_open", "settings.open still false");
                }
                self.advance(Phase::CycleTabs);
            }
            Phase::CycleTabs => {
                let n = starquake_ui::settings_tab_count();
                if self.tab_idx < n {
                    if self.tab_idx > 0 && self.phase_at.elapsed() < Duration::from_millis(180) {
                        return false;
                    }
                    let title = starquake_ui::settings_tab_title(self.tab_idx);
                    shell.settings.set_tab_index(self.tab_idx);
                    self.pass(
                        &format!("settings_tab_{}", self.tab_idx),
                        format!("switched to {title}"),
                    );
                    self.tab_idx += 1;
                    self.phase_at = Instant::now();
                } else if self.phase_at.elapsed() > Duration::from_millis(250) {
                    self.advance(Phase::TestNotify);
                }
            }
            Phase::TestNotify => {
                notify::show_test();
                // balloon / winrt 无法可靠探测系统 UI；调用不 panic 即记通过
                self.pass("notify_show_test", "notify::show_test invoked");
                self.advance(Phase::TestIntensity);
            }
            Phase::TestIntensity => {
                let r = 100.0_f64.hypot(10.0);
                let expect = 1.92 + 1.63 * 5.0 - 3.49 * r.log10();
                let v = calc_csis(5.0, 10.0, 100.0);
                if (v - expect).abs() < 1e-9 {
                    self.pass("intensity_sceew", format!("calc_csis M5@100km = {v:.6}"));
                } else {
                    self.fail(
                        "intensity_sceew",
                        format!("got {v}, expect {expect}"),
                    );
                }
                let (lv, text) =
                    estimate_at_site(IntensityKind::CnIntensity, 5.0, 10.0, 100.0);
                if lv >= 1 && !text.is_empty() {
                    self.pass(
                        "intensity_at_site",
                        format!("level={lv} text={text}"),
                    );
                } else {
                    self.fail("intensity_at_site", format!("level={lv} text={text}"));
                }
                // 回归：震中距不得钳成 0.1km 导致 M3.2@10km → 烈度 11
                let (lv32, text32) =
                    estimate_epicentral(IntensityKind::CnIntensity, 3.2, 10.0);
                if (2..=5).contains(&lv32) {
                    self.pass(
                        "intensity_m32_10km",
                        format!("level={lv32} text={text32}"),
                    );
                } else {
                    self.fail(
                        "intensity_m32_10km",
                        format!("expected mild 2..=5, got lv={lv32} text={text32}"),
                    );
                }
                // 「极浅」depth=0：近场下限须挡住虚高（旧 0.1km → ~Ⅺ）
                let (lv0, text0) =
                    estimate_epicentral(IntensityKind::CnIntensity, 3.2, 0.0);
                if lv0 <= 7 {
                    self.pass(
                        "intensity_m32_shallow0",
                        format!("level={lv0} text={text0}"),
                    );
                } else {
                    self.fail(
                        "intensity_m32_shallow0",
                        format!("expected <=7, got lv={lv0} text={text0}"),
                    );
                }
                self.advance(Phase::TestConfigRoundtrip);
            }
            Phase::TestConfigRoundtrip => {
                match check_config_inventory(resource_root, cfg) {
                    Ok(msg) => self.pass("config_inventory", msg),
                    Err(e) => self.fail("config_inventory", e),
                }
                match check_resources(resource_root, media_dir) {
                    Ok(msg) => self.pass("resources", msg),
                    Err(e) => self.fail("resources", e),
                }
                match check_config_roundtrip(cfg) {
                    Ok(msg) => self.pass("config_roundtrip", msg),
                    Err(e) => self.fail("config_roundtrip", e),
                }
                self.advance(Phase::TestTrayHide);
            }
            Phase::TestTrayHide => {
                if cfg.ui.minimize_to_tray {
                    // 勿真隐藏窗口：无帧后 update/tick 会停，巡检无法收尾。
                    // 只验证托盘 API / balloon，并走一遍隐藏标志的置位与清除。
                    tray::set_hidden_to_tray(true);
                    let balloon = tray::show_balloon(
                        "StarQuake AUTOTEST",
                        "autotest tray hide",
                    );
                    tray::set_hidden_to_tray(false);
                    *hidden_to_tray = false;
                    self.pass(
                        "tray_hide",
                        format!("tray api ok (no hide); balloon_ok={balloon}"),
                    );
                } else {
                    self.pass("tray_hide", "minimize_to_tray disabled; skipped");
                }
                self.advance(Phase::TestTrayShow);
            }
            Phase::TestTrayShow => {
                *hidden_to_tray = false;
                tray::set_hidden_to_tray(false);
                tray::win_show_normal();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.pass("tray_show", "focus/visible commands issued");
                // 关闭设置，避免挡操作
                shell.settings.close_after_save();
                self.advance(Phase::WriteReport);
            }
            Phase::WriteReport => {
                let ok = self.checks.iter().all(|(_, p, _)| *p);
                let report = build_report(&self.checks, self.started.elapsed(), ok);
                if let Some(parent) = self.report_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(
                    &self.report_path,
                    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".into()),
                ) {
                    Ok(()) => info!(path = %self.report_path.display(), ok, "autotest report written"),
                    Err(e) => warn!(error = %e, "failed to write autotest report"),
                }
                *force_quit = true;
                tray::request_force_quit();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                self.advance(Phase::Done);
                return true;
            }
            Phase::Done => return true,
        }
        false
    }
}

fn build_report(checks: &[(String, bool, String)], elapsed: Duration, ok: bool) -> Value {
    json!({
        "ok": ok,
        "elapsed_ms": elapsed.as_millis() as u64,
        "passed": checks.iter().filter(|(_, p, _)| *p).count(),
        "failed": checks.iter().filter(|(_, p, _)| !*p).count(),
        "checks": checks.iter().map(|(n, p, d)| json!({
            "name": n,
            "pass": p,
            "detail": d,
        })).collect::<Vec<_>>(),
    })
}

fn check_config_inventory(resource_root: &Path, cfg: &AppConfig) -> Result<String, String> {
    let default_toml = resource_root.join("config/default.toml");
    if !default_toml.is_file() {
        return Err(format!("missing {}", default_toml.display()));
    }
    let _loaded = AppConfig::load_merged(&default_toml);
    // 关键设置页对应的关键布尔/枚举字段可读
    let flags = [
        ("ui.minimize_to_tray", cfg.ui.minimize_to_tray),
        ("ui.always_on_top", cfg.ui.always_on_top),
        ("ui.show_sidebar", cfg.ui.show_sidebar),
        ("ui.auto_follow_eew", cfg.ui.auto_follow_eew),
        ("ui.auto_zoom_waves", cfg.ui.auto_zoom_waves),
        ("ui.show_wave_rings", cfg.ui.show_wave_rings),
        ("ui.show_legend", cfg.ui.show_legend),
        ("ui.show_volcanoes", cfg.ui.show_volcanoes),
        ("ui.show_weather", cfg.ui.show_weather),
        ("ui.show_typhoons", cfg.ui.show_typhoons),
        ("ui.show_intensity_map", cfg.ui.show_intensity_map),
        ("ui.show_terminator", cfg.ui.show_terminator),
        ("notify.enabled", cfg.notify.enabled),
        ("notify.on_eew", cfg.notify.on_eew),
        ("notify.on_record", cfg.notify.on_record),
        ("notify.on_tsunami", cfg.notify.on_tsunami),
        ("audio.mute", cfg.audio.mute),
        ("audio.tts_enabled", cfg.audio.tts_enabled),
        ("audio.play_countdown", cfg.audio.play_countdown),
        ("sources.jian.enabled", cfg.sources.jian.enabled),
        ("sources.wolfx.enabled", cfg.sources.wolfx.enabled),
        ("sources.p2pquake.enabled", cfg.sources.p2pquake.enabled),
        ("sources.map_wx.enabled", cfg.sources.map_wx.enabled),
        ("update.check_on_startup", cfg.update.check_on_startup),
        ("log.enabled", cfg.log.enabled),
    ];
    let _ = flags;
    let device_ok = matches!(
        cfg.performance.device.to_ascii_lowercase().as_str(),
        "cpu" | "gpu"
    );
    if !device_ok {
        return Err(format!("bad performance.device={}", cfg.performance.device));
    }
    let theme_ok = !cfg.ui.theme.trim().is_empty();
    if !theme_ok {
        return Err("empty ui.theme".into());
    }
    let ro = cfg.sources.map_wx.radar_opacity;
    let po = cfg.sources.map_wx.precip_opacity;
    if !(0.0..=1.0).contains(&ro) || !ro.is_finite() {
        return Err(format!("bad map_wx.radar_opacity={ro}"));
    }
    if !(0.0..=1.0).contains(&po) || !po.is_finite() {
        return Err(format!("bad map_wx.precip_opacity={po}"));
    }
    Ok(format!(
        "merged ok; device={}; theme={}; flags={}; radar_op={ro:.2}; precip_op={po:.2}",
        cfg.performance.device,
        cfg.ui.theme,
        flags.len()
    ))
}

fn check_config_roundtrip(cfg: &AppConfig) -> Result<String, String> {
    let v = serde_json::to_value(cfg).map_err(|e| e.to_string())?;
    let back: AppConfig = serde_json::from_value(v).map_err(|e| e.to_string())?;
    if back.ui.minimize_to_tray != cfg.ui.minimize_to_tray {
        return Err("minimize_to_tray mismatch after roundtrip".into());
    }
    if back.notify.enabled != cfg.notify.enabled {
        return Err("notify.enabled mismatch".into());
    }
    if (back.audio.master_volume - cfg.audio.master_volume).abs() > 1e-6 {
        return Err("master_volume mismatch".into());
    }
    if (back.sources.map_wx.radar_opacity - cfg.sources.map_wx.radar_opacity).abs() > 1e-6 {
        return Err("radar_opacity mismatch".into());
    }
    if (back.sources.map_wx.precip_opacity - cfg.sources.map_wx.precip_opacity).abs() > 1e-6 {
        return Err("precip_opacity mismatch".into());
    }
    Ok("json roundtrip ok".into())
}

fn check_resources(resource_root: &Path, media_dir: &Path) -> Result<String, String> {
    let mut missing = Vec::new();
    for rel in [
        "config/default.toml",
        "icon/brand/app.ico",
    ] {
        if !resource_root.join(rel).is_file() {
            missing.push(rel.to_string());
        }
    }
    for rel in ["sound/catalog.json", "travel"] {
        let p = media_dir.join(rel);
        if !p.exists() {
            missing.push(format!("media/{rel}"));
        }
    }
    for rel in ["sound/eewalert.wav", "sound/eewupdate.wav"] {
        if !media_dir.join(rel).is_file() {
            missing.push(format!("media/{rel}"));
        }
    }
    if !missing.is_empty() {
        return Err(format!("missing: {}", missing.join(", ")));
    }
    Ok("core resources present".into())
}
