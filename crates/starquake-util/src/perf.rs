//! 性能诊断：按 CPU / GPU 天梯、内存档和显示器刷新率给出渲染建议。
//! 天梯分数为经验启发式，非精确跑分。

use crate::ladder::{classify_cpu, classify_gpu, primary_gpu_name};
use starquake_config::recommend_target_fps;

/// 诊断结果与推荐配置。
#[derive(Debug, Clone)]
pub struct PerfDiagnosis {
    pub cpu_name: String,
    pub ram_gb: f64,
    /// 主屏当前刷新率（Hz）；读取失败则为 `None`
    pub display_hz: Option<u32>,
    /// 识别到的显卡名；读不到则为空
    pub gpu_name: String,
    /// CPU 天梯说明
    pub cpu_tier: String,
    /// GPU 天梯说明
    pub gpu_tier: String,
    pub gpu_hint: String,
    /// `cpu` | `gpu`。诊断一律建议 `gpu`（OpenGL）；软渲染只在画面无法创建时由用户手选。
    pub device: String,
    /// `eco` | `normal` | `extreme`
    pub profile: String,
    /// 建议目标帧率（已按机器预算与显示器刷新率封顶并对齐档位）
    pub target_fps: u32,
    pub summary: String,
}

/// 采集本机 CPU、显卡、内存与刷新率，对照天梯后给出建议。
pub fn diagnose() -> PerfDiagnosis {
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();

    let cpu_name = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "未知 CPU".into());
    let gpu_name = primary_gpu_name().unwrap_or_default();
    let cpu_hit = classify_cpu(&cpu_name);
    let gpu_hit = if gpu_name.is_empty() {
        classify_gpu("unknown")
    } else {
        classify_gpu(&gpu_name)
    };
    let ram_gb = sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
    let cpu_score = cpu_hit.score;
    let gpu_score = if gpu_name.is_empty() {
        cpu_score * 0.85
    } else {
        gpu_hit.score
    };
    let ram_score = score_ram(ram_gb);
    let display_hz = primary_display_hz();

    let ram_tier = MemoryTier::from_ram_gb(ram_gb);
    let slots = tile_cache_slots(ram_gb, false);
    // 综合分：CPU 为主，核显/独显把帧率预算拉下来。2G/4G 内存强制节能。
    let headroom = cpu_score * 0.62 + gpu_score * 0.38;
    let (device, profile, machine_budget, gpu_hint) =
        if matches!(ram_tier, MemoryTier::G2 | MemoryTier::G4) || headroom < 32.0 {
            (
                "gpu",
                "eco",
                30_u32,
                format!(
                    "内存档 {}：OpenGL + 节能。空闲约 1.5 秒一帧，瓦片缓存 {slots} 张（约 {}）。画面无法显示时再改 CPU 模式。",
                    ram_tier.label(),
                    ram_tier.tile_budget_hint(),
                ),
            )
        } else if headroom < 52.0 {
            (
                "gpu",
                "normal",
                60,
                format!(
                    "内存档 {}：OpenGL，正常模式，目标约 60 帧。空闲约 1 秒一帧，瓦片缓存 {slots} 张（约 {}）。",
                    ram_tier.label(),
                    ram_tier.tile_budget_hint(),
                ),
            )
        } else if headroom < 75.0 {
            (
                "gpu",
                "normal",
                90,
                format!(
                    "内存档 {}：OpenGL，正常模式（有余量可试极致）。瓦片缓存 {slots} 张（约 {}）。",
                    ram_tier.label(),
                    ram_tier.tile_budget_hint(),
                ),
            )
        } else {
            (
                "gpu",
                "extreme",
                165,
                format!(
                    "内存档 {}：OpenGL，极致模式（帧率跟主屏刷新率）。瓦片缓存 {slots} 张（约 {}）。",
                    ram_tier.label(),
                    ram_tier.tile_budget_hint(),
                ),
            )
        };

    let target_fps = recommend_target_fps(machine_budget, display_hz);
    let hz_text = match display_hz {
        Some(hz) => format!("主屏刷新率：{hz} Hz"),
        None => "主屏刷新率：未检测到（按 60 Hz 封顶）".into(),
    };

    let gpu_line = if gpu_name.is_empty() {
        "GPU：未检测到（按 CPU 档估算）".to_string()
    } else {
        format!("GPU：{gpu_name}")
    };
    let summary = format!(
        "CPU：{cpu_name}\nCPU 天梯：{}（{cpu_score:.0}）\n{gpu_line}\nGPU 天梯：{}（{gpu_score:.0}）\n内存：{ram_gb:.1} GB → 档位 {}（估分 {ram_score:.0}）\n{hz_text}\n{gpu_hint}\n建议：OpenGL，档位={profile}，帧率={target_fps}，瓦片缓存={slots} 张\n（切换渲染设备需重启；性能档位与帧率立即生效）",
        cpu_hit.label,
        gpu_hit.label,
        ram_tier.label(),
    );

    PerfDiagnosis {
        cpu_name,
        ram_gb,
        display_hz,
        gpu_name,
        cpu_tier: cpu_hit.label,
        gpu_tier: gpu_hit.label,
        gpu_hint,
        device: device.into(),
        profile: profile.into(),
        target_fps,
        summary,
    }
}

/// 本机内存档：2G / 4G / 8G / 16G / 大于 16G。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTier {
    /// 约 2GB 及以下
    G2,
    /// 约 4GB
    G4,
    /// 约 8GB
    G8,
    /// 约 16GB
    G16,
    /// 大于 16GB
    Gt16,
}

impl MemoryTier {
    /// 按物理内存归档（边界留余量，避免 7.7GB 被判成 4G）。
    pub fn from_ram_gb(ram_gb: f64) -> Self {
        if ram_gb < 3.0 {
            Self::G2
        } else if ram_gb < 6.0 {
            Self::G4
        } else if ram_gb < 12.0 {
            Self::G8
        } else if ram_gb < 24.0 {
            Self::G16
        } else {
            Self::Gt16
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::G2 => "2G",
            Self::G4 => "4G",
            Self::G8 => "8G",
            Self::G16 => "16G",
            Self::Gt16 => "大于16G",
        }
    }

    /// 瓦片张数上限（约 0.25MB/张 RGBA）。
    pub fn tile_slots(self) -> usize {
        match self {
            Self::G2 => 64,   // ~16MB
            Self::G4 => 128,  // ~32MB
            Self::G8 => 256,  // ~64MB
            Self::G16 => 512, // ~128MB
            Self::Gt16 => 800, // ~200MB
        }
    }

    pub fn tile_budget_hint(self) -> &'static str {
        match self {
            Self::G2 => "16MB",
            Self::G4 => "32MB",
            Self::G8 => "64MB",
            Self::G16 => "128MB",
            Self::Gt16 => "200MB",
        }
    }
}

/// 瓦片张数：按内存五档；软渲染再压一档（最低 2G）。
pub fn tile_cache_slots(ram_gb: f64, software_raster: bool) -> usize {
    let tier = MemoryTier::from_ram_gb(ram_gb);
    
    if software_raster {
        match tier {
            MemoryTier::G2 => MemoryTier::G2.tile_slots(),
            MemoryTier::G4 => MemoryTier::G2.tile_slots(),
            MemoryTier::G8 => MemoryTier::G4.tile_slots(),
            MemoryTier::G16 => MemoryTier::G8.tile_slots(),
            MemoryTier::Gt16 => MemoryTier::G16.tile_slots(),
        }
    } else {
        tier.tile_slots()
    }
}

/// 读取本机内存后给出瓦片张数上限。
pub fn recommended_tile_slots(software_raster: bool) -> usize {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let ram_gb = sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
    tile_cache_slots(ram_gb, software_raster)
}

fn score_ram(gb: f64) -> f64 {
    match MemoryTier::from_ram_gb(gb) {
        MemoryTier::G2 => 5.0,
        MemoryTier::G4 => 15.0,
        MemoryTier::G8 => 30.0,
        MemoryTier::G16 => 45.0,
        MemoryTier::Gt16 => 55.0,
    }
}

/// 读取主显示器当前刷新率（Hz）。
fn primary_display_hz() -> Option<u32> {
    #[cfg(windows)]
    {
        windows_primary_display_hz()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_primary_display_hz() -> Option<u32> {
    const ENUM_CURRENT_SETTINGS: u32 = 0xFFFF_FFFF;

    /// 截取到 `dmDisplayFrequency` 的 DEVMODEW 前缀（Unicode）。
    #[repr(C)]
    struct DevModeWFreq {
        dm_device_name: [u16; 32],
        dm_spec_version: u16,
        dm_driver_version: u16,
        dm_size: u16,
        dm_driver_extra: u16,
        dm_fields: u32,
        _union: [u8; 16],
        dm_color: i16,
        dm_duplex: i16,
        dm_y_resolution: i16,
        dm_tt_option: i16,
        dm_collate: i16,
        dm_form_name: [u16; 32],
        dm_log_pixels: u16,
        dm_bits_per_pel: u32,
        dm_pels_width: u32,
        dm_pels_height: u32,
        dm_display_flags: u32,
        dm_display_frequency: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumDisplaySettingsW(
            lpsz_device_name: *const u16,
            i_mode_num: u32,
            lp_dev_mode: *mut DevModeWFreq,
        ) -> i32;
        fn GetDC(hwnd: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn ReleaseDC(hwnd: *mut core::ffi::c_void, hdc: *mut core::ffi::c_void) -> i32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn GetDeviceCaps(hdc: *mut core::ffi::c_void, index: i32) -> i32;
    }

    let mut mode = unsafe { std::mem::zeroed::<DevModeWFreq>() };
    mode.dm_size = std::mem::size_of::<DevModeWFreq>() as u16;
    let ok = unsafe {
        EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut mode)
    };
    if ok != 0 {
        let freq = mode.dm_display_frequency;
        if (30..=1000).contains(&freq) {
            return Some(freq);
        }
    }

    // 回退：GetDeviceCaps(VREFRESH)
    const VREFRESH: i32 = 116;
    unsafe {
        let hdc = GetDC(std::ptr::null_mut());
        if hdc.is_null() {
            return None;
        }
        let hz = GetDeviceCaps(hdc, VREFRESH);
        ReleaseDC(std::ptr::null_mut(), hdc);
        if (30..=1000).contains(&(hz as u32)) {
            Some(hz as u32)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use starquake_config::{recommend_target_fps, snap_target_fps_at_most};

    #[test]
    fn recommend_respects_display_cap() {
        assert_eq!(snap_target_fps_at_most(240), 240);
        assert_eq!(snap_target_fps_at_most(200), 180);
        assert_eq!(recommend_target_fps(300, Some(240)), 240);
        assert_eq!(recommend_target_fps(60, Some(240)), 60);
        assert_eq!(recommend_target_fps(300, None), 60);
    }

    #[test]
    fn diagnose_reads_local_display() {
        let d = super::diagnose();
        eprintln!(
            "display_hz={:?} target_fps={} profile={}",
            d.display_hz, d.target_fps, d.profile
        );
        assert!(starquake_config::TARGET_FPS_PRESETS.contains(&d.target_fps));
        assert_eq!(d.device, "gpu");
    }

    #[test]
    fn memory_tiers_and_tile_slots() {
        use super::MemoryTier;
        assert_eq!(MemoryTier::from_ram_gb(1.8), MemoryTier::G2);
        assert_eq!(MemoryTier::from_ram_gb(3.9), MemoryTier::G4);
        assert_eq!(MemoryTier::from_ram_gb(7.5), MemoryTier::G8);
        assert_eq!(MemoryTier::from_ram_gb(15.5), MemoryTier::G16);
        assert_eq!(MemoryTier::from_ram_gb(32.0), MemoryTier::Gt16);
        assert_eq!(super::tile_cache_slots(2.0, false), 64);
        assert_eq!(super::tile_cache_slots(4.0, false), 128);
        assert_eq!(super::tile_cache_slots(8.0, false), 256);
        assert_eq!(super::tile_cache_slots(16.0, false), 512);
        assert_eq!(super::tile_cache_slots(32.0, false), 800);
        // 软渲染压一档
        assert_eq!(super::tile_cache_slots(32.0, true), 512);
        assert_eq!(super::tile_cache_slots(8.0, true), 128);
        assert_eq!(super::tile_cache_slots(2.0, true), 64);
    }
}
