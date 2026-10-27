//! 设置页：按 RhythmQuake `design/settings-page.html` + `SettingsControlStyle`
//! 以 egui 重写（统一玻璃面板、·侧分类、扁平行；··授权参考）。

use egui::{
    Color32, Frame, Margin, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2,
};
use crate::i18n::{t, Lang};
use starquake_config::AppConfig;
use starquake_core::HealthStatus;
use starquake_net::{exchange_login_key, format_expire_date, locate_by_public_ip, IpLocateOk};
use std::sync::mpsc;

mod filters_ui;


#[derive(Debug, Clone, Copy)]
struct SettingsChrome {
    page_bg: Color32,
    content: Color32,
    field: Color32,
    border: Color32,
    divider: Color32,
    text: Color32,
    text70: Color32,
    muted: Color32,
    accent: Color32,
    ghost_bg: Color32,
    footer_bg: Color32,
    icon_fill: Color32,
    icon_stroke: Color32,
    status_ok: Color32,
    status_warn: Color32,
    status_bad: Color32,
}

impl SettingsChrome {
    fn for_theme(theme: &str) -> Self {
        if theme.eq_ignore_ascii_case("light") {
            Self::light()
        } else {
            Self::dark()
        }
    }

    fn dark() -> Self {
        Self {
            page_bg: Color32::from_rgb(0x02, 0x02, 0x08),
            content: Color32::from_rgba_unmultiplied(36, 36, 38, 102),
            field: Color32::from_rgba_unmultiplied(255, 255, 255, 23),
            border: Color32::from_rgba_unmultiplied(255, 255, 255, 41),
            divider: Color32::from_rgba_unmultiplied(255, 255, 255, 26),
            text: Color32::WHITE,
            text70: Color32::from_rgba_unmultiplied(255, 255, 255, 178),
            muted: Color32::from_rgba_unmultiplied(255, 255, 255, 148),
            accent: Color32::from_rgb(0x82, 0xB1, 0xFF),
            ghost_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 28),
            footer_bg: Color32::from_rgba_unmultiplied(8, 8, 24, 115),
            icon_fill: Color32::from_rgba_unmultiplied(0x82, 0xB1, 0xFF, 36),
            icon_stroke: Color32::from_rgba_unmultiplied(0x82, 0xB1, 0xFF, 72),
            status_ok: Color32::from_rgb(0x72, 0xD6, 0xB1),
            status_warn: Color32::from_rgb(0xFF, 0xC6, 0x6D),
            status_bad: Color32::from_rgb(0xEF, 0x44, 0x44),
        }
    }

    fn light() -> Self {
        Self {
            page_bg: Color32::from_rgb(0xF2, 0xF4, 0xF7),
            content: Color32::from_rgba_unmultiplied(255, 255, 255, 240),
            field: Color32::from_rgb(0xEE, 0xF2, 0xF7),
            border: Color32::from_rgba_unmultiplied(0x90, 0x9A, 0xA8, 120),
            divider: Color32::from_rgba_unmultiplied(0x90, 0x9A, 0xA8, 70),
            text: Color32::from_rgb(0x1F, 0x29, 0x37),
            text70: Color32::from_rgb(0x4B, 0x55, 0x63),
            muted: Color32::from_rgb(0x6B, 0x72, 0x80),
            accent: Color32::from_rgb(0x25, 0x63, 0xEB),
            ghost_bg: Color32::from_rgb(0xE5, 0xE7, 0xEB),
            footer_bg: Color32::from_rgba_unmultiplied(255, 255, 255, 230),
            icon_fill: Color32::from_rgba_unmultiplied(0x25, 0x63, 0xEB, 28),
            icon_stroke: Color32::from_rgba_unmultiplied(0x25, 0x63, 0xEB, 70),
            status_ok: Color32::from_rgb(0x15, 0x80, 0x3D),
            status_warn: Color32::from_rgb(0xCA, 0x8A, 0x04),
            status_bad: Color32::from_rgb(0xDC, 0x26, 0x26),
        }
    }
}

thread_local! {
    static CHROME: std::cell::Cell<SettingsChrome> =
        std::cell::Cell::new(SettingsChrome::dark());
}

fn set_chrome(c: SettingsChrome) {
    CHROME.set(c);
}

fn chrome() -> SettingsChrome {
    CHROME.get()
}

pub(super) fn muted() -> Color32 {
    chrome().muted
}


const CAT_DATA: Color32 = Color32::from_rgb(0x62, 0xC6, 0xFF);
const CAT_VOICE: Color32 = Color32::from_rgb(0xD6, 0xA5, 0xFF);
const CAT_ADV: Color32 = Color32::from_rgb(0xAE, 0xB8, 0xCC);
const CAT_GENERAL: Color32 = Color32::from_rgb(0x9E, 0xD0, 0xFF);
const CAT_ABOUT: Color32 = Color32::from_rgb(0xFF, 0xC6, 0x6D);
const CAT_NOTICE: Color32 = Color32::from_rgb(0xF5, 0x9E, 0x0B);

const NAV_W: f32 = 228.0;
const HEADER_H: f32 = 78.0;
const NAV_BTN_H: f32 = 58.0;
const FOOTER_H: f32 = 52.0;

/// 随主窗口尺寸切换的设置页度量（小窗紧凑，避免底栏/侧栏被裁切）。
#[derive(Clone, Copy)]
struct SettingsLayout {
    nav_w: f32,
    header_h: f32,
    nav_btn_h: f32,
    footer_h: f32,
    shell_margin_x: i8,
    shell_margin_y: i8,
    compact: bool,
}

impl SettingsLayout {
    fn for_window(win_w: f32, win_h: f32) -> Self {
        let compact = win_h < 520.0 || win_w < 720.0;
        if compact {
            Self {
                nav_w: if win_w < 560.0 { 156.0 } else { 196.0 },
                header_h: 52.0,
                nav_btn_h: 42.0,
                footer_h: 46.0,
                shell_margin_x: 10,
                shell_margin_y: 6,
                compact: true,
            }
        } else {
            Self {
                nav_w: NAV_W,
                header_h: HEADER_H,
                nav_btn_h: NAV_BTN_H,
                footer_h: FOOTER_H,
                shell_margin_x: 28,
                shell_margin_y: 18,
                compact: false,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Sources,
    General,
    Audio,
    Advanced,
    Notices,
    About,
}

impl SettingsTab {
    const ALL: [Self; 6] = [
        Self::Sources,
        Self::General,
        Self::Audio,
        Self::Advanced,
        Self::Notices,
        Self::About,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Sources => "API与数据源",
            Self::General => "通用设置",
            Self::Audio => "音频",
            Self::Advanced => "高级",
            Self::Notices => "通知中心",
            Self::About => "关于",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Sources => "接口开关、鉴权与连接状态",
            Self::General => "主题、底图、窗口与定位",
            Self::Audio => "音效开关、通知与音量",
            Self::Advanced => "走时表、更新、缓存与配置路径",
            Self::Notices => "官方公告、更新与安全通知",
            Self::About => "版本、致谢与版权",
        }
    }

    /// 导航字形（无 Material Icons 时的占位）
    fn glyph(self) -> &'static str {
        match self {
            Self::Sources => "\u{25ce}",
            Self::General => "\u{2630}",
            Self::Audio => "\u{266a}",
            Self::Advanced => "\u{2699}",
            Self::Notices => "\u{1f514}",
            Self::About => "\u{24d8}",
        }
    }

    fn accent(self) -> Color32 {
        match self {
            Self::Sources => CAT_DATA,
            Self::General => CAT_GENERAL,
            Self::Audio => CAT_VOICE,
            Self::Advanced => CAT_ADV,
            Self::Notices => CAT_NOTICE,
            Self::About => CAT_ABOUT,
        }
    }
}

pub enum SettingsAction {
    Saved { cfg: AppConfig, close: bool },
    Cancelled,
    /// 手动检查 GitHub 更新
    CheckForUpdate,
    TestSound {
        mute: bool,
        master_volume: f32,
        countdown_volume: f32,
        event_pack: String,
        countdown_pack: String,
    },
    /// 按草稿 TTS 配置试听一句（无需先保存）
    TestTts {
        engine: String,
        offline_voice: String,
        edge_voice: String,
        rate: f32,
        volume: f32,
    },
    /// 发送一条桌面系统通知测试
    TestNotify,
    ClearTileCache,
    OpenLogDir,
    /// 通知中心：标记单条已读
    MarkNoticeRead(i64),
    /// 通知中心：全部标记已读
    MarkAllNoticesRead,
}

/// 设置页「通知中心」展示用条目（由主循环注入）。
#[derive(Debug, Clone)]
pub struct OfficialNoticeView {
    pub id: i64,
    pub title: String,
    pub content: String,
    pub type_label: String,
    pub created_at: String,
    pub unread: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPickKind {
    DefaultViewport,
    LocalLocation,
}

pub struct SourceHealthView {
    pub jian: HealthStatus,
    pub wolfx: HealthStatus,
    pub p2p: HealthStatus,
    pub pancakes: HealthStatus,
    pub fssn: HealthStatus,
    pub weather: HealthStatus,
    pub cenc_int: HealthStatus,
    pub jian_enabled: bool,
    pub wolfx_enabled: bool,
    pub p2p_enabled: bool,
    pub pancakes_enabled: bool,
    pub fssn_enabled: bool,
    pub weather_enabled: bool,
    pub cenc_int_enabled: bool,
}

pub struct SettingsSession {
    pub open: bool,
    draft: Option<AppConfig>,
    tab: SettingsTab,
    keep_open: bool,
    save_note: Option<String>,
    login_key: String,
    auth_busy: bool,
    /// 后台换票结果（避免阻塞 UI 帧）：token, expire, max_connections, message
    auth_rx: Option<mpsc::Receiver<Result<(String, f64, u32, String), String>>>,
    /// 换票失败弹窗
    auth_alert: Option<(String, String)>,
    /// IP 归属定位进行中
    ip_locate_busy: bool,
    /// 后台 IP 定位结果
    ip_locate_rx: Option<mpsc::Receiver<Result<IpLocateOk, String>>>,
    /// 主地图单击选点目标
    pub map_pick: Option<MapPickKind>,
    /// 下一帧用主地图中心写入默认视口
    pub capture_view: bool,
    /// 打开时刻（忽略短时虚假 close）
    opened_at: Option<std::time::Instant>,
    /// 每次打开递增，避免 egui 记住被撑满的尺寸
    open_gen: u64,
    /// 性能诊断文案
    perf_diag: Option<String>,
    /// 本机离线 SAPI 语音包（打开设置时枚举一次）
    offline_tts_voices: Vec<(String, String)>,
    /// Edge TTS 预设音色
    edge_tts_voices: Vec<(String, String)>,
    /// 官方通知列表（主循环每帧同步）
    pub official_notices: Vec<OfficialNoticeView>,
}

impl Default for SettingsSession {
    fn default() -> Self {
        Self {
            open: false,
            draft: None,
            tab: SettingsTab::Sources,
            keep_open: false,
            save_note: None,
            login_key: String::new(),
            auth_busy: false,
            auth_rx: None,
            auth_alert: None,
            ip_locate_busy: false,
            ip_locate_rx: None,
            map_pick: None,
            capture_view: false,
            opened_at: None,
            open_gen: 0,
            perf_diag: None,
            offline_tts_voices: Vec::new(),
            edge_tts_voices: Vec::new(),
            official_notices: Vec::new(),
        }
    }
}

impl SettingsSession {
    pub fn open_with(&mut self, cfg: &AppConfig) {
        self.open = true;
        self.draft = Some(cfg.clone());
        self.tab = SettingsTab::Sources;
        self.save_note = None;
        self.keep_open = false;
        self.login_key.clear();
        self.auth_busy = false;
        self.auth_rx = None;
        self.auth_alert = None;
        self.ip_locate_busy = false;
        self.ip_locate_rx = None;
        self.map_pick = None;
        self.capture_view = false;
        self.opened_at = Some(std::time::Instant::now());
        self.open_gen = self.open_gen.wrapping_add(1);
        self.perf_diag = None;
        self.offline_tts_voices = starquake_audio::list_offline_voices()
            .into_iter()
            .map(|v| (v.id, v.label))
            .collect();
        self.edge_tts_voices = starquake_audio::edge_voice_presets()
            .into_iter()
            .map(|v| (v.id, v.label))
            .collect();
    }

    /// 打开设置并切到关于页。
    pub fn open_about(&mut self, cfg: &AppConfig) {
        self.open_with(cfg);
        self.tab = SettingsTab::About;
    }

    /// 进入本机位置地图选点（选点期间不挡地图；选完回到设置草稿）。
    pub fn begin_local_pick(&mut self, cfg: &AppConfig) {
        self.open_with(cfg);
        self.tab = SettingsTab::General;
        self.map_pick = Some(MapPickKind::LocalLocation);
    }

    pub fn set_save_note(&mut self, note: impl Into<String>) {
        self.save_note = Some(note.into());
    }

    /// 自动化巡检：按索引切换设置分类（0..settings_tab_count）。
    pub fn set_tab_index(&mut self, idx: usize) {
        if let Some(tab) = SettingsTab::ALL.get(idx).copied() {
            self.tab = tab;
        }
    }

    pub fn close_after_save(&mut self) {
        self.open = false;
        self.draft = None;
        self.opened_at = None;
    }

    /// 用主地图当前中心写入默认视口
    pub fn apply_capture_view(&mut self, lon: f64, lat: f64, zoom: f64) {
        if let Some(draft) = self.draft.as_mut() {
            draft.map.default_lon = lon;
            draft.map.default_lat = lat;
            draft.map.default_zoom = zoom;
            self.save_note = Some(t("已用当前视野写入默认视口").into());
        }
        self.capture_view = false;
    }

    /// 主地图单击坐标写入对应 `map_pick`。?
    pub fn apply_map_pick_coords(
        &mut self,
        kind: MapPickKind,
        lon: f64,
        lat: f64,
        zoom: f64,
    ) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        match kind {
            MapPickKind::DefaultViewport => {
                draft.map.default_lon = lon;
                draft.map.default_lat = lat;
                draft.map.default_zoom = zoom;
                self.save_note = Some(t("已用地图选点写入默认视口").into());
            }
            MapPickKind::LocalLocation => {
                draft.local.longitude = Some(lon);
                draft.local.latitude = Some(lat);
                self.save_note = Some(t("已用地图选点写入本机位置").into());
            }
        }
        self.map_pick = None;
    }

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        live: &AppConfig,
        health: &SourceHealthView,
    ) -> Option<SettingsAction> {
        self.draw_auth_alert(ctx);
        // 选点时不挡地图；设置保持 open，选完自动回来
        if self.map_pick.is_some() {
            return None;
        }
        if !self.open {
            return None;
        }
        if self.draft.is_none() {
            self.draft = Some(live.clone());
        }

        let mut action = None;
        let mut restore = false;
        let mut do_exchange = false;
        let mut do_ip_locate = false;
        let mut close_requested = false;

        // 主窗口内嵌设置（勿再用独立 viewport：会抢焦点，表现为鼠标被隔离）
        if let Some(d) = self.draft.as_ref() {
            crate::i18n::set_lang_from_config(&d.ui.language);
        }
        let theme_key = self
            .draft
            .as_ref()
            .map(|d| d.ui.theme.as_str())
            .unwrap_or("dark");
        let c = SettingsChrome::for_theme(theme_key);
        set_chrome(c);

        // 全屏遮罩：点空白关闭。Window 与 ComboBox 弹出层同为 Foreground
        // （勿用 Tooltip：会盖住 egui ComboBox 的 Foreground 菜单）。
        // dim 先画、Window 后画，同层内 Window 在上；打开下拉时 popup 会 move_to_top。
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("starquake_settings_dim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                let resp = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    Color32::from_black_alpha(140),
                );
                if resp.clicked() {
                    close_requested = true;
                }
            });

        // 定死尺寸：禁止内容抬升；小窗也保留一圈遮罩边，勿贴满主窗口
        let edge_x = (screen.width() * 0.08).clamp(28.0, 56.0);
        let edge_y = (screen.height() * 0.09).clamp(28.0, 56.0);
        let max_w = (screen.width() - edge_x * 2.0)
            .min(screen.width() * 0.88)
            .clamp(280.0, 1100.0);
        let max_h = (screen.height() - edge_y * 2.0)
            .min(screen.height() * 0.84)
            .clamp(200.0, 680.0);
        let win_w = 960.0_f32.min(max_w);
        let win_h = 600.0_f32.min(max_h);
        let layout = SettingsLayout::for_window(win_w, win_h);

        let mut still_open = self.open;
        let mut header_close = false;
        egui::Window::new(format!("{} \u{00b7} {}", crate::APP_DISPLAY_NAME, t("设置")))
            .id(egui::Id::new(("starquake_settings_embedded", self.open_gen)))
            .open(&mut still_open)
            .fixed_size([win_w, win_h])
            .resizable(false)
            .collapsible(false)
            // 不用 egui 标题条：关闭改由页眉右上角 X，小窗也不丢关闭入口
            .title_bar(false)
            .constrain_to(screen)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Foreground)
            .frame(
                Frame::NONE
                    .fill(chrome().page_bg)
                    .stroke(Stroke::new(1.0_f32, chrome().border))
                    .inner_margin(Margin::ZERO)
                    .corner_radius(if layout.compact { 10.0 } else { 14.0 }),
            )
            .show(ctx, |ui| {
                ui.set_min_size(Vec2::new(win_w, win_h));
                ui.set_max_size(Vec2::new(win_w, win_h));
                draw_shell(
                    ui,
                    self,
                    health,
                    &mut action,
                    &mut restore,
                    &mut do_exchange,
                    &mut do_ip_locate,
                    layout,
                    &mut header_close,
                );
            });
        if !still_open || header_close {
            close_requested = true;
        }
        self.open = still_open;

        // Esc 关闭（打开后短暂忽略）；选点模式留给主界面 Esc
        let grace_ok = self
            .opened_at
            .map(|t0| t0.elapsed().as_millis() > 400)
            .unwrap_or(true);
        if grace_ok && self.map_pick.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_requested = true;
        }


        if do_exchange && !self.auth_busy {
            let key = self.login_key.clone();
            let (tx, rx) = mpsc::channel();
            self.auth_busy = true;
            self.auth_rx = Some(rx);
            self.save_note = Some(t("正在换票…").into());
            std::thread::spawn(move || {
                let _ = tx.send(
                    exchange_login_key(&key)
                        .map(|ok| {
                            let exp = if ok.refresh_expire_at > 0.0 {
                                ok.refresh_expire_at
                            } else {
                                ok.expire_at
                            };
                            (ok.token, exp, ok.max_connections, ok.message)
                        }),
                );
            });
        }

        if do_ip_locate && !self.ip_locate_busy {
            let (tx, rx) = mpsc::channel();
            self.ip_locate_busy = true;
            self.ip_locate_rx = Some(rx);
            self.save_note = Some(t("正在按 IP 定位…").into());
            std::thread::spawn(move || {
                let _ = tx.send(locate_by_public_ip());
            });
        }

        let auth_poll = self.auth_rx.as_ref().map(|rx| rx.try_recv());
        match auth_poll {
            Some(Ok(Ok((token, expire_at, max_conn, message)))) => {
                self.auth_rx = None;
                self.auth_busy = false;
                if let Some(draft) = self.draft.as_mut() {
                    draft.sources.jian.refresh_token = Some(token);
                    draft.sources.jian.refresh_expire_at = expire_at;
                    if max_conn > 0 {
                        draft.sources.jian.max_connections = max_conn;
                    }
                }
                let expire = format_expire_date(expire_at)
                    .map(|d| format!("，长期 Token 过期：{d}"))
                    .unwrap_or_default();
                let cap = if max_conn > 0 {
                    format!("，并发上限 {max_conn}")
                } else {
                    String::new()
                };
                self.save_note = Some(format!("{message}{expire}{cap}"));
                self.login_key.clear();
                if let Some(draft) = self.draft.clone() {
                    action = Some(SettingsAction::Saved {
                        cfg: draft,
                        close: false,
                    });
                }
            }
            Some(Ok(Err(e))) => {
                self.auth_rx = None;
                self.auth_busy = false;
                self.save_note = Some(format!("{}{e}", t("鉴权失败：")));
                self.auth_alert = Some((t("鉴权失败").into(), e));
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.auth_rx = None;
                self.auth_busy = false;
                self.save_note = Some(t("鉴权失败：换票线程中断").into());
            }
            None => {}
        }

        let ip_poll = self.ip_locate_rx.as_ref().map(|rx| rx.try_recv());
        match ip_poll {
            Some(Ok(Ok(ok))) => {
                self.ip_locate_rx = None;
                self.ip_locate_busy = false;
                if let Some(draft) = self.draft.as_mut() {
                    draft.local.latitude = Some(ok.lat);
                    draft.local.longitude = Some(ok.lon);
                }
                let place = if ok.location.is_empty() {
                    format!("{:.4}, {:.4}", ok.lat, ok.lon)
                } else {
                    ok.location.clone()
                };
                let ip_hint = if ok.ip.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", ok.ip)
                };
                self.save_note = Some(format!(
                    "{}{place}{ip_hint}",
                    t("已按 IP 写入本机位置：")
                ));
            }
            Some(Ok(Err(e))) => {
                self.ip_locate_rx = None;
                self.ip_locate_busy = false;
                self.save_note = Some(format!("{}{e}", t("IP 定位失败：")));
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.ip_locate_rx = None;
                self.ip_locate_busy = false;
                self.save_note = Some(t("IP 定位失败：查询线程中断").into());
            }
            None => {}
        }

        if restore {
            let token = live.sources.jian.access_token.clone();
            let refresh = live.sources.jian.refresh_token.clone();
            let refresh_exp = live.sources.jian.refresh_expire_at;
            let max_conn = live.sources.jian.max_connections;
            let mut d = AppConfig::default();
            d.sources.jian.access_token = token;
            d.sources.jian.refresh_token = refresh;
            d.sources.jian.refresh_expire_at = refresh_exp;
            d.sources.jian.max_connections = max_conn;
            self.draft = Some(d);
            self.save_note = Some(t("已恢复默认（令牌保留）；点「保存全部」写入").into());
        }

        if close_requested {
            self.open = false;
            self.draft = None;
            self.opened_at = None;
            return Some(SettingsAction::Cancelled);
        }

        if let Some(SettingsAction::Saved { close: true, .. }) = &action {
            // 保存且需关闭：本帧立刻卸窗，避免等主循环 close_after_save 才藏
            self.open = false;
            self.draft = None;
            self.opened_at = None;
            return action;
        }
        if matches!(
            action,
            Some(SettingsAction::Saved { .. })
                | Some(SettingsAction::TestSound { .. })
                | Some(SettingsAction::TestTts { .. })
                | Some(SettingsAction::TestNotify)
                | Some(SettingsAction::ClearTileCache)
                | Some(SettingsAction::OpenLogDir)
                | Some(SettingsAction::CheckForUpdate)
        ) {
            return action;
        }
        if matches!(action, Some(SettingsAction::Cancelled)) {
            self.open = false;
            self.draft = None;
            self.opened_at = None;
            return action;
        }
        None
    }

    fn draw_auth_alert(&mut self, ctx: &egui::Context) {
        let Some((title, body)) = self.auth_alert.clone() else {
            return;
        };
        let mut close = false;
        egui::Window::new(title)
            .id(egui::Id::new("auth_alert"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(280.0);
                ui.label(body);
                ui.add_space(8.0);
                if ui.button(t("知道了")).clicked() {
                    close = true;
                }
            });
        if close {
            self.auth_alert = None;
        }
    }
}

/// 设置左侧分类数量（自动化巡检用）。
pub fn settings_tab_count() -> usize {
    SettingsTab::ALL.len()
}

/// 设置分类标题（自动化巡检用）。
pub fn settings_tab_title(idx: usize) -> &'static str {
    SettingsTab::ALL
        .get(idx)
        .map(|t| t.title())
        .unwrap_or("")
}

fn draw_shell(
    ui: &mut Ui,
    session: &mut SettingsSession,
    health: &SourceHealthView,
    action: &mut Option<SettingsAction>,
    restore: &mut bool,
    do_exchange: &mut bool,
    do_ip_locate: &mut bool,
    layout: SettingsLayout,
    header_close: &mut bool,
) {
    // RQ .shell inset（小窗收紧边距，给面板留高度）
    Frame::NONE
        .inner_margin(Margin::symmetric(layout.shell_margin_x, layout.shell_margin_y))
        .show(ui, |ui| {
    let shell = ui.available_size();
    ui.set_max_size(shell);
    ui.allocate_ui_with_layout(
        shell,
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_max_size(shell);
            // 页眉（RQ .header）；右上角关闭 X
            Frame::NONE
                .inner_margin(Margin::symmetric(4, if layout.compact { 2 } else { 6 }))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(t("设置中心"))
                                    .strong()
                                    .size(if layout.compact { 18.0 } else { 22.0 })
                                    .color(chrome().text),
                            );
                            if !layout.compact {
                                ui.label(
                                    RichText::new(t("StarQuake · 左侧分类与右侧内容合并为同一玻璃面板"))
                                        .size(12.0)
                                        .color(chrome().muted),
                                );
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if settings_close_btn(ui, layout.compact).clicked() {
                                *header_close = true;
                            }
                        });
                    });
                });

            let avail = ui.available_size();
            // 剩余高度扣底栏；禁止再强制 min=200 把底栏挤出可视区
            let body_h = (avail.y - layout.footer_h - 6.0).clamp(96.0, avail.y.max(96.0));
            let body_w = avail.x.max(100.0);

            // 统一面板（RQ .panel）
            Frame::NONE
                .fill(chrome().content)
                .stroke(Stroke::new(1.0_f32, chrome().border))
                .corner_radius(if layout.compact { 10.0 } else { 14.0 })
                .inner_margin(Margin::ZERO)
                .show(ui, |ui| {
                    ui.set_max_size(Vec2::new(body_w, body_h));
                    ui.allocate_ui_with_layout(
                        Vec2::new(body_w, body_h),
                        egui::Layout::left_to_right(egui::Align::TOP),
                        |ui| {
                            ui.set_max_size(Vec2::new(body_w, body_h));
                            draw_nav(ui, session, body_h, layout);
                            // 竖分割线
                            let (line, _) =
                                ui.allocate_exact_size(Vec2::new(1.0, body_h), Sense::hover());
                            ui.painter().rect_filled(line, 0.0, chrome().divider);

                            draw_main(
                                ui,
                                session,
                                health,
                                action,
                                do_exchange,
                                do_ip_locate,
                                body_h,
                                layout,
                            );
                        },
                    );
                });

            ui.add_space(if layout.compact { 4.0 } else { 6.0 });

            // 底栏：StarQuake 草稿保存（RQ 即时写入；此处保留显式保存）
            Frame::NONE
                .fill(chrome().footer_bg)
                .stroke(Stroke::new(1.0_f32, chrome().divider))
                .corner_radius(10.0)
                .inner_margin(Margin::symmetric(if layout.compact { 8 } else { 14 }, 6))
                .show(ui, |ui| {
                    if let Some(n) = session.save_note.as_deref() {
                        ui.label(RichText::new(n).size(11.5).color(chrome().muted));
                        ui.add_space(2.0);
                    }
                    ui.horizontal_wrapped(|ui| {
                        if glass_btn(ui, "恢复默认", false).clicked() {
                            *restore = true;
                        }
                        ui.add_space(8.0);
                        ui.checkbox(
                            &mut session.keep_open,
                            RichText::new(t("保存后不关闭")).size(12.0).color(chrome().text70),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let draft = session.draft.as_ref().unwrap();
                            if glass_btn(ui, "保存全部", true).clicked() {
                                *action = Some(SettingsAction::Saved {
                                    cfg: draft.clone(),
                                    close: !session.keep_open,
                                });
                            }
                            ui.add_space(6.0);
                            if glass_btn(ui, "应用", false).clicked() {
                                *action = Some(SettingsAction::Saved {
                                    cfg: draft.clone(),
                                    close: false,
                                });
                            }
                            ui.add_space(6.0);
                            if glass_btn(ui, "取消", false).clicked() {
                                *action = Some(SettingsAction::Cancelled);
                            }
                        });
                    });
                });
        },
    );
        }); // shell inset
}

fn draw_nav(ui: &mut Ui, session: &mut SettingsSession, body_h: f32, layout: SettingsLayout) {
    let nav_w = layout.nav_w;
    ui.allocate_ui_with_layout(
        Vec2::new(nav_w, body_h),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(nav_w);
            ui.set_max_width(nav_w);

            let head_rect = ui
                .allocate_exact_size(Vec2::new(nav_w, layout.header_h), Sense::hover())
                .0;
            ui.painter().text(
                Pos2::new(head_rect.left() + 14.0, head_rect.center().y),
                egui::Align2::LEFT_CENTER,
                t("应用设置"),
                egui::FontId::proportional(if layout.compact { 13.5 } else { 15.0 }),
                chrome().text,
            );
            paint_fade_rule(ui, head_rect, 12.0);

            let nav_scroll_h = (body_h - layout.header_h - 4.0).max(48.0);
            ScrollArea::vertical()
                .id_salt("settings_nav")
                .auto_shrink([false, false])
                .max_height(nav_scroll_h)
                .show(ui, |ui| {
                    ui.set_max_width(nav_w);
                    ui.add_space(6.0);
                    for tab in SettingsTab::ALL {
                        let selected = session.tab == tab;
                        let accent = tab.accent();
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            let (rect, resp) = ui.allocate_exact_size(
                                Vec2::new((nav_w - 16.0).max(80.0), layout.nav_btn_h),
                                Sense::click(),
                            );

                            let fill = if selected {
                                Color32::from_rgba_unmultiplied(
                                    accent.r(),
                                    accent.g(),
                                    accent.b(),
                                    36,
                                )
                            } else if resp.hovered() {
                                Color32::from_rgba_unmultiplied(255, 255, 255, 16)
                            } else {
                                Color32::TRANSPARENT
                            };
                            let stroke = if selected {
                                Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgba_unmultiplied(
                                        accent.r(),
                                        accent.g(),
                                        accent.b(),
                                        110,
                                    ),
                                )
                            } else {
                                Stroke::NONE
                            };
                            if selected {
                                ui.painter().rect_filled(
                                    rect,
                                    10.0,
                                    Color32::from_rgba_unmultiplied(255, 255, 255, 26),
                                );
                            }
                            ui.painter().rect(
                                rect,
                                10.0,
                                fill,
                                stroke,
                                egui::StrokeKind::Inside,
                            );
                            if selected {
                                let y = rect.min.y + 1.0;
                                ui.painter().line_segment(
                                    [
                                        Pos2::new(rect.min.x + 10.0, y),
                                        Pos2::new(rect.max.x - 10.0, y),
                                    ],
                                    Stroke::new(
                                        1.0_f32,
                                        Color32::from_rgba_unmultiplied(255, 255, 255, 46),
                                    ),
                                );
                            }

                            let icon_c = if selected { accent } else { chrome().muted };
                            let title_c = if selected {
                                chrome().text
                            } else {
                                chrome().text70
                            };
                            let cy = rect.center().y;
                            ui.painter().text(
                                Pos2::new(rect.left() + 12.0, cy),
                                egui::Align2::LEFT_CENTER,
                                tab.glyph(),
                                egui::FontId::proportional(if layout.compact { 14.0 } else { 16.0 }),
                                icon_c,
                            );
                            if layout.compact {
                                ui.painter().text(
                                    Pos2::new(rect.left() + 34.0, cy),
                                    egui::Align2::LEFT_CENTER,
                                    crate::i18n::t(tab.title()),
                                    egui::FontId::proportional(12.5),
                                    title_c,
                                );
                            } else {
                                ui.painter().text(
                                    Pos2::new(rect.left() + 38.0, cy - 8.0),
                                    egui::Align2::LEFT_CENTER,
                                    crate::i18n::t(tab.title()),
                                    egui::FontId::proportional(13.5),
                                    title_c,
                                );
                                ui.painter().text(
                                    Pos2::new(rect.left() + 38.0, cy + 9.0),
                                    egui::Align2::LEFT_CENTER,
                                    crate::i18n::t(tab.subtitle()),
                                    egui::FontId::proportional(10.5),
                                    chrome().muted,
                                );
                            }

                            if resp.clicked() {
                                session.tab = tab;
                            }
                        });
                        ui.add_space(if layout.compact { 2.0 } else { 4.0 });
                    }
                    ui.add_space(6.0);
                });
        },
    );
}

fn draw_main(
    ui: &mut Ui,
    session: &mut SettingsSession,
    health: &SourceHealthView,
    action: &mut Option<SettingsAction>,
    do_exchange: &mut bool,
    do_ip_locate: &mut bool,
    body_h: f32,
    layout: SettingsLayout,
) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(
        Vec2::new(w, body_h),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            let tab = session.tab;
            let accent = tab.accent();

            // 内容头（RQ .content-head）
            let head = ui
                .allocate_exact_size(Vec2::new(ui.available_width(), layout.header_h), Sense::hover())
                .0;
            let icon_sz = if layout.compact { 28.0 } else { 36.0 };
            let icon_rect = Rect::from_center_size(
                Pos2::new(head.left() + 20.0 + icon_sz * 0.5, head.center().y),
                Vec2::splat(icon_sz),
            );
            ui.painter().rect(
                icon_rect,
                8.0,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 34),
                Stroke::NONE,
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                tab.glyph(),
                egui::FontId::proportional(if layout.compact { 15.0 } else { 18.0 }),
                accent,
            );
            let text_x = icon_rect.right() + 10.0;
            if layout.compact {
                ui.painter().text(
                    Pos2::new(text_x, head.center().y),
                    egui::Align2::LEFT_CENTER,
                    crate::i18n::t(tab.title()),
                    egui::FontId::proportional(14.0),
                    chrome().text,
                );
            } else {
                ui.painter().text(
                    Pos2::new(text_x, head.center().y - 9.0),
                    egui::Align2::LEFT_CENTER,
                    crate::i18n::t(tab.title()),
                    egui::FontId::proportional(16.0),
                    chrome().text,
                );
                ui.painter().text(
                    Pos2::new(text_x, head.center().y + 10.0),
                    egui::Align2::LEFT_CENTER,
                    crate::i18n::t(tab.subtitle()),
                    egui::FontId::proportional(11.5),
                    chrome().muted,
                );
            }
            paint_fade_rule(ui, head, 14.0);

            let scroll_h = (body_h - layout.header_h - 4.0).max(48.0);
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(scroll_h)
                .id_salt("settings_body")
                .show(ui, |ui| {
                    // 只限宽，不把 available 写成 min（避免窗体逐帧加宽）
                    ui.set_max_width(ui.available_width());
                    ui.add_space(if layout.compact { 4.0 } else { 8.0 });
                    Frame::NONE
                        .inner_margin(Margin::symmetric(if layout.compact { 10 } else { 16 }, 8))
                        .show(ui, |ui| {
                            draw_content(ui, session, health, action, do_exchange, do_ip_locate);
                        });
                    ui.add_space(12.0);
                });
        },
    );
}

fn paint_fade_rule(ui: &Ui, head: Rect, inset: f32) {
    // RQ .panel-nav-head / .content-head ::after ? mid band with faded ends.
    let y = head.bottom() - 1.0;
    let left = head.left() + inset;
    let right = head.right() - inset;
    let span = (right - left).max(1.0);
    let stops = [
        (0.00, 0),
        (0.18, 30),
        (0.50, 30),
        (0.82, 30),
        (1.00, 0),
    ];
    for w in stops.windows(2) {
        let (t0, a0) = w[0];
        let (t1, a1) = w[1];
        let x0 = left + span * t0 as f32;
        let x1 = left + span * t1 as f32;
        let a = ((a0 + a1) / 2).max(1) as u8;
        ui.painter().line_segment(
            [Pos2::new(x0, y), Pos2::new(x1, y)],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, a)),
        );
    }
}

fn draw_content(
    ui: &mut Ui,
    session: &mut SettingsSession,
    health: &SourceHealthView,
    action: &mut Option<SettingsAction>,
    do_exchange: &mut bool,
    do_ip_locate: &mut bool,
) {
    let tab = session.tab;
    let auth_busy = session.auth_busy;
    let ip_locate_busy = session.ip_locate_busy;
    let mut map_pick = session.map_pick;
    let mut capture_view = session.capture_view;
    let mut perf_diag = session.perf_diag.take();
    let offline_tts_voices = session.offline_tts_voices.clone();
    let edge_tts_voices = session.edge_tts_voices.clone();
    let official_notices = session.official_notices.clone();
    {
        let draft = session.draft.as_mut().unwrap();
        let login_key = &mut session.login_key;
        match tab {
            SettingsTab::Sources => {
                if draw_sources(ui, draft, login_key, auth_busy, health) {
                    *do_exchange = true;
                }
            }
            SettingsTab::General => {
                if draw_general(
                    ui,
                    draft,
                    &mut map_pick,
                    &mut capture_view,
                    &mut perf_diag,
                    ip_locate_busy,
                ) {
                    *do_ip_locate = true;
                }
            }
            SettingsTab::Audio => {
                if let Some(a) = draw_audio(ui, draft, &offline_tts_voices, &edge_tts_voices) {
                    *action = Some(a);
                }
            }
            SettingsTab::Advanced => draw_advanced(ui, draft, action),
            SettingsTab::Notices => {
                if let Some(a) = draw_notices(ui, &official_notices) {
                    *action = Some(a);
                }
            }
            SettingsTab::About => draw_about(ui, draft),
        }
    }
    session.map_pick = map_pick;
    session.capture_view = capture_view;
    session.perf_diag = perf_diag;
}

pub(super) fn section(ui: &mut Ui, title: &str, glyph: &str, add: impl FnOnce(&mut Ui)) {
    let title = t(title);
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).size(16.0).color(chrome().accent));
        ui.label(RichText::new(title).strong().size(15.0).color(chrome().text));
    });
    ui.add_space(12.0);
    add(ui);
    ui.add_space(18.0);
}

pub(super) fn row_divider(ui: &mut Ui) {
    ui.add_space(10.0);
    let w = ui.available_width().max(1.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w.min(900.0), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, chrome().divider);
    ui.add_space(10.0);
}

/// RQ SettingsControlRow：左侧图标盒 + 标题副标题，右侧控件
pub(super) fn control_row(
    ui: &mut Ui,
    title: &str,
    subtitle: &str,
    glyph: &str,
    right: impl FnOnce(&mut Ui),
) {
    let title = t(title);
    let subtitle = if subtitle.is_empty() { "" } else { t(subtitle) };
    ui.horizontal(|ui| {
        ui.set_height(40.0);
        paint_row_icon(ui, glyph);
        ui.add_space(10.0);

        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.vertical(|ui| {
                ui.set_max_width((ui.available_width() - 280.0).max(140.0));
                ui.label(RichText::new(title).size(14.0).strong().color(chrome().text));
                if !subtitle.is_empty() {
                    ui.label(RichText::new(subtitle).size(12.0).color(chrome().muted));
                }
            });
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // 禁止 set_min_width：会把窗体逐帧撑宽
            ui.set_max_width(280.0);
            right(ui);
        });
    });
}

fn paint_row_icon(ui: &mut Ui, glyph: &str) {
    let (icon_r, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
    ui.painter().rect(
        icon_r,
        8.0,
        chrome().icon_fill,
        Stroke::new(1.0_f32, chrome().icon_stroke),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        icon_r.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(14.0),
        chrome().accent,
    );
}

/// 标题行 + 下方通栏填写框（密钥、白名单等）
pub(super) fn control_field_below(
    ui: &mut Ui,
    title: &str,
    subtitle: &str,
    glyph: &str,
    edit: impl FnOnce(&mut Ui),
) {
    let title = t(title);
    let subtitle = if subtitle.is_empty() { "" } else { t(subtitle) };
    ui.horizontal(|ui| {
        paint_row_icon(ui, glyph);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(14.0).strong().color(chrome().text));
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).size(12.0).color(chrome().muted));
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(44.0);
        edit(ui);
    });
}

fn settings_close_btn(ui: &mut Ui, compact: bool) -> egui::Response {
    let side = if compact { 28.0 } else { 32.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    let fill = if resp.hovered() {
        Color32::from_rgba_unmultiplied(239, 68, 68, 56)
    } else {
        chrome().ghost_bg
    };
    ui.painter()
        .rect_filled(rect, 8.0, fill);
    if resp.hovered() {
        ui.painter().rect_stroke(
            rect,
            8.0,
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(239, 68, 68, 140)),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "\u{00d7}",
        egui::FontId::proportional(if compact { 18.0 } else { 20.0 }),
        if resp.hovered() {
            Color32::from_rgb(0xEF, 0x44, 0x44)
        } else {
            chrome().text70
        },
    );
    resp.on_hover_text(t("关闭"))
}

fn glass_btn(ui: &mut Ui, text: &str, emphasized: bool) -> egui::Response {
    let text = t(text);
    let fill = if emphasized {
        Color32::from_rgba_unmultiplied(chrome().accent.r(), chrome().accent.g(), chrome().accent.b(), 90)
    } else {
        chrome().ghost_bg
    };
    let stroke = if emphasized {
        Stroke::new(1.0_f32,
            Color32::from_rgba_unmultiplied(chrome().accent.r(), chrome().accent.g(), chrome().accent.b(), 200),
        )
    } else {
        Stroke::new(1.0_f32, chrome().border)
    };
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).strong().color(chrome().text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(10.0)
            .min_size(Vec2::new(if emphasized { 108.0 } else { 76.0 }, 40.0)),
    )
}

pub(super) fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(46.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let track = if *on {
        Color32::from_rgba_unmultiplied(chrome().accent.r(), chrome().accent.g(), chrome().accent.b(), 97)
    } else {
        chrome().ghost_bg
    };
    ui.painter().rect_filled(rect, 14.0, track);
    let thumb_x = if *on {
        rect.right() - 14.0
    } else {
        rect.left() + 14.0
    };
    let thumb_c = if *on {
        chrome().accent
    } else {
        chrome().text70
    };
    ui.painter()
        .circle_filled(Pos2::new(thumb_x, rect.center().y), 10.0, thumb_c);
    // toggle hover ring
    if resp.hovered() {
        ui.painter().rect_stroke(
            rect,
            14.0,
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(chrome().accent.r(), chrome().accent.g(), chrome().accent.b(), 90)),
            egui::StrokeKind::Outside,
        );
    }
    resp
}

pub(super) fn framed_edit(ui: &mut Ui, text: &mut String, width: f32) {
    Frame::NONE
        .fill(chrome().field)
        .stroke(Stroke::new(1.0_f32, chrome().border))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(width)
                    .text_color(chrome().text),
            );
        });
}

fn draw_sources(
    ui: &mut Ui,
    draft: &mut AppConfig,
    login_key: &mut String,
    auth_busy: bool,
    health: &SourceHealthView,
) -> bool {
    let mut exchange = false;

    section(ui, "服务器状态", "\u{25c9}", |ui| {
        status_row(ui, "Jian Project", health.jian, health.jian_enabled);
        row_divider(ui);
        status_row(ui, "Wolfx", health.wolfx, health.wolfx_enabled);
        row_divider(ui);
        status_row(ui, "P2PQuake", health.p2p, health.p2p_enabled);
        row_divider(ui);
        status_row(ui, "PancakesAPI", health.pancakes, health.pancakes_enabled);
        row_divider(ui);
        status_row(ui, "FSSN", health.fssn, health.fssn_enabled);
        row_divider(ui);
        status_row(ui, "气象预警", health.weather, health.weather_enabled);
        row_divider(ui);
        status_row(ui, "烈度速报", health.cenc_int, health.cenc_int_enabled);
    });

    section(ui, "Jian Project", "\u{25ce}", |ui| {
        control_row(ui, "启用 /all", "连接 Jian 聚合通道（需令牌）", "\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.jian.enabled);
        });
        for fam in starquake_core::EEW_FILTER_FAMILIES {
            row_divider(ui);
            let mut on = jian_eew_flag(draft, *fam);
            let sub = format!("[{}]", fam.bracket_label());
            control_row(ui, fam.display_name(), &sub, "\u{25ce}", |ui| {
                toggle(ui, &mut on);
            });
            set_jian_eew_flag(draft, *fam, on);
        }
        row_divider(ui);
        control_field_below(
            ui,
            "登录密钥 lk_",
            "换取长期 Token rt_ 并写入用户配置",
            "\u{25ce}",
            |ui| {
                let w = (ui.available_width() - 8.0).clamp(200.0, 420.0);
                framed_edit(ui, login_key, w);
            },
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(44.0);
            let label = if auth_busy { "换票中…" } else { "换票并保存" };
            if glass_btn(ui, label, true).clicked() && !auth_busy {
                exchange = true;
            }
        });
        ui.add_space(6.0);
        if let Some(rt) = draft.sources.jian.refresh_token.as_deref() {
            let short = if rt.len() > 12 {
                format!("{}…{}", &rt[..6], &rt[rt.len() - 4..])
            } else {
                rt.to_string()
            };
            let exp = format_expire_date(draft.sources.jian.refresh_expire_at)
                .map(|d| format!("（过期 {d}）"))
                .unwrap_or_default();
            let cap = if draft.sources.jian.max_connections > 0 {
                format!(" · 并发 {}", draft.sources.jian.max_connections)
            } else {
                String::new()
            };
            ui.label(
                RichText::new(format!("{}", t("长期 Token：{short}{exp}").replace("{short}", &short).replace("{exp}", &format!("{exp}{cap}"))))
                    .size(11.5)
                    .color(chrome().muted),
            );
        } else {
            ui.label(
                RichText::new(t("尚未保存长期 Token。也可设环境变量 JIAN_REFRESH_TOKEN。"))
                    .size(11.5)
                    .color(chrome().muted),
            );
        }
        if draft.sources.jian.access_token.is_some() {
            ui.label(
                RichText::new(t("短期 at_ 由 JIAN_ACCESS_TOKEN 注入（不写入文件）。"))
                    .size(11.5)
                    .color(chrome().muted),
            );
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new(t("速报（信息事件）源开关"))
                .strong()
                .size(14.0)
                .color(chrome().text),
        );
        ui.add_space(6.0);
        filters_ui::draw_record_source_switches(ui, draft);
    });

    filters_ui::draw_record_mag_filters(ui, draft);

    section(ui, "Wolfx", "\u{25ce}", |ui| {
        control_row(ui, "启用 Wolfx", "连接 Wolfx 通道", "\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.enabled);
        });
        row_divider(ui);
        control_row(ui, "JMA EEW", "jma_eew", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.jma_eew);
        });
        row_divider(ui);
        control_row(ui, "四川 EEW", "sc_eew", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.sc_eew);
        });
        row_divider(ui);
        control_row(ui, "福建 EEW", "fj_eew", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.fj_eew);
        });
        row_divider(ui);
        control_row(ui, "重庆 EEW", "cq_eew", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.cq_eew);
        });
        row_divider(ui);
        control_row(ui, "CENC EEW", "cenc_eew", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.cenc_eew);
        });
        row_divider(ui);
        control_row(ui, "CWA EEW", "cwa_eew", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.cwa_eew);
        });
        row_divider(ui);
        control_row(ui, "CENC 速报", "cenc_eqlist", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.cenc_eqlist);
        });
        row_divider(ui);
        control_row(ui, "JMA 速报", "jma_eqlist", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.jma_eqlist);
        });
    });

    section(ui, "FSSN", "\u{25ce}", |ui| {
        control_row(ui, "启用 FSSN", "Fan Studio 地震信息", "\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.fssn.enabled);
        });
    });

    section(ui, "P2PQuake", "\u{25ce}", |ui| {
        control_row(ui, "启用 P2PQuake", "气象厅地震情报", "\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.p2pquake.enabled);
        });
        row_divider(ui);
        control_row(ui, "轮询间隔（秒）", "最低 1，默认 1.5；保存后立即重连", "\u{25ce}", |ui| {
            ui.add(
                egui::DragValue::new(&mut draft.sources.p2pquake.poll_secs)
                    .speed(0.1)
                    .range(1.0..=300.0)
                    .fixed_decimals(1),
            );
        });
    });


    section(ui, "PancakesAPI", "\u{25ce}", |ui| {
        control_row(ui, "启用 Pancakes", "GlobalQuake 与 JMA-Atom", "\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.pancakes.enabled);
        });
        if draft.sources.pancakes.enabled {
            row_divider(ui);
            control_row(ui, "GlobalQuake", "全球地震测算推送", "\u{26a1}", |ui| {
                toggle(ui, &mut draft.sources.pancakes.gq);
            });
            row_divider(ui);
            control_row(ui, "GQ 最低震级", "0=不过滤；默认 3.0", "\u{25ce}", |ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.sources.pancakes.gq_min_magnitude)
                        .speed(0.1)
                        .range(0.0..=9.0)
                        .fixed_decimals(1),
                );
            });
            row_divider(ui);
            control_row(ui, "JMA-Atom", "火山活动情报", "\u{25ce}", |ui| {
                toggle(ui, &mut draft.sources.pancakes.jma_volcano);
            });
        }
    });



    section(ui, "台风", "\u{25ce}", |ui| {
        control_row(ui, "启用台风源", "轮询活跃台风路径与风圈", "\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.typhoon.enabled);
        });
        if draft.sources.typhoon.enabled {
            row_divider(ui);
            control_row(ui, "轮询间隔", "秒；默认 15", "\u{25ce}", |ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.sources.typhoon.poll_secs)
                        .speed(1.0)
                        .range(10..=120)
                        .suffix(" s"),
                );
            });
        }
    });

    section(ui, "气象预警", "\u{25ce}", |ui| {
        control_row(
            ui,
            "启用气象预警",
            "Jian /weather 中央台预警（需 at_/rt_）",
            "\u{26a1}",
            |ui| {
                toggle(ui, &mut draft.sources.weather.enabled);
            },
        );
    });

    section(ui, "烈度速报", "\u{25ce}", |ui| {
        control_row(
            ui,
            "启用烈度速报",
            "NowQuake CENC 计测烈度（GET 冷启动 + WS）",
            "\u{26a1}",
            |ui| {
                toggle(ui, &mut draft.sources.cenc_int.enabled);
            },
        );
    });

    section(ui, "实时测站", "\u{25ce}", |ui| {
        control_row(ui, "日本 JMA/NIED", "总开关", "\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.stations.jma_enabled);
        });
        if draft.sources.stations.jma_enabled {
            row_divider(ui);
            control_row(ui, "JMA 测站源", "Jian Project / Kmoni / lmoni", "\u{25ce}", |ui| {
                egui::ComboBox::from_id_salt("set_jma_station_src")
                    .selected_text(jma_station_src_label(&draft.sources.stations.jma_source))
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "jian".into(),
                            "Jian Project",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "kmoni".into(),
                            "Kmoni",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "lmoni".into(),
                            "lmoni",
                        );
                    });
            });
        }
        row_divider(ui);
        control_row(ui, "S-Net", "海上强震网", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.stations.snet_enabled);
        });
        if draft.sources.stations.snet_enabled {
            row_divider(ui);
            control_row(ui, "S-Net 源", "资料约每分钟；Jian 中转另有秒级对时帧（原版无）", "\u{25ce}", |ui| {
                egui::ComboBox::from_id_salt("set_snet_station_src")
                    .selected_text(snet_station_src_label(&draft.sources.stations.snet_source))
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut draft.sources.stations.snet_source,
                            "jian".into(),
                            "S-Net / Jian Project",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.snet_source,
                            "original".into(),
                            "S-Net",
                        );
                    });
            });
        }
        row_divider(ui);
        control_row(ui, "韩国 KMA", "PEWS 测站（Jian API）", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.stations.kma_enabled);
        });
    });


    exchange
}

fn jian_eew_flag(draft: &AppConfig, fam: starquake_core::AgencyFamily) -> bool {
    use starquake_core::AgencyFamily;
    match fam {
        AgencyFamily::Jma => draft.sources.jian.jma_eew,
        AgencyFamily::Cea => draft.sources.jian.cea_eew,
        AgencyFamily::Cwa => draft.sources.jian.cwa_eew,
        AgencyFamily::Kma => draft.sources.jian.kma_eew,
        AgencyFamily::Sa => draft.sources.jian.sa_eew,
        AgencyFamily::EarlyEst => draft.sources.jian.early_est_eew,
        _ => true,
    }
}

fn set_jian_eew_flag(draft: &mut AppConfig, fam: starquake_core::AgencyFamily, on: bool) {
    use starquake_core::AgencyFamily;
    match fam {
        AgencyFamily::Jma => draft.sources.jian.jma_eew = on,
        AgencyFamily::Cea => draft.sources.jian.cea_eew = on,
        AgencyFamily::Cwa => draft.sources.jian.cwa_eew = on,
        AgencyFamily::Kma => draft.sources.jian.kma_eew = on,
        AgencyFamily::Sa => draft.sources.jian.sa_eew = on,
        AgencyFamily::EarlyEst => draft.sources.jian.early_est_eew = on,
        _ => {}
    }
}

fn status_row(ui: &mut Ui, name: &str, status: HealthStatus, enabled: bool) {
    let (color, label) = if !enabled {
        (chrome().text70, t("关闭"))
    } else {
        match status {
            HealthStatus::Normal => (chrome().status_ok, t("正常")),
            HealthStatus::Fluctuating => (chrome().status_warn, t("波动")),
            HealthStatus::Abnormal => (chrome().status_bad, t("异常")),
        }
    };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.5, color);
        ui.label(RichText::new(name).size(14.0).color(chrome().text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(label).strong().size(13.0).color(color));
        });
    });
}



fn is_projection_flat(draft: &AppConfig) -> bool {
    !draft.map.projection.eq_ignore_ascii_case("globe")
}

fn projection_label(draft: &AppConfig) -> &'static str {
    if draft.map.projection.eq_ignore_ascii_case("globe") {
        t("球体")
    } else {
        t("平面")
    }
}

fn is_basemap_auto(draft: &AppConfig) -> bool {
    matches!(draft.map.basemap.trim().to_ascii_lowercase().as_str(), "auto" | "")
}

fn basemap_preset_label(draft: &AppConfig) -> String {
    match draft.map.basemap.trim().to_ascii_lowercase().as_str() {
        "vector" => t("矢量地图").into(),
        "tiles" | "both" => t("地形地图").into(),
        _ => t("跟随主题（推荐）").into(),
    }
}

fn tab_display(s: &str) -> String {
    match s {
        "records" => t("速报").into(),
        "volcano" => t("火山").into(),
        "weather" => t("气象").into(),
        _ => t("预警").into(),
    }
}

fn perf_device_label(v: &str) -> &'static str {
    if v.eq_ignore_ascii_case("cpu") {
        t("CPU 模式")
    } else {
        t("GPU 模式")
    }
}

fn perf_profile_label(v: &str) -> &'static str {
    match v.to_ascii_lowercase().as_str() {
        "eco" | "节能" => t("节能模式"),
        "extreme" | "极致" => t("极致模式"),
        _ => t("正常模式"),
    }
}

fn theme_display(s: &str) -> String {
    if s.eq_ignore_ascii_case("light") {
        t("亮色").into()
    } else {
        t("暗色").into()
    }
}

fn scale_display(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "jma" => t("气象厅震度").into(),
        "cn" => t("中国烈度").into(),
        _ => t("自动").into(),
    }
}

fn tts_engine_display(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "edge" | "online" | "msedge" => t("Edge TTS（在线）").into(),
        _ => t("离线语音包").into(),
    }
}

fn draw_audio(
    ui: &mut Ui,
    draft: &mut AppConfig,
    offline_voices: &[(String, String)],
    edge_voices: &[(String, String)],
) -> Option<SettingsAction> {
    let mut action = None;
    section(ui, "音效设置", "♪", |ui| {
        control_row(
            ui,
            "NHK 一级新闻铃",
            "情报震度达到6弱及以上时播放 NHK 一级新闻铃。",
            "♪",
            |ui| {
                toggle(ui, &mut draft.audio.nhk_news_bell);
            },
        );
        row_divider(ui);
        control_row(
            ui,
            "JMA 警报音",
            "紧急地震速报由预报升级为警报时播放。",
            "♪",
            |ui| {
                toggle(ui, &mut draft.audio.jma_eew_alert_sound);
            },
        );
        row_divider(ui);
        control_row(
            ui,
            "津波警报音",
            "收到津波警报或大津波警报时播放。",
            "♪",
            |ui| {
                toggle(ui, &mut draft.audio.tsunami_ebs_sound);
            },
        );
    });

    section(ui, "系统通知", "🔔", |ui| {
        control_row(ui, "启用系统通知", "预警、速报与海啸弹出桌面 Toast", "◎", |ui| {
            toggle(ui, &mut draft.notify.enabled);
        });
        row_divider(ui);
        control_row(ui, "预警通知", "收到紧急地震速报时通知", "◎", |ui| {
            toggle(ui, &mut draft.notify.on_eew);
        });
        row_divider(ui);
        control_row(ui, "速报通知", "收到地震情报/速报时通知", "◎", |ui| {
            toggle(ui, &mut draft.notify.on_record);
        });
        row_divider(ui);
        control_row(ui, "海啸通知", "收到海啸情报时通知", "◎", |ui| {
            toggle(ui, &mut draft.notify.on_tsunami);
        });
        row_divider(ui);
        control_row(ui, "测试通知", "立即发送一条桌面 Toast，用于确认系统通知可用", "◎", |ui| {
            if ui.button(t("发送测试")).clicked() {
                action = Some(SettingsAction::TestNotify);
            }
        });
    });

    section(ui, "音频设置", "♪", |ui| {
        control_row(ui, "静音", "", "◎", |ui| {
            toggle(ui, &mut draft.audio.mute);
        });
        row_divider(ui);
        control_row(ui, "主音量", "", "◎", |ui| {
            ui.add(egui::Slider::new(&mut draft.audio.master_volume, 0.0..=1.0).show_value(true));
        });
        row_divider(ui);
        control_row(ui, "倒计时音量", "语音与滴答", "◎", |ui| {
            ui.add(
                egui::Slider::new(&mut draft.audio.countdown_volume, 0.0..=1.0).show_value(true),
            );
        });
        row_divider(ui);
        control_row(ui, "倒计时音效", "本机 S 波剩余时播倒计时", "◎", |ui| {
            toggle(ui, &mut draft.audio.play_countdown);
        });
        row_divider(ui);
        control_row(ui, "倒计时语音", "播报十、九…；关闭则只留滴答", "◎", |ui| {
            toggle(ui, &mut draft.audio.countdown_speech);
        });
        row_divider(ui);
        control_row(ui, "开始播报秒数", "剩余秒数降到此值后开始", "◎", |ui| {
            ui.add(
                egui::DragValue::new(&mut draft.audio.countdown_start)
                    .range(0..=60)
                    .speed(1.0),
            );
        });
        row_divider(ui);
        control_row(ui, "仅强有感时播", "本机预估震度≥3 或烈度≥5 时播倒计时", "◎", |ui| {
            toggle(ui, &mut draft.audio.countdown_only_intense);
        });
        row_divider(ui);
        control_row(ui, "试听音效", "按草稿音量立即试听（无需先保存）", "▶", |ui| {
            if glass_btn(ui, "试听首报音", true).clicked() {
                action = Some(SettingsAction::TestSound {
                    mute: draft.audio.mute,
                    master_volume: draft.audio.master_volume,
                    countdown_volume: draft.audio.countdown_volume,
                    event_pack: draft.audio.event_pack.clone(),
                    countdown_pack: draft.audio.countdown_pack.clone(),
                });
            }
        });
    });

    section(ui, "语音播报", "◎", |ui| {
        control_row(
            ui,
            "启用 TTS",
            "音效播放后，再朗读预警/速报文案。",
            "◎",
            |ui| {
                toggle(ui, &mut draft.audio.tts_enabled);
            },
        );
        row_divider(ui);
        control_row(
            ui,
            "TTS 引擎",
            "离线用本机语音包；Edge 需联网。",
            "◎",
            |ui| {
                egui::ComboBox::from_id_salt("set_tts_engine")
                    .selected_text(tts_engine_display(&draft.audio.tts_engine))
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut draft.audio.tts_engine,
                            "offline".into(),
                            t("离线语音包"),
                        );
                        ui.selectable_value(
                            &mut draft.audio.tts_engine,
                            "edge".into(),
                            t("Edge TTS（在线）"),
                        );
                    });
            },
        );
        let use_edge = matches!(
            draft.audio.tts_engine.trim().to_ascii_lowercase().as_str(),
            "edge" | "online" | "msedge"
        );
        row_divider(ui);
        if use_edge {
            let edge_label = edge_voices
                .iter()
                .find(|(id, _)| id == &draft.audio.tts_edge_voice)
                .map(|(_, lab)| lab.clone())
                .unwrap_or_else(|| draft.audio.tts_edge_voice.clone());
            control_row(ui, "Edge 音色", "微软 Neural 在线音色", "◎", |ui| {
                egui::ComboBox::from_id_salt("set_tts_edge_voice")
                    .selected_text(edge_label.clone())
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (id, label) in edge_voices {
                            ui.selectable_value(
                                &mut draft.audio.tts_edge_voice,
                                id.clone(),
                                label,
                            );
                        }
                    });
            });
        } else {
            let offline_label = offline_voices
                .iter()
                .find(|(id, _)| id == &draft.audio.tts_voice)
                .map(|(_, lab)| lab.clone())
                .unwrap_or_else(|| {
                    if draft.audio.tts_voice.is_empty() {
                        t("系统默认").to_string()
                    } else {
                        draft.audio.tts_voice.clone()
                    }
                });
            control_row(
                ui,
                "离线语音包",
                "选择系统已安装的 TTS 音色；没有合适选项时用系统默认。",
                "◎",
                |ui| {
                    egui::ComboBox::from_id_salt("set_tts_offline_voice")
                        .selected_text(offline_label.clone())
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for (id, label) in offline_voices {
                                let shown = if id.is_empty() {
                                    t("系统默认").to_string()
                                } else {
                                    label.clone()
                                };
                                ui.selectable_value(
                                    &mut draft.audio.tts_voice,
                                    id.clone(),
                                    shown,
                                );
                            }
                        });
                },
            );
        }
        row_divider(ui);
        control_row(ui, "语速", "调低更清楚，调高更紧凑", "◎", |ui| {
            ui.add(
                egui::Slider::new(&mut draft.audio.tts_rate, 0.5..=2.0)
                    .show_value(true)
                    .fixed_decimals(2),
            );
        });
        row_divider(ui);
        control_row(ui, "试听语音", "立即用当前草稿配置播放测试句", "▶", |ui| {
            if glass_btn(ui, "播放测试", true).clicked() {
                let vol = if draft.audio.mute {
                    0.0
                } else {
                    draft.audio.master_volume
                };
                action = Some(SettingsAction::TestTts {
                    engine: draft.audio.tts_engine.clone(),
                    offline_voice: draft.audio.tts_voice.clone(),
                    edge_voice: draft.audio.tts_edge_voice.clone(),
                    rate: draft.audio.tts_rate,
                    volume: vol,
                });
            }
        });
    });

    action
}

fn draw_general(
    ui: &mut Ui,
    draft: &mut AppConfig,
    map_pick: &mut Option<MapPickKind>,
    capture_view: &mut bool,
    perf_diag: &mut Option<String>,
    ip_locate_busy: bool,
) -> bool {
    let mut want_ip = false;
    section(ui, "外观", "\u{25d0}", |ui| {
        control_row(ui, "界面主题", "切换后设置页立即跟随；保存后应用到主界面", "\u{25d0}", |ui| {
            egui::ComboBox::from_id_salt("set_theme")
                .selected_text(theme_display(&draft.ui.theme))
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.ui.theme, "dark".into(), t("暗色"));
                    ui.selectable_value(&mut draft.ui.theme, "light".into(), t("亮色"));
                });
        });
        row_divider(ui);
        control_row(ui, "界面语言", "切换后设置页立即跟随；保存后应用到主界面", "\u{25d0}", |ui| {
            let cur = Lang::parse(&draft.ui.language);
            egui::ComboBox::from_id_salt("set_ui_language")
                .selected_text(cur.native_name())
                .width(140.0)
                .show_ui(ui, |ui| {
                    for lang in Lang::ALL {
                        if ui
                            .selectable_label(cur == lang, lang.native_name())
                            .clicked()
                        {
                            draft.ui.language = lang.as_config().into();
                            crate::i18n::set_lang(lang);
                        }
                    }
                });
        });
        row_divider(ui);
        control_row(ui, "地图底图", "跟随主题，或固定矢量 / 地形地图", "\u{25a3}", |ui| {
            egui::ComboBox::from_id_salt("set_basemap_preset")
                .selected_text(basemap_preset_label(draft))
                .width(200.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(is_basemap_auto(draft), t("跟随主题（推荐）"))
                        .clicked()
                    {
                        draft.map.basemap = "auto".into();
                    }
                    if ui
                        .selectable_label(draft.map.basemap == "vector", t("矢量地图"))
                        .clicked()
                    {
                        draft.map.basemap = "vector".into();
                        draft.ui.theme = "dark".into();
                    }
                    if ui
                        .selectable_label(
                            draft.map.basemap == "tiles" || draft.map.basemap == "both",
                            t("地形地图"),
                        )
                        .clicked()
                    {
                        draft.map.basemap = "tiles".into();
                        draft.ui.theme = "light".into();
                    }
                });
        });
        row_divider(ui);
        control_row(ui, "地图投影", "平面或立体球；球体使用矢量底图", "\u{25d0}", |ui| {
            egui::ComboBox::from_id_salt("set_projection")
                .selected_text(projection_label(draft))
                .width(160.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(is_projection_flat(draft), t("平面"))
                        .clicked()
                    {
                        draft.map.projection = "flat".into();
                    }
                    if ui
                        .selectable_label(
                            draft.map.projection.eq_ignore_ascii_case("globe"),
                            t("球体"),
                        )
                        .clicked()
                    {
                        draft.map.projection = "globe".into();
                    }
                });
        });
        if draft.map.projection.eq_ignore_ascii_case("globe") {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(t("球体模式使用矢量底图（瓦片暂不贴球）"))
                    .small()
                    .color(egui::Color32::from_rgb(0x9C, 0xA3, 0xAF)),
            );
        }
    });

    section(ui, "窗口", "\u{25a3}", |ui| {
        control_row(ui, "窗口置顶", "主窗口始终保持在其他窗口之上", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.always_on_top);
        });
        row_divider(ui);
        control_row(ui, "关闭到托盘", "关闭窗口时隐藏到系统托盘（对齐实况栏）；托盘可「显示窗口」或「退出」", "◎", |ui| {
            toggle(ui, &mut draft.ui.minimize_to_tray);
        });
    });

    section(ui, "地图显示", "\u{25a3}", |ui| {
        control_row(ui, "预警自动跟焦", "新预警或报数更新时地图移到震中", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.auto_follow_eew);
        });
        row_divider(ui);
        control_row(ui, "波圈自动缩放", "随 P/S 波传播自动调整地图缩放", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.auto_zoom_waves);
        });
        row_divider(ui);
        control_row(ui, "P/S 波圈", "在地图上绘制走时波阵面", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_wave_rings);
        });
                row_divider(ui);
        control_row(ui, "烈度图例", "地图左下角显示烈度色标", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_legend);
        });
        row_divider(ui);
        control_row(ui, "火山标注", "地图三角标与火山列表图标", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_volcanoes);
        });
        row_divider(ui);
        control_row(ui, "气象卡片", "地图右上气象悬浮卡片（与侧栏卡片模式配合）", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_weather);
        });
        row_divider(ui);
        
        control_row(ui, "本地震源推算", "用测站到时推算震源（紫色叉；非官方预警）", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_local_hypo);
        });
control_row(ui, "中国雷达", "中央台雷达图叠加（可与日/台同时开）", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.map_wx.radar_china);
        });
        row_divider(ui);
        control_row(ui, "日本雷达", "日本气象厅雷达图叠加", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.map_wx.radar_japan);
        });
        row_divider(ui);
        control_row(ui, "台湾雷达", "中国台湾雷达图叠加", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.map_wx.radar_taiwan);
        });
        row_divider(ui);
control_row(ui, "降水预报", "中国降水落区 24 / 48 / 72 小时；关则不拉取", "\u{25ce}", |ui| {
            let h = draft.sources.map_wx.precip_hours;
            let label = match h {
                24 => "24 h",
                48 => "48 h",
                72 => "72 h",
                _ => t("关"),
            };
            egui::ComboBox::from_id_salt("map_wx_precip")
                .selected_text(label)
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.sources.map_wx.precip_hours, 0, t("关"));
                    ui.selectable_value(&mut draft.sources.map_wx.precip_hours, 24, "24 h");
                    ui.selectable_value(&mut draft.sources.map_wx.precip_hours, 48, "48 h");
                    ui.selectable_value(&mut draft.sources.map_wx.precip_hours, 72, "72 h");
                });
        });
                row_divider(ui);
        control_row(ui, "雷达透明度", "中国 / 日本 / 台湾雷达共用；0 全透，1 不透明", "\u{25ce}", |ui| {
            ui.add(
                egui::Slider::new(&mut draft.sources.map_wx.radar_opacity, 0.0..=1.0)
                    .show_value(true)
                    .fixed_decimals(2),
            );
        });
        row_divider(ui);
        control_row(ui, "降水透明度", "乘在落区色块 alpha 上；0 全透，1 不透明", "\u{25ce}", |ui| {
            ui.add(
                egui::Slider::new(&mut draft.sources.map_wx.precip_opacity, 0.0..=1.0)
                    .show_value(true)
                    .fixed_decimals(2),
            );
        });
row_divider(ui);
        control_row(ui, "台风路径", "活跃台风路径、风圈与预报线", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_typhoons);
        });
        row_divider(ui);
        control_row(ui, "活动断层", "默认关；开启后才加载并绘制 GEM/CAFD 迹线", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_faults);
        });
        row_divider(ui);
        control_row(ui, "晨昏线", "按本机时间绘制夜侧与晨昏分界", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_terminator);
        });
        row_divider(ui);
        control_row(ui, "台风预报源", "地图预报路径所用机构", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("typhoon_fc_agency")
                .selected_text(t(&draft.ui.typhoon_forecast_agency))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for a in ["全部", "中国", "中国台湾", "日本", "美国", "中国香港"] {
                        ui.selectable_value(
                            &mut draft.ui.typhoon_forecast_agency,
                            a.to_string(),
                            t(a),
                        );
                    }
                });
        });
    });

    section(ui, "界面偏好", "\u{2630}", |ui| {
        control_row(ui, "显示侧栏", "列表可关；向右/向下拖关闭；关闭后右缘拉开或地图右键", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_sidebar);
        });
        row_divider(ui);
        control_row(ui, "分离侧栏", "也可拖动侧栏标题分离；分离后双击标题嵌回；竖屏时内嵌为底栏", "\u{25ce}", |ui| {
            let before = draft.ui.sidebar_detached;
            toggle(ui, &mut draft.ui.sidebar_detached);
            if draft.ui.sidebar_detached && !before {
                draft.ui.show_sidebar = true;
            }
        });
        row_divider(ui);
        control_row(ui, "东京时间", "主界面额外显示东京时间", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_nied_clock);
        });
        row_divider(ui);
        control_row(ui, "启动侧栏", "仅下次启动生效", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("gen_default_tab")
                .selected_text(tab_display(&draft.ui.sidebar_default_tab))
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.ui.sidebar_default_tab, "eew".into(), t("预警"));
                    ui.selectable_value(
                        &mut draft.ui.sidebar_default_tab,
                        "records".into(),
                        t("速报"),
                    );
                    ui.selectable_value(
                        &mut draft.ui.sidebar_default_tab,
                        "volcano".into(),
                        t("火山"),
                    );
                    ui.selectable_value(
                        &mut draft.ui.sidebar_default_tab,
                        "weather".into(),
                        t("气象"),
                    );
                });
        });
        row_divider(ui);
        control_row(ui, "烈度标度", "自动跟机构；强制中国烈度时 JMA/CWA 仍用震度", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("gen_intensity_scale")
                .selected_text(scale_display(&draft.ui.intensity_scale))
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.ui.intensity_scale, "auto".into(), t("自动"));
                    ui.selectable_value(&mut draft.ui.intensity_scale, "jma".into(), t("气象厅震度"));
                    ui.selectable_value(&mut draft.ui.intensity_scale, "cn".into(), t("中国烈度"));
                });
        });
    });

    section(ui, "默认视口", "\u{25ce}", |ui| {
        control_row(ui, "经度 / 纬度 / 缩放", "Home 无本机位置时使用", "\u{25ce}", |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.map.default_zoom)
                        .speed(0.1)
                        .range(2.0..=12.0)
                        .prefix("Z "),
                );
                ui.add(
                    egui::DragValue::new(&mut draft.map.default_lat)
                        .speed(0.1)
                        .range(-90.0..=90.0)
                        .prefix("\u{03c6} "),
                );
                ui.add(
                    egui::DragValue::new(&mut draft.map.default_lon)
                        .speed(0.1)
                        .range(-180.0..=180.0)
                        .prefix("\u{03bb} "),
                );
            });
        });
        row_divider(ui);
        let picking = matches!(*map_pick, Some(MapPickKind::DefaultViewport));
        let sub = if picking {
            "点击主窗口地图选点，Esc 取消"
        } else {
            "在主地图单击选点，或采用当前视野中心"
        };
        control_row(ui, "地图选点", sub, "\u{25ce}", |ui| {
            ui.horizontal(|ui| {
                if glass_btn(ui, if picking { "选点中…" } else { "选点" }, picking).clicked() {
                    *map_pick = Some(MapPickKind::DefaultViewport);
                }
                if glass_btn(ui, "当前视野", false).clicked() {
                    *capture_view = true;
                }
            });
        });
    });

    section(ui, "性能与渲染", "\u{25ce}", |ui| {
        control_row(ui, "渲染设备", "GPU=OpenGL（失败自动改 DX12）；CPU=低功耗图形；改设备需重启", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("set_perf_device")
                .selected_text(perf_device_label(&draft.performance.device))
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.performance.device, "cpu".into(), t("CPU 模式"));
                    ui.selectable_value(&mut draft.performance.device, "gpu".into(), t("GPU 模式"));
                });
        });
        row_divider(ui);
        control_row(ui, "性能模式", "节能空闲约 1.5 秒，正常/极致空闲约 1 秒；交互与波圈跟目标帧率（保存后立即生效）", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("set_perf_profile")
                .selected_text(perf_profile_label(&draft.performance.profile))
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.performance.profile, "eco".into(), t("节能模式"));
                    ui.selectable_value(
                        &mut draft.performance.profile,
                        "normal".into(),
                        t("正常模式"),
                    );
                    ui.selectable_value(
                        &mut draft.performance.profile,
                        "extreme".into(),
                        t("极致模式"),
                    );
                });
        });
        row_divider(ui);
        control_row(
            ui,
            "目标帧率",
            "交互/波圈跟手上限（镜头跳转按墙钟，与帧率无关）；空闲另有降频；档位跟常见刷新率",
            "\u{25ce}",
            |ui| {
                draft.performance.target_fps =
                    starquake_config::snap_target_fps(draft.performance.target_fps);
                egui::ComboBox::from_id_salt("set_perf_target_fps")
                    .selected_text(format!("{} fps", draft.performance.target_fps))
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for &fps in &starquake_config::TARGET_FPS_PRESETS {
                            ui.selectable_value(
                                &mut draft.performance.target_fps,
                                fps,
                                format!("{fps} fps"),
                            );
                        }
                    });
            },
        );
        row_divider(ui);
        control_row(ui, "性能诊断", "按 CPU/GPU 天梯、内存与主屏刷新率推荐配置", "\u{25ce}", |ui| {
            if glass_btn(ui, "开始诊断", true).clicked() {
                let d = starquake_util::perf::diagnose();
                draft.performance.device = d.device;
                draft.performance.profile = d.profile;
                draft.performance.target_fps = starquake_config::snap_target_fps(d.target_fps);
                *perf_diag = Some(d.summary);
            }
        });
        if let Some(note) = perf_diag.as_ref() {
            ui.add_space(8.0);
            ui.label(RichText::new(note).size(12.0).color(chrome().muted));
        }
    });

    section(ui, "本机位置", "\u{25ce}", |ui| {
        let mut has_local = draft.local.latitude.is_some() && draft.local.longitude.is_some();
        control_row(ui, "启用本机经纬", "倒计时与 Home 标记", "\u{25ce}", |ui| {
            if toggle(ui, &mut has_local).changed() {
                if has_local {
                    draft.local.latitude.get_or_insert(35.68);
                    draft.local.longitude.get_or_insert(139.76);
                } else {
                    draft.local.latitude = None;
                    draft.local.longitude = None;
                }
            }
        });
        row_divider(ui);
        control_row(
            ui,
            "IP 定位",
            "按公网出口查城市级经纬度（仅供参考）",
            "\u{25ce}",
            |ui| {
                let label = if ip_locate_busy {
                    "定位中…"
                } else {
                    "按 IP 定位"
                };
                if glass_btn(ui, label, ip_locate_busy).clicked() && !ip_locate_busy {
                    want_ip = true;
                }
            },
        );
        if has_local {
            row_divider(ui);
            let mut lat = draft.local.latitude.unwrap_or(35.68);
            let mut lon = draft.local.longitude.unwrap_or(139.76);
            control_row(ui, "纬度 / 经度", "", "\u{25ce}", |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut lon).speed(0.01).range(-180.0..=180.0));
                    ui.add(egui::DragValue::new(&mut lat).speed(0.01).range(-90.0..=90.0));
                });
            });
            draft.local.latitude = Some(lat);
            draft.local.longitude = Some(lon);
            row_divider(ui);
            let picking = matches!(*map_pick, Some(MapPickKind::LocalLocation));
            let sub = if picking {
                "点击主窗口地图选点，Esc 取消"
            } else {
                "在主地图单击设置本机位置"
            };
            control_row(ui, "地图选点", sub, "\u{25ce}", |ui| {
                if glass_btn(ui, if picking { "选点中…" } else { "选点" }, picking).clicked() {
                    *map_pick = Some(MapPickKind::LocalLocation);
                }
            });
        }
    });
    want_ip
}

fn draw_advanced(ui: &mut Ui, draft: &mut AppConfig, action: &mut Option<SettingsAction>) {
    section(ui, "震级传播距离", "\u{25ce}", |ui| {
        ui.label(
            RichText::new(t("自定义：最小震级、最大震级、传播距离(km)；留空用内置表。"))
                .size(12.0)
                .color(chrome().muted),
        );
        ui.add_space(8.0);
        let mut remove: Option<usize> = None;
        for (i, pt) in draft.travel.mag_spread_km.iter_mut().enumerate() {
            if i > 0 {
                row_divider(ui);
            }
            let title = format!("{} {}", t("档位"), i + 1);
            control_row(ui, &title, "最小震级 / 最大震级 / 传播距离", "\u{25ce}", |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut pt.min_magnitude)
                            .speed(0.1)
                            .range(0.0..=9.5)
                            .prefix("min M")
                            .fixed_decimals(1),
                    );
                    ui.add(
                        egui::DragValue::new(&mut pt.max_magnitude)
                            .speed(0.1)
                            .range(0.0..=9.5)
                            .prefix("max M")
                            .fixed_decimals(1),
                    );
                    ui.add(
                        egui::DragValue::new(&mut pt.max_km)
                            .speed(10.0)
                            .range(50.0..=9000.0)
                            .suffix(" km"),
                    );
                    if glass_btn(ui, "删", false).clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            draft.travel.mag_spread_km.remove(i);
        }
        row_divider(ui);
        control_row(ui, "添加档位", "命中区间则用该档距离", "\u{25ce}", |ui| {
            if glass_btn(ui, "添加", false).clicked() {
                let last_hi = draft
                    .travel
                    .mag_spread_km
                    .last()
                    .map(|p| p.max_magnitude)
                    .unwrap_or(2.5);
                draft.travel.mag_spread_km.push(starquake_config::MagSpreadPoint {
                    min_magnitude: last_hi,
                    max_magnitude: (last_hi + 1.0).min(9.5),
                    max_km: 500.0,
                });
            }
            if glass_btn(ui, "恢复内置", false).clicked() {
                draft.travel.mag_spread_km.clear();
            }
        });
    });

    section(ui, "走时表", "\u{25ce}", |ui| {
        control_row(ui, "主表（近距）", "默认 JMA2001", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("adv_travel_primary")
                .selected_text(draft.travel.primary.clone())
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.travel.primary, "jma2001".into(), "jma2001");
                    ui.selectable_value(&mut draft.travel.primary, "ak135".into(), "ak135");
                });
        });
        row_divider(ui);
        control_row(ui, "远距回退", "主表插值失败时使用", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("adv_travel_fallback")
                .selected_text(draft.travel.fallback.clone())
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.travel.fallback, "ak135".into(), "ak135");
                    ui.selectable_value(&mut draft.travel.fallback, "jma2001".into(), "jma2001");
                });
        });
        row_divider(ui);
        control_row(ui, "常速直线兜底", "插值失败时启用", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.travel.linear_last_resort);
        });
    });

    section(ui, "瓦片缓存", "\u{25ce}", |ui| {
        control_row(ui, "内存瓦片缓存", "清除后将重新下载可见瓦片", "\u{25ce}", |ui| {
            if glass_btn(ui, "一键清除", false).clicked() {
                *action = Some(SettingsAction::ClearTileCache);
            }
        });
    });

    section(ui, "运行日志", "\u{25ce}", |ui| {
        control_row(ui, "写入文件", "关闭后仅控制台（若有）", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.log.enabled);
        });
        row_divider(ui);
        control_row(ui, "日志等级", "保存后下次启动生效；也可设 RUST_LOG", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("adv_log_level")
                .selected_text(draft.log.level.clone())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for lv in ["error", "warn", "info", "debug", "trace"] {
                        ui.selectable_value(&mut draft.log.level, lv.to_string(), lv);
                    }
                });
        });
        row_divider(ui);
        control_row(ui, "同时输出 stderr", "保存后立即开关 stderr 日志（不分配控制台窗口）", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.log.console);
        });
        row_divider(ui);
        control_row(ui, "保留天数", "按文件修改时间清理旧日志；0=不清理", "\u{25ce}", |ui| {
            ui.add(
                egui::DragValue::new(&mut draft.log.keep_days)
                    .range(0..=365)
                    .speed(1.0),
            );
        });
        row_divider(ui);
        control_row(ui, "日志目录", "相对资源根或绝对路径", "\u{25ce}", |ui| {
            framed_edit(ui, &mut draft.log.dir, 180.0);
        });
        row_divider(ui);
        control_row(ui, "打开目录", "在资源管理器中查看 starquake.log.*", "\u{25ce}", |ui| {
            if glass_btn(ui, "打开日志文件夹", false).clicked() {
                *action = Some(SettingsAction::OpenLogDir);
            }
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(t("日志等级与目录保存后需重启才会应用到写入器；stderr 开关保存后立即生效。"))
                .size(12.0)
                .color(chrome().muted),
        );
    });

    section(ui, "软件更新", "\u{21bb}", |ui| {
        control_row(ui, "启动前检查更新", "进入主界面前拉取清单；有新版本先询问", "\u{21bb}", |ui| {
            toggle(ui, &mut draft.update.check_on_startup);
        });
        row_divider(ui);
        control_row(ui, "确认后自动安装", "确认后下载并替换本机程序（关闭则仅打开发布页）", "\u{21bb}", |ui| {
            toggle(ui, &mut draft.update.auto_install);
        });
        row_divider(ui);
        control_row(ui, "检查更新", "立即拉取 update/manifest.json", "\u{21bb}", |ui| {
            if glass_btn(ui, "检查更新", true).clicked() {
                *action = Some(SettingsAction::CheckForUpdate);
            }
        });
    });

    section(ui, "配置文件", "\u{25ce}", |ui| {
        if let Some(path) = AppConfig::user_config_path() {
            ui.label(
                RichText::new(format!("{}", t("用户覆盖：{}").replace("{}", &path.display().to_string())))
                    .size(12.0)
                    .color(chrome().muted),
            );
        } else {
            ui.label(
                RichText::new(t("未能解析用户配置目录。"))
                    .size(12.0)
                    .color(chrome().muted),
            );
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(t("WebSocket 等开发向端点请直接编辑配置文件；访问令牌不会写入覆盖文件。"))
                .size(12.0)
                .color(chrome().muted),
        );
    });
}

fn jma_station_src_label(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "kmoni" => "Kmoni".into(),
        "lmoni" => "lmoni".into(),
        _ => "Jian Project".into(),
    }
}

fn snet_station_src_label(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "original" => "S-Net".into(),
        _ => "S-Net / Jian Project".into(),
    }
}

fn draw_notices(ui: &mut Ui, notices: &[OfficialNoticeView]) -> Option<SettingsAction> {
    let mut action = None;
    let unread = notices.iter().filter(|n| n.unread).count();
    section(ui, "官方通知", "\u{1f514}", |ui| {
        ui.label(
            RichText::new(format!(
                "{}",
                t("未读 {} 条 · 每 15 秒自动同步").replace("{}", &unread.to_string())
            ))
            .size(12.0)
            .color(chrome().muted),
        );
        ui.add_space(8.0);
        if unread > 0 && glass_btn(ui, "全部标为已读", true).clicked() {
            action = Some(SettingsAction::MarkAllNoticesRead);
        }
    });
    ui.add_space(8.0);
    if notices.is_empty() {
        ui.label(
            RichText::new(t("暂无通知"))
                .size(13.0)
                .color(chrome().muted),
        );
        return action;
    }
    for n in notices {
        egui::Frame::NONE
            .fill(if n.unread {
                Color32::from_rgba_unmultiplied(0xF5, 0x9E, 0x0B, 28)
            } else {
                Color32::from_rgba_unmultiplied(0x80, 0x80, 0x80, 16)
            })
            .inner_margin(Margin::same(10))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&n.type_label)
                            .size(11.0)
                            .color(CAT_NOTICE),
                    );
                    if n.unread {
                        ui.label(
                            RichText::new(t("未读"))
                                .size(11.0)
                                .strong()
                                .color(Color32::from_rgb(0xEF, 0x44, 0x44)),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(&n.created_at)
                                .size(11.0)
                                .color(chrome().muted),
                        );
                    });
                });
                ui.label(
                    RichText::new(&n.title)
                        .size(14.0)
                        .strong()
                        .color(chrome().text),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(&n.content)
                        .size(12.5)
                        .color(chrome().text),
                );
                if n.unread {
                    ui.add_space(6.0);
                    if glass_btn(ui, "标为已读", true).clicked() {
                        action = Some(SettingsAction::MarkNoticeRead(n.id));
                    }
                }
            });
        ui.add_space(8.0);
    }
    action
}

fn draw_about(ui: &mut Ui, _draft: &mut AppConfig) {
    if let Some(tex) = crate::logo_texture(ui.ctx()) {
        ui.add(egui::Image::new((tex.id(), egui::Vec2::splat(64.0))));
        ui.add_space(10.0);
    }
    section(ui, crate::APP_DISPLAY_NAME, "\u{24d8}", |ui| {
        ui.label(
            RichText::new(format!(
                "{}",
                t("版本 {}").replace("{}", crate::APP_VERSION_DISPLAY)
            ))
            .size(14.0)
            .color(chrome().text),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(t(
                "跨平台地震预警与速报视监客户端。聚合多源实时情报，在地图上展示震中、走时波圈与测站烈度，并提供本机倒计时与音效播报。",
            ))
            .size(13.0)
            .color(chrome().text),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(t("数据源"))
                .size(13.0)
                .strong()
                .color(chrome().text),
        );
        ui.label(
            RichText::new(t(
                "\u{00b7} Jian Project API\n\u{00b7} PancakesAPI\n\u{00b7} Wolfx\n\u{00b7} P2PQuake\n\u{00b7} FSSN",
            ))
            .size(12.0)
            .color(chrome().muted),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(t("致谢"))
                .size(13.0)
                .strong()
                .color(chrome().text),
        );
        ui.label(
            RichText::new(t(
                "\u{00b7} 图标提供：平平无奇的4\n\u{00b7} 音效来源：SREV / Open EEW CN\n\u{00b7} 倒计时语音：地牛 Wake Up!\n\u{00b7} 授权参考：悦琳雅韵\u{2606}Rhythm(Stille)\n\u{00b7} 图标提供：lxfly2000\n\u{00b7} 以及各位给与帮助的EEW爱好者",
            ))
            .size(12.0)
            .color(chrome().muted),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(t("参考软件"))
                .size(13.0)
                .strong()
                .color(chrome().text),
        );
        ui.label(
            RichText::new(t(
                "\u{00b7} EEW CN\n\u{00b7} GlobalQuake 0.11\n\u{00b7} Kanameishi\n\u{00b7} RhythmQuake\n\u{00b7} Scratch-Realtime-Earthquake-Viewer-Page",
            ))
            .size(12.0)
            .color(chrome().muted),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(t("版权声明"))
                .size(13.0)
                .strong()
                .color(chrome().text),
        );
        ui.label(
            RichText::new(t(
                "本项目参考以下软件的源代码。\n\u{00b7} RhythmQuake\n\u{00b7} Kanameishi\n\u{00b7} OPEN EEWCN\n\u{00b7} GlobalQuake 0.11",
            ))
            .size(12.0)
            .color(chrome().muted),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new(t("免责声明"))
                .size(13.0)
                .strong()
                .color(chrome().text),
        );
        ui.label(
            RichText::new(t(
                "本软件仅供信息展示与学习研究，不构成官方预警渠道。请以各国气象/地震主管部门发布为准；紧急情况下请遵从当地防灾指引。开发者不保证数据的实时性、完整性与准确性。",
            ))
            .size(12.0)
            .color(chrome().muted),
        );
    });
}
