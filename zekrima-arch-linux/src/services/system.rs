//! Event-driven Linux Volume & Brightness HUD service for Arch / CachyOS.
//!
//! - Volume: listens to `pactl subscribe` (PipeWire `pipewire-pulse`) and reads
//!   the exact volume & mute state via `wpctl get-volume @DEFAULT_AUDIO_SINK@`.
//! - Brightness: listens to `udevadm monitor --kernel --subsystem-match=backlight`
//!   and reads `/sys/class/backlight/*/brightness` / `max_brightness`.
//! Both workers block on kernel/PipeWire events -> 0.0% idle CPU.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const HUD_DURATION: Duration = Duration::from_millis(1800);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudKind {
    Volume,
    Brightness,
}

#[derive(Clone, Debug)]
pub struct HudState {
    pub kind: HudKind,
    pub value: f32,
    pub muted: bool,
    pub visible_until: Option<Instant>,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            kind: HudKind::Volume,
            value: 0.0,
            muted: false,
            visible_until: None,
        }
    }
}

impl HudState {
    pub fn is_visible(&self, now: Instant) -> bool {
        self.visible_until.map(|t| now < t).unwrap_or(false)
    }
}

#[derive(Clone, Default)]
pub struct SystemSnapshot {
    pub hud: HudState,
    pub volume: f32,
    pub muted: bool,
}

pub struct SystemService {
    shared: Arc<Mutex<SystemSnapshot>>,
    shutdown: Arc<AtomicBool>,
    pids: Arc<Mutex<Vec<u32>>>,
    workers: Vec<JoinHandle<()>>,
}

impl SystemService {
    pub fn start(on_update: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Mutex::new(SystemSnapshot::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let pids = Arc::new(Mutex::new(Vec::new()));

        if let Some((vol, muted)) = read_volume() {
            let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.volume = vol;
            guard.muted = muted;
        }

        let mut workers = Vec::new();

        {
            let s = Arc::clone(&shared);
            let sd = Arc::clone(&shutdown);
            let p = Arc::clone(&pids);
            let cb = Arc::clone(&on_update);
            if let Ok(h) = thread::Builder::new()
                .name("zekrima-volume".to_owned())
                .spawn(move || volume_worker(s, sd, p, cb))
            {
                workers.push(h);
            }
        }

        {
            let s = Arc::clone(&shared);
            let sd = Arc::clone(&shutdown);
            let p = Arc::clone(&pids);
            let cb = Arc::clone(&on_update);
            if let Ok(h) = thread::Builder::new()
                .name("zekrima-brightness".to_owned())
                .spawn(move || brightness_worker(s, sd, p, cb))
            {
                workers.push(h);
            }
        }

        Self {
            shared,
            shutdown,
            pids,
            workers,
        }
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl Drop for SystemService {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let pids: Vec<u32> = self
            .pids
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
            .collect();
        for pid in pids {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn volume_worker(
    shared: Arc<Mutex<SystemSnapshot>>,
    shutdown: Arc<AtomicBool>,
    pids: Arc<Mutex<Vec<u32>>>,
    on_update: Arc<dyn Fn() + Send + Sync>,
) {
    let mut last_vol = shared
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .volume;
    let mut last_muted = shared
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .muted;

    while !shutdown.load(Ordering::SeqCst) {
        let Ok(mut child) = Command::new("pactl")
            .arg("subscribe")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            thread::sleep(Duration::from_secs(5));
            continue;
        };

        let pid = child.id();
        pids.lock().unwrap_or_else(|p| p.into_inner()).push(pid);

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(line) = line else { break };
                if !line.contains("on sink") {
                    continue;
                }
                if let Some((vol, muted)) = read_volume() {
                    if (vol - last_vol).abs() >= 0.004 || muted != last_muted {
                        last_vol = vol;
                        last_muted = muted;
                        {
                            let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
                            guard.volume = vol;
                            guard.muted = muted;
                            guard.hud = HudState {
                                kind: HudKind::Volume,
                                value: vol,
                                muted,
                                visible_until: Some(Instant::now() + HUD_DURATION),
                            };
                        }
                        on_update();
                    }
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        pids.lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|&p| p != pid);

        if !shutdown.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(2));
        }
    }
}

fn brightness_worker(
    shared: Arc<Mutex<SystemSnapshot>>,
    shutdown: Arc<AtomicBool>,
    pids: Arc<Mutex<Vec<u32>>>,
    on_update: Arc<dyn Fn() + Send + Sync>,
) {
    let Some(backlight_dir) = find_backlight_dir() else {
        // Desktop PC without internal backlight sysfs: exit worker cleanly (0% CPU).
        return;
    };

    let mut last_val = read_sysfs_brightness(&backlight_dir);

    while !shutdown.load(Ordering::SeqCst) {
        let Ok(mut child) = Command::new("udevadm")
            .args(["monitor", "--kernel", "--subsystem-match=backlight"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };

        let pid = child.id();
        pids.lock().unwrap_or_else(|p| p.into_inner()).push(pid);

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(line) = line else { break };
                if !line.contains("change") || !line.contains("backlight") {
                    continue;
                }
                if let Some(val) = read_sysfs_brightness(&backlight_dir) {
                    let changed = last_val.map(|p| (p - val).abs() >= 0.004).unwrap_or(true);
                    if changed {
                        last_val = Some(val);
                        {
                            let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
                            guard.hud = HudState {
                                kind: HudKind::Brightness,
                                value: val,
                                muted: false,
                                visible_until: Some(Instant::now() + HUD_DURATION),
                            };
                        }
                        on_update();
                    }
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        pids.lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|&p| p != pid);

        if !shutdown.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(2));
        }
    }
}

fn read_volume() -> Option<(f32, bool)> {
    let output = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Expected format: "Volume: 0.65" or "Volume: 0.65 [MUTED]"
    let muted = text.contains("[MUTED]");
    let num = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<f32>().ok())?;
    Some((num.clamp(0.0, 1.0), muted))
}

fn find_backlight_dir() -> Option<PathBuf> {
    let entries = fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.join("brightness").exists() && path.join("max_brightness").exists() {
            return Some(path);
        }
    }
    None
}

fn read_sysfs_brightness(dir: &PathBuf) -> Option<f32> {
    let cur: f32 = fs::read_to_string(dir.join("brightness"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let max: f32 = fs::read_to_string(dir.join("max_brightness"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if max <= 0.0 {
        return None;
    }
    Some((cur / max).clamp(0.0, 1.0))
}
