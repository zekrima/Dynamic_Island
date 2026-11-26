//! On-demand native Win32 System Monitor (CPU, RAM, Battery/Power).
//!
//! Only sampled while the user is viewing the `Sys` tab — 0.0% idle CPU.

use std::mem::size_of;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

#[derive(Clone, Debug, Default)]
pub struct SystemStats {
    pub cpu_percent: f32,
    pub ram_percent: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    pub has_battery: bool,
    pub battery_percent: f32,
    pub is_charging: bool,
}

pub struct SystemMonitor {
    prev_idle: u64,
    prev_kernel: u64,
    prev_user: u64,
    seeded: bool,
    pub stats: SystemStats,
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self {
            prev_idle: 0,
            prev_kernel: 0,
            prev_user: 0,
            seeded: false,
            stats: SystemStats::default(),
        }
    }

    pub fn sample(&mut self) {
        unsafe {
            self.sample_cpu();
            self.sample_memory();
            self.sample_power();
        }
    }

    unsafe fn sample_cpu(&mut self) {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_err() {
            return;
        }
        let idle_u = filetime_to_u64(idle);
        let kernel_u = filetime_to_u64(kernel);
        let user_u = filetime_to_u64(user);

        if self.seeded {
            let d_idle = idle_u.saturating_sub(self.prev_idle);
            let d_total =
                kernel_u.saturating_sub(self.prev_kernel) + user_u.saturating_sub(self.prev_user);
            if d_total > 0 {
                let busy = d_total.saturating_sub(d_idle) as f64;
                self.stats.cpu_percent = ((busy / d_total as f64) * 100.0).clamp(0.0, 100.0) as f32;
            }
        }
        self.prev_idle = idle_u;
        self.prev_kernel = kernel_u;
        self.prev_user = user_u;
        self.seeded = true;
    }

    unsafe fn sample_memory(&mut self) {
        let mut mem = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if GlobalMemoryStatusEx(&mut mem).is_ok() && mem.ullTotalPhys > 0 {
            const GB: f64 = 1024.0 * 1024.0 * 1024.0;
            let total = mem.ullTotalPhys as f64 / GB;
            let used = mem.ullTotalPhys.saturating_sub(mem.ullAvailPhys) as f64 / GB;
            self.stats.ram_total_gb = total as f32;
            self.stats.ram_used_gb = used as f32;
            self.stats.ram_percent = (mem.dwMemoryLoad as f32).clamp(0.0, 100.0);
        }
    }

    unsafe fn sample_power(&mut self) {
        let mut power = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut power).is_ok() {
            // 128 = no system battery, 255 = unknown
            let no_battery = (power.BatteryFlag & 128) != 0 || power.BatteryLifePercent == 255;
            self.stats.has_battery = !no_battery;
            self.stats.battery_percent = if no_battery {
                100.0
            } else {
                (power.BatteryLifePercent as f32).clamp(0.0, 100.0)
            };
            self.stats.is_charging = power.ACLineStatus == 1;
        }
    }
}

fn filetime_to_u64(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}
