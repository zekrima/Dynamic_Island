//! On-demand native Linux System Monitor (`/proc/stat`, `/proc/meminfo`, `/sys/class/power_supply`).
//!
//! Zero external crates; only sampled while the user views the `Sys` tab -> 0.0% idle CPU.

use std::fs;

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
    prev_total: u64,
    prev_idle: u64,
    seeded: bool,
    pub stats: SystemStats,
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self {
            prev_total: 0,
            prev_idle: 0,
            seeded: false,
            stats: SystemStats::default(),
        }
    }

    pub fn sample(&mut self) {
        self.sample_cpu();
        self.sample_memory();
        self.sample_power();
    }

    fn sample_cpu(&mut self) {
        let Ok(content) = fs::read_to_string("/proc/stat") else {
            return;
        };
        let Some(first) = content.lines().next() else {
            return;
        };
        let nums: Vec<u64> = first
            .split_whitespace()
            .skip(1)
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();
        if nums.len() < 4 {
            return;
        }
        let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
        let total: u64 = nums.iter().sum();

        if self.seeded {
            let d_total = total.saturating_sub(self.prev_total);
            let d_idle = idle.saturating_sub(self.prev_idle);
            if d_total > 0 {
                let busy = d_total.saturating_sub(d_idle) as f64;
                self.stats.cpu_percent = ((busy / d_total as f64) * 100.0).clamp(0.0, 100.0) as f32;
            }
        }
        self.prev_total = total;
        self.prev_idle = idle;
        self.seeded = true;
    }

    fn sample_memory(&mut self) {
        let Ok(content) = fs::read_to_string("/proc/meminfo") else {
            return;
        };
        let mut total_kb: u64 = 0;
        let mut avail_kb: u64 = 0;
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                avail_kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }
        if total_kb > 0 {
            const KB_PER_GB: f64 = 1024.0 * 1024.0;
            let used_kb = total_kb.saturating_sub(avail_kb);
            self.stats.ram_total_gb = (total_kb as f64 / KB_PER_GB) as f32;
            self.stats.ram_used_gb = (used_kb as f64 / KB_PER_GB) as f32;
            self.stats.ram_percent =
                ((used_kb as f64 / total_kb as f64) * 100.0).clamp(0.0, 100.0) as f32;
        }
    }

    fn sample_power(&mut self) {
        let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
            self.stats.has_battery = false;
            self.stats.battery_percent = 100.0;
            return;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("BAT") {
                continue;
            }
            if let Ok(cap_str) = fs::read_to_string(path.join("capacity")) {
                if let Ok(cap) = cap_str.trim().parse::<f32>() {
                    let status = fs::read_to_string(path.join("status")).unwrap_or_default();
                    self.stats.has_battery = true;
                    self.stats.battery_percent = cap.clamp(0.0, 100.0);
                    self.stats.is_charging = status.trim().eq_ignore_ascii_case("Charging");
                    return;
                }
            }
        }

        self.stats.has_battery = false;
        self.stats.battery_percent = 100.0;
        self.stats.is_charging = true;
    }
}
