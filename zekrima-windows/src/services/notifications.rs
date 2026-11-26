//! Windows notification mirroring & interception modeled after Pillar.
//!
//! Features:
//! - Uses `UserNotificationListener::Current()` + `RequestAccessAsync()` (with
//!   timeout) so desktop apps with Windows notification access enabled work
//!   reliably.
//! - Subscribes to `NotificationChanged` with retry for startup races
//!   (`0x80070490`). When real-time events fire for `Added`, reads the full
//!   notification (app name, title, body, timestamp, AUMID) and suppresses the
//!   duplicate native Windows banner via `RemoveNotification`.
//! - If `NotificationChanged` is unavailable on the system (common `0x80070490`
//!   on some unpackaged builds even when `GetNotificationsAsync` works), falls
//!   back automatically to lightweight periodic polling so notifications still
//!   work seamlessly.
//! - Supports activating the notifying app on click via
//!   `AllowSetForegroundWindow` + `ShellExecuteW("shell:AppsFolder\\<AUMID>")`,
//!   dismissing individual or all notifications, and opening Windows
//!   Notification Privacy Settings directly.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::{w, HSTRING, Result};
use windows::Foundation::{AsyncStatus, TypedEventHandler};
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{
    NotificationKinds, UserNotification, UserNotificationChangedEventArgs,
    UserNotificationChangedKind,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, PostMessageW, ASFW_ANY, SW_SHOWNORMAL,
};

pub const WM_NOTIFICATION_UPDATED: u32 = 0x8003;
/// How long an incoming notification toast stays expanded before absorbing into
/// the collapsed pill badge (matches Pillar's ~3.8 s cadence).
pub const NOTIFICATION_TOAST_DURATION: Duration = Duration::from_millis(3800);
/// Maximum active notifications kept in the island list.
const MAX_NOTIFICATIONS: usize = 12;
/// Polling interval when `NotificationChanged` returns `0x80070490`.
const FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(1800);
/// Slow re-check interval while waiting for the user to enable Windows access.
const ACCESS_RECHECK_INTERVAL: Duration = Duration::from_secs(12);

const E_ELEMENT_NOT_FOUND: i32 = 0x80070490u32 as i32;

#[derive(Clone, Debug, Default)]
pub struct NotificationItem {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
    pub timestamp_ms: u64,
    pub aumid: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NotificationSnapshot {
    pub access_granted: bool,
    /// Active notifications (newest first) shown in the badge and expanded list.
    pub items: Vec<NotificationItem>,
    /// Currently popping incoming toast notification (absorbs into badge after duration).
    pub toast: Option<NotificationItem>,
    pub toast_started_at: Option<Instant>,
    pub toast_until: Option<Instant>,
}

impl NotificationSnapshot {
    pub fn is_toast_visible(&self, now: Instant) -> bool {
        self.toast.is_some() && self.toast_until.map(|t| now < t).unwrap_or(false)
    }

    /// Remaining toast countdown progress in `0.0..=1.0` (`1.0` at start, `0.0` at end).
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

enum Command {
    Intercepted(NotificationItem),
    Refresh,
    Dismiss(u32),
    ClearAll,
    Activate(u32),
    RecheckAccess,
    Shutdown,
}

pub struct NotificationService {
    shared: Arc<Mutex<NotificationSnapshot>>,
    commands: mpsc::Sender<Command>,
    worker: Option<JoinHandle<()>>,
    hwnd: HWND,
}

impl NotificationService {
    pub fn start(hwnd: HWND) -> Self {
        let shared = Arc::new(Mutex::new(NotificationSnapshot::default()));
        let (commands, receiver) = mpsc::channel();

        let worker_shared = Arc::clone(&shared);
        let worker_commands = commands.clone();
        let hwnd_raw = hwnd.0 as usize;

        let worker = thread::Builder::new()
            .name("zekrima-notifications".to_owned())
            .spawn(move || unsafe {
                notification_worker(hwnd_raw, worker_shared, worker_commands, receiver);
            })
            .ok();

        Self {
            shared,
            commands,
            worker,
            hwnd,
        }
    }

    pub fn snapshot(&self) -> NotificationSnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Dismisses a notification from Zekrima and Windows Action Center.
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
        let _ = self.commands.send(Command::Dismiss(id));
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_NOTIFICATION_UPDATED, WPARAM(0), LPARAM(0));
        }
    }

    /// Hides the active incoming toast early (absorbs into badge without deleting from list).
    pub fn clear_toast(&self) {
        {
            let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.toast = None;
            guard.toast_started_at = None;
            guard.toast_until = None;
        }
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_NOTIFICATION_UPDATED, WPARAM(0), LPARAM(0));
        }
    }

    /// Clears all notifications from Zekrima and Windows Action Center.
    pub fn clear_all(&self) {
        {
            let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.items.clear();
            guard.toast = None;
            guard.toast_started_at = None;
            guard.toast_until = None;
        }
        let _ = self.commands.send(Command::ClearAll);
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_NOTIFICATION_UPDATED, WPARAM(0), LPARAM(0));
        }
    }

    /// Activates (brings to foreground) the app that created notification `id`,
    /// then dismisses it from the island just like Pillar / Action Center.
    pub fn activate(&self, id: u32) {
        let _ = self.commands.send(Command::Activate(id));
        self.dismiss(id);
    }

    /// Pushes a local in-app toast (e.g. Pomodoro completion alert) directly
    /// into the Pillar toast UI without requiring external notification access.
    pub fn push_local_toast(&self, app: &str, title: &str, body: &str) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let id = 0x8000_0000u32 | ((now_ms as u32) & 0x7FFF_FFFF);
        let item = NotificationItem {
            id,
            app: app.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            timestamp_ms: now_ms,
            aumid: None,
        };
        let now = Instant::now();
        {
            let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            guard.items.insert(0, item.clone());
            guard.items.truncate(MAX_NOTIFICATIONS);
            guard.toast = Some(item);
            guard.toast_started_at = Some(now);
            guard.toast_until = Some(now + NOTIFICATION_TOAST_DURATION);
        }
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_NOTIFICATION_UPDATED, WPARAM(0), LPARAM(0));
        }
    }

    /// Opens Windows Settings -> Privacy & security -> Notifications and queues
    /// an access re-check so permissions take effect without restarting.
    pub fn open_windows_settings(&self) {
        unsafe {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = ShellExecuteW(
                None,
                w!("open"),
                w!("ms-settings:privacy-notifications"),
                None,
                None,
                SW_SHOWNORMAL,
            );
        }
        let _ = self.commands.send(Command::RecheckAccess);
    }
}

impl Drop for NotificationService {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

unsafe fn notification_worker(
    hwnd_raw: usize,
    shared: Arc<Mutex<NotificationSnapshot>>,
    commands: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Command>,
) {
    if RoInitialize(RO_INIT_MULTITHREADED).is_err() {
        return;
    }

    let hwnd = HWND(hwnd_raw as *mut c_void);
    let mut seen_ids: HashSet<u32> = HashSet::new();
    let mut listener: Option<UserNotificationListener> = None;
    let mut event_token = None;
    let mut access_granted = false;
    let mut real_time_subscribed = false;

    // Initial attempt to acquire listener + access.
    setup_listener(
        hwnd,
        &shared,
        &commands,
        &mut listener,
        &mut event_token,
        &mut access_granted,
        &mut real_time_subscribed,
        &mut seen_ids,
    );

    loop {
        let cmd = if !access_granted {
            match receiver.recv_timeout(ACCESS_RECHECK_INTERVAL) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => Command::RecheckAccess,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else if real_time_subscribed {
            // Event-driven: zero polling, 0.0% idle CPU.
            match receiver.recv() {
                Ok(c) => c,
                Err(_) => break,
            }
        } else {
            // `NotificationChanged` unavailable (e.g. 0x80070490 on unpackaged build):
            // lightweight fallback poll like Pillar.
            match receiver.recv_timeout(FALLBACK_POLL_INTERVAL) {
                Ok(c) => c,
                Err(mpsc::RecvTimeoutError::Timeout) => Command::Refresh,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };

        match cmd {
            Command::Intercepted(item) => {
                if seen_ids.len() > 512 {
                    seen_ids.clear();
                }
                let is_new = seen_ids.insert(item.id);
                if is_new {
                    let now = Instant::now();
                    {
                        let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
                        guard.items.retain(|existing| existing.id != item.id);
                        guard.items.insert(0, item.clone());
                        guard.items.truncate(MAX_NOTIFICATIONS);
                        guard.toast = Some(item);
                        guard.toast_started_at = Some(now);
                        guard.toast_until = Some(now + NOTIFICATION_TOAST_DURATION);
                    }
                    let _ = PostMessageW(hwnd, WM_NOTIFICATION_UPDATED, WPARAM(0), LPARAM(0));
                }
            }
            Command::Refresh => {
                if let Some(ref l) = listener {
                    sync_from_windows(hwnd, l, &shared, &mut seen_ids, true);
                }
            }
            Command::Dismiss(id) => {
                if let Some(ref l) = listener {
                    let _ = l.RemoveNotification(id);
                }
            }
            Command::ClearAll => {
                if let Some(ref l) = listener {
                    if let Ok(items) = read_notifications(l) {
                        for item in items {
                            let _ = l.RemoveNotification(item.id);
                        }
                    }
                }
            }
            Command::Activate(id) => {
                let aumid = {
                    let guard = shared.lock().unwrap_or_else(|p| p.into_inner());
                    guard
                        .items
                        .iter()
                        .find(|item| item.id == id)
                        .and_then(|item| item.aumid.clone())
                        .or_else(|| {
                            guard
                                .toast
                                .as_ref()
                                .filter(|t| t.id == id)
                                .and_then(|t| t.aumid.clone())
                        })
                };

                let aumid = aumid.or_else(|| {
                    listener.as_ref().and_then(|l| {
                        read_notifications(l)
                            .ok()?
                            .into_iter()
                            .find(|n| n.id == id)
                            .and_then(|n| n.aumid)
                    })
                });

                if let Some(aumid) = aumid {
                    activate_app_by_aumid(&aumid);
                }
            }
            Command::RecheckAccess => {
                if !access_granted {
                    setup_listener(
                        hwnd,
                        &shared,
                        &commands,
                        &mut listener,
                        &mut event_token,
                        &mut access_granted,
                        &mut real_time_subscribed,
                        &mut seen_ids,
                    );
                }
            }
            Command::Shutdown => break,
        }
    }

    if let (Some(ref l), Some(token)) = (&listener, event_token.take()) {
        let _ = l.RemoveNotificationChanged(token);
    }
    drop(listener);
    RoUninitialize();
}

unsafe fn setup_listener(
    hwnd: HWND,
    shared: &Arc<Mutex<NotificationSnapshot>>,
    commands: &mpsc::Sender<Command>,
    listener_slot: &mut Option<UserNotificationListener>,
    token_slot: &mut Option<windows::Foundation::EventRegistrationToken>,
    access_granted: &mut bool,
    real_time_subscribed: &mut bool,
    seen_ids: &mut HashSet<u32>,
) {
    let Ok(l) = UserNotificationListener::Current() else {
        return;
    };

    let allowed = request_access_with_timeout(&l);
    *access_granted = allowed;
    {
        let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());
        guard.access_granted = allowed;
    }
    let _ = PostMessageW(hwnd, WM_NOTIFICATION_UPDATED, WPARAM(0), LPARAM(0));

    if !allowed {
        return;
    }

    // Seed initial Action Center notifications into the list without popping
    // toasts for already-existing items.
    sync_from_windows(hwnd, &l, shared, seen_ids, false);

    // Subscribe to real-time `NotificationChanged` (with Pillar's 3-retry logic
    // for transient `0x80070490` startup races).
    if let Some(token) = subscribe_notification_changed(&l, commands) {
        *token_slot = Some(token);
        *real_time_subscribed = true;
    } else {
        *real_time_subscribed = false;
    }

    *listener_slot = Some(l);
}

fn subscribe_notification_changed(
    listener: &UserNotificationListener,
    commands: &mpsc::Sender<Command>,
) -> Option<windows::Foundation::EventRegistrationToken> {
    const RETRIES: usize = 3;
    const RETRY_DELAY: Duration = Duration::from_millis(500);

    for attempt in 1..=RETRIES {
        let tx = commands.clone();
        let handler = TypedEventHandler::new(
            move |_sender: &Option<UserNotificationListener>,
                  args: &Option<UserNotificationChangedEventArgs>| {
                if let Some(args) = args {
                    if let Ok(UserNotificationChangedKind::Added) = args.ChangeKind() {
                        if let Ok(notif_id) = args.UserNotificationId() {
                            if let Ok(l) = UserNotificationListener::Current() {
                                if let Ok(items) = read_notifications(&l) {
                                    if let Some(item) = items.into_iter().find(|n| n.id == notif_id)
                                    {
                                        let _ = tx.send(Command::Intercepted(item));
                                        // Suppress duplicate native Windows toast banner
                                        // (exact Pillar interception behavior).
                                        let _ = l.RemoveNotification(notif_id);
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                }
                let _ = tx.send(Command::Refresh);
                Ok(())
            },
        );

        match listener.NotificationChanged(&handler) {
            Ok(token) => return Some(token),
            Err(err) => {
                if err.code().0 == E_ELEMENT_NOT_FOUND && attempt < RETRIES {
                    thread::sleep(RETRY_DELAY);
                    continue;
                }
                return None;
            }
        }
    }

    None
}

unsafe fn sync_from_windows(
    hwnd: HWND,
    listener: &UserNotificationListener,
    shared: &Arc<Mutex<NotificationSnapshot>>,
    seen_ids: &mut HashSet<u32>,
    trigger_toast: bool,
) {
    let Ok(mut fetched) = read_notifications(listener) else {
        return;
    };
    fetched.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms).then(b.id.cmp(&a.id)));

    let newest_unseen = if trigger_toast {
        fetched
            .iter()
            .find(|item| !seen_ids.contains(&item.id))
            .cloned()
    } else {
        None
    };

    if seen_ids.len() > 512 {
        seen_ids.clear();
    }
    for item in &fetched {
        seen_ids.insert(item.id);
    }

    let mut changed = false;
    {
        let mut guard = shared.lock().unwrap_or_else(|p| p.into_inner());

        // Merge Windows notifications with any already-intercepted items.
        if !fetched.is_empty() || !trigger_toast {
            let mut merged: Vec<NotificationItem> = Vec::with_capacity(MAX_NOTIFICATIONS);
            for item in fetched.into_iter().take(MAX_NOTIFICATIONS) {
                if !merged.iter().any(|m| m.id == item.id) {
                    merged.push(item);
                }
            }
            for existing in &guard.items {
                if merged.len() >= MAX_NOTIFICATIONS {
                    break;
                }
                if !merged.iter().any(|m| m.id == existing.id) {
                    merged.push(existing.clone());
                }
            }
            merged.sort_by(|a, b| b.timestamp_ms.cmp(&a.timestamp_ms).then(b.id.cmp(&a.id)));
            merged.truncate(MAX_NOTIFICATIONS);

            if guard.items.len() != merged.len()
                || guard
                    .items
                    .iter()
                    .zip(merged.iter())
                    .any(|(a, b)| a.id != b.id)
            {
                guard.items = merged;
                changed = true;
            }
        }

        if let Some(new_item) = newest_unseen {
            let now = Instant::now();
            guard.toast = Some(new_item);
            guard.toast_started_at = Some(now);
            guard.toast_until = Some(now + NOTIFICATION_TOAST_DURATION);
            changed = true;
        }
    }

    if changed {
        let _ = PostMessageW(hwnd, WM_NOTIFICATION_UPDATED, WPARAM(0), LPARAM(0));
    }
}

fn request_access_with_timeout(listener: &UserNotificationListener) -> bool {
    let Ok(op) = listener.RequestAccessAsync() else {
        return false;
    };

    for _ in 0..100 {
        match op.Status() {
            Ok(AsyncStatus::Completed) => {
                return matches!(
                    op.GetResults(),
                    Ok(UserNotificationListenerAccessStatus::Allowed)
                );
            }
            Ok(AsyncStatus::Error) | Ok(AsyncStatus::Canceled) | Err(_) => return false,
            _ => thread::sleep(Duration::from_millis(30)),
        }
    }
    false
}

fn read_notifications(listener: &UserNotificationListener) -> Result<Vec<NotificationItem>> {
    let op = listener.GetNotificationsAsync(NotificationKinds::Toast)?;
    for _ in 0..100 {
        match op.Status()? {
            AsyncStatus::Completed => {
                let notifs = op.GetResults()?;
                let count = notifs.Size()? as usize;
                let mut out = Vec::with_capacity(count.min(MAX_NOTIFICATIONS));
                for i in 0..count {
                    if let Ok(notif) = notifs.GetAt(i as u32) {
                        if let Some(item) = extract_notification(&notif, i) {
                            out.push(item);
                        }
                    }
                }
                return Ok(out);
            }
            AsyncStatus::Error | AsyncStatus::Canceled => {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(-1)));
            }
            _ => thread::sleep(Duration::from_millis(20)),
        }
    }
    Err(windows::core::Error::from_hresult(windows::core::HRESULT(-1)))
}

fn extract_notification(notif: &UserNotification, idx: usize) -> Option<NotificationItem> {
    let id = notif.Id().ok()?;

    let app = notif
        .AppInfo()
        .ok()
        .and_then(|a| a.DisplayInfo().ok())
        .and_then(|d| d.DisplayName().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Windows App".to_string());

    let aumid = notif
        .AppInfo()
        .ok()
        .and_then(|a| a.AppUserModelId().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    let toast = notif.Notification().ok()?;
    let visual = toast.Visual().ok()?;
    let bindings = visual.Bindings().ok()?;
    let binding_count = bindings.Size().ok()?;

    let mut title = String::new();
    let mut body = String::new();
    for i in 0..binding_count {
        if let Ok(binding) = bindings.GetAt(i) {
            if let Ok(elements) = binding.GetTextElements() {
                if let Ok(n) = elements.Size() {
                    for j in 0..n {
                        if let Ok(t) = elements.GetAt(j) {
                            if let Ok(s) = t.Text() {
                                let text = s.to_string();
                                let trimmed = text.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }
                                if title.is_empty() {
                                    title = trimmed.to_string();
                                } else if body.is_empty() {
                                    body = trimmed.to_string();
                                } else {
                                    body.push_str(" • ");
                                    body.push_str(trimmed);
                                }
                            }
                        }
                    }
                }
            }
            if !title.is_empty() || !body.is_empty() {
                break;
            }
        }
    }

    if title.is_empty() && body.is_empty() {
        return None;
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let timestamp_ms = notif
        .CreationTime()
        .ok()
        .map(|dt| {
            const EPOCH_OFFSET_100NS: i64 = 116_444_736_000_000_000;
            ((dt.UniversalTime - EPOCH_OFFSET_100NS) / 10_000) as u64
        })
        .filter(|&t| t > 0 && t < now_ms + 86_400_000)
        .unwrap_or_else(|| now_ms.saturating_sub(idx as u64 * 60_000));

    Some(NotificationItem {
        id,
        app,
        title,
        body,
        timestamp_ms,
        aumid,
    })
}

/// Brings the notifying application to the foreground using its AUMID, matching
/// Windows Action Center and Pillar (`shell:AppsFolder\<AUMID>`).
unsafe fn activate_app_by_aumid(aumid: &str) {
    if aumid.is_empty() {
        return;
    }

    let _ = AllowSetForegroundWindow(ASFW_ANY);

    let shell_path = HSTRING::from(format!("shell:AppsFolder\\{aumid}"));
    let result = ShellExecuteW(
        None,
        w!("open"),
        &shell_path,
        None,
        None,
        SW_SHOWNORMAL,
    );
    if result.0 as isize > 32 {
        return;
    }

    let _ = ShellExecuteW(
        None,
        w!("open"),
        w!("explorer.exe"),
        &shell_path,
        None,
        SW_SHOWNORMAL,
    );
}
