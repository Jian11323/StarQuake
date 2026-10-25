//! Cue + 事件包配置 → 文件路径。

use std::path::{Path, PathBuf};

use crate::catalog::SoundCatalog;
use crate::cue::{AudioCue, TsunamiCueKind};

pub struct Router {
    sound_root: PathBuf,
    /// 配置覆盖：事件包名（默认走 routing.default_eew）；海啸/试听用
    event_pack: String,
    countdown_pack: String,
}

impl Router {
    pub fn new(sound_root: impl Into<PathBuf>, event_pack: &str, countdown_pack: &str) -> Self {
        Self {
            sound_root: sound_root.into(),
            event_pack: event_pack.to_string(),
            countdown_pack: countdown_pack.to_string(),
        }
    }

    pub fn set_packs(&mut self, event_pack: &str, countdown_pack: &str) {
        self.event_pack = event_pack.to_string();
        self.countdown_pack = countdown_pack.to_string();
    }

    pub fn sound_root(&self) -> &Path {
        &self.sound_root
    }

    pub fn resolve(&self, catalog: &SoundCatalog, cue: &AudioCue) -> Option<PathBuf> {
        self.resolve_in_pack(catalog, None, cue)
    }

    /// `pack_override` 指定包名时优先用该包解析事件键（如非 JMA 走 `eewcn`）。
    pub fn resolve_in_pack(
        &self,
        catalog: &SoundCatalog,
        pack_override: Option<&str>,
        cue: &AudioCue,
    ) -> Option<PathBuf> {
        let (pack, event_key) = match cue {
            AudioCue::Issue => (
                self.pick_event_pack(catalog, pack_override),
                "issue".into(),
            ),
            AudioCue::Update => (
                self.pick_event_pack(catalog, pack_override),
                "update".into(),
            ),
            AudioCue::Final => (
                self.pick_event_pack(catalog, pack_override),
                "final".into(),
            ),
            AudioCue::Cancel => (
                self.pick_event_pack(catalog, pack_override),
                "cancel".into(),
            ),
            AudioCue::Countdown(sec) => (self.countdown_pack.clone(), format!("{sec}s")),
            AudioCue::CountdownLoop(_) => (self.countdown_pack.clone(), "countdown_loop".into()),
            AudioCue::Prompt => (
                self.pick_event_pack(catalog, pack_override),
                "prompt".into(),
            ),
            AudioCue::Caution => (
                self.pick_event_pack(catalog, pack_override),
                "caution".into(),
            ),
            AudioCue::Warn => (
                self.pick_event_pack(catalog, pack_override),
                "warn".into(),
            ),
            AudioCue::NewRecord => (
                catalog
                    .pack_for_route("record")
                    .unwrap_or("eewcn")
                    .to_string(),
                "record".into(),
            ),
            AudioCue::Critical => (
                self.pick_event_pack(catalog, pack_override),
                "critical".into(),
            ),
            AudioCue::ArriveS => (
                catalog
                    .pack_for_route("default_eew")
                    .unwrap_or("eewcn")
                    .to_string(),
                "arrive_s".into(),
            ),
            AudioCue::Tsunami { grade, kind } => {
                let g = (*grade).clamp(1, 3);
                let k = match kind {
                    TsunamiCueKind::Issue => "issue",
                    TsunamiCueKind::Update => "update",
                    TsunamiCueKind::Cancel => "cancel",
                };
                // 海啸始终 SREV
                (
                    catalog
                        .pack_for_route("jma_eew")
                        .unwrap_or("srev")
                        .to_string(),
                    format!("tsunami{g}_{k}"),
                )
            }
            AudioCue::UiConnectOk => ("ui".into(), "connect_ok".into()),
            AudioCue::UiConnectFail => ("ui".into(), "connect_fail".into()),
            AudioCue::UiNotify => ("ui".into(), "notify".into()),
            AudioCue::NhkNewsBell => ("special".into(), "nhk_news_bell".into()),
            AudioCue::JmaEewAlert => ("special".into(), "jma_eew_alert".into()),
            AudioCue::TsunamiEbs => ("special".into(), "tsunami_ebs".into()),
        };

        let rel = catalog.event_rel_path(&pack, &event_key)?;
        let full = self.sound_root.join(&rel);
        if full.is_file() {
            return Some(full);
        }
        tracing::warn!(path = %full.display(), "audio file missing");
        None
    }

    fn pick_event_pack(&self, catalog: &SoundCatalog, pack_override: Option<&str>) -> String {
        if let Some(p) = pack_override {
            if catalog.packs.contains_key(p) {
                return p.to_string();
            }
        }
        if catalog.packs.contains_key(&self.event_pack) {
            self.event_pack.clone()
        } else {
            catalog
                .pack_for_route("default_eew")
                .unwrap_or("eewcn")
                .to_string()
        }
    }
}
