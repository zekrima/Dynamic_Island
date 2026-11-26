//! Event-driven Linux desktop notification monitor (Pillar-style).
//!
//! Monitors D-Bus `org.freedesktop.Notifications.Notify` calls via
//! `dbus-monitor --session` so every notification (Discord, Telegram, Chrome,
//! Firefox, Spotify, `notify-send`, etc.) appears smoothly in Zekrima without
//! replacing or conflicting with KDE Plasma / SwayNC / Dunst / Mako.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const NOTIFICATION_TOAST_DURATION: Duration = Duration::from_millis(3800);
const MAX_NOTIFICATIONS: usize = 12;

#[derive(Clone, Debug, Default)]
pub struct NotificationItem {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
    pub timestamp_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct NotificationSnapshot {
    pub items: Vec<NotificationItem>,
    pub toast: Option<NotificationItem>,
    pub toast_started_at: Option<Instant>,
    pub toast_until: Option<Instant>,
}

impl NotificationSnapshot {
    pub fn is_toast_visible(&self, now: Instant) -> bool {
        self.toast.is_some() && self.toast_until.map(|t| now < t).unwrap_or(false)
    }

    pub fn toast_progress(&self, now: Instant) -> f32 {
        let (Some(start), Some(end)) = (self.toast_started_at, self.toast_until) else {
            return 0.0;
        };
        let total = end.saturating_duration_since(start).as_secs_f32();
        if total <= 0.001 {
            return 0.0;
        }
        let remaining = end.saturating_duration_since(now).as_secs_f32();
        (remaining / total).clamp(0.0, 1.0)
    }

    pub fn unread_count(&self) -> usize {
        self.items.len()
    }

    pub fn latest_app_name(&self) -> Option<&str> {
        self.items.first().map(|item| item.app.as_str())
    }
}

pub struct NotificationService {
    shared: Arc<Mutex<NotificationSnapshot>>,
    shutdown: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    next_id: Arc<AtomicU32>,
    on_update: Arc<dyn Fn() + Send + Sync>,
    worker: Option<JoinHandle<()>>,
}

impl NotificationService {
    pub fn start(on_update: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Mutex::new(NotificationSnapshot::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let child_pid = Arc::new(Mutex::new(None));
        let next_id = Arc::new(AtomicU32::new(1));

        let w_shared = Arc::clone(&shared);
        let w_shutdown = Arc::clone(&shutdown);
        let w_pid = Arc::clone(&child_pid);
        let w_id = Arc::clone(&next_id);
        let w_cb = Arc::clone(&on_update);

        let worker = thread::Builder::new()
            .name("zekrima-notifications".to_owned())
            .spawn(move || notification_worker(w_shared, w_shutdown, w_pid, w_id, w_cb))
            .ok();

        Self {
            shared,
            shutdown,
            child_pid,
            next_id,
            on_update,
            worker,
        }
    }

    pub fn snapshot(&self) -> NotificationSnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn push_manual(&self, app: &str, title: &str, body: &str) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let item = NotificationItem {
            id,
            app: app.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            timestamp_ms: now_ms(),
        };
        push_notification(&self.shared, item);
        (self.on_update)();
    }

    pub fn dismiss(&self, id: u32) {
        {
            let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.items.retain(|item| item.id != id);
            if guard.toast.as_ref().map(|t| t.id) == Some(id) {
                guard.toast = None;
                guard.toast_started_at = None;
                guard.toast_until = None;
            }
        }
        (self.on_update)();
    }

    pub fn clear_toast(&self) {
        {
            let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.toast = None;
            guard.toast_started_at = None;
            guard.toast_until = None;
        }
        (self.on_update)();
    }

    pub fn clear_all(&self) {
        {
            let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.items.clear();
            guard.toast = None;
            guard.toast_started_at = None;
            guard.toast_until = None;
        }
        (self.on_update)();
    }
}

impl Drop for NotificationService {
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

fn push_notification(shared: &Arc<Mutex<NotificationSnapshot>>, item: NotificationItem) {
    let now = Instant::now();
    let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
    guard.items.insert(0, item.clone());
    guard.items.truncate(MAX_NOTIFICATIONS);
    guard.toast = Some(item);
    guard.toast_started_at = Some(now);
    guard.toast_until = Some(now + NOTIFICATION_TOAST_DURATION);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn notification_worker(
    shared: Arc<Mutex<NotificationSnapshot>>,
    shutdown: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    next_id: Arc<AtomicU32>,
    on_update: Arc<dyn Fn() + Send + Sync>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        let child = Command::new("dbus-monitor")
            .args([
                "--session",
                "type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        let Ok(mut child) = child else {
            thread::sleep(Duration::from_secs(5));
            continue;
        };

        *child_pid.lock().unwrap_or_else(|p| p.into_inner()) = Some(child.id());

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            // Notify signature: (s app_name, u replaces_id, s app_icon, s summary, s body, ...)
            let mut in_notify = false;
            let mut strings: Vec<String> = Vec::new();
            let mut multi_line: Option<String> = None;

            for line in reader.lines() {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(line) = line else { break };

                if let Some(ref mut acc) = multi_line {
                    if let Some(end) = line.rfind('"') {
                        acc.push(' ');
                        acc.push_str(line[..end].trim());
                        let done = multi_line.take().unwrap();
                        strings.push(done);
                        if strings.len() >= 4 {
                            emit_if_valid(&shared, &next_id, &on_update, &strings);
                            in_notify = false;
                            strings.clear();
                        }
                    } else {
                        acc.push(' ');
                        acc.push_str(line.trim());
                    }
                    continue;
                }

                let trimmed = line.trim();
                if trimmed.starts_with("method call ") && trimmed.contains("member=Notify") {
                    in_notify = true;
                    strings.clear();
                    continue;
                }

                if !in_notify {
                    continue;
                }

                // Only parse top-level string arguments (indented with 3 spaces in dbus-monitor).
                if line.starts_with("   string \"") {
                    let rest = &line["   string \"".len()..];
                    if let Some(end) = rest.rfind('"') {
                        strings.push(unescape_dbus_str(&rest[..end]));
                        if strings.len() >= 4 {
                            emit_if_valid(&shared, &next_id, &on_update, &strings);
                            in_notify = false;
                            strings.clear();
                        }
                    } else {
                        multi_line = Some(rest.to_string());
                    }
                } else if trimmed.starts_with("array [") {
                    // Reached actions array; flush whatever strings we collected.
                    if strings.len() >= 3 {
                        emit_if_valid(&shared, &next_id, &on_update, &strings);
                    }
                    in_notify = false;
                    strings.clear();
                }
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        *child_pid.lock().unwrap_or_else(|p| p.into_inner()) = None;

        if !shutdown.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(3));
        }
    }
}

fn emit_if_valid(
    shared: &Arc<Mutex<NotificationSnapshot>>,
    next_id: &Arc<AtomicU32>,
    on_update: &Arc<dyn Fn() + Send + Sync>,
    strings: &[String],
) {
    // strings: [0]=app_name, [1]=app_icon, [2]=summary (title), [3]=body
    let app = strings
        .first()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("Notification")
        .to_string();
    let title = strings
        .get(2)
        .map(|s| s.trim())
        .unwrap_or("")
        .to_string();
    let body = strings
        .get(3)
        .map(|s| strip_markup(s.trim()))
        .unwrap_or_default();

    if title.is_empty() && body.is_empty() {
        return;
    }

    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let item = NotificationItem {
        id,
        app,
        title,
        body,
        timestamp_ms: now_ms(),
    };
    push_notification(shared, item);
    on_update();
}

fn unescape_dbus_str(s: &str) -> String {
    s.replace("\\n", " ")
        .replace("\\r", "")
        .replace("\\t", " ")
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

/// Strips simple HTML/Pango tags some Linux apps embed in notification bodies.
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .trim()
        .to_string()
}
