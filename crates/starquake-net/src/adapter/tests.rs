//! adapter 跨源集成单测。

use super::jian::{agency_from_list_response, parse_jian_record};
use super::pancakes::strip_jma_volcano_name_label;
use super::{
    ingest_fssn_frame, ingest_jian_frame, ingest_p2p_items, ingest_snet_frame,
    ingest_wolfx_eqlist, ingest_wolfx_frame, KmoniState,
};
use crate::bus::{channel, NetEvent};
use starquake_core::IntensityKind;


    #[test]
    fn fssn_initial_keeps_zh_place_and_estimates_intensity() {
        let body = r#"{
          "type": "initial",
          "Data": {
            "id": "FSSN2026tsch",
            "shockTime": "2026-10-08 14:46:47",
            "latitude": -52.5994,
            "longitude": 140.3181,
            "depth": 10,
            "magnitude": 5.4,
            "placeName": "West of Macquarie Island",
            "placeName_zh": "麦夸里岛以西",
            "infoTypeName": "正式(已核实)"
          }
        }"#;
        let (tx, mut rx) = channel();
        ingest_fssn_frame(&tx, body);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert_eq!(rec.agency.0, "fssn");
        assert!(rec.place.contains("麦夸里岛以西"));
        assert!((rec.magnitude - 5.4).abs() < 1e-6);
        assert!((rec.depth_km - 10.0).abs() < 1e-6);
        // M5.4@10km 震中估：约 7.x → 档位 7~8
        assert!(
            (6..=9).contains(&rec.intensity_level),
            "expected estimated CSIS level, got {}",
            rec.intensity_level
        );
        assert_ne!(rec.intensity_text, "—");
        assert!(rec.origin_ms > 0);
    }

    #[test]
    fn fssn_deep_quake_shows_low_estimated_intensity() {
        let body = r#"{
          "type": "update",
          "Data": {
            "shockTime": "2026-10-25 10:36:30",
            "latitude": 27.0,
            "longitude": 142.0,
            "depth": 515,
            "magnitude": 4.9,
            "placeName_zh": "日本小笠原群岛地区"
          }
        }"#;
        let (tx, mut rx) = channel();
        ingest_fssn_frame(&tx, body);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert_eq!(rec.intensity_level, 0);
        assert!(
            rec.intensity_text.contains('.'),
            "deep M4.9@515km should show decimal estimate, got {}",
            rec.intensity_text
        );
        assert_ne!(rec.intensity_text, "—");
    }

    #[test]
    fn fssn_skips_when_depth_missing() {
        let body = r#"{
          "type": "update",
          "Data": {
            "shockTime": "2026-10-08 14:46:47",
            "latitude": 1.0,
            "longitude": 2.0,
            "magnitude": 4.0,
            "placeName_zh": "测试"
          }
        }"#;
        let (tx, mut rx) = channel();
        ingest_fssn_frame(&tx, body);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn strip_jma_volcano_name_label_removes_field_prefix() {
        assert_eq!(
            strip_jma_volcano_name_label("\u{706b}\u{5c71}\u{540d}\u{3000}\u{5341}\u{52dd}\u{5cb3}\u{3000}\u{964d}\u{7070}\u{4e88}\u{5831}\u{ff08}\u{5b9a}\u{65f6}\u{ff09}"),
            "\u{5341}\u{52dd}\u{5cb3}\u{3000}\u{964d}\u{7070}\u{4e88}\u{5831}\u{ff08}\u{5b9a}\u{65f6}\u{ff09}"
        );
        assert_eq!(
            strip_jma_volcano_name_label("\u{685c}\u{5cf6}\u{3000}\u{63a8}\u{5b9a}\u{5674}\u{7159}\u{6d41}\u{5411}\u{5831}"),
            "\u{685c}\u{5cf6}\u{3000}\u{63a8}\u{5b9a}\u{5674}\u{7159}\u{6d41}\u{5411}\u{5831}"
        );
    }

    #[test]
    fn list_response_agency_keeps_early_est() {
        assert_eq!(agency_from_list_response("early-estlist_response"), "early-est");
        assert_eq!(agency_from_list_response("jma-eewlist_response"), "jma-eew");
        assert_eq!(agency_from_list_response("cea-prlist_response"), "cea-pr");
        assert_eq!(agency_from_list_response("cenclist_response"), "cenc");
        assert_eq!(agency_from_list_response("jma_eew_list_response"), "jma_eew");
    }

    #[test]
    fn jian_record_keeps_western_southern_coords() {
        // 负经纬合法；旧逻辑把 Number<0 当调查中，导致无法跳转且地名修正失效
        let data = serde_json::json!({
            "placeName": "Offshore Chile",
            "magnitude": 5.0,
            "depth": 10.0,
            "latitude": -30.5,
            "longitude": -72.0,
            "originTime": "2026-10-14 20:36:00"
        });
        let rec = parse_jian_record("ga", &data).expect("record");
        assert!(!rec.hypocenter_unknown);
        assert!(rec.has_map_coord());
        assert!((rec.latitude - (-30.5)).abs() < 1e-9);
        assert!((rec.longitude - (-72.0)).abs() < 1e-9);
    }

    #[test]
    fn wolfx_cenc_parses_string_numbers() {
        let body = r#"{
            "No1": {
                "type": "reviewed",
                "EventID": "CC.test",
                "time": "2026-10-10 12:14:01",
                "location": "巴拿马",
                "placeName": "巴拿马",
                "magnitude": "6.0",
                "depth": "10",
                "latitude": "7.50",
                "longitude": "-80.60",
                "intensity": "8"
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_wolfx_eqlist(&tx, body);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert!((rec.magnitude - 6.0).abs() < 1e-9);
        assert!((rec.latitude - 7.5).abs() < 1e-9);
        assert!((rec.longitude - (-80.6)).abs() < 1e-9);
        assert!((rec.depth_km - 10.0).abs() < 1e-9);
        assert_eq!(rec.intensity_level, 8);
        assert_eq!(rec.place, "巴拿马");
        assert!(!rec.is_auto);
        assert!(rec.origin_ms > 0, "CENC time must parse to origin_ms");
        // 2026-10-10 12:14:01 CST
        assert_eq!(rec.origin_ms, 1_791_605_641_000);
    }

    #[test]
    fn snet_accepts_data_skips_time_sync_repush() {
        // 首包 stations；资料帧上送一次；同 time+int 仅换 timestamp 的对时帧不上送
        let mut state = KmoniState::default();
        let (tx, mut rx) = channel();
        ingest_snet_frame(
            &mut state,
            &tx,
            r#"{"type":"s-net","timestamp":1,"stations":[{"code":"A","name":"a","lat":35.0,"lon":139.0},{"code":"B","name":"b","lat":36.0,"lon":140.0}],"source":"s-net"}"#,
        );
        assert!(rx.try_recv().is_err(), "meta-only must not emit Stations");
        ingest_snet_frame(
            &mut state,
            &tx,
            r#"{"type":"s-net","time":"20261014084800","timestamp":1791967763000,"int":[-1.3,-4.0],"source":"s-net"}"#,
        );
        let NetEvent::Stations { network, list } = rx.try_recv().expect("stations") else {
            panic!("expected Stations");
        };
        assert_eq!(network, "snet");
        // -1.3 有感档；-4.0 缺测跳过
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "A");
        assert!((list[0].instrumental - (-1.3)).abs() < 1e-9);

        // 对时重推：timestamp 变了，time/int 不变 → 只刷时钟，不上送 Stations
        ingest_snet_frame(
            &mut state,
            &tx,
            r#"{"type":"s-net","time":"20261014084800","timestamp":1791967764000,"int":[-1.3,-4.0],"source":"s-net"}"#,
        );
        match rx.try_recv().expect("time-sync clock") {
            NetEvent::StationClock {
                network,
                ts_ms: Some(ts),
            } => {
                assert_eq!(network, "snet");
                assert_eq!(ts, 1_791_967_764_000);
            }
            other => panic!("expected StationClock, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "time-sync must not emit Stations");

        // 同分钟、int 变了（中转资料更新）→ 再上送新震度
        ingest_snet_frame(
            &mut state,
            &tx,
            r#"{"type":"s-net","time":"20261014084800","timestamp":1791967765000,"int":[2.4,-4.0],"source":"s-net"}"#,
        );
        let NetEvent::Stations { list, .. } = rx.try_recv().expect("int update") else {
            panic!("expected Stations");
        };
        assert_eq!(list.len(), 1);
        assert!((list[0].instrumental - 2.4).abs() < 1e-9);

        // 分钟变了也上送
        ingest_snet_frame(
            &mut state,
            &tx,
            r#"{"type":"s-net","time":"20261014084900","timestamp":1791967820000,"int":[-0.5,-4.0],"source":"s-net"}"#,
        );
        let NetEvent::Stations { list, .. } = rx.try_recv().expect("data update") else {
            panic!("expected Stations");
        };
        assert_eq!(list.len(), 1);
        assert!((list[0].instrumental - (-0.5)).abs() < 1e-9);
    }

    #[test]
    fn snet_relay_intensity_replaces_snapshot() {
        // 中转：资料帧写入快照；对时帧不改震度；下一帧新 int 覆盖旧值
        let mut state = KmoniState::default();
        let (tx, mut rx) = channel();
        let mut snap = starquake_core::AppSnapshot::empty();
        let frames = [
            r#"{"type":"s-net","stations":[{"code":"A","name":"a","lat":35.0,"lon":139.0}]}"#,
            r#"{"type":"s-net","time":"20261014084800","timestamp":1791967763000,"int":[-1.3]}"#,
            r#"{"type":"s-net","time":"20261014084800","timestamp":1791967764000,"int":[-1.3]}"#,
            r#"{"type":"s-net","time":"20261014084800","timestamp":1791967765000,"int":[1.6]}"#,
        ];
        for text in frames {
            ingest_snet_frame(&mut state, &tx, text);
            if let Ok(ev) = rx.try_recv() {
                crate::apply_event(&mut snap, ev);
            }
        }
        let s = snap
            .stations
            .iter()
            .find(|s| s.id == "snet:A")
            .expect("station A");
        assert!((s.instrumental - 1.6).abs() < 1e-9);
        assert!(snap.station_updated_ms.get("snet").copied().unwrap_or(0) > 0);
    }

    #[test]
    fn wolfx_cenc_skips_when_depth_missing() {
        let body = r#"{
            "No1": {
                "type": "reviewed",
                "time": "2026-10-10 12:14:01",
                "placeName": "巴拿马",
                "magnitude": "6.0",
                "latitude": "7.50",
                "longitude": "-80.60",
                "intensity": "8"
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_wolfx_eqlist(&tx, body);
        assert!(rx.try_recv().is_err(), "missing depth must not invent 10km");
    }

    #[test]
    fn wolfx_cenc_automatic_flag() {
        let body = r#"{
            "No1": {
                "type": "automatic",
                "time": "2026-10-10 12:14:01",
                "placeName": "四川",
                "magnitude": "3.2",
                "depth": "10",
                "latitude": "30.0",
                "longitude": "103.0",
                "intensity": "3"
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_wolfx_eqlist(&tx, body);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert!(rec.is_auto);
    }

    #[test]
    fn wolfx_all_dispatches_sc_eew() {
        let body = r#"{
            "type": "sc_eew",
            "ID": 1,
            "EventID": "SC202610110001",
            "ReportTime": "2026-10-11 12:00:00",
            "ReportNum": 2,
            "OriginTime": "2026-10-11 11:59:30",
            "HypoCenter": "四川阿坝州红原县",
            "Latitude": 32.8,
            "Longitude": 102.6,
            "Magnitude": 4.5,
            "Depth": 12.0,
            "MaxIntensity": 5.0
        }"#;
        let (tx, mut rx) = channel();
        ingest_wolfx_frame(&tx, body, &starquake_config::WolfxSourceConfig {
            enabled: true,
            eew_enabled: true,
            eqlist_enabled: true,
            jma_eew: true,
            sc_eew: true,
            fj_eew: true,
            cq_eew: true,
            cenc_eew: true,
            cwa_eew: true,
            cenc_eqlist: true,
            jma_eqlist: true,
            ws_urls: vec![],
            http_eqlist: String::new(),
        });
        let NetEvent::Eew(e) = rx.try_recv().expect("eew") else {
            panic!("expected Eew");
        };
        assert_eq!(e.agency.0, "wolfx-cea-pr:四川");
        assert_eq!(e.event_id, "SC202610110001");
        assert_eq!(e.serial, 2);
        assert!((e.magnitude - 4.5).abs() < 1e-9);
        assert_eq!(e.intensity_kind, IntensityKind::CnIntensity);
    }

    #[test]
    fn wolfx_all_dispatches_jma_eqlist() {
        let body = r#"{
            "type": "jma_eqlist",
            "No1": {
                "time": "2026/10/11 12:00:00",
                "location": "千葉県東方沖",
                "magnitude": "4.2",
                "shindo": "3",
                "depth": "40km",
                "latitude": "35.5",
                "longitude": "140.5"
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_wolfx_frame(&tx, body, &starquake_config::WolfxSourceConfig {
            enabled: true,
            eew_enabled: true,
            eqlist_enabled: true,
            jma_eew: true,
            sc_eew: true,
            fj_eew: true,
            cq_eew: true,
            cenc_eew: true,
            cwa_eew: true,
            cenc_eqlist: true,
            jma_eqlist: true,
            ws_urls: vec![],
            http_eqlist: String::new(),
        });
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert_eq!(rec.agency.0, "wolfx-jma");
        assert!((rec.magnitude - 4.2).abs() < 1e-9);
        assert_eq!(rec.intensity_kind, IntensityKind::JmaShindo);
        assert!(rec.origin_ms > 0);
    }

    #[test]
    fn wolfx_eew_toggle_skips_when_disabled() {
        let body = r#"{
            "type": "jma_eew",
            "EventID": "20261011201300",
            "Serial": 1,
            "OriginTime": "2026/10/11 20:13:00",
            "Hypocenter": "未知",
            "Latitude": 35.0,
            "Longitude": 139.0,
            "Magnitude": 5.0,
            "Depth": 10.0,
            "MaxIntensity": "3"
        }"#;
        let (tx, mut rx) = channel();
        ingest_wolfx_frame(&tx, body, &starquake_config::WolfxSourceConfig {
            enabled: true,
            eew_enabled: true,
            eqlist_enabled: true,
            jma_eew: false,
            sc_eew: true,
            fj_eew: true,
            cq_eew: true,
            cenc_eew: true,
            cwa_eew: true,
            cenc_eqlist: true,
            jma_eqlist: true,
            ws_urls: vec![],
            http_eqlist: String::new(),
        });
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn jian_eew_skips_training_and_maps_plum() {
        let body = r#"{
            "type": "jma-eew",
            "Data": {
                "id": "202610120001",
                "originTime": "2026-10-12T08:00:00+09:00",
                "latitude": 35.0,
                "longitude": 139.0,
                "depth": 10.0,
                "magnitude": 4.0,
                "placeName": "東京湾",
                "infoTypeName": "予報",
                "intensity": "3",
                "serial": 1,
                "isWarn": false,
                "isFinal": false,
                "isCancel": false,
                "isTraining": true,
                "isPLUM": false,
                "warnArea": []
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_jian_frame(&tx, body, None);
        assert!(rx.try_recv().is_err());

        let body2 = body
            .replace("\"isTraining\": true", "\"isTraining\": false")
            .replace("\"isPLUM\": false", "\"isPLUM\": true");
        ingest_jian_frame(&tx, &body2, None);
        let NetEvent::Eew(e) = rx.try_recv().expect("eew") else {
            panic!("expected Eew");
        };
        assert!(e.is_assumption);
        assert_eq!(e.event_id, "202610120001");
    }

    #[test]
    fn jian_record_investigating_keeps_intensity() {
        let body = r#"{
            "type": "jma",
            "Data": {
                "id": "20261014120000",
                "originTime": "2026-10-14T12:00:00+09:00",
                "latitude": 0,
                "longitude": 0,
                "depth": 0,
                "magnitude": 0,
                "placeName": "調査中",
                "intensity": "4",
                "infoType": "発表",
                "hypocenterUnknown": true,
                "magnitudeUnknown": true
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_jian_frame(&tx, body, None);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert!(rec.magnitude_unknown);
        assert!(rec.hypocenter_unknown);
        assert!(!rec.has_map_coord());
        assert_eq!(rec.intensity_text, "4");
        assert_eq!(rec.intensity_level, 4);
        assert_eq!(rec.place, "调查中");
        assert_eq!(rec.magnitude_label(), "调查中");
    }

    #[test]
    fn jma_depth_jishen_string_keeps_zero_and_label() {
        let body = r#"{
            "type": "jma",
            "Data": {
                "id": "20261015080000",
                "originTime": "2026-10-15T08:00:00+09:00",
                "latitude": 35.0,
                "longitude": 139.0,
                "depth": "极浅",
                "magnitude": 3.2,
                "placeName": "横浜湾",
                "intensity": "2",
                "infoType": "発表"
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_jian_frame(&tx, body, None);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert!((rec.depth_km - 0.0).abs() < 1e-9);
        assert_eq!(rec.depth_label(), "极浅");
        assert!(!rec.hypocenter_unknown);
    }

    #[test]
    fn jma_depth_goku_asai_japanese_normalizes_to_jishen() {
        let body = r#"{
            "type": "jma",
            "Data": {
                "id": "20261015080100",
                "originTime": "2026-10-15T08:01:00+09:00",
                "latitude": 35.0,
                "longitude": 139.0,
                "depth": "ごく浅い",
                "magnitude": 2.8,
                "placeName": "横浜湾",
                "intensity": "1",
                "infoType": "発表"
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_jian_frame(&tx, body, None);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert!((rec.depth_km - 0.0).abs() < 1e-9);
        assert_eq!(rec.depth_label(), "极浅");
        assert!(!rec.depth_label().contains("ごく"));
    }

    #[test]
    fn p2p_depth_zero_is_jishen_not_unknown() {
        let body = r#"[{
            "code": 551,
            "earthquake": {
                "time": "2026/10/15 08:00:00",
                "maxScale": 20,
                "hypocenter": {
                    "name": "熊本県熊本地方",
                    "latitude": 32.5,
                    "longitude": 130.6,
                    "depth": 0,
                    "magnitude": 2.6
                }
            }
        }]"#;
        let (tx, mut rx) = channel();
        ingest_p2p_items(&tx, body);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert!(!rec.hypocenter_unknown);
        assert!((rec.depth_km - 0.0).abs() < 1e-9);
        assert_eq!(rec.depth_label(), "极浅");
    }

    #[test]
    fn p2p_scale_prompt_investigating_hypocenter() {
        let body = r#"[{
            "code": 551,
            "earthquake": {
                "time": "2026/10/14 12:00:00",
                "maxScale": 40,
                "hypocenter": {
                    "name": "",
                    "latitude": -200,
                    "longitude": -200,
                    "depth": -1,
                    "magnitude": -1
                }
            }
        }]"#;
        let (tx, mut rx) = channel();
        ingest_p2p_items(&tx, body);
        let NetEvent::Record(rec) = rx.try_recv().expect("record") else {
            panic!("expected Record");
        };
        assert!(rec.hypocenter_unknown);
        assert!(rec.magnitude_unknown);
        assert_eq!(rec.intensity_text, "4");
        assert_eq!(rec.place, "调查中");
    }

    #[test]
    fn jian_tsunami_parses_area_array_and_cancel() {
        let body = r#"{
            "type": "jma-tsunami",
            "Data": {
                "id": "20261010025608",
                "title": "津波警報",
                "level": "津波警報",
                "headline": "",
                "number": 1,
                "originTime": 1791574620000,
                "infoTypeName": "発表",
                "cancel": false,
                "cleared": false,
                "orgUnit": "日本気象庁",
                "telegram": "VTSE41",
                "area": [
                    {"code": "100", "name": "北海道太平洋沿岸東部", "kind": "津波警報", "kindCode": "52", "cleared": false},
                    {"code": "101", "name": "青森県太平洋沿岸", "kind": "津波なし", "kindCode": "00", "cleared": true}
                ]
            }
        }"#;
        let (tx, mut rx) = channel();
        ingest_jian_frame(&tx, body, None);
        let NetEvent::Tsunami(tsu) = rx.try_recv().expect("tsu") else {
            panic!("expected Tsunami");
        };
        assert!(!tsu.cancelled);
        assert_eq!(tsu.grade, "Warning");
        assert_eq!(tsu.areas_list.len(), 1);
        assert_eq!(tsu.areas_list[0].name, "北海道太平洋沿岸東部");

        let cancel = body
            .replace("\"cancel\": false", "\"cancel\": true")
            .replace("\"infoTypeName\": \"発表\"", "\"infoTypeName\": \"取消\"");
        ingest_jian_frame(&tx, &cancel, None);
        let NetEvent::Tsunami(tsu2) = rx.try_recv().expect("tsu2") else {
            panic!("expected Tsunami");
        };
        assert!(tsu2.cancelled);
    }
