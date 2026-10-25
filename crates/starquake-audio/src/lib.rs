//! 音效引擎：catalog 路由 + rodio 播放 + 倒计时状态机。

mod catalog;
mod countdown;
mod cue;
mod player;
mod router;
mod tts_speak;

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub use catalog::SoundCatalog;
pub use countdown::CountdownMachine;
pub use cue::{AudioCue, TsunamiCueKind};
pub use router::Router;
pub use tts_speak::{
    configure_tts, edge_voice_presets, list_offline_voices, speak_async, speak_with_config,
    TtsEngine, TtsRuntimeConfig, TtsVoiceInfo,
};

use player::Player;

pub struct AudioEngine {
    catalog: SoundCatalog,
    router: Router,
    player: Option<Player>,
    countdown: CountdownMachine,
    /// 已播过的 EEW 键 → 上次 serial（避免同报重复）
    eew_seen: HashMap<String, u32>,
    /// 本事件已播 caution / warn（对齐要石：各最多一次）
    caution_played: HashSet<String>,
    warn_played: HashSet<String>,
    /// 当前倒计时会话是否已播本机 S 波抵达音
    s_arrive_played: bool,
}

impl AudioEngine {
    pub fn from_catalog(
        catalog_path: impl AsRef<Path>,
        sound_root: impl Into<PathBuf>,
        event_pack: &str,
        countdown_pack: &str,
        master_volume: f32,
        countdown_volume: f32,
        mute: bool,
    ) -> Result<Self> {
        let catalog_path = catalog_path.as_ref();
        let catalog = SoundCatalog::load(catalog_path)
            .with_context(|| format!("load catalog {}", catalog_path.display()))?;
        info!(
            version = catalog.version,
            routing = ?catalog.routing,
            "audio catalog loaded"
        );
        let router = Router::new(sound_root, event_pack, countdown_pack);
        let player = match Player::try_open(master_volume, countdown_volume, mute) {
            Ok(p) => Some(p),
            Err(e) => {
                warn!("audio device unavailable, mute mode: {e:#}");
                None
            }
        };
        Ok(Self {
            catalog,
            router,
            player,
            countdown: CountdownMachine::default(),
            eew_seen: HashMap::new(),
            caution_played: HashSet::new(),
            warn_played: HashSet::new(),
            s_arrive_played: false,
        })
    }

    pub fn play(&mut self, cue: AudioCue) {
        self.play_in_pack(None, cue);
    }

    /// 在指定音效包内播放（如非 JMA 预警用 `eewcn`，JMA 用 `srev`）。
    pub fn play_in_pack(&mut self, pack: Option<&str>, cue: AudioCue) {
        match &cue {
            AudioCue::Countdown(_) => {
                if let Some(path) = self.router.resolve_in_pack(&self.catalog, pack, &cue) {
                    if let Some(p) = &self.player {
                        p.play_speech(&path);
                    }
                }
            }
            AudioCue::CountdownLoop(on) => {
                if *on {
                    if let Some(path) = self.router.resolve_in_pack(&self.catalog, pack, &cue) {
                        if let Some(p) = &self.player {
                            p.set_countdown_loop(Some(&path));
                        }
                    }
                } else if let Some(p) = &self.player {
                    p.set_countdown_loop(None);
                }
            }
            other => {
                if let Some(path) = self.router.resolve_in_pack(&self.catalog, pack, other) {
                    if let Some(p) = &self.player {
                        p.play_oneshot(&path);
                    } else {
                        tracing::debug!(?other, path = %path.display(), "would play (no device)");
                    }
                }
            }
        }
    }

    /// 根据剩余秒驱动地牛倒计时。`start_at` 为开始播报的剩余秒（要石默认 10）。
    pub fn tick_countdown(&mut self, remain_s: Option<i32>, start_at: i32, speech: bool) {
        for cue in self.countdown.tick(remain_s, start_at, speech) {
            self.play(cue);
        }
    }

    /// 启动灌入时静默登记已见报，避免稍后把历史首报当新 issue 再播。
    /// `event_key` 须为稳定键（与 UI 一致：`eew_sidebar_group_key|event_id`），勿用原始 agency 字符串。
    pub fn seed_eew_seen(&mut self, event_key: &str, serial: u32) {
        let key = event_key.to_string();
        let prev = self.eew_seen.get(&key).copied().unwrap_or(0);
        if serial >= prev {
            self.eew_seen.insert(key, serial);
        }
    }

    /// 预警事件音：`pack` 为 `srev`（JMA）或 `eewcn`（其余机构）。
    /// SREV 另附 caution / warn；eewcn：危急仅首报 critical，其后更新一律 update；非危急首报 alert。
    /// `event_key` 同 [`seed_eew_seen`]：同源槽镜像共用一把钥匙，避免 Wolfx/Jian 各播一次首报音。
    pub fn on_eew(
        &mut self,
        event_key: &str,
        serial: u32,
        is_final: bool,
        is_cancel: bool,
        is_warn: bool,
        play_caution: bool,
        pack: &str,
        magnitude: f64,
        intensity_level: u8,
        magnitude_unknown: bool,
    ) {
        let key = event_key.to_string();
        let pack = Some(pack);
        if is_cancel {
            self.play_in_pack(pack, AudioCue::Cancel);
            self.eew_seen.insert(key, serial);
            return;
        }
        // eewcn 危急：震级≥M5 **或者** 烈度≥7（满足其一，不是且）；仅首见播 critical，抬报一律 update
        let eewcn_critical = pack == Some("eewcn")
            && (intensity_level >= 7 || (!magnitude_unknown && magnitude >= 5.0));
        let prev = self.eew_seen.get(&key).copied();
        let advanced = match prev {
            None => {
                // 中途接入且已是第 2+ 报：播更新/最终，勿当首报
                if serial > 1 || is_final {
                    if is_final {
                        self.play_in_pack(pack, AudioCue::Final);
                    } else {
                        self.play_in_pack(pack, AudioCue::Update);
                    }
                } else if eewcn_critical {
                    self.play_in_pack(pack, AudioCue::Critical);
                } else {
                    self.play_in_pack(pack, AudioCue::Issue);
                }
                true
            }
            Some(p) if serial > p => {
                if is_final {
                    self.play_in_pack(pack, AudioCue::Final);
                } else {
                    self.play_in_pack(pack, AudioCue::Update);
                }
                true
            }
            _ => false, // 同 serial 或更旧
        };
        if !advanced {
            return;
        }
        // SREV / 要石：警报播 warn（并吞掉 caution）；否则震度≥3 播 caution
        if pack == Some("srev") {
            if is_warn {
                if !self.warn_played.contains(&key) {
                    self.play_in_pack(pack, AudioCue::Warn);
                    self.warn_played.insert(key.clone());
                    self.caution_played.insert(key.clone());
                }
            } else if play_caution && !self.caution_played.contains(&key) {
                self.play_in_pack(pack, AudioCue::Caution);
                self.caution_played.insert(key.clone());
            }
        }
        self.eew_seen.insert(key, serial);
    }

    /// 本机 S 波剩余秒：首次落到 0 时播抵达音；会话清空后可再触发。
    pub fn tick_s_arrive(&mut self, remain: Option<i32>) {
        match remain {
            Some(sec) if sec <= 0 => {
                if !self.s_arrive_played {
                    self.play(AudioCue::ArriveS);
                    self.s_arrive_played = true;
                }
            }
            Some(_) => {
                self.s_arrive_played = false;
            }
            None => {
                self.s_arrive_played = false;
            }
        }
    }


    /// 播放指定文件（可重复）；用于自定义音效路径与试听。
    pub fn play_file_repeat(&mut self, path: &Path, repeat: u8) {
        let n = repeat.clamp(1, 10);
        if let Some(p) = &self.player {
            p.play_oneshot(path);
            for _ in 1..n {
                p.play_oneshot_queued(path);
            }
        } else {
            tracing::debug!(path = %path.display(), repeat = n, "would play file (no device)");
        }
    }

    /// 解析 cue；若 custom 路径存在则优先，并按 repeat 播放。
    pub fn play_special(
        &mut self,
        cue: AudioCue,
        custom_path: Option<&str>,
        repeat: u8,
    ) {
        if let Some(c) = custom_path.map(str::trim).filter(|s| !s.is_empty()) {
            let p = PathBuf::from(c);
            if p.is_file() {
                self.play_file_repeat(&p, repeat);
                return;
            }
        }
        if let Some(path) = self.router.resolve(&self.catalog, &cue) {
            self.play_file_repeat(&path, repeat);
        }
    }

    pub fn set_mute(&mut self, mute: bool) {
        if let Some(p) = &mut self.player {
            p.set_mute(mute);
        }
    }

    pub fn set_volumes(&mut self, master: f32, countdown_volume: f32) {
        if let Some(p) = &mut self.player {
            p.set_volumes(master, countdown_volume);
        }
    }

    pub fn set_packs(&mut self, event_pack: &str, countdown_pack: &str) {
        self.router.set_packs(event_pack, countdown_pack);
    }
}


/// 仅测路由决策（不播设备）：供单测核对首报/更新映射（须与 [`AudioEngine::on_eew`] 一致）。
pub fn eew_cue_for(
    prev: Option<u32>,
    serial: u32,
    is_final: bool,
    is_cancel: bool,
    eewcn_critical: bool,
) -> Option<&'static str> {
    if is_cancel {
        return Some("cancel");
    }
    match prev {
        None => {
            if serial > 1 || is_final {
                Some(if is_final { "final" } else { "update" })
            } else if eewcn_critical {
                Some("critical")
            } else {
                Some("issue")
            }
        }
        Some(p) if serial > p => {
            if is_final {
                Some("final")
            } else {
                Some("update")
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::eew_cue_for;

    #[test]
    fn first_serial1_is_issue() {
        assert_eq!(eew_cue_for(None, 1, false, false, false), Some("issue"));
    }

    #[test]
    fn first_serial2_is_update_not_issue() {
        assert_eq!(eew_cue_for(None, 2, false, false, false), Some("update"));
    }

    #[test]
    fn advance_serial_is_update() {
        assert_eq!(eew_cue_for(Some(1), 2, false, false, false), Some("update"));
    }

    #[test]
    fn same_serial_silent() {
        assert_eq!(eew_cue_for(Some(2), 2, false, false, false), None);
    }

    #[test]
    fn final_on_advance() {
        assert_eq!(eew_cue_for(Some(1), 2, true, false, false), Some("final"));
    }

    #[test]
    fn critical_only_on_first_then_update() {
        assert_eq!(eew_cue_for(None, 1, false, false, true), Some("critical"));
        // 危急事件抬报：播 update，不再反复 critical
        assert_eq!(eew_cue_for(Some(1), 2, false, false, true), Some("update"));
        assert_eq!(eew_cue_for(None, 2, false, false, true), Some("update"));
    }
}
