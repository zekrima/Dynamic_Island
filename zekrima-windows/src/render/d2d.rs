//! Direct2D renderer for the layered island window.
//!
//! Renders:
//! - Collapsed pill (bold clock, Zekrima brand / album art, 5-bar real-time FFT
//!   music equalizer, live Pomodoro timer pill, Pillar notification badge)
//! - Transient Volume / Brightness HUD & Pillar incoming notification toast
//! - Unified 5-tab Expanded Island (`Media`, `Timer`, `Clip`, `Sys`, `Notifs`)

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use windows::core::{w, Error, PCWSTR, Result};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Brush, ID2D1DCRenderTarget, ID2D1Factory,
    ID2D1PathGeometry, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteInlineObject,
    IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_REGULAR,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use crate::layout::{CANVAS_HEIGHT_LOGICAL, CANVAS_WIDTH_LOGICAL, PILL_TOP_LOGICAL};
use crate::render::widgets::{
    app_accent_rgb, app_initial, clip_row_rect, control_rect, expansion_progress,
    format_relative_time, notif_card_dismiss_rect, notif_card_rect, smoothstep, tab_rect,
    timer_preset_rect, ExpandedTab, HitTarget, RectF, ALL_TABS, ARTIST_BOX, CLIP_CLEAR_BTN,
    CLIP_MAX_VISIBLE, COLLAPSED_ART, CONTROLS_TOP, EQUALIZER_BAR_COUNT, EQUALIZER_BAR_GAP,
    EQUALIZER_BAR_WIDTH, EQUALIZER_RIGHT, EXPANDED_ART, NOTIF_CARD_MAX_VISIBLE, NOTIF_CLEAR_BTN,
    NOTIF_SETTINGS_BTN, TIMER_PROGRESS_BAR, TIMER_RESET_BTN, TIMER_TOGGLE_BTN, TITLE_BOX,
    TOAST_AVATAR, TOAST_DISMISS_BTN, TOAST_PROGRESS_RECT,
};
use crate::services::media::{AlbumArt, MediaControl, MediaInfo};
use crate::services::notifications::{NotificationItem, NotificationSnapshot};
use crate::services::pomodoro::{PomodoroTimer, TIMER_PRESETS_SECS};
use crate::services::sysmon::SystemStats;
use crate::services::system::{HudKind, HudState, SystemSnapshot};

const PILL_FILL: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 10.0 / 255.0,
    g: 10.0 / 255.0,
    b: 13.0 / 255.0,
    a: 0.96,
};
const PILL_BORDER: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.11,
};
const PILL_ACCENT: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 235.0 / 255.0,
    g: 0.0,
    b: 40.0 / 255.0,
    a: 1.0,
};
const BADGE_RED: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 239.0 / 255.0,
    g: 68.0 / 255.0,
    b: 68.0 / 255.0,
    a: 1.0,
};
const TITLE_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const ARTIST_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.65,
};
const ICON_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.92,
};
const HOVER_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};
const EQUALIZER_ACCENT: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.88,
};
const ART_PLACEHOLDER: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 0.07,
};

const TITLE_FONT_SIZE: f32 = 19.0;
const ARTIST_FONT_SIZE: f32 = 15.0;
const COLLAPSED_ART_RADIUS: f32 = 6.0;
const EXPANDED_ART_RADIUS: f32 = 14.0;
const HOVER_RADIUS: f32 = 16.0;
const PAUSED_EQUALIZER: [f32; EQUALIZER_BAR_COUNT] = [0.22, 0.45, 0.30, 0.50, 0.25];

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

pub struct Renderer {
    target: ID2D1DCRenderTarget,
    fill_brush: ID2D1SolidColorBrush,
    border_brush: ID2D1SolidColorBrush,
    accent_brush: ID2D1SolidColorBrush,
    badge_red_brush: ID2D1SolidColorBrush,
    dynamic_brush: ID2D1SolidColorBrush,
    title_brush: ID2D1SolidColorBrush,
    artist_brush: ID2D1SolidColorBrush,
    icon_brush: ID2D1SolidColorBrush,
    hover_brush: ID2D1SolidColorBrush,
    equalizer_accent_brush: ID2D1SolidColorBrush,
    placeholder_brush: ID2D1SolidColorBrush,
    hud_track_brush: ID2D1SolidColorBrush,
    hud_fill_brush: ID2D1SolidColorBrush,
    hud_text_brush: ID2D1SolidColorBrush,
    notif_title_brush: ID2D1SolidColorBrush,
    notif_body_brush: ID2D1SolidColorBrush,

    dwrite: IDWriteFactory,
    title_format: IDWriteTextFormat,
    artist_format: IDWriteTextFormat,
    hud_format: IDWriteTextFormat,
    hud_value_format: IDWriteTextFormat,
    clock_collapsed_format: IDWriteTextFormat,
    clock_media_format: IDWriteTextFormat,
    clock_hero_format: IDWriteTextFormat,
    notif_app_format: IDWriteTextFormat,
    notif_title_format: IDWriteTextFormat,
    notif_body_format: IDWriteTextFormat,
    badge_format: IDWriteTextFormat,
    pill_btn_format: IDWriteTextFormat,
    center_title_format: IDWriteTextFormat,
    center_sub_format: IDWriteTextFormat,
    title_trim: IDWriteInlineObject,
    artist_trim: IDWriteInlineObject,
    notif_title_trim: IDWriteInlineObject,
    notif_body_trim: IDWriteInlineObject,
    cached_title: String,
    cached_artist: String,
    title_layout: Option<IDWriteTextLayout>,
    artist_layout: Option<IDWriteTextLayout>,

    previous_icon: ID2D1PathGeometry,
    next_icon: ID2D1PathGeometry,
    play_icon: ID2D1PathGeometry,

    art_revision: u64,
    art_bitmap: Option<ID2D1Bitmap>,

    mem_dc: HDC,
    dib: HBITMAP,
    previous_bitmap: HGDIOBJ,
    width: u32,
    height: u32,
    dpi: f32,
}

impl Renderer {
    pub unsafe fn new(width: u32, height: u32, dpi: f32) -> Result<Self> {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;

        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: dpi,
            dpiY: dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };

        let target = factory.CreateDCRenderTarget(&properties)?;
        let fill_brush = target.CreateSolidColorBrush(&PILL_FILL, None)?;
        let border_brush = target.CreateSolidColorBrush(&PILL_BORDER, None)?;
        let accent_brush = target.CreateSolidColorBrush(&PILL_ACCENT, None)?;
        let badge_red_brush = target.CreateSolidColorBrush(&BADGE_RED, None)?;
        let dynamic_brush = target.CreateSolidColorBrush(&PILL_ACCENT, None)?;
        let title_brush = target.CreateSolidColorBrush(&TITLE_COLOR, None)?;
        let artist_brush = target.CreateSolidColorBrush(&ARTIST_COLOR, None)?;
        let icon_brush = target.CreateSolidColorBrush(&ICON_COLOR, None)?;
        let hover_brush = target.CreateSolidColorBrush(&HOVER_COLOR, None)?;
        let equalizer_accent_brush = target.CreateSolidColorBrush(&EQUALIZER_ACCENT, None)?;
        let placeholder_brush = target.CreateSolidColorBrush(&ART_PLACEHOLDER, None)?;
        let hud_track_brush = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.16,
            },
            None,
        )?;
        let hud_fill_brush = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.92,
            },
            None,
        )?;
        let hud_text_brush = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.88,
            },
            None,
        )?;
        let notif_title_brush = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.96,
            },
            None,
        )?;
        let notif_body_brush = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.68,
            },
            None,
        )?;

        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let family = font_family(&dwrite);

        let title_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            TITLE_FONT_SIZE,
            w!("en-us"),
        )?;
        let artist_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_REGULAR,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            ARTIST_FONT_SIZE,
            w!("en-us"),
        )?;

        let hud_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            12.0,
            w!("en-us"),
        )?;
        hud_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        hud_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;

        let hud_value_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            11.5,
            w!("en-us"),
        )?;
        hud_value_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        hud_value_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;

        let clock_collapsed_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            16.5,
            w!("en-us"),
        )?;
        clock_collapsed_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        clock_collapsed_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

        let clock_media_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            18.5,
            w!("en-us"),
        )?;
        clock_media_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        clock_media_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

        let clock_hero_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            34.0,
            w!("en-us"),
        )?;
        clock_hero_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        clock_hero_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;

        let notif_app_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            11.0,
            w!("en-us"),
        )?;
        notif_app_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;

        let notif_title_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            12.5,
            w!("en-us"),
        )?;
        notif_title_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;

        let notif_body_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_REGULAR,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            11.5,
            w!("en-us"),
        )?;
        notif_body_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;

        let badge_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            11.0,
            w!("en-us"),
        )?;
        badge_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        badge_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

        let pill_btn_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            11.0,
            w!("en-us"),
        )?;
        pill_btn_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        pill_btn_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;

        let center_title_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            13.5,
            w!("en-us"),
        )?;
        center_title_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

        let center_sub_format = dwrite.CreateTextFormat(
            family,
            None,
            DWRITE_FONT_WEIGHT_REGULAR,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            11.5,
            w!("en-us"),
        )?;
        center_sub_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

        let title_trim = dwrite.CreateEllipsisTrimmingSign(&title_format)?;
        let artist_trim = dwrite.CreateEllipsisTrimmingSign(&artist_format)?;
        let notif_title_trim = dwrite.CreateEllipsisTrimmingSign(&notif_title_format)?;
        let notif_body_trim = dwrite.CreateEllipsisTrimmingSign(&notif_body_format)?;

        let previous_icon = build_control_icon(&factory, MediaControl::Previous)?;
        let next_icon = build_control_icon(&factory, MediaControl::Next)?;
        let play_icon = build_control_icon(&factory, MediaControl::PlayPause)?;

        let mut renderer = Self {
            target,
            fill_brush,
            border_brush,
            accent_brush,
            badge_red_brush,
            dynamic_brush,
            title_brush,
            artist_brush,
            icon_brush,
            hover_brush,
            equalizer_accent_brush,
            placeholder_brush,
            hud_track_brush,
            hud_fill_brush,
            hud_text_brush,
            notif_title_brush,
            notif_body_brush,
            dwrite,
            title_format,
            artist_format,
            hud_format,
            hud_value_format,
            clock_collapsed_format,
            clock_media_format,
            clock_hero_format,
            notif_app_format,
            notif_title_format,
            notif_body_format,
            badge_format,
            pill_btn_format,
            center_title_format,
            center_sub_format,
            title_trim,
            artist_trim,
            notif_title_trim,
            notif_body_trim,
            cached_title: String::new(),
            cached_artist: String::new(),
            title_layout: None,
            artist_layout: None,
            previous_icon,
            next_icon,
            play_icon,
            art_revision: 0,
            art_bitmap: None,
            mem_dc: HDC::default(),
            dib: HBITMAP::default(),
            previous_bitmap: HGDIOBJ::default(),
            width: 0,
            height: 0,
            dpi: 0.0,
        };
        renderer.resize(width, height, dpi)?;
        Ok(renderer)
    }

    pub unsafe fn resize(&mut self, width: u32, height: u32, dpi: f32) -> Result<()> {
        if width == self.width && height == self.height && dpi == self.dpi {
            return Ok(());
        }

        self.release_surface();
        self.create_surface(width, height)?;
        self.target.SetDpi(dpi, dpi);
        self.width = width;
        self.height = height;
        self.dpi = dpi;
        Ok(())
    }

    pub unsafe fn render(&mut self, hwnd: HWND, frame: &Frame) -> Result<()> {
        self.prepare_art(frame.media);
        self.prepare_text(frame.media);

        let bounds = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: self.width as i32,
            bottom: self.height as i32,
        };
        self.target.BindDC(self.mem_dc, &bounds)?;

        self.target.BeginDraw();

        let transparent = D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };
        self.target.Clear(Some(&transparent));

        let width = frame.width.clamp(1.0, CANVAS_WIDTH_LOGICAL);
        let height = frame.height.clamp(1.0, CANVAS_HEIGHT_LOGICAL);
        let radius = frame.radius.clamp(0.0, width.min(height) / 2.0);

        let left = (CANVAS_WIDTH_LOGICAL - width) / 2.0;
        let top = PILL_TOP_LOGICAL;
        let pill = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left,
                top,
                right: left + width,
                bottom: top + height,
            },
            radiusX: radius,
            radiusY: radius,
        };

        self.target.FillRoundedRectangle(&pill, &self.fill_brush);

        let now = std::time::Instant::now();
        let hud_visible = frame.system.hud.is_visible(now);
        let toast_visible = frame.notification.is_toast_visible(now);

        // Top-edge accent highlight
        let inset = radius.max(14.0);
        if width > inset * 2.0 + 4.0 {
            let accent_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: left + inset,
                    top: top + 1.0,
                    right: left + width - inset,
                    bottom: top + 2.5,
                },
                radiusX: 0.75,
                radiusY: 0.75,
            };
            if toast_visible && !hud_visible && !frame.expanded {
                if let Some(toast) = &frame.notification.toast {
                    let (r, g, b) = app_accent_rgb(&toast.app);
                    self.dynamic_brush
                        .SetColor(&D2D1_COLOR_F { r, g, b, a: 0.65 });
                    self.target
                        .FillRoundedRectangle(&accent_rect, &self.dynamic_brush);
                }
            } else {
                self.accent_brush.SetOpacity(0.42);
                self.target
                    .FillRoundedRectangle(&accent_rect, &self.accent_brush);
                self.accent_brush.SetOpacity(1.0);
            }
        }

        // Clip all interior content to pill bounds
        let clip = D2D_RECT_F {
            left: left + 1.0,
            top: top + 1.0,
            right: (left + width - 1.0).max(left + 1.0),
            bottom: (top + height - 1.0).max(top + 1.0),
        };
        self.target
            .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);

        let expanded_progress = expansion_progress(height);

        if hud_visible {
            self.draw_hud(left, top, width, height, &frame.system.hud);
        } else if toast_visible && !frame.expanded {
            if let Some(toast) = &frame.notification.toast {
                let progress = frame.notification.toast_progress(now);
                self.draw_notification_toast(left, top, toast, progress, frame.hovered_target);
            }
        } else if frame.expanded || expanded_progress >= 0.28 {
            let alpha = smoothstep(((expanded_progress - 0.15) / 0.85).clamp(0.0, 1.0));
            if alpha > 0.01 {
                self.draw_tab_bar(left, top, frame, alpha);
                match frame.expanded_tab {
                    ExpandedTab::Media => {
                        if frame.media.has_session {
                            self.draw_expanded_media(left, top, frame, alpha);
                        } else {
                            self.draw_idle_expanded(left, top, width, height, frame.system, frame.notification, alpha);
                        }
                    }
                    ExpandedTab::Timer => {
                        self.draw_timer_panel(left, top, frame.timer, frame.hovered_target, alpha);
                    }
                    ExpandedTab::Clipboard => {
                        self.draw_clipboard_panel(
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
                        self.draw_sysmon_panel(left, top, width, frame.sys_stats, alpha);
                    }
                    ExpandedTab::Notifications => {
                        self.draw_notifications_panel(
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
            let collapsed_alpha = (1.0 - expanded_progress * 3.5).clamp(0.0, 1.0);
            if frame.media.has_session {
                self.draw_collapsed_media(left, top, width, height, frame, collapsed_alpha);
            } else {
                self.draw_collapsed_idle(left, top, width, height, frame, collapsed_alpha);
            }
        }

        self.target.PopAxisAlignedClip();

        self.target
            .DrawRoundedRectangle(&pill, &self.border_brush, 1.0, None);

        self.target.EndDraw(None, None)?;
        self.present(hwnd)
    }

    /// Top 5-tab pill switcher shared by all expanded views.
    unsafe fn draw_tab_bar(&self, origin_x: f32, origin_y: f32, frame: &Frame, alpha: f32) {
        let unread = frame.notification.unread_count();
        for tab in ALL_TABS {
            let rect = tab_rect(tab);
            let active = frame.expanded_tab == tab;
            let hovered = frame.hovered_target == Some(HitTarget::SelectTab(tab));

            let pill = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: origin_x + rect.left,
                    top: origin_y + rect.top,
                    right: origin_x + rect.right,
                    bottom: origin_y + rect.bottom,
                },
                radiusX: rect.height() / 2.0,
                radiusY: rect.height() / 2.0,
            };

            if active {
                self.accent_brush.SetOpacity(0.28 * alpha);
                self.target.FillRoundedRectangle(&pill, &self.accent_brush);
                self.accent_brush.SetOpacity(0.65 * alpha);
                self.target
                    .DrawRoundedRectangle(&pill, &self.accent_brush, 1.0, None);
                self.accent_brush.SetOpacity(1.0);
            } else {
                self.hover_brush
                    .SetOpacity(if hovered { 0.14 } else { 0.055 } * alpha);
                self.target.FillRoundedRectangle(&pill, &self.hover_brush);
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

            let u16_lbl: Vec<u16> = label.encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &u16_lbl,
                &self.pill_btn_format,
                rect.width(),
                rect.height(),
            ) {
                let text_alpha = if active {
                    1.0
                } else if hovered {
                    0.92
                } else {
                    0.68
                };
                self.title_brush.SetOpacity(text_alpha * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: origin_x + rect.left,
                        y: origin_y + rect.top,
                    },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
    }

    unsafe fn draw_collapsed_media(
        &self,
        origin_x: f32,
        origin_y: f32,
        pill_width: f32,
        pill_height: f32,
        frame: &Frame,
        alpha: f32,
    ) {
        self.draw_art(origin_x, origin_y, &COLLAPSED_ART, COLLAPSED_ART_RADIUS);

        let unread = frame.notification.unread_count();
        let center_text = if frame.timer.running {
            frame.timer.formatted()
        } else {
            current_clock_string()
        };
        let center_u16: Vec<u16> = center_text.encode_utf16().collect();

        if unread > 0 {
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &center_u16,
                &self.clock_collapsed_format,
                66.0,
                pill_height,
            ) {
                if frame.timer.running {
                    self.accent_brush.SetOpacity(alpha);
                    self.target.DrawTextLayout(
                        D2D_POINT_2F { x: origin_x + 36.0, y: origin_y },
                        &layout,
                        &self.accent_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                    self.accent_brush.SetOpacity(1.0);
                } else {
                    self.title_brush.SetOpacity(0.96 * alpha);
                    self.target.DrawTextLayout(
                        D2D_POINT_2F { x: origin_x + 36.0, y: origin_y },
                        &layout,
                        &self.title_brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }

            self.draw_equalizer(
                origin_x,
                origin_y,
                pill_height / 2.0,
                &frame.eq_levels,
                frame.media.is_playing,
                alpha,
                pill_width - 62.0,
                EQUALIZER_BAR_WIDTH,
                EQUALIZER_BAR_GAP,
                3.5,
                14.5,
            );
            if let Some(app_name) = frame.notification.latest_app_name() {
                self.draw_collapsed_notification_badge(
                    origin_x,
                    origin_y,
                    pill_width,
                    pill_height,
                    app_name,
                    unread,
                    alpha,
                );
            }
        } else {
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &center_u16,
                &self.clock_collapsed_format,
                76.0,
                pill_height,
            ) {
                self.title_brush.SetOpacity(0.97 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + 42.0, y: origin_y },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            self.draw_equalizer(
                origin_x,
                origin_y,
                pill_height / 2.0,
                &frame.eq_levels,
                frame.media.is_playing,
                alpha,
                EQUALIZER_RIGHT,
                EQUALIZER_BAR_WIDTH,
                EQUALIZER_BAR_GAP,
                3.5,
                14.5,
            );
        }
    }

    unsafe fn draw_collapsed_idle(
        &self,
        pill_left: f32,
        pill_top: f32,
        pill_width: f32,
        pill_height: f32,
        frame: &Frame,
        alpha: f32,
    ) {
        let dot = D2D1_ELLIPSE {
            point: D2D_POINT_2F {
                x: pill_left + 18.0,
                y: pill_top + pill_height / 2.0,
            },
            radiusX: 4.0,
            radiusY: 4.0,
        };
        self.accent_brush.SetOpacity(0.95 * alpha);
        self.target.FillEllipse(&dot, &self.accent_brush);
        self.accent_brush.SetOpacity(1.0);

        let clock = current_clock_string();
        let clock_u16: Vec<u16> = clock.encode_utf16().collect();
        let unread = frame.notification.unread_count();

        if frame.timer.running {
            // [●] 24:59   10:22  (or [●] 24:59   [Avatar][Badge])
            let t_str = frame.timer.formatted();
            let t_u16: Vec<u16> = t_str.encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &t_u16,
                &self.clock_collapsed_format,
                64.0,
                pill_height,
            ) {
                self.accent_brush.SetOpacity(alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: pill_left + 28.0, y: pill_top },
                    &layout,
                    &self.accent_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                self.accent_brush.SetOpacity(1.0);
            }

            if unread > 0 {
                if let Some(app_name) = frame.notification.latest_app_name() {
                    self.draw_collapsed_notification_badge(
                        pill_left, pill_top, pill_width, pill_height, app_name, unread, alpha,
                    );
                }
            } else if let Ok(layout) = self.dwrite.CreateTextLayout(
                &clock_u16,
                &self.clock_collapsed_format,
                66.0,
                pill_height,
            ) {
                self.title_brush.SetOpacity(0.95 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: pill_left + pill_width - 74.0, y: pill_top },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            return;
        }

        if unread > 0 {
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &clock_u16,
                &self.clock_collapsed_format,
                68.0,
                pill_height,
            ) {
                self.title_brush.SetOpacity(0.98 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: pill_left + 30.0, y: pill_top },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            if let Some(app_name) = frame.notification.latest_app_name() {
                self.draw_collapsed_notification_badge(
                    pill_left, pill_top, pill_width, pill_height, app_name, unread, alpha,
                );
            }
        } else {
            let label: Vec<u16> = "Zekrima".encode_utf16().collect();
            if let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&label, &self.hud_format, 72.0, pill_height)
            {
                self.hud_text_brush.SetOpacity(0.82 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: pill_left + 29.0,
                        y: pill_top + (pill_height - 15.0) / 2.0,
                    },
                    &layout,
                    &self.hud_text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &clock_u16,
                &self.clock_collapsed_format,
                66.0,
                pill_height,
            ) {
                self.title_brush.SetOpacity(0.98 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: pill_left + pill_width - 74.0, y: pill_top },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
    }

    unsafe fn draw_expanded_media(&self, origin_x: f32, origin_y: f32, frame: &Frame, alpha: f32) {
        self.draw_art(origin_x, origin_y, &EXPANDED_ART, EXPANDED_ART_RADIUS);
        self.draw_text(origin_x, origin_y, alpha);

        let hovered_ctrl = match frame.hovered_target {
            Some(HitTarget::MediaControl(c)) => Some(c),
            _ => None,
        };
        self.draw_controls(origin_x, origin_y, alpha, frame.media.is_playing, hovered_ctrl);

        // Live 5-bar FFT equalizer
        self.draw_equalizer(
            origin_x,
            origin_y,
            CONTROLS_TOP + 15.0,
            &frame.eq_levels,
            frame.media.is_playing,
            alpha,
            344.0,
            4.0,
            3.5,
            4.0,
            20.0,
        );

        // Bold clock at bottom-right
        let clock = current_clock_string();
        let clock_u16: Vec<u16> = clock.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&clock_u16, &self.clock_media_format, 74.0, 30.0)
        {
            self.title_brush.SetOpacity(0.96 * alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: origin_x + 350.0,
                    y: origin_y + CONTROLS_TOP,
                },
                &layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    unsafe fn draw_idle_expanded(
        &self,
        pill_left: f32,
        pill_top: f32,
        pill_width: f32,
        pill_height: f32,
        system: &SystemSnapshot,
        notif: &NotificationSnapshot,
        alpha: f32,
    ) {
        let pad_x = 22.0f32;
        let content_w = (pill_width - pad_x * 2.0).max(40.0);

        // Hero clock (`34px` Bold) + short date & status
        let st = GetLocalTime();
        let clock = format!("{:02}:{:02}", st.wHour, st.wMinute);
        let clock_u16: Vec<u16> = clock.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&clock_u16, &self.clock_hero_format, 125.0, 44.0)
        {
            self.title_brush.SetOpacity(alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: pill_left + pad_x,
                    y: pill_top + 48.0,
                },
                &layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let date_str = format_short_date(&st);
        let sub_text = if notif.unread_count() > 0 {
            format!("{date_str}  •  {} unread", notif.unread_count())
        } else {
            format!("{date_str}  •  No media playing")
        };
        let sub: Vec<u16> = sub_text.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&sub, &self.artist_format, content_w - 122.0, 24.0)
        {
            self.artist_brush.SetOpacity(0.78 * alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: pill_left + pad_x + 118.0,
                    y: pill_top + 62.0,
                },
                &layout,
                &self.artist_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        // Live system volume bar
        let bar_y = pill_top + pill_height - 24.0;
        let bar_h = 6.0f32;
        let vol_label = if system.muted {
            "Volume  •  Muted".to_string()
        } else {
            format!("Volume  •  {}%", (system.volume * 100.0).round() as u32)
        };
        let vol_u16: Vec<u16> = vol_label.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&vol_u16, &self.hud_format, content_w, 16.0)
        {
            self.hud_text_brush.SetOpacity(0.66 * alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: pill_left + pad_x,
                    y: bar_y - 19.0,
                },
                &layout,
                &self.hud_text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let track = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: pill_left + pad_x,
                top: bar_y,
                right: pill_left + pad_x + content_w,
                bottom: bar_y + bar_h,
            },
            radiusX: bar_h / 2.0,
            radiusY: bar_h / 2.0,
        };
        self.hud_track_brush.SetOpacity(alpha);
        self.target
            .FillRoundedRectangle(&track, &self.hud_track_brush);

        let fill_w = (content_w * system.volume.clamp(0.0, 1.0))
            .max(if system.volume > 0.001 { bar_h } else { 0.0 });
        if fill_w > 0.0 {
            let fill = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pill_left + pad_x,
                    top: bar_y,
                    right: pill_left + pad_x + fill_w,
                    bottom: bar_y + bar_h,
                },
                radiusX: bar_h / 2.0,
                radiusY: bar_h / 2.0,
            };
            self.hud_fill_brush
                .SetOpacity(if system.muted { 0.30 } else { 0.88 } * alpha);
            self.target
                .FillRoundedRectangle(&fill, &self.hud_fill_brush);
        }
    }

    /// Focus / Pomodoro Timer panel.
    unsafe fn draw_timer_panel(
        &self,
        origin_x: f32,
        origin_y: f32,
        timer: &PomodoroTimer,
        hovered: Option<HitTarget>,
        alpha: f32,
    ) {
        let time_str = timer.formatted();
        let time_u16: Vec<u16> = time_str.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&time_u16, &self.clock_hero_format, 180.0, 44.0)
        {
            self.title_brush.SetOpacity(alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: origin_x + 22.0,
                    y: origin_y + 44.0,
                },
                &layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let status = if timer.running {
            "Focus • Running"
        } else if timer.completed {
            "Focus • Complete!"
        } else if timer.remaining_secs < timer.total_secs {
            "Focus • Paused"
        } else {
            "Focus • Ready"
        };
        let status_u16: Vec<u16> = status.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&status_u16, &self.notif_app_format, 180.0, 18.0)
        {
            if timer.running {
                self.accent_brush.SetOpacity(0.95 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + 22.0, y: origin_y + 88.0 },
                    &layout,
                    &self.accent_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                self.accent_brush.SetOpacity(1.0);
            } else {
                self.artist_brush.SetOpacity(0.75 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + 22.0, y: origin_y + 88.0 },
                    &layout,
                    &self.artist_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }

        // Start / Pause accent button
        let toggle_lbl = if timer.running { "Pause" } else { "Start" };
        let toggle_hovered = hovered == Some(HitTarget::TimerToggle);
        let toggle_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: origin_x + TIMER_TOGGLE_BTN.left,
                top: origin_y + TIMER_TOGGLE_BTN.top,
                right: origin_x + TIMER_TOGGLE_BTN.right,
                bottom: origin_y + TIMER_TOGGLE_BTN.bottom,
            },
            radiusX: 15.0,
            radiusY: 15.0,
        };
        self.accent_brush
            .SetOpacity(if toggle_hovered { 0.96 } else { 0.80 } * alpha);
        self.target
            .FillRoundedRectangle(&toggle_rect, &self.accent_brush);
        self.accent_brush.SetOpacity(1.0);

        let t_u16: Vec<u16> = toggle_lbl.encode_utf16().collect();
        if let Ok(layout) = self.dwrite.CreateTextLayout(
            &t_u16,
            &self.pill_btn_format,
            TIMER_TOGGLE_BTN.width(),
            TIMER_TOGGLE_BTN.height(),
        ) {
            self.title_brush.SetOpacity(alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: origin_x + TIMER_TOGGLE_BTN.left,
                    y: origin_y + TIMER_TOGGLE_BTN.top,
                },
                &layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        // Reset button
        self.draw_action_pill(
            origin_x,
            origin_y,
            TIMER_RESET_BTN,
            "Reset",
            hovered == Some(HitTarget::TimerReset),
            alpha,
        );

        // 4 preset buttons (5m, 15m, 25m, 45m)
        for (i, &secs) in TIMER_PRESETS_SECS.iter().enumerate() {
            let rect = timer_preset_rect(i);
            let selected = timer.total_secs == secs;
            let is_hov = hovered == Some(HitTarget::TimerPreset(secs));
            let lbl = format!("{}m", secs / 60);

            let d2d_r = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: origin_x + rect.left,
                    top: origin_y + rect.top,
                    right: origin_x + rect.right,
                    bottom: origin_y + rect.bottom,
                },
                radiusX: 12.0,
                radiusY: 12.0,
            };
            self.hover_brush.SetOpacity(
                if selected {
                    0.20
                } else if is_hov {
                    0.15
                } else {
                    0.07
                } * alpha,
            );
            self.target.FillRoundedRectangle(&d2d_r, &self.hover_brush);

            let lbl_u16: Vec<u16> = lbl.encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &lbl_u16,
                &self.pill_btn_format,
                rect.width(),
                rect.height(),
            ) {
                self.title_brush
                    .SetOpacity(if selected || is_hov { 1.0 } else { 0.72 } * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: origin_x + rect.left,
                        y: origin_y + rect.top,
                    },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }

        // Bottom progress bar
        let bar_h = TIMER_PROGRESS_BAR.height();
        let track = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: origin_x + TIMER_PROGRESS_BAR.left,
                top: origin_y + TIMER_PROGRESS_BAR.top,
                right: origin_x + TIMER_PROGRESS_BAR.right,
                bottom: origin_y + TIMER_PROGRESS_BAR.bottom,
            },
            radiusX: bar_h / 2.0,
            radiusY: bar_h / 2.0,
        };
        self.hud_track_brush.SetOpacity(alpha);
        self.target
            .FillRoundedRectangle(&track, &self.hud_track_brush);

        let fill_w = (TIMER_PROGRESS_BAR.width() * timer.progress()).max(0.0);
        if fill_w > 1.0 {
            let fill = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: origin_x + TIMER_PROGRESS_BAR.left,
                    top: origin_y + TIMER_PROGRESS_BAR.top,
                    right: origin_x + TIMER_PROGRESS_BAR.left + fill_w,
                    bottom: origin_y + TIMER_PROGRESS_BAR.bottom,
                },
                radiusX: bar_h / 2.0,
                radiusY: bar_h / 2.0,
            };
            self.accent_brush.SetOpacity(0.92 * alpha);
            self.target.FillRoundedRectangle(&fill, &self.accent_brush);
            self.accent_brush.SetOpacity(1.0);
        }
    }

    /// Quick Clipboard History panel (click any row to copy back).
    unsafe fn draw_clipboard_panel(
        &self,
        origin_x: f32,
        origin_y: f32,
        pill_width: f32,
        items: &[String],
        copied_flash: bool,
        hovered: Option<HitTarget>,
        alpha: f32,
    ) {
        let header = if copied_flash {
            "Copied to clipboard!".to_string()
        } else {
            format!("Clipboard History ({})", items.len())
        };
        let header_u16: Vec<u16> = header.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&header_u16, &self.hud_format, 260.0, 18.0)
        {
            if copied_flash {
                self.dynamic_brush.SetColor(&D2D1_COLOR_F {
                    r: 34.0 / 255.0,
                    g: 197.0 / 255.0,
                    b: 94.0 / 255.0,
                    a: alpha,
                });
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + 20.0, y: origin_y + 41.0 },
                    &layout,
                    &self.dynamic_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            } else {
                self.hud_text_brush.SetOpacity(0.82 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + 20.0, y: origin_y + 41.0 },
                    &layout,
                    &self.hud_text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }

        if !items.is_empty() {
            self.draw_action_pill(
                origin_x,
                origin_y,
                CLIP_CLEAR_BTN,
                "Clear",
                hovered == Some(HitTarget::ClipClear),
                alpha,
            );
        }

        if items.is_empty() {
            let m1: Vec<u16> = "Clipboard history is empty".encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &m1,
                &self.center_title_format,
                pill_width - 32.0,
                22.0,
            ) {
                self.title_brush.SetOpacity(0.88 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + 16.0, y: origin_y + 82.0 },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            let m2: Vec<u16> = "Copy any text (Ctrl+C) to pin it here".encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &m2,
                &self.center_sub_format,
                pill_width - 32.0,
                20.0,
            ) {
                self.artist_brush.SetOpacity(0.65 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + 16.0, y: origin_y + 106.0 },
                    &layout,
                    &self.artist_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            return;
        }

        for (i, text) in items.iter().take(CLIP_MAX_VISIBLE).enumerate() {
            let rect = clip_row_rect(i);
            let is_hov = hovered == Some(HitTarget::ClipCopy(i));

            let row = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: origin_x + rect.left,
                    top: origin_y + rect.top,
                    right: origin_x + rect.right,
                    bottom: origin_y + rect.bottom,
                },
                radiusX: 8.0,
                radiusY: 8.0,
            };
            self.hover_brush
                .SetOpacity(if is_hov { 0.14 } else { 0.06 } * alpha);
            self.target.FillRoundedRectangle(&row, &self.hover_brush);

            // Index badge (`#1`, `#2`, `#3`)
            let idx_str = format!("#{}", i + 1);
            let idx_u16: Vec<u16> = idx_str.encode_utf16().collect();
            if let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&idx_u16, &self.badge_format, 24.0, rect.height())
            {
                self.accent_brush.SetOpacity(0.90 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: origin_x + rect.left + 6.0,
                        y: origin_y + rect.top,
                    },
                    &layout,
                    &self.accent_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                self.accent_brush.SetOpacity(1.0);
            }

            let single_line = text.replace(['\r', '\n', '\t'], " ");
            if let Ok(layout) = create_layout(
                &self.dwrite,
                &self.notif_body_format,
                &self.notif_body_trim,
                &single_line,
                rect.width() - 42.0,
                18.0,
            ) {
                self.title_brush
                    .SetOpacity(if is_hov { 1.0 } else { 0.86 } * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: origin_x + rect.left + 34.0,
                        y: origin_y + rect.top + 5.0,
                    },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
    }

    /// Compact System Monitor panel (CPU, RAM, Battery/Power).
    unsafe fn draw_sysmon_panel(
        &self,
        origin_x: f32,
        origin_y: f32,
        pill_width: f32,
        stats: &SystemStats,
        alpha: f32,
    ) {
        let pad_x = 22.0f32;
        let bar_w = (pill_width - pad_x * 2.0).max(80.0);

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

        let rows: [(&str, String, f32, bool); 3] = [
            ("CPU", format!("{:.0}%", stats.cpu_percent), stats.cpu_percent / 100.0, false),
            ("Memory", ram_val, stats.ram_percent / 100.0, false),
            (power_lbl, power_val, stats.battery_percent / 100.0, true),
        ];

        for (i, (label, value, frac, is_battery)) in rows.iter().enumerate() {
            let y = origin_y + 46.0 + i as f32 * 36.0;

            let l_u16: Vec<u16> = label.encode_utf16().collect();
            if let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&l_u16, &self.hud_format, bar_w, 16.0)
            {
                self.hud_text_brush.SetOpacity(0.90 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + pad_x, y },
                    &layout,
                    &self.hud_text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            let v_u16: Vec<u16> = value.encode_utf16().collect();
            if let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&v_u16, &self.hud_value_format, bar_w, 16.0)
            {
                self.hud_text_brush.SetOpacity(0.75 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: origin_x + pad_x, y },
                    &layout,
                    &self.hud_text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            let bar_y = y + 19.0;
            let bar_h = 7.0f32;
            let track = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: origin_x + pad_x,
                    top: bar_y,
                    right: origin_x + pad_x + bar_w,
                    bottom: bar_y + bar_h,
                },
                radiusX: bar_h / 2.0,
                radiusY: bar_h / 2.0,
            };
            self.hud_track_brush.SetOpacity(alpha);
            self.target
                .FillRoundedRectangle(&track, &self.hud_track_brush);

            let clamped = frac.clamp(0.0, 1.0);
            let fill_w = (bar_w * clamped).max(if clamped > 0.005 { bar_h } else { 0.0 });
            if fill_w > 0.0 {
                let (r, g, b) = metric_bar_color(clamped, *is_battery);
                self.dynamic_brush
                    .SetColor(&D2D1_COLOR_F { r, g, b, a: 0.92 * alpha });
                let fill = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: origin_x + pad_x,
                        top: bar_y,
                        right: origin_x + pad_x + fill_w,
                        bottom: bar_y + bar_h,
                    },
                    radiusX: bar_h / 2.0,
                    radiusY: bar_h / 2.0,
                };
                self.target.FillRoundedRectangle(&fill, &self.dynamic_brush);
            }
        }
    }

    unsafe fn draw_art(&self, origin_x: f32, origin_y: f32, rect: &RectF, radius: f32) {
        let destination = D2D_RECT_F {
            left: origin_x + rect.left,
            top: origin_y + rect.top,
            right: origin_x + rect.right,
            bottom: origin_y + rect.bottom,
        };

        if let Some(bitmap) = &self.art_bitmap {
            self.target.DrawBitmap(
                bitmap,
                Some(&destination),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
            return;
        }

        let placeholder = D2D1_ROUNDED_RECT {
            rect: destination,
            radiusX: radius,
            radiusY: radius,
        };
        self.target
            .FillRoundedRectangle(&placeholder, &self.placeholder_brush);
    }

    unsafe fn draw_text(&self, origin_x: f32, origin_y: f32, alpha: f32) {
        if let Some(layout) = &self.title_layout {
            self.title_brush.SetOpacity(alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: origin_x + TITLE_BOX.left,
                    y: origin_y + TITLE_BOX.top,
                },
                layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        if let Some(layout) = &self.artist_layout {
            self.artist_brush.SetOpacity(alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: origin_x + ARTIST_BOX.left,
                    y: origin_y + ARTIST_BOX.top,
                },
                layout,
                &self.artist_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    unsafe fn draw_controls(
        &self,
        origin_x: f32,
        origin_y: f32,
        alpha: f32,
        is_playing: bool,
        hovered: Option<MediaControl>,
    ) {
        let transform = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: origin_x,
            M32: origin_y,
        };
        let identity = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        };
        self.target.SetTransform(&transform);

        if let Some(control) = hovered {
            let rect = control_rect(control);
            let (center_x, center_y) = rect.center();
            self.hover_brush.SetOpacity(0.12 * alpha);
            let ellipse = D2D1_ELLIPSE {
                point: D2D_POINT_2F {
                    x: center_x,
                    y: center_y,
                },
                radiusX: HOVER_RADIUS,
                radiusY: HOVER_RADIUS,
            };
            self.target.FillEllipse(&ellipse, &self.hover_brush);
        }

        self.icon_brush.SetOpacity(0.92 * alpha);

        self.target.FillGeometry(
            &self.previous_icon,
            &self.icon_brush,
            None::<&ID2D1Brush>,
        );

        if is_playing {
            let left = control_rect(MediaControl::PlayPause).left;
            for offset in [10.5f32, 16.5] {
                let bar = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: left + offset,
                        top: CONTROLS_TOP + 9.0,
                        right: left + offset + 3.0,
                        bottom: CONTROLS_TOP + 21.0,
                    },
                    radiusX: 1.5,
                    radiusY: 1.5,
                };
                self.target.FillRoundedRectangle(&bar, &self.icon_brush);
            }
        } else {
            self.target
                .FillGeometry(&self.play_icon, &self.icon_brush, None::<&ID2D1Brush>);
        }

        self.target
            .FillGeometry(&self.next_icon, &self.icon_brush, None::<&ID2D1Brush>);

        self.target.SetTransform(&identity);
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_equalizer(
        &self,
        origin_x: f32,
        origin_y: f32,
        center_y: f32,
        levels: &[f32; EQUALIZER_BAR_COUNT],
        is_playing: bool,
        alpha: f32,
        right_edge: f32,
        bar_width: f32,
        bar_gap: f32,
        min_height: f32,
        max_delta: f32,
    ) {
        let total_width = EQUALIZER_BAR_COUNT as f32 * bar_width
            + (EQUALIZER_BAR_COUNT as f32 - 1.0) * bar_gap;
        let start_x = right_edge - total_width;
        let bar_alpha = if is_playing { alpha } else { alpha * 0.45 };

        for index in 0..EQUALIZER_BAR_COUNT {
            let strength = if is_playing {
                levels[index].clamp(0.0, 1.0)
            } else {
                PAUSED_EQUALIZER[index]
            };
            let bar_height = min_height + strength * max_delta;
            let half_h = bar_height / 2.0;
            let x = start_x + index as f32 * (bar_width + bar_gap);
            let bar = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: origin_x + x,
                    top: origin_y + center_y - half_h,
                    right: origin_x + x + bar_width,
                    bottom: origin_y + center_y + half_h,
                },
                radiusX: bar_width / 2.0,
                radiusY: bar_width / 2.0,
            };

            if index % 2 == 0 {
                self.accent_brush.SetOpacity(bar_alpha);
                self.target.FillRoundedRectangle(&bar, &self.accent_brush);
            } else {
                self.equalizer_accent_brush.SetOpacity(bar_alpha * 0.92);
                self.target
                    .FillRoundedRectangle(&bar, &self.equalizer_accent_brush);
            }
        }
        self.accent_brush.SetOpacity(1.0);
    }

    unsafe fn draw_collapsed_notification_badge(
        &self,
        pill_left: f32,
        pill_top: f32,
        pill_width: f32,
        pill_height: f32,
        app_name: &str,
        count: usize,
        alpha: f32,
    ) {
        let cy = pill_top + pill_height / 2.0;

        let count_text = if count > 9 {
            "9+".to_string()
        } else {
            count.to_string()
        };
        let count_w = if count > 9 { 24.0 } else { 20.0 };
        let count_right = pill_left + pill_width - 9.0;
        let count_left = count_right - count_w;
        let count_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: count_left,
                top: cy - 10.0,
                right: count_right,
                bottom: cy + 10.0,
            },
            radiusX: 10.0,
            radiusY: 10.0,
        };
        self.badge_red_brush.SetOpacity(alpha);
        self.target
            .FillRoundedRectangle(&count_rect, &self.badge_red_brush);

        let count_u16: Vec<u16> = count_text.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&count_u16, &self.badge_format, count_w, 20.0)
        {
            self.title_brush.SetOpacity(alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: count_left,
                    y: cy - 10.0,
                },
                &layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let avatar_cx = count_left - 14.0;
        let (r, g, b) = app_accent_rgb(app_name);
        self.dynamic_brush.SetColor(&D2D1_COLOR_F { r, g, b, a: alpha });
        let avatar = D2D1_ELLIPSE {
            point: D2D_POINT_2F { x: avatar_cx, y: cy },
            radiusX: 10.0,
            radiusY: 10.0,
        };
        self.target.FillEllipse(&avatar, &self.dynamic_brush);

        let initial = app_initial(app_name);
        let init_u16: Vec<u16> = initial.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&init_u16, &self.badge_format, 20.0, 20.0)
        {
            self.title_brush.SetOpacity(alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: avatar_cx - 10.0,
                    y: cy - 10.0,
                },
                &layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    unsafe fn draw_notification_toast(
        &self,
        pill_left: f32,
        pill_top: f32,
        item: &NotificationItem,
        progress: f32,
        hovered: Option<HitTarget>,
    ) {
        let (ar, ag, ab) = app_accent_rgb(&item.app);

        let avatar_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: pill_left + TOAST_AVATAR.left,
                top: pill_top + TOAST_AVATAR.top,
                right: pill_left + TOAST_AVATAR.right,
                bottom: pill_top + TOAST_AVATAR.bottom,
            },
            radiusX: 10.0,
            radiusY: 10.0,
        };
        self.dynamic_brush.SetColor(&D2D1_COLOR_F {
            r: ar,
            g: ag,
            b: ab,
            a: 0.36,
        });
        self.target
            .FillRoundedRectangle(&avatar_rect, &self.dynamic_brush);
        self.dynamic_brush.SetColor(&D2D1_COLOR_F {
            r: ar,
            g: ag,
            b: ab,
            a: 0.55,
        });
        self.target
            .DrawRoundedRectangle(&avatar_rect, &self.dynamic_brush, 1.0, None);

        let init_u16: Vec<u16> = app_initial(&item.app).encode_utf16().collect();
        if let Ok(layout) = self.dwrite.CreateTextLayout(
            &init_u16,
            &self.badge_format,
            TOAST_AVATAR.width(),
            TOAST_AVATAR.height(),
        ) {
            self.title_brush.SetOpacity(1.0);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: pill_left + TOAST_AVATAR.left,
                    y: pill_top + TOAST_AVATAR.top,
                },
                &layout,
                &self.title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let text_left = pill_left + 64.0;
        let text_w = (TOAST_DISMISS_BTN.left - 68.0).max(60.0);
        let header = format!("{}  •  just now", item.app);
        let header_u16: Vec<u16> = header.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&header_u16, &self.notif_app_format, text_w, 16.0)
        {
            self.dynamic_brush.SetColor(&D2D1_COLOR_F {
                r: ar,
                g: ag,
                b: ab,
                a: 1.0,
            });
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: text_left,
                    y: pill_top + 13.0,
                },
                &layout,
                &self.dynamic_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let title_str = if item.title.is_empty() {
            "Notification"
        } else {
            item.title.as_str()
        };
        if let Ok(layout) = create_layout(
            &self.dwrite,
            &self.notif_title_format,
            &self.notif_title_trim,
            title_str,
            text_w,
            19.0,
        ) {
            self.notif_title_brush.SetOpacity(1.0);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: text_left,
                    y: pill_top + 30.0,
                },
                &layout,
                &self.notif_title_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        if !item.body.is_empty() {
            if let Ok(layout) = create_layout(
                &self.dwrite,
                &self.notif_body_format,
                &self.notif_body_trim,
                &item.body,
                text_w + 18.0,
                30.0,
            ) {
                self.notif_body_brush.SetOpacity(1.0);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: text_left,
                        y: pill_top + 49.0,
                    },
                    &layout,
                    &self.notif_body_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }

        self.draw_dismiss_x(
            pill_left,
            pill_top,
            TOAST_DISMISS_BTN,
            hovered == Some(HitTarget::ToastDismiss(item.id)),
            1.0,
        );

        let bar_w = TOAST_PROGRESS_RECT.width() * progress.clamp(0.0, 1.0);
        if bar_w > 1.0 {
            let bar = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pill_left + TOAST_PROGRESS_RECT.left,
                    top: pill_top + TOAST_PROGRESS_RECT.top,
                    right: pill_left + TOAST_PROGRESS_RECT.left + bar_w,
                    bottom: pill_top + TOAST_PROGRESS_RECT.bottom,
                },
                radiusX: 1.25,
                radiusY: 1.25,
            };
            self.dynamic_brush.SetColor(&D2D1_COLOR_F {
                r: ar,
                g: ag,
                b: ab,
                a: 0.72,
            });
            self.target.FillRoundedRectangle(&bar, &self.dynamic_brush);
        }
    }

    unsafe fn draw_notifications_panel(
        &self,
        pill_left: f32,
        pill_top: f32,
        pill_width: f32,
        notif: &NotificationSnapshot,
        hovered: Option<HitTarget>,
        alpha: f32,
    ) {
        let unread = notif.unread_count();
        let header_text = if unread > NOTIF_CARD_MAX_VISIBLE {
            format!(
                "Notifications ({unread})  •  +{} more",
                unread - NOTIF_CARD_MAX_VISIBLE
            )
        } else if unread > 0 {
            format!("Notifications ({unread})")
        } else {
            "Notifications".to_string()
        };
        let header_u16: Vec<u16> = header_text.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&header_u16, &self.hud_format, 260.0, 18.0)
        {
            self.hud_text_brush.SetOpacity(0.84 * alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: pill_left + 20.0,
                    y: pill_top + 41.0,
                },
                &layout,
                &self.hud_text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        if !notif.items.is_empty() {
            self.draw_action_pill(
                pill_left,
                pill_top,
                NOTIF_CLEAR_BTN,
                "Clear all",
                hovered == Some(HitTarget::ClearAllNotifications),
                alpha,
            );
        }

        if !notif.access_granted && notif.items.is_empty() {
            let msg1: Vec<u16> = "Notification access required".encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &msg1,
                &self.center_title_format,
                pill_width - 32.0,
                22.0,
            ) {
                self.title_brush.SetOpacity(0.92 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: pill_left + 16.0,
                        y: pill_top + 66.0,
                    },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            let msg2: Vec<u16> =
                "Enable desktop app access in Windows Privacy Settings"
                    .encode_utf16()
                    .collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &msg2,
                &self.center_sub_format,
                pill_width - 32.0,
                20.0,
            ) {
                self.artist_brush.SetOpacity(0.75 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: pill_left + 16.0,
                        y: pill_top + 88.0,
                    },
                    &layout,
                    &self.artist_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            let btn_hovered = hovered == Some(HitTarget::OpenNotificationSettings);
            let btn_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pill_left + NOTIF_SETTINGS_BTN.left,
                    top: pill_top + NOTIF_SETTINGS_BTN.top,
                    right: pill_left + NOTIF_SETTINGS_BTN.right,
                    bottom: pill_top + NOTIF_SETTINGS_BTN.bottom,
                },
                radiusX: 15.0,
                radiusY: 15.0,
            };
            self.accent_brush
                .SetOpacity(if btn_hovered { 0.95 } else { 0.78 } * alpha);
            self.target
                .FillRoundedRectangle(&btn_rect, &self.accent_brush);
            self.accent_brush.SetOpacity(1.0);

            let btn_lbl: Vec<u16> = "Open Windows Settings".encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &btn_lbl,
                &self.pill_btn_format,
                NOTIF_SETTINGS_BTN.width(),
                NOTIF_SETTINGS_BTN.height(),
            ) {
                self.title_brush.SetOpacity(alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: pill_left + NOTIF_SETTINGS_BTN.left,
                        y: pill_top + NOTIF_SETTINGS_BTN.top,
                    },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            return;
        }

        if notif.items.is_empty() {
            let msg1: Vec<u16> = "No notifications".encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &msg1,
                &self.center_title_format,
                pill_width - 32.0,
                22.0,
            ) {
                self.title_brush.SetOpacity(0.90 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: pill_left + 16.0,
                        y: pill_top + 82.0,
                    },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            let msg2: Vec<u16> = "All caught up!".encode_utf16().collect();
            if let Ok(layout) = self.dwrite.CreateTextLayout(
                &msg2,
                &self.center_sub_format,
                pill_width - 32.0,
                20.0,
            ) {
                self.artist_brush.SetOpacity(0.65 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: pill_left + 16.0,
                        y: pill_top + 106.0,
                    },
                    &layout,
                    &self.artist_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
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
            let card_hovered = hovered == Some(HitTarget::NotifActivate(item.id));
            let dismiss_hovered = hovered == Some(HitTarget::NotifDismiss(item.id));

            let card_d2d = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pill_left + card.left,
                    top: pill_top + card.top,
                    right: pill_left + card.right,
                    bottom: pill_top + card.bottom,
                },
                radiusX: 10.0,
                radiusY: 10.0,
            };
            self.hover_brush
                .SetOpacity(if card_hovered { 0.11 } else { 0.055 } * alpha);
            self.target
                .FillRoundedRectangle(&card_d2d, &self.hover_brush);

            let (ar, ag, ab) = app_accent_rgb(&item.app);
            let av_left = pill_left + card.left + 8.0;
            let av_top = pill_top + card.top + 8.0;
            let av_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: av_left,
                    top: av_top,
                    right: av_left + 25.0,
                    bottom: av_top + 25.0,
                },
                radiusX: 6.0,
                radiusY: 6.0,
            };
            self.dynamic_brush.SetColor(&D2D1_COLOR_F {
                r: ar,
                g: ag,
                b: ab,
                a: 0.90 * alpha,
            });
            self.target
                .FillRoundedRectangle(&av_rect, &self.dynamic_brush);

            let init_u16: Vec<u16> = app_initial(&item.app).encode_utf16().collect();
            if let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&init_u16, &self.badge_format, 25.0, 25.0)
            {
                self.title_brush.SetOpacity(alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F { x: av_left, y: av_top },
                    &layout,
                    &self.title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            let tx = pill_left + card.left + 40.0;
            let tw = (dismiss_rect.left - (card.left + 44.0)).max(60.0);
            let meta = format!("{}  •  {}", item.app, format_relative_time(item.timestamp_ms));
            let meta_u16: Vec<u16> = meta.encode_utf16().collect();
            if let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&meta_u16, &self.notif_app_format, tw, 14.0)
            {
                self.dynamic_brush.SetColor(&D2D1_COLOR_F {
                    r: ar,
                    g: ag,
                    b: ab,
                    a: 0.95 * alpha,
                });
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: tx,
                        y: pill_top + card.top + 4.0,
                    },
                    &layout,
                    &self.dynamic_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            let line2 = if item.body.is_empty() {
                item.title.clone()
            } else if item.title.is_empty() {
                item.body.clone()
            } else {
                format!("{} — {}", item.title, item.body)
            };
            if let Ok(layout) = create_layout(
                &self.dwrite,
                &self.notif_title_format,
                &self.notif_title_trim,
                &line2,
                tw,
                18.0,
            ) {
                self.notif_title_brush.SetOpacity(0.94 * alpha);
                self.target.DrawTextLayout(
                    D2D_POINT_2F {
                        x: tx,
                        y: pill_top + card.top + 19.0,
                    },
                    &layout,
                    &self.notif_title_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            self.draw_dismiss_x(pill_left, pill_top, dismiss_rect, dismiss_hovered, alpha);
        }
    }

    unsafe fn draw_action_pill(
        &self,
        origin_x: f32,
        origin_y: f32,
        rect: RectF,
        label: &str,
        hovered: bool,
        alpha: f32,
    ) {
        let d2d_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: origin_x + rect.left,
                top: origin_y + rect.top,
                right: origin_x + rect.right,
                bottom: origin_y + rect.bottom,
            },
            radiusX: rect.height() / 2.0,
            radiusY: rect.height() / 2.0,
        };
        self.hover_brush
            .SetOpacity(if hovered { 0.20 } else { 0.10 } * alpha);
        self.target
            .FillRoundedRectangle(&d2d_rect, &self.hover_brush);

        let u16_lbl: Vec<u16> = label.encode_utf16().collect();
        if let Ok(layout) = self.dwrite.CreateTextLayout(
            &u16_lbl,
            &self.pill_btn_format,
            rect.width(),
            rect.height(),
        ) {
            self.hud_text_brush
                .SetOpacity(if hovered { 1.0 } else { 0.82 } * alpha);
            self.target.DrawTextLayout(
                D2D_POINT_2F {
                    x: origin_x + rect.left,
                    y: origin_y + rect.top,
                },
                &layout,
                &self.hud_text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    unsafe fn draw_dismiss_x(
        &self,
        origin_x: f32,
        origin_y: f32,
        rect: RectF,
        hovered: bool,
        alpha: f32,
    ) {
        let (cx, cy) = rect.center();
        let x = origin_x + cx;
        let y = origin_y + cy;

        if hovered {
            let circle = D2D1_ELLIPSE {
                point: D2D_POINT_2F { x, y },
                radiusX: 10.5,
                radiusY: 10.5,
            };
            self.hover_brush.SetOpacity(0.16 * alpha);
            self.target.FillEllipse(&circle, &self.hover_brush);
        }

        let r = 3.8;
        self.icon_brush
            .SetOpacity(if hovered { 0.92 } else { 0.45 } * alpha);
        self.target.DrawLine(
            D2D_POINT_2F { x: x - r, y: y - r },
            D2D_POINT_2F { x: x + r, y: y + r },
            &self.icon_brush,
            1.6,
            None,
        );
        self.target.DrawLine(
            D2D_POINT_2F { x: x + r, y: y - r },
            D2D_POINT_2F { x: x - r, y: y + r },
            &self.icon_brush,
            1.6,
            None,
        );
    }

    unsafe fn draw_hud(
        &self,
        pill_left: f32,
        pill_top: f32,
        pill_width: f32,
        pill_height: f32,
        hud: &HudState,
    ) {
        let pad_x = 20.0f32;
        let bar_h = 6.0f32;
        let bar_w = (pill_width - pad_x * 2.0).max(40.0);
        let bar_x = pill_left + (pill_width - bar_w) / 2.0;
        let bar_y = pill_top + (pill_height * 0.56).max(24.0);

        let label = match hud.kind {
            HudKind::Volume => "Volume",
            HudKind::Brightness => "Brightness",
        };
        let value_text = if hud.kind == HudKind::Volume && hud.muted {
            "Muted".to_string()
        } else {
            format!("{}%", (hud.value.clamp(0.0, 1.0) * 100.0).round() as u32)
        };

        let label_y = (bar_y - 20.0).max(pill_top + 8.0);
        let label_u16: Vec<u16> = label.encode_utf16().collect();
        if let Ok(layout) = self
            .dwrite
            .CreateTextLayout(&label_u16, &self.hud_format, bar_w, 18.0)
        {
            self.hud_text_brush.SetOpacity(0.92);
            self.target.DrawTextLayout(
                D2D_POINT_2F { x: bar_x, y: label_y },
                &layout,
                &self.hud_text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let value_u16: Vec<u16> = value_text.encode_utf16().collect();
        if let Ok(layout) =
            self.dwrite
                .CreateTextLayout(&value_u16, &self.hud_value_format, bar_w, 18.0)
        {
            self.hud_text_brush.SetOpacity(0.75);
            self.target.DrawTextLayout(
                D2D_POINT_2F { x: bar_x, y: label_y },
                &layout,
                &self.hud_text_brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }

        let track = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: bar_x,
                top: bar_y,
                right: bar_x + bar_w,
                bottom: bar_y + bar_h,
            },
            radiusX: bar_h / 2.0,
            radiusY: bar_h / 2.0,
        };
        self.hud_track_brush.SetOpacity(1.0);
        self.target
            .FillRoundedRectangle(&track, &self.hud_track_brush);

        let clamped = hud.value.clamp(0.0, 1.0);
        let fill_w = (bar_w * clamped).max(if clamped > 0.001 { bar_h } else { 0.0 });
        if fill_w > 0.0 {
            let fill = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: bar_x,
                    top: bar_y,
                    right: bar_x + fill_w,
                    bottom: bar_y + bar_h,
                },
                radiusX: bar_h / 2.0,
                radiusY: bar_h / 2.0,
            };
            let alpha = if hud.muted { 0.35 } else { 0.95 };
            self.hud_fill_brush.SetOpacity(alpha);
            self.target
                .FillRoundedRectangle(&fill, &self.hud_fill_brush);
        }
    }

    unsafe fn prepare_art(&mut self, media: &MediaInfo) {
        if media.art_revision == self.art_revision {
            return;
        }
        self.art_revision = media.art_revision;
        self.art_bitmap = None;
        if let Some(art) = &media.album_art {
            if let Ok(bitmap) = self.create_art_bitmap(art) {
                self.art_bitmap = Some(bitmap);
            }
        }
    }

    unsafe fn create_art_bitmap(&self, art: &AlbumArt) -> Result<ID2D1Bitmap> {
        let properties = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        self.target.CreateBitmap(
            D2D_SIZE_U {
                width: art.width,
                height: art.height,
            },
            Some(art.pixels.as_ptr() as *const c_void),
            art.width * 4,
            &properties,
        )
    }

    unsafe fn prepare_text(&mut self, media: &MediaInfo) {
        let title = if media.title.trim().is_empty() {
            "Unknown title"
        } else {
            media.title.as_str()
        };
        let artist = media.artist.as_str();

        if title == self.cached_title && artist == self.cached_artist {
            return;
        }
        self.cached_title = title.to_owned();
        self.cached_artist = artist.to_owned();

        self.title_layout = create_layout(
            &self.dwrite,
            &self.title_format,
            &self.title_trim,
            title,
            TITLE_BOX.width(),
            TITLE_BOX.height(),
        )
        .ok();
        self.artist_layout = create_layout(
            &self.dwrite,
            &self.artist_format,
            &self.artist_trim,
            artist,
            ARTIST_BOX.width(),
            ARTIST_BOX.height(),
        )
        .ok();
    }

    unsafe fn create_surface(&mut self, width: u32, height: u32) -> Result<()> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(HDC::default(), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;

        let mem_dc = CreateCompatibleDC(None);
        if mem_dc.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(dib.0));
            return Err(Error::from_win32());
        }

        let previous_bitmap = SelectObject(mem_dc, HGDIOBJ(dib.0));
        if previous_bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            let _ = DeleteObject(HGDIOBJ(dib.0));
            return Err(Error::from_win32());
        }

        self.mem_dc = mem_dc;
        self.dib = dib;
        self.previous_bitmap = previous_bitmap;
        Ok(())
    }

    unsafe fn present(&self, hwnd: HWND) -> Result<()> {
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err(Error::from_win32());
        }

        let size = SIZE {
            cx: self.width as i32,
            cy: self.height as i32,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };

        let result = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&size),
            self.mem_dc,
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        let _ = ReleaseDC(None, screen_dc);
        result
    }

    unsafe fn release_surface(&mut self) {
        if !self.mem_dc.is_invalid() {
            if !self.previous_bitmap.is_invalid() {
                SelectObject(self.mem_dc, self.previous_bitmap);
            }
            let _ = DeleteDC(self.mem_dc);
            self.mem_dc = HDC::default();
            self.previous_bitmap = HGDIOBJ::default();
        }

        if !self.dib.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            self.dib = HBITMAP::default();
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            self.release_surface();
        }
    }
}

fn metric_bar_color(frac: f32, is_battery: bool) -> (f32, f32, f32) {
    if is_battery {
        if frac < 0.20 {
            (239.0 / 255.0, 68.0 / 255.0, 68.0 / 255.0) // red
        } else if frac < 0.40 {
            (245.0 / 255.0, 158.0 / 255.0, 11.0 / 255.0) // amber
        } else {
            (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0) // green
        }
    } else if frac > 0.85 {
        (235.0 / 255.0, 0.0, 40.0 / 255.0) // crimson
    } else if frac > 0.60 {
        (245.0 / 255.0, 158.0 / 255.0, 11.0 / 255.0) // amber
    } else {
        (34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0) // green
    }
}

unsafe fn font_family(factory: &IDWriteFactory) -> PCWSTR {
    let mut collection: Option<IDWriteFontCollection> = None;
    if factory
        .GetSystemFontCollection(&mut collection, BOOL(0))
        .is_ok()
    {
        if let Some(collection) = collection {
            let mut index = 0u32;
            let mut exists = BOOL(0);
            if collection
                .FindFamilyName(w!("Segoe UI Variable Text"), &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
            {
                return w!("Segoe UI Variable Text");
            }
        }
    }

    w!("Segoe UI")
}

unsafe fn create_layout(
    factory: &IDWriteFactory,
    format: &IDWriteTextFormat,
    sign: &IDWriteInlineObject,
    text: &str,
    width: f32,
    height: f32,
) -> Result<IDWriteTextLayout> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let layout = factory.CreateTextLayout(&utf16, format, width, height)?;

    let trimming = DWRITE_TRIMMING {
        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
        delimiter: 0,
        delimiterCount: 0,
    };
    layout.SetTrimming(&trimming, sign)?;

    Ok(layout)
}

unsafe fn build_control_icon(
    factory: &ID2D1Factory,
    control: MediaControl,
) -> Result<ID2D1PathGeometry> {
    let rect = control_rect(control);
    let left = rect.left;
    let top = rect.top;

    let mut polygons: Vec<Vec<D2D_POINT_2F>> = Vec::new();

    match control {
        MediaControl::Previous => {
            polygons.push(vec![
                point(left + 3.0, top + 8.5),
                point(left + 5.0, top + 8.5),
                point(left + 5.0, top + 21.5),
                point(left + 3.0, top + 21.5),
            ]);
            polygons.push(vec![
                point(left + 21.0, top + 8.5),
                point(left + 21.0, top + 21.5),
                point(left + 8.0, top + 15.0),
            ]);
        }
        MediaControl::Next => {
            polygons.push(vec![
                point(left + 9.0, top + 8.5),
                point(left + 9.0, top + 21.5),
                point(left + 22.0, top + 15.0),
            ]);
            polygons.push(vec![
                point(left + 25.0, top + 8.5),
                point(left + 27.0, top + 8.5),
                point(left + 27.0, top + 21.5),
                point(left + 25.0, top + 21.5),
            ]);
        }
        MediaControl::PlayPause => {
            polygons.push(vec![
                point(left + 11.0, top + 8.0),
                point(left + 11.0, top + 22.0),
                point(left + 22.0, top + 15.0),
            ]);
        }
    }

    let geometry = factory.CreatePathGeometry()?;
    let sink = geometry.Open()?;

    for polygon in &polygons {
        sink.BeginFigure(polygon[0], D2D1_FIGURE_BEGIN_FILLED);
        for vertex in &polygon[1..] {
            sink.AddLine(*vertex);
        }
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    }

    sink.Close()?;
    Ok(geometry)
}

fn point(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

unsafe fn current_clock_string() -> String {
    let st = GetLocalTime();
    format!("{:02}:{:02}", st.wHour, st.wMinute)
}

fn format_short_date(st: &windows::Win32::Foundation::SYSTEMTIME) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let day = DAYS.get(st.wDayOfWeek as usize).copied().unwrap_or("");
    let month = st
        .wMonth
        .checked_sub(1)
        .and_then(|i| MONTHS.get(i as usize))
        .copied()
        .unwrap_or("");
    format!("{day}, {} {month}", st.wDay)
}
