//! Event-driven Linux MPRIS2 media controller via `playerctl --follow`.
//!
//! Supports Spotify, Firefox, Chromium, Brave, VLC, mpv, Cider, and any other
//! MPRIS2 media player. Zero polling: `playerctl --follow` sleeps on D-Bus
//! signals until metadata or playback state changes.

use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct MediaInfo {
    pub has_session: bool,
    pub title: String,
    pub artist: String,
    pub is_playing: bool,
    /// Encoded image bytes (PNG/JPEG/WebP) decoded on the GTK main thread.
    pub art_bytes: Option<Arc<Vec<u8>>>,
    pub art_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaControl {
    Previous,
    PlayPause,
    Next,
}

pub struct MediaService {
    shared: Arc<Mutex<MediaInfo>>,
    shutdown: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    worker: Option<JoinHandle<()>>,
}

impl MediaService {
    pub fn start(on_update: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Mutex::new(MediaInfo::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let child_pid = Arc::new(Mutex::new(None));

        let w_shared = Arc::clone(&shared);
        let w_shutdown = Arc::clone(&shutdown);
        let w_pid = Arc::clone(&child_pid);

        let worker = thread::Builder::new()
            .name("zekrima-media".to_owned())
            .spawn(move || media_worker(w_shared, w_shutdown, w_pid, on_update))
            .ok();

        Self {
            shared,
            shutdown,
            child_pid,
            worker,
        }
    }

    pub fn snapshot(&self) -> MediaInfo {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn control(&self, control: MediaControl) {
        let arg = match control {
            MediaControl::Previous => "previous",
            MediaControl::PlayPause => "play-pause",
            MediaControl::Next => "next",
        };
        let _ = Command::new("playerctl")
            .arg(arg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(pid) = self
            .child_pid
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn media_worker(
    shared: Arc<Mutex<MediaInfo>>,
    shutdown: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    on_update: Arc<dyn Fn() + Send + Sync>,
) {
    let mut last_art_url = String::new();
    let mut last_art_bytes: Option<Arc<Vec<u8>>> = None;
    let mut art_revision: u64 = 0;

    while !shutdown.load(Ordering::SeqCst) {
        let child = Command::new("playerctl")
            .args([
                "--follow",
                "metadata",
                "--format",
                "{{status}}\t{{title}}\t{{artist}}\t{{mpris:artUrl}}",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        let Ok(mut child) = child else {
            thread::sleep(Duration::from_secs(3));
            continue;
        };

        *child_pid.lock().unwrap_or_else(|p| p.into_inner()) = Some(child.id());

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(line) = line else { break };
                let parts: Vec<&str> = line.split('\t').collect();
                let status = parts.first().copied().unwrap_or("").trim();
                let title = parts.get(1).copied().unwrap_or("").trim().to_string();
                let artist = parts.get(2).copied().unwrap_or("").trim().to_string();
                let art_url = parts.get(3).copied().unwrap_or("").trim();

                let has_session = matches!(status, "Playing" | "Paused")
                    && (!title.is_empty() || !artist.is_empty());
                let is_playing = status == "Playing" && has_session;

                if has_session && art_url != last_art_url {
                    last_art_url = art_url.to_string();
                    last_art_bytes = load_art_url(art_url).map(Arc::new);
                    art_revision = art_revision.wrapping_add(1);
                } else if !has_session && last_art_bytes.is_some() {
                    last_art_url.clear();
                    last_art_bytes = None;
                    art_revision = art_revision.wrapping_add(1);
                }

                let next = MediaInfo {
                    has_session,
                    title,
                    artist,
                    is_playing,
                    art_bytes: if has_session {
                        last_art_bytes.clone()
                    } else {
                        None
                    },
                    art_revision,
                };

                {
                    let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
                    *guard = next;
                }
                on_update();
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        *child_pid.lock().unwrap_or_else(|p| p.into_inner()) = None;

        if !shutdown.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(2));
        }
    }
}

fn load_art_url(url: &str) -> Option<Vec<u8>> {
    if url.is_empty() {
        return None;
    }
    if let Some(rest) = url.strip_prefix("file://") {
        let decoded = percent_decode(rest);
        return fs::read(decoded).ok().filter(|b| !b.is_empty());
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        let output = Command::new("curl")
            .args([
                "-fsSL",
                "--max-time",
                "4",
                "--max-filesize",
                "5242880",
                url,
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() && !output.stdout.is_empty() {
            return Some(output.stdout);
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
