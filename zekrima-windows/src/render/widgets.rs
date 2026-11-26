//! Shared widget geometry, 5-tab navigation bar, Pillar notifications,
//! Pomodoro timer, Clipboard history, System Monitor, and hit-targets.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::render::spring::{
    COLLAPSED_HEIGHT, EXPANDED_HEIGHT, EXPANDED_WIDTH, NOTIFICATION_HEIGHT, NOTIFICATION_WIDTH,
};
use crate::services::media::MediaControl;
use crate::services::notifications::NotificationSnapshot;
use crate::services::pomodoro::TIMER_PRESETS_SECS;

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
}

/// Five productivity tabs in the expanded island header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandedTab {
    Media,
    Timer,
    Clipboard,
    System,
    Notifications,
}

pub const ALL_TABS: [ExpandedTab; 5] = [
    ExpandedTab::Media,
    ExpandedTab::Timer,
    ExpandedTab::Clipboard,
    ExpandedTab::System,
    ExpandedTab::Notifications,
];

/// Interactive hit targets inside the pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitTarget {
    SelectTab(ExpandedTab),
    MediaControl(MediaControl),
    ToastActivate(u32),
    ToastDismiss(u32),
    ClearAllNotifications,
    OpenNotificationSettings,
    NotifActivate(u32),
    NotifDismiss(u32),
    TimerToggle,
    TimerReset,
    TimerPreset(u32),
    ClipCopy(usize),
    ClipClear,
}

// -----------------------------------------------------------------------------
// Top 5-tab bar (y = 11..33, width = 408 across 440px expanded pill)
// -----------------------------------------------------------------------------

pub fn tab_rect(tab: ExpandedTab) -> RectF {
    let idx = match tab {
        ExpandedTab::Media => 0.0,
        ExpandedTab::Timer => 1.0,
        ExpandedTab::Clipboard => 2.0,
        ExpandedTab::System => 3.0,
        ExpandedTab::Notifications => 4.0,
    };
    let pad = 16.0;
    let gap = 6.0;
    let tab_w = (EXPANDED_WIDTH - pad * 2.0 - gap * 4.0) / 5.0; // 76.8
    let left = pad + idx * (tab_w + gap);
    RectF::new(left, 11.0, left + tab_w, 33.0)
}

// -----------------------------------------------------------------------------
// Media tab layout (below top tab bar: y = 42..152)
// -----------------------------------------------------------------------------

pub const COLLAPSED_ART: RectF = RectF::new(10.0, 7.0, 30.0, 27.0);
pub const EXPANDED_ART: RectF = RectF::new(18.0, 48.0, 102.0, 132.0);

pub const TITLE_BOX: RectF = RectF::new(116.0, 46.0, 422.0, 74.0);
pub const ARTIST_BOX: RectF = RectF::new(116.0, 74.0, 422.0, 96.0);

pub const CONTROL_SIZE: f32 = 30.0;
pub const CONTROL_GAP: f32 = 8.0;
pub const CONTROLS_TOP: f32 = 104.0;

pub const EQUALIZER_BAR_COUNT: usize = 5;
pub const EQUALIZER_BAR_WIDTH: f32 = 3.0;
pub const EQUALIZER_BAR_GAP: f32 = 2.5;
pub const EQUALIZER_RIGHT: f32 = 164.0;

// -----------------------------------------------------------------------------
// Incoming notification toast layout (380 x 88)
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
// Notifications tab layout (below top tab bar: y = 40..154)
// -----------------------------------------------------------------------------

pub const NOTIF_CARD_MAX_VISIBLE: usize = 2;
pub const NOTIF_CLEAR_BTN: RectF = RectF::new(EXPANDED_WIDTH - 80.0, 40.0, EXPANDED_WIDTH - 16.0, 58.0);
pub const NOTIF_SETTINGS_BTN: RectF = RectF::new(124.0, 114.0, 316.0, 144.0);

pub fn notif_card_rect(index: usize) -> RectF {
    let top = 64.0 + index as f32 * 45.0;
    RectF::new(16.0, top, EXPANDED_WIDTH - 16.0, top + 41.0)
}

pub fn notif_card_dismiss_rect(index: usize) -> RectF {
    let card = notif_card_rect(index);
    RectF::new(
        card.right - 28.0,
        card.top + 8.0,
        card.right - 6.0,
        card.top + 30.0,
    )
}

// -----------------------------------------------------------------------------
// Pomodoro Timer tab layout (y = 42..154)
// -----------------------------------------------------------------------------

pub const TIMER_TOGGLE_BTN: RectF = RectF::new(248.0, 52.0, 334.0, 82.0);
pub const TIMER_RESET_BTN: RectF = RectF::new(342.0, 52.0, 422.0, 82.0);
pub const TIMER_PROGRESS_BAR: RectF = RectF::new(20.0, 144.0, 420.0, 150.0);

pub fn timer_preset_rect(index: usize) -> RectF {
    let pad = 20.0;
    let gap = 10.0;
    let w = (EXPANDED_WIDTH - pad * 2.0 - gap * 3.0) / 4.0; // 92.5
    let left = pad + index as f32 * (w + gap);
    RectF::new(left, 110.0, left + w, 134.0)
}

// -----------------------------------------------------------------------------
// Clipboard tab layout (y = 40..154)
// -----------------------------------------------------------------------------

pub const CLIP_MAX_VISIBLE: usize = 3;
pub const CLIP_CLEAR_BTN: RectF = RectF::new(EXPANDED_WIDTH - 72.0, 40.0, EXPANDED_WIDTH - 16.0, 58.0);

pub fn clip_row_rect(index: usize) -> RectF {
    let top = 64.0 + index as f32 * 30.0;
    RectF::new(16.0, top, EXPANDED_WIDTH - 16.0, top + 26.0)
}

// -----------------------------------------------------------------------------
// Media control helpers
// -----------------------------------------------------------------------------

const CONTROLS_ACTIVE_HEIGHT: f32 = EXPANDED_HEIGHT - 12.0;

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
    pub clip_count: usize,
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
            return Some(HitTarget::ToastActivate(toast.id));
        }
        return None;
    }

    if !ctx.expanded || !controls_active(ctx.pill_height) {
        return None;
    }

    // Top 5-tab bar works from any expanded tab.
    for tab in ALL_TABS {
        if tab_rect(tab).contains(x, y) {
            return Some(HitTarget::SelectTab(tab));
        }
    }

    match ctx.expanded_tab {
        ExpandedTab::Media => {
            if ctx.has_media {
                if let Some(ctrl) = control_at(x, y) {
                    return Some(HitTarget::MediaControl(ctrl));
                }
            }
            None
        }
        ExpandedTab::Timer => {
            if TIMER_TOGGLE_BTN.contains(x, y) {
                return Some(HitTarget::TimerToggle);
            }
            if TIMER_RESET_BTN.contains(x, y) {
                return Some(HitTarget::TimerReset);
            }
            for (i, &secs) in TIMER_PRESETS_SECS.iter().enumerate() {
                if timer_preset_rect(i).contains(x, y) {
                    return Some(HitTarget::TimerPreset(secs));
                }
            }
            None
        }
        ExpandedTab::Clipboard => {
            if ctx.clip_count > 0 && CLIP_CLEAR_BTN.contains(x, y) {
                return Some(HitTarget::ClipClear);
            }
            for i in 0..ctx.clip_count.min(CLIP_MAX_VISIBLE) {
                if clip_row_rect(i).contains(x, y) {
                    return Some(HitTarget::ClipCopy(i));
                }
            }
            None
        }
        ExpandedTab::System => None,
        ExpandedTab::Notifications => {
            if !ctx.notifications.items.is_empty() && NOTIF_CLEAR_BTN.contains(x, y) {
                return Some(HitTarget::ClearAllNotifications);
            }
            if !ctx.notifications.access_granted && ctx.notifications.items.is_empty() {
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

pub fn app_accent_rgb(app_name: &str) -> (f32, f32, f32) {
    const PALETTE: [(f32, f32, f32); 6] = [
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
