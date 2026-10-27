//! 领域模型与官方色标。

pub mod agency;
pub mod estimate;
pub mod intensity;
pub mod palette;
pub mod sun;
pub mod timezone;
pub mod tts;

use serde::{Deserialize, Serialize};

pub use agency::{
    agency_bracket, agency_family, cea_pr_agency_id, cea_pr_province, eew_feed_priority,
    place_with_agency, AgencyFamily, EEW_FILTER_FAMILIES, RECORD_FILTER_FAMILIES,
};
pub use tts::{
    agency_tts_name, build_eew_tts_script, build_record_tts_script, should_play_eew_tts,
};
pub use estimate::{
    calc_csis, calc_jma_instrumental, estimate_at_site, estimate_epicentral, fill_intensity,
    hypocentral_km, intensity_radius_km, intensity_rings,
};
pub use intensity::{
    cn_intensity_display_unknown, cn_intensity_level, cn_intensity_text,
    instrumental_display_text, instrumental_half_step_text, intensity_kind_for_agency,
    jma_from_instrumental, jma_shindo_level, jma_shindo_text, prefer_intensity_scale,
};
pub use palette::{instrumental_band_level, instrumental_band_rgb, instrumental_rgb};
pub use timezone::{cst, jst};
pub use sun::{
    format_hm_minutes, is_night, solar_altitude_deg, sun_geometry, sunrise_sunset_local_hm,
    terminator_lon_at_lat, UserNotice, UserNoticeKind,
};

// tsunami_official_rgb / tsunami_grade_zh 定义于本文件

/// 机构 / 源 ID（与 WS type 对齐）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgencyId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntensityKind {
    JmaShindo,
    CnIntensity,
}

/// 单次预警报文（归一化后）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EewReport {
    pub agency: AgencyId,
    pub event_id: String,
    pub serial: u32,
    pub place: String,
    pub magnitude: f64,
    pub depth_km: f64,
    pub latitude: f64,
    pub longitude: f64,
    /// 发震时刻 unix ms
    pub origin_ms: i64,
    /// 最大震度/烈度展示文案，如 "5弱" / "Ⅶ"
    pub max_intensity_text: String,
    pub intensity_kind: IntensityKind,
    /// 用于上色的档位索引（JMA: 0–9 对应 0～7；CN: 1–12）
    pub intensity_level: u8,
    #[serde(default)]
    pub is_final: bool,
    #[serde(default)]
    pub is_cancel: bool,
    /// 仮定震源 / 调查中（Wolfx `isAssumption` 等）；不驱动本地倒计时
    #[serde(default)]
    pub is_assumption: bool,
    /// 紧急地震速报（警报）（Wolfx `isWarn` 或震度≥5弱）
    #[serde(default)]
    pub is_warn: bool,
    /// 收到最终报的本地时刻 unix ms（0=尚未最终）
    #[serde(default)]
    pub final_at_ms: i64,
}

/// JMA：`depth=0` 与「极浅」/「ごく浅い」同义；展示为「极浅」，不写成 `0km`。
pub fn format_depth_label(agency: &str, depth_km: f64, hypocenter_unknown: bool) -> String {
    if hypocenter_unknown {
        return "调查中".into();
    }
    if agency_family(agency) == AgencyFamily::Jma && depth_km.abs() < 1e-9 {
        return "极浅".into();
    }
    format!("{:.0}km", depth_km)
}

impl EewReport {
    /// Header / TTS 用深度文案。
    pub fn depth_label(&self) -> String {
        let hypo_unknown = self.is_assumption
            && self.latitude.abs() < 0.01
            && self.longitude.abs() < 0.01;
        format_depth_label(&self.agency.0, self.depth_km, hypo_unknown)
    }

    /// 侧栏/Header 报数标记：挂在机构括号后。JMA 最终报为 `#终`，否则 `#N`。
    pub fn serial_mark(&self) -> Option<String> {
        if self.is_cancel {
            return None;
        }
        if self.is_final && agency_family(&self.agency.0) == AgencyFamily::Jma {
            return Some("#\u{7ec8}".into());
        }
        if self.serial > 0 && self.serial < 10_000 {
            Some(format!("#{}", self.serial))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqRecord {
    pub agency: AgencyId,
    pub place: String,
    pub time_text: String,
    pub magnitude: f64,
    pub intensity_text: String,
    pub intensity_kind: IntensityKind,
    pub intensity_level: u8,
    pub latitude: f64,
    pub longitude: f64,
    pub depth_km: f64,
    /// 发震时刻 unix ms（0 表示未知）
    #[serde(default)]
    pub origin_ms: i64,
    /// CENC 等：自动测定（Wolfx `type=automatic`）
    #[serde(default)]
    pub is_auto: bool,
    /// 震级调查中（上游 `magnitudeUnknown` / M=-1 / 「調査中」）；`magnitude` 无业务含义
    #[serde(default)]
    pub magnitude_unknown: bool,
    /// 震源调查中（上游 `hypocenterUnknown` / 坐标缺测）；经纬深无地图/走时含义
    #[serde(default)]
    pub hypocenter_unknown: bool,
}

impl EqRecord {
    /// 侧栏 / TTS 用震级文案。
    pub fn magnitude_label(&self) -> String {
        if self.magnitude_unknown {
            "调查中".into()
        } else {
            format!("M{:.1}", self.magnitude)
        }
    }

    /// 侧栏 / Header 用深度文案。JMA `depth=0` / 「极浅」原文→「极浅」，不显 `0km`。
    pub fn depth_label(&self) -> String {
        format_depth_label(&self.agency.0, self.depth_km, self.hypocenter_unknown)
    }

    /// 是否有可投影的震中坐标。
    pub fn has_map_coord(&self) -> bool {
        !self.hypocenter_unknown && (self.latitude.abs() > 0.01 || self.longitude.abs() > 0.01)
    }

    /// 供 Header / 地图标记展示（非实时 EEW，serial=0，不驱动正式倒计时语义）
    pub fn as_header_report(&self) -> EewReport {
        EewReport {
            agency: self.agency.clone(),
            event_id: format!("record:{}", self.place),
            serial: 0,
            place: self.place.clone(),
            magnitude: if self.magnitude_unknown {
                0.0
            } else {
                self.magnitude
            },
            depth_km: if self.hypocenter_unknown {
                0.0
            } else {
                self.depth_km
            },
            latitude: if self.hypocenter_unknown {
                0.0
            } else {
                self.latitude
            },
            longitude: if self.hypocenter_unknown {
                0.0
            } else {
                self.longitude
            },
            origin_ms: self.origin_ms,
            max_intensity_text: self.intensity_text.clone(),
            intensity_kind: self.intensity_kind,
            intensity_level: self.intensity_level,
            is_final: false,
            is_cancel: false,
            // 调查中：不驱动本地倒计时（与 EEW 仮定同源开关）
            is_assumption: self.hypocenter_unknown || self.magnitude_unknown,
            is_warn: false,
            final_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationSample {
    pub id: String,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub intensity_text: String,
    pub intensity_kind: IntensityKind,
    pub intensity_level: u8,
    /// 計測震度（JMA 网约 -3～7；CN/KMA 可填烈度近似值；缺测用 -99）
    #[serde(default = "default_instrumental")]
    pub instrumental: f64,
}

fn default_instrumental() -> f64 {
    -99.0
}

/// 海啸影响区域一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsunamiArea {
    pub name: String,
    /// Forecast / Watch / Warning / MajorWarning
    pub grade: String,
}

/// JMA 海啸情报（预报 / 注意报 / 警报等）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TsunamiInfo {
    /// 标题，如「海啸警报」「海啸注意报」「海啸预报」
    pub title: String,
    /// 等级原文或归一化：Forecast / Watch / Warning / MajorWarning / Cancel
    pub grade: String,
    /// 区域摘要（顿号分隔）
    pub areas: String,
    /// 分区明细（有则优先用于地图着色）
    #[serde(default)]
    pub areas_list: Vec<TsunamiArea>,
    pub cancelled: bool,
    /// 发布时间 unix ms（0 未知）
    #[serde(default)]
    pub issued_ms: i64,
    /// 来源标识：jian / pancakes（历史兼容可含 p2p）
    #[serde(default)]
    pub source: String,
    /// 来源优先级：Jian=3 > Pancakes=2（高者优先；旧 P2P=1 仅兼容）
    #[serde(default)]
    pub source_priority: u8,
    /// 预报有效截止/预计解除 unix ms（0 未知）。注意报/警报级通常靠解除报文，不用此字段自动清。
    #[serde(default)]
    pub expire_ms: i64,
}

/// JMA 海啸四档官方色（预报青绿 / 注意报黄 / 警报红 / 大海啸紫）。
pub fn tsunami_official_rgb(grade: &str) -> (u8, u8, u8) {
    match grade {
        "MajorWarning" | "Major" => (0xC0, 0x00, 0xC0),
        "Warning" => (0xFF, 0x00, 0x00),
        "Watch" => (0xFF, 0xFF, 0x00),
        _ => (0x00, 0xE5, 0xFF), // Forecast 津波予報
    }
}

pub fn tsunami_grade_zh(grade: &str) -> &'static str {
    match grade {
        "MajorWarning" | "Major" => "大海啸警报",
        "Warning" => "海啸警报",
        "Watch" => "海啸注意报",
        "Cancel" => "海啸情报解除",
        _ => "海啸预报",
    }
}


/// 台风风圈四象限半径（km）：东北 / 东南 / 西南 / 西北；0 表示该象限无数据。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TyphoonQuadrants {
    pub ne: f64,
    pub se: f64,
    pub sw: f64,
    pub nw: f64,
}

impl TyphoonQuadrants {
    pub fn from_pipe(s: &str) -> Option<Self> {
        let t = s.trim();
        if t.is_empty() {
            return None;
        }
        let parts: Vec<&str> = t.split('|').collect();
        if parts.len() == 1 {
            let v: f64 = parts[0].trim().parse().ok()?;
            if v <= 0.0 {
                return None;
            }
            return Some(Self {
                ne: v,
                se: v,
                sw: v,
                nw: v,
            });
        }
        let mut q = [0.0_f64; 4];
        for i in 0..4 {
            if i < parts.len() {
                q[i] = parts[i].trim().parse::<f64>().unwrap_or(0.0).max(0.0);
            }
        }
        if q.iter().sum::<f64>() <= 0.0 {
            return None;
        }
        Some(Self {
            ne: q[0],
            se: q[1],
            sw: q[2],
            nw: q[3],
        })
    }

    pub fn max_km(self) -> f64 {
        self.ne.max(self.se).max(self.sw).max(self.nw)
    }

    pub fn as_array(self) -> [f64; 4] {
        [self.ne, self.se, self.sw, self.nw]
    }
}

/// 预报路径点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TyphoonForecastPoint {
    pub longitude: f64,
    pub latitude: f64,
    #[serde(default)]
    pub time_text: String,
    #[serde(default)]
    pub strong: String,
    #[serde(default)]
    pub power: i32,
}

/// 某一机构的预报路径。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TyphoonForecast {
    /// 机构名：中国 / 中国台湾 / 日本 / 美国 / 中国香港 等
    pub agency: String,
    pub points: Vec<TyphoonForecastPoint>,
}

/// 实况路径点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TyphoonTrackPoint {
    pub longitude: f64,
    pub latitude: f64,
    #[serde(default)]
    pub time_text: String,
    #[serde(default)]
    pub strong: String,
    #[serde(default)]
    pub power: i32,
    #[serde(default)]
    pub speed_ms: f64,
    #[serde(default)]
    pub pressure_hpa: f64,
    #[serde(default)]
    pub move_direction: String,
    #[serde(default)]
    pub move_speed_kmh: f64,
    #[serde(default)]
    pub radius7: Option<TyphoonQuadrants>,
    #[serde(default)]
    pub radius10: Option<TyphoonQuadrants>,
    #[serde(default)]
    pub radius12: Option<TyphoonQuadrants>,
    /// 仅最新点通常带多源预报
    #[serde(default)]
    pub forecasts: Vec<TyphoonForecast>,
}

/// 活跃台风一条（路径 + 当前中心 + 风圈）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TyphoonInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub name_en: String,
    /// 等级文案，如「强热带风暴」
    #[serde(default)]
    pub strong: String,
    #[serde(default)]
    pub power: i32,
    pub longitude: f64,
    pub latitude: f64,
    #[serde(default)]
    pub pressure_hpa: f64,
    #[serde(default)]
    pub wind_speed_ms: f64,
    #[serde(default)]
    pub move_direction: String,
    #[serde(default)]
    pub move_speed_kmh: f64,
    #[serde(default)]
    pub update_text: String,
    pub points: Vec<TyphoonTrackPoint>,
}

/// 台风等级色（对齐 eewcn）：超强紫 / 强台粉 / 台风橙红 / 强热橙 / 热黄 / 低压蓝。
pub fn typhoon_grade_level(strong: &str, power: i32) -> usize {
    let s = strong
        .chars()
        .filter(|c| *c != '(' && *c != ')' && *c != '\u{ff08}' && *c != '\u{ff09}')
        .collect::<String>();
    let keys = [
        "超强台风",
        "强台风",
        "台风",
        "强热带风暴",
        "热带风暴",
        "热带低压",
    ];
    for (i, k) in keys.iter().enumerate() {
        if s.contains(k) {
            return i;
        }
    }
    if power >= 16 {
        0
    } else if power >= 14 {
        1
    } else if power >= 12 {
        2
    } else if power >= 10 {
        3
    } else if power >= 8 {
        4
    } else {
        5
    }
}

pub fn typhoon_color_rgb(strong: &str, power: i32) -> (u8, u8, u8) {
    const TABLE: [(u8, u8, u8); 6] = [
        (0xCC, 0x00, 0xCC),
        (0xFF, 0x00, 0x66),
        (0xFF, 0x44, 0x00),
        (0xFF, 0x88, 0x00),
        (0xFF, 0xCC, 0x00),
        (0x33, 0x99, 0xFF),
    ];
    TABLE[typhoon_grade_level(strong, power).min(5)]
}

pub fn typhoon_forecast_agency_rgb(agency: &str) -> (u8, u8, u8) {
    match agency.trim() {
        "中国台湾" | "台湾" => (0x00, 0xAA, 0xFF),
        "中国香港" | "香港" => (0xFF, 0xDD, 0x00),
        "日本" => (0xFF, 0x66, 0x00),
        "美国" => (0x44, 0xDD, 0x44),
        "中国" => (0xCC, 0x00, 0xCC),
        _ => (0xCC, 0x00, 0xCC),
    }
}

/// 雷达图种类（中国 / 日本 / 台湾）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadarKind {
    China,
    Japan,
    Taiwan,
}

impl RadarKind {
    pub const ALL: [Self; 3] = [Self::China, Self::Japan, Self::Taiwan];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::China => "china",
            Self::Japan => "japan",
            Self::Taiwan => "taiwan",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::China => 0,
            Self::Japan => 1,
            Self::Taiwan => 2,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "china" | "cn" | "radar" => Some(Self::China),
            "japan" | "jp" | "radar-japan" => Some(Self::Japan),
            "taiwan" | "tw" | "radar-taiwan" => Some(Self::Taiwan),
            _ => None,
        }
    }
}

/// GET 雷达叠加帧（bounds 为地理包围；像素为网络线程已解码的 RGBA8）。
#[derive(Debug, Clone)]
pub struct RadarFrame {
    pub kind: RadarKind,
    pub time: String,
    /// 南纬 / 西经
    pub sw_lat: f64,
    pub sw_lon: f64,
    /// 北纬 / 东经
    pub ne_lat: f64,
    pub ne_lon: f64,
    pub width: u32,
    pub height: u32,
    /// 宽×高×4 的 RGBA8；UI 直接上传纹理，避免主线程再解 PNG。
    pub rgba: std::sync::Arc<Vec<u8>>,
}

/// 降水区多边形（经度在前）。
#[derive(Debug, Clone)]
pub struct PrecipPoly {
    pub level: String,
    pub fill: [u8; 4],
    pub ring: Vec<[f64; 2]>,
}

/// 中国降水 24/48/72 小时预报区。
#[derive(Debug, Clone)]
pub struct PrecipFrame {
    pub hours: u16,
    pub time: String,
    pub polys: Vec<PrecipPoly>,
}

/// 单条台风指纹：id/点数/末点时空与强度。
pub fn typhoon_one_fingerprint(tf: &TyphoonInfo) -> String {
    let last = tf.points.last();
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        tf.id,
        tf.points.len(),
        last.map(|p| p.time_text.as_str()).unwrap_or(""),
        last.map(|p| p.longitude).unwrap_or(tf.longitude),
        last.map(|p| p.latitude).unwrap_or(tf.latitude),
        last.map(|p| p.power).unwrap_or(tf.power),
        last.map(|p| p.strong.as_str()).unwrap_or(tf.strong.as_str()),
    )
}

/// 台风列表指纹（对齐 eewcn `_typhoonDataFingerprint`）。
pub fn typhoon_data_fingerprint(list: &[TyphoonInfo]) -> String {
    let mut parts: Vec<String> = list.iter().map(typhoon_one_fingerprint).collect();
    parts.sort();
    parts.join(";")
}

/// 末点最大风圈半径（km）；无风圈时 0。
pub fn typhoon_max_wind_radius_km(tf: &TyphoonInfo) -> f64 {
    let Some(last) = tf.points.last() else {
        return 0.0;
    };
    let mut max_r = 0.0_f64;
    for q in [last.radius7, last.radius10, last.radius12].into_iter().flatten() {
        max_r = max_r.max(q.max_km());
    }
    max_r
}

const TYPHOON_DEFAULT_FOCUS_RADIUS_KM: f64 = 250.0;

/// 单条台风视角目标：中心 + 跟焦半径 km（无风圈时默认 250）。
pub fn typhoon_focus_one(tf: &TyphoonInfo) -> Option<(f64, f64, f64)> {
    if tf.latitude.abs() <= 0.01 && tf.longitude.abs() <= 0.01 {
        return None;
    }
    let max_r = typhoon_max_wind_radius_km(tf);
    let radius = if max_r > 0.0 {
        max_r
    } else {
        TYPHOON_DEFAULT_FOCUS_RADIUS_KM
    };
    Some((tf.longitude, tf.latitude, radius))
}

/// 多条台风时取中心均值（仅测试/兼容；跟焦请用「本轮变更」的 `typhoon_focus_one`）。
pub fn typhoon_focus_lonlat_radius(list: &[TyphoonInfo]) -> Option<(f64, f64, f64)> {
    let mut sum_lon = 0.0_f64;
    let mut sum_lat = 0.0_f64;
    let mut n = 0_usize;
    let mut max_r = 0.0_f64;
    for tf in list {
        if tf.latitude.abs() <= 0.01 && tf.longitude.abs() <= 0.01 {
            continue;
        }
        sum_lon += tf.longitude;
        sum_lat += tf.latitude;
        n += 1;
        max_r = max_r.max(typhoon_max_wind_radius_km(tf));
    }
    if n == 0 {
        return None;
    }
    let radius = if max_r > 0.0 {
        max_r
    } else {
        TYPHOON_DEFAULT_FOCUS_RADIUS_KM
    };
    Some((sum_lon / n as f64, sum_lat / n as f64, radius))
}

/// JMA 火山情报（降灰预报等；不进地震速报列表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolcanoNotice {
    pub agency: AgencyId,
    /// 火山名（或含情报类型的短标题；不含上游「火山名」字段标签）
    pub name: String,
    /// 描述/标题，如「十勝岳　降灰予報（定時）」或「降灰予報（定時）」
    pub title: String,
    pub time_text: String,
    pub issued_ms: i64,
    #[serde(default)]
    pub latitude: f64,
    #[serde(default)]
    pub longitude: f64,
}

/// 中央台气象预警（Jian `/weather`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherAlert {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub headline: String,
    /// 生效/发布时间原文（UTC+8）
    pub origin_time: String,
    #[serde(default)]
    pub relieve_time: Option<String>,
    #[serde(default)]
    pub province: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub district: String,
    #[serde(default)]
    pub adcode: Option<String>,
    /// 区域中心；上游无坐标时省略（用 Option 语义：NaN 表示无）
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    /// 中央台图标码 p00…
    #[serde(default)]
    pub icon_type: String,
    #[serde(default)]
    pub description: String,
    /// 排序用：解析 origin_time 失败则为 0
    #[serde(default)]
    pub origin_ms: i64,
}

impl WeatherAlert {
    /// `relieveTime` 解析为毫秒（UTC+8）；无法解析则 None。
    pub fn relieve_ms(&self) -> Option<i64> {
        self.relieve_time
            .as_deref()
            .and_then(parse_weather_cst_time_ms)
    }

    /// 是否仍在生效：`relieveTime` 已到则 false；无解除时间则视为仍生效。
    pub fn is_active_at(&self, now_ms: i64) -> bool {
        match self.relieve_ms() {
            Some(rt) => now_ms < rt,
            None => true,
        }
    }
}

/// 气象时间字符串（UTC+8）：`YYYY/MM/DD HH:mm` 等。
pub fn parse_weather_cst_time_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    use chrono::{FixedOffset, NaiveDateTime, TimeZone};
    for fmt in [
        "%Y/%m/%d %H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            let cst = FixedOffset::east_opt(8 * 3600)?;
            return Some(cst.from_local_datetime(&ndt).single()?.timestamp_millis());
        }
    }
    None
}

/// CENC 烈度速报计测台站（NowQuake）；经纬仅供投影，UI 不展示明文坐标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CencIntStation {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub place: String,
    pub latitude: f64,
    pub longitude: f64,
    /// 计测烈度（仪器）
    #[serde(default)]
    pub intensity: Option<f64>,
    #[serde(default)]
    pub forecast_int: Option<f64>,
    #[serde(default)]
    pub pga: Option<f64>,
    #[serde(default)]
    pub pgv: Option<f64>,
}

/// CENC 烈度速报事件（NowQuake）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CencIntEvent {
    pub eq_id: String,
    pub happen_time: String,
    #[serde(default)]
    pub update_time: String,
    #[serde(default)]
    pub hypocenter: String,
    pub magnitude: f64,
    #[serde(default)]
    pub depth_km: Option<f64>,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub max_intensity: Option<f64>,
    #[serde(default)]
    pub max_forecast_intensity: Option<f64>,
    #[serde(default)]
    pub info: String,
    #[serde(default)]
    pub stations: Vec<CencIntStation>,
    /// 发震毫秒（UTC+8 墙钟解析）
    #[serde(default)]
    pub happen_ms: i64,
}

impl CencIntEvent {
    /// 最大计测烈度罗马数字文案（四舍五入）。
    pub fn max_intensity_roman(&self) -> String {
        self.max_intensity
            .map(|v| cn_intensity_text(cn_intensity_round(v)).to_string())
            .unwrap_or_default()
    }
}

/// 仪器烈度 → CSIS 档位（四舍五入，1–12）。
pub fn cn_intensity_round(v: f64) -> u8 {
    if !v.is_finite() || v <= 0.0 {
        return 0;
    }
    ((v + 0.5).floor() as u8).clamp(1, 12)
}

impl StationSample {
    /// 测站选中时的 Header 展示（无震级/深度波圈语义）
    pub fn as_header_report(&self) -> EewReport {
        EewReport {
            agency: AgencyId("station".into()),
            event_id: format!("station:{}", self.id),
            serial: 0,
            place: self.name.clone(),
            magnitude: 0.0,
            depth_km: 0.0,
            latitude: self.latitude,
            longitude: self.longitude,
            origin_ms: 0,
            max_intensity_text: self.intensity_text.clone(),
            intensity_kind: self.intensity_kind,
            intensity_level: self.intensity_level,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: false,
            final_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// 正常
    Normal,
    /// 波动
    Fluctuating,
    /// 异常
    Abnormal,
}

impl HealthStatus {
    pub fn as_zh(self) -> &'static str {
        match self {
            Self::Normal => "正常",
            Self::Fluctuating => "波动",
            Self::Abnormal => "异常",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarTab {
    Records,
    #[default]
    Eew,
    /// JMA 火山情报（与地震速报分离）
    Volcano,
    /// 中央台气象预警
    Weather,
}

/// 列表选中项（驱动高亮 + 地图居中）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListSelection {
    pub tab: SidebarTab,
    pub index: usize,
}

/// 叠加层模式：EEW/速报画波圈；测站只画点
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMode {
    Wave,
    MarkerOnly,
}


/// 本地测站到时推算的震源（仅展示；非官方预警）。
#[derive(Debug, Clone, Copy)]
pub struct LocalHypoEstimate {
    pub latitude: f64,
    pub longitude: f64,
    pub depth_km: f64,
    pub error_level: f64,
    pub used_stations: usize,
}

/// UI 用快照
#[derive(Debug, Clone)]
pub struct AppSnapshot {
    pub active: Option<EewReport>,
    pub overlay_mode: OverlayMode,
    pub records: Vec<EqRecord>,
    pub eew_list: Vec<EewReport>,
    pub stations: Vec<StationSample>,
    pub countdown_s: Option<i32>,
    /// 本机预估烈度/震度（与 countdown_s 同步；无倒计时时为空）
    pub countdown_local_kind: IntensityKind,
    pub countdown_local_level: u8,
    pub countdown_local_text: String,
    pub health_jian: HealthStatus,
    pub health_wolfx: HealthStatus,
    pub health_p2p: HealthStatus,
    pub health_pancakes: HealthStatus,
    pub health_fssn: HealthStatus,
    pub health_weather: HealthStatus,
    pub health_cenc_int: HealthStatus,
    pub tab: SidebarTab,
    pub selection: Option<ListSelection>,
    /// 图例 P-S：是否绘制走时波圈
    pub show_wave_rings: bool,
    /// 当前有效的 JMA 海啸情报（取消后为 None）
    pub tsunami: Option<TsunamiInfo>,
    /// JMA 火山情报（与 records 分离）
    pub volcanoes: Vec<VolcanoNotice>,
    /// 中央台气象预警
    pub weather: Vec<WeatherAlert>,
    /// CENC 烈度速报
    pub cenc_int: Vec<CencIntEvent>,
    /// 活跃台风（路径 / 风圈 / 预报）
    pub typhoons: Vec<TyphoonInfo>,
    /// 当前雷达叠加（中国/日本/台湾可并存，按下标：0中国 1日本 2台湾）
    pub radars: [Option<RadarFrame>; 3],
    /// 中国降水预报区（24/48/72）
    pub precip: Option<PrecipFrame>,
    /// 待弹窗用户提示（鉴权 / 超限 / 封禁）；主循环 take
    pub pending_notices: Vec<UserNotice>,
    /// 台风列表指纹；用于判定「有更新」再播提示音 / 跟焦（对齐 eewcn）
    pub typhoon_fingerprint: String,
    /// 台风指纹变化后待飞跃：(lon, lat, radius_km)；主循环换算 zoom 后 take
    pub pending_typhoon_fly: Option<(f64, f64, f64)>,
    /// 台风指纹变化且非首包时播 UI 提示音
    pub pending_typhoon_hint: bool,
    /// 各测站网最近一帧到达时刻 unix ms（kmoni/snet/kma）
    pub station_updated_ms: std::collections::HashMap<String, i64>,
    /// 本地测站推算震源（无解则 None）
    pub local_hypo: Option<LocalHypoEstimate>,
    /// 新速报写入后待主循环飞跃的 (lon, lat, zoom)；take 后清空
    pub pending_record_fly: Option<(f64, f64, f64)>,
    /// 待朗读速报（预警中不跳转时仍可 TTS；主循环 take）
    pub pending_tts_record: Option<EqRecord>,
    /// 自定义震级区间→最大传播距离(km)：(min_mag, max_mag, max_km)；空则用内置表
    pub wave_spread_km: Vec<(f64, f64, f64)>,
}

impl AppSnapshot {
    /// 单元测试夹具（不进运行时；无上游时用 `empty`）。
    #[cfg(test)]
    pub fn offline_sample() -> Self {
        let eew_a = EewReport {
            agency: AgencyId("jma-eew".into()),
            event_id: "sample-001".into(),
            serial: 3,
            place: "千叶县西北部".into(),
            magnitude: 5.4,
            depth_km: 40.0,
            latitude: 35.7,
            longitude: 140.1,
            origin_ms: 0,
            max_intensity_text: "5弱".into(),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: 5,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: true,
            final_at_ms: 0,
        };
        let eew_b = EewReport {
            agency: AgencyId("cea".into()),
            event_id: "sample-002".into(),
            serial: 2,
            place: "缅甸".into(),
            magnitude: 4.1,
            depth_km: 12.0,
            latitude: 22.0,
            longitude: 98.5,
            origin_ms: 0,
            max_intensity_text: "Ⅵ".into(),
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: 6,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: false,
            final_at_ms: 0,
        };
        let eew_pr = EewReport {
            agency: AgencyId("cea-pr:四川".into()),
            event_id: "sample-002b".into(),
            serial: 1,
            place: "四川阿坝州红原县".into(),
            magnitude: 4.4,
            depth_km: 5.0,
            latitude: 33.0,
            longitude: 102.9,
            origin_ms: 0,
            max_intensity_text: "Ⅵ".into(),
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: 6,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: false,
            final_at_ms: 0,
        };
        let eew_c = EewReport {
            agency: AgencyId("jma-eew".into()),
            event_id: "sample-003".into(),
            serial: 1,
            place: "宫城县冲".into(),
            magnitude: 6.1,
            depth_km: 50.0,
            latitude: 38.2,
            longitude: 142.0,
            origin_ms: 0,
            max_intensity_text: "5强".into(),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: 6,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: true,
            final_at_ms: 0,
        };

        Self {
            active: Some(eew_a.clone()),
            overlay_mode: OverlayMode::Wave,
            records: vec![
                EqRecord {
                    agency: AgencyId("cenc".into()),
                    place: "四川阿坝州汶川县".into(),
                    time_text: "2026-10-09 12:01".into(),
                    magnitude: 4.2,
                    intensity_text: "Ⅴ".into(),
                    intensity_kind: IntensityKind::CnIntensity,
                    intensity_level: 5,
                    latitude: 31.5,
                    longitude: 103.6,
                    depth_km: 12.0,
                    origin_ms: 0,
                    is_auto: false,
                    magnitude_unknown: false,
                    hypocenter_unknown: false,
                },
                EqRecord {
                    agency: AgencyId("jma".into()),
                    place: "千叶县西北部".into(),
                    time_text: "2026-10-09 11:58".into(),
                    magnitude: 5.4,
                    intensity_text: "5弱".into(),
                    intensity_kind: IntensityKind::JmaShindo,
                    intensity_level: 5,
                    latitude: 35.7,
                    longitude: 140.1,
                    depth_km: 40.0,
                    origin_ms: 0,
                    is_auto: false,
                    magnitude_unknown: false,
                    hypocenter_unknown: false,
                },
                EqRecord {
                    agency: AgencyId("jma".into()),
                    place: "福岛县中通".into(),
                    time_text: "2026-10-09 09:12".into(),
                    magnitude: 4.6,
                    intensity_text: "4".into(),
                    intensity_kind: IntensityKind::JmaShindo,
                    intensity_level: 4,
                    latitude: 37.4,
                    longitude: 140.4,
                    depth_km: 10.0,
                    origin_ms: 0,
                    is_auto: false,
                    magnitude_unknown: false,
                    hypocenter_unknown: false,
                },
            ],
            eew_list: vec![eew_a, eew_b, eew_pr, eew_c],
            stations: vec![
                StationSample {
                    id: "TOKYO".into(),
                    name: "东京".into(),
                    latitude: 35.68,
                    longitude: 139.76,
                    intensity_text: "3".into(),
                    intensity_kind: IntensityKind::JmaShindo,
                    intensity_level: 3,
                    instrumental: 3.2,
                },
                StationSample {
                    id: "OSAKA".into(),
                    name: "大阪".into(),
                    latitude: 34.69,
                    longitude: 135.50,
                    intensity_text: "2".into(),
                    intensity_kind: IntensityKind::JmaShindo,
                    intensity_level: 2,
                    instrumental: 2.1,
                },
                StationSample {
                    id: "CD01".into(),
                    name: "成都".into(),
                    latitude: 30.67,
                    longitude: 104.06,
                    intensity_text: "Ⅳ".into(),
                    intensity_kind: IntensityKind::CnIntensity,
                    intensity_level: 4,
                    instrumental: 4.0,
                },
            ],
            countdown_s: Some(28),
            countdown_local_kind: IntensityKind::CnIntensity,
            countdown_local_level: 3,
            countdown_local_text: "3".into(),
            health_jian: HealthStatus::Normal,
            health_wolfx: HealthStatus::Normal,
            health_p2p: HealthStatus::Fluctuating,
            health_pancakes: HealthStatus::Abnormal,
            health_fssn: HealthStatus::Abnormal,
            health_weather: HealthStatus::Normal,
            health_cenc_int: HealthStatus::Normal,
            tab: SidebarTab::Eew,
            selection: Some(ListSelection {
                tab: SidebarTab::Eew,
                index: 0,
            }),
            show_wave_rings: true,
            tsunami: None,
            volcanoes: Vec::new(),
            weather: Vec::new(),
            cenc_int: Vec::new(),
            typhoons: Vec::new(),
            radars: [None, None, None],
            precip: None,
            pending_notices: Vec::new(),
            typhoon_fingerprint: String::new(),
            pending_typhoon_fly: None,
            pending_typhoon_hint: false,
            station_updated_ms: std::collections::HashMap::new(),
            local_hypo: None,
            pending_record_fly: None,
            pending_tts_record: None,
            wave_spread_km: Vec::new(),
        }
    }

    /// 选中列表项 → 更新 Header / 叠加模式；返回应居中的 (lon, lat, 建议 zoom)
    pub fn select_list_item(&mut self, tab: SidebarTab, index: usize) -> Option<(f64, f64, f64)> {
        self.tab = tab;
        self.selection = Some(ListSelection { tab, index });
        match tab {
            SidebarTab::Eew => {
                let ev = self.eew_list.get(index)?.clone();
                let lon = ev.longitude;
                let lat = ev.latitude;
                self.active = Some(ev);
                self.overlay_mode = OverlayMode::Wave;
                Some((lon, lat, 8.0))
            }
            SidebarTab::Records => {
                let r = self.records.get(index)?;
                let lon = r.longitude;
                let lat = r.latitude;
                let has_coord = r.has_map_coord();
                self.active = Some(r.as_header_report());
                self.overlay_mode = OverlayMode::Wave;
                if has_coord {
                    Some((lon, lat, 8.0))
                } else {
                    None
                }
            }
            SidebarTab::Volcano => {
                let v = self.volcanoes.get(index)?;
                let lon = v.longitude;
                let lat = v.latitude;
                // 火山不进左上预警条
                self.active = None;
                self.overlay_mode = OverlayMode::MarkerOnly;
                if lat.abs() <= 0.01 && lon.abs() <= 0.01 {
                    None
                } else {
                    Some((lon, lat, 8.0))
                }
            }
            SidebarTab::Weather => {
                let w = self.weather.get(index)?;
                self.active = None;
                self.overlay_mode = OverlayMode::MarkerOnly;
                match (w.longitude, w.latitude) {
                    (Some(lon), Some(lat)) if lon.abs() > 0.01 || lat.abs() > 0.01 => {
                        Some((lon, lat, 7.5))
                    }
                    _ => None,
                }
            }
        }
    }

    /// 空快照（真数据模式起点；列表由网络事件填充）
    pub fn empty() -> Self {
        Self {
            active: None,
            overlay_mode: OverlayMode::Wave,
            records: Vec::new(),
            eew_list: Vec::new(),
            stations: Vec::new(),
            countdown_s: None,
            countdown_local_kind: IntensityKind::CnIntensity,
            countdown_local_level: 0,
            countdown_local_text: String::new(),
            health_jian: HealthStatus::Abnormal,
            health_wolfx: HealthStatus::Abnormal,
            health_p2p: HealthStatus::Abnormal,
            health_pancakes: HealthStatus::Abnormal,
            health_fssn: HealthStatus::Abnormal,
            health_weather: HealthStatus::Abnormal,
            health_cenc_int: HealthStatus::Abnormal,
            tab: SidebarTab::Eew,
            selection: None,
            show_wave_rings: true,
            tsunami: None,
            volcanoes: Vec::new(),
            weather: Vec::new(),
            cenc_int: Vec::new(),
            typhoons: Vec::new(),
            radars: [None, None, None],
            precip: None,
            pending_notices: Vec::new(),
            typhoon_fingerprint: String::new(),
            pending_typhoon_fly: None,
            pending_typhoon_hint: false,
            station_updated_ms: std::collections::HashMap::new(),
            local_hypo: None,
            pending_record_fly: None,
            pending_tts_record: None,
            wave_spread_km: Vec::new(),
        }
    }

    pub fn push_notice(&mut self, notice: UserNotice) {
        if self
            .pending_notices
            .iter()
            .any(|n| n.dedupe_key == notice.dedupe_key)
        {
            return;
        }
        self.pending_notices.push(notice);
    }

    pub fn take_notices(&mut self) -> Vec<UserNotice> {
        std::mem::take(&mut self.pending_notices)
    }

    pub fn set_tsunami(&mut self, info: Option<TsunamiInfo>) {
        self.tsunami = info;
    }

    /// 海啸并入：Jian > Pancakes；同级取更新的 issued_ms。
    /// 解除报文（注意报/警报等）直接清空；预报可带 expire_ms，到期由 `prune_expired_tsunami` 清除。
    pub fn upsert_tsunami(&mut self, info: TsunamiInfo) {
        let cancel = info.cancelled || info.grade.eq_ignore_ascii_case("Cancel");
        if cancel {
            if let Some(cur) = self.tsunami.as_ref() {
                if info.source_priority < cur.source_priority {
                    return;
                }
            }
            self.tsunami = None;
            return;
        }
        // 入库时已过期的预报直接丢弃，并清掉同级旧残留
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if info.expire_ms > 0 && now_ms >= info.expire_ms {
            if let Some(cur) = self.tsunami.as_ref() {
                if info.source_priority >= cur.source_priority {
                    self.tsunami = None;
                }
            }
            return;
        }
        if let Some(cur) = self.tsunami.as_ref() {
            if info.source_priority < cur.source_priority {
                return;
            }
            if info.source_priority == cur.source_priority
                && info.issued_ms > 0
                && cur.issued_ms > info.issued_ms
            {
                return;
            }
        }
        self.tsunami = Some(info);
    }

    /// 清除已解除/已到期的海啸顶条。
    /// 预报：优先 `expire_ms`；缺截止时用 issued 后 2 小时兜底。注意报/警报等仍主要靠解除报文。
    pub fn prune_expired_tsunami(&mut self, now_ms: i64) {
        let Some(t) = self.tsunami.as_ref() else {
            return;
        };
        if t.cancelled || t.grade.eq_ignore_ascii_case("Cancel") {
            self.tsunami = None;
            return;
        }
        if t.expire_ms > 0 && now_ms >= t.expire_ms {
            self.tsunami = None;
            return;
        }
        let is_forecast = matches!(t.grade.as_str(), "Forecast" | "forecast" | "")
            || t.title.contains("预报")
            || t.title.contains("予報");
        if !is_forecast {
            return;
        }
        // 无解除时间的预报：避免永久残留
        let base = if t.issued_ms > 0 {
            t.issued_ms
        } else {
            return;
        };
        const FORECAST_TTL_MS: i64 = 2 * 60 * 60 * 1000;
        if now_ms >= base + FORECAST_TTL_MS {
            self.tsunami = None;
        }
    }

    pub fn set_health(&mut self, source: &str, status: HealthStatus) {
        match source {
            "jian" => self.health_jian = status,
            // 测站 /kmoni：仅测站通时标「波动」，避免无 /all 却显示 Jian「正常」
            "kmoni" => {
                if status == HealthStatus::Normal {
                    if self.health_jian == HealthStatus::Abnormal {
                        self.health_jian = HealthStatus::Fluctuating;
                    }
                } else if self.health_jian != HealthStatus::Normal {
                    self.health_jian = status;
                }
            }
            "wolfx" => self.health_wolfx = status,
            "p2pquake" | "p2p" => self.health_p2p = status,
            "pancakes" => self.health_pancakes = status,
            "fssn" => self.health_fssn = status,
            "weather" => self.health_weather = status,
            "cenc_int" | "cenc-int" => self.health_cenc_int = status,
            _ => {}
        }
    }

    /// 列表按龄裁剪（供回看）；不因波走完删记录。地图叉标另按 `eew_wave_active`。
    pub fn prune_finished_eews(&mut self) {
        // 每源最新一条常驻；其余按 14 天裁剪
        prune_eew_list(&mut self.eew_list, 14 * 24 * 60 * 60 * 1000, 80);
    }

    /// 插入/更新 EEW：跨 API 同机构族同一事件合并（如 Jian `jma-eew` 与 Wolfx `wolfx-jma-eew`）。
    /// 报数更高优先；报数相同取更高镜像源优先级。不同事件各留一条供多震波圈。
    /// 新预警：刷新左上角 + 切到预警 Tab；不改列表选中（与跟焦解绑，对齐 eewcn）。
    /// 发震时刻超过 [`ORIGIN_STALE_MAX_AGE_MS`]：仍进列表供回看，但不切 Tab、不抢 active、不挡速报飞跃。
    pub fn upsert_eew(&mut self, mut report: EewReport) {
        let fresh = !origin_is_stale(report.origin_ms);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let pos = self
            .eew_list
            .iter()
            .position(|e| same_eew_event(e, &report));
        if let Some(pos) = pos {
            if !prefer_eew_report(&self.eew_list[pos], &report) {
                return;
            }
            let old = &self.eew_list[pos];
            // 保留已有最终报时间戳；新升最终报则打点
            if report.is_final {
                if old.final_at_ms > 0 {
                    report.final_at_ms = old.final_at_ms;
                } else if !old.is_final || report.final_at_ms <= 0 {
                    report.final_at_ms = now_ms;
                }
            } else {
                report.final_at_ms = 0;
            }
            self.eew_list[pos] = report.clone();
        } else {
            if report.is_final && report.final_at_ms <= 0 {
                report.final_at_ms = now_ms;
            }
            self.eew_list.push(report.clone());
        }
        sort_eew_newest_first(&mut self.eew_list);
        prune_eew_list(&mut self.eew_list, 14 * 24 * 60 * 60 * 1000, 80);
        // 过期推送：只维护列表，不触发预警态
        if !fresh {
            return;
        }
        if let Some(pos) = self
            .eew_list
            .iter()
            .position(|e| same_eew_event(e, &report))
        {
            self.active = Some(self.eew_list[pos].clone());
            self.overlay_mode = OverlayMode::Wave;
            self.tab = SidebarTab::Eew;
            // 取消尚未执行的速报飞跃，避免盖过预警跟焦
            self.pending_record_fly = None;
        }
    }

    /// 是否仍有传播中的真预警（按自定义传播表；表空则用内置近似）。
    fn has_live_eew(&self) -> bool {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.eew_list
            .iter()
            .any(|e| eew_wave_active_spread(e, now_ms, &self.wave_spread_km))
    }

    /// 插入速报；全源混合后按机构族+发震时刻去重；新→旧。
    /// 无传播中预警时：收到即选中并飞跃（不论是否列表最新）。
    /// 有传播中预警：只入列表不跳转、不改 active/Tab；符合条件时仍排队 TTS。
    /// 发震时刻超过 [`ORIGIN_STALE_MAX_AGE_MS`]：仍可进列表，但不 TTS、不切 Tab、不飞跃。
    pub fn upsert_record(&mut self, record: EqRecord) {
        // 火山不得混入地震速报
        if record.agency.0.eq_ignore_ascii_case("jma-volcano")
            || record.agency.0.to_ascii_lowercase().contains("volcano")
        {
            return;
        }
        let fresh = record_fresh_for_tts(record.origin_ms);
        let mut is_new_or_upgrade = true;
        let mut announce_tts = false;
        // 跨 API 同机构族同一事件：更新槽位（Jian cenc ↔ Wolfx wolfx-cenc；自动→正式）
        if let Some(pos) = self.records.iter().position(|r| same_record_event(r, &record)) {
            let cur = &self.records[pos];
            // 列表轮询/快照重放：内容未变则静默，避免反复 TTS
            if records_content_equivalent(cur, &record) {
                return;
            }
            let auto_to_final = cur.is_auto && !record.is_auto;
            // 震度速报（调查中）→ 震源\u{00b7}震度情报：优先采用已测定条目
            let detail_upgrade = (cur.hypocenter_unknown && !record.hypocenter_unknown)
                || (cur.magnitude_unknown && !record.magnitude_unknown);
            let prefer_new = if auto_to_final || detail_upgrade {
                true
            } else if (!cur.is_auto && record.is_auto)
                || (!cur.hypocenter_unknown && record.hypocenter_unknown)
            {
                false
            } else {
                let pn = eew_feed_priority(&record.agency.0);
                let po = eew_feed_priority(&cur.agency.0);
                pn > po || (pn == po && record.origin_ms > cur.origin_ms)
            };
            if prefer_new {
                // 近时窗口内才朗读（自动→正式亦同）；过期推送只更新列表
                announce_tts = fresh;
                self.records[pos] = record.clone();
                sort_records_newest_first(&mut self.records);
            } else {
                is_new_or_upgrade = false;
            }
        } else {
            self.records.push(record.clone());
            sort_records_newest_first(&mut self.records);
            if self.records.len() > 80 {
                self.records.truncate(80);
            }
            // 新条目：仅近时事件朗读（冷启动/定期 list 里的旧报不读）
            announce_tts = fresh;
        }
        if !is_new_or_upgrade {
            return;
        }
        // 过期：只维护列表，不触发播报/跟焦
        if !fresh {
            return;
        }
        if announce_tts {
            self.pending_tts_record = Some(record.clone());
        }
        if self.has_live_eew() {
            return;
        }
        let lon = record.longitude;
        let lat = record.latitude;
        let has_coord = record.has_map_coord();
        if let Some(i) = self.records.iter().position(|r| {
            r.agency == record.agency
                && r.place == record.place
                && r.origin_ms == record.origin_ms
                && r.magnitude_unknown == record.magnitude_unknown
                && (record.magnitude_unknown || (r.magnitude - record.magnitude).abs() < 0.05)
        }) {
            self.tab = SidebarTab::Records;
            self.selection = Some(ListSelection {
                tab: SidebarTab::Records,
                index: i,
            });
            self.active = Some(record.as_header_report());
            self.overlay_mode = OverlayMode::Wave;
            if has_coord {
                // 收到即跳：对准本条，而非「列表最新」
                self.pending_record_fly = Some((lon, lat, 7.2));
            }
        }
    }

    /// 按种类写入或清除一路雷达（`None` 清除该 kind）。
    pub fn set_radar(&mut self, kind: RadarKind, frame: Option<RadarFrame>) {
        self.radars[kind.index()] = frame;
    }

    /// 启用的雷达帧切片（跳过空槽）。
    pub fn active_radars(&self) -> impl Iterator<Item = &RadarFrame> {
        self.radars.iter().filter_map(|r| r.as_ref())
    }

    pub fn set_precip(&mut self, frame: Option<PrecipFrame>) {
        self.precip = frame;
    }

    /// 整表替换活跃台风列表（轮询快照）。
    /// 首包只建账，不抢默认速报/预警视角；之后仅对「本轮有变更」的那条置飞跃目标。
    pub fn replace_typhoons(&mut self, list: Vec<TyphoonInfo>) {
        let fp = typhoon_data_fingerprint(&list);
        if fp == self.typhoon_fingerprint {
            self.typhoons = list;
            return;
        }
        let first_pack = self.typhoon_fingerprint.is_empty();
        let old_fp: std::collections::HashMap<String, String> = self
            .typhoons
            .iter()
            .map(|t| (t.id.clone(), typhoon_one_fingerprint(t)))
            .collect();
        let mut changed_ids: Vec<String> = Vec::new();
        for tf in &list {
            let new_one = typhoon_one_fingerprint(tf);
            match old_fp.get(&tf.id) {
                None => changed_ids.push(tf.id.clone()),
                Some(old) if *old != new_one => changed_ids.push(tf.id.clone()),
                _ => {}
            }
        }
        self.typhoon_fingerprint = fp;
        self.typhoons = list;
        if first_pack || changed_ids.is_empty() {
            return;
        }
        // 多条同时变：取末点时间文案最新的一条（同则列表中靠后）
        let target = self
            .typhoons
            .iter()
            .filter(|t| changed_ids.iter().any(|id| id == &t.id))
            .max_by(|a, b| {
                let ta = a
                    .points
                    .last()
                    .map(|p| p.time_text.as_str())
                    .unwrap_or(a.update_text.as_str());
                let tb = b
                    .points
                    .last()
                    .map(|p| p.time_text.as_str())
                    .unwrap_or(b.update_text.as_str());
                ta.cmp(tb).then_with(|| a.id.cmp(&b.id))
            });
        if let Some(tf) = target {
            self.pending_typhoon_hint = true;
            self.pending_typhoon_fly = typhoon_focus_one(tf);
        }
    }

    /// 插入火山情报；按 name+title+time 去重；新→旧。
    pub fn upsert_volcano(&mut self, notice: VolcanoNotice) {
        let dup = self.volcanoes.iter().any(|v| {
            v.name == notice.name
                && v.title == notice.title
                && v.time_text == notice.time_text
        });
        if dup {
            return;
        }
        self.volcanoes.push(notice);
        self.volcanoes.sort_by_key(|a| std::cmp::Reverse(a.issued_ms));
        if self.volcanoes.len() > 40 {
            self.volcanoes.truncate(40);
        }
    }

    /// 插入/更新气象预警（按 id）；新→旧，上限 100。
    /// 已过 `relieveTime` 的条目不入库，若已存在则移除。
    pub fn upsert_weather(&mut self, alert: WeatherAlert) {
        let now_ms = wall_clock_ms();
        if !alert.is_active_at(now_ms) {
            self.weather.retain(|w| w.id != alert.id);
            return;
        }
        if let Some(i) = self.weather.iter().position(|w| w.id == alert.id) {
            self.weather[i] = alert;
        } else {
            self.weather.push(alert);
        }
        self.weather.sort_by_key(|a| std::cmp::Reverse(a.origin_ms));
        if self.weather.len() > 100 {
            self.weather.truncate(100);
        }
    }

    /// 整表替换气象预警（冷启动 / 列表同步）；仅保留仍生效条目。
    pub fn replace_weather(&mut self, list: Vec<WeatherAlert>) {
        let now_ms = wall_clock_ms();
        self.weather = list
            .into_iter()
            .filter(|a| a.is_active_at(now_ms))
            .collect();
        self.weather.sort_by_key(|a| std::cmp::Reverse(a.origin_ms));
        if self.weather.len() > 100 {
            self.weather.truncate(100);
        }
    }

    /// 清除已过 `relieveTime` 的气象预警；返回是否有移除。
    pub fn prune_expired_weather(&mut self, now_ms: i64) -> bool {
        let before = self.weather.len();
        self.weather.retain(|w| w.is_active_at(now_ms));
        before != self.weather.len()
    }

    /// 插入/更新烈度速报（按 eq_id）；有 stations 的覆盖无 stations 的；新→旧，上限 50。
    pub fn upsert_cenc_int(&mut self, event: CencIntEvent) {
        if let Some(i) = self.cenc_int.iter().position(|e| e.eq_id == event.eq_id) {
            let old = &self.cenc_int[i];
            // 新包无测站且旧包有测站：保留测站，只刷新摘要字段
            if event.stations.is_empty() && !old.stations.is_empty() {
                let mut merged = event;
                merged.stations = old.stations.clone();
                self.cenc_int[i] = merged;
            } else {
                self.cenc_int[i] = event;
            }
        } else {
            self.cenc_int.push(event);
        }
        self.cenc_int.sort_by_key(|a| std::cmp::Reverse(a.happen_ms));
        if self.cenc_int.len() > 50 {
            self.cenc_int.truncate(50);
        }
    }

    /// 冷启动列表批量写入（不覆盖已有 stations）。
    pub fn merge_cenc_int_list(&mut self, list: Vec<CencIntEvent>) {
        for e in list {
            self.upsert_cenc_int(e);
        }
    }

    /// 启动/空闲时跟焦最新预警；切 Tab + 刷新左上角，不锁列表选中。
    /// 跳过发震过期条目（仍在列表，但不自动触发预警态）。
    /// 返回 `(lon, lat, zoom)`。
    pub fn focus_latest_event(&mut self) -> Option<(f64, f64, f64)> {
        let idx = self.eew_list.iter().position(|e| {
            !e.is_cancel
                && !origin_is_stale(e.origin_ms)
                && (e.latitude.abs() > 0.01 || e.longitude.abs() > 0.01)
        })?;
        let ev = self.eew_list[idx].clone();
        let lon = ev.longitude;
        let lat = ev.latitude;
        self.active = Some(ev);
        self.overlay_mode = OverlayMode::Wave;
        self.tab = SidebarTab::Eew;
        Some((lon, lat, 7.2))
    }

    /// 无预警时取最新有坐标速报的镜头目标；不改 Tab（仅相机跟焦）。
    pub fn latest_record_camera(&self) -> Option<(f64, f64, f64, String)> {
        let idx = self.records.iter().position(|r| {
            r.latitude.abs() > 0.01 || r.longitude.abs() > 0.01
        })?;
        let rec = &self.records[idx];
        let key = format!(
            "{}|{}|{}|{:.2}",
            rec.agency.0, rec.place, rec.origin_ms, rec.magnitude
        );
        // 略近于默认视口，闲时对准列表第一条震中
        Some((rec.longitude, rec.latitude, 8.5, key))
    }

    /// 启动超时无预警时：选中最新速报并返回镜头目标。
    pub fn focus_latest_record(&mut self) -> Option<(f64, f64, f64, String)> {
        let (lon, lat, zoom, key) = self.latest_record_camera()?;
        let idx = self.records.iter().position(|r| {
            let k = format!(
                "{}|{}|{}|{:.2}",
                r.agency.0, r.place, r.origin_ms, r.magnitude
            );
            k == key
        })?;
        let rec = self.records[idx].clone();
        self.tab = SidebarTab::Records;
        self.selection = Some(ListSelection {
            tab: SidebarTab::Records,
            index: idx,
        });
        self.active = Some(rec.as_header_report());
        self.overlay_mode = OverlayMode::Wave;
        Some((lon, lat, zoom, key))
    }

    /// 选中指定台网当前震度/烈度最大的测站，并返回镜头目标。
    /// `network` 为 `kmoni` / `snet` / `kma`；`all` 表示全网。
    pub fn focus_max_station(&mut self, network: &str) -> Option<(f64, f64, f64)> {
        let prefix = format!("{network}:");
        let best = self
            .stations
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                if network == "all" {
                    true
                } else {
                    s.id.starts_with(&prefix)
                }
            })
            .filter(|(_, s)| {
                s.latitude.abs() > 1e-6 || s.longitude.abs() > 1e-6
            })
            .max_by(|(_, a), (_, b)| {
                let ka = station_rank_key(a);
                let kb = station_rank_key(b);
                ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
            });
        let (_, s) = best?;
        let lon = s.longitude;
        let lat = s.latitude;
        let report = s.as_header_report();
        // 测站不再进侧栏列表；仅更新 Header / 镜头，不改侧栏页签
        self.selection = None;
        self.active = Some(report);
        self.overlay_mode = OverlayMode::MarkerOnly;
        Some((lon, lat, 10.0))
    }

    /// 用一帧有感测站快照替换数据（按震度降序）
    pub fn replace_stations(&mut self, stations: Vec<StationSample>) {
        self.merge_stations("default", stations);
    }

    /// 刷新测站网卡片时钟（中转对时帧 / 原版轮询保活）；不改测站数据。
    pub fn touch_station_clock(&mut self, network: &str, ts_ms: Option<i64>) {
        let ts = ts_ms.filter(|t| *t > 0).unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        });
        if ts > 0 {
            self.station_updated_ms.insert(network.to_string(), ts);
        }
    }

    /// 按网络前缀合并测站帧（`kmoni:` / `snet:` / `kma:`），互不覆盖。
    pub fn merge_stations(&mut self, network: &str, mut stations: Vec<StationSample>) {
        let prefix = format!("{network}:");
        let keep_id = self
            .active
            .as_ref()
            .and_then(|e| e.event_id.strip_prefix("station:"))
            .map(|s| s.to_string());

        for s in &mut stations {
            if !s.id.starts_with(&prefix) {
                s.id = format!("{prefix}{}", s.id);
            }
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        self.stations.retain(|s| !s.id.starts_with(&prefix));
        // 资料帧到达即刷新卡片时钟（对时重推走 StationClock，不经此路径）
        self.station_updated_ms.insert(network.to_string(), now_ms);
        self.stations.extend(stations);
        self.stations.sort_by(|a, b| {
            b.instrumental
                .partial_cmp(&a.instrumental)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.intensity_level.cmp(&a.intensity_level))
                .then_with(|| a.id.cmp(&b.id))
        });
        // 全网测站上图；上限放宽以免截掉低計測值点
        if self.stations.len() > 2500 {
            self.stations.truncate(2500);
        }

        // 有传播中预警时，测站帧不得把 active 抢回测站（否则左上角预警条/跟焦丢失）
        let has_live_eew = self.eew_list.iter().any(|e| eew_wave_active(e, now_ms));
        if let Some(id) = keep_id {
            if !has_live_eew {
                if let Some(s) = self.stations.iter().find(|s| s.id == id) {
                    self.active = Some(s.as_header_report());
                    self.overlay_mode = OverlayMode::MarkerOnly;
                }
            }
        }
    }
}

/// 可作为地图波圈绘制的预警（非测站/速报头、未取消、有坐标）。
/// 台网内「最大」排序键：烈度用等级，震度用計測值。
fn station_rank_key(s: &StationSample) -> f64 {
    match s.intensity_kind {
        IntensityKind::CnIntensity => s.intensity_level as f64,
        IntensityKind::JmaShindo => s.instrumental,
    }
}

pub fn is_wave_candidate(e: &EewReport) -> bool {
    !e.is_cancel
        && !e.event_id.starts_with("station:")
        && !e.event_id.starts_with("record:")
        && (e.latitude.abs() > 0.01 || e.longitude.abs() > 0.01)
}

/// 与 starquake_travel::max_s_wave_spread_km 对齐的本地副本（core 不依赖 travel）。
fn approx_max_s_km(magnitude: f64) -> f64 {
    if magnitude <= 2.0 {
        200.0
    } else if magnitude <= 2.5 {
        350.0
    } else if magnitude <= 3.0 {
        500.0
    } else if magnitude <= 3.5 {
        700.0
    } else if magnitude <= 4.0 {
        900.0
    } else if magnitude <= 4.5 {
        1200.0
    } else if magnitude <= 5.0 {
        1600.0
    } else if magnitude <= 5.5 {
        2100.0
    } else if magnitude <= 6.0 {
        2600.0
    } else if magnitude <= 6.5 {
        3400.0
    } else if magnitude <= 7.0 {
        4200.0
    } else if magnitude <= 7.5 {
        5200.0
    } else if magnitude <= 8.0 {
        6200.0
    } else {
        7500.0
    }
}

/// 按自定义表或内置表取 S 波最大传播距离（km）。
/// `custom`：(min_mag, max_mag, max_km)；命中区间则用该档，否则回退内置表。
pub fn max_s_spread_km(magnitude: f64, custom: &[(f64, f64, f64)]) -> f64 {
    if custom.is_empty() {
        return approx_max_s_km(magnitude);
    }
    for &(lo, hi, km) in custom {
        let a = lo.min(hi);
        let b = lo.max(hi);
        if magnitude + 1e-9 >= a && magnitude <= b + 1e-9 {
            return km;
        }
    }
    approx_max_s_km(magnitude)
}

/// S 波是否仍在传播（地表等效约 5 km/s，与走时兜底一致；走完即视为结束）。
/// 与 GlobalQuake 一致：不按「最终报 + N 分钟」人为延长，波走完即结束。
pub fn eew_wave_active(e: &EewReport, now_ms: i64) -> bool {
    eew_wave_active_spread(e, now_ms, &[])
}

/// 同 [`eew_wave_active`]，可传入自定义震级区间→传播距离表。
pub fn eew_wave_active_spread(e: &EewReport, now_ms: i64, custom: &[(f64, f64, f64)]) -> bool {
    if !is_wave_candidate(e) {
        return false;
    }
    if e.origin_ms <= 0 {
        return true; // 演示样本
    }
    // 过期推送：列表可留，但不算传播中（无波圈/跟焦/预警态）
    if origin_is_stale(e.origin_ms) {
        return false;
    }
    let age_ms = now_ms - e.origin_ms;
    if age_ms < -60_000 {
        return false;
    }
    let elapsed_s = (age_ms as f64 / 1000.0).max(0.0);
    let max_km = max_s_spread_km(e.magnitude, custom);
    // 无走时表时的近似：对齐 eewcn SWAVE_SPEED=4（勿用 5，否则左上条比波圈更早消失）
    elapsed_s * 4.0 < max_km * 1.05
}

/// 仍在传播中的多震：PS 走完即排除（不按 30 分钟挂着）。`max_age_ms` 仅兜底。
pub fn live_wave_eews(list: &[EewReport], now_ms: i64, _max_age_ms: i64) -> Vec<&EewReport> {
    live_wave_eews_spread(list, now_ms, &[])
}

pub fn live_wave_eews_spread<'a>(
    list: &'a [EewReport],
    now_ms: i64,
    spread: &[(f64, f64, f64)],
) -> Vec<&'a EewReport> {
    list.iter()
        .filter(|e| eew_wave_active_spread(e, now_ms, spread))
        .collect()
}

/// 地图叉标：与波圈相同，PS 消失即去掉（GQ 多震不另留 30 分钟）。
pub fn eew_map_markers(list: &[EewReport], now_ms: i64) -> Vec<&EewReport> {
    live_wave_eews(list, now_ms, 0)
}

pub fn eew_map_markers_spread<'a>(
    list: &'a [EewReport],
    now_ms: i64,
    spread: &[(f64, f64, f64)],
) -> Vec<&'a EewReport> {
    live_wave_eews_spread(list, now_ms, spread)
}

/// 预警侧栏：每个侧栏源槽只显示最新一条（跨 API 镜像已去重）；波走完仍留这条供回看。
/// 国网 CEA 与省级 cea-pr 分槽，避免地方局挤掉 `[CN]`。
/// 地图多震仍用 live_wave_eews，与列表无关。
pub fn eew_sidebar_entries(list: &[EewReport]) -> Vec<(usize, &EewReport)> {
    latest_eew_per_source(list)
        .into_iter()
        .filter_map(|e| {
            list.iter()
                .position(|x| x.agency.0 == e.agency.0 && x.event_id == e.event_id)
                .map(|i| (i, e))
        })
        .collect()
}

/// 侧栏分组键：同族镜像合并；国网 CEA（`[CN]`）与省级 cea-pr 分槽。
/// 各省 cea-pr（宁夏/甘肃/云南…）共用一个 `cea-pr` 槽，避免同震多省残留。
pub fn eew_sidebar_group_key(agency: &str) -> String {
    if cea_pr_province(agency).is_some() {
        return "cea-pr".into();
    }
    agency_family(agency).as_filter_key().to_string()
}

/// 每个侧栏源槽（如 JMA，含 Jian/Wolfx 镜像；CEA 国网与各省分槽）只保留发震最新的一条。
pub fn latest_eew_per_source(list: &[EewReport]) -> Vec<&EewReport> {
    let mut best: Vec<&EewReport> = Vec::new();
    for e in list {
        if e.is_cancel
            || e.event_id.starts_with("station:")
            || e.event_id.starts_with("record:")
            || (e.latitude.abs() <= 0.01 && e.longitude.abs() <= 0.01)
        {
            continue;
        }
        let key = eew_sidebar_group_key(&e.agency.0);
        if let Some(pos) = best
            .iter()
            .position(|b| eew_sidebar_group_key(&b.agency.0) == key)
        {
            let cur = best[pos];
            let newer = recency_ms(e.origin_ms) > recency_ms(cur.origin_ms)
                || (e.origin_ms == cur.origin_ms && prefer_eew_report(cur, e));
            if newer {
                best[pos] = e;
            }
        } else {
            best.push(e);
        }
    }
    best.sort_by(|a, b| {
        recency_ms(b.origin_ms)
            .cmp(&recency_ms(a.origin_ms))
            .then(b.serial.cmp(&a.serial))
    });
    best
}

/// 速报内容是否实质相同（列表重放判定）。
fn records_content_equivalent(a: &EqRecord, b: &EqRecord) -> bool {
    a.is_auto == b.is_auto
        && a.magnitude_unknown == b.magnitude_unknown
        && a.hypocenter_unknown == b.hypocenter_unknown
        && (a.magnitude_unknown || (a.magnitude - b.magnitude).abs() < 0.05)
        && a.intensity_level == b.intensity_level
        && a.intensity_text == b.intensity_text
        && a.place == b.place
        && (a.hypocenter_unknown
            || ((a.depth_km - b.depth_km).abs() < 0.5
                && (a.latitude - b.latitude).abs() < 0.02
                && (a.longitude - b.longitude).abs() < 0.02))
}

/// 发震时刻相对现在超过此年龄则视为过期（防 API 推送陈旧预警/速报）。
pub const ORIGIN_STALE_MAX_AGE_MS: i64 = 10 * 60 * 1000;

fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `origin_ms <= 0`（未知）不判过期；已知且年龄超过 [`ORIGIN_STALE_MAX_AGE_MS`] 则为过期。
pub fn origin_is_stale(origin_ms: i64) -> bool {
    if origin_ms <= 0 {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    now.saturating_sub(origin_ms) > ORIGIN_STALE_MAX_AGE_MS
}

/// 近时速报才触发 TTS（与入库过期窗口一致）。
fn record_fresh_for_tts(origin_ms: i64) -> bool {
    origin_ms > 0 && !origin_is_stale(origin_ms)
}

/// 跨 API 是否同一速报事件（同机构族 + 发震时刻接近；调查中条目跳过震中/震级近邻）。
fn same_record_event(a: &EqRecord, b: &EqRecord) -> bool {
    if agency_family(&a.agency.0) != agency_family(&b.agency.0) {
        return false;
    }
    if a.origin_ms > 0 && b.origin_ms > 0 {
        if (a.origin_ms - b.origin_ms).abs() > 120_000 {
            return false;
        }
    } else if a.time_text != b.time_text {
        return false;
    }
    // 震度速报（震源/震级调查中）与后续震源\u{00b7}震度情报：同发震窗即合并
    if a.hypocenter_unknown || b.hypocenter_unknown || a.magnitude_unknown || b.magnitude_unknown
    {
        return true;
    }
    let dlat = (a.latitude - b.latitude).abs();
    let dlon = (a.longitude - b.longitude).abs();
    let dmag = (a.magnitude - b.magnitude).abs();
    dlat <= 0.35 && dlon <= 0.35 && dmag <= 0.6
}

pub fn same_eew_event(a: &EewReport, b: &EewReport) -> bool {
    // 国网 CEA 与各省 cea-pr 分源；同槽内 Jian/Wolfx 镜像仍合并
    if eew_sidebar_group_key(&a.agency.0) != eew_sidebar_group_key(&b.agency.0) {
        return false;
    }
    let id_a = a.event_id.trim();
    let id_b = b.event_id.trim();
    let id_usable = |id: &str| {
        !id.is_empty()
            && id != "—"
            && id != "wolfx-unknown"
            && !id.starts_with("anon:")
            && !id.starts_with("station:")
            && !id.starts_with("record:")
    };
    if id_usable(id_a) && id_usable(id_b) && id_a == id_b {
        return true;
    }
    if a.origin_ms > 0 && b.origin_ms > 0 && (a.origin_ms - b.origin_ms).abs() <= 180_000 {
        let dlat = (a.latitude - b.latitude).abs();
        let dlon = (a.longitude - b.longitude).abs();
        let dmag = (a.magnitude - b.magnitude).abs();
        if dlat <= 0.5 && dlon <= 0.5 && dmag <= 0.8 {
            return true;
        }
    }
    false
}

/// `new` 是否应覆盖 `old`：报数优先，其次镜像源优先级。
///
/// 取消报例外：必须覆盖同事件已有预报，即使 `serial` 回退
/// （如 2026-10-16 JMA 误报：已发到 #42 后取消电报仍标 `serialNo=37`）。
fn prefer_eew_report(old: &EewReport, new: &EewReport) -> bool {
    if new.is_cancel && !old.is_cancel {
        return true;
    }
    if old.is_cancel && !new.is_cancel {
        // 已取消后仅接受更高报数的异常续报
        return new.serial > old.serial;
    }
    if new.serial != old.serial {
        return new.serial > old.serial;
    }
    eew_feed_priority(&new.agency.0) >= eew_feed_priority(&old.agency.0)
}

/// 同事件镜像合并为一条；**不同** `event_id` 均保留（供 cinema / 多震跟焦）。
/// 与 [`latest_eew_per_source`] 不同：同源多震不会被压成一条。
///
/// 排序稳定、按序轮播：发震旧→新 → 源槽 → `event_id`（一源多震与多源多震同一队列）。
pub fn dedupe_same_eew_events<'a>(list: &[&'a EewReport]) -> Vec<&'a EewReport> {
    let mut best: Vec<&EewReport> = Vec::new();
    for &e in list {
        if e.is_cancel
            || e.event_id.starts_with("station:")
            || e.event_id.starts_with("record:")
            || (e.latitude.abs() <= 0.01 && e.longitude.abs() <= 0.01)
        {
            continue;
        }
        if let Some(pos) = best.iter().position(|b| same_eew_event(b, e)) {
            if prefer_eew_report(best[pos], e) {
                best[pos] = e;
            }
        } else {
            best.push(e);
        }
    }
    best.sort_by(|a, b| {
        recency_ms(a.origin_ms)
            .cmp(&recency_ms(b.origin_ms))
            .then(
                eew_sidebar_group_key(&a.agency.0).cmp(&eew_sidebar_group_key(&b.agency.0)),
            )
            .then(a.event_id.cmp(&b.event_id))
            .then(a.serial.cmp(&b.serial))
    });
    best
}

/// cinema / 左上框用的稳定事件键（源槽 + event_id，不受镜像 agency 字符串影响）。
pub fn cinema_event_key(e: &EewReport) -> String {
    format!("{}|{}", eew_sidebar_group_key(&e.agency.0), e.event_id)
}

fn recency_ms(origin_ms: i64) -> i64 {
    if origin_ms <= 0 {
        i64::MIN / 4
    } else {
        origin_ms
    }
}

/// 列表回看：每 agency 保留最新非取消一条（对齐 eewcn）；其余按龄/条数裁剪。
fn prune_eew_list(list: &mut Vec<EewReport>, max_age_ms: i64, max_len: usize) {
    if list.is_empty() {
        return;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // 每源最新（非取消、有坐标）必须保留，否则冷启动历史会被 6h 窗清空
    let mut keep_ids: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    // 每侧栏源槽最新一条（国网 CEA / 各省 cea-pr 分槽；跨 API 镜像只留一条）
    let mut best_by_family: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, e) in list.iter().enumerate() {
        if e.is_cancel
            || e.event_id.starts_with("station:")
            || e.event_id.starts_with("record:")
            || (e.latitude.abs() <= 0.01 && e.longitude.abs() <= 0.01)
        {
            continue;
        }
        let fam = eew_sidebar_group_key(&e.agency.0);
        match best_by_family.get(&fam) {
            None => {
                best_by_family.insert(fam, i);
            }
            Some(&j) => {
                let cur = &list[j];
                let newer = recency_ms(e.origin_ms) > recency_ms(cur.origin_ms)
                    || (e.origin_ms == cur.origin_ms && prefer_eew_report(cur, e));
                if newer {
                    best_by_family.insert(fam, i);
                }
            }
        }
    }
    for &i in best_by_family.values() {
        keep_ids.insert((list[i].agency.0.clone(), list[i].event_id.clone()));
    }

    list.retain(|e| {
        if e.event_id.starts_with("station:") || e.event_id.starts_with("record:") {
            return false;
        }
        if keep_ids.contains(&(e.agency.0.clone(), e.event_id.clone())) {
            return true;
        }
        if e.origin_ms <= 0 {
            return true;
        }
        let age = now_ms - e.origin_ms;
        age >= -60_000 && age <= max_age_ms
    });
    if list.len() > max_len {
        // 先保住每源最新，再按新→旧截断
        let protected: std::collections::HashSet<(String, String)> = keep_ids;
        sort_eew_newest_first(list);
        let mut out = Vec::with_capacity(max_len);
        for e in list.drain(..) {
            let key = (e.agency.0.clone(), e.event_id.clone());
            if (protected.contains(&key) || out.len() < max_len)
                && (out.len() < max_len || protected.contains(&key)) {
                    out.push(e);
                }
        }
        // 若 protected 把长度顶破，再按新→旧截到 max_len（仍尽量留 protected）
        if out.len() > max_len {
            let mut keep_p: Vec<EewReport> = Vec::new();
            let mut rest: Vec<EewReport> = Vec::new();
            for e in out {
                let key = (e.agency.0.clone(), e.event_id.clone());
                if protected.contains(&key) {
                    keep_p.push(e);
                } else {
                    rest.push(e);
                }
            }
            sort_eew_newest_first(&mut rest);
            let room = max_len.saturating_sub(keep_p.len());
            rest.truncate(room);
            out = keep_p;
            out.extend(rest);
            sort_eew_newest_first(&mut out);
        }
        *list = out;
    }
}

fn sort_eew_newest_first(list: &mut [EewReport]) {
    list.sort_by(|a, b| {
        recency_ms(b.origin_ms)
            .cmp(&recency_ms(a.origin_ms))
            .then(b.serial.cmp(&a.serial))
    });
}

fn sort_records_newest_first(list: &mut [EqRecord]) {
    list.sort_by_key(|a| std::cmp::Reverse(recency_ms(a.origin_ms)));
}

#[cfg(test)]
mod eew_dedup_tests {
    use super::*;

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn sample(agency: &str, event_id: &str, serial: u32) -> EewReport {
        EewReport {
            agency: AgencyId(agency.into()),
            event_id: event_id.into(),
            serial,
            place: "宫古岛近海".into(),
            magnitude: 4.6,
            depth_km: 10.0,
            latitude: 24.8,
            longitude: 125.3,
            origin_ms: now_ms(),
            max_intensity_text: "2".into(),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: 2,
            is_final: false,
            is_cancel: false,
            is_assumption: false,
            is_warn: false,
            final_at_ms: 0,
        }
    }

    #[test]
    fn serial_mark_jma_final_is_zhong() {
        let mut ev = sample("jma-eew", "1", 4);
        assert_eq!(ev.serial_mark().as_deref(), Some("#4"));
        ev.is_final = true;
        assert_eq!(ev.serial_mark().as_deref(), Some("#\u{7ec8}"));
        let mut gq = sample("gq", "2", 5);
        gq.is_final = true;
        assert_eq!(gq.serial_mark().as_deref(), Some("#5"));
    }

    #[test]
    fn upsert_merges_jian_and_wolfx_jma() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        snap.active = None;
        snap.upsert_eew(sample("wolfx-jma-eew", "20261011201300", 6));
        snap.upsert_eew(sample("jma-eew", "20261011201300", 6));
        assert_eq!(snap.eew_list.len(), 1);
        assert_eq!(snap.eew_list[0].agency.0, "jma-eew");
        let side = eew_sidebar_entries(&snap.eew_list);
        assert_eq!(side.len(), 1);
    }

    #[test]
    fn cancel_overrides_even_when_serial_goes_backwards() {
        // 复现 2026-10-16 误报：已到 #42，取消电报仍标 serial=37
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        snap.active = None;
        snap.upsert_eew(sample("jma-eew", "20261016123810", 42));
        assert_eq!(snap.eew_list.len(), 1);
        assert!(!snap.eew_list[0].is_cancel);

        let mut cancel = sample("jma-eew", "20261016123810", 37);
        cancel.is_cancel = true;
        cancel.magnitude = 0.0;
        cancel.depth_km = 0.0;
        cancel.latitude = 0.0;
        cancel.longitude = 0.0;
        snap.upsert_eew(cancel);

        assert_eq!(snap.eew_list.len(), 1);
        assert!(snap.eew_list[0].is_cancel);
        assert_eq!(snap.eew_list[0].serial, 37);
        // 侧栏 / 波圈候选跳过取消
        assert!(eew_sidebar_entries(&snap.eew_list).is_empty());
    }

    #[test]
    fn older_non_cancel_does_not_override_cancel() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        let mut cancel = sample("jma-eew", "evt-c", 5);
        cancel.is_cancel = true;
        snap.upsert_eew(cancel);
        snap.upsert_eew(sample("jma-eew", "evt-c", 4));
        assert!(snap.eew_list[0].is_cancel);
        assert_eq!(snap.eew_list[0].serial, 5);
    }

    #[test]
    fn sidebar_keeps_national_cea_and_cea_pr() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        let mut cea = sample("cea", "cn-1", 2);
        cea.place = "缅甸".into();
        cea.latitude = 22.0;
        cea.longitude = 98.5;
        let mut pr = sample("cea-pr:云南", "pr-1", 1);
        pr.place = "云南某地".into();
        pr.latitude = 25.0;
        pr.longitude = 102.0;
        pr.origin_ms = now_ms() - 30_000;
        snap.upsert_eew(cea);
        snap.upsert_eew(pr);
        let side = eew_sidebar_entries(&snap.eew_list);
        assert_eq!(side.len(), 2);
        let agencies: Vec<_> = side.iter().map(|(_, e)| e.agency.0.as_str()).collect();
        assert!(agencies.contains(&"cea"));
        assert!(agencies.contains(&"cea-pr:云南"));
    }

    #[test]
    fn sidebar_merges_all_cea_pr_provinces_to_one_slot() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        let t0 = now_ms();
        let mut nx = sample("cea-pr:宁夏", "nx-1", 1);
        nx.place = "宁夏隆德县".into();
        nx.latitude = 35.6;
        nx.longitude = 106.1;
        nx.origin_ms = t0 - 60_000;
        let mut gs = sample("cea-pr:甘肃", "gs-1", 2);
        gs.place = "宁夏固原市原州区".into();
        gs.latitude = 36.0;
        gs.longitude = 106.2;
        gs.origin_ms = t0;
        let mut cn = sample("cea", "cn-1", 2);
        cn.place = "宁夏固原市原州区".into();
        cn.latitude = 36.0;
        cn.longitude = 106.2;
        cn.origin_ms = t0;
        snap.upsert_eew(nx);
        snap.upsert_eew(gs);
        snap.upsert_eew(cn);
        let side = eew_sidebar_entries(&snap.eew_list);
        assert_eq!(side.len(), 2);
        let keys: Vec<_> = side
            .iter()
            .map(|(_, e)| eew_sidebar_group_key(&e.agency.0))
            .collect();
        assert!(keys.iter().any(|k| k == "cea"));
        assert!(keys.iter().any(|k| k == "cea-pr"));
        // 省级槽保留更新报（甘肃 #2）
        let pr = side
            .iter()
            .find(|(_, e)| eew_sidebar_group_key(&e.agency.0) == "cea-pr")
            .unwrap()
            .1;
        assert_eq!(pr.agency.0, "cea-pr:甘肃");
        assert_eq!(pr.serial, 2);
    }

    #[test]
    fn wolfx_cenc_eew_merges_into_cn_slot_prefer_jian() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        snap.upsert_eew(sample("wolfx-cea", "cn-evt", 3));
        snap.upsert_eew(sample("cea", "cn-evt", 3));
        assert_eq!(snap.eew_list.len(), 1);
        assert_eq!(snap.eew_list[0].agency.0, "cea");
        assert_eq!(eew_sidebar_group_key("wolfx-cea"), "cea");
        assert_eq!(agency_bracket("wolfx-cea"), "CN");
    }

    #[test]
    fn stale_origin_record_lists_without_side_effects() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        snap.records.clear();
        snap.pending_tts_record = None;
        snap.pending_record_fly = None;
        let tab0 = snap.tab;
        let old = EqRecord {
            agency: AgencyId("cenc".into()),
            place: "测试地点".into(),
            time_text: "2020-01-01".into(),
            magnitude: 4.0,
            intensity_text: "Ⅲ".into(),
            intensity_kind: IntensityKind::CnIntensity,
            intensity_level: 3,
            latitude: 30.0,
            longitude: 104.0,
            depth_km: 10.0,
            origin_ms: 1_577_836_800_000, // 2020，远超 10 分钟窗口
            is_auto: false,
            magnitude_unknown: false,
            hypocenter_unknown: false,
        };
        snap.upsert_record(old);
        assert!(snap.records.iter().any(|r| r.place == "测试地点"));
        assert!(snap.pending_tts_record.is_none());
        assert!(snap.pending_record_fly.is_none());
        assert_eq!(snap.tab, tab0);
    }

    #[test]
    fn stale_origin_eew_enters_list_without_alert() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        snap.active = None;
        let tab0 = snap.tab;
        let mut eew = sample("jma-eew", "old-evt", 1);
        eew.origin_ms = now_ms() - ORIGIN_STALE_MAX_AGE_MS - 1_000;
        snap.upsert_eew(eew);
        assert_eq!(snap.eew_list.len(), 1);
        assert_eq!(snap.eew_list[0].event_id, "old-evt");
        assert!(snap.active.is_none());
        assert_eq!(snap.tab, tab0);
        assert!(!eew_wave_active(&snap.eew_list[0], now_ms()));
    }

    #[test]
    fn higher_serial_from_wolfx_updates_jian_slot() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        snap.active = None;
        snap.upsert_eew(sample("jma-eew", "20261011201300", 5));
        snap.upsert_eew(sample("wolfx-jma-eew", "20261011201300", 6));
        assert_eq!(snap.eew_list.len(), 1);
        assert_eq!(snap.eew_list[0].serial, 6);
    }

    #[test]
    fn live_eew_blocks_record_from_stealing_active() {
        let mut snap = AppSnapshot::offline_sample();
        snap.eew_list.clear();
        snap.records.clear();
        snap.active = None;
        let mut eew = sample("jma-eew", "20261011201300", 3);
        eew.origin_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        snap.upsert_eew(eew.clone());
        assert_eq!(snap.tab, SidebarTab::Eew);

        // active 被清掉时，旧逻辑会让速报抢走；传播中预警必须挡住
        snap.active = None;
        snap.upsert_record(EqRecord {
            agency: AgencyId("p2pquake".into()),
            place: "宫古岛近海".into(),
            time_text: "2026-10-12 12:00".into(),
            magnitude: 4.6,
            intensity_text: "2".into(),
            intensity_kind: IntensityKind::JmaShindo,
            intensity_level: 2,
            latitude: 24.8,
            longitude: 125.3,
            depth_km: 10.0,
            origin_ms: eew.origin_ms,
            is_auto: false,
            magnitude_unknown: false,
            hypocenter_unknown: false,
        });
        assert!(snap.records.iter().any(|r| r.place == "宫古岛近海"));
        assert!(snap.pending_tts_record.is_some());
        assert!(snap.active.is_none());
        assert_eq!(snap.tab, SidebarTab::Eew);
        assert!(snap.pending_record_fly.is_none());
    }

    fn sample_typhoon(id: &str, lon: f64, lat: f64, time: &str) -> TyphoonInfo {
        TyphoonInfo {
            id: id.into(),
            name: id.into(),
            name_en: String::new(),
            strong: "热带风暴".into(),
            power: 8,
            longitude: lon,
            latitude: lat,
            pressure_hpa: 990.0,
            wind_speed_ms: 20.0,
            move_direction: "北".into(),
            move_speed_kmh: 10.0,
            update_text: time.into(),
            points: vec![TyphoonTrackPoint {
                longitude: lon,
                latitude: lat,
                time_text: time.into(),
                strong: "热带风暴".into(),
                power: 8,
                speed_ms: 20.0,
                pressure_hpa: 990.0,
                move_direction: "北".into(),
                move_speed_kmh: 10.0,
                radius7: None,
                radius10: None,
                radius12: None,
                forecasts: vec![],
            }],
        }
    }

    #[test]
    fn typhoon_first_pack_no_fly() {
        let mut snap = AppSnapshot::empty();
        snap.replace_typhoons(vec![
            sample_typhoon("a", 120.0, 25.0, "2026-10-24 12:00"),
            sample_typhoon("b", 140.0, 30.0, "2026-10-24 12:00"),
        ]);
        assert!(snap.pending_typhoon_fly.is_none());
        assert!(!snap.pending_typhoon_hint);
        assert_eq!(snap.typhoons.len(), 2);
    }

    #[test]
    fn typhoon_update_flies_only_changed() {
        let mut snap = AppSnapshot::empty();
        snap.replace_typhoons(vec![
            sample_typhoon("a", 120.0, 25.0, "2026-10-24 12:00"),
            sample_typhoon("b", 140.0, 30.0, "2026-10-24 12:00"),
        ]);
        snap.replace_typhoons(vec![
            sample_typhoon("a", 120.0, 25.0, "2026-10-24 12:00"),
            sample_typhoon("b", 141.0, 31.0, "2026-10-24 15:00"),
        ]);
        let fly = snap.pending_typhoon_fly.expect("should fly");
        assert!((fly.0 - 141.0).abs() < 1e-6);
        assert!((fly.1 - 31.0).abs() < 1e-6);
        assert!(snap.pending_typhoon_hint);
    }

    fn sample_weather(id: &str, relieve: Option<&str>) -> WeatherAlert {
        WeatherAlert {
            id: id.into(),
            title: "测试地点发布测试预警".into(),
            headline: String::new(),
            origin_time: "2026/10/25 08:00".into(),
            relieve_time: relieve.map(str::to_string),
            province: "测试省".into(),
            city: String::new(),
            district: String::new(),
            adcode: None,
            latitude: None,
            longitude: None,
            icon_type: "p0003003".into(),
            description: String::new(),
            origin_ms: 0,
        }
    }

    #[test]
    fn weather_upsert_skips_and_removes_expired() {
        let mut snap = AppSnapshot::empty();
        snap.upsert_weather(sample_weather("wx-a", Some("2099/12/31 23:59")));
        assert_eq!(snap.weather.len(), 1);

        snap.upsert_weather(sample_weather("wx-a", Some("2020/01/01 00:00")));
        assert!(snap.weather.is_empty());

        snap.upsert_weather(sample_weather("wx-b", Some("2099/12/31 23:59")));
        assert_eq!(snap.weather.len(), 1);
        snap.upsert_weather(sample_weather("wx-c", Some("2020/01/01 00:00")));
        assert_eq!(snap.weather.len(), 1);
        assert_eq!(snap.weather[0].id, "wx-b");
    }

    #[test]
    fn weather_prune_expired_by_relieve_time() {
        let mut snap = AppSnapshot::empty();
        snap.upsert_weather(sample_weather("active", Some("2099/12/31 23:59")));
        snap.upsert_weather(sample_weather("gone", Some("2020/01/01 00:00")));
        assert_eq!(snap.weather.len(), 1);
        let now = now_ms();
        assert!(!snap.prune_expired_weather(now));
        assert_eq!(snap.weather.len(), 1);
    }

    #[test]
    fn weather_replace_filters_expired() {
        let mut snap = AppSnapshot::empty();
        snap.replace_weather(vec![
            sample_weather("keep", Some("2099/12/31 23:59")),
            sample_weather("drop", Some("2020/01/01 00:00")),
        ]);
        assert_eq!(snap.weather.len(), 1);
        assert_eq!(snap.weather[0].id, "keep");
    }
}
