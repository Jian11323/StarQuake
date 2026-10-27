//! 配置加载。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub ui: UiConfig,
    #[serde(default)]
    pub performance: PerformanceConfig,
    /// 主窗口位置与大小（调整后自动保存）
    #[serde(default)]
    pub window: WindowConfig,
    pub map: MapConfig,
    pub audio: AudioConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
    pub travel: TravelConfig,
    pub local: LocalConfig,
    pub sources: SourcesConfig,
    /// 速报震级阈值与地点白名单
    #[serde(default)]
    pub filters: FiltersConfig,
    /// 文件日志（便携版无控制台时仍写入资源根下 logs/）
    #[serde(default)]
    pub log: LogConfig,
    /// 自动更新（静态 update/manifest.json）
    #[serde(default)]
    pub update: UpdateConfig,
}

/// 启动检查与一键安装更新。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// 启动主窗口前拉取更新清单
    #[serde(default = "default_true")]
    pub check_on_startup: bool,
    /// 用户确认后自动下载并应用（退出后替换便携包 / 启动安装包）
    #[serde(default = "default_true")]
    pub auto_install: bool,
    /// 启动询问时点「否」记下的远程版本；与清单一致则不再弹窗（设置里手动检查仍会问）
    #[serde(default)]
    pub last_dismissed_update_version: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            auto_install: true,
            last_dismissed_update_version: String::new(),
        }
    }
}

/// 运行日志：等级、目录、是否同时打 stderr、保留天数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// 是否写文件（关闭则仅依赖 `RUST_LOG` + 控制台层，若有）
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `error` | `warn` | `info` | `debug` | `trace`（可被环境变量 `RUST_LOG` 覆盖）
    #[serde(default = "default_log_level")]
    pub level: String,
    /// 相对资源根或绝对路径；默认 `logs`
    #[serde(default = "default_log_dir")]
    pub dir: String,
    /// 是否同时输出到 stderr（release 便携默认关；`cargo run` debug 构建仍会打控制台）
    #[serde(default)]
    pub console: bool,
    /// 按修改时间清理旧日志文件的保留天数（0=不清理）
    #[serde(default = "default_log_keep_days")]
    pub keep_days: u64,
}

fn default_log_level() -> String {
    "info".into()
}
fn default_log_dir() -> String {
    "logs".into()
}
fn default_log_keep_days() -> u64 {
    14
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: default_log_level(),
            dir: default_log_dir(),
            console: false,
            keep_days: default_log_keep_days(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub theme: String,
    /// 界面语言：`zh-CN` | `zh-TW` | `ja` | `en`
    #[serde(default = "default_ui_language")]
    pub language: String,
    pub show_nied_clock: bool,
    pub sidebar_default_tab: String,
    /// 右侧情报列表侧栏（可关；关闭后用地图右键菜单）
    #[serde(default = "default_true")]
    pub show_sidebar: bool,
    /// 侧栏分离为独立窗口（对齐 eewcn QDock 浮动）
    #[serde(default)]
    pub sidebar_detached: bool,
    /// 分离侧栏窗口几何（拖动/缩放后写入用户配置）
    #[serde(default)]
    pub sidebar_window: SidebarWindowConfig,
    pub intensity_scale: String,
    /// 主窗口始终置顶（对齐滚动字幕「窗口置顶」）
    #[serde(default)]
    pub always_on_top: bool,
    /// 关闭/最小化到系统托盘（默认开）
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    /// 新预警/报数更新时地图自动跟焦震中
    #[serde(default = "default_true")]
    pub auto_follow_eew: bool,
    /// 跟焦时按 P/S 波圈自动缩放
    #[serde(default = "default_true")]
    pub auto_zoom_waves: bool,
    /// 绘制 P/S 走时波圈
    #[serde(default = "default_true")]
    pub show_wave_rings: bool,
    /// 地图左下角烈度图例
    #[serde(default = "default_true")]
    pub show_legend: bool,
    /// 地图绘制火山三角标，并在火山列表显示图标
    #[serde(default = "default_true")]
    pub show_volcanoes: bool,
    /// 地图右上气象悬浮卡片（不再地图打点）
    #[serde(default = "default_true")]
    pub show_weather: bool,
    /// 本地测站到时震源推算（不含检知；需足够触发站）
    #[serde(default = "default_true")]
    pub show_local_hypo: bool,
    /// 气象侧栏展示：`list` 列表 / `card` 地图悬浮卡片
    #[serde(default = "default_weather_display_mode")]
    pub weather_display_mode: String,
    /// 气象预警地名筛选（侧栏输入，空为不过滤）
    #[serde(default)]
    pub weather_location_filter: String,
    /// 已下线：烈度速报不再侧栏/地图展示；保留字段兼容旧 settings.json
    #[serde(default = "default_false")]
    pub show_cenc_int: bool,
    /// 地图绘制台风路径 / 风圈 / 预报
    #[serde(default = "default_true")]
    pub show_typhoons: bool,
    /// 地图绘制活动断层（全球 GEM + 中国 CAFD）
    #[serde(default = "default_false")]
    pub show_faults: bool,
    /// 全球晨昏线与夜侧遮罩
    #[serde(default = "default_false")]
    pub show_terminator: bool,
    /// 预报机构：全部 / 中国 / 中国台湾 / 日本 / 美国 / 中国香港
    #[serde(default = "default_typhoon_forecast_agency")]
    pub typhoon_forecast_agency: String,
}


/// 分离侧栏独立窗口几何。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidebarWindowConfig {
    #[serde(default = "default_sidebar_window_width")]
    pub width: f32,
    #[serde(default = "default_sidebar_window_height")]
    pub height: f32,
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
}

fn default_sidebar_window_width() -> f32 {
    280.0
}
fn default_sidebar_window_height() -> f32 {
    640.0
}

impl Default for SidebarWindowConfig {
    fn default() -> Self {
        Self {
            width: default_sidebar_window_width(),
            height: default_sidebar_window_height(),
            x: None,
            y: None,
        }
    }
}

/// 主窗口几何；缺省由系统摆放，用户拖动/缩放后写入用户配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    #[serde(default = "default_window_width")]
    pub width: f32,
    #[serde(default = "default_window_height")]
    pub height: f32,
    /// 外框左上角 X（物理像素近似）；缺省不强制
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
    #[serde(default)]
    pub maximized: bool,
}

fn default_window_width() -> f32 {
    1280.0
}
fn default_window_height() -> f32 {
    720.0
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: default_window_width(),
            height: default_window_height(),
            x: None,
            y: None,
            maximized: false,
        }
    }
}

/// 渲染设备与性能档位（改渲染设备需重启后生效）。GPU 固定 OpenGL。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// `cpu` = 软渲染；`gpu` = OpenGL 硬件加速
    #[serde(default = "default_perf_device")]
    pub device: String,
    /// `eco` 节能 | `normal` 正常 | `extreme` 极致
    #[serde(default = "default_perf_profile")]
    pub profile: String,
    /// 目标帧率：常见刷新率档（默认 60）
    #[serde(default = "default_target_fps")]
    pub target_fps: u32,
}

/// 目标帧率可选档位（对齐常见显示器刷新率）。
pub const TARGET_FPS_PRESETS: [u32; 11] = [
    30, 45, 60, 75, 90, 120, 144, 165, 180, 240, 300,
];

/// 将任意 fps 就近映射到档位之一（兼容旧配置）。
pub fn snap_target_fps(fps: u32) -> u32 {
    TARGET_FPS_PRESETS
        .iter()
        .copied()
        .min_by_key(|&p| (p as i64 - fps as i64).unsigned_abs())
        .unwrap_or(60)
}

/// 取不超过 `max_fps` 的最高档（用于按显示器刷新率封顶）。
pub fn snap_target_fps_at_most(max_fps: u32) -> u32 {
    TARGET_FPS_PRESETS
        .iter()
        .copied()
        .filter(|&p| p <= max_fps.max(1))
        .max()
        .unwrap_or_else(|| snap_target_fps(max_fps))
}

/// 综合机器预算与显示器刷新率，给出建议目标帧率。
pub fn recommend_target_fps(machine_budget: u32, display_hz: Option<u32>) -> u32 {
    let display_cap = display_hz
        .map(snap_target_fps_at_most)
        .unwrap_or(60);
    let machine = snap_target_fps(machine_budget);
    snap_target_fps_at_most(machine.min(display_cap))
}

fn default_perf_device() -> String {
    "gpu".into()
}
fn default_perf_profile() -> String {
    "normal".into()
}
fn default_target_fps() -> u32 {
    60
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            device: default_perf_device(),
            profile: default_perf_profile(),
            target_fps: default_target_fps(),
        }
    }
}

impl PerformanceConfig {
    /// 映射到可选档位。
    pub fn clamped_fps(&self) -> u32 {
        snap_target_fps(self.target_fps)
    }

    fn software_raster(&self) -> bool {
        self.device.eq_ignore_ascii_case("cpu")
    }

    /// 目标帧率对应的重绘间隔（毫秒）。软渲染最高约 30fps，避免像素光栅化占满 CPU。
    pub fn repaint_interval_ms(&self) -> u64 {
        let fps = self.clamped_fps().max(1) as f64;
        let ms = (1000.0 / fps).round().clamp(1.0, 250.0) as u64;
        if self.software_raster() {
            ms.max(33)
        } else {
            ms
        }
    }

    fn profile_norm(&self) -> String {
        self.profile.to_ascii_lowercase()
    }

    /// 空闲（无交互、无波圈）重绘间隔。正常/极致约 1 秒，节能约 1.5 秒，软渲染约 2 秒。
    /// 极致档只在交互/波圈时拉满目标帧率，空闲同样降频，避免 GPU 空转。
    pub fn idle_repaint_ms(&self) -> u64 {
        if self.software_raster() {
            return 2000;
        }
        match self.profile_norm().as_str() {
            "eco" => 1500,
            _ => 1000,
        }
    }

    /// 波圈 / 跟焦缩放等动画重绘间隔。节能与软渲染约 10fps，其余跟目标帧率。
    pub fn wave_repaint_ms(&self) -> u64 {
        if self.software_raster() || self.profile_norm() == "eco" {
            self.repaint_interval_ms().max(100)
        } else {
            self.repaint_interval_ms()
        }
    }

    /// 设置页打开、倒计时秒显等「软忙」间隔（无需跟手 60fps）。
    pub fn soft_repaint_ms(&self) -> u64 {
        if self.software_raster() {
            return 400;
        }
        match self.profile_norm().as_str() {
            "extreme" => 100,
            "eco" => 300,
            _ => 200,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapConfig {
    /// 底图：`vector`（默认 GeoJSON）| `tiles`（XYZ 瓦片）| `both`（瓦片+矢量边界）
    #[serde(default = "default_basemap")]
    pub basemap: String,
    /// 投影：`flat`（平面）| `globe`（球体）
    #[serde(default = "default_projection")]
    pub projection: String,
    /// 相对仓库根或绝对路径；`vector` / `both` 时加载
    #[serde(default = "default_geodata_dir")]
    pub geodata_dir: String,
    pub tile_base: String,
    pub tile_source: String,
    pub default_lon: f64,
    pub default_lat: f64,
    pub default_zoom: f64,
}

fn default_basemap() -> String {
    "auto".into()
}
fn default_projection() -> String {
    "flat".into()
}
fn default_geodata_dir() -> String {
    "geodata".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    pub event_pack: String,
    pub countdown_pack: String,
    pub master_volume: f32,
    pub countdown_volume: f32,
    pub mute: bool,
    /// 预警/速报 TTS 语音播报
    #[serde(default)]
    pub tts_enabled: bool,
    /// TTS 引擎：`offline`（系统离线语音包）/ `edge`（微软 Edge 在线）
    #[serde(default = "default_tts_engine")]
    pub tts_engine: String,
    /// 离线 TTS 音色（Windows SAPI 语音名；空=系统默认）
    #[serde(default)]
    pub tts_voice: String,
    /// Edge TTS 音色（如 zh-CN-XiaoxiaoNeural）
    #[serde(default = "default_tts_edge_voice")]
    pub tts_edge_voice: String,
    /// TTS 语速倍率（0.5–2.0，1.0 为默认）
    #[serde(default = "default_tts_rate")]
    pub tts_rate: f32,
    /// 本地 S 波倒计时音（地牛语音 + 滴答）
    #[serde(default = "default_true")]
    pub play_countdown: bool,
    /// 播报「十、九…」语音；关闭则只留滴答
    #[serde(default = "default_true")]
    pub countdown_speech: bool,
    /// 从剩余该秒开始播（对齐要石 countdownStart，默认 10）
    #[serde(default = "default_countdown_start")]
    pub countdown_start: i32,
    /// 仅本机预估达强有感（震度≥3 / 烈度≥5）时播
    #[serde(default)]
    pub countdown_only_intense: bool,
    /// NHK 一级新闻铃：情报震度达到 6 弱及以上
    #[serde(default = "default_true")]
    pub nhk_news_bell: bool,
    /// JMA 紧急地震速报「警报」（预报升级为警报）专用音
    #[serde(default = "default_true")]
    pub jma_eew_alert_sound: bool,
    /// 津波警报 / 大津波警报：紧急警报放送第 2 种开始信号
    #[serde(default = "default_true")]
    pub tsunami_ebs_sound: bool,
}

/// 桌面系统通知（Win10/11 Toast）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub on_eew: bool,
    #[serde(default = "default_true")]
    pub on_record: bool,
    #[serde(default = "default_true")]
    pub on_tsunami: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            on_eew: true,
            on_record: true,
            on_tsunami: true,
        }
    }
}

fn default_countdown_start() -> i32 {
    10
}

fn default_tts_engine() -> String {
    "offline".into()
}

fn default_tts_edge_voice() -> String {
    "zh-CN-XiaoxiaoNeural".into()
}

fn default_tts_rate() -> f32 {
    1.0
}

/// 自定义震级区间 → 最大 S 波传播距离。
/// 兼容旧键 `magnitude`（视为最小震级，最大默认为 9.5）。
#[derive(Debug, Clone, Serialize)]
pub struct MagSpreadPoint {
    pub min_magnitude: f64,
    pub max_magnitude: f64,
    pub max_km: f64,
}

impl<'de> serde::Deserialize<'de> for MagSpreadPoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            min_magnitude: Option<f64>,
            max_magnitude: Option<f64>,
            magnitude: Option<f64>,
            max_km: f64,
        }
        let r = Raw::deserialize(deserializer)?;
        let min_m = r
            .min_magnitude
            .or(r.magnitude)
            .ok_or_else(|| serde::de::Error::missing_field("min_magnitude"))?;
        let max_m = r.max_magnitude.unwrap_or(9.5);
        Ok(Self {
            min_magnitude: min_m,
            max_magnitude: max_m,
            max_km: r.max_km,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TravelConfig {
    pub primary: String,
    pub fallback: String,
    pub linear_last_resort: bool,
    /// 震级区间→最大 S 波传播距离（km）；空则用内置表
    #[serde(default)]
    pub mag_spread_km: Vec<MagSpreadPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcesConfig {
    pub jian: JianSourceConfig,
    pub wolfx: WolfxSourceConfig,
    pub p2pquake: P2pSourceConfig,
    /// FSSN 地震信息（Fan Studio Seismic Network）
    #[serde(default)]
    pub fssn: FssnSourceConfig,
    /// PancakesAPI（api.aloys23.link）全灾害源
    #[serde(default)]
    pub pancakes: PancakesSourceConfig,
    /// 测站网：JMA（kmoni/lmoni/中转）、S-Net、KMA
    #[serde(default)]
    pub stations: StationsConfig,
    /// 活跃台风路径
    #[serde(default)]
    pub typhoon: TyphoonSourceConfig,
    /// 中央台气象预警（Jian `/weather`）
    #[serde(default)]
    pub weather: WeatherSourceConfig,
    /// CENC 烈度速报（NowQuake）
    #[serde(default)]
    pub cenc_int: CencIntSourceConfig,
    /// 地图雷达 / 降水叠加（GET）
    #[serde(default)]
    pub map_wx: MapWxSourceConfig,
}


/// 地图雷达与降水叠加（HTTP 轮询 sismotide GET）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapWxSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 中国雷达（可与日/台同时开）
    #[serde(default)]
    pub radar_china: bool,
    /// 日本雷达
    #[serde(default)]
    pub radar_japan: bool,
    /// 中国台湾雷达
    #[serde(default)]
    pub radar_taiwan: bool,
    /// 旧版三选一字段；仅反序列化迁移，不写出
    #[serde(default, skip_serializing)]
    pub radar_layer: String,
    /// 0=关；24 / 48 / 72
    #[serde(default)]
    pub precip_hours: u16,
    /// 雷达贴图透明度（0=全透明，1=不透明）
    #[serde(default = "default_map_wx_radar_opacity")]
    pub radar_opacity: f32,
    /// 降水落区透明度（乘在上游色块 alpha 上）
    #[serde(default = "default_map_wx_precip_opacity")]
    pub precip_opacity: f32,
    #[serde(default = "default_map_wx_poll")]
    pub poll_secs: u64,
    #[serde(default = "default_radar_china_url")]
    pub radar_china_url: String,
    #[serde(default = "default_radar_japan_url")]
    pub radar_japan_url: String,
    #[serde(default = "default_radar_taiwan_url")]
    pub radar_taiwan_url: String,
    #[serde(default = "default_precip_url")]
    pub precip_url: String,
}

impl Default for MapWxSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            radar_china: false,
            radar_japan: false,
            radar_taiwan: false,
            radar_layer: String::new(),
            precip_hours: 0,
            radar_opacity: default_map_wx_radar_opacity(),
            precip_opacity: default_map_wx_precip_opacity(),
            poll_secs: default_map_wx_poll(),
            radar_china_url: default_radar_china_url(),
            radar_japan_url: default_radar_japan_url(),
            radar_taiwan_url: default_radar_taiwan_url(),
            precip_url: default_precip_url(),
        }
    }
}


impl MapWxSourceConfig {
    /// 将旧 `radar_layer` 三选一迁移为三路独立开关（仅当三路均为关时）。
    pub fn migrate_radar_layer(&mut self) {
        if self.radar_china || self.radar_japan || self.radar_taiwan {
            self.radar_layer.clear();
            return;
        }
        match self.radar_layer.trim().to_ascii_lowercase().as_str() {
            "china" | "cn" | "radar" => self.radar_china = true,
            "japan" | "jp" | "radar-japan" => self.radar_japan = true,
            "taiwan" | "tw" | "radar-taiwan" => self.radar_taiwan = true,
            _ => {}
        }
        self.radar_layer.clear();
    }

    pub fn any_radar(&self) -> bool {
        self.radar_china || self.radar_japan || self.radar_taiwan
    }

    /// 钳制透明度到 0..=1（非法/NaN 回落默认）。
    pub fn clamp_opacities(&mut self) {
        self.radar_opacity = sanitize_opacity(self.radar_opacity, default_map_wx_radar_opacity());
        self.precip_opacity = sanitize_opacity(self.precip_opacity, default_map_wx_precip_opacity());
    }
}

/// 台风实时路径（HTTP 轮询）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TyphoonSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_typhoon_url")]
    pub url: String,
    #[serde(default = "default_typhoon_poll")]
    pub poll_secs: u64,
}

impl Default for TyphoonSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            url: default_typhoon_url(),
            poll_secs: default_typhoon_poll(),
        }
    }
}

/// 气象预警 WebSocket（鉴权复用 `sources.jian` 的 at_/rt_）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_weather_ws")]
    pub ws_url: String,
}

impl Default for WeatherSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ws_url: default_weather_ws(),
        }
    }
}

/// NowQuake CENC 烈度速报（冷启动 GET + WebSocket）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CencIntSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_cenc_int_http")]
    pub http_base: String,
    #[serde(default = "default_cenc_int_ws")]
    pub ws_url: String,
    #[serde(default = "default_cenc_int_page_size")]
    pub list_page_size: u32,
}

impl Default for CencIntSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            http_base: default_cenc_int_http(),
            ws_url: default_cenc_int_ws(),
            list_page_size: default_cenc_int_page_size(),
        }
    }
}


/// PancakesAPI：仅 GlobalQuake（gq）与 JMA-Atom（上游 source=`jma_volcano`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PancakesSourceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_pancakes_base")]
    pub base_url: String,
    #[serde(default = "default_pancakes_ws")]
    pub ws_url: String,
    #[serde(default = "default_pancakes_client")]
    pub client_id: String,
    /// GlobalQuake 最低震级（0=不过滤）
    #[serde(default = "default_gq_min_mag")]
    pub gq_min_magnitude: f64,
    #[serde(default = "default_true")]
    pub gq: bool,
    /// JMA-Atom（火山情报；上游 `jma_volcano`）
    #[serde(default = "default_true", alias = "jma_atom")]
    pub jma_volcano: bool,
}

impl Default for PancakesSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: default_pancakes_base(),
            ws_url: default_pancakes_ws(),
            client_id: default_pancakes_client(),
            gq_min_magnitude: default_gq_min_mag(),
            gq: true,
            jma_volcano: true,
        }
    }
}

/// 实时测站源选择（用户可切换原版 / Jian 中转）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationsConfig {
    /// 日本 JMA/NIED 测站总开关
    #[serde(default = "default_true")]
    pub jma_enabled: bool,
    /// `jian` | `kmoni` | `lmoni`
    #[serde(default = "default_jma_station_source")]
    pub jma_source: String,
    #[serde(default = "default_jian_station_ws")]
    pub jma_jian_ws: String,
    /// S-Net 海上强震网
    #[serde(default)]
    pub snet_enabled: bool,
    /// `jian` | `original`
    #[serde(default = "default_snet_station_source")]
    pub snet_source: String,
    #[serde(default = "default_snet_jian_ws")]
    pub snet_jian_ws: String,
    /// 韩国 KMA PEWS（仅 Jian API）
    #[serde(default)]
    pub kma_enabled: bool,
    #[serde(default = "default_kma_station_ws")]
    pub kma_jian_ws: String,
}

impl Default for StationsConfig {
    fn default() -> Self {
        Self {
            jma_enabled: true,
            jma_source: default_jma_station_source(),
            jma_jian_ws: default_jian_station_ws(),
            snet_enabled: false,
            snet_source: default_snet_station_source(),
            snet_jian_ws: default_snet_jian_ws(),
            kma_enabled: true,
            kma_jian_ws: default_kma_station_ws(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JianSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `/all` 预警总开关
    #[serde(default = "default_true")]
    pub eew_enabled: bool,
    /// `/all` 速报总开关
    #[serde(default = "default_true")]
    pub record_enabled: bool,
    #[serde(default = "default_true")]
    pub jma_eew: bool,
    #[serde(default = "default_true")]
    pub cea_eew: bool,
    #[serde(default = "default_true")]
    pub cwa_eew: bool,
    #[serde(default = "default_true")]
    pub kma_eew: bool,
    #[serde(default = "default_true")]
    pub sa_eew: bool,
    #[serde(default = "default_true")]
    pub early_est_eew: bool,
    /// 建议用 `/all` 单连接，避免占满并发额度
    #[serde(default = "default_jian_ws")]
    pub ws_url: String,
    /// 短期访问令牌 `at_…`（运行时 / 环境变量；不写入用户覆盖）
    #[serde(default)]
    pub access_token: Option<String>,
    /// 长期 Token `rt_…`（设置页用 lk_ 换取后写入用户目录，约 180 天）
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// 长期 Token 过期 unix 秒；0 表示未知
    #[serde(default)]
    pub refresh_expire_at: f64,
    /// 每把长期 Token 最大并发连接数（服务器返回；未知为 0）
    #[serde(default)]
    pub max_connections: u32,
    /// 兼容旧键：与 `stations.jma_enabled` 对齐；实际由 stations 模块拉 `/kmoni`（测站中转无需令牌）
    #[serde(default = "default_true")]
    pub station_enabled: bool,
    #[serde(default = "default_jian_station_ws")]
    pub station_ws_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WolfxSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Wolfx `all_eew` 预警总开关
    #[serde(default = "default_true")]
    pub eew_enabled: bool,
    /// Wolfx 速报列表总开关（WS query + HTTP）
    #[serde(default = "default_true")]
    pub eqlist_enabled: bool,
    #[serde(default = "default_true")]
    pub jma_eew: bool,
    /// 四川 EEW（cea-pr）；默认关，避免与 Jian 省级源重复
    #[serde(default = "default_false")]
    pub sc_eew: bool,
    #[serde(default = "default_false")]
    pub fj_eew: bool,
    #[serde(default = "default_false")]
    pub cq_eew: bool,
    /// Wolfx CENC 国网预警（入库为 wolfx-cea，展示 [CN]）
    #[serde(default = "default_true")]
    pub cenc_eew: bool,
    #[serde(default = "default_false")]
    pub cwa_eew: bool,
    #[serde(default = "default_true")]
    pub cenc_eqlist: bool,
    #[serde(default = "default_true")]
    pub jma_eqlist: bool,
    #[serde(default = "default_wolfx_ws")]
    pub ws_urls: Vec<String>,
    /// 冷启动速报列表（HTTP JSON）
    #[serde(default = "default_wolfx_eqlist")]
    pub http_eqlist: String,
}

impl WolfxSourceConfig {
    pub fn type_enabled(&self, typ: &str) -> bool {
        // 仅看各子源开关；总开关字段保留兼容旧配置，不再参与门控
        match typ {
            "jma_eew" => self.jma_eew,
            "sc_eew" => self.sc_eew,
            "fj_eew" => self.fj_eew,
            "cq_eew" => self.cq_eew,
            "cenc_eew" => self.cenc_eew,
            "cwa_eew" => self.cwa_eew,
            "cenc_eqlist" => self.cenc_eqlist,
            "jma_eqlist" => self.jma_eqlist,
            _ => true,
        }
    }

    pub fn any_eew(&self) -> bool {
        self.jma_eew || self.sc_eew || self.fj_eew || self.cq_eew || self.cenc_eew || self.cwa_eew
    }

    pub fn any_eqlist(&self) -> bool {
        self.cenc_eqlist || self.jma_eqlist
    }
}

/// 速报过滤：源开关与震级阈值分离。
///
/// - `record_enabled`：各机构速报开关（关则整源丢弃）
/// - `record_mag`：`0` 不过滤；`>0` 最低震级。白名单可绕过震级，不能绕过关闭的源。
/// - 兼容旧配置：`record_mag = -1` 视为该源关闭，并按 `0` 读阈值。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FiltersConfig {
    /// 各机构速报开关；缺省：`other` 关，其余开
    #[serde(default)]
    pub record_enabled: HashMap<String, bool>,
    #[serde(default)]
    pub record_mag: HashMap<String, f64>,
    #[serde(default)]
    pub place_whitelist: String,
}

impl FiltersConfig {
    /// 该机构速报是否接收（与震级阈值无关）。
    pub fn record_source_enabled(&self, key: &str) -> bool {
        if let Some(on) = self.record_enabled.get(key) {
            return *on;
        }
        // 旧配置用 mag=-1 表示关闭
        if self.record_mag.get(key).copied().unwrap_or(0.0) < 0.0 {
            return false;
        }
        key != "other"
    }

    pub fn set_record_source_enabled(&mut self, key: &str, on: bool) {
        self.record_enabled.insert(key.to_string(), on);
    }

    /// `0` 不过滤；正数为最低震级。负值（旧「不接收」）按 `0` 处理。
    pub fn record_mag_threshold(&self, key: &str) -> f64 {
        let v = self.record_mag.get(key).copied().unwrap_or(0.0);
        if v < 0.0 {
            0.0
        } else {
            v
        }
    }

    pub fn set_record_mag(&mut self, key: &str, val: f64) {
        self.record_mag.insert(key.to_string(), val.max(0.0));
    }

    /// 将旧 `record_mag=-1` 迁移为源开关关闭，避免与震级过滤混用。
    pub fn migrate_legacy_record_off(&mut self) {
        let keys: Vec<String> = self.record_mag.keys().cloned().collect();
        for key in keys {
            if self.record_mag.get(&key).copied().unwrap_or(0.0) < 0.0 {
                self.record_enabled.entry(key.clone()).or_insert(false);
                self.record_mag.insert(key, 0.0);
            }
        }
        self.record_enabled.entry("other".into()).or_insert(false);
    }

    pub fn place_whitelist_hit(&self, place: &str) -> bool {
        let raw = self.place_whitelist.trim();
        if raw.is_empty() {
            return false;
        }
        raw.split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .any(|kw| place.contains(kw))
    }
}

/// FSSN 地震信息 WebSocket（`/fssn`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FssnSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_fssn_ws")]
    pub ws_url: String,
}

impl Default for FssnSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ws_url: default_fssn_ws(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_p2p_http")]
    pub http_history: String,
    #[serde(default = "default_p2p_ws")]
    pub ws_url: String,
    /// HTTP 轮询间隔（秒）；默认 1.5，最低 1.0。
    #[serde(default = "default_p2p_poll")]
    pub poll_secs: f64,
}

fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}
fn default_jian_ws() -> String {
    "wss://api.sismotide.top/all".into()
}
fn default_jian_station_ws() -> String {
    "wss://api.sismotide.top/kmoni".into()
}
fn default_jma_station_source() -> String {
    "jian".into()
}
fn default_snet_station_source() -> String {
    "jian".into()
}
fn default_snet_jian_ws() -> String {
    "wss://api.sismotide.top/s-net".into()
}
fn default_kma_station_ws() -> String {
    "wss://api.sismotide.top/kma-station".into()
}
fn default_wolfx_ws() -> Vec<String> {
    vec!["wss://ws-api.wolfx.jp/all_eew".into()]
}
fn default_wolfx_eqlist() -> String {
    "https://api.wolfx.jp/cenc_eqlist.json".into()
}
fn default_p2p_http() -> String {
    "https://api.p2pquake.net/v2/history".into()
}
fn default_p2p_ws() -> String {
    "wss://api.p2pquake.net/v2/ws".into()
}
fn default_p2p_poll() -> f64 {
    1.5
}
fn default_fssn_ws() -> String {
    "wss://fssn-api.fanstudio.tech/fssn".into()
}
fn default_pancakes_base() -> String {
    "https://api.aloys23.link".into()
}
fn default_pancakes_ws() -> String {
    "wss://api.aloys23.link".into()
}
fn default_pancakes_client() -> String {
    "starquake".into()
}
fn default_gq_min_mag() -> f64 {
    3.0
}
fn default_typhoon_url() -> String {
    "https://api.sismotide.top/get/typhoon.json".into()
}
fn default_map_wx_radar_opacity() -> f32 {
    0.92
}

fn default_map_wx_precip_opacity() -> f32 {
    0.75
}

fn sanitize_opacity(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        fallback
    }
}

fn default_map_wx_poll() -> u64 {
    60
}
fn default_radar_china_url() -> String {
    "https://api.sismotide.top/get/radar.json".into()
}
fn default_radar_japan_url() -> String {
    "https://api.sismotide.top/get/radar-japan.json".into()
}
fn default_radar_taiwan_url() -> String {
    "https://api.sismotide.top/get/radar-taiwan.json".into()
}
fn default_precip_url() -> String {
    "https://api.sismotide.top/get/precip-forecast.json".into()
}
fn default_weather_ws() -> String {
    "wss://api.sismotide.top/weather".into()
}

fn default_weather_display_mode() -> String {
    "card".into()
}
fn default_cenc_int_http() -> String {
    "https://api-cencint-public.nowquake.cn".into()
}
fn default_cenc_int_ws() -> String {
    "wss://api-cencint-public.nowquake.cn/websocket".into()
}
fn default_cenc_int_page_size() -> u32 {
    20
}
fn default_typhoon_poll() -> u64 {
    15
}
fn default_typhoon_forecast_agency() -> String {
    "中国".into()
}
fn default_ui_language() -> String {
    "zh-CN".into()
}

/// 归一化界面语言配置值。
pub fn normalize_ui_language(s: &str) -> String {
    match s.trim() {
        "zh-TW" | "zh_TW" | "zh-tw" | "zh_Hant" | "zh-Hant" => "zh-TW".into(),
        "ja" | "ja-JP" | "jp" | "japanese" => "ja".into(),
        "en" | "en-US" | "en-GB" | "english" => "en".into(),
        "zh-CN" | "zh_CN" | "zh-cn" | "zh" | "zh_Hans" | "zh-Hans" => "zh-CN".into(),
        _ => "zh-CN".into(),
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ui: UiConfig {
                theme: "dark".into(),
                language: default_ui_language(),
                show_nied_clock: false,
                sidebar_default_tab: "eew".into(),
                show_sidebar: true,
                sidebar_detached: false,
                sidebar_window: SidebarWindowConfig::default(),
                intensity_scale: "auto".into(),
                always_on_top: false,
                minimize_to_tray: true,
                auto_follow_eew: true,
                auto_zoom_waves: true,
                show_wave_rings: true,
                show_legend: true,
                show_volcanoes: true,
                show_weather: true,
                show_local_hypo: true,
                weather_display_mode: default_weather_display_mode(),
                weather_location_filter: String::new(),
                show_cenc_int: false,
                show_typhoons: true,
                show_faults: false,
                show_terminator: false,
                typhoon_forecast_agency: default_typhoon_forecast_agency(),
            },
            performance: PerformanceConfig::default(),
            window: WindowConfig::default(),
            map: MapConfig {
                basemap: default_basemap(),
                projection: default_projection(),
                geodata_dir: default_geodata_dir(),
                tile_base: "https://tilemap.sismotide.top".into(),
                tile_source: "arcwob".into(),
                default_lon: 120.0,
                default_lat: 30.0,
                default_zoom: 6.8,
            },
            audio: AudioConfig {
                event_pack: "srev".into(),
                countdown_pack: "countdown".into(),
                master_volume: 0.8,
                countdown_volume: 1.0,
                mute: false,
                tts_enabled: false,
                tts_engine: default_tts_engine(),
                tts_voice: String::new(),
                tts_edge_voice: default_tts_edge_voice(),
                tts_rate: default_tts_rate(),
                play_countdown: true,
                countdown_speech: true,
                countdown_start: default_countdown_start(),
                countdown_only_intense: false,
                nhk_news_bell: true,
                jma_eew_alert_sound: true,
                tsunami_ebs_sound: true,
            },
            notify: NotifyConfig::default(),
            travel: TravelConfig {
                primary: "jma2001".into(),
                fallback: "ak135".into(),
                linear_last_resort: true,
                mag_spread_km: Vec::new(),
            },
            local: LocalConfig {
                latitude: None,
                longitude: None,
            },
            sources: SourcesConfig {
                jian: JianSourceConfig {
                    enabled: true,
                    eew_enabled: true,
                    record_enabled: true,
                    jma_eew: true,
                    cea_eew: true,
                    cwa_eew: true,
                    kma_eew: true,
                    sa_eew: true,
                    early_est_eew: true,
                    ws_url: default_jian_ws(),
                    access_token: None,
                    refresh_token: None,
                    refresh_expire_at: 0.0,
                    max_connections: 0,
                    station_enabled: true,
                    station_ws_url: default_jian_station_ws(),
                },
                wolfx: WolfxSourceConfig {
                    enabled: true,
                    eew_enabled: true,
                    eqlist_enabled: true,
                    jma_eew: true,
                    sc_eew: false,
                    fj_eew: false,
                    cq_eew: false,
                    cenc_eew: true,
                    cwa_eew: false,
                    cenc_eqlist: true,
                    jma_eqlist: true,
                    ws_urls: default_wolfx_ws(),
                    http_eqlist: default_wolfx_eqlist(),
                },
                p2pquake: P2pSourceConfig {
                    enabled: true,
                    http_history: default_p2p_http(),
                    ws_url: default_p2p_ws(),
                    poll_secs: default_p2p_poll(),
                },
                fssn: FssnSourceConfig::default(),
                pancakes: PancakesSourceConfig::default(),
                stations: StationsConfig::default(),
                typhoon: TyphoonSourceConfig::default(),
                weather: WeatherSourceConfig::default(),
                cenc_int: CencIntSourceConfig::default(),
                map_wx: MapWxSourceConfig::default(),
            },
            filters: FiltersConfig::default(),
            log: LogConfig::default(),
            update: UpdateConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load_from_toml(path: impl AsRef<Path>) -> Result<Self> {
        let text = fs::read_to_string(path.as_ref())
            .with_context(|| format!("read config {}", path.as_ref().display()))?;
        let mut cfg: Self = toml::from_str(&text)?;
        cfg.apply_env_overrides();
        Ok(cfg)
    }

    pub fn load_default_or_builtin(repo_config: impl AsRef<Path>) -> Self {
        match Self::load_from_toml(repo_config.as_ref()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "使用内置默认配置（未能读 {}: {e:#})",
                    repo_config.as_ref().display()
                );
                let mut c = Self::default();
                c.apply_env_overrides();
                c
            }
        }
    }

    /// 仓库默认 + 用户目录覆盖 + 环境变量（令牌仍以环境变量优先）。
    /// Windows 用户覆盖：`%APPDATA%\StarQuake\settings.json`。
    pub fn load_merged(repo_config: impl AsRef<Path>) -> Self {
        let mut cfg = Self::load_default_or_builtin(repo_config);
        Self::migrate_legacy_app_dir();
        if let Some(path) = Self::user_config_path() {
            if path.is_file() {
                match Self::apply_user_overlay_file(&mut cfg, &path) {
                    Ok(()) => tracing_log(&format!("已合并用户配置 {}", path.display())),
                    Err(e) => eprintln!("用户配置无效，已忽略 {}: {e:#}", path.display()),
                }
            } else {
                // 一次性迁移更早路径（sismotide / config.toml）
                for legacy in Self::legacy_user_config_paths() {
                    if legacy.is_file() {
                        match Self::apply_user_overlay_file(&mut cfg, &legacy) {
                            Ok(()) => {
                                tracing_log(&format!(
                                    "已从旧配置迁移 {} \u{2192} settings.json",
                                    legacy.display()
                                ));
                                let _ = cfg.save_user_overlay();
                                break;
                            }
                            Err(e) => eprintln!("旧配置无法读取 {}: {e:#}", legacy.display()),
                        }
                    }
                }
            }
        }
        cfg.apply_env_overrides();
        cfg.filters.migrate_legacy_record_off();
        cfg.sources.map_wx.migrate_radar_layer();
        cfg.sources.map_wx.clamp_opacities();
        cfg.migrate_removed_station_sources();
        cfg.migrate_removed_sidebar_tab();
        cfg.migrate_legacy_client_id();
        cfg.restore_refresh_token_if_missing();
        cfg
    }

    /// 旧默认 Pancakes clientId `eewview` → `starquake`。
    fn migrate_legacy_client_id(&mut self) {
        if self.sources.pancakes.client_id.trim().eq_ignore_ascii_case("eewview") {
            self.sources.pancakes.client_id = default_pancakes_client();
        }
    }

    fn apply_user_overlay_file(cfg: &mut Self, path: &Path) -> Result<()> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.ends_with(".json") {
            let overlay: serde_json::Value =
                serde_json::from_str(&text).context("parse settings.json")?;
            let base = serde_json::to_value(&*cfg).context("serialize base config")?;
            let merged = merge_json(base, overlay);
            *cfg = serde_json::from_value(merged).context("settings.json → AppConfig")?;
        } else {
            let overlay: toml::Value = text.parse().context("parse TOML overlay")?;
            let base = toml::Value::try_from(&*cfg).context("serialize base config")?;
            let merged = merge_toml(base, overlay);
            *cfg = merged.try_into().context("TOML overlay → AppConfig")?;
        }
        Ok(())
    }

    pub fn user_config_path() -> Option<PathBuf> {
        Self::user_config_dir().map(|d| d.join("settings.json"))
    }

    /// 写入用户目录覆盖为 `settings.json`（不含短期 `at_`；保留长期 `rt_`）。
    pub fn save_user_overlay(&self) -> Result<PathBuf> {
        let dir = Self::user_config_dir().context("无法解析用户配置目录")?;
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join("settings.json");
        let mut sanitized = self.clone();
        sanitized.filters.migrate_legacy_record_off();
        sanitized.sources.map_wx.migrate_radar_layer();
        sanitized.sources.map_wx.clamp_opacities();
        sanitized.migrate_removed_station_sources();
        sanitized.migrate_removed_sidebar_tab();
        sanitized.migrate_legacy_client_id();
        sanitized.sources.jian.access_token = None;
        // 禁止用 None 覆盖磁盘上已有的 rt_
        if sanitized.sources.jian.refresh_token.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true)
        {
            if let Some(existing) = Self::read_disk_refresh_token(&path) {
                sanitized.sources.jian.refresh_token = Some(existing.token);
                if sanitized.sources.jian.refresh_expire_at <= 0.0 && existing.expire_at > 0.0 {
                    sanitized.sources.jian.refresh_expire_at = existing.expire_at;
                }
                if sanitized.sources.jian.max_connections == 0 && existing.max_connections > 0 {
                    sanitized.sources.jian.max_connections = existing.max_connections;
                }
            }
        }
        let text = serde_json::to_string_pretty(&sanitized).context("serialize settings.json")?;
        fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
        // 并行保险文件，升级丢 settings 时仍可回填
        Self::write_jian_token_sidecar(
            &dir,
            sanitized.sources.jian.refresh_token.as_deref(),
            sanitized.sources.jian.refresh_expire_at,
            sanitized.sources.jian.max_connections,
        );
        Ok(path)
    }

    fn read_disk_refresh_token(settings_path: &Path) -> Option<JianTokenSidecar> {
        if settings_path.is_file() {
            if let Ok(text) = fs::read_to_string(settings_path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(tok) = v
                        .pointer("/sources/jian/refresh_token")
                        .and_then(|x| x.as_str())
                        .map(str::trim)
                        .filter(|s| s.starts_with("rt_"))
                    {
                        let expire_at = v
                            .pointer("/sources/jian/refresh_expire_at")
                            .and_then(|x| x.as_f64())
                            .unwrap_or(0.0);
                        let max_connections = v
                            .pointer("/sources/jian/max_connections")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0) as u32;
                        return Some(JianTokenSidecar {
                            token: tok.to_string(),
                            expire_at,
                            max_connections,
                        });
                    }
                }
            }
        }
        let sidecar = settings_path
            .parent()
            .map(|d| d.join("jian_token.json"))?;
        Self::read_jian_token_sidecar(&sidecar)
    }

    fn write_jian_token_sidecar(
        dir: &Path,
        token: Option<&str>,
        expire_at: f64,
        max_connections: u32,
    ) {
        let Some(tok) = token.map(str::trim).filter(|s| s.starts_with("rt_")) else {
            return;
        };
        let path = dir.join("jian_token.json");
        let body = JianTokenSidecar {
            token: tok.to_string(),
            expire_at,
            max_connections,
        };
        if let Ok(text) = serde_json::to_string_pretty(&body) {
            let _ = fs::write(&path, text);
        }
    }

    fn read_jian_token_sidecar(path: &Path) -> Option<JianTokenSidecar> {
        let text = fs::read_to_string(path).ok()?;
        let body: JianTokenSidecar = serde_json::from_str(&text).ok()?;
        if body.token.trim().starts_with("rt_") {
            Some(body)
        } else {
            None
        }
    }

    /// 启动时：若内存无 rt_，从 sidecar / 旧目录补入。
    fn restore_refresh_token_if_missing(&mut self) {
        let has = self
            .sources
            .jian
            .refresh_token
            .as_ref()
            .map(|s| s.trim().starts_with("rt_"))
            .unwrap_or(false);
        if has {
            // 仍写 sidecar，保证保险文件存在
            if let Some(dir) = Self::user_app_dir() {
                Self::write_jian_token_sidecar(
                    &dir,
                    self.sources.jian.refresh_token.as_deref(),
                    self.sources.jian.refresh_expire_at,
                    self.sources.jian.max_connections,
                );
            }
            return;
        }
        // 1) 本目录 sidecar
        if let Some(dir) = Self::user_app_dir() {
            if let Some(sc) = Self::read_jian_token_sidecar(&dir.join("jian_token.json")) {
                self.sources.jian.refresh_token = Some(sc.token);
                if self.sources.jian.refresh_expire_at <= 0.0 {
                    self.sources.jian.refresh_expire_at = sc.expire_at;
                }
                if self.sources.jian.max_connections == 0 {
                    self.sources.jian.max_connections = sc.max_connections;
                }
                tracing_log("已从 jian_token.json 回填长期 Token");
                return;
            }
        }
        // 2) 旧目录 settings / sidecar
        for src in Self::legacy_user_app_dirs() {
            if let Some(sc) = Self::read_disk_refresh_token(&src.join("settings.json")) {
                self.sources.jian.refresh_token = Some(sc.token.clone());
                if self.sources.jian.refresh_expire_at <= 0.0 {
                    self.sources.jian.refresh_expire_at = sc.expire_at;
                }
                if self.sources.jian.max_connections == 0 {
                    self.sources.jian.max_connections = sc.max_connections;
                }
                tracing_log(&format!(
                    "已从旧目录补入长期 Token {}",
                    src.display()
                ));
                if let Some(dir) = Self::user_app_dir() {
                    Self::write_jian_token_sidecar(
                        &dir,
                        Some(&sc.token),
                        sc.expire_at,
                        sc.max_connections,
                    );
                }
                return;
            }
            if let Some(sc) = Self::read_jian_token_sidecar(&src.join("jian_token.json")) {
                self.sources.jian.refresh_token = Some(sc.token.clone());
                if self.sources.jian.refresh_expire_at <= 0.0 {
                    self.sources.jian.refresh_expire_at = sc.expire_at;
                }
                if self.sources.jian.max_connections == 0 {
                    self.sources.jian.max_connections = sc.max_connections;
                }
                tracing_log(&format!(
                    "已从旧目录 jian_token.json 补入长期 Token {}",
                    src.display()
                ));
                if let Some(dir) = Self::user_app_dir() {
                    Self::write_jian_token_sidecar(
                        &dir,
                        Some(&sc.token),
                        sc.expire_at,
                        sc.max_connections,
                    );
                }
                return;
            }
        }
    }

    /// 已下线的测站源回落（如旧 `yahoo` → `jian`）。
    pub fn migrate_removed_station_sources(&mut self) {
        if self.sources.stations.jma_source.eq_ignore_ascii_case("yahoo") {
            self.sources.stations.jma_source = "jian".into();
        }
    }

    /// 已下线的侧栏页签回落（旧 `station` / `cenc_int` → `eew`）。
    pub fn migrate_removed_sidebar_tab(&mut self) {
        let t = self.ui.sidebar_default_tab.to_ascii_lowercase();
        if matches!(
            t.as_str(),
            "station" | "cenc_int" | "cenc-int" | "intensity"
        ) {
            self.ui.sidebar_default_tab = "eew".into();
        }
    }

    /// `stations.jma_*` 为准；旧键 `jian.station_enabled` 仅镜像，不得反杀测站开关。
    pub fn sync_station_compat(&mut self) {
        self.migrate_removed_station_sources();
        if self.sources.stations.jma_enabled && self.sources.stations.jma_source == "jian" {
            self.sources.jian.station_enabled = true;
            self.sources.jian.station_ws_url = self.sources.stations.jma_jian_ws.clone();
        } else if self.sources.stations.jma_source == "jian" {
            self.sources.jian.station_enabled = false;
        } else {
            // 原版 kmoni/lmoni：关闭 Jian /kmoni 连接
            self.sources.jian.station_enabled = false;
        }
    }

    /// 环境变量覆盖敏感项（不写进仓库）
    pub fn apply_env_overrides(&mut self) {
        self.sync_station_compat();
        self.filters.migrate_legacy_record_off();
        self.sources.map_wx.migrate_radar_layer();
        self.sources.map_wx.clamp_opacities();
        if let Ok(tok) = env::var("JIAN_REFRESH_TOKEN") {
            if tok.trim().starts_with("rt_") {
                self.sources.jian.refresh_token = Some(tok.trim().to_string());
            }
        }
        if let Ok(tok) = env::var("JIAN_ACCESS_TOKEN") {
            if !tok.trim().is_empty() {
                self.sources.jian.access_token = Some(tok.trim().to_string());
            }
        }
    }

    /// 用户数据根目录。
    /// Windows：`%APPDATA%\StarQuake`（配置 `settings.json`、日志 `log.txt`）。
    pub fn user_config_dir() -> Option<PathBuf> {
        Self::user_app_dir()
    }

    /// 与 `user_config_dir` 相同（本机可写用户数据）。
    pub fn user_data_local_dir() -> Option<PathBuf> {
        Self::user_app_dir()
    }

    /// 平台用户目录：`StarQuake`。
    pub fn user_app_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            env::var_os("APPDATA").map(|p| PathBuf::from(p).join("StarQuake"))
        }
        #[cfg(not(windows))]
        {
            directories::BaseDirs::new().map(|b| b.config_dir().join("StarQuake"))
        }
    }

    /// 旧版用户数据目录名（仅迁移读取）。
    fn legacy_user_app_dirs() -> Vec<PathBuf> {
        let mut out = Vec::new();
        #[cfg(windows)]
        {
            if let Some(appdata) = env::var_os("APPDATA") {
                let base = PathBuf::from(appdata);
                out.push(base.join("eew view"));
                out.push(base.join("EEW View"));
                out.push(base.join("EEWView"));
            }
        }
        #[cfg(not(windows))]
        {
            if let Some(b) = directories::BaseDirs::new() {
                let cfg = b.config_dir();
                out.push(cfg.join("eew view"));
                out.push(cfg.join("EEW View"));
                out.push(cfg.join("eewview"));
                out.push(cfg.join("eew-view"));
            }
        }
        out
    }

    /// 若新目录尚无 `settings.json`，从旧 `eew view` 等目录复制配置与日志。
    /// 若新目录已有 settings 但无 `rt_`，仍从旧目录合并补入 token（见 `restore_refresh_token_if_missing`）。
    fn migrate_legacy_app_dir() {
        let Some(dest) = Self::user_app_dir() else {
            return;
        };
        let dest_settings = dest.join("settings.json");
        if dest_settings.is_file() {
            // 已有新配置：顺带把旧目录 sidecar 拷过来（若新目录还没有）
            let dest_sidecar = dest.join("jian_token.json");
            if !dest_sidecar.is_file() {
                for src in Self::legacy_user_app_dirs() {
                    let src_sc = src.join("jian_token.json");
                    if src_sc.is_file() {
                        let _ = fs::copy(&src_sc, &dest_sidecar);
                        break;
                    }
                }
            }
            return;
        }
        for src in Self::legacy_user_app_dirs() {
            if !src.is_dir() || src == dest {
                continue;
            }
            let src_settings = src.join("settings.json");
            if !src_settings.is_file() {
                continue;
            }
            if let Err(e) = fs::create_dir_all(&dest) {
                eprintln!("无法创建 {}: {e}", dest.display());
                return;
            }
            if let Err(e) = fs::copy(&src_settings, &dest_settings) {
                eprintln!(
                    "迁移配置失败 {} \u{2192} {}: {e}",
                    src_settings.display(),
                    dest_settings.display()
                );
                return;
            }
            let src_log = src.join("log.txt");
            let dest_log = dest.join("log.txt");
            if src_log.is_file() && !dest_log.is_file() {
                let _ = fs::copy(&src_log, &dest_log);
            }
            let src_sc = src.join("jian_token.json");
            let dest_sc = dest.join("jian_token.json");
            if src_sc.is_file() && !dest_sc.is_file() {
                let _ = fs::copy(&src_sc, &dest_sc);
            }
            tracing_log(&format!(
                "已从旧目录迁移 {} \u{2192} {}",
                src.display(),
                dest.display()
            ));
            return;
        }
    }

    /// 更早的 sismotide / jian-project 配置路径（仅迁移读取，不再写入）。
    fn legacy_user_config_paths() -> Vec<PathBuf> {
        let mut out = Vec::new();
        for dir in Self::legacy_user_app_dirs() {
            out.push(dir.join("settings.json"));
            out.push(dir.join("config.toml"));
        }
        if let Some(proj) = directories::ProjectDirs::from("top", "sismotide", "jian-project") {
            let base = proj.config_dir().to_path_buf();
            out.push(base.join("config.toml"));
            out.push(base.join("config").join("config.toml"));
            out.push(base.join("settings.json"));
        }
        out
    }

    pub fn any_source_enabled(&self) -> bool {
        let st = &self.sources.stations;
        self.sources.jian.enabled
            || self.sources.jian.station_enabled
            || st.jma_enabled
            || st.snet_enabled
            || st.kma_enabled
            || (self.sources.wolfx.enabled
                && (self.sources.wolfx.any_eew() || self.sources.wolfx.any_eqlist()))
            || self.sources.p2pquake.enabled
            || self.sources.fssn.enabled
            || self.sources.pancakes.enabled
            || self.sources.typhoon.enabled
    }

    pub fn sources_runtime_equal(&self, other: &Self) -> bool {
        self.sources.jian.enabled == other.sources.jian.enabled
            && self.sources.jian.eew_enabled == other.sources.jian.eew_enabled
            && self.sources.jian.record_enabled == other.sources.jian.record_enabled
            && self.sources.jian.jma_eew == other.sources.jian.jma_eew
            && self.sources.jian.cea_eew == other.sources.jian.cea_eew
            && self.sources.jian.cwa_eew == other.sources.jian.cwa_eew
            && self.sources.jian.kma_eew == other.sources.jian.kma_eew
            && self.sources.jian.sa_eew == other.sources.jian.sa_eew
            && self.sources.jian.early_est_eew == other.sources.jian.early_est_eew
            && self.sources.jian.station_enabled == other.sources.jian.station_enabled
            && self.sources.jian.ws_url == other.sources.jian.ws_url
            && self.sources.jian.station_ws_url == other.sources.jian.station_ws_url
            && self.sources.jian.refresh_token == other.sources.jian.refresh_token
            && self.sources.jian.access_token == other.sources.jian.access_token
            && self.sources.stations.jma_enabled == other.sources.stations.jma_enabled
            && self.sources.stations.jma_source == other.sources.stations.jma_source
            && self.sources.stations.jma_jian_ws == other.sources.stations.jma_jian_ws
            && self.sources.stations.snet_enabled == other.sources.stations.snet_enabled
            && self.sources.stations.snet_source == other.sources.stations.snet_source
            && self.sources.stations.snet_jian_ws == other.sources.stations.snet_jian_ws
            && self.sources.stations.kma_enabled == other.sources.stations.kma_enabled
            && self.sources.stations.kma_jian_ws == other.sources.stations.kma_jian_ws
            && self.sources.wolfx.enabled == other.sources.wolfx.enabled
            && self.sources.wolfx.eew_enabled == other.sources.wolfx.eew_enabled
            && self.sources.wolfx.eqlist_enabled == other.sources.wolfx.eqlist_enabled
            && self.sources.wolfx.jma_eew == other.sources.wolfx.jma_eew
            && self.sources.wolfx.sc_eew == other.sources.wolfx.sc_eew
            && self.sources.wolfx.fj_eew == other.sources.wolfx.fj_eew
            && self.sources.wolfx.cq_eew == other.sources.wolfx.cq_eew
            && self.sources.wolfx.cenc_eew == other.sources.wolfx.cenc_eew
            && self.sources.wolfx.cwa_eew == other.sources.wolfx.cwa_eew
            && self.sources.wolfx.cenc_eqlist == other.sources.wolfx.cenc_eqlist
            && self.sources.wolfx.jma_eqlist == other.sources.wolfx.jma_eqlist
            && self.sources.wolfx.ws_urls == other.sources.wolfx.ws_urls
            && self.sources.wolfx.http_eqlist == other.sources.wolfx.http_eqlist
            && self.sources.p2pquake.enabled == other.sources.p2pquake.enabled
            && self.sources.p2pquake.http_history == other.sources.p2pquake.http_history
            && self.sources.p2pquake.ws_url == other.sources.p2pquake.ws_url
            && self.sources.p2pquake.poll_secs == other.sources.p2pquake.poll_secs
            && self.sources.fssn.enabled == other.sources.fssn.enabled
            && self.sources.fssn.ws_url == other.sources.fssn.ws_url
            && self.sources.pancakes.enabled == other.sources.pancakes.enabled
            && self.sources.pancakes.base_url == other.sources.pancakes.base_url
            && self.sources.pancakes.ws_url == other.sources.pancakes.ws_url
            && self.sources.pancakes.client_id == other.sources.pancakes.client_id
            && self.sources.pancakes.gq_min_magnitude == other.sources.pancakes.gq_min_magnitude
            && self.sources.pancakes.gq == other.sources.pancakes.gq
            && self.sources.pancakes.jma_volcano == other.sources.pancakes.jma_volcano
            && self.sources.typhoon.enabled == other.sources.typhoon.enabled
            && self.sources.typhoon.url == other.sources.typhoon.url
            && self.sources.typhoon.poll_secs == other.sources.typhoon.poll_secs
            && self.sources.weather.enabled == other.sources.weather.enabled
            && self.sources.weather.ws_url == other.sources.weather.ws_url
            && self.sources.cenc_int.enabled == other.sources.cenc_int.enabled
            && self.sources.cenc_int.http_base == other.sources.cenc_int.http_base
            && self.sources.cenc_int.ws_url == other.sources.cenc_int.ws_url
            && self.sources.cenc_int.list_page_size == other.sources.cenc_int.list_page_size
            && self.sources.map_wx.enabled == other.sources.map_wx.enabled
            && self.sources.map_wx.radar_china == other.sources.map_wx.radar_china
            && self.sources.map_wx.radar_japan == other.sources.map_wx.radar_japan
            && self.sources.map_wx.radar_taiwan == other.sources.map_wx.radar_taiwan
            && self.sources.map_wx.precip_hours == other.sources.map_wx.precip_hours
            && (self.sources.map_wx.radar_opacity - other.sources.map_wx.radar_opacity).abs()
                < 1e-6
            && (self.sources.map_wx.precip_opacity - other.sources.map_wx.precip_opacity).abs()
                < 1e-6
            && self.sources.map_wx.poll_secs == other.sources.map_wx.poll_secs
            && self.sources.map_wx.radar_china_url == other.sources.map_wx.radar_china_url
            && self.sources.map_wx.radar_japan_url == other.sources.map_wx.radar_japan_url
            && self.sources.map_wx.radar_taiwan_url == other.sources.map_wx.radar_taiwan_url
            && self.sources.map_wx.precip_url == other.sources.map_wx.precip_url
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JianTokenSidecar {
    token: String,
    #[serde(default)]
    expire_at: f64,
    #[serde(default)]
    max_connections: u32,
}

fn merge_json(base: serde_json::Value, overlay: serde_json::Value) -> serde_json::Value {
    match (base, overlay) {
        (serde_json::Value::Object(mut a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                match a.remove(&k) {
                    Some(prev) => {
                        a.insert(k, merge_json(prev, v));
                    }
                    None => {
                        a.insert(k, v);
                    }
                }
            }
            serde_json::Value::Object(a)
        }
        (_, overlay) => overlay,
    }
}

fn merge_toml(base: toml::Value, overlay: toml::Value) -> toml::Value {
    match (base, overlay) {
        (toml::Value::Table(mut a), toml::Value::Table(b)) => {
            for (k, v) in b {
                match a.remove(&k) {
                    Some(prev) => {
                        a.insert(k, merge_toml(prev, v));
                    }
                    None => {
                        a.insert(k, v);
                    }
                }
            }
            toml::Value::Table(a)
        }
        (_, overlay) => overlay,
    }
}

fn tracing_log(msg: &str) {
    eprintln!("{msg}");
}

#[cfg(test)]
mod fps_preset_tests {
    use super::{recommend_target_fps, snap_target_fps, snap_target_fps_at_most, TARGET_FPS_PRESETS};

    #[test]
    fn presets_include_common_refresh_rates() {
        assert!(TARGET_FPS_PRESETS.contains(&240));
        assert!(TARGET_FPS_PRESETS.contains(&144));
        assert!(TARGET_FPS_PRESETS.contains(&300));
        assert_eq!(snap_target_fps(100), 90);
        assert_eq!(snap_target_fps(250), 240);
        assert_eq!(snap_target_fps_at_most(200), 180);
        assert_eq!(recommend_target_fps(300, Some(240)), 240);
        assert_eq!(recommend_target_fps(120, Some(240)), 120);
    }

    #[test]
    fn idle_and_cpu_repaint_are_throttled() {
        let mut p = super::PerformanceConfig::default();
        assert_eq!(p.idle_repaint_ms(), 1000);
        p.profile = "eco".into();
        assert_eq!(p.idle_repaint_ms(), 1500);
        assert!(p.wave_repaint_ms() >= 100);
        p.profile = "extreme".into();
        p.target_fps = 60;
        assert_eq!(p.idle_repaint_ms(), 1000);
        p.device = "cpu".into();
        assert_eq!(p.idle_repaint_ms(), 2000);
        assert!(p.repaint_interval_ms() >= 33);
        assert!(p.wave_repaint_ms() >= 100);
        assert!(p.soft_repaint_ms() >= 400);
    }
}
