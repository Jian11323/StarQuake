//! 系统托盘：对齐「滚动字幕」行为。
//!
//! - 关闭 → hide（Win32 SW_HIDE），托盘右键「显示窗口 / 退出」
//! - 显示 → showNormal（SW_RESTORE + SW_SHOW + 前台）
//! - 退出 → `_force_quit`，真正关闭
//! - 系统通知优先走托盘 balloon（等同 Qt `QSystemTrayIcon.showMessage`）
//!
//! eframe 在 `Visible(false)` 后常不调 `update`：隐藏期间用 heartbeat
//! `request_repaint`，保证托盘命令与网络事件仍被处理。

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    Quit,
}

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static TRAY_UID: AtomicU32 = AtomicU32::new(0);
static FORCE_QUIT: AtomicBool = AtomicBool::new(false);
static HIDDEN_TO_TRAY: AtomicBool = AtomicBool::new(false);
/// 最小化（任务栏）：与托盘隐藏一样需要 heartbeat 唤醒 update，否则网络/音效停摆。
static MINIMIZED_BG: AtomicBool = AtomicBool::new(false);
static HEARTBEAT_STARTED: AtomicBool = AtomicBool::new(false);

pub fn set_main_hwnd(hwnd: isize) {
    if hwnd != 0 {
        MAIN_HWND.store(hwnd, Ordering::SeqCst);
    }
}

pub fn take_force_quit() -> bool {
    FORCE_QUIT.swap(false, Ordering::SeqCst)
}

pub fn request_force_quit() {
    FORCE_QUIT.store(true, Ordering::SeqCst);
}

pub fn set_hidden_to_tray(hidden: bool) {
    HIDDEN_TO_TRAY.store(hidden, Ordering::SeqCst);
}

pub fn set_minimized_bg(minimized: bool) {
    MINIMIZED_BG.store(minimized, Ordering::SeqCst);
}

fn needs_background_heartbeat() -> bool {
    HIDDEN_TO_TRAY.load(Ordering::SeqCst) || MINIMIZED_BG.load(Ordering::SeqCst)
}

pub struct TrayHandle {
    rx: Receiver<TrayCmd>,
    _tray: Option<tray_icon::TrayIcon>,
    wake: Arc<Mutex<Option<egui::Context>>>,
}

impl TrayHandle {
    pub fn try_create(icon_rgba: Option<(Vec<u8>, u32, u32)>) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let wake = Arc::new(Mutex::new(None));
        match build_tray(tx, icon_rgba, wake.clone()) {
            Ok(tray) => {
                #[cfg(windows)]
                discover_tray_notify_target();
                Some(Self {
                    rx,
                    _tray: Some(tray),
                    wake,
                })
            }
            Err(e) => {
                warn!(error = %e, "system tray unavailable");
                None
            }
        }
    }

    pub fn bind_ctx(&self, ctx: &egui::Context) {
        if let Ok(mut g) = self.wake.lock() {
            *g = Some(ctx.clone());
        }
        ensure_heartbeat(self.wake.clone());
    }

    pub fn poll(&self) -> Option<TrayCmd> {
        self.rx.try_recv().ok()
    }
}

fn wake_ui(wake: &Arc<Mutex<Option<egui::Context>>>) {
    if let Ok(g) = wake.lock() {
        if let Some(ctx) = g.as_ref() {
            ctx.request_repaint();
        }
    }
}

fn ensure_heartbeat(wake: Arc<Mutex<Option<egui::Context>>>) {
    if HEARTBEAT_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("tray-heartbeat".into())
        .spawn(move || loop {
            if needs_background_heartbeat() {
                wake_ui(&wake);
                std::thread::sleep(Duration::from_millis(200));
            } else {
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
}

fn build_tray(
    tx: Sender<TrayCmd>,
    icon_rgba: Option<(Vec<u8>, u32, u32)>,
    wake: Arc<Mutex<Option<egui::Context>>>,
) -> Result<tray_icon::TrayIcon, Box<dyn std::error::Error>> {
    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let icon = match icon_rgba {
        Some((rgba, w, h)) => Icon::from_rgba(rgba, w, h)?,
        None => {
            let mut rgba = vec![0u8; 16 * 16 * 4];
            for px in rgba.chunks_exact_mut(4) {
                px[0] = 0x1a;
                px[1] = 0x5f;
                px[2] = 0xb0;
                px[3] = 0xff;
            }
            Icon::from_rgba(rgba, 16, 16)?
        }
    };

    // 对齐滚动字幕菜单：「显示窗口」「退出」
    let show_label = "\u{663e}\u{793a}\u{7a97}\u{53e3}";
    let quit_label = "\u{9000}\u{51fa}";
    let show_item = MenuItem::with_id("show", show_label, true, None);
    let quit_item = MenuItem::with_id("quit", quit_label, true, None);
    let menu = Menu::new();
    menu.append(&show_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("StarQuake")
        .with_icon(icon)
        .build()?;

    let tx_click = tx.clone();
    let wake_click = wake.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => {
                win_show_normal();
                let _ = tx_click.send(TrayCmd::Show);
                wake_ui(&wake_click);
            }
            _ => {}
        }
    }));

    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let id = event.id.0.as_str();
        match id {
            "show" => {
                win_show_normal();
                let _ = tx.send(TrayCmd::Show);
                wake_ui(&wake);
            }
            "quit" => {
                request_force_quit();
                set_hidden_to_tray(false);
                win_show_normal();
                win_post_close();
                let _ = tx.send(TrayCmd::Quit);
                wake_ui(&wake);
            }
            other => debug!(id = other, "tray menu ignored"),
        }
    }));

    Ok(tray)
}

/// 对齐滚动字幕 `_show_from_tray`。
pub fn win_show_normal() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, BringWindowToTop, SetForegroundWindow, ShowWindow,
            ASFW_ANY, SW_RESTORE, SW_SHOW,
        };
        let hwnd = MAIN_HWND.load(Ordering::SeqCst);
        if hwnd == 0 {
            return;
        }
        unsafe {
            ShowWindow(hwnd as _, SW_RESTORE);
            ShowWindow(hwnd as _, SW_SHOW);
            BringWindowToTop(hwnd as _);
            AllowSetForegroundWindow(ASFW_ANY);
            SetForegroundWindow(hwnd as _);
        }
    }
}

/// 对齐滚动字幕 `hide()`。
pub fn win_hide() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        let hwnd = MAIN_HWND.load(Ordering::SeqCst);
        if hwnd == 0 {
            return;
        }
        unsafe {
            ShowWindow(hwnd as _, SW_HIDE);
        }
    }
}

fn win_post_close() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        let hwnd = MAIN_HWND.load(Ordering::SeqCst);
        if hwnd == 0 {
            return;
        }
        unsafe {
            let _ = PostMessageW(hwnd as _, WM_CLOSE, 0, 0);
        }
    }
}

/// 对齐滚动字幕 `QSystemTrayIcon.showMessage`。
pub fn show_balloon(title: &str, body: &str) -> bool {
    #[cfg(windows)]
    {
        discover_tray_notify_target();
        let hwnd = TRAY_HWND.load(Ordering::SeqCst);
        let uid = TRAY_UID.load(Ordering::SeqCst);
        if hwnd == 0 {
            return false;
        }
        if uid != 0 && balloon_on_icon(hwnd, uid, title, body) {
            return true;
        }
        for try_uid in 1u32..=32 {
            if balloon_on_icon(hwnd, try_uid, title, body) {
                TRAY_UID.store(try_uid, Ordering::SeqCst);
                return true;
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
        false
    }
}

#[cfg(windows)]
fn discover_tray_notify_target() {
    if TRAY_HWND.load(Ordering::SeqCst) != 0 {
        return;
    }
    let mut found: isize = 0;
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(
            Some(enum_tray_windows),
            &mut found as *mut isize as isize,
        );
    }
    if found != 0 {
        TRAY_HWND.store(found, Ordering::SeqCst);
        // tray-icon Builder 通常让内部 uID=2；正式发送时仍会扫描
        if TRAY_UID.load(Ordering::SeqCst) == 0 {
            TRAY_UID.store(2, Ordering::SeqCst);
        }
        debug!(hwnd = found, "tray hwnd resolved");
    }
}

#[cfg(windows)]
unsafe extern "system" fn enum_tray_windows(
    hwnd: windows_sys::Win32::Foundation::HWND,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::BOOL {
    use windows_sys::Win32::Foundation::{FALSE, TRUE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetWindowThreadProcessId,
    };

    let out = &mut *(lparam as *mut isize);
    let mut class = [0u16; 64];
    let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
    if n <= 0 {
        return TRUE;
    }
    let class_name = String::from_utf16_lossy(&class[..n as usize]);
    if class_name != "tray_icon_app" {
        return TRUE;
    }
    let mut wnd_pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut wnd_pid);
    if wnd_pid != GetCurrentProcessId() {
        return TRUE;
    }
    *out = hwnd as isize;
    FALSE
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
}

#[cfg(windows)]
fn balloon_on_icon(hwnd: isize, uid: u32, title: &str, body: &str) -> bool {
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIM_MODIFY, NOTIFYICONDATAW,
    };

    let title_w = to_wide_trunc(title, 63);
    let body_w = to_wide_trunc(body, 255);
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd as _;
        nid.uID = uid;
        nid.uFlags = NIF_INFO;
        nid.Anonymous.uTimeout = 8_000;
        nid.dwInfoFlags = NIIF_INFO;
        for (i, c) in title_w.iter().enumerate().take(63) {
            nid.szInfoTitle[i] = *c;
        }
        for (i, c) in body_w.iter().enumerate().take(255) {
            nid.szInfo[i] = *c;
        }
        Shell_NotifyIconW(NIM_MODIFY, &mut nid) != 0
    }
}

fn to_wide_trunc(s: &str, max_chars: usize) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().take(max_chars).collect();
    if v.len() == max_chars {
        v.pop();
    }
    v.push(0);
    v
}
