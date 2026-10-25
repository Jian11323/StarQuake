//! 业务侧发出的播报意图（由 core/app 决定「要不要响」，audio 只负责播）。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCue {
    /// 首报 / 一般发行
    Issue,
    /// 后续报更新
    Update,
    /// 最终报
    Final,
    /// 取消报
    Cancel,
    /// 本地 S 波剩余整秒播报（0–60）
    Countdown(i32),
    /// 倒计时循环底噪开/关
    CountdownLoop(bool),
    /// 检出提示（SREV prompt）
    Prompt,
    /// 预报注意级（SREV caution，震度≥3）
    Caution,
    /// 警报级（SREV warn，震度≥5弱 / is_warn）
    Warn,
    /// 速报提示（Open EEW CN `newrecord.wav`）
    NewRecord,
    /// 非 JMA 危急首报（M≥5 **或** 烈度≥7，满足其一 → `eewcritical.wav`）
    Critical,
    /// 本机 S 波抵达（`swavearrive.wav`）
    ArriveS,
    /// 海啸播报：grade 1=预报 2=注意报 3=警报/大海啸；kind=issue|update|cancel
    Tsunami { grade: u8, kind: TsunamiCueKind },
    /// UI：连接成功 / 失败 / 通知
    UiConnectOk,
    UiConnectFail,
    UiNotify,
    /// NHK 一级新闻铃（震度 ≥6 弱）
    NhkNewsBell,
    /// JMA 紧急地震速报「警报」专用音
    JmaEewAlert,
    /// 津波警报 / 大津波警报：紧急警报放送第 2 种开始信号
    TsunamiEbs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsunamiCueKind {
    Issue,
    Update,
    Cancel,
}
