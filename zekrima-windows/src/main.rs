#![windows_subsystem = "windows"]

mod layout;
mod render;
mod services;
mod tray;
mod window;

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::time::Instant;

use windows::core::{w, Error, Result, GUID, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, UnregisterPowerSettingNotification, HPOWERNOTIFY,
    POWERBROADCAST_SETTING,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_VOLUME_DOWN, VK_VOLUME_MUTE,
    VK_VOLUME_UP,
};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RID_INPUT, RIM_TYPEHID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreatePopupMenu, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, MessageBoxW, PostMessageW,
    PostQuitMessage, SetForegroundWindow, SetTimer, SetWindowsHookExW, TrackPopupMenu,
    TranslateMessage, UnhookWindowsHookEx, DEVICE_NOTIFY_WINDOW_HANDLE, HC_ACTION, HHOOK,
    KBDLLHOOKSTRUCT, MA_NOACTIVATE, MB_ICONERROR, MB_OK, MF_STRING, MSG, PBT_POWERSETTINGCHANGE,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WH_KEYBOARD_LL, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_INPUT,
    WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCHITTEST, WM_NULL,
    WM_POWERBROADCAST, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_TIMER,
};

use crate::render::d2d::{Frame, Renderer};
use crate::render::spring::{PillMode, PillSprings};
use crate::render::widgets::{ExpandedTab, HitContext, HitTarget};
use crate::services::audio_viz::AudioVisualizer;
use crate::services::media::{MediaInfo, MediaService, WM_MEDIA_UPDATED};
use crate::services::notifications::{
    NotificationService, NotificationSnapshot, WM_NOTIFICATION_UPDATED,
};
use crate::services::system::{SystemService, SystemSnapshot, WM_SYSTEM_UPDATED};
use crate::tray::{
    Tray, IDM_EXIT, IDM_NOTIF_SETTINGS, IDM_RESET, IDM_STARTUP, WM_TRAY_COMMAND,
};
use crate::window::fullscreen::{FullscreenMonitor, TIMER_FULLSCREEN_CHECK};
use crate::window::hit_test;
use crate::window::host::HostWindow;

const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_ANIMATION: usize = 1;
const ANIMATION_INTERVAL_MS: u32 = 16;
const TIMER_HUD_HIDE: usize = 3;
const TIMER_NOTIF_HIDE: usize = 4;
const HIDE_TIMER_MARGIN_MS: u32 = 30;
const FRAME_SECONDS: f32 = 0.016;

// GUID_DEVICE_POWER_POLICY_VIDEO_BRIGHTNESS {aded5e82-b909-4619-9949-f5d71dac0bcb}
const GUID_MONITOR_BRIGHTNESS: GUID = GUID::from_u128(0xaded5e82_b909_4619_9949_f5d71dac0bcb);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

struct PowerNotify(HPOWERNOTIFY);

impl Drop for PowerNotify {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterPowerSettingNotification(self.0);
        }
    }
}

struct App {
    host: HostWindow,
    renderer: Renderer,
    springs: PillSprings,
    media: MediaService,
    media_info: MediaInfo,
    audio_viz: AudioVisualizer,
    system: SystemService,
    system_snapshot: SystemSnapshot,
    notifications: NotificationService,
    notification_snapshot: NotificationSnapshot,
    last_clock_minute: (u16, u16),
    expanded: bool,
    expanded_tab: ExpandedTab,
    hovered: bool,
    hovered_target: Option<HitTarget>,
    tracking_leave: bool,
    timer_running: bool,
    tray: Tray,
    fullscreen_monitor: FullscreenMonitor,
    _power_notify: Option<PowerNotify>,
}

impl App {
    unsafe fn new() -> Result<Self> {
        let host = HostWindow::create(Some(wndproc))?;
        let media = MediaService::start(host.hwnd());
        let media_info = media.snapshot();
        let audio_viz = AudioVisualizer::start();
        audio_viz.set_active(media_info.has_session && media_info.is_playing);
        let system = SystemService::start(host.hwnd());
        let system_snapshot = system.snapshot();
        let notifications = NotificationService::start(host.hwnd());
        let notification_snapshot = notifications.snapshot();
        let renderer = Renderer::new(host.width_px(), host.height_px(), host.dpi() as f32)?;

        let tray = Tray::new(host.hwnd())?;
        let fullscreen_monitor = FullscreenMonitor::new(host.hwnd())?;
        let st = GetLocalTime();

        let power_notify = RegisterPowerSettingNotification(
            host.hwnd(),
            &GUID_MONITOR_BRIGHTNESS,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
        .ok()
        .map(PowerNotify);

        let mut app = Self {
            host,
            renderer,
            springs: PillSprings::new(),
            media,
            media_info,
            audio_viz,
            system,
            system_snapshot,
            notifications,
            notification_snapshot,
            last_clock_minute: (st.wHour, st.wMinute),
            expanded: false,
            expanded_tab: ExpandedTab::Media,
            hovered: false,
            hovered_target: None,
            tracking_leave: false,
            timer_running: false,
            tray,
            fullscreen_monitor,
            _power_notify: power_notify,
        };

        app.render()?;
        app.host.show();
        app.tray.set_visible(true)?;
        app.update_timer();
        register_raw_input(app.host.hwnd());
        Ok(app)
    }

    unsafe fn render(&mut self) -> Result<()> {
        let (width, height, radius) = self.springs.geometry();
        let frame = Frame {
            width,
            height,
            radius,
            expanded: self.expanded,
            expanded_tab: self.expanded_tab,
            media: &self.media_info,
            hovered_target: self.hovered_target,
            eq_levels: self.audio_viz.levels(),
            system: &self.system_snapshot,
            notification: &self.notification_snapshot,
        };
        self.renderer.render(self.host.hwnd(), &frame)
    }

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
        }
    }

    /// Real-time audio visualizer bars animate while music is actively playing
    /// (both in the collapsed pill and the expanded media card).
    fn equalizer_active(&self) -> bool {
        self.media_info.has_session
            && self.media_info.is_playing
            && !self.hud_visible()
            && !self.toast_visible()
            && (!self.expanded || self.expanded_tab == ExpandedTab::Media)
    }

    /// The 60 fps toast countdown progress bar animates only during the ~3.8 s
    /// incoming toast, then stops completely when absorbed into the badge.
    fn toast_progress_active(&self) -> bool {
        self.toast_visible() && !self.expanded && !self.hud_visible()
    }

    unsafe fn set_expanded(&mut self, expanded: bool) {
        if self.expanded == expanded {
            return;
        }

        self.expanded = expanded;
        if expanded {
            // Smart default tab: if notifications exist and no media is active,
            // open straight to the Notifications list like Pillar.
            self.expanded_tab =
                if !self.media_info.has_session && self.notification_snapshot.unread_count() > 0 {
                    ExpandedTab::Notifications
                } else {
                    ExpandedTab::Media
                };
        } else {
            self.hovered_target = None;
        }
        self.apply_pill_mode();
        self.update_timer();
    }

    unsafe fn set_expanded_tab(&mut self, tab: ExpandedTab) {
        if self.expanded_tab == tab {
            return;
        }
        self.expanded_tab = tab;
        self.hovered_target = None;
        self.apply_pill_mode();
        self.update_timer();
        let _ = self.render();
    }

    unsafe fn update_timer(&mut self) {
        if self.springs.is_settled()
            && !self.equalizer_active()
            && !self.toast_progress_active()
        {
            self.stop_timer();
        } else {
            self.start_timer();
        }
    }

    unsafe fn start_timer(&mut self) {
        if !self.timer_running {
            let timer = SetTimer(
                self.host.hwnd(),
                TIMER_ANIMATION,
                ANIMATION_INTERVAL_MS,
                None,
            );
            self.timer_running = timer != 0;
        }
    }

    unsafe fn stop_timer(&mut self) {
        if self.timer_running {
            let _ = KillTimer(self.host.hwnd(), TIMER_ANIMATION);
            self.timer_running = false;
        }
    }

    unsafe fn stop_all_timers(&mut self) {
        self.stop_timer();
        let _ = KillTimer(self.host.hwnd(), TIMER_HUD_HIDE);
        let _ = KillTimer(self.host.hwnd(), TIMER_NOTIF_HIDE);
    }

    unsafe fn on_timer(&mut self) -> Result<()> {
        let moving = self.springs.update(FRAME_SECONDS);
        let eq_anim = self.equalizer_active();
        let toast_anim = self.toast_progress_active();

        self.render()?;

        if !moving && !eq_anim && !toast_anim {
            self.stop_timer();
        }

        Ok(())
    }

    unsafe fn on_media_updated(&mut self) {
        self.media_info = self.media.snapshot();
        self.audio_viz
            .set_active(self.media_info.has_session && self.media_info.is_playing);
        self.apply_pill_mode();
        self.update_timer();
        let _ = self.render();
    }

    /// Refreshes the displayed clock whenever the local minute rolls over.
    unsafe fn on_clock_tick(&mut self) {
        let st = GetLocalTime();
        let current = (st.wHour, st.wMinute);
        if current != self.last_clock_minute {
            self.last_clock_minute = current;
            if !self.timer_running {
                let _ = self.render();
            }
        }
    }

    unsafe fn on_system_updated(&mut self) {
        self.system_snapshot = self.system.snapshot();
        self.schedule_hud_hide();
        self.apply_pill_mode();
        self.update_timer();
        let _ = self.render();
    }

    unsafe fn on_notification_updated(&mut self) {
        self.notification_snapshot = self.notifications.snapshot();
        self.schedule_notif_hide();
        self.apply_pill_mode();
        self.update_timer();
        let _ = self.render();
    }

    unsafe fn schedule_hud_hide(&mut self) {
        let _ = KillTimer(self.host.hwnd(), TIMER_HUD_HIDE);
        let now = Instant::now();
        if let Some(until) = self.system_snapshot.hud.visible_until {
            if until > now {
                let wait_ms = (until - now).as_millis() as u32 + HIDE_TIMER_MARGIN_MS;
                let _ = SetTimer(self.host.hwnd(), TIMER_HUD_HIDE, wait_ms, None);
            }
        }
    }

    unsafe fn schedule_notif_hide(&mut self) {
        let _ = KillTimer(self.host.hwnd(), TIMER_NOTIF_HIDE);
        let now = Instant::now();
        if let Some(until) = self.notification_snapshot.toast_until {
            if until > now {
                let wait_ms = (until - now).as_millis() as u32 + HIDE_TIMER_MARGIN_MS;
                let _ = SetTimer(self.host.hwnd(), TIMER_NOTIF_HIDE, wait_ms, None);
            }
        }
    }

    unsafe fn on_hud_timer(&mut self) {
        let _ = KillTimer(self.host.hwnd(), TIMER_HUD_HIDE);
        self.system_snapshot = self.system.snapshot();

        if self.hud_visible() {
            self.schedule_hud_hide();
            return;
        }

        self.apply_pill_mode();
        self.update_timer();
        let _ = self.render();
    }

    unsafe fn on_notif_timer(&mut self) {
        let _ = KillTimer(self.host.hwnd(), TIMER_NOTIF_HIDE);
        self.notification_snapshot = self.notifications.snapshot();

        if self.toast_visible() {
            self.schedule_notif_hide();
            return;
        }

        // Toast finished its ~3.8 s display -> smoothly absorb into collapsed badge!
        self.apply_pill_mode();
        self.update_timer();
        let _ = self.render();
    }

    fn hud_visible(&self) -> bool {
        self.system_snapshot.hud.is_visible(Instant::now())
    }

    fn toast_visible(&self) -> bool {
        self.notification_snapshot.is_toast_visible(Instant::now())
    }

    unsafe fn apply_pill_mode(&mut self) {
        let mode = if self.hud_visible() {
            PillMode::Hud
        } else if self.expanded {
            match self.expanded_tab {
                ExpandedTab::Media => PillMode::Expanded,
                ExpandedTab::Notifications => PillMode::ExpandedNotifications,
            }
        } else if self.toast_visible() {
            PillMode::Notification
        } else if self.notification_snapshot.unread_count() > 0 {
            PillMode::CollapsedWithBadge
        } else {
            PillMode::Collapsed
        };
        self.springs.set_mode(mode);
    }

    unsafe fn on_mouse_move(&mut self) {
        if !self.tracking_leave {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.host.hwnd(),
                dwHoverTime: 0,
            };
            self.tracking_leave = TrackMouseEvent(&mut track).is_ok();
        }

        if !self.hovered {
            self.hovered = true;
            // Don't interrupt an active incoming toast with auto-expand so the
            // user can directly click the toast or its `×` dismiss button.
            if !self.toast_visible() {
                self.set_expanded(true);
            }
        }

        let ctx = self.hit_context();
        let target = hit_test::target_at_cursor(self.host.hwnd(), self.host.dpi(), &self.springs, &ctx);
        if target != self.hovered_target {
            self.hovered_target = target;
            if !self.timer_running {
                let _ = self.render();
            }
        }
    }

    unsafe fn on_mouse_leave(&mut self) {
        self.tracking_leave = false;
        self.hovered = false;
        self.hovered_target = None;
        self.set_expanded(false);
    }

    unsafe fn on_click(&mut self, lparam: LPARAM) {
        let ctx = self.hit_context();
        if let Some(target) = hit_test::target_at(lparam, self.host.dpi(), &self.springs, &ctx) {
            match target {
                HitTarget::MediaControl(control) => {
                    self.media.control(control);
                }
                HitTarget::ToastActivate(id) => {
                    self.notifications.activate(id);
                }
                HitTarget::ToastDismiss(id) => {
                    self.notifications.dismiss(id);
                }
                HitTarget::TabMedia => {
                    self.set_expanded_tab(ExpandedTab::Media);
                }
                HitTarget::TabNotifications => {
                    self.set_expanded_tab(ExpandedTab::Notifications);
                }
                HitTarget::ClearAllNotifications => {
                    self.notifications.clear_all();
                }
                HitTarget::OpenNotificationSettings => {
                    self.notifications.open_windows_settings();
                }
                HitTarget::NotifActivate(id) => {
                    self.notifications.activate(id);
                }
                HitTarget::NotifDismiss(id) => {
                    self.notifications.dismiss(id);
                }
            }
            return;
        }

        if self.toast_visible() && !self.expanded {
            self.notifications.clear_toast();
            return;
        }

        self.set_expanded(!self.expanded);
    }

    unsafe fn on_right_click(&self) -> Result<()> {
        let menu = CreatePopupMenu()?;
        AppendMenuW(menu, MF_STRING, IDM_NOTIF_SETTINGS, w!("Notification Settings"))?;
        AppendMenuW(menu, MF_STRING, IDM_RESET, w!("Reset Position"))?;
        AppendMenuW(menu, MF_STRING, IDM_EXIT, w!("Exit"))?;

        let _ = SetForegroundWindow(self.host.hwnd());

        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor)?;

        let command = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            cursor.x,
            cursor.y,
            0,
            self.host.hwnd(),
            None,
        );

        let _ = DestroyMenu(menu);
        let _ = PostMessageW(self.host.hwnd(), WM_NULL, WPARAM(0), LPARAM(0));

        match command.0 as usize {
            IDM_NOTIF_SETTINGS => {
                self.notifications.open_windows_settings();
            }
            IDM_RESET => {
                let _ = self.host.recenter();
            }
            IDM_EXIT => {
                PostMessageW(self.host.hwnd(), WM_CLOSE, WPARAM(0), LPARAM(0))?;
            }
            _ => {}
        }

        Ok(())
    }

    unsafe fn on_dpi_changed(&mut self, dpi: u32) -> Result<()> {
        self.host.apply_dpi(dpi)?;
        self.renderer.resize(
            self.host.width_px(),
            self.host.height_px(),
            self.host.dpi() as f32,
        )?;
        self.render()
    }
}

fn with_app<R>(callback: impl FnOnce(&App) -> R) -> Option<R> {
    APP.with(|slot| slot.borrow().as_ref().map(callback))
}

fn with_app_mut<R>(callback: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|slot| slot.borrow_mut().as_mut().map(callback))
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => {
            let hit = with_app(|app| hit_test::handle(hwnd, lparam, app.host.dpi(), &app.springs));
            hit.unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam))
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            if with_app_mut(|app| app.on_mouse_move()).is_some() {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_MOUSELEAVE => {
            if with_app_mut(|app| app.on_mouse_leave()).is_some() {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_LBUTTONUP => {
            if with_app_mut(|app| app.on_click(lparam)).is_some() {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_MEDIA_UPDATED => {
            if with_app_mut(|app| app.on_media_updated()).is_some() {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_SYSTEM_UPDATED => {
            if with_app_mut(|app| app.on_system_updated()).is_some() {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_NOTIFICATION_UPDATED => {
            if with_app_mut(|app| app.on_notification_updated()).is_some() {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_INPUT => {
            handle_raw_input(lparam);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_POWERBROADCAST => {
            if wparam.0 as u32 == PBT_POWERSETTINGCHANGE && lparam.0 != 0 {
                let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                if setting.PowerSetting == GUID_MONITOR_BRIGHTNESS
                    && setting.DataLength >= size_of::<u32>() as u32
                {
                    let percent = std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32);
                    with_app(|app| app.system.on_power_brightness(percent));
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        WM_RBUTTONUP => {
            if with_app(|app| {
                let _ = app.on_right_click();
            })
            .is_some()
            {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_TIMER if wparam.0 == TIMER_ANIMATION => {
            match with_app_mut(|app| app.on_timer().is_ok()) {
                Some(true) => LRESULT(0),
                Some(false) => {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    LRESULT(0)
                }
                None => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
        WM_TIMER if wparam.0 == TIMER_HUD_HIDE => {
            with_app_mut(|app| app.on_hud_timer());
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_NOTIF_HIDE => {
            with_app_mut(|app| app.on_notif_timer());
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_FULLSCREEN_CHECK => {
            let _ = with_app_mut(|app| {
                let _ = app.fullscreen_monitor.check_and_react();
                app.on_clock_tick();
            });
            LRESULT(0)
        }
        WM_TRAY_COMMAND => {
            if with_app_mut(|app| {
                let command = wparam.0;
                match command {
                    IDM_EXIT => {
                        let _ = PostMessageW(app.host.hwnd(), WM_CLOSE, WPARAM(0), LPARAM(0));
                        Some(true)
                    }
                    IDM_STARTUP => {
                        let currently_enabled = crate::tray::is_startup_enabled();
                        let _ = crate::tray::set_startup_registry(!currently_enabled);
                        Some(true)
                    }
                    IDM_NOTIF_SETTINGS => {
                        app.notifications.open_windows_settings();
                        Some(true)
                    }
                    IDM_RESET => {
                        let _ = app.host.recenter();
                        Some(true)
                    }
                    _ => None,
                }
            })
            .is_some()
            {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as u32;
            match with_app_mut(|app| app.on_dpi_changed(dpi).is_ok()) {
                Some(true) => LRESULT(0),
                Some(false) => {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    LRESULT(0)
                }
                None => DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            with_app_mut(|app| app.stop_all_timers());
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let msg = wparam.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            if event.vkCode == VK_ESCAPE.0 as u32 {
                let over_pill = with_app(|app| {
                    hit_test::cursor_over_pill(app.host.hwnd(), app.host.dpi(), &app.springs)
                })
                .unwrap_or(false);

                if over_pill {
                    with_app(|app| {
                        let _ = PostMessageW(app.host.hwnd(), WM_CLOSE, WPARAM(0), LPARAM(0));
                    });
                    return LRESULT(1);
                }
            }

            let vk = event.vkCode as u16;
            if vk == VK_VOLUME_UP.0 || vk == VK_VOLUME_DOWN.0 || vk == VK_VOLUME_MUTE.0 {
                with_app(|app| app.system.notify_volume_key());
            }
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}

struct ComApartment;

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

struct KeyboardHook(HHOOK);

impl KeyboardHook {
    unsafe fn install() -> Result<Self> {
        let module = GetModuleHandleW(None)?;
        let instance = HINSTANCE(module.0);
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), instance, 0)?;
        Ok(Self(hook))
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
    }
}

unsafe fn message_loop() -> Result<()> {
    let mut message = MSG::default();

    loop {
        let result = GetMessageW(&mut message, None, 0, 0);
        if result.0 == -1 {
            return Err(Error::from_win32());
        }
        if result.0 == 0 {
            return Ok(());
        }

        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

fn register_raw_input(hwnd: HWND) {
    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x0C,
        usUsage: 0x01,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe {
        let _ = RegisterRawInputDevices(&[rid], size_of::<RAWINPUTDEVICE>() as u32);
    }
}

unsafe fn handle_raw_input(lparam: LPARAM) {
    let mut size: u32 = 0;
    let hraw = windows::Win32::UI::Input::HRAWINPUT(lparam.0 as *mut c_void);
    if GetRawInputData(
        hraw,
        RID_INPUT,
        None,
        &mut size,
        size_of::<RAWINPUTHEADER>() as u32,
    ) != 0
        || size == 0
    {
        return;
    }

    let mut buf = vec![0u8; size as usize];
    let read = GetRawInputData(
        hraw,
        RID_INPUT,
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut size,
        size_of::<RAWINPUTHEADER>() as u32,
    );
    if read == u32::MAX || read == 0 {
        return;
    }

    let raw = &*(buf.as_ptr() as *const RAWINPUT);
    if raw.header.dwType != RIM_TYPEHID.0 {
        return;
    }

    let hid = &raw.data.hid;
    let report_len = hid.dwSizeHid as usize;
    let count = hid.dwCount as usize;
    if report_len == 0 || count == 0 {
        return;
    }

    let base = &hid.bRawData as *const u8;
    for i in 0..count {
        let report = std::slice::from_raw_parts(base.add(i * report_len), report_len);
        let mut saw_brightness = false;
        let mut saw_volume = false;
        for start in [0usize, 1] {
            if report.len() >= start + 2 {
                for w in report[start..].chunks_exact(2) {
                    let usage = u16::from_le_bytes([w[0], w[1]]);
                    match usage {
                        0x006F | 0x0070 => saw_brightness = true,
                        0x00E9 | 0x00EA | 0x00E2 => saw_volume = true,
                        _ => {}
                    }
                }
            }
        }
        if saw_brightness {
            with_app(|app| app.system.notify_brightness_key());
        } else if saw_volume {
            with_app(|app| app.system.notify_volume_key());
        }
    }
}

unsafe fn run() -> Result<()> {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
    CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    let _apartment = ComApartment;

    // Single-instance guard: prevent multiple islands from running simultaneously.
    let mutex_name = w!("Global\\ZekrimaSingleInstanceMutex");
    if let Ok(mutex) = windows::Win32::System::Threading::CreateMutexW(None, false, mutex_name) {
        if windows::Win32::Foundation::GetLastError()
            == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
        {
            return Ok(());
        }
        let _ = mutex;
    }

    let app = App::new()?;
    APP.with(|slot| {
        *slot.borrow_mut() = Some(app);
    });

    let hook = KeyboardHook::install()?;
    let result = message_loop();
    drop(hook);

    APP.with(|slot| {
        slot.borrow_mut().take();
    });

    result
}

fn show_error(message: &str) {
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("Zekrima - Error"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn main() {
    unsafe {
        if let Err(error) = run() {
            show_error(&format!("Zekrima could not start.\n\n{error}"));
        }
    }
}
