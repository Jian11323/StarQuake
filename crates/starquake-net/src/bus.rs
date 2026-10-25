//! 网络事件总线（tokio mpsc）。

use starquake_core::{
    CencIntEvent, EewReport, EqRecord, HealthStatus, PrecipFrame, RadarFrame, RadarKind,
    StationSample,
    TsunamiInfo, TyphoonInfo, UserNotice, VolcanoNotice, WeatherAlert,
};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum NetEvent {
    Health {
        source: &'static str,
        status: HealthStatus,
    },
    Eew(EewReport),
    Record(EqRecord),
    /// 有感测站一帧快照；`network` 为 `kmoni` / `snet` / `kma` 等，用于多网合并
    Stations {
        network: &'static str,
        list: Vec<StationSample>,
    },
    /// 测站网对时/保活（如 Jian `/s-net` 每秒重推只刷新 `timestamp`）；不改列表，只刷新卡片时钟
    StationClock {
        network: &'static str,
        /// 上游信封毫秒时间；缺省由消费端用本机墙钟
        ts_ms: Option<i64>,
    },
    /// JMA 海啸情报（Jian Project API `jma-tsunami`）
    Tsunami(TsunamiInfo),
    /// JMA 火山情报（与地震速报分离）
    Volcano(VolcanoNotice),
    /// 中央台气象预警
    Weather(WeatherAlert),
    /// 气象预警列表（整表同步，仅保留仍生效条目）
    WeatherList(Vec<WeatherAlert>),
    /// CENC 烈度速报（单条，含测站时覆盖）
    CencInt(CencIntEvent),
    /// CENC 烈度速报冷启动列表
    CencIntList(Vec<CencIntEvent>),
    /// 活跃台风快照（整表替换）
    Typhoons(Vec<TyphoonInfo>),
    /// 按种类更新/清除一路雷达（frame=None 清除该路）
    Radar {
        kind: RadarKind,
        frame: Option<RadarFrame>,
    },
    /// 降水叠加
    Precip(Option<PrecipFrame>),
    /// 需弹窗提醒用户（鉴权 / 超限 / 封禁）
    UserNotice(UserNotice),
}

pub type NetTx = mpsc::UnboundedSender<NetEvent>;
pub type NetRx = mpsc::UnboundedReceiver<NetEvent>;

pub fn channel() -> (NetTx, NetRx) {
    mpsc::unbounded_channel()
}

pub fn emit_health(tx: &NetTx, source: &'static str, status: HealthStatus) {
    let _ = tx.send(NetEvent::Health { source, status });
}
