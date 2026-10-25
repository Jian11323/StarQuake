//! CPU / GPU 天梯识别。分数是相对档位（约 0–100），用来选性能模式，不是跑分。

/// 天梯命中结果。
#[derive(Debug, Clone)]
pub struct TierHit {
    pub score: f64,
    pub label: String,
}

/// 识别 CPU 营销名。
pub fn classify_cpu(name: &str) -> TierHit {
    let n = name.to_ascii_lowercase();
    if let Some(hit) = apple(&n) {
        return hit;
    }
    if let Some(hit) = intel_core_ultra(&n) {
        return hit;
    }
    if let Some(hit) = intel_core(&n) {
        return hit;
    }
    if let Some(hit) = intel_low_power(&n) {
        return hit;
    }
    if let Some(hit) = amd_ryzen(&n) {
        return hit;
    }
    if let Some(hit) = amd_legacy(&n) {
        return hit;
    }
    TierHit {
        score: 36.0,
        label: "未识别 CPU（按入门估）".into(),
    }
}

/// 识别显卡名（DXGI / 驱动字符串）。
pub fn classify_gpu(name: &str) -> TierHit {
    let n = name.to_ascii_lowercase();
    if n.contains("basic render") || n.contains("microsoft basic") {
        return TierHit {
            score: 8.0,
            label: "基本显示驱动".into(),
        };
    }
    if let Some(hit) = intel_gpu(&n) {
        return hit;
    }
    if let Some(hit) = nvidia_gpu(&n) {
        return hit;
    }
    if let Some(hit) = amd_gpu(&n) {
        return hit;
    }
    TierHit {
        score: 30.0,
        label: "未识别显卡（按入门核显估）".into(),
    }
}

fn apple(n: &str) -> Option<TierHit> {
    let (score, label) = if n.contains("m4 max") || n.contains("m3 max") {
        (96.0, "Apple Max")
    } else if n.contains("m4 pro") || n.contains("m3 pro") || n.contains("m2 max") {
        (90.0, "Apple Pro / M2 Max")
    } else if n.contains("m4") || n.contains("m3") {
        (84.0, "Apple M3/M4")
    } else if n.contains("m2") || n.contains("m1") {
        (76.0, "Apple M1/M2")
    } else {
        return None;
    };
    Some(TierHit {
        score,
        label: label.into(),
    })
}

fn intel_core_ultra(n: &str) -> Option<TierHit> {
    if !n.contains("ultra") {
        return None;
    }
    let rank = if n.contains("ultra 9") {
        9
    } else if n.contains("ultra 7") {
        7
    } else {
        5
    };
    let series2 = n.contains("2") && (n.contains("ultra 5 2") || n.contains("ultra 7 2") || n.contains("ultra 9 2") || n.contains("series 2"));
    let base = match rank {
        9 => 92.0_f64,
        7 => 86.0,
        _ => 78.0,
    } + if series2 { 4.0 } else { 0.0 };
    Some(TierHit {
        score: base.clamp(10.0, 100.0),
        label: format!("Intel Core Ultra {rank}"),
    })
}

fn intel_core(n: &str) -> Option<TierHit> {
    let (rank, model, suffix) = intel_sku(n)?;
    let gen = intel_gen(model);
    let i5_desktop = match gen {
        0 | 1 => 28.0,
        2 => 34.0,
        3 => 38.0,
        4 => 44.0,
        5 => 48.0,
        6 => 52.0,
        7 => 56.0,
        8 => 64.0,
        9 => 70.0,
        10 => 74.0,
        11 => 78.0,
        12 => 86.0,
        13 => 92.0,
        14 => 95.0,
        _ => 96.0,
    };
    let power = intel_power_scale(gen, &suffix);
    let rank_delta = match rank {
        3 => -12.0,
        7 => 6.0,
        9 => 12.0,
        _ => 0.0,
    };
    let score = (i5_desktop * power + rank_delta).clamp(12.0, 100.0);
    let kind = if suffix_is_u(&suffix) {
        "低压"
    } else if suffix_is_h(&suffix) {
        "标压"
    } else {
        "桌面"
    };
    Some(TierHit {
        score,
        label: format!("Intel {gen} 代{kind} i{rank}"),
    })
}

fn intel_sku(n: &str) -> Option<(u8, u32, String)> {
    for rank in [9u8, 7, 5, 3] {
        let key = format!("i{rank}-");
        let Some(idx) = n.find(&key) else {
            continue;
        };
        let rest = &n[idx + key.len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() < 3 {
            continue;
        }
        let model: u32 = digits.parse().ok()?;
        let suffix: String = rest[digits.len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        return Some((rank, model, suffix));
    }
    None
}

fn intel_gen(model: u32) -> u32 {
    let s = model.to_string();
    if s.len() >= 5 {
        s[..2].parse().unwrap_or(1)
    } else if s.len() == 4 {
        let two: u32 = s[..2].parse().unwrap_or(0);
        if (10..=19).contains(&two) {
            two
        } else {
            s[..1].parse().unwrap_or(1)
        }
    } else {
        1
    }
}

fn suffix_is_u(suffix: &str) -> bool {
    let s = suffix.to_ascii_lowercase();
    s.contains('u') || s.contains('y') || (s.contains('g') && !s.contains('h'))
}

fn suffix_is_h(suffix: &str) -> bool {
    let s = suffix.to_ascii_lowercase();
    s.contains('h')
}

fn intel_power_scale(gen: u32, suffix: &str) -> f64 {
    if suffix_is_u(suffix) {
        if gen <= 7 {
            0.72
        } else {
            0.84
        }
    } else if suffix_is_h(suffix) {
        0.94
    } else {
        1.0
    }
}

fn intel_low_power(n: &str) -> Option<TierHit> {
    let (score, label) = if n.contains("n100") || n.contains("n95") || n.contains("n97") || n.contains("n200")
    {
        (42.0, "Intel N100/N200")
    } else if n.contains("pentium silver") || n.contains("celeron n") || n.contains("n4000") || n.contains("n4100")
    {
        (22.0, "Pentium Silver / 赛扬 N")
    } else if n.contains("pentium gold") {
        (36.0, "Pentium Gold")
    } else if n.contains("celeron") || n.contains("pentium") {
        (24.0, "Pentium / 赛扬")
    } else if n.contains("atom") {
        (14.0, "Atom")
    } else if n.contains("xeon") {
        (70.0, "Xeon")
    } else {
        return None;
    };
    Some(TierHit {
        score,
        label: label.into(),
    })
}

fn amd_ryzen(n: &str) -> Option<TierHit> {
    if !n.contains("ryzen") {
        return None;
    }
    let rank = if n.contains("ryzen 9") {
        9
    } else if n.contains("ryzen 7") {
        7
    } else if n.contains("ryzen 3") || n.contains("ryzen 3 ") {
        3
    } else {
        5
    };
    let model = trailing_model(n)?;
    let series = if model >= 1000 { model / 1000 } else { model / 100 };
    let base = match series {
        1 => 60.0_f64,
        2 => 66.0,
        3 => 74.0,
        4 => 76.0,
        5 => 84.0,
        6 => 82.0,
        7 => 90.0,
        8 => 88.0,
        9 => 96.0,
        _ => 70.0,
    };
    let power = if n.contains("u") && !n.contains("hx") {
        0.82
    } else if n.contains("h") {
        0.96
    } else {
        1.0
    };
    let rank_delta = match rank {
        3 => -10.0,
        7 => 6.0,
        9 => 12.0,
        _ => 0.0,
    };
    Some(TierHit {
        score: (base * power + rank_delta).clamp(20.0, 100.0),
        label: format!("Ryzen {rank} {series}000"),
    })
}

fn amd_legacy(n: &str) -> Option<TierHit> {
    let (score, label) = if n.contains("threadripper") {
        (90.0, "Threadripper")
    } else if n.contains("fx-") || n.contains("fx ") {
        (32.0, "AMD FX")
    } else if n.contains("athlon") {
        (28.0, "Athlon")
    } else if n.contains("phenom") {
        (24.0, "Phenom")
    } else if n.contains("a12") || n.contains("a10") || n.contains("a8") || n.contains("a6") || n.contains("a4")
    {
        (22.0, "AMD A 系列")
    } else {
        return None;
    };
    Some(TierHit {
        score,
        label: label.into(),
    })
}

fn intel_gpu(n: &str) -> Option<TierHit> {
    if !(n.contains("intel") || n.contains("hd graphics") || n.contains("uhd") || n.contains("iris") || n.contains("arc "))
    {
        return None;
    }
    if n.contains("arc") {
        let score = if n.contains("b580") || n.contains("b570") {
            86.0
        } else if n.contains("a770") || n.contains("a750") {
            78.0
        } else if n.contains("a580") || n.contains("a380") || n.contains("a310") {
            68.0
        } else {
            72.0
        };
        return Some(TierHit {
            score,
            label: "Intel Arc".into(),
        });
    }
    if n.contains("iris xe") || n.contains("iris(r) xe") {
        return Some(TierHit {
            score: 64.0,
            label: "Iris Xe".into(),
        });
    }
    if n.contains("iris plus") || n.contains("iris") {
        return Some(TierHit {
            score: 54.0,
            label: "Iris".into(),
        });
    }
    let num = gpu_model_number(n);
    if n.contains("uhd") {
        let score = match num {
            Some(x) if x >= 770 => 58.0,
            Some(x) if x >= 730 => 54.0,
            Some(x) if x >= 630 => 50.0,
            Some(x) if x >= 620 => 48.0,
            Some(x) if x >= 600 => 46.0,
            _ => 48.0,
        };
        return Some(TierHit {
            score,
            label: match num {
                Some(x) => format!("UHD Graphics {x}"),
                None => "UHD Graphics".into(),
            },
        });
    }
    if n.contains("hd graphics") || n.contains("hd graphics") || n.contains("hd ") {
        let score = match num {
            Some(x) if x <= 3000 => 16.0,
            Some(4000) => 24.0,
            Some(x) if (4200..5000).contains(&x) => 30.0,
            Some(x) if (5000..5300).contains(&x) => 34.0,
            Some(x) if (5300..7000).contains(&x) => 38.0,
            Some(x) if (500..600).contains(&x) => 42.0,
            Some(x) if (600..700).contains(&x) => 46.0,
            Some(_) => 36.0,
            None => 28.0,
        };
        return Some(TierHit {
            score,
            label: match num {
                Some(x) => format!("HD Graphics {x}"),
                None => "HD Graphics".into(),
            },
        });
    }
    None
}

fn nvidia_gpu(n: &str) -> Option<TierHit> {
    if !(n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") || n.contains("gtx") || n.contains("quadro"))
    {
        return None;
    }
    if n.contains("quadro") || n.contains("rtx a") {
        return Some(TierHit {
            score: 70.0,
            label: "NVIDIA 专业卡".into(),
        });
    }
    let kind = if n.contains("rtx") {
        "rtx"
    } else if n.contains("gtx") {
        "gtx"
    } else if n.contains("mx") {
        "mx"
    } else if n.contains("gt") {
        "gt"
    } else {
        return Some(TierHit {
            score: 40.0,
            label: "NVIDIA".into(),
        });
    };
    let model = first_number_after(n, kind).unwrap_or(0);
    let ti = n.contains("ti") || n.contains("super");
    let score = nvidia_score(kind, model, ti);
    Some(TierHit {
        score,
        label: format!(
            "NVIDIA {} {model}{}",
            kind.to_ascii_uppercase(),
            if ti { " Ti/Super" } else { "" }
        ),
    })
}

fn nvidia_score(kind: &str, model: u32, ti: bool) -> f64 {
    if kind == "gt" && model >= 1030 {
        return 38.0;
    }
    let (floor, ceil) = match kind {
        "gt" => (16.0, 30.0),
        "mx" => (40.0, 58.0),
        "gtx" => match model {
            400..=599 => (30.0, 44.0),
            600..=799 => (36.0, 58.0),
            900..=999 => (44.0, 70.0),
            1000..=1099 => (52.0, 80.0),
            1600..=1699 => (60.0, 76.0),
            _ => (40.0, 64.0),
        },
        _ => match model {
            2000..=2099 => (66.0, 92.0),
            3000..=3099 => (72.0, 98.0),
            4000..=4099 => (78.0, 100.0),
            5000..=5099 => (84.0, 100.0),
            _ => (70.0, 92.0),
        },
    };
    let class = (model % 100) as f64;
    let t = ((class - 30.0) / 60.0).clamp(0.0, 1.0);
    (floor + (ceil - floor) * t + if ti { 3.0 } else { 0.0 }).clamp(10.0, 100.0)
}

fn amd_gpu(n: &str) -> Option<TierHit> {
    if !(n.contains("radeon") || n.contains("amd") || n.contains("rx ")) {
        return None;
    }
    if let Some(model) = first_number_after(n, "rx").or_else(|| first_number_after(n, "rx ")) {
        let score = match model {
            400..=499 => 40.0 + (model % 100) as f64 * 0.25,
            500..=599 => 50.0 + (model % 100) as f64 * 0.2,
            5000..=5999 => 68.0 + (model % 100) as f64 * 0.15,
            6000..=6999 => 76.0 + (model % 100) as f64 * 0.12,
            7000..=7999 => 84.0 + (model % 100) as f64 * 0.1,
            _ => 48.0,
        };
        return Some(TierHit {
            score: score.clamp(20.0, 100.0),
            label: format!("Radeon RX {model}"),
        });
    }
    if n.contains("vega") {
        let score = if n.contains("vega 11") || n.contains("vega 10") {
            52.0
        } else if n.contains("vega 8") {
            48.0
        } else if n.contains("vega 3") {
            40.0
        } else {
            46.0
        };
        return Some(TierHit {
            score,
            label: "Radeon Vega".into(),
        });
    }
    if n.contains("radeon graphics") || n.contains("radeon(tm) graphics") {
        return Some(TierHit {
            score: 46.0,
            label: "Radeon 核显".into(),
        });
    }
    if n.contains("hd 7") || n.contains("hd 6") || n.contains("r5") || n.contains("r7") {
        return Some(TierHit {
            score: 26.0,
            label: "早期 Radeon".into(),
        });
    }
    None
}

fn first_number_after(n: &str, key: &str) -> Option<u32> {
    let idx = n.find(key)?;
    let rest = n[idx + key.len()..].trim_start_matches(|c: char| !c.is_ascii_digit());
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn trailing_model(n: &str) -> Option<u32> {
    let mut last = None;
    let bytes = n.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i - start >= 4 {
                last = n[start..i].parse().ok();
            }
        } else {
            i += 1;
        }
    }
    last
}

fn gpu_model_number(n: &str) -> Option<u32> {
    trailing_model(n).or_else(|| {
        let mut best = None;
        let bytes = n.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if (3..=4).contains(&(i - start)) {
                    best = n[start..i].parse().ok();
                }
            } else {
                i += 1;
            }
        }
        best
    })
}

#[cfg(windows)]
pub fn primary_gpu_name() -> Option<String> {
    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }
    const IID_FACTORY1: Guid = Guid {
        data1: 0x770a_ae78,
        data2: 0xf26f,
        data3: 0x4dba,
        data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    };
    #[repr(C)]
    struct Luid {
        low: u32,
        high: i32,
    }
    #[repr(C)]
    struct AdapterDesc {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        adapter_luid: Luid,
    }
    #[link(name = "dxgi")]
    extern "system" {
        fn CreateDXGIFactory1(riid: *const Guid, pp: *mut *mut core::ffi::c_void) -> i32;
    }
    unsafe {
        let mut factory = core::ptr::null_mut();
        if CreateDXGIFactory1(&IID_FACTORY1, &mut factory) < 0 || factory.is_null() {
            return None;
        }
        let vtbl = *(factory as *const *const usize);
        type EnumAdapters1 = unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut *mut core::ffi::c_void) -> i32;
        type Release = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
        type GetDesc = unsafe extern "system" fn(*mut core::ffi::c_void, *mut AdapterDesc) -> i32;
        let enum_adapters1: EnumAdapters1 = core::mem::transmute(*vtbl.add(12));
        let factory_release: Release = core::mem::transmute(*vtbl.add(2));
        let mut best: Option<(u64, String)> = None;
        let mut i = 0u32;
        while i < 8 {
            let mut adapter = core::ptr::null_mut();
            if enum_adapters1(factory, i, &mut adapter) < 0 || adapter.is_null() {
                break;
            }
            let av = *(adapter as *const *const usize);
            let get_desc: GetDesc = core::mem::transmute(*av.add(8));
            let adapter_release: Release = core::mem::transmute(*av.add(2));
            let mut desc = core::mem::zeroed::<AdapterDesc>();
            if get_desc(adapter, &mut desc) >= 0 {
                let name = String::from_utf16_lossy(
                    &desc.description[..desc.description.iter().position(|c| *c == 0).unwrap_or(128)],
                );
                let name = name.trim().to_string();
                let low = name.to_ascii_lowercase();
                if !low.contains("basic render") && !name.is_empty() {
                    let rank = desc.dedicated_video_memory as u64;
                    if best.as_ref().map(|(r, _)| rank >= *r).unwrap_or(true) {
                        best = Some((rank, name));
                    }
                }
            }
            adapter_release(adapter);
            i += 1;
        }
        factory_release(factory);
        best.map(|(_, n)| n)
    }
}

#[cfg(not(windows))]
pub fn primary_gpu_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::{classify_cpu, classify_gpu};

    #[test]
    fn broadwell_u_is_not_a_desktop_i7() {
        let u = classify_cpu("Intel(R) Core(TM) i7-5500U CPU @ 2.40GHz");
        let desk = classify_cpu("Intel(R) Core(TM) i7-14700K");
        assert!(u.label.contains("5 代"));
        assert!(u.label.contains("低压"));
        assert!(u.score < 50.0, "{}", u.score);
        assert!(desk.score > 90.0, "{}", desk.score);
        assert!(desk.score > u.score + 40.0);
    }

    #[test]
    fn ryzen_and_gpu_orders() {
        let r5 = classify_cpu("AMD Ryzen 5 5600 6-Core Processor");
        let old = classify_cpu("Intel(R) Core(TM) i7-5500U CPU @ 2.40GHz");
        assert!(r5.score > old.score);
        let hd = classify_gpu("Intel(R) HD Graphics 5500");
        let uhd = classify_gpu("Intel(R) UHD Graphics 620");
        let rtx = classify_gpu("NVIDIA GeForce RTX 3060");
        assert!(hd.label.contains("5500"));
        assert!(hd.score < uhd.score);
        assert!(uhd.score < rtx.score);
        assert!((hd.score - 38.0).abs() < 1.0, "{}", hd.score);
    }

    #[test]
    fn eighth_gen_u_above_broadwell_u() {
        let b = classify_cpu("Intel Core i7-5500U");
        let c = classify_cpu("Intel Core i5-8250U");
        assert!(c.score > b.score, "{} vs {}", c.score, b.score);
    }
}
