//! 信息事件（速报）源开关与震级过滤。

use super::{control_row, muted, row_divider, toggle};
use crate::i18n::t;
use egui::{RichText, Ui};
use starquake_config::AppConfig;
use starquake_core::RECORD_FILTER_FAMILIES;

/// 各机构速报开关 + 开启时显示震级阈值（挂在 Jian Project 区内）。
pub fn draw_record_source_switches(ui: &mut Ui, draft: &mut AppConfig) {
    ui.label(
        RichText::new(t("各机构速报独立开关；关闭后该源整条丢弃，震级阈值一并隐藏。"))
            .size(12.0)
            .color(muted()),
    );
    ui.add_space(10.0);
    let mut first = true;
    for fam in RECORD_FILTER_FAMILIES {
        if !first {
            row_divider(ui);
        }
        first = false;
        let key = fam.as_filter_key();
        let mut on = draft.filters.record_source_enabled(key);
        control_row(ui, fam.display_name(), &format!("[{}]", fam.bracket_label()), "\u{25ce}", |ui| {
            toggle(ui, &mut on);
        });
        draft.filters.set_record_source_enabled(key, on);
        if on {
            ui.add_space(4.0);
            let mut val = draft.filters.record_mag_threshold(key);
            control_row(
                ui,
                "最低震级",
                "0=不过滤；低于阈值不显示",
                "\u{25ce}",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut val)
                            .speed(0.1)
                            .range(0.0..=9.9)
                            .fixed_decimals(1)
                            .prefix("M "),
                    );
                },
            );
            draft.filters.set_record_mag(key, val);
        }
    }
}
