//! 情报网络：Jian / Wolfx / P2PQuake / FSSN / Pancakes → 归一化事件总线。

pub mod adapter;
pub mod bus;
pub mod auth;
pub mod ip_locate;
pub mod notice;
pub mod place;
pub mod sources;

pub use auth::{
    exchange_access_token, exchange_login_key, format_expire_date, verify_refresh_token, TokenOk,
};
pub use ip_locate::{locate_by_public_ip, IpLocateOk};
pub use notice::{notice_from_server, parse_server_error};
pub mod time_parse;

use starquake_config::{FiltersConfig, SourcesConfig};
use starquake_core::agency_family;
use tokio::task::JoinHandle;
use tracing::info;

pub use bus::{channel, NetEvent, NetRx, NetTx};
pub use place::{fix_place, load_from_dir as load_place_fix};

/// 三源任务句柄：丢弃或 `shutdown` 会中止连接循环，便于设置页热切换。
pub struct NetHub {
    rx: NetRx,
    /// 保留发送端，避免任务通道在 hub 存活期内被关闭
    _tx: NetTx,
    tasks: Vec<JoinHandle<()>>,
}

impl NetHub {
    pub fn try_recv(&mut self) -> Result<NetEvent, tokio::sync::mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }

    pub fn shutdown(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }
}

impl Drop for NetHub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Jian,
    Wolfx,
    P2pQuake,
    Fssn,
    Pancakes,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jian => "jian",
            Self::Wolfx => "wolfx",
            Self::P2pQuake => "p2pquake",
            Self::Fssn => "fssn",
            Self::Pancakes => "pancakes",
        }
    }
}

/// 在已有 tokio runtime 内启动情报源与测站任务。
pub fn spawn_hub(sources: &SourcesConfig) -> NetHub {
    spawn_hub_with_assets(sources, None)
}

/// `assets_stations` 指向 `media/stations/`（含 `nied_point.csv` / `pixels.csv`；原版 kmoni/lmoni / S-Net）。
pub fn spawn_hub_with_assets(
    sources: &SourcesConfig,
    assets_stations: Option<std::path::PathBuf>,
) -> NetHub {
    let (tx, rx) = channel();
    info!(
        jian = sources.jian.enabled,
        wolfx = sources.wolfx.enabled,
        p2p = sources.p2pquake.enabled,
        fssn = sources.fssn.enabled,
        pancakes = sources.pancakes.enabled,
        jma_st = sources.stations.jma_enabled,
        snet = sources.stations.snet_enabled,
        kma = sources.stations.kma_enabled,
        typhoon = sources.typhoon.enabled,
        weather = sources.weather.enabled,
        cenc_int = sources.cenc_int.enabled,
        map_wx = sources.map_wx.enabled,
        "starquake-net: spawning source tasks"
    );
    let mut tasks = Vec::new();
    // 测站由 stations 模块统一管理（无需鉴权）；jian 侧不再重复拉 /kmoni
    let mut jian_cfg = sources.jian.clone();
    jian_cfg.station_enabled = false;
    tasks.extend(sources::jian::spawn(tx.clone(), jian_cfg.clone()));
    tasks.extend(sources::wolfx::spawn(tx.clone(), sources.wolfx.clone()));
    tasks.extend(sources::p2pquake::spawn(tx.clone(), sources.p2pquake.clone()));
    tasks.extend(sources::fssn::spawn(tx.clone(), sources.fssn.clone()));
    tasks.extend(sources::pancakes::spawn(tx.clone(), sources.pancakes.clone()));
    tasks.extend(sources::stations::spawn(
        tx.clone(),
        sources.stations.clone(),
        assets_stations,
    ));
    tasks.extend(sources::typhoon::spawn(tx.clone(), sources.typhoon.clone()));
    tasks.extend(sources::weather::spawn(
        tx.clone(),
        sources.weather.clone(),
        jian_cfg,
    ));
    tasks.extend(sources::cenc_int::spawn(tx.clone(), sources.cenc_int.clone()));
    tasks.extend(sources::map_wx::spawn(tx.clone(), sources.map_wx.clone()));
    NetHub { rx, _tx: tx, tasks }
}

/// 将 `NetEvent` 合并进 `AppSnapshot`（供 UI 帧循环调用）。
pub fn apply_event(snap: &mut starquake_core::AppSnapshot, ev: NetEvent) {
    apply_event_filtered(snap, ev, None);
}

/// 带速报源开关 / 震级阈值 / 白名单过滤（预警由各数据源开关负责）。
pub fn apply_event_filtered(
    snap: &mut starquake_core::AppSnapshot,
    ev: NetEvent,
    filters: Option<&FiltersConfig>,
) {
    match ev {
        NetEvent::Health { source, status } => snap.set_health(source, status),
        NetEvent::Eew(e) => {
            snap.upsert_eew(e);
        }
        NetEvent::Record(r) => {
            if let Some(f) = filters {
                if !record_allowed(
                    f,
                    &r.agency.0,
                    r.magnitude,
                    &r.place,
                    r.magnitude_unknown,
                ) {
                    return;
                }
            }
            snap.upsert_record(r);
        }
        NetEvent::Stations { network, list } => snap.merge_stations(network, list),
        NetEvent::StationClock { network, ts_ms } => snap.touch_station_clock(network, ts_ms),
        NetEvent::Tsunami(t) => snap.upsert_tsunami(t),
        NetEvent::Volcano(v) => snap.upsert_volcano(v),
        NetEvent::Weather(w) => snap.upsert_weather(w),
        NetEvent::WeatherList(list) => snap.replace_weather(list),
        NetEvent::CencInt(e) => snap.upsert_cenc_int(e),
        NetEvent::CencIntList(list) => snap.merge_cenc_int_list(list),
        NetEvent::Typhoons(list) => snap.replace_typhoons(list),
        NetEvent::Radar { kind, frame } => snap.set_radar(kind, frame),
        NetEvent::Precip(frame) => snap.set_precip(frame),
        NetEvent::UserNotice(n) => snap.push_notice(n),
    }
}

fn record_allowed(
    f: &FiltersConfig,
    agency: &str,
    magnitude: f64,
    place: &str,
    magnitude_unknown: bool,
) -> bool {
    let key = agency_family(agency).as_filter_key();
    // 源开关优先：关则整源丢弃（与震级过滤无关）
    if !f.record_source_enabled(key) {
        return false;
    }
    let thr = f.record_mag_threshold(key);
    if thr <= 0.0 {
        return true;
    }
    // 震级调查中（震度速报等）不过滤丢弃
    if magnitude_unknown {
        return true;
    }
    if magnitude + 1e-9 >= thr {
        return true;
    }
    let _ = place;
    false
}
