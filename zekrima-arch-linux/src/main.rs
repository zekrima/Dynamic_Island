//! Zekrima — Native Dynamic Island overlay for Arch Linux / CachyOS.
//!
//! GTK4 + `gtk4-layer-shell` + Cairo/PangoCairo with full v0.2.0 productivity
//! suite: Media + PipeWire FFT visualizer, Pomodoro Timer, Clipboard History,
//! on-demand System Monitor, Pillar-style D-Bus Notifications, and HUD.

mod layout;
mod render;
mod services;

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, Timelike};
use gtk4::cairo;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib::{self, ControlFlow};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, CssProvider, DrawingArea, PopoverMenu};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::layout::{CANVAS_HEIGHT_LOGICAL, CANVAS_WIDTH_LOGICAL, PILL_TOP_LOGICAL};
use crate::render::cairo_ui::{CairoRenderer, Frame};
use crate::render::spring::{PillMode, PillSprings};
use crate::render::widgets::{self, ExpandedTab, HitContext, HitTarget};
use crate::services::audio_viz::AudioVisualizer;
use crate::services::media::{MediaInfo, MediaService};
use crate::services::notifications::{NotificationService, NotificationSnapshot};
use crate::services::pomodoro::PomodoroTimer;
use crate::services::sysmon::SystemMonitor;
use crate::services::system::{SystemService, SystemSnapshot};

const FRAME_MS: u64 = 16;
const FRAME_SECONDS: f32 = 0.016;
const MAX_CLIP_ITEMS: usize = 6;
const MAX_CLIP_CHARS: usize = 2000;

struct AppState {
    window: ApplicationWindow,
    area: DrawingArea,
    renderer: CairoRenderer,
    springs: PillSprings,
    media: MediaService,
    media_info: MediaInfo,
    audio_viz: AudioVisualizer,
    system: SystemService,
    system_snapshot: SystemSnapshot,
    notifications: NotificationService,
    notification_snapshot: NotificationSnapshot,
    timer: PomodoroTimer,
    clip_items: Vec<String>,
    clip_copied_flash_until: Option<Instant>,
    ignore_next_clip: bool,
    sysmon: SystemMonitor,
    last_clock_minute: (u32, u32),
    last_input_rect: Option<(i32, i32, i32, i32)>,
    expanded: bool,
    expanded_tab: ExpandedTab,
    hovered: bool,
    hovered_target: Option<HitTarget>,
    anim_running: bool,
    hud_timer: Option<glib::SourceId>,
    notif_timer: Option<glib::SourceId>,
}

impl AppState {
    fn hit_context(&self) -> HitContext<'_> {
        let (_, pill_height, _) = self.springs.geometry();
        HitContext {
            pill_height,
            expanded: self.expanded,
            expanded_tab: self.expanded_tab,
            has_media: self.media_info.has_session,
            hud_visible: self.hud_visible(),
            toast_visible: self.toast_visible(),
            notifications: &self.notification_snapshot,
            clip_count: self.clip_items.len(),
        }
    }

    fn hud_visible(&self) -> bool {
        self.system_snapshot.hud.is_visible(Instant::now())
    }

    fn toast_visible(&self) -> bool {
        self.notification_snapshot.is_toast_visible(Instant::now())
    }

    fn clip_flash_active(&self) -> bool {
        self.clip_copied_flash_until
            .map(|t| Instant::now() < t)
            .unwrap_or(false)
    }

    fn equalizer_active(&self) -> bool {
        self.media_info.has_session
            && self.media_info.is_playing
            && !self.hud_visible()
            && !self.toast_visible()
            && (!self.expanded || self.expanded_tab == ExpandedTab::Media)
    }

    fn toast_progress_active(&self) -> bool {
        self.toast_visible() && !self.expanded && !self.hud_visible()
    }

    fn pill_origin(&self) -> (f32, f32, f32, f32) {
        let (w, h, _) = self.springs.geometry();
        let left = (CANVAS_WIDTH_LOGICAL - w) / 2.0;
        (left, PILL_TOP_LOGICAL, w, h)
    }

    fn update_input_region(&mut self) {
        let (left, top, w, h) = self.pill_origin();
        let rect_tuple = (
            left.floor() as i32,
            top.floor() as i32,
            w.ceil() as i32,
            h.ceil() as i32,
        );
        if self.last_input_rect == Some(rect_tuple) {
            return;
        }
        if let Some(surface) = self.window.surface() {
            let rect = cairo::RectangleInt::new(
                rect_tuple.0,
                rect_tuple.1,
                rect_tuple.2.max(1),
                rect_tuple.3.max(1),
            );
            let region = cairo::Region::create_rectangle(&rect);
            surface.set_input_region(&region);
            self.last_input_rect = Some(rect_tuple);
        }
    }

    fn apply_pill_mode(&mut self) {
        let mode = if self.hud_visible() {
            PillMode::Hud
        } else if self.expanded {
            PillMode::Expanded
        } else if self.toast_visible() {
            PillMode::Notification
        } else if self.notification_snapshot.unread_count() > 0 {
            PillMode::CollapsedWithBadge
        } else {
            PillMode::Collapsed
        };
        self.springs.set_mode(mode);
    }

    fn set_expanded(&mut self, expanded: bool) {
        if self.expanded == expanded {
            return;
        }
        self.expanded = expanded;
        if expanded {
            if self.expanded_tab == ExpandedTab::System {
                self.sysmon.sample();
            }
        } else {
            self.hovered_target = None;
        }
        self.apply_pill_mode();
    }

    fn set_expanded_tab(&mut self, tab: ExpandedTab) {
        if self.expanded_tab == tab {
            return;
        }
        self.expanded_tab = tab;
        self.hovered_target = None;
        if tab == ExpandedTab::System {
            self.sysmon.sample();
        }
        self.apply_pill_mode();
        self.area.queue_draw();
    }

    fn push_clip_text(&mut self, raw: &str) -> bool {
        let normalized: String = raw.trim().chars().take(MAX_CLIP_CHARS).collect();
        if normalized.is_empty() {
            return false;
        }
        if self.clip_items.first().map(|s| s.as_str()) == Some(normalized.as_str()) {
            return false;
        }
        self.clip_items.retain(|existing| existing != &normalized);
        self.clip_items.insert(0, normalized);
        self.clip_items.truncate(MAX_CLIP_ITEMS);
        true
    }
}

fn ensure_anim_timer(state_rc: &Rc<RefCell<AppState>>) {
    let should_start = {
        let mut st = state_rc.borrow_mut();
        let needs_anim = !st.springs.is_settled()
            || st.equalizer_active()
            || st.toast_progress_active();
        if needs_anim && !st.anim_running {
            st.anim_running = true;
            true
        } else {
            false
        }
    };

    if !should_start {
        return;
    }

    let weak = Rc::downgrade(state_rc);
    glib::timeout_add_local(Duration::from_millis(FRAME_MS), move || {
        let Some(rc) = weak.upgrade() else {
            return ControlFlow::Break;
        };
        let mut st = rc.borrow_mut();
        let moving = st.springs.update(FRAME_SECONDS);
        let eq = st.equalizer_active();
        let toast = st.toast_progress_active();

        st.update_input_region();
        st.area.queue_draw();

        if !moving && !eq && !toast {
            st.anim_running = false;
            ControlFlow::Break
        } else {
            ControlFlow::Continue
        }
    });
}

fn schedule_hud_hide(state_rc: &Rc<RefCell<AppState>>) {
    let wait_ms = {
        let mut st = state_rc.borrow_mut();
        if let Some(id) = st.hud_timer.take() {
            id.remove();
        }
        let now = Instant::now();
        st.system_snapshot
            .hud
            .visible_until
            .and_then(|u| u.checked_duration_since(now))
            .map(|d| d.as_millis() as u64 + 35)
    };

    if let Some(ms) = wait_ms {
        let weak = Rc::downgrade(state_rc);
        let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            let Some(rc) = weak.upgrade() else { return };
            let still_visible = {
                let mut st = rc.borrow_mut();
                st.hud_timer = None;
                st.system_snapshot = st.system.snapshot();
                if st.hud_visible() {
                    true
                } else {
                    st.apply_pill_mode();
                    st.area.queue_draw();
                    false
                }
            };
            if still_visible {
                schedule_hud_hide(&rc);
            } else {
                ensure_anim_timer(&rc);
            }
        });
        state_rc.borrow_mut().hud_timer = Some(id);
    }
}

fn schedule_notif_hide(state_rc: &Rc<RefCell<AppState>>) {
    let wait_ms = {
        let mut st = state_rc.borrow_mut();
        if let Some(id) = st.notif_timer.take() {
            id.remove();
        }
        let now = Instant::now();
        st.notification_snapshot
            .toast_until
            .and_then(|u| u.checked_duration_since(now))
            .map(|d| d.as_millis() as u64 + 35)
    };

    if let Some(ms) = wait_ms {
        let weak = Rc::downgrade(state_rc);
        let id = glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            let Some(rc) = weak.upgrade() else { return };
            let still_visible = {
                let mut st = rc.borrow_mut();
                st.notif_timer = None;
                st.notification_snapshot = st.notifications.snapshot();
                if st.toast_visible() {
                    true
                } else {
                    st.apply_pill_mode();
                    st.area.queue_draw();
                    false
                }
            };
            if still_visible {
                schedule_notif_hide(&rc);
            } else {
                ensure_anim_timer(&rc);
            }
        });
        state_rc.borrow_mut().notif_timer = Some(id);
    }
}

fn autostart_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/autostart/zekrima.desktop"))
}

fn is_autostart_enabled() -> bool {
    autostart_path().map(|p| p.exists()).unwrap_or(false)
}

fn toggle_autostart() {
    let Some(path) = autostart_path() else { return };
    if path.exists() {
        let _ = fs::remove_file(path);
    } else {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "zekrima".to_string());
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Zekrima\nComment=Native Dynamic Island Overlay\nExec={exe}\nTerminal=false\nCategories=Utility;\nX-GNOME-Autostart-enabled=true\n"
        );
        let _ = fs::write(path, entry);
    }
}

fn build_ui(app: &Application) {
    let provider = CssProvider::new();
    provider.load_from_data(
        "window.zekrima-window, window.zekrima-window > * { background-color: transparent; background-image: none; box-shadow: none; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Zekrima")
        .default_width(CANVAS_WIDTH_LOGICAL as i32)
        .default_height(CANVAS_HEIGHT_LOGICAL as i32)
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("zekrima-window");

    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("zekrima"));
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(KeyboardMode::None);
    }

    let area = DrawingArea::new();
    area.set_content_width(CANVAS_WIDTH_LOGICAL as i32);
    area.set_content_height(CANVAS_HEIGHT_LOGICAL as i32);
    window.set_child(Some(&area));

    let media_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {
        glib::MainContext::default().invoke(|| {
            APP_HANDLE.with(|cell| {
                if let Some(rc) = cell.borrow().as_ref().and_then(|w| w.upgrade()) {
                    {
                        let mut st = rc.borrow_mut();
                        st.media_info = st.media.snapshot();
                        let active = st.media_info.has_session && st.media_info.is_playing;
                        st.audio_viz.set_active(active);
                        st.apply_pill_mode();
                        st.area.queue_draw();
                    }
                    ensure_anim_timer(&rc);
                }
            });
        });
    });

    let system_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {
        glib::MainContext::default().invoke(|| {
            APP_HANDLE.with(|cell| {
                if let Some(rc) = cell.borrow().as_ref().and_then(|w| w.upgrade()) {
                    {
                        let mut st = rc.borrow_mut();
                        st.system_snapshot = st.system.snapshot();
                        st.apply_pill_mode();
                        st.area.queue_draw();
                    }
                    schedule_hud_hide(&rc);
                    ensure_anim_timer(&rc);
                }
            });
        });
    });

    let notif_wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {
        glib::MainContext::default().invoke(|| {
            APP_HANDLE.with(|cell| {
                if let Some(rc) = cell.borrow().as_ref().and_then(|w| w.upgrade()) {
                    {
                        let mut st = rc.borrow_mut();
                        st.notification_snapshot = st.notifications.snapshot();
                        st.apply_pill_mode();
                        st.area.queue_draw();
                    }
                    schedule_notif_hide(&rc);
                    ensure_anim_timer(&rc);
                }
            });
        });
    });

    let media = MediaService::start(media_wake);
    let media_info = media.snapshot();
    let audio_viz = AudioVisualizer::start();
    audio_viz.set_active(media_info.has_session && media_info.is_playing);
    let system = SystemService::start(system_wake);
    let system_snapshot = system.snapshot();
    let notifications = NotificationService::start(notif_wake);
    let notification_snapshot = notifications.snapshot();
    let mut sysmon = SystemMonitor::new();
    sysmon.sample();

    let now_local = Local::now();
    let state = Rc::new(RefCell::new(AppState {
        window: window.clone(),
        area: area.clone(),
        renderer: CairoRenderer::new(),
        springs: PillSprings::new(),
        media,
        media_info,
        audio_viz,
        system,
        system_snapshot,
        notifications,
        notification_snapshot,
        timer: PomodoroTimer::default(),
        clip_items: Vec::with_capacity(MAX_CLIP_ITEMS),
        clip_copied_flash_until: None,
        ignore_next_clip: false,
        sysmon,
        last_clock_minute: (now_local.hour(), now_local.minute()),
        last_input_rect: None,
        expanded: false,
        expanded_tab: ExpandedTab::Media,
        hovered: false,
        hovered_target: None,
        anim_running: false,
        hud_timer: None,
        notif_timer: None,
    }));

    APP_HANDLE.with(|cell| {
        *cell.borrow_mut() = Some(Rc::downgrade(&state));
    });

    // Event-driven GTK4 Clipboard watcher (zero polling)
    if let Some(display) = gdk::Display::default() {
        let clipboard = display.clipboard();
        let weak = Rc::downgrade(&state);
        clipboard.connect_changed(move |cb| {
            let weak_inner = weak.clone();
            cb.read_text_async(None::<&gio::Cancellable>, move |res| {
                let Ok(Some(text)) = res else { return };
                let Some(rc) = weak_inner.upgrade() else {
                    return;
                };
                let mut st = rc.borrow_mut();
                if st.ignore_next_clip {
                    st.ignore_next_clip = false;
                    return;
                }
                if st.push_clip_text(&text)
                    && st.expanded
                    && st.expanded_tab == ExpandedTab::Clipboard
                    && !st.anim_running
                {
                    st.area.queue_draw();
                }
            });
        });
    }

    // Draw function
    {
        let st_rc = Rc::clone(&state);
        area.set_draw_func(move |_area, cr, _w, _h| {
            let mut st = st_rc.borrow_mut();
            let (width, height, radius) = st.springs.geometry();
            let expanded = st.expanded;
            let expanded_tab = st.expanded_tab;
            let hovered_target = st.hovered_target;
            let eq_levels = st.audio_viz.levels();
            let media = st.media_info.clone();
            let system = st.system_snapshot.clone();
            let notification = st.notification_snapshot.clone();
            let timer = st.timer.clone();
            let clip_items = st.clip_items.clone();
            let clip_copied_flash = st.clip_flash_active();
            let sys_stats = st.sysmon.stats.clone();

            let frame = Frame {
                width,
                height,
                radius,
                expanded,
                expanded_tab,
                media: &media,
                hovered_target,
                eq_levels,
                system: &system,
                notification: &notification,
                timer: &timer,
                clip_items: &clip_items,
                clip_copied_flash,
                sys_stats: &sys_stats,
            };
            st.renderer.draw(cr, &frame);
        });
    }

    {
        let st_rc = Rc::clone(&state);
        window.connect_realize(move |_| {
            st_rc.borrow_mut().update_input_region();
        });
    }

    // Hover & motion tracking
    let motion = gtk4::EventControllerMotion::new();
    {
        let st_rc = Rc::clone(&state);
        motion.connect_enter(move |_, x, y| {
            {
                let mut st = st_rc.borrow_mut();
                if !st.hovered {
                    st.hovered = true;
                    if !st.toast_visible() {
                        st.set_expanded(true);
                    }
                }
                update_hover_target(&mut st, x as f32, y as f32);
            }
            ensure_anim_timer(&st_rc);
        });
    }
    {
        let st_rc = Rc::clone(&state);
        motion.connect_motion(move |_, x, y| {
            let mut st = st_rc.borrow_mut();
            if update_hover_target(&mut st, x as f32, y as f32) && !st.anim_running {
                st.area.queue_draw();
            }
        });
    }
    {
        let st_rc = Rc::clone(&state);
        motion.connect_leave(move |_| {
            {
                let mut st = st_rc.borrow_mut();
                st.hovered = false;
                st.hovered_target = None;
                st.set_expanded(false);
            }
            ensure_anim_timer(&st_rc);
        });
    }
    area.add_controller(motion);

    // Left-click handler
    let left_click = gtk4::GestureClick::builder().button(1).build();
    {
        let st_rc = Rc::clone(&state);
        left_click.connect_released(move |_, _, x, y| {
            {
                let mut st = st_rc.borrow_mut();
                let (left, top, _, _) = st.pill_origin();
                let lx = x as f32 - left;
                let ly = y as f32 - top;
                let target = {
                    let ctx = st.hit_context();
                    widgets::hit_target_at(lx, ly, &ctx)
                };

                if let Some(target) = target {
                    match target {
                        HitTarget::SelectTab(tab) => st.set_expanded_tab(tab),
                        HitTarget::MediaControl(c) => st.media.control(c),
                        HitTarget::ToastDismiss(id) => st.notifications.dismiss(id),
                        HitTarget::ToastClear(_) => st.notifications.clear_toast(),
                        HitTarget::ClearAllNotifications => st.notifications.clear_all(),
                        HitTarget::NotifDismiss(id) => st.notifications.dismiss(id),
                        HitTarget::TimerToggle => st.timer.toggle_start_pause(),
                        HitTarget::TimerReset => st.timer.reset(),
                        HitTarget::TimerPreset(secs) => st.timer.select_preset(secs),
                        HitTarget::ClipCopy(idx) => {
                            if let Some(text) = st.clip_items.get(idx).cloned() {
                                if let Some(display) = gdk::Display::default() {
                                    st.ignore_next_clip = true;
                                    display.clipboard().set_text(&text);
                                    st.clip_items.remove(idx);
                                    st.clip_items.insert(0, text);
                                    st.clip_copied_flash_until =
                                        Some(Instant::now() + Duration::from_millis(1400));
                                }
                            }
                        }
                        HitTarget::ClipClear => {
                            st.clip_items.clear();
                            st.clip_copied_flash_until = None;
                        }
                    }
                } else if st.toast_visible() && !st.expanded {
                    st.notifications.clear_toast();
                } else {
                    let next = !st.expanded;
                    st.set_expanded(next);
                }
                st.area.queue_draw();
            }
            ensure_anim_timer(&st_rc);
        });
    }
    area.add_controller(left_click);

    // Right-click menu
    let action_group = gio::SimpleActionGroup::new();
    {
        let act = gio::SimpleAction::new("toggle-autostart", None);
        act.connect_activate(|_, _| toggle_autostart());
        action_group.add_action(&act);
    }
    {
        let st_rc = Rc::clone(&state);
        let act = gio::SimpleAction::new("test-notif", None);
        act.connect_activate(move |_, _| {
            st_rc.borrow().notifications.push_manual(
                "Zekrima",
                "Dynamic Island Active",
                "Notifications on CachyOS are working!",
            );
        });
        action_group.add_action(&act);
    }
    {
        let app_clone = app.clone();
        let act = gio::SimpleAction::new("quit", None);
        act.connect_activate(move |_, _| app_clone.quit());
        action_group.add_action(&act);
    }
    window.insert_action_group("island", Some(&action_group));

    let right_click = gtk4::GestureClick::builder().button(3).build();
    {
        let area_clone = area.clone();
        right_click.connect_released(move |_, _, x, y| {
            let menu = gio::Menu::new();
            let autostart_label = if is_autostart_enabled() {
                "✓ Run at Login"
            } else {
                "Run at Login"
            };
            menu.append(Some(autostart_label), Some("island.toggle-autostart"));
            menu.append(Some("Test Notification"), Some("island.test-notif"));
            menu.append(Some("Quit Zekrima"), Some("island.quit"));

            let popover = PopoverMenu::from_model(Some(&menu));
            popover.set_parent(&area_clone);
            popover.set_has_arrow(false);
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.connect_closed(|p| p.unparent());
            popover.popup();
        });
    }
    area.add_controller(right_click);

    // 1-second housekeeping: minute clock rollover, Pomodoro tick, on-demand SysMon
    {
        let weak = Rc::downgrade(&state);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(rc) = weak.upgrade() else {
                return ControlFlow::Break;
            };
            let mut st = rc.borrow_mut();
            let mut needs_draw = false;

            let now = Local::now();
            let cur = (now.hour(), now.minute());
            if cur != st.last_clock_minute {
                st.last_clock_minute = cur;
                needs_draw = true;
            }

            if st.timer.running {
                let finished = st.timer.tick();
                needs_draw = true;
                if finished {
                    st.apply_pill_mode();
                    st.notifications.push_manual(
                        "Pomodoro",
                        "Focus Session Complete!",
                        "Time's up — take a short break.",
                    );
                }
            }

            if st.expanded && st.expanded_tab == ExpandedTab::System {
                st.sysmon.sample();
                needs_draw = true;
            }

            if st.expanded
                && st.expanded_tab == ExpandedTab::Clipboard
                && st.clip_copied_flash_until.is_some()
                && !st.clip_flash_active()
            {
                st.clip_copied_flash_until = None;
                needs_draw = true;
            }

            if needs_draw && !st.anim_running {
                st.area.queue_draw();
            }
            ControlFlow::Continue
        });
    }

    ensure_anim_timer(&state);
    window.present();
}

fn update_hover_target(st: &mut AppState, canvas_x: f32, canvas_y: f32) -> bool {
    let (left, top, _, _) = st.pill_origin();
    let lx = canvas_x - left;
    let ly = canvas_y - top;
    let next = {
        let ctx = st.hit_context();
        widgets::hit_target_at(lx, ly, &ctx)
    };
    if next != st.hovered_target {
        st.hovered_target = next;
        true
    } else {
        false
    }
}

thread_local! {
    static APP_HANDLE: RefCell<Option<std::rc::Weak<RefCell<AppState>>>> = const { RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id("io.zekrima.DynamicIsland")
        .build();
    app.connect_activate(build_ui);
    app.run()
}
