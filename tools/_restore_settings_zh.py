# -*- coding: utf-8 -*-
"""Restore Chinese in settings.rs. Write only via UTF-8 bytes. Avoid U+00B7 literal in output."""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
P = ROOT / "crates/starquake-ui/src/settings.rs"
LOG = ROOT / "target" / "_restore_result.txt"

# ASCII-safe middle dot in Rust source
MD = r"\u{00b7}"


def heal_to_text(raw: bytes) -> str:
    # Fix double-encoded / lone middle-dot BEFORE scanning
    raw = raw.replace(b"\xc2\xc2\xb7", b".")  # placeholder
    raw = raw.replace(b"\xc3\x82\xc2\xb7", b".")
    raw = raw.replace(b"\xc2\xb7", b".")
    raw = raw.replace(b"\xb7", b".")
    # multiply sign
    raw = raw.replace(b"\xc3\x97", b"x")
    raw = raw.replace(b"\xd7", b"x")

    out = bytearray()
    i = 0
    while i < len(raw):
        b = raw[i]
        if b < 0x80:
            out.append(b)
            i += 1
            continue
        if 0xC2 <= b <= 0xDF and i + 1 < len(raw) and 0x80 <= raw[i + 1] <= 0xBF:
            out.extend(raw[i : i + 2])
            i += 2
            continue
        if (
            0xE0 <= b <= 0xEF
            and i + 2 < len(raw)
            and 0x80 <= raw[i + 1] <= 0xBF
            and 0x80 <= raw[i + 2] <= 0xBF
        ):
            out.extend(raw[i : i + 3])
            i += 3
            continue
        i += 1  # drop orphan
    return out.decode("utf-8")


def write_utf8(path: Path, text: str) -> None:
    path.write_bytes(text.encode("utf-8"))


def replace_fn(text: str, fn_name: str, new_body: str) -> str:
    a = text.find(f"fn {fn_name}(")
    if a < 0:
        raise SystemExit(f"missing fn {fn_name}")
    m = re.search(r"\nfn |\Z", text[a + 1 :])
    end = a + 1 + (m.start() if m else len(text) - a)
    return text[:a] + new_body.rstrip() + "\n\n" + text[end:].lstrip("\n")


def soft_sub(text: str, pattern: str, repl: str, count: int = 1) -> str:
    text2, n = re.subn(pattern, repl, text, count=count)
    if not n:
        LOG.write_text(
            (LOG.read_text(encoding="utf-8") + f"\nWARN miss: {pattern[:100]}\n")
            if LOG.exists()
            else f"WARN miss: {pattern[:100]}\n",
            encoding="utf-8",
        )
    return text2


SOURCES_INNER = rf'''
    section(ui, "服务器状态", "\u{{25c9}}", |ui| {{
        status_row(ui, "Jian Project", health.jian);
        row_divider(ui);
        status_row(ui, "Wolfx", health.wolfx);
        row_divider(ui);
        status_row(ui, "P2PQuake", health.p2p);
    }});

    section(ui, "Jian Project", "\u{{25ce}}", |ui| {{
        control_row(ui, "启用 /all", "预警与速报（需令牌）", "\u{{26a1}}", |ui| {{
            toggle(ui, &mut draft.sources.jian.enabled);
        }});
        row_divider(ui);
        control_row(ui, "登录密钥 lk_", "换取长期 Token rt_ 并写入用·配置", "\u{{25ce}}", |ui| {{
            framed_edit(ui, login_key, 200.0);
        }});
        ui.add_space(8.0);
        ui.horizontal(|ui| {{
            let label = if auth_busy {{ "换票中…" }} else {{ "换票并保存" }};
            if glass_btn(ui, label, true).clicked() && !auth_busy {{
                exchange = true;
            }}
        }});
        ui.add_space(6.0);
        if let Some(rt) = draft.sources.jian.refresh_token.as_deref() {{
            let short = if rt.len() > 12 {{
                format!("{{}}…{{}}", &rt[..6], &rt[rt.len() - 4..])
            }} else {{
                rt.to_string()
            }};
            let exp = format_expire_date(draft.sources.jian.refresh_expire_at)
                .map(|d| format!("（约至 {{d}}）"))
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("长期 Token：{{short}}{{exp}}"))
                    .size(11.5)
                    .color(chrome().muted),
            );
        }} else {{
            ui.label(
                RichText::new("尚未保存长期 Token。也可设环境变量 JIAN_REFRESH_TOKEN。")
                    .size(11.5)
                    .color(chrome().muted),
            );
        }}
        if draft.sources.jian.access_token.is_some() {{
            ui.label(
                RichText::new("短期 at_ ·由 JIAN_ACCESS_TOKEN 注入（不写入文件）。")
                    .size(11.5)
                    .color(chrome().muted),
            );
        }}
    }});

    section(ui, "Wolfx", "\u{{25ce}}", |ui| {{
        control_row(ui, "启用 Wolfx", "总开关", "\u{{26a1}}", |ui| {{
            toggle(ui, &mut draft.sources.wolfx.enabled);
        }});
        row_divider(ui);
        control_row(ui, "JMA EEW", "Wolfx 预警通道", "\u{{26a1}}", |ui| {{
            toggle(ui, &mut draft.sources.wolfx.eew_enabled);
        }});
        row_divider(ui);
        control_row(ui, "CENC 速报列表", "Wolfx 台网速报", "\u{{25ce}}", |ui| {{
            toggle(ui, &mut draft.sources.wolfx.eqlist_enabled);
        }});
    }});

    section(ui, "P2PQuake", "\u{{25ce}}", |ui| {{
        control_row(ui, "启用 P2PQuake", "气象厅地震情报", "\u{{26a1}}", |ui| {{
            toggle(ui, &mut draft.sources.p2pquake.enabled);
        }});
        row_divider(ui);
        control_row(ui, "轮询间隔（秒）", "保存后立即重连", "\u{{25ce}}", |ui| {{
            ui.add(
                egui::DragValue::new(&mut draft.sources.p2pquake.poll_secs)
                    .speed(1.0)
                    .range(10..=300),
            );
        }});
    }});


    section(ui, "实时测站", "\u{{25ce}}", |ui| {{
        control_row(ui, "日本 JMA/NIED", "总开关", "\u{{26a1}}", |ui| {{
            toggle(ui, &mut draft.sources.stations.jma_enabled);
        }});
        if draft.sources.stations.jma_enabled {{
            row_divider(ui);
            control_row(ui, "JMA 测站源", "原版 kmoni/lmoni 或 Jian 中转", "\u{{25ce}}", |ui| {{
                egui::ComboBox::from_id_salt("set_jma_station_src")
                    .selected_text(jma_station_src_label(&draft.sources.stations.jma_source))
                    .width(200.0)
                    .show_ui(ui, |ui| {{
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "jian".into(),
                            "Jian 中转 /kmoni",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "kmoni".into(),
                            "原版 NIED kmoni",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "lmoni".into(),
                            "原版 NIED lmoni",
                        );
                    }});
            }});
        }}
        row_divider(ui);
        control_row(ui, "S-Net", "·上强震网（中转 / 原版 MSIL）", "\u{{25ce}}", |ui| {{
            toggle(ui, &mut draft.sources.stations.snet_enabled);
        }});
        if draft.sources.stations.snet_enabled {{
            row_divider(ui);
            control_row(ui, "S-Net 源", "原版需 media/stations/pixels.csv", "\u{{25ce}}", |ui| {{
                egui::ComboBox::from_id_salt("set_snet_station_src")
                    .selected_text(snet_station_src_label(&draft.sources.stations.snet_source))
                    .width(200.0)
                    .show_ui(ui, |ui| {{
                        ui.selectable_value(
                            &mut draft.sources.stations.snet_source,
                            "jian".into(),
                            "Jian 中转 /s-net",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.snet_source,
                            "original".into(),
                            "原版 MSIL 切片",
                        );
                    }});
            }});
        }}
        row_divider(ui);
        control_row(ui, "韩国 KMA", "PEWS 测站（Jian API）", "\u{{25ce}}", |ui| {{
            toggle(ui, &mut draft.sources.stations.kma_enabled);
        }});
    }});


    filters_ui::draw_eew_and_record_filters(ui, draft);
'''

# Fix the f-string mess - use normal string with MD
SOURCES_INNER = '''
    section(ui, "服务器状态", "\\u{25c9}", |ui| {
        status_row(ui, "Jian Project", health.jian);
        row_divider(ui);
        status_row(ui, "Wolfx", health.wolfx);
        row_divider(ui);
        status_row(ui, "P2PQuake", health.p2p);
    });

    section(ui, "Jian Project", "\\u{25ce}", |ui| {
        control_row(ui, "启用 /all", "预警与速报（需令牌）", "\\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.jian.enabled);
        });
        row_divider(ui);
        control_row(ui, "登录密钥 lk_", "换取长期 Token rt_ 并写入用·配置", "\\u{25ce}", |ui| {
            framed_edit(ui, login_key, 200.0);
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let label = if auth_busy { "换票中…" } else { "换票并保存" };
            if glass_btn(ui, label, true).clicked() && !auth_busy {
                exchange = true;
            }
        });
        ui.add_space(6.0);
        if let Some(rt) = draft.sources.jian.refresh_token.as_deref() {
            let short = if rt.len() > 12 {
                format!("{}…{}", &rt[..6], &rt[rt.len() - 4..])
            } else {
                rt.to_string()
            };
            let exp = format_expire_date(draft.sources.jian.refresh_expire_at)
                .map(|d| format!("（约至 {d}）"))
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("长期 Token：{short}{exp}"))
                    .size(11.5)
                    .color(chrome().muted),
            );
        } else {
            ui.label(
                RichText::new("尚未保存长期 Token。也可设环境变量 JIAN_REFRESH_TOKEN。")
                    .size(11.5)
                    .color(chrome().muted),
            );
        }
        if draft.sources.jian.access_token.is_some() {
            ui.label(
                RichText::new("短期 at_ ·由 JIAN_ACCESS_TOKEN 注入（不写入文件）。")
                    .size(11.5)
                    .color(chrome().muted),
            );
        }
    });

    section(ui, "Wolfx", "\\u{25ce}", |ui| {
        control_row(ui, "启用 Wolfx", "总开关", "\\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.enabled);
        });
        row_divider(ui);
        control_row(ui, "JMA EEW", "Wolfx 预警通道", "\\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.eew_enabled);
        });
        row_divider(ui);
        control_row(ui, "CENC 速报列表", "Wolfx 台网速报", "\\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.wolfx.eqlist_enabled);
        });
    });

    section(ui, "P2PQuake", "\\u{25ce}", |ui| {
        control_row(ui, "启用 P2PQuake", "气象厅地震情报", "\\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.p2pquake.enabled);
        });
        row_divider(ui);
        control_row(ui, "轮询间隔（秒）", "保存后立即重连", "\\u{25ce}", |ui| {
            ui.add(
                egui::DragValue::new(&mut draft.sources.p2pquake.poll_secs)
                    .speed(1.0)
                    .range(10..=300),
            );
        });
    });


    section(ui, "实时测站", "\\u{25ce}", |ui| {
        control_row(ui, "日本 JMA/NIED", "总开关", "\\u{26a1}", |ui| {
            toggle(ui, &mut draft.sources.stations.jma_enabled);
        });
        if draft.sources.stations.jma_enabled {
            row_divider(ui);
            control_row(ui, "JMA 测站源", "原版 kmoni/lmoni 或 Jian 中转", "\\u{25ce}", |ui| {
                egui::ComboBox::from_id_salt("set_jma_station_src")
                    .selected_text(jma_station_src_label(&draft.sources.stations.jma_source))
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "jian".into(),
                            "Jian 中转 /kmoni",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "kmoni".into(),
                            "原版 NIED kmoni",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.jma_source,
                            "lmoni".into(),
                            "原版 NIED lmoni",
                        );
                    });
            });
        }
        row_divider(ui);
        control_row(ui, "S-Net", "·上强震网（中转 / 原版 MSIL）", "\\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.stations.snet_enabled);
        });
        if draft.sources.stations.snet_enabled {
            row_divider(ui);
            control_row(ui, "S-Net 源", "原版需 media/stations/pixels.csv", "\\u{25ce}", |ui| {
                egui::ComboBox::from_id_salt("set_snet_station_src")
                    .selected_text(snet_station_src_label(&draft.sources.stations.snet_source))
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut draft.sources.stations.snet_source,
                            "jian".into(),
                            "Jian 中转 /s-net",
                        );
                        ui.selectable_value(
                            &mut draft.sources.stations.snet_source,
                            "original".into(),
                            "原版 MSIL 切片",
                        );
                    });
            });
        }
        row_divider(ui);
        control_row(ui, "韩国 KMA", "PEWS 测站（Jian API）", "\\u{25ce}", |ui| {
            toggle(ui, &mut draft.sources.stations.kma_enabled);
        });
    });


    filters_ui::draw_eew_and_record_filters(ui, draft);
'''

AUDIO_BODY = r'''fn draw_audio(ui: &mut Ui, draft: &mut AppConfig) -> bool {
    let mut test = false;
    section(ui, "音频设置", "\u{266a}", |ui| {
        control_row(ui, "静音", "", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.audio.mute);
        });
        row_divider(ui);
        control_row(ui, "主音量", "", "\u{25ce}", |ui| {
            ui.add(egui::Slider::new(&mut draft.audio.master_volume, 0.0..=1.0).show_value(true));
        });
        row_divider(ui);
        control_row(ui, "倒计时音量", "语音与滴答", "\u{25ce}", |ui| {
            ui.add(
                egui::Slider::new(&mut draft.audio.countdown_volume, 0.0..=1.0).show_value(true),
            );
        });
        row_divider(ui);
        control_row(ui, "倒计时音效", "本机 S 波剩余时播倒计时", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.audio.play_countdown);
        });
        row_divider(ui);
        control_row(ui, "倒计时语音", "播报十、九…；关闭则只留滴答", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.audio.countdown_speech);
        });
        row_divider(ui);
        control_row(ui, "开始播报秒数", "剩余秒数降到此值后开始", "\u{25ce}", |ui| {
            ui.add(
                egui::DragValue::new(&mut draft.audio.countdown_start)
                    .range(0..=60)
                    .speed(1.0),
            );
        });
        row_divider(ui);
        control_row(ui, "仅强有感时播", "本机预估震度≥3 或烈度≥5 时播倒计时", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.audio.countdown_only_intense);
        });
        row_divider(ui);
        control_row(ui, "Windows TTS", "预警/速报语音播报（系统朗读）", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.audio.tts_enabled);
        });
        row_divider(ui);
        control_row(ui, "试听", "按草稿音量立即试听（无需先保存）", "\u{25b6}", |ui| {
            if glass_btn(ui, "试听首报音", true).clicked() {
                test = true;
            }
        });
    });

    test
}
'''

GENERAL_BODY = r'''fn draw_general(
    ui: &mut Ui,
    draft: &mut AppConfig,
    map_pick: &mut Option<MapPickKind>,
    capture_view: &mut bool,
) {
    section(ui, "外观", "\u{25d0}", |ui| {
        control_row(ui, "界面主题", "切换后设置页立即·随；保存后应用到主界面", "\u{25d0}", |ui| {
            egui::ComboBox::from_id_salt("set_theme")
                .selected_text(theme_display(&draft.ui.theme))
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.ui.theme, "dark".into(), "暗色");
                    ui.selectable_value(&mut draft.ui.theme, "light".into(), "亮色");
                });
        });
        row_divider(ui);
        control_row(ui, "地图底图", "·随主题，或固定矢量 / 地形地图", "\u{25a3}", |ui| {
            egui::ComboBox::from_id_salt("set_basemap_preset")
                .selected_text(basemap_preset_label(draft))
                .width(200.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(is_basemap_auto(draft), "·随主题（推荐）")
                        .clicked()
                    {
                        draft.map.basemap = "auto".into();
                    }
                    if ui
                        .selectable_label(draft.map.basemap == "vector", "矢量地图")
                        .clicked()
                    {
                        draft.map.basemap = "vector".into();
                        draft.ui.theme = "dark".into();
                    }
                    if ui
                        .selectable_label(
                            draft.map.basemap == "tiles" || draft.map.basemap == "both",
                            "地形地图",
                        )
                        .clicked()
                    {
                        draft.map.basemap = "tiles".into();
                        draft.ui.theme = "light".into();
                    }
                });
        });
    });

    section(ui, "窗口", "\u{25a3}", |ui| {
        control_row(ui, "窗口置顶", "主窗口始终保持在其他窗口之上", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.always_on_top);
        });
    });

    section(ui, "地图显示", "\u{25a3}", |ui| {
        control_row(ui, "预警自动·焦", "新预警或报数更新时地图移到震中", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.auto_follow_eew);
        });
        row_divider(ui);
        control_row(ui, "波圈自动缩放", "随 P/S 波传播自动调整地图缩放", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.auto_zoom_waves);
        });
        row_divider(ui);
        control_row(ui, "P/S 波圈", "在地图上绘制走时波阵面", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_wave_rings);
        });
        row_divider(ui);
        control_row(ui, "烈度图例", "地图·下角显示烈度色标", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_legend);
        });
    });

    section(ui, "界面偏好", "\u{2630}", |ui| {
        control_row(ui, "JST 时钟", "主界面额外显示日本标准时", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.ui.show_nied_clock);
        });
        row_divider(ui);
        control_row(ui, "启动侧栏", "仅下次启动生效", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("gen_default_tab")
                .selected_text(tab_display(&draft.ui.sidebar_default_tab))
                .width(120.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.ui.sidebar_default_tab, "eew".into(), "预警");
                    ui.selectable_value(
                        &mut draft.ui.sidebar_default_tab,
                        "records".into(),
                        "速报",
                    );
                    ui.selectable_value(
                        &mut draft.ui.sidebar_default_tab,
                        "station".into(),
                        "测站",
                    );
                });
        });
        row_divider(ui);
        control_row(ui, "烈度标度", "列表与色标优先使用的标度", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("gen_intensity_scale")
                .selected_text(scale_display(&draft.ui.intensity_scale))
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.ui.intensity_scale, "auto".into(), "自动");
                    ui.selectable_value(&mut draft.ui.intensity_scale, "jma".into(), "气象厅震度");
                    ui.selectable_value(&mut draft.ui.intensity_scale, "cn".into(), "中国烈度");
                });
        });
    });

    section(ui, "默认视口", "\u{25ce}", |ui| {
        control_row(ui, "经度 / 纬度 / 缩放", "Home 无本机位置时使用", "\u{25ce}", |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.map.default_zoom)
                        .speed(0.1)
                        .range(2.0..=12.0)
                        .prefix("Z "),
                );
                ui.add(
                    egui::DragValue::new(&mut draft.map.default_lat)
                        .speed(0.1)
                        .range(-90.0..=90.0)
                        .prefix("\u{03c6} "),
                );
                ui.add(
                    egui::DragValue::new(&mut draft.map.default_lon)
                        .speed(0.1)
                        .range(-180.0..=180.0)
                        .prefix("\u{03bb} "),
                );
            });
        });
        row_divider(ui);
        let picking = matches!(*map_pick, Some(MapPickKind::DefaultViewport));
        let sub = if picking {
            "·点击主窗口地图选点，Esc 取消"
        } else {
            "在主地图单击选点，或采用当前视野中心"
        };
        control_row(ui, "地图选点", sub, "\u{25ce}", |ui| {
            ui.horizontal(|ui| {
                if glass_btn(ui, if picking { "选点中…" } else { "选点" }, picking).clicked() {
                    *map_pick = Some(MapPickKind::DefaultViewport);
                }
                if glass_btn(ui, "当前视野", false).clicked() {
                    *capture_view = true;
                }
            });
        });
    });

    section(ui, "本机位置", "\u{25ce}", |ui| {
        let mut has_local = draft.local.latitude.is_some() && draft.local.longitude.is_some();
        control_row(ui, "启用本机经纬", "倒计时与 Home 标记", "\u{25ce}", |ui| {
            if toggle(ui, &mut has_local).changed() {
                if has_local {
                    draft.local.latitude.get_or_insert(35.68);
                    draft.local.longitude.get_or_insert(139.76);
                } else {
                    draft.local.latitude = None;
                    draft.local.longitude = None;
                }
            }
        });
        if has_local {
            row_divider(ui);
            let mut lat = draft.local.latitude.unwrap_or(35.68);
            let mut lon = draft.local.longitude.unwrap_or(139.76);
            control_row(ui, "纬度 / 经度", "", "\u{25ce}", |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut lon).speed(0.01).range(-180.0..=180.0));
                    ui.add(egui::DragValue::new(&mut lat).speed(0.01).range(-90.0..=90.0));
                });
            });
            draft.local.latitude = Some(lat);
            draft.local.longitude = Some(lon);
            row_divider(ui);
            let picking = matches!(*map_pick, Some(MapPickKind::LocalLocation));
            let sub = if picking {
                "·点击主窗口地图选点，Esc 取消"
            } else {
                "在主地图单击设置本机位置"
            };
            control_row(ui, "地图选点", sub, "\u{25ce}", |ui| {
                if glass_btn(ui, if picking { "选点中…" } else { "选点" }, picking).clicked() {
                    *map_pick = Some(MapPickKind::LocalLocation);
                }
            });
        }
    });
}
'''

ADVANCED_BODY = r'''fn draw_advanced(ui: &mut Ui, draft: &mut AppConfig, action: &mut Option<SettingsAction>) {
    section(ui, "走时表", "\u{25ce}", |ui| {
        control_row(ui, "主表（近·）", "默认 JMA2001", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("adv_travel_primary")
                .selected_text(draft.travel.primary.clone())
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.travel.primary, "jma2001".into(), "jma2001");
                    ui.selectable_value(&mut draft.travel.primary, "ak135".into(), "ak135");
                });
        });
        row_divider(ui);
        control_row(ui, "远·回退", "主表插值失败时使用", "\u{25ce}", |ui| {
            egui::ComboBox::from_id_salt("adv_travel_fallback")
                .selected_text(draft.travel.fallback.clone())
                .width(140.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.travel.fallback, "ak135".into(), "ak135");
                    ui.selectable_value(&mut draft.travel.fallback, "jma2001".into(), "jma2001");
                });
        });
        row_divider(ui);
        control_row(ui, "常速直线兜底", "插值失败时启用", "\u{25ce}", |ui| {
            toggle(ui, &mut draft.travel.linear_last_resort);
        });
    });

    section(ui, "瓦片缓存", "\u{25ce}", |ui| {
        control_row(ui, "内存瓦片缓存", "清除后将重新下载可见瓦片", "\u{25ce}", |ui| {
            if glass_btn(ui, "一键清除", false).clicked() {
                *action = Some(SettingsAction::ClearTileCache);
            }
        });
    });

    section(ui, "配置文件", "\u{25ce}", |ui| {
        if let Some(path) = AppConfig::user_config_path() {
            ui.label(
                RichText::new(format!("用·覆盖：{}", path.display()))
                    .size(12.0)
                    .color(chrome().muted),
            );
        } else {
            ui.label(
                RichText::new("未能解析用·配置目录。")
                    .size(12.0)
                    .color(chrome().muted),
            );
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new("WebSocket 等开发向端点·直接编辑配置文件；访问令牌不会写入覆盖文件。")
                .size(12.0)
                .color(chrome().muted),
        );
    });
}
'''

ABOUT_BODY = (
    "fn draw_about(ui: &mut Ui) {\n"
    f'    section(ui, crate::APP_DISPLAY_NAME, "\\u{{24d8}}", |ui| {{\n'
    '        ui.label(\n'
    '            RichText::new(format!("版本 {}", crate::APP_VERSION_DISPLAY))\n'
    "                .size(14.0)\n"
    "                .color(chrome().text),\n"
    "        );\n"
    "        ui.add_space(8.0);\n"
    "        ui.label(\n"
    "            RichText::new(\n"
    '                "·平台地震预警与速报视监客·端。聚合多源实时情报，在地图上展示震中、\\\n'
    '走时波圈与测站烈度，并提供本机倒计时与音效播报。",\n'
    "            )\n"
    "            .size(13.0)\n"
    "            .color(chrome().text),\n"
    "        );\n"
    "        ui.add_space(10.0);\n"
    "        ui.label(\n"
    '            RichText::new("数据源")\n'
    "                .size(13.0)\n"
    "                .strong()\n"
    "                .color(chrome().text),\n"
    "        );\n"
    "        ui.label(\n"
    "            RichText::new(\n"
    f'                "{MD} Jian Project API（预警 / 速报 / 测站中转）\\n\\\n'
    f'{MD} Wolfx（JMA EEW、台网速报）\\n\\\n'
    f'{MD} P2PQuake（气象厅地震情报）\\n\\\n'
    f'{MD} NIED kmoni / lmoni 原版测站图像（可选）",\n'
    "            )\n"
    "            .size(12.0)\n"
    "            .color(chrome().muted),\n"
    "        );\n"
    "        ui.add_space(10.0);\n"
    "        ui.label(\n"
    '            RichText::new("致谢")\n'
    "                .size(13.0)\n"
    "                .strong()\n"
    "                .color(chrome().text),\n"
    "        );\n"
    "        ui.label(\n"
    "            RichText::new(\n"
    '                "界面布局参考 RhythmQuake（··授权）。地图标记 SVG 与部分交互参考 eewcn（··授权）。\\\n'
    '音效包 srev 采用 CC BY-SA 2.0。底图与走时表等第三方数据按其各自许可使用。",\n'
    "            )\n"
    "            .size(12.0)\n"
    "            .color(chrome().muted),\n"
    "        );\n"
    "        ui.add_space(10.0);\n"
    "        ui.label(\n"
    '            RichText::new("免责声明")\n'
    "                .size(13.0)\n"
    "                .strong()\n"
    "                .color(chrome().text),\n"
    "        );\n"
    "        ui.label(\n"
    "            RichText::new(\n"
    '                "本软件仅供信息展示与学习研究，不构成官方预警渠道。·以各国气象/地震主管部门发布为准；\\\n'
    '紧急情况下·遵从当地防灾指引。开发者不保证数据的实时性、完整性与准确性。",\n'
    "            )\n"
    "            .size(12.0)\n"
    "            .color(chrome().muted),\n"
    "        );\n"
    "    });\n"
    "}\n"
)

HELPERS = r'''fn jma_station_src_label(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "kmoni" => "原版 NIED kmoni".into(),
        "lmoni" => "原版 NIED lmoni".into(),
        _ => "Jian 中转 /kmoni".into(),
    }
}

fn snet_station_src_label(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "original" => "原版 MSIL 切片".into(),
        _ => "Jian 中转 /s-net".into(),
    }
}
'''


def main() -> int:
    warns: list[str] = []
    raw = P.read_bytes()
    text = heal_to_text(raw)

    def sub(pat: str, repl: str) -> None:
        nonlocal text
        # Use callable repl so backslashes like \u{00b7} are literal, not re escapes.
        text2, n = re.subn(pat, lambda _m: repl, text, count=1)
        if not n:
            warns.append(pat[:100])
        else:
            text = text2

    sub(
        r"//! \?+ RhythmQuake `design/settings-page\.html` \+ `SettingsControlStyle`\n//! .+",
        "//! 设置页：按 RhythmQuake `design/settings-page.html` + `SettingsControlStyle`\n"
        "//! 以 egui 重写（统一玻璃面板、·侧分类、扁平行；··授权参考）。",
    )
    sub(
        r'            Self::Sources => "API\?+",\n'
        r'            Self::General => "\?+",\n'
        r'            Self::Audio => "\?+",\n'
        r'            Self::Advanced => "\?+",\n'
        r'            Self::About => "\?+",\n',
        '            Self::Sources => "API与数据源",\n'
        '            Self::General => "通用设置",\n'
        '            Self::Audio => "音频",\n'
        '            Self::Advanced => "高级",\n'
        '            Self::About => "关于",\n',
    )
    sub(
        r'            Self::Sources => "\?+",\n'
        r'            Self::General => "\?+",\n'
        r'            Self::Audio => "\?+",\n'
        r'            Self::Advanced => "\?+",\n'
        r'            Self::About => "\?+",\n',
        '            Self::Sources => "接口开关、鉴权与连接状态",\n'
        '            Self::General => "主题、底图、窗口与定位",\n'
        '            Self::Audio => "音量、静音与音效包",\n'
        '            Self::Advanced => "走时表、缓存与配置·径",\n'
        '            Self::About => "版本、许可与署名",\n',
    )
    sub(r"/// \?+ Material Icons \?+", "/// 导航字形（无 Material Icons 时的占位）")
    sub(r"/// \?+\n    pub map_pick:", "/// 主地图单击选点目标\n    pub map_pick:")
    sub(r"/// \?+\n    pub capture_view:", "/// 下一帧用主地图中心写入默认视口\n    pub capture_view:")
    sub(r"/// \?+\n    pub fn apply_capture_view", "/// 用主地图当前中心写入默认视口\n    pub fn apply_capture_view")
    sub(r"/// \?+ `map_pick`\.?", "/// 主地图单击坐标写入对应 `map_pick`。")
    sub(
        r'self\.save_note = Some\("\?+"\.into\(\)\);\n        \}\n        self\.capture_view = false;',
        'self.save_note = Some("·用当前视野写入默认视口".into());\n        }\n        self.capture_view = false;',
    )
    sub(
        r'MapPickKind::DefaultViewport => \{\n'
        r'                draft\.map\.default_lon = lon;\n'
        r'                draft\.map\.default_lat = lat;\n'
        r'                draft\.map\.default_zoom = zoom;\n'
        r'                self\.save_note = Some\("\?+"\.into\(\)\);',
        "MapPickKind::DefaultViewport => {\n"
        "                draft.map.default_lon = lon;\n"
        "                draft.map.default_lat = lat;\n"
        "                draft.map.default_zoom = zoom;\n"
        '                self.save_note = Some("·用地图选点写入默认视口".into());',
    )
    sub(
        r'MapPickKind::LocalLocation => \{\n'
        r'                draft\.local\.longitude = Some\(lon\);\n'
        r'                draft\.local\.latitude = Some\(lat\);\n'
        r'                self\.save_note = Some\("\?+"\.into\(\)\);',
        "MapPickKind::LocalLocation => {\n"
        "                draft.local.longitude = Some(lon);\n"
        "                draft.local.latitude = Some(lat);\n"
        '                self.save_note = Some("·用地图选点写入本机位置".into());',
    )
    sub(
        r'\.with_title\("EEWView [^"]*"\)',
        f'.with_title("StarQuake {MD} 设置")',
    )
    sub(
        r"// .*RQ .*1180.660 .*",
        "// 对齐 RQ 原型约 1180x660 比例，略收以适应常见桌面",
    )
    sub(r'egui::Window::new\("\?+"\)', 'egui::Window::new("设置")')
    sub(r'\.map\(\|d\| format!\("\?+ \{d\}"\)\)', '.map(|d| format!("，约至 {d}"))')
    sub(r'format!\("\?+\{e\}"\)', 'format!("鉴权失败：{e}")')
    sub(
        r'self\.save_note = Some\("\?+"\.into\(\)\);\n        \}\n\n        if close_requested',
        'self.save_note = Some("·恢复默认（令牌保留）；点「保存全部」写入".into());\n        }\n\n        if close_requested',
    )
    sub(r"// \?+RQ \.header\??", "// 页眉（RQ .header）")
    sub(
        r'RichText::new\("\?+"\)\n                                    \.strong\(\)\n                                    \.size\(22\.0\)',
        'RichText::new("设置中心")\n                                    .strong()\n                                    .size(22.0)',
    )
    sub(
        r'RichText::new\("EEWView [^"]*"\)',
        f'RichText::new("StarQuake {MD} 左侧分类与右侧内容合并为同一玻璃面板")',
    )
    sub(r"// \?+RQ \.panel\??", "// 统一面板（RQ .panel）")
    sub(r"// \?+\n                            let \(line, _\)", "// 竖分割线\n                            let (line, _)")
    sub(r"// \?+EEWView \?+RQ \?+", "// 底栏：EEW View 草稿保存（RQ 即时写入；此处保留显式保存）")
    sub(
        r'glass_btn\(ui, "\?+", false\)\.clicked\(\) \{\n                            \*restore = true;',
        'glass_btn(ui, "恢复默认", false).clicked() {\n                            *restore = true;',
    )
    sub(
        r'RichText::new\("\?+"\)\.size\(12\.0\)\.color\(chrome\(\)\.text70\)',
        'RichText::new("保存后不关闭").size(12.0).color(chrome().text70)',
    )
    sub(
        r'glass_btn\(ui, "\?+", true\)\.clicked\(\) \{\n                                \*action = Some\(SettingsAction::Saved \{\n                                    cfg: draft\.clone\(\),\n                                    close: !session\.keep_open,',
        'glass_btn(ui, "保存全部", true).clicked() {\n                                *action = Some(SettingsAction::Saved {\n                                    cfg: draft.clone(),\n                                    close: !session.keep_open,',
    )
    sub(
        r'glass_btn\(ui, "\?+", false\)\.clicked\(\) \{\n                                \*action = Some\(SettingsAction::Saved \{\n                                    cfg: draft\.clone\(\),\n                                    close: false,',
        'glass_btn(ui, "应用", false).clicked() {\n                                *action = Some(SettingsAction::Saved {\n                                    cfg: draft.clone(),\n                                    close: false,',
    )
    sub(
        r'glass_btn\(ui, "\?+", false\)\.clicked\(\) \{\n                                \*action = Some\(SettingsAction::Cancelled\);',
        'glass_btn(ui, "取消", false).clicked() {\n                                *action = Some(SettingsAction::Cancelled);',
    )
    sub(
        r'egui::Align2::LEFT_CENTER,\n                "\?+",\n                egui::FontId::proportional\(15\.0\),\n                chrome\(\)\.text,',
        'egui::Align2::LEFT_CENTER,\n                "应用设置",\n                egui::FontId::proportional(15.0),\n                chrome().text,',
    )
    sub(r"// \?+RQ \.content-head\??", "// 内容头（RQ .content-head）")
    sub(
        r"/// RQ SettingsControlRow\?+",
        "/// RQ SettingsControlRow：·侧图标盒 + 标题副标题，右侧控件",
    )
    sub(
        r'HealthStatus::Normal => \(chrome\(\)\.status_ok, "\?+"\),\n'
        r'        HealthStatus::Fluctuating => \(chrome\(\)\.status_warn, "\?+"\),\n'
        r'        HealthStatus::Abnormal => \(chrome\(\)\.status_bad, "\?+"\),',
        'HealthStatus::Normal => (chrome().status_ok, "正常"),\n'
        '        HealthStatus::Fluctuating => (chrome().status_warn, "波动"),\n'
        '        HealthStatus::Abnormal => (chrome().status_bad, "异常"),',
    )
    sub(
        r'"vector" => "\?+"\.into\(\),\n'
        r'        "tiles" \| "both" => "\?+"\.into\(\),\n'
        r'        _ => "\?+"\.into\(\),',
        '"vector" => "矢量地图".into(),\n'
        '        "tiles" | "both" => "地形地图".into(),\n'
        '        _ => "·随主题（推荐）".into(),',
    )
    sub(
        r'"records" \| "record" => "\?+"\.into\(\),\n'
        r'        "station" => "\?+"\.into\(\),\n'
        r'        _ => "\?+"\.into\(\),',
        '"records" | "record" => "速报".into(),\n'
        '        "station" => "测站".into(),\n'
        '        _ => "预警".into(),',
    )
    sub(
        r'if s\.eq_ignore_ascii_case\("light"\) \{\n'
        r'        "\?+"\.into\(\)\n'
        r'    \} else \{\n'
        r'        "\?+"\.into\(\)\n'
        r'    \}',
        'if s.eq_ignore_ascii_case("light") {\n'
        '        "亮色".into()\n'
        "    } else {\n"
        '        "暗色".into()\n'
        "    }",
    )
    sub(
        r'"jma" => "\?+"\.into\(\),\n'
        r'        "cn" => "\?+"\.into\(\),\n'
        r'        _ => "\?+"\.into\(\),',
        '"jma" => "气象厅震度".into(),\n'
        '        "cn" => "中国烈度".into(),\n'
        '        _ => "自动".into(),',
    )

    a = text.find("fn draw_sources(")
    start = text.find('    section(ui, "', a)
    end_m = text.find("    filters_ui::draw_eew_and_record_filters(ui, draft);", start)
    if start < 0 or end_m < 0:
        raise SystemExit("draw_sources inner missing")
    end = text.find("\n", end_m) + 1
    text = text[:start] + SOURCES_INNER.lstrip("\n") + text[end:]

    text = replace_fn(text, "draw_audio", AUDIO_BODY)
    text = replace_fn(text, "draw_general", GENERAL_BODY)
    text = replace_fn(text, "draw_advanced", ADVANCED_BODY)

    ja = text.find("fn jma_station_src_label")
    aa = text.find("fn draw_about(")
    if ja < 0 or aa < 0:
        raise SystemExit("helpers/about missing")
    m = re.search(r"\nfn |\Z", text[aa + 1 :])
    end = aa + 1 + (m.start() if m else len(text) - aa)
    text = text[:ja] + HELPERS.rstrip() + "\n\n" + ABOUT_BODY.rstrip() + "\n" + text[end:].lstrip("\n")

    # Never leave raw U+00B7 in file — only \u{00b7}
    text = text.replace("·", MD)
    text = text.replace("×", "x")

    write_utf8(P, text)
    check = P.read_text(encoding="utf-8")
    cjk = sum(1 for c in check if "\u4e00" <= c <= "\u9fff")
    bad = [f"{i}: {l}" for i, l in enumerate(check.splitlines(), 1) if "????" in l]
    report = [
        f"cjk={cjk}",
        f"bad={len(bad)}",
        f"设置中心={'设置中心' in check}",
        f"应用设置={'应用设置' in check}",
        f"免责声明={'免责声明' in check}",
        f"warns={len(warns)}",
    ]
    report.extend(f"WARN {w}" for w in warns[:20])
    report.extend(bad[:20])
    write_utf8(LOG, "\n".join(report) + "\n")
    print(f"ok cjk={cjk} bad={len(bad)} warns={len(warns)}")
    return 0 if cjk >= 800 and not bad else 1


if __name__ == "__main__":
    sys.exit(main())
