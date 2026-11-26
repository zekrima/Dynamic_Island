//! Real-time PipeWire / PulseAudio loopback FFT audio visualizer for Arch/CachyOS.
//!
//! Captures `@DEFAULT_MONITOR@` via `parec` (provided by `pipewire-pulse`,
//! pre-installed on CachyOS), computes a 512-point radix-2 FFT in pure Rust,
//! and outputs 5 normalized rhythm bands (`[f32; 5]`) via lock-free atomics.
//!
//! When `set_active(false)` (paused or no media), the `parec` child is killed
//! immediately and the worker thread sleeps on a condvar (0.0% CPU).

use std::f32::consts::PI;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const EQ_BANDS: usize = 5;
const FFT_SIZE: usize = 512;
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;

struct SharedViz {
    levels: [AtomicU32; EQ_BANDS],
    active: AtomicBool,
    shutdown: AtomicBool,
    wake_lock: Mutex<()>,
    wake_cv: Condvar,
    child_pid: Mutex<Option<u32>>,
}

impl SharedViz {
    fn new() -> Self {
        Self {
            levels: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            active: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            wake_lock: Mutex::new(()),
            wake_cv: Condvar::new(),
            child_pid: Mutex::new(None),
        }
    }

    fn set_levels(&self, values: [f32; EQ_BANDS]) {
        for (slot, val) in self.levels.iter().zip(values) {
            slot.store(val.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    fn read_levels(&self) -> [f32; EQ_BANDS] {
        std::array::from_fn(|i| f32::from_bits(self.levels[i].load(Ordering::Relaxed)))
    }

    fn kill_child(&self) {
        if let Some(pid) = self.child_pid.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

pub struct AudioVisualizer {
    shared: Arc<SharedViz>,
    worker: Option<JoinHandle<()>>,
}

impl AudioVisualizer {
    pub fn start() -> Self {
        let shared = Arc::new(SharedViz::new());
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("zekrima-audio-viz".to_owned())
            .spawn(move || viz_worker(worker_shared))
            .ok();

        Self { shared, worker }
    }

    pub fn set_active(&self, active: bool) {
        let prev = self.shared.active.swap(active, Ordering::SeqCst);
        if prev != active {
            if !active {
                self.shared.set_levels([0.0; EQ_BANDS]);
                self.shared.kill_child();
            }
            let _guard = self.shared.wake_lock.lock().unwrap_or_else(|p| p.into_inner());
            self.shared.wake_cv.notify_all();
        }
    }

    pub fn levels(&self) -> [f32; EQ_BANDS] {
        self.shared.read_levels()
    }
}

impl Drop for AudioVisualizer {
    fn drop(&mut self) {
        self.shared.active.store(false, Ordering::SeqCst);
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.kill_child();
        {
            let _guard = self.shared.wake_lock.lock().unwrap_or_else(|p| p.into_inner());
            self.shared.wake_cv.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn spawn_monitor_capture() -> Option<Child> {
    Command::new("parec")
        .args([
            "--format=float32le",
            "--rate=48000",
            "--channels=2",
            "--device=@DEFAULT_MONITOR@",
            "--latency-msec=20",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn viz_worker(shared: Arc<SharedViz>) {
    let hann = build_hann_window();
    let mut ring = [0.0f32; FFT_SIZE];
    let mut smoothed = [0.0f32; EQ_BANDS];
    // 256 stereo f32 frames = 2048 bytes (~5.3 ms per chunk)
    let mut buf = [0u8; 256 * CHANNELS * 4];

    while !shared.shutdown.load(Ordering::SeqCst) {
        if !shared.active.load(Ordering::SeqCst) {
            smoothed = [0.0; EQ_BANDS];
            shared.set_levels(smoothed);
            let guard = shared.wake_lock.lock().unwrap_or_else(|p| p.into_inner());
            if !shared.active.load(Ordering::SeqCst) && !shared.shutdown.load(Ordering::SeqCst) {
                drop(shared.wake_cv.wait(guard));
            }
            continue;
        }

        let Some(mut child) = spawn_monitor_capture() else {
            thread::sleep(Duration::from_millis(800));
            continue;
        };

        *shared.child_pid.lock().unwrap_or_else(|p| p.into_inner()) = Some(child.id());

        if let Some(mut stdout) = child.stdout.take() {
            ring.fill(0.0);
            let mut ring_pos: usize = 0;
            let mut running_peak = 0.08f32;

            while shared.active.load(Ordering::SeqCst) && !shared.shutdown.load(Ordering::SeqCst) {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let usable = n - (n % (CHANNELS * 4));
                        for frame in buf[..usable].chunks_exact(CHANNELS * 4) {
                            let left = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
                            let right =
                                f32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
                            ring[ring_pos] = 0.5 * (left + right);
                            ring_pos = (ring_pos + 1) & (FFT_SIZE - 1);
                        }

                        let raw = analyze_spectrum(&ring, ring_pos, &hann, SAMPLE_RATE);
                        let max_band = raw.iter().copied().fold(0.0f32, f32::max);

                        if max_band < 0.0008 {
                            for s in &mut smoothed {
                                *s *= 0.80;
                                if *s < 0.01 {
                                    *s = 0.0;
                                }
                            }
                        } else {
                            running_peak = (running_peak * 0.990).max(max_band).max(0.015);
                            for i in 0..EQ_BANDS {
                                let norm = (raw[i] / running_peak).clamp(0.0, 1.0).powf(0.75);
                                let alpha = if norm > smoothed[i] { 0.62 } else { 0.22 };
                                smoothed[i] += (norm - smoothed[i]) * alpha;
                            }
                        }
                        shared.set_levels(smoothed);
                    }
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        *shared.child_pid.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

fn build_hann_window() -> [f32; FFT_SIZE] {
    let mut w = [0.0f32; FFT_SIZE];
    let denom = (FFT_SIZE - 1) as f32;
    for (i, slot) in w.iter_mut().enumerate() {
        *slot = 0.5 * (1.0 - (2.0 * PI * i as f32 / denom).cos());
    }
    w
}

fn analyze_spectrum(
    ring: &[f32; FFT_SIZE],
    ring_pos: usize,
    hann: &[f32; FFT_SIZE],
    sample_rate: u32,
) -> [f32; EQ_BANDS] {
    let mut re = [0.0f32; FFT_SIZE];
    let mut im = [0.0f32; FFT_SIZE];

    for i in 0..FFT_SIZE {
        let idx = (ring_pos + i) & (FFT_SIZE - 1);
        re[i] = ring[idx] * hann[i];
    }

    fft_in_place(&mut re, &mut im);

    let bin_hz = sample_rate as f32 / FFT_SIZE as f32;
    const EDGES_HZ: [(f32, f32, f32); EQ_BANDS] = [
        (40.0, 180.0, 0.85),
        (180.0, 480.0, 1.00),
        (480.0, 1400.0, 1.25),
        (1400.0, 3600.0, 1.55),
        (3600.0, 9500.0, 1.95),
    ];

    let max_bin = FFT_SIZE / 2 - 1;
    let mut bands = [0.0f32; EQ_BANDS];
    let inv_n = 2.0 / FFT_SIZE as f32;

    for (b, &(lo_hz, hi_hz, weight)) in EDGES_HZ.iter().enumerate() {
        let lo = ((lo_hz / bin_hz).floor() as usize).clamp(1, max_bin);
        let hi = ((hi_hz / bin_hz).ceil() as usize).clamp(lo, max_bin);
        let mut sum_sq = 0.0f32;
        let count = (hi - lo + 1) as f32;
        for k in lo..=hi {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() * inv_n;
            sum_sq += mag * mag;
        }
        bands[b] = (sum_sq / count).sqrt() * weight;
    }

    bands
}

fn fft_in_place(re: &mut [f32; FFT_SIZE], im: &mut [f32; FFT_SIZE]) {
    let mut j = 0usize;
    for i in 1..FFT_SIZE {
        let mut bit = FFT_SIZE >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= FFT_SIZE {
        let half = len >> 1;
        let angle = -2.0 * PI / len as f32;
        let w_len_re = angle.cos();
        let w_len_im = angle.sin();

        let mut start = 0usize;
        while start < FFT_SIZE {
            let mut w_re = 1.0f32;
            let mut w_im = 0.0f32;
            for k in 0..half {
                let u = start + k;
                let v = u + half;
                let t_re = re[v] * w_re - im[v] * w_im;
                let t_im = re[v] * w_im + im[v] * w_re;
                re[v] = re[u] - t_re;
                im[v] = im[u] - t_im;
                re[u] += t_re;
                im[u] += t_im;

                let next_re = w_re * w_len_re - w_im * w_len_im;
                let next_im = w_re * w_len_im + w_im * w_len_re;
                w_re = next_re;
                w_im = next_im;
            }
            start += len;
        }

        len <<= 1;
    }
}
