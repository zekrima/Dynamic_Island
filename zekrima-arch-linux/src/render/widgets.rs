//! Shared widget geometry, Pillar-style notification layout, and hit-targets.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandedTab {
    Media,
    Notifications,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitTarget {
    MediaControl(MediaControl),
    ToastDismiss(u32),
    ToastClear(u32),
    TabMedia,
    TabNotifications,
    ClearAllNotifications,
    NotifDismiss(u32),
}

pub const COLLAPSED_ART: RectF = RectF::new(10.0, 7.0, 30.0, 27.0);
pub const EXPANDED_ART: RectF = RectF::new(16.0, 24.0, 88.0, 96.0);

pub const TITLE_BOX: RectF = RectF::new(104.0, 15.0, 344.0, 43.0);
pub const ARTIST_BOX: RectF = RectF::new(104.0, 43.0, 422.0, 67.0);

pub const CONTROL_SIZE: f32 = 30.0;
pub const CONTROL_GAP: f32 = 8.0;
pub const CONTROLS_TOP: f32 = 76.0;

pub const EQUALIZER_BAR_COUNT: usize = 5;
pub const EQUALIZER_BAR_WIDTH: f32 = 3.0;
pub const EQUALIZER_BAR_GAP: f32 = 2.5;
pub const EQUALIZER_RIGHT: f32 = 164.0;

pub const EXPANDED_NOTIFS_BTN: RectF =
    RectF::new(EXPANDED_WIDTH - 88.0, 14.0, EXPANDED_WIDTH - 16.0, 36.0);

pub const TOAST_AVATAR: RectF = RectF::new(16.0, 22.0, 52.0, 58.0);
pub const TOAST_DISMISS_BTN: RectF =
    RectF::new(NOTIFICATION_WIDTH - 38.0, 12.0, NOTIFICATION_WIDTH - 12.0, 38.0);
pub const TOAST_PROGRESS_RECT: RectF = RectF::new(
    22.0,
    NOTIFICATION_HEIGHT - 6.0,
    NOTIFICATION_WIDTH - 22.0,
    NOTIFICATION_HEIGHT - 3.5,
);

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

pub fn notif_card_rect(index: usize) -> RectF {
    let top = 42.0 + index as f32 * 56.0;
    RectF::new(16.0, top, EXPANDED_NOTIFS_WIDTH - 16.0, top + 50.0)
}

pub fn notif_card_dismiss_rect(index: usize) -> RectF {
    let card = notif_card_rect(index);
    RectF::new(
        card.right - 30.0,
        card.top + 8.0,
        card.right - 6.0,
        card.top + 32.0,
    )
}

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

pub fn expansion_progress(height: f32) -> f32 {
    ((height - COLLAPSED_HEIGHT) / (EXPANDED_HEIGHT - COLLAPSED_HEIGHT)).clamp(0.0, 1.0)
}

pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub struct HitContext<'a> {
    pub pill_height: f32,
    pub expanded: bool,
    pub expanded_tab: ExpandedTab,
    pub has_media: bool,
    pub hud_visible: bool,
    pub toast_visible: bool,
    pub notifications: &'a NotificationSnapshot,
}

pub fn hit_target_at(x: f32, y: f32, ctx: &HitContext) -> Option<HitTarget> {
    if ctx.hud_visible {
        return None;
    }

    if ctx.toast_visible && !ctx.expanded {
        if let Some(toast) = &ctx.notifications.toast {
            if TOAST_DISMISS_BTN.contains(x, y) {
                return Some(HitTarget::ToastDismiss(toast.id));
            }
            return Some(HitTarget::ToastClear(toast.id));
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

            for (index, item) in ctx
                .notifications
                .items
                .iter()
                .take(NOTIF_CARD_MAX_VISIBLE)
                .enumerate()
            {
                if notif_card_dismiss_rect(index).contains(x, y)
                    || notif_card_rect(index).contains(x, y)
                {
                    return Some(HitTarget::NotifDismiss(item.id));
                }
            }
            None
        }
    }
}

pub fn app_accent_rgb(app_name: &str) -> (f64, f64, f64) {
    const PALETTE: [(f64, f64, f64); 6] = [
        (59.0 / 255.0, 130.0 / 255.0, 246.0 / 255.0),
        (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0),
        (168.0 / 255.0, 85.0 / 255.0, 247.0 / 255.0),
        (236.0 / 255.0, 72.0 / 255.0, 153.0 / 255.0),
        (249.0 / 255.0, 115.0 / 255.0, 22.0 / 255.0),
        (6.0 / 255.0, 182.0 / 255.0, 212.0 / 255.0),
    ];

    let mut hash: i32 = 0;
    for unit in app_name.encode_utf16() {
        hash = (unit as i32).wrapping_add(hash.wrapping_shl(5).wrapping_sub(hash));
    }
    let idx = (hash.unsigned_abs() as usize) % PALETTE.len();
    PALETTE[idx]
}

pub fn app_initial(app_name: &str) -> String {
    app_name
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "N".to_string())
}

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
