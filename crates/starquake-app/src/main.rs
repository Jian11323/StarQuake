//! StarQuake 桌面客户端入口。

// GUI 子系统：无控制台窗口；logging 永不 AllocConsole，仅可选写 stderr。
#![windows_subsystem = "windows"]

mod autotest;
mod logging;
mod notify;
mod official_notice;
mod tray;
mod update;

use anyhow::Result;
use eframe::egui;
use starquake_audio::{
    configure_tts, speak_async, speak_with_config, AudioCue, AudioEngine, TsunamiCueKind,
    TtsEngine, TtsRuntimeConfig,
};
use starquake_config::AppConfig;
use starquake_core::{
    LocalHypoEstimate, agency_family, build_eew_tts_script, build_record_tts_script, eew_sidebar_entries,
    cinema_event_key, dedupe_same_eew_events, eew_sidebar_group_key, eew_wave_active_spread,
    is_wave_candidate, max_s_spread_km, origin_is_stale, same_eew_event, should_play_eew_tts,
    AgencyFamily,
    AppSnapshot, EewReport, HealthStatus, IntensityKind, jma_shindo_text,
    tsunami_grade_zh,
};
use std::collections::{HashMap, HashSet};
use starquake_net::{load_place_fix, spawn_hub_with_assets, NetEvent, NetHub};
use starquake_hypo::{locate, ArrivalPick, LocateConfig};
use starquake_travel::{s_wave_still_spreading_to, TravelEngine};
use starquake_map::{zoom_to_fit_radius_km, BasemapMode, ProjectionMode};
use starquake_ui::{
    draw_splash, MainShell, OfficialNoticeView, SettingsAction, SplashGate, AppUiTheme,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;
use tokio::runtime::Runtime;
use official_notice::{fetch_notices, OfficialNoticeState, RemoteNotice};
use update::{
    AvailableUpdate, StartupUpdateOutcome, apply_and_relaunch, check_latest,
    download_asset_with_progress,
    open_release_page, run_startup_auto_update,
};

fn looks_like_resource_root(dir: &Path) -> bool {
    dir.join("media").is_dir()
        && dir.join("icon").is_dir()
        && (dir.join("geodata").is_dir() || dir.join("config").is_dir())
}

/// 发行包：exe 同目录；开发：`cargo run` 时沿路径找到仓库根。
/// 可用 `STARQUAKE_ROOT` 覆盖（兼容旧名 `EEWVIEW_ROOT`）。
fn resource_root() -> PathBuf {
    for key in ["STARQUAKE_ROOT", "EEWVIEW_ROOT"] {
        if let Ok(p) = std::env::var(key) {
            let p = PathBuf::from(p);
            if p.is_dir() {
                return p;
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..6 {
            let Some(d) = dir else { break };
            if looks_like_resource_root(&d) {
                return d;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop(); // crates
    dir.pop(); // repo root
    dir
}

fn media_root() -> PathBuf {
    resource_root().join("media")
}

fn resolve_geodata_dir(configured: &str) -> PathBuf {
    let p = Path::new(configured);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        resource_root().join(p)
    }
}

/// 嵌入霞鹜文楷作 CJK 回退。拉丁走 egui 默认字体，汉字才回退文楷（避免文楷当主字体撑爆字形图集）。
/// 使用 from_static：字体页落在映像映射里，避免 from_owned 再堆一份 24MB。
fn install_cjk_fonts(ctx: &egui::Context) {
    const LXGW_WENKAI: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../media/fonts/lxgw-wenkai-v1.522/LXGWWenKai-Regular.ttf"
    ));

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "lxgw_wenkai".to_string(),
        egui::FontData::from_static(LXGW_WENKAI).into(),
    );
    fonts.families.insert(
        egui::FontFamily::Name("lxgw_wenkai".into()),
        vec!["lxgw_wenkai".to_string()],
    );
    if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        if !fam.iter().any(|n| n == "lxgw_wenkai") {
            fam.push("lxgw_wenkai".to_string());
        }
    }
    if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        if !fam.iter().any(|n| n == "lxgw_wenkai") {
            fam.push("lxgw_wenkai".to_string());
        }
    }
    ctx.set_fonts(fonts);
    tracing::info!(bytes = LXGW_WENKAI.len(), "embedded LXGW WenKai as CJK fallback");
}

fn window_title() -> String {
    // 展示形如「Beta 2.3.4」（后续发版一律 Beta，不再标正式版）
    format!(
        "{} {}",
        starquake_ui::APP_DISPLAY_NAME,
        starquake_ui::APP_VERSION_DISPLAY
    )
}

fn main() {
    // rustls 0.23：多后端并存时必须显式安装，否则 TLS（含 Edge TTS）会 panic
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Windows Toast 须在首条通知前注册 AUMID
    notify::register_app_id();

    if let Err(e) = run_app() {
        let msg = format!("{e:#}");
        logging::show_fatal_dialog(starquake_ui::APP_DISPLAY_NAME, &format!("启动失败：\n{msg}"));
        std::process::exit(1);
    }
}

fn run_app() -> Result<()> {
    // 日志订阅器进程内只能装一次。OpenGL 失败回退会再次进入 run_app_inner，
    // 守卫必须留在这一层，否则第二次 init 会 panic，且已安装的写入端被提前拆掉。
    let root = resource_root();
    let cfg = AppConfig::load_merged(root.join("config/default.toml"));
    let log_guard = logging::init_logging(&root, &cfg.log);
    let result = run_app_inner(false);
    drop(log_guard);
    result
}

/// GPU/OpenGL 创建失败时回退 CPU 软渲染的错误特征。
fn should_fallback_soft_render(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("es context")
        || e.contains("wgl")
        || e.contains("opengl")
        || e.contains("glutin")
        || e.contains("glow")
        || e.contains("egl")
        || e.contains("pixel format")
        || e.contains("create_context")
        || e.contains("create context")
        || e.contains("no suitable")
        || e.contains("hardware")
        || e.contains("gpu")
        || e.contains("vsync")
        || e.contains("swap")
        || e.contains("display")
        || e.contains("eframe")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

/// 自动化：记录已走「GPU 失败 → CPU 软渲染」回退路径。
fn write_gpu_fallback_marker(resource_root: &Path) {
    let dir = resource_root.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("gpu_fallback_ok.txt");
    if let Err(e) = std::fs::write(&path, b"soft\n") {
        tracing::warn!(error = %e, "write gpu_fallback_ok.txt failed");
    } else {
        tracing::info!(path = %path.display(), "GPU fallback marker written");
    }
}

#[cfg(test)]
mod gl_fallback_tests {
    use super::{build_native_options, should_fallback_soft_render};
    use starquake_config::PerformanceConfig;

    #[test]
    fn detects_wgl_es_failure() {
        let log = "glutin error: extension to create ES context with wgl is not present";
        assert!(should_fallback_soft_render(log));
        assert!(should_fallback_soft_render("eframe: Failed to create glutin display"));
        assert!(should_fallback_soft_render("OpenGL context creation failed"));
        assert!(should_fallback_soft_render("no suitable pixel format"));
        assert!(!should_fallback_soft_render("disk full"));
        assert!(!should_fallback_soft_render("permission denied"));
    }

    #[test]
    fn force_soft_turns_off_hardware_acceleration() {
        let perf = PerformanceConfig {
            device: "gpu".into(),
            profile: "normal".into(),
            target_fps: 60,
        };
        let vp = egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]);
        let soft = build_native_options(vp.clone(), &perf, true);
        assert!(matches!(
            soft.hardware_acceleration,
            eframe::HardwareAcceleration::Off
        ));
        let gpu = build_native_options(vp, &perf, false);
        assert!(matches!(
            gpu.hardware_acceleration,
            eframe::HardwareAcceleration::Preferred
        ));
    }

    #[test]
    fn config_cpu_device_also_disables_hw() {
        let perf = PerformanceConfig {
            device: "cpu".into(),
            profile: "normal".into(),
            target_fps: 60,
        };
        let vp = egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]);
        let opts = build_native_options(vp, &perf, false);
        assert!(matches!(
            opts.hardware_acceleration,
            eframe::HardwareAcceleration::Off
        ));
    }
}

fn run_app_inner(force_soft: bool) -> Result<()> {
    let root = resource_root();
    if env_flag("STARQUAKE_AUTOTEST") {
        let dir = root.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            dir.join("autotest_env_seen.txt"),
            format!("force_soft={force_soft}\nroot={}\n", root.display()),
        );
    }
    // 模拟 GPU/OpenGL 不可用：首轮强制走软渲染回退（供自动化验证，不递归）。
    if !force_soft && env_flag("STARQUAKE_SIMULATE_GPU_FAIL") {
        let fake = "glutin error: extension to create ES context with wgl is not present (simulated)";
        tracing::warn!(error = %fake, "STARQUAKE_SIMULATE_GPU_FAIL：模拟 GPU 不可用，回退 CPU");
        assert!(should_fallback_soft_render(fake));
        write_gpu_fallback_marker(&root);
        return run_app_inner(true);
    }
    let media = media_root();
    let cfg = AppConfig::load_merged(root.join("config/default.toml"));
    let mut cfg = cfg;
    cfg.ui.language = starquake_config::normalize_ui_language(&cfg.ui.language);
    starquake_ui::set_lang_from_config(&cfg.ui.language);
    tracing::info!(
        root = %root.display(),
        version = env!("CARGO_PKG_VERSION"),
        "resource root"
    );

    // 启动主窗口前检查更新（复用 update 清单竞速逻辑；确认安装则退出，不进主界面）。
    // GPU 软渲染重试 / 自动化巡检不再弹窗、也不在 splash 后再自动查。
    let mut startup_update_done = force_soft || env_flag("STARQUAKE_AUTOTEST");
    let mut pending_startup_install: Option<AvailableUpdate> = None;
    if !force_soft && !env_flag("STARQUAKE_AUTOTEST") {
        let dismissed_before = cfg.update.last_dismissed_update_version.clone();
        match run_startup_auto_update(&mut cfg.update, starquake_ui::APP_VERSION_DISPLAY) {
            StartupUpdateOutcome::Exit => return Ok(()),
            StartupUpdateOutcome::Continue { checked } => {
                startup_update_done = checked;
                if cfg.update.last_dismissed_update_version != dismissed_before {
                    if let Err(e) = cfg.save_user_overlay() {
                        tracing::warn!("保存已跳过更新版本失败: {e:#}");
                    }
                }
            }
            StartupUpdateOutcome::PendingInstall(info) => {
                startup_update_done = true;
                pending_startup_install = Some(info);
            }
        }
    }
    let basemap = BasemapMode::resolve(&cfg.map.basemap, &cfg.ui.theme);
    let geodata_dir = resolve_geodata_dir(&cfg.map.geodata_dir);
    tracing::info!(
        event_pack = %cfg.audio.event_pack,
        theme = %cfg.ui.theme,
        basemap = basemap.as_str(),
        geodata = %geodata_dir.display(),
        tile = %format!("{}/{}", cfg.map.tile_base, cfg.map.tile_source),
        jian = cfg.sources.jian.enabled,
        wolfx = cfg.sources.wolfx.enabled,
        p2p = cfg.sources.p2pquake.enabled,
        fssn = cfg.sources.fssn.enabled,
        jian_at = cfg
            .sources
            .jian
            .access_token
            .as_ref()
            .map(|s| s.trim().starts_with("at_"))
            .unwrap_or(false),
        jian_rt = cfg
            .sources
            .jian
            .refresh_token
            .as_ref()
            .map(|s| s.trim().starts_with("rt_"))
            .unwrap_or(false),
        "config ready"
    );

    let travel = match TravelEngine::load_named(
        media.join("travel"),
        &cfg.travel.primary,
        &cfg.travel.fallback,
        cfg.travel.linear_last_resort,
    ) {
        Ok(eng) => {
            let t = eng.travel_time(starquake_travel::Wave::S, 40.0, 100.0);
            tracing::info!(
                primary = %cfg.travel.primary,
                s_travel_40km_100km_s = t,
                "travel table loaded"
            );
            Some(eng)
        }
        Err(e) => {
            tracing::warn!("走时表未加载（先 cargo run -p convert_travel_tables）: {e}");
            None
        }
    };

    load_place_fix(media.join("place"));

    let audio = match AudioEngine::from_catalog(
        media.join("sound/catalog.json"),
        media.join("sound"),
        &cfg.audio.event_pack,
        &cfg.audio.countdown_pack,
        cfg.audio.master_volume,
        cfg.audio.countdown_volume,
        cfg.audio.mute,
    ) {
        Ok(a) => {
            let sound = media.join("sound");
            let has = sound.join("0s.mp3").is_file() && sound.join("countdown.wav").is_file();
            if has {
                tracing::info!(path = %sound.display(), "倒计时素材已加载");
            } else {
                tracing::warn!(
                    path = %sound.display(),
                    "未找到倒计时素材（0s.mp3 / countdown.wav），倒计时语音将静默"
                );
            }
            Some(a)
        }
        Err(e) => {
            tracing::warn!("音效引擎未启动: {e:#}");
            None
        }
    };
    apply_tts_runtime(&cfg.audio);

    let live = cfg.any_source_enabled();
    let mut snap = AppSnapshot::empty();
    if live {
        tracing::info!("waiting for source events");
    } else {
        tracing::info!("all sources disabled");
    }

    let rt = Runtime::new()?;
    let net_rx = {
        let _enter = rt.enter();
        let stations_dir = media.join("stations");
        Some(spawn_hub_with_assets(
            &cfg.sources,
            stations_dir.is_dir().then_some(stations_dir),
        ))
    };
    let _ = live;

    refresh_countdown(
        &mut snap,
        travel.as_ref(),
        cfg.local.latitude,
        cfg.local.longitude,
        None,
    );

    let mut shell = MainShell::default();
    shell.show_nied_clock = cfg.ui.show_nied_clock;
    shell.intensity_scale = cfg.ui.intensity_scale.clone();
    shell.local_lat = cfg.local.latitude;
    shell.local_lon = cfg.local.longitude;
    shell.configure_map(
        basemap,
        cfg.map.tile_base.clone(),
        cfg.map.tile_source.clone(),
        Some(geodata_dir.as_path()),
        cfg.map.default_lon,
        cfg.map.default_lat,
        cfg.map.default_zoom,
        ProjectionMode::parse(&cfg.map.projection),
        cfg.ui.show_faults,
    );
    shell
        .map
        .set_tile_capacity(starquake_util::perf::recommended_tile_slots(
            cfg.performance.device.eq_ignore_ascii_case("cpu"),
        ));
    snap.tab = match cfg.ui.sidebar_default_tab.to_ascii_lowercase().as_str() {
        "records" | "record" => starquake_core::SidebarTab::Records,
        "volcano" => starquake_core::SidebarTab::Volcano,
        "weather" => starquake_core::SidebarTab::Weather,
        // 旧配置 station / cenc_int 已下线侧栏页签，回落到预警
        _ => starquake_core::SidebarTab::Eew,
    };
    if let Some(ev) = &snap.active {
        shell.map.center_lon = ev.longitude;
        shell.map.center_lat = ev.latitude;
    }

    let win = &cfg.window;
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([
            win.width.clamp(640.0, 7680.0),
            win.height.clamp(480.0, 4320.0),
        ])
        .with_min_inner_size([640.0, 480.0])
        .with_title(window_title());
    if let (Some(x), Some(y)) = (win.x, win.y) {
        viewport = viewport.with_position([x, y]);
    }
    if win.maximized {
        viewport = viewport.with_maximized(true);
    }
    if let Some(icon) = starquake_ui::app_icon_data() {
        viewport = viewport.with_icon(icon);
    }
    if cfg.ui.always_on_top {
        viewport = viewport.with_always_on_top();
    }

    // 固定 OpenGL（Glow）。WGL 无法创建 ES 上下文时自动改软渲染再试一次。
    let options = build_native_options(viewport, &cfg.performance, force_soft);

    let started = Instant::now();
    let has_local = cfg.local.latitude.is_some() && cfg.local.longitude.is_some();
    snap.show_wave_rings = cfg.ui.show_wave_rings;
    let run_err = eframe::run_native(
        starquake_ui::APP_DISPLAY_NAME,
        options,
        Box::new(move |cc| {
            install_cjk_fonts(&cc.egui_ctx);
            // 关闭嵌入式 viewport：侧栏分离时用真正的独立 OS 窗口
            cc.egui_ctx.set_embed_viewports(false);
            let autotest = autotest::AutoTest::from_env(&root);
            Ok(Box::new(StarQuakeApp {
                resource_root: root,
                snap,
                shell,
                travel,
                started,
                local_lat: cfg.local.latitude,
                local_lon: cfg.local.longitude,
                has_local,
                geodata_dir,
                media_dir: media,
                cfg,
                rt,
                net_rx,
                audio,
                auto_center_pending: live,
                last_follow_key: None,
                last_follow_cinema: None,
                last_record_follow_key: None,
                last_eew_tts: HashMap::new(),
                last_eew_tts_script: HashMap::new(),
                cinema_idx: 0,
                cinema_next_at: Instant::now(),
                cinema_seen_key: String::new(),
                cinema_prev_live: Vec::new(),
                prev_health: [HealthStatus::Abnormal; 5],
                prev_tsunami_key: String::new(),
                eew_alert_played: HashSet::new(),
                nhk_bell_played: HashSet::new(),
                window_geom_save_at: None,
                last_window_geom: None,
                splash: SplashGate::default(),
                update_rx: None,
                update_available: None,
                update_status: String::new(),
                update_busy: false,
                update_progress: None,
                update_progress_rx: None,
                update_auto_install: pending_startup_install,
                hypo_first_trigger: std::collections::HashMap::new(),
                hypo_last_run: Instant::now(),
                update_checked: startup_update_done,
                update_check_manual: false,

                official_notices: OfficialNoticeState::load(),
                notice_rx: None,
                notice_fetching: false,
                tray: {
                    let icon = starquake_ui::app_icon_data().map(|i| {
                        (i.rgba, i.width, i.height)
                    });
                    tray::TrayHandle::try_create(icon)
                },
                hidden_to_tray: false,
                force_quit: false,
                bg_tiles_cleared: false,
                autotest,
            }))
        }),
    );
    if let Err(e) = run_err {
        let msg = e.to_string();
        if !force_soft && should_fallback_soft_render(&msg) {
            tracing::warn!(error = %msg, "GPU/OpenGL 不可用，回退 CPU 软渲染后重试");
            write_gpu_fallback_marker(&resource_root());
            return run_app_inner(true);
        }
        return Err(anyhow::anyhow!("eframe: {msg}"));
    }
    Ok(())
}

fn build_native_options(
    viewport: egui::ViewportBuilder,
    perf: &starquake_config::PerformanceConfig,
    force_soft: bool,
) -> eframe::NativeOptions {
    let device = perf.device.to_ascii_lowercase();
    let profile = perf.profile.to_ascii_lowercase();
    let hw = if force_soft || device == "cpu" {
        eframe::HardwareAcceleration::Off
    } else {
        eframe::HardwareAcceleration::Preferred
    };
    eframe::NativeOptions {
        viewport,
        vsync: profile != "extreme",
        hardware_acceleration: hw,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    }
}

/// 按忙碌类型选重绘间隔，避免老机器空载/软忙仍按目标帧率烧 CPU/GPU。
#[derive(Clone, Copy)]
enum RepaintNeed {
    /// 拖拽/飞跃/启动画面：跟手
    Interactive,
    /// 波圈动画
    Waves,
    /// 设置页打开或倒计时秒显
    Soft,
    /// 完全空闲
    Idle,
}

fn repaint_ms(perf: &starquake_config::PerformanceConfig, need: RepaintNeed) -> u64 {
    match need {
        // 跟手/飞跃：严格按用户目标帧率（60→约17ms；不再被波圈档位二次压低）
        RepaintNeed::Interactive => perf.repaint_interval_ms(),
        RepaintNeed::Waves => perf.wave_repaint_ms(),
        RepaintNeed::Soft => perf.soft_repaint_ms(),
        RepaintNeed::Idle => perf.idle_repaint_ms(),
    }
}

/// 要石「仅强有感」：日本震度 ≥3，中国烈度 ≥5。
fn countdown_is_intense(snap: &AppSnapshot) -> bool {
    use starquake_core::IntensityKind;
    match snap.countdown_local_kind {
        IntensityKind::JmaShindo => snap.countdown_local_level >= 3,
        IntensityKind::CnIntensity => snap.countdown_local_level >= 5,
    }
}

fn clear_countdown(snap: &mut AppSnapshot) {

    snap.countdown_s = None;
    snap.countdown_local_level = 0;
    snap.countdown_local_text.clear();
}

fn refresh_countdown(
    snap: &mut AppSnapshot,
    travel: Option<&TravelEngine>,
    local_lat: Option<f64>,
    local_lon: Option<f64>,
    elapsed: Option<f64>,
) {
    let (Some(eng), Some(lat), Some(lon), Some(ev)) =
        (travel, local_lat, local_lon, snap.active.as_ref())
    else {
        if local_lat.is_none() || local_lon.is_none() || snap.active.is_none() {
            clear_countdown(snap);
        }
        return;
    };
    // 调查中（仮定震源）：对齐要石，不播/不显本地倒计时
    if ev.is_cancel || ev.is_assumption || (ev.origin_ms <= 0 && elapsed.is_none()) {
        clear_countdown(snap);
        return;
    }
    let elapsed_s = if let Some(e) = elapsed {
        e
    } else if ev.origin_ms > 0 {
        let now_ms = chrono::Utc::now().timestamp_millis();
        ((now_ms - ev.origin_ms) as f64 / 1000.0).max(0.0)
    } else {
        0.0
    };
    // PS 波走完最大传播距离后停止并隐藏（与地图波圈 / 左上预警条同一条件）
    let max_km = max_s_spread_for(ev.magnitude, &snap.wave_spread_km);
    if !s_wave_still_spreading_to(eng, ev.depth_km, elapsed_s, max_km) {
        clear_countdown(snap);
        return;
    }
    let surface = starquake_util::haversine_km(ev.longitude, ev.latitude, lon, lat);
    // 超过 2000km：不再显示本地倒计时（与走时表远距回退边界一致）
    if surface > 2000.0 {
        clear_countdown(snap);
        return;
    }
    // 本机已在该震级 S 波最大传播范围之外：波到不了，不显示倒计时
    let max_reach = max_s_spread_for(ev.magnitude, &snap.wave_spread_km);
    if surface > max_reach * 1.05 {
        clear_countdown(snap);
        return;
    }
    // JMA2001 超 2000km 自动回退 ak135（再不行直线兜底）
    let total = eng.eta_s_to_site(ev.longitude, ev.latitude, ev.depth_km, lon, lat);
    if !total.is_finite() || total <= 0.0 {
        clear_countdown(snap);
        return;
    }
    // 对齐 eewcn：到达后挂 0 秒，直到 PS 波圈走完才卸倒计时
    let remain = (total - elapsed_s).ceil().max(0.0) as i32;
    let (lv, text) = starquake_core::estimate_at_site(
        ev.intensity_kind,
        ev.magnitude,
        ev.depth_km,
        surface,
    );
    snap.countdown_s = Some(remain);
    snap.countdown_local_kind = ev.intensity_kind;
    snap.countdown_local_level = lv;
    snap.countdown_local_text = text;
}

fn max_s_spread_for(magnitude: f64, custom: &[(f64, f64, f64)]) -> f64 {
    max_s_spread_km(magnitude, custom)
}

fn eew_wave_live(
    e: &EewReport,
    now_ms: i64,
    spread: &[(f64, f64, f64)],
    travel: Option<&TravelEngine>,
) -> bool {
    if let Some(eng) = travel {
        if !is_wave_candidate(e) {
            return false;
        }
        if e.origin_ms <= 0 {
            return true;
        }
        let age_ms = now_ms - e.origin_ms;
        if age_ms < -60_000 {
            return false;
        }
        let elapsed = (age_ms as f64 / 1000.0).max(0.0);
        let max_km = max_s_spread_km(e.magnitude, spread);
        s_wave_still_spreading_to(eng, e.depth_km, elapsed, max_km)
    } else {
        eew_wave_active_spread(e, now_ms, spread)
    }
}

struct StarQuakeApp {
    resource_root: PathBuf,
    snap: AppSnapshot,
    shell: MainShell,
    travel: Option<TravelEngine>,
    started: Instant,
    local_lat: Option<f64>,
    local_lon: Option<f64>,
    has_local: bool,
    geodata_dir: PathBuf,
    media_dir: PathBuf,
    cfg: AppConfig,
    rt: Runtime,
    net_rx: Option<NetHub>,
    audio: Option<AudioEngine>,
    auto_center_pending: bool,
    /// 已跟焦的预警 (event_id, serial)，避免每帧重复居中
    last_follow_key: Option<(String, u32)>,
    /// 已跟焦过的 cinema 键（防 upsert 先写 active 导致首报不飞）
    last_follow_cinema: Option<String>,
    last_record_follow_key: Option<String>,
    /// 各侧栏源槽上次已播 TTS 的报文（对齐 eewcn last_report_data_dict）
    last_eew_tts: HashMap<String, EewReport>,
    /// 同槽上次朗读原文；文案相同绝不重读
    last_eew_tts_script: HashMap<String, String>,
    /// 多震 cinema 轮播：当前索引与下次切换时刻
    cinema_idx: usize,
    cinema_next_at: Instant,
    /// 已见过多震 id 签名，用于发现新震立刻跳转
    cinema_seen_key: String,
    /// 上一帧 cinema 队列（用于同事件换源/换 id 时勿当新震飞跃）
    cinema_prev_live: Vec<EewReport>,
    prev_health: [HealthStatus; 5],
    prev_tsunami_key: String,
    /// 已播 JMA 警报专用音的事件键 agency|event_id
    eew_alert_played: HashSet<String>,
    /// 已播 NHK 一级新闻铃的速报键
    nhk_bell_played: HashSet<String>,
    /// 窗口几何变更后延迟写入用户配置
    window_geom_save_at: Option<Instant>,
    last_window_geom: Option<(f32, f32, Option<f32>, Option<f32>, bool)>,
    splash: SplashGate,
    /// 后台检查更新结果
    update_rx: Option<mpsc::Receiver<Result<Option<AvailableUpdate>, String>>>,
    update_available: Option<AvailableUpdate>,
    update_status: String,
    update_busy: bool,
    /// 下载进度：(已下载, 总长度)
    update_progress: Option<(u64, Option<u64>)>,
    update_progress_rx: Option<mpsc::Receiver<(u64, Option<u64>)>>,
    /// 启动 MessageBox 确认后，首帧自动开始下载
    update_auto_install: Option<AvailableUpdate>,
    /// station id -> first trigger unix sec (for hypo)
    hypo_first_trigger: std::collections::HashMap<String, f64>,
    hypo_last_run: Instant,
    update_checked: bool,
    /// 本次检查是否由用户手动触发（启动自动检查无更新时不弹「已是最新」）
    update_check_manual: bool,
    /// 官方通知中心
    official_notices: OfficialNoticeState,
    notice_rx: Option<mpsc::Receiver<Result<Vec<RemoteNotice>, String>>>,
    notice_fetching: bool,
    /// 系统托盘（关闭到托盘）
    tray: Option<tray::TrayHandle>,
    /// 已隐藏到托盘（避免重复 CancelClose）
    hidden_to_tray: bool,
    /// 托盘「退出」：允许真正关闭
    force_quit: bool,
    /// 后台（托盘/最小化）已清过瓦片，避免反复 clear
    bg_tiles_cleared: bool,
    /// `STARQUAKE_AUTOTEST=1` 时进程内逐项巡检
    autotest: Option<autotest::AutoTest>,
}

impl eframe::App for StarQuakeApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 保持非嵌入，供分离侧栏 `show_viewport_immediate` 开独立窗
        ctx.set_embed_viewports(false);
        self.capture_main_hwnd(frame);
        if let Some(tray) = self.tray.as_ref() {
            tray.bind_ctx(ctx);
        }
        self.tick_tray(ctx);

        if self
            .autotest
            .as_ref()
            .is_some_and(|a| !a.finished())
        {
            let mut at = self.autotest.take().expect("autotest");
            let splash = self.splash.active();
            let root = self.resource_root.clone();
            let media = self.media_dir.clone();
            let cfg = self.cfg.clone();
            let quit = at.tick(
                splash,
                &mut self.shell,
                &cfg,
                &root,
                &media,
                &mut self.hidden_to_tray,
                &mut self.force_quit,
                ctx,
            );
            self.autotest = Some(at);
            if quit {
                ctx.request_repaint();
            }
        }

        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        tray::set_minimized_bg(minimized && !self.hidden_to_tray);
        let backgrounded = self.hidden_to_tray || minimized;
        if backgrounded {
            if !self.bg_tiles_cleared {
                self.shell.map.clear_tile_cache();
                self.bg_tiles_cleared = true;
            }
            // 后台仍要及时处理 net_rx / 音效；勿被 Idle 降到数秒一帧
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        } else {
            self.bg_tiles_cleared = false;
        }

        self.tick_window_geometry(ctx);
        self.tick_update_check(ctx);
        self.tick_official_notices(ctx);

        if let Some(rx) = self.net_rx.as_mut() {
            while let Ok(ev) = rx.try_recv() {
                let eew_for_tts = match &ev {
                    NetEvent::Eew(report) => Some(report.clone()),
                    _ => None,
                };
                if let NetEvent::Eew(ref report) = ev {
                    // 发震过期：不播预警音（仍可进列表供回看）
                    let alert_ok = !origin_is_stale(report.origin_ms);
                    let splash = self.splash.active();
                    let is_jma = agency_family(&report.agency.0) == AgencyFamily::Jma;
                    // 与 cinema / 侧栏同键：源槽分组 + event_id（勿用原始 agency，否则镜像各播一次首报）
                    let eew_key = cinema_event_key(report);
                    if let Some(audio) = self.audio.as_mut() {
                        if !alert_ok || splash {
                            // 过期 / 启动灌入：只登记不播
                            audio.seed_eew_seen(&eew_key, report.serial);
                            if report.is_warn {
                                self.eew_alert_played.insert(eew_key.clone());
                            }
                        } else {
                            let pack = if is_jma { "srev" } else { "eewcn" };
                            let play_caution = is_jma
                                && report.intensity_kind == IntensityKind::JmaShindo
                                && report.intensity_level >= 3
                                && !report.is_assumption;
                            audio.on_eew(
                                &eew_key,
                                report.serial,
                                report.is_final,
                                report.is_cancel,
                                report.is_warn,
                                play_caution,
                                pack,
                                report.magnitude,
                                report.intensity_level,
                                report.is_assumption || report.magnitude < 0.0,
                            );
                            // JMA：预报升级为警报 → NHK 紧急地震速报音
                            if self.cfg.audio.jma_eew_alert_sound
                                && is_jma
                                && report.is_warn
                                && !report.is_cancel
                                && !self.eew_alert_played.contains(&eew_key)
                            {
                                audio.play(AudioCue::JmaEewAlert);
                                self.eew_alert_played.insert(eew_key.clone());
                            }
                            // 预警震度 >=6 弱 → NHK 一级新闻铃
                            if self.cfg.audio.nhk_news_bell
                                && report.intensity_kind == IntensityKind::JmaShindo
                                && report.intensity_level >= 7
                                && !report.is_cancel
                                && !self.nhk_bell_played.contains(&eew_key)
                            {
                                audio.play(AudioCue::NhkNewsBell);
                                self.nhk_bell_played.insert(eew_key.clone());
                            }
                        }
                    }
                    // 系统通知：预警
                    if alert_ok
                        && !splash
                        && self.cfg.notify.enabled
                        && self.cfg.notify.on_eew
                        && !report.is_cancel
                    {
                        let title = if report.is_warn {
                            "紧急地震速报（警报）"
                        } else {
                            "紧急地震速报"
                        };
                        let shindo = if report.intensity_kind == IntensityKind::JmaShindo {
                            jma_shindo_text(report.intensity_level)
                        } else {
                            ""
                        };
                        let body = if shindo.is_empty() {
                            format!(
                                "{}  M{:.1}  第{}报",
                                report.place, report.magnitude, report.serial
                            )
                        } else {
                            format!(
                                "{}  M{:.1}  震度{}  第{}报",
                                report.place, report.magnitude, shindo, report.serial
                            )
                        };
                        notify::show(title, &body);
                    }
                }
                starquake_net::apply_event_filtered(
                    &mut self.snap,
                    ev,
                    Some(&self.cfg.filters),
                );
                // 预警 TTS：侧栏展示条 + 内容/文案去重。
                // 启动界面未结束：只登记基线，不朗读；消失后才播后续更新。
                if let Some(report) = eew_for_tts {
                    if self.cfg.audio.tts_enabled
                        && !self.cfg.audio.mute
                        && !report.is_cancel
                        && !origin_is_stale(report.origin_ms)
                    {
                        if let Some((_, shown)) = eew_sidebar_entries(&self.snap.eew_list)
                            .into_iter()
                            .find(|(_, e)| same_eew_event(e, &report))
                        {
                            let accepted = shown.serial == report.serial
                                && (shown.magnitude - report.magnitude).abs() < 0.05
                                && shown.agency.0 == report.agency.0;
                            if accepted {
                                let slot = eew_sidebar_group_key(&shown.agency.0);
                                let script = build_eew_tts_script(shown);
                                let same_script = self
                                    .last_eew_tts_script
                                    .get(&slot)
                                    .is_some_and(|s| s == &script);
                                let last = self.last_eew_tts.get(&slot);
                                if !same_script && should_play_eew_tts(last, shown) {
                                    if !self.splash.active() {
                                        speak_async(script.clone());
                                    }
                                    self.last_eew_tts.insert(slot.clone(), shown.clone());
                                    self.last_eew_tts_script.insert(slot, script);
                                }
                            }
                        }
                    }
                }
            }
        }

        // 速报：Open EEW CN newrecord + 可选 TTS（启动界面丢弃，避免冷启动灌入）
        if let Some(rec) = self.snap.pending_tts_record.take() {
            if !self.splash.active() {
                if let Some(audio) = self.audio.as_mut() {
                    audio.play(AudioCue::NewRecord);
                    // 情报震度 >=6 弱 → NHK 一级新闻铃
                    if self.cfg.audio.nhk_news_bell
                        && rec.intensity_kind == IntensityKind::JmaShindo
                        && rec.intensity_level >= 7
                    {
                        let rk = format!("{}|{}|{:.4}|{:.4}|{}", rec.agency.0, rec.origin_ms, rec.latitude, rec.longitude, rec.intensity_level);
                        if !self.nhk_bell_played.contains(&rk) {
                            audio.play(AudioCue::NhkNewsBell);
                            self.nhk_bell_played.insert(rk);
                        }
                    }
                }
                if self.cfg.audio.tts_enabled && !self.cfg.audio.mute {
                    speak_async(build_record_tts_script(&rec));
                }
                if self.cfg.notify.enabled && self.cfg.notify.on_record {
                    let shindo = if rec.intensity_kind == IntensityKind::JmaShindo {
                        jma_shindo_text(rec.intensity_level)
                    } else {
                        ""
                    };
                    let body = if shindo.is_empty() {
                        format!("{}  M{:.1}", rec.place, rec.magnitude)
                    } else {
                        format!("{}  M{:.1}  震度{}", rec.place, rec.magnitude, shindo)
                    };
                    notify::show("地震情报", &body);
                }
            }
        }

        // 同步自定义震级传播表到快照（仅变更时写入，避免每帧分配+排序）
        {
            let mut next: Vec<(f64, f64, f64)> = self
                .cfg
                .travel
                .mag_spread_km
                .iter()
                .map(|p| (p.min_magnitude, p.max_magnitude, p.max_km))
                .collect();
            next.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if self.snap.wave_spread_km != next {
                self.snap.wave_spread_km = next;
            }
        }

        self.tick_health_audio();
        self.tick_tsunami_audio();
        self.tick_typhoon_update();

        // 启动：稍等快照汇聚后优先跟有效预警；没有则跟最新速报。
        // 注意：eew_list 里常有已过期历史项，不能仅用「列表非空」卡住 pending，
        // 否则相机一直停在默认东亚视角，侧栏速报已选中也不会飞过去。
        if self.auto_center_pending {
            let elapsed = self.started.elapsed().as_secs_f64();
            if elapsed >= 1.2 {
                if let Some((lon, lat, _zoom)) = self.snap.focus_latest_event() {
                    let z = if self.snap.active.is_some() {
                        gq_jump_zoom()
                    } else {
                        8.5
                    };
                    self.shell.map.fly_to(lon, lat, Some(z), 0.9);
                    self.auto_center_pending = false;
                    if let Some(ev) = &self.snap.active {
                        self.last_follow_key = Some((ev.event_id.clone(), ev.serial));
                    }
                } else if let Some((lon, lat, zoom, key)) = self.snap.focus_latest_record() {
                    // 速报对准震中，勿用球体日本构图东偏（否则堪察加会被拉成北太平洋全景）
                    self.shell.map.fly_to_centered(lon, lat, Some(zoom), 1.0);
                    self.last_record_follow_key = Some(key);
                    self.auto_center_pending = false;
                } else if elapsed >= 4.0 {
                    // 仍无坐标数据：放弃启动跟焦，交给后续实时跟焦
                    self.auto_center_pending = false;
                }
            }
        } else if self.cfg.ui.auto_follow_eew {
            // 拖拽空闲 15s：解除镜头锁并恢复跟焦（对齐 GQ cinemaModeReenable）
            let _ = self.shell.map.clear_idle_user_camera(15.0);
            // 多震轮播：新震立刻跳；到点切换；更新报只刷数据，不打断停留计时
            // 注意：upsert_eew 已先写入 active，不能用 active==live 判断「是否新震」
            let browsing_station = self
                .snap
                .active
                .as_ref()
                .is_some_and(|e| e.event_id.starts_with("station:"));
            let browsing_record = self
                .snap
                .active
                .as_ref()
                .is_some_and(|e| e.event_id.starts_with("record:"));
            // 用户点选速报并锁镜头时，勿把 active/Tab 抢回预警（新预警到达仍强制打断）
            let hold_record_pick =
                browsing_record && self.shell.map.user_camera_override;
            let now = chrono::Utc::now().timestamp_millis();
            // 跟焦/轮播：传播中独立地震全进同一队列（一源多震 + 多源多震），按发震旧→新顺序轮切
            // 左上框仍每源一框；仅合并同事件镜像
            let live: Vec<&EewReport> = {
                let raw: Vec<&EewReport> = self
                    .snap
                    .eew_list
                    .iter()
                    .filter(|e| {
                        eew_wave_live(e, now, &self.snap.wave_spread_km, self.travel.as_ref())
                    })
                    .collect();
                dedupe_same_eew_events(&raw)
            };
            // 新预警必须打断测站浏览；仅轮播停留时保留测站锁定
            let allow_follow = !browsing_station || !live.is_empty();
            if allow_follow {
                let live = live;
                let seen_key = live
                    .iter()
                    .map(|e| cinema_event_key(e))
                    .collect::<Vec<_>>()
                    .join(";");
                if live.is_empty() {
                    self.cinema_seen_key.clear();
                    self.cinema_prev_live.clear();
                    self.last_follow_cinema = None;
                } else if live.len() == 1 {
                    let ev = live[0];
                    let key = (ev.event_id.clone(), ev.serial);
                    let ck = cinema_event_key(ev);
                    // 勿用 active 判新震（upsert 已写入）；以 cinema 键 + 上一帧队列为准
                    let is_new_event = self.last_follow_cinema.as_ref() != Some(&ck)
                        && !self
                            .cinema_prev_live
                            .iter()
                            .any(|p| same_eew_event(p, ev));
                    let active_is_this_eew = self
                        .snap
                        .active
                        .as_ref()
                        .is_some_and(|a| is_eew_camera_target(a) && same_eew_event(a, ev));
                    if is_new_event {
                        // 新预警：默认飞跃；震中已在近距则只切 active，保持缩放（防相近源反复拉 Z12）
                        let prev = self.snap.active.as_ref().filter(|a| is_eew_camera_target(a));
                        if !skip_eew_camera_jump(&self.shell.map, ev, prev) {
                            let z = gq_jump_zoom();
                            self.shell.map.fly_to(ev.longitude, ev.latitude, Some(z), 0.9);
                            self.shell.map.user_camera_override = false;
                            self.shell.map.last_user_camera_at = None;
                        }
                        self.last_follow_key = Some(key);
                        self.last_follow_cinema = Some(ck);
                        self.cinema_idx = 0;
                        self.cinema_seen_key = seen_key;
                        self.snap.active = Some(ev.clone());
                        self.snap.tab = starquake_core::SidebarTab::Eew;
                        self.snap.overlay_mode = starquake_core::OverlayMode::Wave;
                    } else if hold_record_pick {
                        // 保留用户点选的速报镜头
                    } else if self.last_follow_key.as_ref() != Some(&key) || !active_is_this_eew
                    {
                        // 同震更新报：只刷 active / 侧栏，绝不 fly_to
                        self.last_follow_key = Some(key);
                        self.last_follow_cinema = Some(ck);
                        self.snap.active = Some(ev.clone());
                        self.snap.tab = starquake_core::SidebarTab::Eew;
                        self.snap.overlay_mode = starquake_core::OverlayMode::Wave;
                    }
                    self.cinema_prev_live = live.iter().map(|e| (*e).clone()).collect();
                } else {
                    let mut jump_idx: Option<usize> = None;
                    let mut is_new_arrival = false;
                    if seen_key != self.cinema_seen_key {
                        let old: std::collections::HashSet<&str> = self
                            .cinema_seen_key
                            .split(';')
                            .filter(|s| !s.is_empty())
                            .collect();
                        // 新震：键未见过，且与上一帧队列无 same_eew_event（防换源假新震）
                        for (i, e) in live.iter().enumerate() {
                            let id = cinema_event_key(e);
                            if old.contains(id.as_str()) {
                                continue;
                            }
                            let known = self.cinema_prev_live.iter().any(|p| same_eew_event(p, e));
                            if !known {
                                jump_idx = Some(i);
                                is_new_arrival = true;
                                break;
                            }
                        }
                        if jump_idx.is_none() {
                            // 集合变化（有震结束 / 仅换 id）：钉住当前事件，不飞跃
                            let cur_key = self
                                .last_follow_cinema
                                .clone()
                                .or_else(|| {
                                    self.snap
                                        .active
                                        .as_ref()
                                        .filter(|a| is_eew_camera_target(a))
                                        .map(cinema_event_key)
                                })
                                .or_else(|| {
                                    self.last_follow_key.as_ref().and_then(|(id, _)| {
                                        live.iter()
                                            .find(|e| e.event_id == *id)
                                            .map(|e| cinema_event_key(e))
                                    })
                                })
                                .unwrap_or_default();
                            if let Some(i) =
                                live.iter().position(|e| cinema_event_key(e) == cur_key)
                            {
                                self.cinema_idx = i;
                            } else if let Some(i) = self.snap.active.as_ref().and_then(|a| {
                                live.iter().position(|e| same_eew_event(e, a))
                            }) {
                                self.cinema_idx = i;
                            } else {
                                // 当前震已结束：切到队列内最近一项，但不标 new_arrival（不强制解锁）
                                jump_idx = Some(self.cinema_idx.min(live.len() - 1));
                            }
                        }
                        self.cinema_seen_key = seen_key;
                    } else if !self.shell.map.user_camera_override
                        && Instant::now() >= self.cinema_next_at
                    {
                        // 按队列顺序 +1 轮播（跨源、同源同一圈）
                        jump_idx = Some((self.cinema_idx + 1) % live.len());
                    }
                    if let Some(idx) = jump_idx {
                        if is_new_arrival || !self.shell.map.user_camera_override {
                            let ev = live[idx];
                            let prev = self
                                .snap
                                .active
                                .as_ref()
                                .filter(|a| is_eew_camera_target(a))
                                .or_else(|| {
                                    live.get(self.cinema_idx.min(live.len().saturating_sub(1)))
                                        .copied()
                                });
                            let near = skip_eew_camera_jump(&self.shell.map, ev, prev);
                            if !near {
                                let z = gq_jump_zoom();
                                self.shell.map.fly_to(ev.longitude, ev.latitude, Some(z), 0.9);
                                if is_new_arrival {
                                    self.shell.map.user_camera_override = false;
                                    self.shell.map.last_user_camera_at = None;
                                }
                            }
                            self.last_follow_key = Some((ev.event_id.clone(), ev.serial));
                            self.last_follow_cinema = Some(cinema_event_key(ev));
                            self.cinema_idx = idx;
                            // GlobalQuake cinemaModeSwitchTime 默认 10 秒
                            self.cinema_next_at =
                                Instant::now() + std::time::Duration::from_secs_f64(10.0);
                            self.snap.active = Some(ev.clone());
                            self.snap.tab = starquake_core::SidebarTab::Eew;
                            self.snap.overlay_mode = starquake_core::OverlayMode::Wave;
                        }
                    } else if !hold_record_pick {
                        // 左上框点选 / 侧栏选中某条 live 时，跟齐 cinema 索引并重置停留
                        if let Some(a) = self.snap.active.as_ref() {
                            if let Some(i) = live.iter().position(|e| same_eew_event(e, a)) {
                                if i != self.cinema_idx {
                                    self.cinema_idx = i;
                                    self.cinema_next_at = Instant::now()
                                        + std::time::Duration::from_secs_f64(10.0);
                                }
                            }
                        }
                        // 停留中：同步当前震中最新报文，绝不因 serial 重置计时或跳转
                        let idx = self.cinema_idx.min(live.len() - 1);
                        let ev = live[idx];
                        let key = (ev.event_id.clone(), ev.serial);
                        let ck = cinema_event_key(ev);
                        if self.last_follow_key.as_ref() != Some(&key)
                            || self.snap.active.as_ref().map(|a| a.serial) != Some(ev.serial)
                            || !self
                                .snap
                                .active
                                .as_ref()
                                .is_some_and(|a| same_eew_event(a, ev))
                        {
                            self.last_follow_key = Some(key);
                            self.last_follow_cinema = Some(ck);
                            self.snap.active = Some(ev.clone());
                            self.snap.tab = starquake_core::SidebarTab::Eew;
                            self.snap.overlay_mode = starquake_core::OverlayMode::Wave;
                        }
                    }
                    self.cinema_prev_live = live.iter().map(|e| (*e).clone()).collect();
                }
            }
        }
        // 海啸预报到期自动清除（注意报/警报等靠解除报文）
        {
            let now = chrono::Utc::now().timestamp_millis();
            self.snap.prune_expired_tsunami(now);
            self.snap.prune_expired_weather(now);
        }
        self.tick_local_hypo();

        // 默认视角：无预警时始终跟最新速报（新速报强制飞，无视拖拽锁）；有预警则上面 cinema 已跟
        if !self.auto_center_pending && self.started.elapsed().as_secs_f64() >= 2.5 {
            let _ = self.shell.map.clear_idle_user_camera(15.0);
            let now = chrono::Utc::now().timestamp_millis();
            let eew_live = self.snap.eew_list.iter().any(|e| {
                eew_wave_live(e, now, &self.snap.wave_spread_km, self.travel.as_ref())
            });
            let browsing_station = self
                .snap
                .active
                .as_ref()
                .is_some_and(|e| e.event_id.starts_with("station:"));
            if eew_live {
                self.snap.pending_record_fly = None;
                self.last_record_follow_key = None;
            } else if !browsing_station {
                // 新速报 pending：强制飞跃并解锁（对齐「收到即跳」）
                if let Some((lon, lat, zoom)) = self.snap.pending_record_fly.take() {
                    if lat.abs() > 0.01 || lon.abs() > 0.01 {
                        self.shell.map.fly_to_centered(lon, lat, Some(zoom), 1.0);
                        self.shell.map.user_camera_override = false;
                        self.shell.map.last_user_camera_at = None;
                        if let Some((_, _, _, key)) = self.snap.latest_record_camera() {
                            let _ = self.snap.focus_latest_record();
                            self.last_record_follow_key = Some(key);
                        }
                    }
                }
                // 无预警且未锁镜头：列表头变化时跟焦最新速报
                if !self.shell.map.user_camera_override {
                    if let Some((lon, lat, zoom, key)) = self.snap.latest_record_camera() {
                        if self.last_record_follow_key.as_ref() != Some(&key) {
                            let _ = self.snap.focus_latest_record();
                            self.shell.map.fly_to_centered(lon, lat, Some(zoom), 1.0);
                            self.last_record_follow_key = Some(key);
                        }
                    }
                }
            }
        } else {
            self.snap.pending_record_fly = None;
        }

        if let Some(ev) = self.snap.active.as_ref() {
            let is_eew = !ev.event_id.starts_with("record:")
                && !ev.event_id.starts_with("station:");
            if is_eew {
                let elapsed = if ev.origin_ms > 0 {
                    let now = chrono::Utc::now().timestamp_millis();
                    ((now - ev.origin_ms) as f64 / 1000.0).max(0.0)
                } else {
                    self.started.elapsed().as_secs_f64()
                };
                refresh_countdown(
                    &mut self.snap,
                    self.travel.as_ref(),
                    self.local_lat,
                    self.local_lon,
                    Some(elapsed),
                );
            } else {
                clear_countdown(&mut self.snap);
            }
        }

        // 地牛倒计时：本机位置有效、开关打开、启动画面结束后才播
        // S 波抵达音：有本机位置且倒计时会话有效时，剩余首次到 0 播 swavearrive
        if let Some(audio) = self.audio.as_mut() {
            let intense_ok = !self.cfg.audio.countdown_only_intense || countdown_is_intense(&self.snap);
            let remain = if self.has_local
                && self.cfg.audio.play_countdown
                && !self.splash.active()
                && intense_ok
            {
                self.snap.countdown_s
            } else {
                None
            };
            let arrive_remain = if self.has_local && !self.splash.active() {
                self.snap.countdown_s
            } else {
                None
            };
            audio.tick_s_arrive(arrive_remain);
            audio.tick_countdown(
                remain,
                self.cfg.audio.countdown_start,
                self.cfg.audio.countdown_speech,
            );
        }

        // 托盘隐藏 / 任务栏最小化：跳过地图与面板绘制，避免后台仍按帧烧内存。
        if backgrounded {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }

        self.shell.settings.official_notices = self
            .official_notices
            .items
            .iter()
            .map(|n| OfficialNoticeView {
                id: n.id,
                title: n.title.clone(),
                content: n.content.clone(),
                type_label: n.type_label().to_string(),
                created_at: n.created_at_display(),
                unread: !self.official_notices.is_read(n.id),
            })
            .collect();
        let (ui_action, persist_cfg) = self.shell.ui(
            ctx,
            &mut self.snap,
            self.travel.as_ref(),
            &mut self.cfg,
        );
        if persist_cfg {
            if let Err(e) = self.cfg.save_user_overlay() {
                tracing::warn!("保存侧栏/图例偏好失败: {e:#}");
            }
        }
        if let Some(action) = ui_action {
            match action {
                SettingsAction::Saved { cfg, close } => {
                    match self.apply_saved_config(cfg) {
                        Ok((path, renderer_changed)) => {
                            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                                if self.cfg.ui.always_on_top {
                                    egui::WindowLevel::AlwaysOnTop
                                } else {
                                    egui::WindowLevel::Normal
                                },
                            ));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title()));
                            self.snap.show_wave_rings = self.cfg.ui.show_wave_rings;
                            let mut note = format!("已保存到 {}", path.display());
                            if renderer_changed {
                                note.push_str("；渲染设备需重启生效，其余已热重载");
                            }
                            self.shell.settings.set_save_note(note);
                            if close {
                                self.shell.settings.close_after_save();
                            }
                        }
                        Err(e) => self
                            .shell
                            .settings
                            .set_save_note(format!("保存失败：{e}")),
                    }
                }
                SettingsAction::TestSound {
                    mute,
                    master_volume,
                    countdown_volume,
                    event_pack,
                    countdown_pack,
                } => {
                    if let Some(audio) = self.audio.as_mut() {
                        audio.set_mute(mute);
                        audio.set_volumes(master_volume, countdown_volume);
                        audio.set_packs(&event_pack, &countdown_pack);
                        audio.play(AudioCue::Issue);
                        // 试听后恢复已应用配置，避免草稿音量「偷改」运行态
                        audio.set_mute(self.cfg.audio.mute);
                        audio.set_volumes(
                            self.cfg.audio.master_volume,
                            self.cfg.audio.countdown_volume,
                        );
                        audio.set_packs(
                            &self.cfg.audio.event_pack,
                            &self.cfg.audio.countdown_pack,
                        );
                    }
                }
                SettingsAction::TestTts {
                    engine,
                    offline_voice,
                    edge_voice,
                    rate,
                    volume,
                } => {
                    speak_with_config(
                        "StarQuake 语音播报测试。",
                        TtsRuntimeConfig {
                            engine: TtsEngine::parse(&engine),
                            offline_voice,
                            edge_voice,
                            rate,
                            volume,
                        },
                    );
                }
                SettingsAction::TestNotify => {
                    notify::show_test();
                    self.shell
                        .settings
                        .set_save_note("已发送系统通知测试（请查看通知中心）");
                }
                SettingsAction::Cancelled => {}
                SettingsAction::ClearTileCache => {
                    self.shell.map.clear_tile_cache();
                    self.shell
                        .settings
                        .set_save_note("已清除内存瓦片缓存");
                }
                SettingsAction::OpenLogDir => {
                    let dir = logging::resolve_log_dir(&self.resource_root, &self.cfg.log);
                    match logging::open_log_dir(&dir) {
                        Ok(()) => self
                            .shell
                            .settings
                            .set_save_note(format!("已打开日志目录 {}", dir.display())),
                        Err(e) => self
                            .shell
                            .settings
                            .set_save_note(format!("打开日志目录失败：{e}")),
                    }
                }
                SettingsAction::CheckForUpdate => {
                    self.begin_update_check(true);
                }
                SettingsAction::MarkNoticeRead(id) => {
                    self.official_notices.mark_read(id);
                }
                SettingsAction::MarkAllNoticesRead => {
                    self.official_notices.mark_all_read();
                }
            }
        }

        // 启动画面：底图可用后淡出关闭（最短展示 + 约 0.55s 淡出）
        let load = self.shell.map.load_state();
        if self.splash.tick(load) {
            let theme = AppUiTheme::from_config(&self.cfg.ui.theme);
            draw_splash(
                ctx,
                &theme.palette(),
                self.shell.map.load_status(),
                load,
                self.splash.opacity(),
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        self.draw_update_dialog(ctx);
        self.draw_official_notice_popup(ctx);

        // 有指针活动立即重绘，避免空闲降频把点击「吃掉」
        let pointer_busy = ctx.input(|i| {
            i.pointer.any_down()
                || i.pointer.button_clicked(egui::PointerButton::Primary)
                || i.pointer.button_released(egui::PointerButton::Primary)
                || i.pointer.button_clicked(egui::PointerButton::Secondary)
                || i.smooth_scroll_delta.length_sq() > 0.0
        });
        let now_ms = chrono::Utc::now().timestamp_millis();
        let waves_busy = self.snap.show_wave_rings
            && self.snap.eew_list.iter().any(|e| {
                eew_wave_live(e, now_ms, &self.snap.wave_spread_km, self.travel.as_ref())
            });
        let need = if pointer_busy
            || self.shell.map.wants_anim_repaint()
            || self.splash.active()
        {
            RepaintNeed::Interactive
        } else if waves_busy {
            RepaintNeed::Waves
        } else if self.shell.settings.open || self.snap.countdown_s.is_some() {
            RepaintNeed::Soft
        } else {
            RepaintNeed::Idle
        };
        let fps_ms = repaint_ms(&self.cfg.performance, need);
        ctx.request_repaint_after(std::time::Duration::from_millis(fps_ms));
    }
}

impl StarQuakeApp {
    fn capture_main_hwnd(&self, frame: &eframe::Frame) {
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = frame.window_handle() {
                if let RawWindowHandle::Win32(w) = handle.as_raw() {
                    tray::set_main_hwnd(w.hwnd.get() as isize);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = frame;
        }
    }

    fn tick_tray(&mut self, ctx: &egui::Context) {
        if tray::take_force_quit() {
            self.force_quit = true;
        }
        // 托盘命令（对齐滚动字幕：Show→showNormal，Quit→强制退出）
        while let Some(cmd) = self.tray.as_ref().and_then(|t| t.poll()) {
            match cmd {
                tray::TrayCmd::Show => {
                    self.hidden_to_tray = false;
                    tray::set_hidden_to_tray(false);
                    tray::win_show_normal();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                }
                tray::TrayCmd::Quit => {
                    self.force_quit = true;
                    self.hidden_to_tray = false;
                    tray::set_hidden_to_tray(false);
                    tray::win_show_normal();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // 关闭窗口 → 隐藏到托盘（对齐滚动字幕 closeEvent：ignore + hide + showMessage）
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested
            && !self.force_quit
            && self.cfg.ui.minimize_to_tray
            && self.tray.is_some()
            && !self.hidden_to_tray
        {
            self.hidden_to_tray = true;
            tray::set_hidden_to_tray(true);
            self.shell.map.clear_tile_cache();
            self.bg_tiles_cleared = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            tray::win_hide();
            let _ = tray::show_balloon(
                "StarQuake",
                "\u{7a0b}\u{5e8f}\u{5df2}\u{6700}\u{5c0f}\u{5316}\u{5230}\u{7cfb}\u{7edf}\u{6258}\u{76d8}\u{ff0c}\u{4ecd}\u{5728}\u{540e}\u{53f0}\u{63a5}\u{6536}\u{5730}\u{9707}\u{4fe1}\u{606f}\u{3002}",
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn tick_official_notices(&mut self, ctx: &egui::Context) {
        if self.splash.active() {
            return;
        }
        if let Some(rx) = self.notice_rx.as_ref() {
            match rx.try_recv() {
                Ok(Ok(items)) => {
                    self.notice_fetching = false;
                    self.notice_rx = None;
                    self.official_notices.apply_fetched(items);
                    ctx.request_repaint();
                }
                Ok(Err(e)) => {
                    self.notice_fetching = false;
                    self.notice_rx = None;
                    tracing::debug!("官方通知拉取失败: {e}");
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.notice_fetching = false;
                    self.notice_rx = None;
                }
            }
        }
        if self.notice_fetching {
            return;
        }
        if !self.official_notices.needs_poll() {
            return;
        }
        self.notice_fetching = true;
        let (tx, rx) = mpsc::channel();
        self.notice_rx = Some(rx);
        std::thread::spawn(move || {
            let res = fetch_notices().map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
        });
    }

    fn draw_official_notice_popup(&mut self, ctx: &egui::Context) {
        let Some(n) = self.official_notices.popup_queue.first().cloned() else {
            return;
        };
        let mut close = false;
        egui::Window::new(format!("{} · {}", n.type_label(), n.title))
            .id(egui::Id::new("official_notice_popup"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(320.0);
                ui.set_max_width(420.0);
                let created_disp = n.created_at_display();
                if !created_disp.is_empty() {
                    ui.label(
                        egui::RichText::new(created_disp)
                            .size(11.0)
                            .color(egui::Color32::GRAY),
                    );
                    ui.add_space(4.0);
                }
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    ui.label(&n.content);
                });
                ui.add_space(10.0);
                if ui.button("知道了").clicked() {
                    close = true;
                }
            });
        if close {
            self.official_notices.mark_read(n.id);
        }
    }


    fn begin_update_install(&mut self, info: AvailableUpdate) {
        if self.update_busy {
            return;
        }
        self.update_available = None;
        self.update_busy = true;
        self.update_status = "正在下载更新…".into();
        self.update_progress = Some((0, None));
        let (tx, rx) = mpsc::channel();
        let (ptx, prx) = mpsc::channel();
        self.update_rx = Some(rx);
        self.update_progress_rx = Some(prx);
        std::thread::spawn(move || {
            let progress = move |done: u64, total: Option<u64>| {
                let _ = ptx.send((done, total));
            };
            match download_asset_with_progress(&info, Some(&progress)) {
                Ok(path) => {
                    let _ = tx.send(Err("__INSTALLING__".into()));
                    if let Err(e) = apply_and_relaunch(&path) {
                        let _ = tx.send(Err(format!("应用更新失败：{e:#}")));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("下载失败：{e:#}")));
                }
            }
        });
    }

    fn begin_update_check(&mut self, manual: bool) {
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        self.update_checked = true;
        self.update_check_manual = manual;
        self.update_status = if manual {
            "正在检查更新…".into()
        } else {
            String::new()
        };
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        let local = starquake_ui::APP_VERSION_DISPLAY.to_string();
        std::thread::spawn(move || {
            let res = check_latest(&local).map_err(|e| format!("{e:#}"));
            let _ = tx.send(res);
        });
    }

    fn tick_update_check(&mut self, ctx: &egui::Context) {
        if let Some(info) = self.update_auto_install.take() {
            if self.cfg.update.auto_install {
                self.begin_update_install(info);
            } else {
                open_release_page(&info.html_url);
            }
        }
        // 启动前已检查过则不再自动查；设置里「检查更新」仍走 begin_update_check(true)
        if !self.update_checked && !self.splash.active() && self.cfg.update.check_on_startup {
            self.begin_update_check(false);
        }
        if let Some(prx) = self.update_progress_rx.as_ref() {
            while let Ok(p) = prx.try_recv() {
                self.update_progress = Some(p);
                ctx.request_repaint();
            }
        }
        let Some(rx) = self.update_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Some(info))) => {
                self.update_busy = false;
                self.update_rx = None;
                self.update_progress_rx = None;
                self.update_progress = None;
                self.update_status.clear();
                self.update_available = Some(info);
                ctx.request_repaint();
            }
            Ok(Ok(None)) => {
                self.update_busy = false;
                self.update_rx = None;
                self.update_progress_rx = None;
                self.update_progress = None;
                self.update_available = None;
                // 启动静默：无更新不弹窗；手动检查才提示「已是最新」
                self.update_status = if self.update_check_manual {
                    "已是最新版本".into()
                } else {
                    String::new()
                };
                ctx.request_repaint();
            }
            Ok(Err(e)) => {
                if e == "__INSTALLING__" {
                    self.update_status = "正在安装更新…".into();
                    self.update_progress = None;
                    ctx.request_repaint();
                    return;
                }
                self.update_busy = false;
                self.update_rx = None;
                self.update_progress_rx = None;
                self.update_progress = None;
                if e.contains("更新失败") || e.contains("下载") || e.contains("应用") {
                    self.update_status = e;
                } else if self.update_check_manual {
                    tracing::warn!("检查更新失败: {e}");
                    self.update_status = format!("检查更新失败：{e}");
                } else {
                    tracing::warn!("启动检查更新失败: {e}");
                    self.update_status.clear();
                }
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.update_busy = false;
                self.update_rx = None;
            }
        }
    }

    fn draw_update_dialog(&mut self, ctx: &egui::Context) {
        if self.update_busy && self.update_available.is_none() {
            if self.update_status.is_empty() {
                return;
            }
            egui::Window::new("StarQuake 更新")
                .id(egui::Id::new("starquake_update_busy"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    ui.label(egui::RichText::new(&self.update_status).size(16.0));
                    ui.add_space(10.0);
                    if let Some((done, total)) = self.update_progress {
                        if let Some(total) = total.filter(|t| *t > 0) {
                            let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
                            let bar = egui::ProgressBar::new(frac)
                                .text(format!("{:.0}%", frac * 100.0));
                            ui.add(bar);
                            ui.label(format!(
                                "{:.1} / {:.1} MB",
                                done as f64 / 1_048_576.0,
                                total as f64 / 1_048_576.0
                            ));
                        } else {
                            ui.add(egui::ProgressBar::new(0.0).animate(true));
                            ui.label(format!("{:.1} MB", done as f64 / 1_048_576.0));
                        }
                    } else if self.update_status.contains("安装") {
                        ui.add(egui::ProgressBar::new(0.0).animate(true));
                    }
                });
            ctx.request_repaint();
            return;
        }

        if self.update_available.is_none() {
            if self.update_status.is_empty() {
                return;
            }
            // 最新 / 失败提示
            let mut clear = false;
            egui::Window::new("StarQuake")
                .id(egui::Id::new("starquake_update_note"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 48.0])
                .show(ctx, |ui| {
                    ui.label(&self.update_status);
                    if ui.button("知道了").clicked() {
                        clear = true;
                    }
                });
            if clear {
                self.update_status.clear();
            }
            return;
        }

        let info = self.update_available.as_ref().unwrap().clone();
        let mut dismiss = false;
        let mut do_install = false;
        let mut open_page = false;
        egui::Window::new(format!("发现新版本 {}", info.version))
            .id(egui::Id::new("starquake_update_prompt"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                if !info.name.is_empty() {
                    ui.label(egui::RichText::new(&info.name).strong());
                    ui.add_space(4.0);
                }
                let body = info.body.trim();
                if !body.is_empty() {
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        ui.label(body);
                    });
                    ui.add_space(8.0);
                }
                ui.label(format!("资源：{}", info.asset_name));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new("立即更新")
                                .fill(egui::Color32::from_rgb(0x25, 0x63, 0xEB)),
                        )
                        .clicked()
                    {
                        do_install = true;
                    }
                    if ui.button("打开发布页").clicked() {
                        open_page = true;
                    }
                    if ui.button("稍后").clicked() {
                        dismiss = true;
                    }
                });
            });

        if open_page {
            open_release_page(&info.html_url);
        }
        if dismiss {
            // 与启动前「否」一致：记下版本，避免下次启动再弹同一版
            self.cfg.update.last_dismissed_update_version = info.version.clone();
            if let Err(e) = self.cfg.save_user_overlay() {
                tracing::warn!("保存已跳过更新版本失败: {e:#}");
            }
            self.update_available = None;
            self.update_status.clear();
        }
        if do_install {
            if !self.cfg.update.auto_install {
                open_release_page(&info.html_url);
                self.update_available = None;
                return;
            }
            self.begin_update_install(info);
        }
    }

    fn tick_window_geometry(&mut self, ctx: &egui::Context) {
        let (w, h, x, y, maximized) = ctx.input(|i| {
            let v = i.viewport();
            let inner = v.inner_rect.unwrap_or(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(self.cfg.window.width, self.cfg.window.height),
            ));
            let outer = v.outer_rect;
            let (x, y) = outer
                .map(|r| (Some(r.min.x), Some(r.min.y)))
                .unwrap_or((self.cfg.window.x, self.cfg.window.y));
            (
                inner.width().round(),
                inner.height().round(),
                x.map(|v| v.round()),
                y.map(|v| v.round()),
                v.maximized.unwrap_or(false),
            )
        });
        if w < 200.0 || h < 150.0 {
            return;
        }
        let geom = (w, h, x, y, maximized);
        if self.last_window_geom.as_ref() == Some(&geom) {
            if let Some(at) = self.window_geom_save_at {
                if at.elapsed() >= std::time::Duration::from_millis(600) {
                    self.window_geom_save_at = None;
                    let same = (self.cfg.window.width - w).abs() < 0.5
                        && (self.cfg.window.height - h).abs() < 0.5
                        && self.cfg.window.x == x
                        && self.cfg.window.y == y
                        && self.cfg.window.maximized == maximized;
                    if same {
                        return;
                    }
                    self.cfg.window.width = w;
                    self.cfg.window.height = h;
                    self.cfg.window.x = x;
                    self.cfg.window.y = y;
                    self.cfg.window.maximized = maximized;
                    if let Err(e) = self.cfg.save_user_overlay() {
                        tracing::warn!("保存窗口几何失败: {e:#}");
                    } else {
                        tracing::debug!(w, h, ?x, ?y, maximized, "窗口几何已保存");
                    }
                }
            }
            return;
        }
        self.last_window_geom = Some(geom);
        self.window_geom_save_at = Some(Instant::now());
    }

    /// 台风本轮有变更：提示音 + 飞到**变更的那条**（首包不飞；有预警时不抢镜头）。
    fn tick_typhoon_update(&mut self) {
        // 启动界面只登记 pending，消失后再播/飞
        if self.splash.active() {
            return;
        }
        let play_hint = self.snap.pending_typhoon_hint;
        self.snap.pending_typhoon_hint = false;
        let fly = self.snap.pending_typhoon_fly.take();
        if play_hint {
            if let Some(audio) = self.audio.as_mut() {
                audio.play(AudioCue::UiNotify);
            }
        }
        let Some((lon, lat, radius_km)) = fly else {
            return;
        };
        if !self.cfg.ui.show_typhoons || !self.cfg.sources.typhoon.enabled {
            return;
        }
        // 默认视角：有预警跟预警；无预警跟最新速报。预警传播中不抢台风镜头。
        let now = chrono::Utc::now().timestamp_millis();
        let eew_live = self.snap.eew_list.iter().any(|e| {
            eew_wave_live(e, now, &self.snap.wave_spread_km, self.travel.as_ref())
        });
        if eew_live {
            return;
        }
        // zoom<=3 不跟焦；用户拖拽锁定也不跟
        if self.shell.map.zoom <= 3.0 || self.shell.map.user_camera_override {
            return;
        }
        let z = zoom_to_fit_radius_km(
            lat,
            radius_km.max(1.0),
            self.shell.map.last_viewport_min_px,
            0.45,
        )
        .min(7.0);
        // 对准台风中心，勿走球体日本构图东偏（否则落点无标记）
        self.shell.map.fly_to_centered(lon, lat, Some(z), 1.0);
    }

    fn tick_health_audio(&mut self) {
        let cur = [
            self.snap.health_jian,
            self.snap.health_wolfx,
            self.snap.health_p2p,
            self.snap.health_pancakes,
            self.snap.health_fssn,
        ];
        let prev = self.prev_health;
        if prev == cur {
            return;
        }
        // 启动界面未结束：源接通不滴声，避免误扰
        if self.splash.active() {
            self.prev_health = cur;
            return;
        }
        let any_ok = |h: [HealthStatus; 5]| h.iter().any(|s| matches!(s, HealthStatus::Normal));
        let all_bad = |h: [HealthStatus; 5]| h.iter().all(|s| matches!(s, HealthStatus::Abnormal));
        if let Some(audio) = self.audio.as_mut() {
            if !any_ok(prev) && any_ok(cur) {
                audio.play(AudioCue::UiConnectOk);
            } else if !all_bad(prev) && all_bad(cur) {
                audio.play(AudioCue::UiConnectFail);
            }
        }
        self.prev_health = cur;
    }



    fn tick_local_hypo(&mut self) {
        if !self.cfg.ui.show_local_hypo {
            self.snap.local_hypo = None;
            return;
        }
        let Some(travel) = self.travel.as_ref() else {
            return;
        };
        if self.hypo_last_run.elapsed().as_millis() < 800 {
            return;
        }
        self.hypo_last_run = Instant::now();
        let now_sec = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        // Trigger threshold: instrumental intensity roughly felt
        const TRIG: f64 = 0.5;
        let mut alive = std::collections::HashSet::new();
        for s in &self.snap.stations {
            if !s.instrumental.is_finite() || s.instrumental < TRIG {
                continue;
            }
            if s.latitude.abs() <= 0.01 && s.longitude.abs() <= 0.01 {
                continue;
            }
            alive.insert(s.id.clone());
            self.hypo_first_trigger
                .entry(s.id.clone())
                .or_insert(now_sec);
        }
        self.hypo_first_trigger.retain(|id, _| alive.contains(id));
        let mut picks: Vec<ArrivalPick> = Vec::new();
        for s in &self.snap.stations {
            let Some(&det) = self.hypo_first_trigger.get(&s.id) else {
                continue;
            };
            picks.push(ArrivalPick {
                id: s.id.clone(),
                latitude: s.latitude,
                longitude: s.longitude,
                detect_sec: det,
            });
        }
        let cfg = LocateConfig::default();
        if picks.len() < cfg.min_picks {
            self.snap.local_hypo = None;
            return;
        }
        match locate(travel, &picks, &[], now_sec, &cfg) {
            Some(est) => {
                self.snap.local_hypo = Some(LocalHypoEstimate {
                    latitude: est.latitude,
                    longitude: est.longitude,
                    depth_km: est.depth_km,
                    error_level: est.error_level,
                    used_stations: est.used_pick_indices.len(),
                });
            }
            None => {
                self.snap.local_hypo = None;
            }
        }
    }

    fn tick_tsunami_audio(&mut self) {
        let key = match self.snap.tsunami.as_ref() {
            Some(t) if !t.cancelled => format!("{}|{}", t.grade, t.issued_ms),
            Some(_) => "cancel".into(),
            None => String::new(),
        };
        if key == self.prev_tsunami_key {
            return;
        }
        let prev = self.prev_tsunami_key.clone();
        self.prev_tsunami_key = key.clone();
        // 启动界面期间灌入的海啸状态不播 SREV 海啸音
        if self.splash.active() {
            return;
        }
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        // upsert 解除报会直接清空为 None；到期 prune 同理 → key 为空也视为解除
        let is_cancel =
            key == "cancel" || (key.is_empty() && !prev.is_empty() && prev != "cancel");
        if is_cancel {
            if !prev.is_empty() && prev != "cancel" {
                let g = tsunami_audio_grade(prev.split('|').next().unwrap_or("Forecast"));
                audio.play(AudioCue::Tsunami {
                    grade: g,
                    kind: TsunamiCueKind::Cancel,
                });
            }
            return;
        }
        if key.is_empty() {
            return;
        }
        let grade = key.split('|').next().unwrap_or("Forecast");
        let g = tsunami_audio_grade(grade);
        let kind = if prev.is_empty() || prev == "cancel" {
            TsunamiCueKind::Issue
        } else {
            TsunamiCueKind::Update
        };
        // 津波警报 / 大津波警报：优先播紧急警报放送第 2 种；其余档仍走 SREV
        if g == 3 && self.cfg.audio.tsunami_ebs_sound {
            audio.play(AudioCue::TsunamiEbs);
        } else {
            audio.play(AudioCue::Tsunami { grade: g, kind });
        }
        if self.cfg.notify.enabled && self.cfg.notify.on_tsunami {
            notify::show("海啸情报", tsunami_grade_zh(grade));
        }
    }

    fn apply_saved_config(&mut self, new_cfg: AppConfig) -> Result<(PathBuf, bool), String> {
        let mut new_cfg = new_cfg;
        new_cfg.ui.language = starquake_config::normalize_ui_language(&new_cfg.ui.language);
        starquake_ui::set_lang_from_config(&new_cfg.ui.language);

        let sources_same = self.cfg.sources_runtime_equal(&new_cfg);
        let travel_same = self.cfg.travel.primary == new_cfg.travel.primary
            && self.cfg.travel.fallback == new_cfg.travel.fallback
            && self.cfg.travel.linear_last_resort == new_cfg.travel.linear_last_resort;
        let renderer_changed = self.cfg.performance.device.to_ascii_lowercase()
            != new_cfg.performance.device.to_ascii_lowercase();
        self.cfg = new_cfg;
        // 日志 stderr 热切换（控制台层启动时已挂载，此处只拨门闩 + Alloc/Free）
        logging::set_console_output(self.cfg.log.console);

        self.local_lat = self.cfg.local.latitude;
        self.local_lon = self.cfg.local.longitude;
        self.has_local = self.local_lat.is_some() && self.local_lon.is_some();
        self.shell.local_lat = self.local_lat;
        self.shell.local_lon = self.local_lon;
        self.shell.show_nied_clock = self.cfg.ui.show_nied_clock;
        self.shell.intensity_scale = self.cfg.ui.intensity_scale.clone();
        self.shell.home_lon = self.cfg.map.default_lon;
        self.shell.home_lat = self.cfg.map.default_lat;
        self.shell.home_zoom = self.cfg.map.default_zoom;

        let basemap = BasemapMode::resolve(&self.cfg.map.basemap, &self.cfg.ui.theme);
        self.geodata_dir = resolve_geodata_dir(&self.cfg.map.geodata_dir);
        self.shell.reload_basemap(
            basemap,
            self.cfg.map.tile_base.clone(),
            self.cfg.map.tile_source.clone(),
            Some(self.geodata_dir.as_path()),
            ProjectionMode::parse(&self.cfg.map.projection),
            self.cfg.ui.show_faults,
        );
        self.shell
            .map
            .set_tile_capacity(starquake_util::perf::recommended_tile_slots(
                self.cfg.performance.device.eq_ignore_ascii_case("cpu"),
            ));

        if !travel_same {
            match TravelEngine::load_named(
                self.media_dir.join("travel"),
                &self.cfg.travel.primary,
                &self.cfg.travel.fallback,
                self.cfg.travel.linear_last_resort,
            ) {
                Ok(eng) => {
                    tracing::info!("走时表已按设置重载");
                    self.travel = Some(eng);
                }
                Err(e) => tracing::warn!("走时表重载失败: {e}"),
            }
        }

        if let Some(audio) = self.audio.as_mut() {
            audio.set_mute(self.cfg.audio.mute);
            audio.set_volumes(self.cfg.audio.master_volume, self.cfg.audio.countdown_volume);
            audio.set_packs(&self.cfg.audio.event_pack, &self.cfg.audio.countdown_pack);
        }
        apply_tts_runtime(&self.cfg.audio);

        if !sources_same {
            tracing::info!("数据源已变更，正在重连");
            drop(self.net_rx.take());
            let _enter = self.rt.enter();
            let stations_dir = self.media_dir.join("stations");
            self.net_rx = Some(spawn_hub_with_assets(
                &self.cfg.sources,
                stations_dir.is_dir().then_some(stations_dir),
            ));
        }
        match self.cfg.save_user_overlay() {
            Ok(path) => {
                tracing::info!(path = %path.display(), "用户配置已保存");
                Ok((path, renderer_changed))
            }
            Err(e) => {
                tracing::warn!("保存用户配置失败: {e:#}");
                Err(format!("{e:#}"))
            }
        }
    }
}



fn apply_tts_runtime(audio: &starquake_config::AudioConfig) {
    let volume = if audio.mute { 0.0 } else { audio.master_volume };
    configure_tts(TtsRuntimeConfig {
        engine: TtsEngine::parse(&audio.tts_engine),
        offline_voice: audio.tts_voice.clone(),
        edge_voice: audio.tts_edge_voice.clone(),
        rate: audio.tts_rate,
        volume,
    });
}

fn is_eew_camera_target(e: &EewReport) -> bool {
    !e.event_id.starts_with("record:") && !e.event_id.starts_with("station:")
}

/// 预警跟焦落地固定 Z12；落地后由 auto_zoom 按「S 贴齐视口短边」跟焦（可缓慢缩小）。
fn gq_jump_zoom() -> f64 {
    12.0
}

/// 震中相近（约 250km）：影院轮播 / 新源到达时不飞跃、不重拉 Z12，只切 active。
const NEAR_EPICENTER_KM: f64 = 250.0;

fn haversine_km(lon0: f64, lat0: f64, lon1: f64, lat1: f64) -> f64 {
    let r = 6371.0_f64;
    let to_r = |d: f64| d.to_radians();
    let dlat = to_r(lat1 - lat0);
    let dlon = to_r(lon1 - lon0);
    let a = (dlat * 0.5).sin().powi(2)
        + to_r(lat0).cos() * to_r(lat1).cos() * (dlon * 0.5).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

fn epicenters_near(lon0: f64, lat0: f64, lon1: f64, lat1: f64) -> bool {
    haversine_km(lon0, lat0, lon1, lat1) <= NEAR_EPICENTER_KM
}

/// 目标震中与当前镜头或上一跟焦震中相近时跳过 fly_to。
fn skip_eew_camera_jump(
    map: &starquake_map::MapViewport,
    target: &EewReport,
    prev: Option<&EewReport>,
) -> bool {
    if epicenters_near(map.center_lon, map.center_lat, target.longitude, target.latitude) {
        return true;
    }
    if let Some(p) = prev {
        if epicenters_near(p.longitude, p.latitude, target.longitude, target.latitude) {
            return true;
        }
    }
    false
}

fn tsunami_audio_grade(grade: &str) -> u8 {
    match grade {
        "MajorWarning" | "Major" | "Warning" => 3,
        "Watch" => 2,
        _ => 1,
    }
}
