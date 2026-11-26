//! Native Cairo + PangoCairo renderer for Zekrima on Arch Linux / CachyOS.
//!
//! Full parity with Windows v0.2.0:
//! - Collapsed pill (bold clock, Zekrima / album art, 5-bar PipeWire FFT music
//!   equalizer, running Pomodoro countdown, Pillar notification badge)
//! - Unified 5-tab Expanded Island (`Media`, `Timer`, `Clip`, `Sys`, `Notifs`)
//! - Incoming Pillar notification toast & Volume/Brightness HUD

use std::f64::consts::{FRAC_PI_2, PI};

use chrono::Local;
use gtk4::cairo;
use gtk4::gdk_pixbuf::{self, Pixbuf};
use gtk4::pango;
use gtk4::prelude::*;

use crate::layout::{CANVAS_HEIGHT_LOGICAL, CANVAS_WIDTH_LOGICAL, PILL_TOP_LOGICAL};
use crate::render::widgets::{
    app_accent_rgb, app_initial, clip_row_rect, control_rect, expansion_progress,
    format_relative_time, notif_card_dismiss_rect, notif_card_rect, smoothstep, tab_rect,
    timer_preset_rect, ExpandedTab, HitTarget, RectF, ALL_TABS, ARTIST_BOX, CLIP_CLEAR_BTN,
    CLIP_MAX_VISIBLE, COLLAPSED_ART, CONTROLS_TOP, EQUALIZER_BAR_COUNT, EQUALIZER_BAR_GAP,
    EQUALIZER_BAR_WIDTH, EQUALIZER_RIGHT, EXPANDED_ART, NOTIF_CARD_MAX_VISIBLE, NOTIF_CLEAR_BTN,
    TIMER_PROGRESS_BAR, TIMER_RESET_BTN, TIMER_TOGGLE_BTN, TITLE_BOX, TOAST_AVATAR,
    TOAST_DISMISS_BTN, TOAST_PROGRESS_RECT,
};
use crate::services::media::{MediaControl, MediaInfo};
use crate::services::notifications::{NotificationItem, NotificationSnapshot};
use crate::services::pomodoro::{PomodoroTimer, TIMER_PRESETS_SECS};
use crate::services::sysmon::SystemStats;
use crate::services::system::{HudKind, HudState, SystemSnapshot};

const PAUSED_EQUALIZER: [f32; EQUALIZER_BAR_COUNT] = [0.22, 0.45, 0.30, 0.50, 0.25];
const ACCENT_RED: (f64, f64, f64) = (235.0 / 255.0, 0.0, 40.0 / 255.0);
const BADGE_RED: (f64, f64, f64) = (239.0 / 255.0, 68.0 / 255.0, 68.0 / 255.0);

pub struct Frame<'a> {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub expanded: bool,
    pub expanded_tab: ExpandedTab,
    pub media: &'a MediaInfo,
    pub hovered_target: Option<HitTarget>,
    pub eq_levels: [f32; EQUALIZER_BAR_COUNT],
    pub system: &'a SystemSnapshot,
    pub notification: &'a NotificationSnapshot,
    pub timer: &'a PomodoroTimer,
    pub clip_items: &'a [String],
    pub clip_copied_flash: bool,
    pub sys_stats: &'a SystemStats,
}

pub struct CairoRenderer {
    art_revision: u64,
    art_pixbuf: Option<Pixbuf>,
}

impl CairoRenderer {
    pub fn new() -> Self {
        Self {
            art_revision: 0,
            art_pixbuf: None,
        }
    }

    pub fn prepare_art(&mut self, media: &MediaInfo) {
        if media.art_revision == self.art_revision {
            return;
        }
        self.art_revision = media.art_revision;
        self.art_pixbuf = None;

        if let Some(bytes) = &media.art_bytes {
            let loader = gdk_pixbuf::PixbufLoader::new();
            if loader.write(bytes).is_ok() && loader.close().is_ok() {
                self.art_pixbuf = loader.pixbuf();
            }
        }
    }

    pub fn draw(&mut self, cr: &cairo::Context, frame: &Frame) {
        self.prepare_art(frame.media);

        cr.set_operator(cairo::Operator::Clear);
        let _ = cr.paint();
        cr.set_operator(cairo::Operator::Over);

        let width = frame.width.clamp(1.0, CANVAS_WIDTH_LOGICAL) as f64;
        let height = frame.height.clamp(1.0, CANVAS_HEIGHT_LOGICAL) as f64;
        let radius = (frame.radius as f64).clamp(0.0, width.min(height) / 2.0);
        let left = (CANVAS_WIDTH_LOGICAL as f64 - width) / 2.0;
        let top = PILL_TOP_LOGICAL as f64;

        rounded_rect_path(cr, left, top, width, height, radius);
        cr.set_source_rgba(10.0 / 255.0, 10.0 / 255.0, 13.0 / 255.0, 0.96);
        let _ = cr.fill();

        let now = std::time::Instant::now();
        let hud_visible = frame.system.hud.is_visible(now);
        let toast_visible = frame.notification.is_toast_visible(now);

        let inset = radius.max(14.0);
        if width > inset * 2.0 + 4.0 {
            rounded_rect_path(cr, left + inset, top + 1.0, width - inset * 2.0, 1.5, 0.75);
            if toast_visible && !hud_visible && !frame.expanded {
                if let Some(toast) = &frame.notification.toast {
                    let (r, g, b) = app_accent_rgb(&toast.app);
                    cr.set_source_rgba(r, g, b, 0.65);
                    let _ = cr.fill();
                }
            } else {
                cr.set_source_rgba(ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, 0.42);
                let _ = cr.fill();
            }
        }

        let _ = cr.save();
        rounded_rect_path(
            cr,
            left + 1.0,
            top + 1.0,
            (width - 2.0).max(1.0),
            (height - 2.0).max(1.0),
            (radius - 1.0).max(0.0),
        );
        cr.clip();

        let expanded_progress = expansion_progress(frame.height);

        if hud_visible {
            self.draw_hud(cr, left, top, width, height, &frame.system.hud);
        } else if toast_visible && !frame.expanded {
            if let Some(toast) = &frame.notification.toast {
                let progress = frame.notification.toast_progress(now);
                self.draw_notification_toast(cr, left, top, toast, progress, frame.hovered_target);
            }
        } else if frame.expanded || expanded_progress >= 0.28 {
            let alpha = smoothstep(((expanded_progress - 0.15) / 0.85).clamp(0.0, 1.0)) as f64;
            if alpha > 0.01 {
                draw_tab_bar(cr, left, top, frame, alpha);
                match frame.expanded_tab {
                    ExpandedTab::Media => {
                        if frame.media.has_session {
                            self.draw_expanded_media(cr, left, top, frame, alpha);
                        } else {
                            self.draw_idle_expanded(
                                cr,
                                left,
                                top,
                                width,
                                height,
                                frame.system,
                                frame.notification,
                                alpha,
                            );
                        }
                    }
                    ExpandedTab::Timer => {
                        draw_timer_panel(cr, left, top, frame.timer, frame.hovered_target, alpha);
                    }
                    ExpandedTab::Clipboard => {
                        draw_clipboard_panel(
                            cr,
                            left,
                            top,
                            width,
                            frame.clip_items,
                            frame.clip_copied_flash,
                            frame.hovered_target,
                            alpha,
                        );
                    }
                    ExpandedTab::System => {
                        draw_sysmon_panel(cr, left, top, width, frame.sys_stats, alpha);
                    }
                    ExpandedTab::Notifications => {
                        self.draw_notifications_panel(
                            cr,
                            left,
                            top,
                            width,
                            frame.notification,
                            frame.hovered_target,
                            alpha,
                        );
                    }
                }
            }
        } else {
            let collapsed_alpha = (1.0 - expanded_progress * 3.5).clamp(0.0, 1.0) as f64;
            if frame.media.has_session {
                self.draw_collapsed_media(cr, left, top, width, height, frame, collapsed_alpha);
            } else {
                self.draw_collapsed_idle(cr, left, top, width, height, frame, collapsed_alpha);
            }
        }

        let _ = cr.restore();

        rounded_rect_path(cr, left + 0.5, top + 0.5, width - 1.0, height - 1.0, radius);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }

    fn draw_collapsed_media(
        &self,
        cr: &cairo::Context,
        ox: f64,
        oy: f64,
        pill_w: f64,
        pill_h: f64,
        frame: &Frame,
        alpha: f64,
    ) {
        self.draw_art(cr, ox, oy, &COLLAPSED_ART, 6.0);

        let unread = frame.notification.unread_count();
        let center_text = if frame.timer.running {
            frame.timer.formatted()
        } else {
            current_clock_string()
        };
        let center_color = if frame.timer.running {
            (ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, alpha)
        } else {
            (1.0, 1.0, 1.0, 0.96 * alpha)
        };

        if unread > 0 {
            draw_text_centered_v(
                cr,
                &center_text,
                ox + 36.0,
                oy,
                66.0,
                pill_h,
                16.0,
                true,
                pango::Alignment::Center,
                center_color,
            );
            draw_equalizer(
                cr,
                ox,
                oy,
                pill_h / 2.0,
                &frame.eq_levels,
                frame.media.is_playing,
                alpha,
                pill_w - 62.0,
                EQUALIZER_BAR_WIDTH as f64,
                EQUALIZER_BAR_GAP as f64,
                3.5,
                14.5,
            );
            if let Some(app_name) = frame.notification.latest_app_name() {
                draw_collapsed_notification_badge(
                    cr, ox, oy, pill_w, pill_h, app_name, unread, alpha,
                );
            }
        } else {
            draw_text_centered_v(
                cr,
                &center_text,
                ox + 42.0,
                oy,
                76.0,
                pill_h,
                16.5,
                true,
                pango::Alignment::Center,
                center_color,
            );
            draw_equalizer(
                cr,
                ox,
                oy,
                pill_h / 2.0,
                &frame.eq_levels,
                frame.media.is_playing,
                alpha,
                EQUALIZER_RIGHT as f64,
                EQUALIZER_BAR_WIDTH as f64,
                EQUALIZER_BAR_GAP as f64,
                3.5,
                14.5,
            );
        }
    }

    fn draw_collapsed_idle(
        &self,
        cr: &cairo::Context,
        ox: f64,
        oy: f64,
        pill_w: f64,
        pill_h: f64,
        frame: &Frame,
        alpha: f64,
    ) {
        cr.arc(ox + 18.0, oy + pill_h / 2.0, 4.0, 0.0, 2.0 * PI);
        cr.set_source_rgba(ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, 0.95 * alpha);
        let _ = cr.fill();

        let clock = current_clock_string();
        let unread = frame.notification.unread_count();

        if frame.timer.running {
            draw_text_centered_v(
                cr,
                &frame.timer.formatted(),
                ox + 28.0,
                oy,
                64.0,
                pill_h,
                16.0,
                true,
                pango::Alignment::Center,
                (ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, alpha),
            );
            if unread > 0 {
                if let Some(app_name) = frame.notification.latest_app_name() {
                    draw_collapsed_notification_badge(
                        cr, ox, oy, pill_w, pill_h, app_name, unread, alpha,
                    );
                }
            } else {
                draw_text_centered_v(
                    cr,
                    &clock,
                    ox + pill_w - 74.0,
                    oy,
                    66.0,
                    pill_h,
                    16.0,
                    true,
                    pango::Alignment::Center,
                    (1.0, 1.0, 1.0, 0.95 * alpha),
                );
            }
            return;
        }

        if unread > 0 {
            draw_text_centered_v(
                cr,
                &clock,
                ox + 30.0,
                oy,
                68.0,
                pill_h,
                16.5,
                true,
                pango::Alignment::Center,
                (1.0, 1.0, 1.0, 0.98 * alpha),
            );
            if let Some(app_name) = frame.notification.latest_app_name() {
                draw_collapsed_notification_badge(
                    cr, ox, oy, pill_w, pill_h, app_name, unread, alpha,
                );
            }
        } else {
            draw_text_centered_v(
                cr,
                "Zekrima",
                ox + 29.0,
                oy,
                72.0,
                pill_h,
                12.5,
                true,
                pango::Alignment::Left,
                (1.0, 1.0, 1.0, 0.82 * alpha),
            );
            draw_text_centered_v(
                cr,
                &clock,
                ox + pill_w - 74.0,
                oy,
                66.0,
                pill_h,
                16.5,
                true,
                pango::Alignment::Center,
                (1.0, 1.0, 1.0, 0.98 * alpha),
            );
        }
    }

    fn draw_expanded_media(
        &self,
        cr: &cairo::Context,
        ox: f64,
        oy: f64,
        frame: &Frame,
        alpha: f64,
    ) {
        self.draw_art(cr, ox, oy, &EXPANDED_ART, 14.0);

        let title = if frame.media.title.trim().is_empty() {
            "Unknown title"
        } else {
            frame.media.title.as_str()
        };
        draw_text(
            cr,
            title,
            ox + TITLE_BOX.left as f64,
            oy + TITLE_BOX.top as f64,
            TITLE_BOX.width() as f64,
            18.5,
            true,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, alpha),
        );
        draw_text(
            cr,
            &frame.media.artist,
            ox + ARTIST_BOX.left as f64,
            oy + ARTIST_BOX.top as f64,
            ARTIST_BOX.width() as f64,
            14.5,
            false,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, 0.65 * alpha),
        );

        let hovered_ctrl = match frame.hovered_target {
            Some(HitTarget::MediaControl(c)) => Some(c),
            _ => None,
        };
        draw_controls(cr, ox, oy, alpha, frame.media.is_playing, hovered_ctrl);

        draw_equalizer(
            cr,
            ox,
            oy,
            CONTROLS_TOP as f64 + 15.0,
            &frame.eq_levels,
            frame.media.is_playing,
            alpha,
            344.0,
            4.0,
            3.5,
            4.0,
            20.0,
        );

        draw_text_centered_v(
            cr,
            &current_clock_string(),
            ox + 350.0,
            oy + CONTROLS_TOP as f64,
            74.0,
            30.0,
            18.5,
            true,
            pango::Alignment::Right,
            (1.0, 1.0, 1.0, 0.96 * alpha),
        );
    }

    fn draw_art(&self, cr: &cairo::Context, ox: f64, oy: f64, rect: &RectF, radius: f64) {
        let x = ox + rect.left as f64;
        let y = oy + rect.top as f64;
        let w = rect.width() as f64;
        let h = rect.height() as f64;

        let _ = cr.save();
        rounded_rect_path(cr, x, y, w, h, radius);
        cr.clip();

        if let Some(ref pixbuf) = self.art_pixbuf {
            let pw = pixbuf.width().max(1) as f64;
            let ph = pixbuf.height().max(1) as f64;
            let scale = (w / pw).max(h / ph);
            let draw_w = pw * scale;
            let draw_h = ph * scale;
            let dx = x + (w - draw_w) / 2.0;
            let dy = y + (h - draw_h) / 2.0;

            cr.translate(dx, dy);
            cr.scale(scale, scale);
            cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
            let _ = cr.paint();
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.08);
            let _ = cr.paint();
        }

        let _ = cr.restore();
    }

    fn draw_idle_expanded(
        &self,
        cr: &cairo::Context,
        ox: f64,
        oy: f64,
        pill_w: f64,
        pill_h: f64,
        system: &SystemSnapshot,
        notif: &NotificationSnapshot,
        alpha: f64,
    ) {
        let pad_x = 22.0;
        let content_w = (pill_w - pad_x * 2.0).max(40.0);

        let now_local = Local::now();
        let clock = now_local.format("%H:%M").to_string();
        draw_text(
            cr,
            &clock,
            ox + pad_x,
            oy + 48.0,
            125.0,
            34.0,
            true,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, alpha),
        );

        let date_str = now_local.format("%a, %d %b").to_string();
        let sub = if notif.unread_count() > 0 {
            format!("{date_str}  •  {} unread", notif.unread_count())
        } else {
            format!("{date_str}  •  No media playing")
        };
        draw_text(
            cr,
            &sub,
            ox + pad_x + 118.0,
            oy + 62.0,
            content_w - 122.0,
            14.5,
            false,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, 0.76 * alpha),
        );

        let bar_y = oy + pill_h - 24.0;
        let bar_h = 6.0;
        let vol_lbl = if system.muted {
            "Volume  •  Muted".to_string()
        } else {
            format!("Volume  •  {}%", (system.volume * 100.0).round() as u32)
        };
        draw_text(
            cr,
            &vol_lbl,
            ox + pad_x,
            bar_y - 19.0,
            content_w,
            12.0,
            true,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, 0.66 * alpha),
        );

        rounded_rect_path(cr, ox + pad_x, bar_y, content_w, bar_h, bar_h / 2.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.16 * alpha);
        let _ = cr.fill();

        let clamped = system.volume.clamp(0.0, 1.0) as f64;
        let fill_w = (content_w * clamped).max(if clamped > 0.001 { bar_h } else { 0.0 });
        if fill_w > 0.0 {
            rounded_rect_path(cr, ox + pad_x, bar_y, fill_w, bar_h, bar_h / 2.0);
            cr.set_source_rgba(
                1.0,
                1.0,
                1.0,
                if system.muted { 0.30 } else { 0.88 } * alpha,
            );
            let _ = cr.fill();
        }
    }

    fn draw_notification_toast(
        &self,
        cr: &cairo::Context,
        ox: f64,
        oy: f64,
        item: &NotificationItem,
        progress: f32,
        hovered: Option<HitTarget>,
    ) {
        let (ar, ag, ab) = app_accent_rgb(&item.app);

        let ax = ox + TOAST_AVATAR.left as f64;
        let ay = oy + TOAST_AVATAR.top as f64;
        let aw = TOAST_AVATAR.width() as f64;
        let ah = TOAST_AVATAR.height() as f64;
        rounded_rect_path(cr, ax, ay, aw, ah, 10.0);
        cr.set_source_rgba(ar, ag, ab, 0.36);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(ar, ag, ab, 0.58);
        cr.set_line_width(1.0);
        let _ = cr.stroke();

        draw_text_centered_v(
            cr,
            &app_initial(&item.app),
            ax,
            ay,
            aw,
            ah,
            14.0,
            true,
            pango::Alignment::Center,
            (1.0, 1.0, 1.0, 1.0),
        );

        let tx = ox + 64.0;
        let tw = (TOAST_DISMISS_BTN.left - 68.0).max(60.0) as f64;
        let header = format!("{}  •  just now", item.app);
        draw_text(
            cr,
            &header,
            tx,
            oy + 13.0,
            tw,
            11.5,
            true,
            pango::Alignment::Left,
            (ar, ag, ab, 1.0),
        );

        let title = if item.title.is_empty() {
            "Notification"
        } else {
            item.title.as_str()
        };
        draw_text(
            cr,
            title,
            tx,
            oy + 30.0,
            tw,
            13.0,
            true,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, 0.96),
        );

        if !item.body.is_empty() {
            draw_text(
                cr,
                &item.body,
                tx,
                oy + 49.0,
                tw + 18.0,
                11.5,
                false,
                pango::Alignment::Left,
                (1.0, 1.0, 1.0, 0.68),
            );
        }

        draw_dismiss_x(
            cr,
            ox,
            oy,
            TOAST_DISMISS_BTN,
            hovered == Some(HitTarget::ToastDismiss(item.id)),
            1.0,
        );

        let bar_w = TOAST_PROGRESS_RECT.width() as f64 * progress.clamp(0.0, 1.0) as f64;
        if bar_w > 1.0 {
            rounded_rect_path(
                cr,
                ox + TOAST_PROGRESS_RECT.left as f64,
                oy + TOAST_PROGRESS_RECT.top as f64,
                bar_w,
                TOAST_PROGRESS_RECT.height() as f64,
                1.25,
            );
            cr.set_source_rgba(ar, ag, ab, 0.72);
            let _ = cr.fill();
        }
    }

    fn draw_notifications_panel(
        &self,
        cr: &cairo::Context,
        ox: f64,
        oy: f64,
        pill_w: f64,
        notif: &NotificationSnapshot,
        hovered: Option<HitTarget>,
        alpha: f64,
    ) {
        let unread = notif.unread_count();
        let header = if unread > NOTIF_CARD_MAX_VISIBLE {
            format!(
                "Notifications ({unread})  •  +{} more",
                unread - NOTIF_CARD_MAX_VISIBLE
            )
        } else if unread > 0 {
            format!("Notifications ({unread})")
        } else {
            "Notifications".to_string()
        };
        draw_text(
            cr,
            &header,
            ox + 20.0,
            oy + 41.0,
            260.0,
            12.0,
            true,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, 0.84 * alpha),
        );

        if !notif.items.is_empty() {
            draw_action_pill(
                cr,
                ox,
                oy,
                NOTIF_CLEAR_BTN,
                "Clear all",
                hovered == Some(HitTarget::ClearAllNotifications),
                alpha,
            );
        }

        if notif.items.is_empty() {
            draw_text(
                cr,
                "No notifications",
                ox + 16.0,
                oy + 82.0,
                pill_w - 32.0,
                13.5,
                true,
                pango::Alignment::Center,
                (1.0, 1.0, 1.0, 0.90 * alpha),
            );
            draw_text(
                cr,
                "All caught up!",
                ox + 16.0,
                oy + 106.0,
                pill_w - 32.0,
                11.5,
                false,
                pango::Alignment::Center,
                (1.0, 1.0, 1.0, 0.65 * alpha),
            );
            return;
        }

        for (index, item) in notif
            .items
            .iter()
            .take(NOTIF_CARD_MAX_VISIBLE)
            .enumerate()
        {
            let card = notif_card_rect(index);
            let dismiss_rect = notif_card_dismiss_rect(index);
            let card_hovered = hovered == Some(HitTarget::NotifDismiss(item.id));

            rounded_rect_path(
                cr,
                ox + card.left as f64,
                oy + card.top as f64,
                card.width() as f64,
                card.height() as f64,
                10.0,
            );
            cr.set_source_rgba(
                1.0,
                1.0,
                1.0,
                if card_hovered { 0.11 } else { 0.055 } * alpha,
            );
            let _ = cr.fill();

            let (ar, ag, ab) = app_accent_rgb(&item.app);
            let av_x = ox + card.left as f64 + 8.0;
            let av_y = oy + card.top as f64 + 8.0;
            rounded_rect_path(cr, av_x, av_y, 25.0, 25.0, 6.0);
            cr.set_source_rgba(ar, ag, ab, 0.90 * alpha);
            let _ = cr.fill();

            draw_text_centered_v(
                cr,
                &app_initial(&item.app),
                av_x,
                av_y,
                25.0,
                25.0,
                11.5,
                true,
                pango::Alignment::Center,
                (1.0, 1.0, 1.0, alpha),
            );

            let tx = ox + card.left as f64 + 40.0;
            let tw = (dismiss_rect.left - (card.left + 44.0)).max(60.0) as f64;
            let meta = format!("{}  •  {}", item.app, format_relative_time(item.timestamp_ms));
            draw_text(
                cr,
                &meta,
                tx,
                oy + card.top as f64 + 4.0,
                tw,
                10.5,
                true,
                pango::Alignment::Left,
                (ar, ag, ab, 0.95 * alpha),
            );

            let line2 = if item.body.is_empty() {
                item.title.clone()
            } else if item.title.is_empty() {
                item.body.clone()
            } else {
                format!("{} — {}", item.title, item.body)
            };
            draw_text(
                cr,
                &line2,
                tx,
                oy + card.top as f64 + 19.0,
                tw,
                12.0,
                true,
                pango::Alignment::Left,
                (1.0, 1.0, 1.0, 0.94 * alpha),
            );

            draw_dismiss_x(cr, ox, oy, dismiss_rect, card_hovered, alpha);
        }
    }

    fn draw_hud(
        &self,
        cr: &cairo::Context,
        ox: f64,
        oy: f64,
        pill_w: f64,
        pill_h: f64,
        hud: &HudState,
    ) {
        let pad_x = 20.0;
        let bar_h = 6.0;
        let bar_w = (pill_w - pad_x * 2.0).max(40.0);
        let bar_x = ox + (pill_w - bar_w) / 2.0;
        let bar_y = oy + (pill_h * 0.58).max(24.0);
        let label_y = (bar_y - 20.0).max(oy + 8.0);

        let label = match hud.kind {
            HudKind::Volume => "Volume",
            HudKind::Brightness => "Brightness",
        };
        let value_text = if hud.kind == HudKind::Volume && hud.muted {
            "Muted".to_string()
        } else {
            format!("{}%", (hud.value.clamp(0.0, 1.0) * 100.0).round() as u32)
        };

        draw_text(
            cr,
            label,
            bar_x,
            label_y,
            bar_w,
            12.5,
            true,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, 0.92),
        );
        draw_text(
            cr,
            &value_text,
            bar_x,
            label_y,
            bar_w,
            12.0,
            true,
            pango::Alignment::Right,
            (1.0, 1.0, 1.0, 0.78),
        );

        rounded_rect_path(cr, bar_x, bar_y, bar_w, bar_h, bar_h / 2.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.18);
        let _ = cr.fill();

        let clamped = hud.value.clamp(0.0, 1.0) as f64;
        let fill_w = (bar_w * clamped).max(if clamped > 0.001 { bar_h } else { 0.0 });
        if fill_w > 0.0 {
            rounded_rect_path(cr, bar_x, bar_y, fill_w, bar_h, bar_h / 2.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, if hud.muted { 0.35 } else { 0.95 });
            let _ = cr.fill();
        }
    }
}

fn draw_tab_bar(cr: &cairo::Context, ox: f64, oy: f64, frame: &Frame, alpha: f64) {
    let unread = frame.notification.unread_count();
    for tab in ALL_TABS {
        let rect = tab_rect(tab);
        let active = frame.expanded_tab == tab;
        let hovered = frame.hovered_target == Some(HitTarget::SelectTab(tab));

        let x = ox + rect.left as f64;
        let y = oy + rect.top as f64;
        let w = rect.width() as f64;
        let h = rect.height() as f64;

        rounded_rect_path(cr, x, y, w, h, h / 2.0);
        if active {
            cr.set_source_rgba(ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, 0.28 * alpha);
            let _ = cr.fill_preserve();
            cr.set_source_rgba(ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, 0.68 * alpha);
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, if hovered { 0.14 } else { 0.055 } * alpha);
            let _ = cr.fill();
        }

        let label = match tab {
            ExpandedTab::Media => "Media".to_string(),
            ExpandedTab::Timer => {
                if frame.timer.running {
                    frame.timer.formatted()
                } else {
                    "Timer".to_string()
                }
            }
            ExpandedTab::Clipboard => "Clip".to_string(),
            ExpandedTab::System => "Sys".to_string(),
            ExpandedTab::Notifications => {
                if unread > 9 {
                    "Notifs 9+".to_string()
                } else if unread > 0 {
                    format!("Notifs {unread}")
                } else {
                    "Notifs".to_string()
                }
            }
        };

        let text_a = if active {
            1.0
        } else if hovered {
            0.92
        } else {
            0.68
        };
        draw_text_centered_v(
            cr,
            &label,
            x,
            y,
            w,
            h,
            11.0,
            true,
            pango::Alignment::Center,
            (1.0, 1.0, 1.0, text_a * alpha),
        );
    }
}

fn draw_timer_panel(
    cr: &cairo::Context,
    ox: f64,
    oy: f64,
    timer: &PomodoroTimer,
    hovered: Option<HitTarget>,
    alpha: f64,
) {
    draw_text(
        cr,
        &timer.formatted(),
        ox + 22.0,
        oy + 44.0,
        180.0,
        34.0,
        true,
        pango::Alignment::Left,
        (1.0, 1.0, 1.0, alpha),
    );

    let (status, color) = if timer.running {
        (
            "Focus • Running",
            (ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, 0.95 * alpha),
        )
    } else if timer.completed {
        ("Focus • Complete!", (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0, alpha))
    } else if timer.remaining_secs < timer.total_secs {
        ("Focus • Paused", (1.0, 1.0, 1.0, 0.72 * alpha))
    } else {
        ("Focus • Ready", (1.0, 1.0, 1.0, 0.68 * alpha))
    };
    draw_text(
        cr,
        status,
        ox + 22.0,
        oy + 88.0,
        180.0,
        11.5,
        true,
        pango::Alignment::Left,
        color,
    );

    // Start / Pause button
    let toggle_lbl = if timer.running { "Pause" } else { "Start" };
    let toggle_hov = hovered == Some(HitTarget::TimerToggle);
    rounded_rect_path(
        cr,
        ox + TIMER_TOGGLE_BTN.left as f64,
        oy + TIMER_TOGGLE_BTN.top as f64,
        TIMER_TOGGLE_BTN.width() as f64,
        TIMER_TOGGLE_BTN.height() as f64,
        15.0,
    );
    cr.set_source_rgba(
        ACCENT_RED.0,
        ACCENT_RED.1,
        ACCENT_RED.2,
        if toggle_hov { 0.96 } else { 0.80 } * alpha,
    );
    let _ = cr.fill();
    draw_text_centered_v(
        cr,
        toggle_lbl,
        ox + TIMER_TOGGLE_BTN.left as f64,
        oy + TIMER_TOGGLE_BTN.top as f64,
        TIMER_TOGGLE_BTN.width() as f64,
        TIMER_TOGGLE_BTN.height() as f64,
        12.0,
        true,
        pango::Alignment::Center,
        (1.0, 1.0, 1.0, alpha),
    );

    draw_action_pill(
        cr,
        ox,
        oy,
        TIMER_RESET_BTN,
        "Reset",
        hovered == Some(HitTarget::TimerReset),
        alpha,
    );

    for (i, &secs) in TIMER_PRESETS_SECS.iter().enumerate() {
        let rect = timer_preset_rect(i);
        let selected = timer.total_secs == secs;
        let is_hov = hovered == Some(HitTarget::TimerPreset(secs));
        let lbl = format!("{}m", secs / 60);

        rounded_rect_path(
            cr,
            ox + rect.left as f64,
            oy + rect.top as f64,
            rect.width() as f64,
            rect.height() as f64,
            12.0,
        );
        cr.set_source_rgba(
            1.0,
            1.0,
            1.0,
            if selected {
                0.20
            } else if is_hov {
                0.15
            } else {
                0.07
            } * alpha,
        );
        let _ = cr.fill();

        draw_text_centered_v(
            cr,
            &lbl,
            ox + rect.left as f64,
            oy + rect.top as f64,
            rect.width() as f64,
            rect.height() as f64,
            11.5,
            true,
            pango::Alignment::Center,
            (1.0, 1.0, 1.0, if selected || is_hov { 1.0 } else { 0.72 } * alpha),
        );
    }

    let bar_h = TIMER_PROGRESS_BAR.height() as f64;
    rounded_rect_path(
        cr,
        ox + TIMER_PROGRESS_BAR.left as f64,
        oy + TIMER_PROGRESS_BAR.top as f64,
        TIMER_PROGRESS_BAR.width() as f64,
        bar_h,
        bar_h / 2.0,
    );
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.16 * alpha);
    let _ = cr.fill();

    let fill_w = TIMER_PROGRESS_BAR.width() as f64 * timer.progress() as f64;
    if fill_w > 1.0 {
        rounded_rect_path(
            cr,
            ox + TIMER_PROGRESS_BAR.left as f64,
            oy + TIMER_PROGRESS_BAR.top as f64,
            fill_w,
            bar_h,
            bar_h / 2.0,
        );
        cr.set_source_rgba(ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, 0.92 * alpha);
        let _ = cr.fill();
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_clipboard_panel(
    cr: &cairo::Context,
    ox: f64,
    oy: f64,
    pill_w: f64,
    items: &[String],
    copied_flash: bool,
    hovered: Option<HitTarget>,
    alpha: f64,
) {
    let (header, color) = if copied_flash {
        (
            "Copied to clipboard!".to_string(),
            (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0, alpha),
        )
    } else {
        (
            format!("Clipboard History ({})", items.len()),
            (1.0, 1.0, 1.0, 0.82 * alpha),
        )
    };
    draw_text(
        cr,
        &header,
        ox + 20.0,
        oy + 41.0,
        260.0,
        12.0,
        true,
        pango::Alignment::Left,
        color,
    );

    if !items.is_empty() {
        draw_action_pill(
            cr,
            ox,
            oy,
            CLIP_CLEAR_BTN,
            "Clear",
            hovered == Some(HitTarget::ClipClear),
            alpha,
        );
    }

    if items.is_empty() {
        draw_text(
            cr,
            "Clipboard history is empty",
            ox + 16.0,
            oy + 82.0,
            pill_w - 32.0,
            13.5,
            true,
            pango::Alignment::Center,
            (1.0, 1.0, 1.0, 0.88 * alpha),
        );
        draw_text(
            cr,
            "Copy any text (Ctrl+C) to pin it here",
            ox + 16.0,
            oy + 106.0,
            pill_w - 32.0,
            11.5,
            false,
            pango::Alignment::Center,
            (1.0, 1.0, 1.0, 0.65 * alpha),
        );
        return;
    }

    for (i, text) in items.iter().take(CLIP_MAX_VISIBLE).enumerate() {
        let rect = clip_row_rect(i);
        let is_hov = hovered == Some(HitTarget::ClipCopy(i));

        rounded_rect_path(
            cr,
            ox + rect.left as f64,
            oy + rect.top as f64,
            rect.width() as f64,
            rect.height() as f64,
            8.0,
        );
        cr.set_source_rgba(1.0, 1.0, 1.0, if is_hov { 0.14 } else { 0.06 } * alpha);
        let _ = cr.fill();

        let idx_str = format!("#{}", i + 1);
        draw_text_centered_v(
            cr,
            &idx_str,
            ox + rect.left as f64 + 6.0,
            oy + rect.top as f64,
            24.0,
            rect.height() as f64,
            11.0,
            true,
            pango::Alignment::Center,
            (ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, 0.92 * alpha),
        );

        let single_line = text.replace(['\r', '\n', '\t'], " ");
        draw_text_centered_v(
            cr,
            &single_line,
            ox + rect.left as f64 + 34.0,
            oy + rect.top as f64,
            rect.width() as f64 - 42.0,
            rect.height() as f64,
            11.5,
            false,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, if is_hov { 1.0 } else { 0.86 } * alpha),
        );
    }
}

fn draw_sysmon_panel(
    cr: &cairo::Context,
    ox: f64,
    oy: f64,
    pill_w: f64,
    stats: &SystemStats,
    alpha: f64,
) {
    let pad_x = 22.0;
    let bar_w = (pill_w - pad_x * 2.0).max(80.0);

    let ram_val = if stats.ram_total_gb > 0.1 {
        format!(
            "{:.1} / {:.1} GB  ({:.0}%)",
            stats.ram_used_gb, stats.ram_total_gb, stats.ram_percent
        )
    } else {
        format!("{:.0}%", stats.ram_percent)
    };
    let power_lbl = if stats.has_battery { "Battery" } else { "Power" };
    let power_val = if stats.has_battery {
        if stats.is_charging {
            format!("{:.0}%  •  Charging", stats.battery_percent)
        } else {
            format!("{:.0}%", stats.battery_percent)
        }
    } else {
        "AC Power".to_string()
    };

    let rows: [(&str, String, f64, bool); 3] = [
        (
            "CPU",
            format!("{:.0}%", stats.cpu_percent),
            (stats.cpu_percent / 100.0) as f64,
            false,
        ),
        ("Memory", ram_val, (stats.ram_percent / 100.0) as f64, false),
        (
            power_lbl,
            power_val,
            (stats.battery_percent / 100.0) as f64,
            true,
        ),
    ];

    for (i, (label, value, frac, is_battery)) in rows.iter().enumerate() {
        let y = oy + 46.0 + i as f64 * 36.0;
        draw_text(
            cr,
            label,
            ox + pad_x,
            y,
            bar_w,
            12.0,
            true,
            pango::Alignment::Left,
            (1.0, 1.0, 1.0, 0.90 * alpha),
        );
        draw_text(
            cr,
            value,
            ox + pad_x,
            y,
            bar_w,
            11.5,
            true,
            pango::Alignment::Right,
            (1.0, 1.0, 1.0, 0.75 * alpha),
        );

        let bar_y = y + 19.0;
        let bar_h = 7.0;
        rounded_rect_path(cr, ox + pad_x, bar_y, bar_w, bar_h, bar_h / 2.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.16 * alpha);
        let _ = cr.fill();

        let clamped = frac.clamp(0.0, 1.0);
        let fill_w = (bar_w * clamped).max(if clamped > 0.005 { bar_h } else { 0.0 });
        if fill_w > 0.0 {
            let (r, g, b) = metric_bar_color(clamped, *is_battery);
            rounded_rect_path(cr, ox + pad_x, bar_y, fill_w, bar_h, bar_h / 2.0);
            cr.set_source_rgba(r, g, b, 0.92 * alpha);
            let _ = cr.fill();
        }
    }
}

fn metric_bar_color(frac: f64, is_battery: bool) -> (f64, f64, f64) {
    if is_battery {
        if frac < 0.20 {
            (239.0 / 255.0, 68.0 / 255.0, 68.0 / 255.0)
        } else if frac < 0.40 {
            (245.0 / 255.0, 158.0 / 255.0, 11.0 / 255.0)
        } else {
            (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0)
        }
    } else if frac > 0.85 {
        ACCENT_RED
    } else if frac > 0.60 {
        (245.0 / 255.0, 158.0 / 255.0, 11.0 / 255.0)
    } else {
        (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0)
    }
}

fn current_clock_string() -> String {
    Local::now().format("%H:%M").to_string()
}

#[allow(clippy::too_many_arguments)]
fn draw_equalizer(
    cr: &cairo::Context,
    ox: f64,
    oy: f64,
    center_y: f64,
    levels: &[f32; EQUALIZER_BAR_COUNT],
    is_playing: bool,
    alpha: f64,
    right_edge: f64,
    bar_width: f64,
    bar_gap: f64,
    min_height: f64,
    max_delta: f64,
) {
    let total_w = EQUALIZER_BAR_COUNT as f64 * bar_width
        + (EQUALIZER_BAR_COUNT as f64 - 1.0) * bar_gap;
    let start_x = right_edge - total_w;
    let bar_alpha = if is_playing { alpha } else { alpha * 0.45 };

    for index in 0..EQUALIZER_BAR_COUNT {
        let strength = if is_playing {
            levels[index].clamp(0.0, 1.0) as f64
        } else {
            PAUSED_EQUALIZER[index] as f64
        };
        let h = min_height + strength * max_delta;
        let x = ox + start_x + index as f64 * (bar_width + bar_gap);
        let y = oy + center_y - h / 2.0;

        rounded_rect_path(cr, x, y, bar_width, h, bar_width / 2.0);
        if index % 2 == 0 {
            cr.set_source_rgba(ACCENT_RED.0, ACCENT_RED.1, ACCENT_RED.2, bar_alpha);
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.90 * bar_alpha);
        }
        let _ = cr.fill();
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_collapsed_notification_badge(
    cr: &cairo::Context,
    ox: f64,
    oy: f64,
    pill_w: f64,
    pill_h: f64,
    app_name: &str,
    count: usize,
    alpha: f64,
) {
    let cy = oy + pill_h / 2.0;
    let count_text = if count > 9 {
        "9+".to_string()
    } else {
        count.to_string()
    };
    let count_w = if count > 9 { 24.0 } else { 20.0 };
    let count_right = ox + pill_w - 9.0;
    let count_left = count_right - count_w;

    rounded_rect_path(cr, count_left, cy - 10.0, count_w, 20.0, 10.0);
    cr.set_source_rgba(BADGE_RED.0, BADGE_RED.1, BADGE_RED.2, alpha);
    let _ = cr.fill();

    draw_text_centered_v(
        cr,
        &count_text,
        count_left,
        cy - 10.0,
        count_w,
        20.0,
        11.0,
        true,
        pango::Alignment::Center,
        (1.0, 1.0, 1.0, alpha),
    );

    let avatar_cx = count_left - 14.0;
    let (r, g, b) = app_accent_rgb(app_name);
    cr.arc(avatar_cx, cy, 10.0, 0.0, 2.0 * PI);
    cr.set_source_rgba(r, g, b, alpha);
    let _ = cr.fill();

    draw_text_centered_v(
        cr,
        &app_initial(app_name),
        avatar_cx - 10.0,
        cy - 10.0,
        20.0,
        20.0,
        11.0,
        true,
        pango::Alignment::Center,
        (1.0, 1.0, 1.0, alpha),
    );
}

fn draw_controls(
    cr: &cairo::Context,
    ox: f64,
    oy: f64,
    alpha: f64,
    is_playing: bool,
    hovered: Option<MediaControl>,
) {
    if let Some(ctrl) = hovered {
        let r = control_rect(ctrl);
        let (cx, cy) = r.center();
        cr.arc(ox + cx as f64, oy + cy as f64, 15.5, 0.0, 2.0 * PI);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.14 * alpha);
        let _ = cr.fill();
    }

    cr.set_source_rgba(1.0, 1.0, 1.0, 0.92 * alpha);

    let prev = control_rect(MediaControl::Previous);
    let pl = ox + prev.left as f64;
    let pt = oy + prev.top as f64;
    cr.rectangle(pl + 4.0, pt + 8.5, 2.2, 13.0);
    let _ = cr.fill();
    cr.move_to(pl + 21.0, pt + 8.5);
    cr.line_to(pl + 21.0, pt + 21.5);
    cr.line_to(pl + 8.0, pt + 15.0);
    cr.close_path();
    let _ = cr.fill();

    let pp = control_rect(MediaControl::PlayPause);
    let cl = ox + pp.left as f64;
    let ct = oy + pp.top as f64;
    if is_playing {
        rounded_rect_path(cr, cl + 10.5, ct + 9.0, 3.0, 12.0, 1.5);
        let _ = cr.fill();
        rounded_rect_path(cr, cl + 16.5, ct + 9.0, 3.0, 12.0, 1.5);
        let _ = cr.fill();
    } else {
        cr.move_to(cl + 11.0, ct + 8.0);
        cr.line_to(cl + 11.0, ct + 22.0);
        cr.line_to(cl + 22.5, ct + 15.0);
        cr.close_path();
        let _ = cr.fill();
    }

    let next = control_rect(MediaControl::Next);
    let nl = ox + next.left as f64;
    let nt = oy + next.top as f64;
    cr.move_to(nl + 9.0, nt + 8.5);
    cr.line_to(nl + 9.0, nt + 21.5);
    cr.line_to(nl + 22.0, nt + 15.0);
    cr.close_path();
    let _ = cr.fill();
    cr.rectangle(nl + 24.5, nt + 8.5, 2.2, 13.0);
    let _ = cr.fill();
}

fn draw_action_pill(
    cr: &cairo::Context,
    ox: f64,
    oy: f64,
    rect: RectF,
    label: &str,
    hovered: bool,
    alpha: f64,
) {
    let x = ox + rect.left as f64;
    let y = oy + rect.top as f64;
    let w = rect.width() as f64;
    let h = rect.height() as f64;

    rounded_rect_path(cr, x, y, w, h, h / 2.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, if hovered { 0.20 } else { 0.10 } * alpha);
    let _ = cr.fill();

    draw_text_centered_v(
        cr,
        label,
        x,
        y,
        w,
        h,
        11.0,
        true,
        pango::Alignment::Center,
        (1.0, 1.0, 1.0, if hovered { 1.0 } else { 0.82 } * alpha),
    );
}

fn draw_dismiss_x(cr: &cairo::Context, ox: f64, oy: f64, rect: RectF, hovered: bool, alpha: f64) {
    let (cx, cy) = rect.center();
    let x = ox + cx as f64;
    let y = oy + cy as f64;

    if hovered {
        cr.arc(x, y, 10.5, 0.0, 2.0 * PI);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.16 * alpha);
        let _ = cr.fill();
    }

    let r = 3.8;
    cr.set_source_rgba(1.0, 1.0, 1.0, if hovered { 0.92 } else { 0.45 } * alpha);
    cr.set_line_width(1.6);
    cr.move_to(x - r, y - r);
    cr.line_to(x + r, y + r);
    cr.move_to(x + r, y - r);
    cr.line_to(x - r, y + r);
    let _ = cr.stroke();
}

fn rounded_rect_path(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    cr: &cairo::Context,
    text: &str,
    x: f64,
    y: f64,
    max_w: f64,
    size_px: f64,
    bold: bool,
    align: pango::Alignment,
    rgba: (f64, f64, f64, f64),
) {
    let layout = pangocairo::functions::create_layout(cr);
    let mut desc = pango::FontDescription::from_string(if bold { "Sans Bold" } else { "Sans" });
    desc.set_absolute_size(size_px * pango::SCALE as f64);
    layout.set_font_description(Some(&desc));
    layout.set_width((max_w.max(10.0) * pango::SCALE as f64) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_alignment(align);
    layout.set_text(text);

    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, &layout);
}

#[allow(clippy::too_many_arguments)]
fn draw_text_centered_v(
    cr: &cairo::Context,
    text: &str,
    x: f64,
    y: f64,
    max_w: f64,
    box_h: f64,
    size_px: f64,
    bold: bool,
    align: pango::Alignment,
    rgba: (f64, f64, f64, f64),
) {
    let layout = pangocairo::functions::create_layout(cr);
    let mut desc = pango::FontDescription::from_string(if bold { "Sans Bold" } else { "Sans" });
    desc.set_absolute_size(size_px * pango::SCALE as f64);
    layout.set_font_description(Some(&desc));
    layout.set_width((max_w.max(10.0) * pango::SCALE as f64) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_alignment(align);
    layout.set_text(text);

    let (_, text_h) = layout.pixel_size();
    let dy = y + (box_h - text_h as f64) / 2.0;

    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.move_to(x, dy);
    pangocairo::functions::show_layout(cr, &layout);
}
