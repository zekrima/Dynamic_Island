//! Shared widget geometry, Pillar-style notification layout, and hit-targets.
//!
//! All rectangles are in pill-local logical pixels: `(0, 0)` is the top-left
//! corner of the animated pill. The renderer adds the pill origin before
//! drawing, and hit testing converts client pixels back into this space.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::render::spring::{
    COLLAPSED_HEIGHT, EXPANDED_HEIGHT, EXPANDED_NOTIFS_WIDTH, EXPANDED_WIDTH,
    NOTIFICATION_HEIGHT, NOTIFICATION_WIDTH,
};
use crate::services::media::MediaControl;
use crate::services::notifications::NotificationSnapshot;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl RectF {
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn center(&self) -> (f32, f32) {
        (
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        )
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn lerp(&self, other: &RectF, t: f32) -> RectF {
        RectF {
            left: lerp(self.left, other.left, t),
            top: lerp(self.top, other.top, t),
            right: lerp(self.right, other.right, t),
            bottom: lerp(self.bottom, other.bottom, t),
        }
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Active tab when the pill is expanded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandedTab {
    Media,
    Notifications,
}

/// Interactive hit targets inside the pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitTarget {
    MediaControl(MediaControl),
    ToastActivate(u32),
    ToastDismiss(u32),
    TabMedia,
    TabNotifications,
    ClearAllNotifications,
    OpenNotificationSettings,
    NotifActivate(u32),
    NotifDismiss(u32),
}

// -----------------------------------------------------------------------------
// Media widget layout (pill-local logical pixels)
// -----------------------------------------------------------------------------

/// Miniature album art shown in the collapsed pill.
pub const COLLAPSED_ART: RectF = RectF::new(10.0, 7.0, 30.0, 27.0);
/// Full album art shown in the expanded pill.
pub const EXPANDED_ART: RectF = RectF::new(16.0, 24.0, 88.0, 96.0);

/// Track title text layout box (expanded). Leaves room on the right for the Notifs pill.
pub const TITLE_BOX: RectF = RectF::new(104.0, 15.0, 344.0, 43.0);
/// Artist name text layout box (expanded).
pub const ARTIST_BOX: RectF = RectF::new(104.0, 43.0, 422.0, 67.0);

pub const CONTROL_SIZE: f32 = 30.0;
pub const CONTROL_GAP: f32 = 8.0;
pub const CONTROLS_TOP: f32 = 76.0;

/// Five thin audio-reactive equalizer bars, vertically centered in the pill.
pub const EQUALIZER_BAR_COUNT: usize = 5;
pub const EQUALIZER_BAR_WIDTH: f32 = 3.0;
pub const EQUALIZER_BAR_GAP: f32 = 2.5;
pub const EQUALIZER_RIGHT: f32 = 164.0;

/// Top-right button in expanded media/idle view to switch to Notifications.
pub const EXPANDED_NOTIFS_BTN: RectF =
    RectF::new(EXPANDED_WIDTH - 88.0, 14.0, EXPANDED_WIDTH - 16.0, 36.0);

// -----------------------------------------------------------------------------
// Incoming notification toast layout (380 x 88 logical pixels)
// -----------------------------------------------------------------------------

pub const TOAST_AVATAR: RectF = RectF::new(16.0, 22.0, 52.0, 58.0);
pub const TOAST_DISMISS_BTN: RectF =
    RectF::new(NOTIFICATION_WIDTH - 38.0, 12.0, NOTIFICATION_WIDTH - 12.0, 38.0);
pub const TOAST_PROGRESS_RECT: RectF = RectF::new(
    22.0,
    NOTIFICATION_HEIGHT - 6.0,
    NOTIFICATION_WIDTH - 22.0,
    NOTIFICATION_HEIGHT - 3.5,
);

// -----------------------------------------------------------------------------
// Expanded notifications list layout (440 x 162 logical pixels)
// -----------------------------------------------------------------------------

pub const NOTIF_CARD_MAX_VISIBLE: usize = 2;
pub const NOTIF_CLEAR_BTN: RectF = RectF::new(
    EXPANDED_NOTIFS_WIDTH - 78.0,
    13.0,
    EXPANDED_NOTIFS_WIDTH - 16.0,
    35.0,
);
pub const NOTIF_MEDIA_TAB_BTN: RectF = RectF::new(
    EXPANDED_NOTIFS_WIDTH - 146.0,
    13.0,
    EXPANDED_NOTIFS_WIDTH - 84.0,
    35.0,
);
pub const NOTIF_MEDIA_TAB_BTN_RIGHT: RectF = NOTIF_CLEAR_BTN;
pub const NOTIF_SETTINGS_BTN: RectF = RectF::new(124.0, 110.0, 316.0, 140.0);

pub fn notif_card_rect(index: usize) -> RectF {
    let top = 42.0 + index as f32 * 56.0;
    RectF::new(16.0, top, EXPANDED_NOTIFS_WIDTH - 16.0, top + 50.0)
}

pub fn notif_card_dismiss_rect(index: usize) -> RectF {
    let card = notif_card_rect(index);
    RectF::new(card.right - 30.0, card.top + 8.0, card.right - 6.0, card.top + 32.0)
}

/// The control strip only accepts clicks when the pill is essentially expanded.
const CONTROLS_ACTIVE_HEIGHT: f32 = EXPANDED_HEIGHT - 8.0;

pub fn control_rect(control: MediaControl) -> RectF {
    let index = match control {
        MediaControl::Previous => 0.0,
        MediaControl::PlayPause => 1.0,
        MediaControl::Next => 2.0,
    };
    let left = TITLE_BOX.left + index * (CONTROL_SIZE + CONTROL_GAP);
    RectF::new(
        left,
        CONTROLS_TOP,
        left + CONTROL_SIZE,
        CONTROLS_TOP + CONTROL_SIZE,
    )
}

pub fn control_at(x: f32, y: f32) -> Option<MediaControl> {
    [
        MediaControl::Previous,
        MediaControl::PlayPause,
        MediaControl::Next,
    ]
    .into_iter()
    .find(|control| control_rect(*control).contains(x, y))
}

pub fn controls_active(height: f32) -> bool {
    height >= CONTROLS_ACTIVE_HEIGHT
}

/// Progress of the collapsed -> expanded transition in the range 0..=1.
pub fn expansion_progress(height: f32) -> f32 {
    ((height - COLLAPSED_HEIGHT) / (EXPANDED_HEIGHT - COLLAPSED_HEIGHT)).clamp(0.0, 1.0)
}

/// Classic smoothstep, used to fade the expanded content in.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Context needed to resolve pill-local interactive hit targets.
pub struct HitContext<'a> {
    pub pill_height: f32,
    pub expanded: bool,
    pub expanded_tab: ExpandedTab,
    pub has_media: bool,
    pub hud_visible: bool,
    pub toast_visible: bool,
    pub notifications: &'a NotificationSnapshot,
}

/// Resolves which interactive target (if any) lies at pill-local `(x, y)`.
pub fn hit_target_at(x: f32, y: f32, ctx: &HitContext) -> Option<HitTarget> {
    if ctx.hud_visible {
        return None;
    }

    // Incoming toast card (when not manually expanded).
    if ctx.toast_visible && !ctx.expanded {
        if let Some(toast) = &ctx.notifications.toast {
            if TOAST_DISMISS_BTN.contains(x, y) {
                return Some(HitTarget::ToastDismiss(toast.id));
            }
            return Some(HitTarget::ToastActivate(toast.id));
        }
        return None;
    }

    if !ctx.expanded || !controls_active(ctx.pill_height) {
        return None;
    }

    match ctx.expanded_tab {
        ExpandedTab::Media => {
            if EXPANDED_NOTIFS_BTN.contains(x, y) {
                return Some(HitTarget::TabNotifications);
            }
            if ctx.has_media {
                if let Some(ctrl) = control_at(x, y) {
                    return Some(HitTarget::MediaControl(ctrl));
                }
            }
            None
        }
        ExpandedTab::Notifications => {
            let has_items = !ctx.notifications.items.is_empty();
            if has_items && NOTIF_CLEAR_BTN.contains(x, y) {
                return Some(HitTarget::ClearAllNotifications);
            }
            let media_btn = if has_items {
                NOTIF_MEDIA_TAB_BTN
            } else {
                NOTIF_MEDIA_TAB_BTN_RIGHT
            };
            if media_btn.contains(x, y) {
                return Some(HitTarget::TabMedia);
            }

            if !ctx.notifications.access_granted {
                if NOTIF_SETTINGS_BTN.contains(x, y) {
                    return Some(HitTarget::OpenNotificationSettings);
                }
                return None;
            }

            for (index, item) in ctx
                .notifications
                .items
                .iter()
                .take(NOTIF_CARD_MAX_VISIBLE)
                .enumerate()
            {
                if notif_card_dismiss_rect(index).contains(x, y) {
                    return Some(HitTarget::NotifDismiss(item.id));
                }
                if notif_card_rect(index).contains(x, y) {
                    return Some(HitTarget::NotifActivate(item.id));
                }
            }
            None
        }
    }
}

/// Consistent Pillar accent color palette per app name (blue, green, purple, pink, orange, cyan).
pub fn app_accent_rgb(app_name: &str) -> (f32, f32, f32) {
    const PALETTE: [(f32, f32, f32); 6] = [
        (59.0 / 255.0, 130.0 / 255.0, 246.0 / 255.0), // blue   #3b82f6
        (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0),  // green  #22c55e
        (168.0 / 255.0, 85.0 / 255.0, 247.0 / 255.0), // purple #a855f7
        (236.0 / 255.0, 72.0 / 255.0, 153.0 / 255.0), // pink   #ec4899
        (249.0 / 255.0, 115.0 / 255.0, 22.0 / 255.0), // orange #f97316
        (6.0 / 255.0, 182.0 / 255.0, 212.0 / 255.0),  // cyan   #06b6d4
    ];

    let mut hash: i32 = 0;
    for unit in app_name.encode_utf16() {
        hash = (unit as i32)
            .wrapping_add(hash.wrapping_shl(5).wrapping_sub(hash));
    }
    let idx = (hash.unsigned_abs() as usize) % PALETTE.len();
    PALETTE[idx]
}

/// Uppercase initial letter for an app avatar badge.
pub fn app_initial(app_name: &str) -> String {
    app_name
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "N".to_string())
}

/// Human-friendly relative timestamp (`just now`, `2m ago`, `1h ago`, `3d ago`).
pub fn format_relative_time(timestamp_ms: u64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let diff = now_ms.saturating_sub(timestamp_ms);
    if diff < 60_000 {
        "just now".to_string()
    } else if diff < 3_600_000 {
        format!("{}m ago", diff / 60_000)
    } else if diff < 86_400_000 {
        format!("{}h ago", diff / 3_600_000)
    } else {
        format!("{}d ago", diff / 86_400_000)
    }
}
