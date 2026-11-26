//! Real-time WASAPI loopback audio spectrum analyzer.
//!
//! Captures the Windows default audio render stream via
//! `AUDCLNT_STREAMFLAGS_LOOPBACK`, computes a 512-point radix-2 FFT in pure
//! Rust, and maps the spectrum into 5 musically spaced rhythm bands
//! (sub-bass, bass, low-mid, high-mid, treble) with dynamic auto-gain and
//! snappy attack / smooth decay.
//!
//! Power & CPU design:
//! - When `set_active(false)` (no media playing or paused), the WASAPI client
//!   is stopped and released and the worker thread sleeps on a condvar
//!   (0.0% CPU, zero audio overhead).
//! - Levels are published via lock-free atomics so the UI thread never blocks.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

pub const EQ_BANDS: usize = 5;
const FFT_SIZE: usize = 512;
const WAVE_FORMAT_PCM_TAG: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE_TAG: u16 = 0xFFFE;
// First 4 bytes of KSDATAFORMAT_SUBTYPE_IEEE_FLOAT / PCM
const SUBFORMAT_PCM: u32 = 0x0000_0001;
const SUBFORMAT_IEEE_FLOAT: u32 = 0x0000_0003;

struct SharedViz {
    levels: [AtomicU32; EQ_BANDS],
    active: AtomicBool,
    shutdown: AtomicBool,
    wake_lock: Mutex<()>,
    wake_cv: Condvar,
}

impl SharedViz {
    fn new() -> Self {
        Self {
            levels: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            active: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            wake_lock: Mutex::new(()),
            wake_cv: Condvar::new(),
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
            .spawn(move || unsafe {
                viz_worker(worker_shared);
            })
            .ok();

        Self { shared, worker }
    }

    /// Enables or disables live loopback capture. When `false`, the capture
    /// stream is torn down and the thread sleeps with 0% CPU.
    pub fn set_active(&self, active: bool) {
        let prev = self.shared.active.swap(active, Ordering::SeqCst);
        if prev != active {
            if !active {
                self.shared.set_levels([0.0; EQ_BANDS]);
            }
            let _guard = self.shared.wake_lock.lock().unwrap_or_else(|p| p.into_inner());
            self.shared.wake_cv.notify_all();
        }
    }

    /// Returns the latest normalized `[0.0..=1.0; 5]` rhythm band energies.
    pub fn levels(&self) -> [f32; EQ_BANDS] {
        self.shared.read_levels()
    }
}

impl Drop for AudioVisualizer {
    fn drop(&mut self) {
        self.shared.active.store(false, Ordering::SeqCst);
        self.shared.shutdown.store(true, Ordering::SeqCst);
        {
            let _guard = self.shared.wake_lock.lock().unwrap_or_else(|p| p.into_inner());
            self.shared.wake_cv.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Copy)]
enum SampleKind {
    Float32,
    Pcm16,
}

struct CaptureSession {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    channels: usize,
    sample_rate: u32,
    sample_kind: SampleKind,
}

impl CaptureSession {
    unsafe fn open() -> Option<Self> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .ok()?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;

        let mix_ptr: *mut WAVEFORMATEX = client.GetMixFormat().ok()?;
        if mix_ptr.is_null() {
            return None;
        }

        let mix = *mix_ptr;
        let channels = (mix.nChannels as usize).max(1);
        let sample_rate = mix.nSamplesPerSec.max(8_000);
        let sample_kind = detect_sample_kind(mix_ptr, &mix);

        // 40 ms shared-mode loopback buffer (in 100-ns units).
        let init_res = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            400_000,
            0,
            mix_ptr,
            None,
        );
        CoTaskMemFree(Some(mix_ptr as *const _));
        init_res.ok()?;

        let capture: IAudioCaptureClient = client.GetService().ok()?;
        client.Start().ok()?;

        Some(Self {
            client,
            capture,
            channels,
            sample_rate,
            sample_kind: sample_kind?,
        })
    }

    unsafe fn close(self) {
        let _ = self.client.Stop();
    }
}

unsafe fn detect_sample_kind(ptr: *const WAVEFORMATEX, wf: &WAVEFORMATEX) -> Option<SampleKind> {
    match wf.wFormatTag {
        WAVE_FORMAT_IEEE_FLOAT_TAG if wf.wBitsPerSample == 32 => Some(SampleKind::Float32),
        WAVE_FORMAT_PCM_TAG if wf.wBitsPerSample == 16 => Some(SampleKind::Pcm16),
        WAVE_FORMAT_EXTENSIBLE_TAG if wf.cbSize >= 22 => {
            // In WAVEFORMATEXTENSIBLE, SubFormat GUID starts 6 bytes after WAVEFORMATEX
            // (wValidBitsPerSample: u16 + dwChannelMask: u32 = 6 bytes).
            let subformat_ptr =
                (ptr as *const u8).add(std::mem::size_of::<WAVEFORMATEX>() + 6) as *const u32;
            let data1 = std::ptr::read_unaligned(subformat_ptr);
            match (data1, wf.wBitsPerSample) {
                (SUBFORMAT_IEEE_FLOAT, 32) => Some(SampleKind::Float32),
                (SUBFORMAT_PCM, 16) => Some(SampleKind::Pcm16),
                _ => None,
            }
        }
        _ => None,
    }
}

unsafe fn viz_worker(shared: Arc<SharedViz>) {
    let com_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();

    let hann = build_hann_window();
    let mut ring = [0.0f32; FFT_SIZE];
    let mut smoothed = [0.0f32; EQ_BANDS];

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

        let Some(session) = CaptureSession::open() else {
            // Audio endpoint unavailable transiently; wait briefly before retry.
            thread::sleep(Duration::from_millis(500));
            continue;
        };

        ring.fill(0.0);
        let mut ring_pos: usize = 0;
        let mut running_peak = 0.08f32;

        while shared.active.load(Ordering::SeqCst) && !shared.shutdown.load(Ordering::SeqCst) {
            let mut got_audio = false;
            let mut device_error = false;

            loop {
                let packet_frames = match session.capture.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => {
                        device_error = true;
                        break;
                    }
                };
                if packet_frames == 0 {
                    break;
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut frames_read: u32 = 0;
                let mut flags: u32 = 0;
                if session
                    .capture
                    .GetBuffer(&mut data_ptr, &mut frames_read, &mut flags, None, None)
                    .is_err()
                {
                    device_error = true;
                    break;
                }

                let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                let frames = frames_read as usize;
                let ch = session.channels;

                if silent || data_ptr.is_null() {
                    for _ in 0..frames {
                        ring[ring_pos] = 0.0;
                        ring_pos = (ring_pos + 1) & (FFT_SIZE - 1);
                    }
                } else {
                    match session.sample_kind {
                        SampleKind::Float32 => {
                            let samples = std::slice::from_raw_parts(
                                data_ptr as *const f32,
                                frames * ch,
                            );
                            for frame in samples.chunks_exact(ch) {
                                let sum: f32 = frame.iter().copied().sum();
                                ring[ring_pos] = sum / ch as f32;
                                ring_pos = (ring_pos + 1) & (FFT_SIZE - 1);
                            }
                        }
                        SampleKind::Pcm16 => {
                            let samples = std::slice::from_raw_parts(
                                data_ptr as *const i16,
                                frames * ch,
                            );
                            for frame in samples.chunks_exact(ch) {
                                let sum: f32 =
                                    frame.iter().map(|&s| s as f32 / 32768.0).sum();
                                ring[ring_pos] = sum / ch as f32;
                                ring_pos = (ring_pos + 1) & (FFT_SIZE - 1);
                            }
                        }
                    }
                }

                let _ = session.capture.ReleaseBuffer(frames_read);
                got_audio = true;
            }

            if device_error {
                break;
            }

            if got_audio {
                let raw = analyze_spectrum(&ring, ring_pos, &hann, session.sample_rate);
                let max_band = raw.iter().copied().fold(0.0f32, f32::max);

                if max_band < 0.0008 {
                    // Truly silent frame: smoothly let bars settle to rest.
                    for s in &mut smoothed {
                        *s *= 0.78;
                        if *s < 0.01 {
                            *s = 0.0;
                        }
                    }
                } else {
                    // Adaptive gain so bars respond vividly at any Windows volume.
                    running_peak = (running_peak * 0.990).max(max_band).max(0.015);
                    for i in 0..EQ_BANDS {
                        // Gentle perceptual compression so beats punch cleanly.
                        let norm = (raw[i] / running_peak).clamp(0.0, 1.0).powf(0.75);
                        let alpha = if norm > smoothed[i] { 0.62 } else { 0.22 };
                        smoothed[i] += (norm - smoothed[i]) * alpha;
                    }
                }
                shared.set_levels(smoothed);
            } else {
                // No new packet this tick (e.g. player momentarily silent): decay smoothly.
                for s in &mut smoothed {
                    *s *= 0.85;
                    if *s < 0.01 {
                        *s = 0.0;
                    }
                }
                shared.set_levels(smoothed);
            }

            thread::sleep(Duration::from_millis(15));
        }

        session.close();
    }

    if com_ok {
        CoUninitialize();
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

/// Computes 5 musically spaced band magnitudes via a 512-point radix-2 FFT.
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
    // Musical band edges in Hz: Sub-Bass, Bass/Kick, Low-Mid/Vocals, Presence, Treble.
    const EDGES_HZ: [(f32, f32, f32); EQ_BANDS] = [
        (40.0, 180.0, 0.85),    // Sub-bass / kick
        (180.0, 480.0, 1.00),   // Punchy bass
        (480.0, 1400.0, 1.25),  // Vocals / snare
        (1400.0, 3600.0, 1.55), // Melody / presence
        (3600.0, 9500.0, 1.95), // Treble / hi-hats
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

/// In-place iterative radix-2 Cooley-Tukey FFT (`FFT_SIZE` = power of 2).
fn fft_in_place(re: &mut [f32; FFT_SIZE], im: &mut [f32; FFT_SIZE]) {
    // Bit-reversal permutation
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
