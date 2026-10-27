//! 中央台气象预警图标（`p00…`）异步拉取与纹理缓存。

use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

const MAX_CACHED: usize = 64;
const MAX_IN_FLIGHT: usize = 4;

/// `https://weather.cma.cn/assets/img/alarm/{type}.png`
const CMA_ALARM_BASE: &str = "https://weather.cma.cn/assets/img/alarm";
/// 备用：NMC 同路径
const NMC_ALARM_BASE: &str = "https://www.nmc.cn/assets/img/alarm";

enum IconState {
    Pending,
    Ready(TextureHandle),
    Failed,
}

struct Decoded {
    key: String,
    image: Option<ColorImage>,
}

pub struct WeatherIconCache {
    entries: HashMap<String, IconState>,
    lru: VecDeque<String>,
    tx: Sender<Decoded>,
    rx: Receiver<Decoded>,
    in_flight: usize,
    want: VecDeque<String>,
}

impl Default for WeatherIconCache {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            entries: HashMap::new(),
            lru: VecDeque::new(),
            tx,
            rx,
            in_flight: 0,
            want: VecDeque::new(),
        }
    }
}

impl WeatherIconCache {
    /// 请求并返回已就绪的纹理；未就绪时排队拉取，失败返回 `None`。
    pub fn get(&mut self, ctx: &Context, icon_type: &str) -> Option<TextureHandle> {
        self.poll(ctx);
        let key = normalize_icon_type(icon_type)?;
        if !self.entries.contains_key(&key) {
            self.enqueue(key.clone());
        }
        self.spawn_jobs();
        if matches!(self.entries.get(&key), Some(IconState::Ready(_))) {
            self.touch_lru(&key);
        }
        match self.entries.get(&key) {
            Some(IconState::Ready(tex)) => Some(tex.clone()),
            _ => None,
        }
    }

    pub fn poll(&mut self, ctx: &Context) {
        while let Ok(decoded) = self.rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            match decoded.image {
                Some(img) => {
                    let tex = ctx.load_texture(
                        format!("weather-alarm-{}", decoded.key),
                        img,
                        TextureOptions::LINEAR,
                    );
                    self.entries
                        .insert(decoded.key.clone(), IconState::Ready(tex));
                    self.touch_lru(&decoded.key);
                }
                None => {
                    self.entries.insert(decoded.key, IconState::Failed);
                }
            }
            // 走地图同款下限，避免空闲时 40ms 抢跑
            starquake_map::request_repaint_after_capped(ctx, 40);
        }
        self.spawn_jobs();
    }

    fn enqueue(&mut self, key: String) {
        if self.entries.contains_key(&key) {
            return;
        }
        if self.want.iter().any(|k| *k == key) {
            return;
        }
        self.want.push_back(key.clone());
        self.entries.insert(key, IconState::Pending);
    }

    fn spawn_jobs(&mut self) {
        while self.in_flight < MAX_IN_FLIGHT {
            let Some(key) = self.want.pop_front() else {
                break;
            };
            if !matches!(self.entries.get(&key), Some(IconState::Pending)) {
                continue;
            }
            let tx = self.tx.clone();
            self.in_flight += 1;
            let _ = thread::Builder::new()
                .name("weather-icon".into())
                .spawn(move || {
                    let image = fetch_alarm_png(&key);
                    let _ = tx.send(Decoded { key, image });
                });
        }
    }

    fn touch_lru(&mut self, key: &str) {
        if let Some(pos) = self.lru.iter().position(|k| k == key) {
            self.lru.remove(pos);
        }
        self.lru.push_back(key.to_string());
        while self.lru.len() > MAX_CACHED {
            if let Some(old) = self.lru.pop_front() {
                if !matches!(self.entries.get(&old), Some(IconState::Pending)) {
                    self.entries.remove(&old);
                }
            }
        }
    }
}

fn normalize_icon_type(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    // 仅允许中央台图标码字符，避免拼进 URL
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    if s.len() > 32 {
        return None;
    }
    Some(s)
}

fn fetch_alarm_png(icon_type: &str) -> Option<ColorImage> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("StarQuake/0.1")
        .build()
        .ok()?;
    for base in [CMA_ALARM_BASE, NMC_ALARM_BASE] {
        let url = format!("{base}/{icon_type}.png");
        match client.get(&url).send() {
            Ok(r) if r.status().is_success() => {
                let bytes = r.bytes().ok()?;
                let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
                let size = [img.width() as usize, img.height() as usize];
                return Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()));
            }
            Ok(r) => {
                tracing::debug!(%url, status = %r.status(), "weather icon http fail");
            }
            Err(e) => {
                tracing::debug!(%url, error = %e, "weather icon fetch err");
            }
        }
    }
    None
}

/// 图标码末三位等级色：001 红 / 002 橙 / 003 黄 / 004 蓝。
pub fn weather_level_color(icon_type: &str) -> Color32 {
    let digits: String = icon_type
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    let code = if digits.len() >= 3 {
        &digits[digits.len() - 3..]
    } else {
        ""
    };
    match code {
        "001" => Color32::from_rgb(0xE6, 0x00, 0x12),
        "002" => Color32::from_rgb(0xFA, 0x8C, 0x16),
        "003" => Color32::from_rgb(0xFA, 0xDB, 0x14),
        "004" => Color32::from_rgb(0x18, 0x90, 0xFF),
        _ => Color32::from_rgb(0x8C, 0x8C, 0x8C),
    }
}
