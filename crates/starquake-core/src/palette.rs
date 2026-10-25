//! 官方震度/烈度色。
//!
//! 中国烈度色对齐 EEWCN（`getIntColors`）；离散 JMA 震度色与 `icon/intensity/shindo` SVG 主色一致。
//! 测站計測色带仍对齐 EEWCN `getNiedStationColor`（仅测站点，不用于列表/图例右列）。

use crate::IntensityKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_egui(self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }
}

/// JMA 震度 0–9：0,1,2,3,4,5弱,5强,6弱,6强,7（与 shindo SVG 主色一致）
pub fn jma_shindo_rgb(level: u8) -> Rgb {
    match level.min(9) {
        0 => Rgb::new(0x86, 0x86, 0x86),
        1 => Rgb::new(0x66, 0x66, 0x66),
        2 => Rgb::new(0x1F, 0x7B, 0xFE),
        3 => Rgb::new(0x2F, 0xB5, 0x6C),
        4 => Rgb::new(0xFE, 0xB8, 0x01),
        5 => Rgb::new(0xFD, 0xAA, 0x11), // 5弱
        6 => Rgb::new(0xF0, 0x6F, 0x14), // 5强
        7 => Rgb::new(0xFD, 0x00, 0x00), // 6弱
        8 => Rgb::new(0x9F, 0x00, 0x02), // 6强
        _ => Rgb::new(0x5D, 0x00, 0x8D), // 7
    }
}

/// 中国烈度 1–12（对齐 EEWCN `getIntColors`）
pub fn cn_intensity_rgb(level: u8) -> Rgb {
    if level == 0 {
        return Rgb::new(64, 64, 64);
    }
    match level.clamp(1, 12) {
        1 => Rgb::new(0x9b, 0xc4, 0xe4),
        2 => Rgb::new(0x00, 0xa0, 0xf1),
        3 => Rgb::new(0x00, 0x62, 0xf5),
        4 => Rgb::new(0x46, 0xbc, 0x67),
        5 => Rgb::new(0x12, 0x99, 0x4e),
        6 => Rgb::new(0xf6, 0xb7, 0x2d),
        7 => Rgb::new(0xff, 0x84, 0x00),
        8 => Rgb::new(0xfa, 0x51, 0x51),
        9 => Rgb::new(0xf4, 0x44, 0x0d),
        10 => Rgb::new(0xff, 0x00, 0x0d),
        11 => Rgb::new(0xc2, 0x00, 0x07),
        _ => Rgb::new(0xfd, 0x2f, 0xc2),
    }
}

pub fn color_for(kind: IntensityKind, level: u8) -> Rgb {
    match kind {
        IntensityKind::JmaShindo => jma_shindo_rgb(level),
        IntensityKind::CnIntensity => cn_intensity_rgb(level),
    }
}

/// EEWCN / NIED 計測震度色带（level 0–20，对应约 -3～7）。
/// 对齐 `MapStationLogic.js` `getNiedStationColor`。
const INSTRUMENTAL_BAND: [Rgb; 21] = [
    Rgb::new(0x97, 0xb7, 0xcc),
    Rgb::new(0x90, 0xb3, 0xca),
    Rgb::new(0x89, 0xaf, 0xc8),
    Rgb::new(0x71, 0xa2, 0xcb),
    Rgb::new(0x5e, 0xa7, 0xac),
    Rgb::new(0x38, 0xa4, 0x77),
    Rgb::new(0x0f, 0xb0, 0x2b),
    Rgb::new(0xf4, 0xe2, 0x00),
    Rgb::new(0xfb, 0xc3, 0x00),
    Rgb::new(0xff, 0xaf, 0x00),
    Rgb::new(0xff, 0x99, 0x00),
    Rgb::new(0xff, 0x7e, 0x00),
    Rgb::new(0xff, 0x62, 0x00),
    Rgb::new(0xfc, 0x4c, 0x02),
    Rgb::new(0xf5, 0x36, 0x05),
    Rgb::new(0xf1, 0x15, 0x20),
    Rgb::new(0xed, 0x00, 0x47),
    Rgb::new(0xe3, 0x00, 0x71),
    Rgb::new(0xdc, 0x00, 0x9c),
    Rgb::new(0xc9, 0x00, 0xba),
    Rgb::new(0xb6, 0x00, 0xd7),
];

/// 計測震度 → 色带档位 0–20（对齐要石 `getLevelFromInstShindo`）。
/// `< -3` 返回 `None`（缺测）。
pub fn instrumental_band_level(v: f64) -> Option<u8> {
    if !v.is_finite() || v < -3.0 {
        None
    } else if (v + 3.0).abs() < 1e-9 {
        Some(0)
    } else if v >= 6.5 {
        Some(20)
    } else {
        Some((v * 2.0 + 7.0).floor().clamp(0.0, 20.0) as u8)
    }
}

/// 計測震度上色；缺测时返回中性灰。
pub fn instrumental_rgb(v: f64) -> Rgb {
    match instrumental_band_level(v) {
        Some(lv) => INSTRUMENTAL_BAND[lv as usize],
        None => Rgb::new(0xcf, 0xcf, 0xcf),
    }
}

/// 图例等离散取样：给定色带档位取色。
pub fn instrumental_band_rgb(level: u8) -> Rgb {
    INSTRUMENTAL_BAND[level.min(20) as usize]
}
