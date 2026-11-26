//! Event-driven System HUD service for volume and brightness.
//!
//! - Volume/mute changes are tracked natively via Core Audio's
//!   `IAudioEndpointVolumeCallback` (plus `IMMNotificationClient` so plugging in
//!   headphones or Bluetooth audio automatically rebinds to the new default
//!   endpoint). Zero polling, zero spawned threads per keypress.
//! - Brightness changes arrive directly from `WM_POWERBROADCAST`
//!   (`GUID_MONITOR_BRIGHTNESS`) on internal panels, with an on-demand worker
//!   fallback (WMI `WmiMonitorBrightness` -> DDC/CI `GetMonitorBrightness`) when
//!   HID brightness keys are pressed on external displays.

use std::ffi::c_void;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::{implement, BSTR, PCWSTR, Result, VARIANT};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CoUninitialize, CLSCTX_ALL,
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Wmi::{
    IWbemLocator, IWbemServices, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_INFINITE,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Posted to the island window when volume/brightness HUD or state updates.
pub const WM_SYSTEM_UPDATED: u32 = 0x8002;

/// How long the transient HUD stays visible after the last change.
pub const HUD_DURATION: Duration = Duration::from_millis(1800);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudKind {
    Volume,
    Brightness,
}

#[derive(Clone, Debug)]
pub struct HudState {
    pub kind: HudKind,
    pub value: f32, // 0.0..=1.0
    pub muted: bool,
    pub visible_until: Option<Instant>,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            kind: HudKind::Volume,
            value: 0.0,
            muted: false,
            visible_until: None,
        }
    }
}

impl HudState {
    pub fn is_visible(&self, now: Instant) -> bool {
        self.visible_until.map(|t| now < t).unwrap_or(false)
    }
}

#[derive(Clone, Default)]
pub struct SystemSnapshot {
    pub hud: HudState,
    pub volume: f32,
    pub muted: bool,
}

struct SharedState {
    snapshot: SystemSnapshot,
    brightness_seeded: bool,
    last_brightness: Option<f32>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            snapshot: SystemSnapshot::default(),
            brightness_seeded: false,
            last_brightness: None,
        }
    }
}

enum Command {
    RefreshVolume { show_hud: bool },
    RefreshBrightness,
    ReattachAudio,
    Shutdown,
}

pub struct SystemService {
    shared: Arc<Mutex<SharedState>>,
    commands: mpsc::Sender<Command>,
    worker: Option<JoinHandle<()>>,
    hwnd: HWND,
}

impl SystemService {
    pub fn start(hwnd: HWND) -> Self {
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let (commands, receiver) = mpsc::channel();

        let worker_shared = Arc::clone(&shared);
        let worker_commands = commands.clone();
        let hwnd_raw = hwnd.0 as usize;

        let worker = thread::Builder::new()
            .name("zekrima-system".to_owned())
            .spawn(move || unsafe {
                worker_main(hwnd_raw, worker_shared, worker_commands, receiver);
            })
            .ok();

        Self {
            shared,
            commands,
            worker,
            hwnd,
        }
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .snapshot
            .clone()
    }

    /// Called directly from `WM_POWERBROADCAST` (`GUID_MONITOR_BRIGHTNESS`).
    /// The first notification sent by Windows immediately after registration
    /// only seeds the baseline value without flashing the HUD on startup.
    pub fn on_power_brightness(&self, percent: u32) {
        let value = (percent.min(100) as f32) / 100.0;
        let should_notify = {
            let mut state = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            if !state.brightness_seeded {
                state.brightness_seeded = true;
                state.last_brightness = Some(value);
                false
            } else {
                let changed = state
                    .last_brightness
                    .map(|prev| (prev - value).abs() >= 0.004)
                    .unwrap_or(true);
                state.last_brightness = Some(value);
                if changed {
                    state.snapshot.hud = HudState {
                        kind: HudKind::Brightness,
                        value,
                        muted: false,
                        visible_until: Some(Instant::now() + HUD_DURATION),
                    };
                }
                changed
            }
        };

        if should_notify {
            unsafe {
                let _ = PostMessageW(self.hwnd, WM_SYSTEM_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// Called when an explicit HID brightness key (`0x006F` / `0x0070`) arrives.
    /// Offloads the WMI/DDC read to the worker thread so the UI never stalls.
    pub fn notify_brightness_key(&self) {
        let _ = self.commands.send(Command::RefreshBrightness);
    }

    /// Called on `VK_VOLUME_UP/DOWN/MUTE` key-down so the HUD still appears even
    /// if the volume is already clamped at 0% or 100% and Core Audio emits no
    /// delta callback.
    pub fn notify_volume_key(&self) {
        let _ = self.commands.send(Command::RefreshVolume { show_hud: true });
    }
}

impl Drop for SystemService {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    hwnd_raw: usize,
    shared: Arc<Mutex<SharedState>>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if pnotify.is_null() {
            return Ok(());
        }
        let (value, muted) = unsafe {
            let data = &*pnotify;
            (data.fMasterVolume.clamp(0.0, 1.0), data.bMuted.as_bool())
        };

        publish_volume(self.hwnd_raw, &self.shared, value, muted, true);
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceNotifyCallback {
    commands: mpsc::Sender<Command>,
}

impl IMMNotificationClient_Impl for DeviceNotifyCallback_Impl {
    fn OnDeviceStateChanged(&self, _pwstrdeviceid: &PCWSTR, _dwnewstate: DEVICE_STATE) -> Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _pwstrdefaultdeviceid: &PCWSTR,
    ) -> Result<()> {
        if flow == eRender && role == eMultimedia {
            let _ = self.commands.send(Command::ReattachAudio);
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _pwstrdeviceid: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

fn publish_volume(
    hwnd_raw: usize,
    shared: &Arc<Mutex<SharedState>>,
    value: f32,
    muted: bool,
    show_hud: bool,
) {
    {
        let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
        state.snapshot.volume = value;
        state.snapshot.muted = muted;
        if show_hud {
            state.snapshot.hud = HudState {
                kind: HudKind::Volume,
                value,
                muted,
                visible_until: Some(Instant::now() + HUD_DURATION),
            };
        }
    }

    if show_hud {
        let hwnd = HWND(hwnd_raw as *mut c_void);
        unsafe {
            let _ = PostMessageW(hwnd, WM_SYSTEM_UPDATED, WPARAM(0), LPARAM(0));
        }
    }
}

fn publish_brightness(hwnd_raw: usize, shared: &Arc<Mutex<SharedState>>, value: f32) {
    {
        let mut state = shared.lock().unwrap_or_else(|p| p.into_inner());
        state.brightness_seeded = true;
        state.last_brightness = Some(value);
        state.snapshot.hud = HudState {
            kind: HudKind::Brightness,
            value,
            muted: false,
            visible_until: Some(Instant::now() + HUD_DURATION),
        };
    }

    let hwnd = HWND(hwnd_raw as *mut c_void);
    unsafe {
        let _ = PostMessageW(hwnd, WM_SYSTEM_UPDATED, WPARAM(0), LPARAM(0));
    }
}

struct AudioSession {
    endpoint: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl AudioSession {
    unsafe fn attach(
        enumerator: &IMMDeviceEnumerator,
        hwnd_raw: usize,
        shared: &Arc<Mutex<SharedState>>,
    ) -> Option<Self> {
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;

        // Seed initial volume without showing the transient HUD.
        if let (Ok(v), Ok(m)) = (endpoint.GetMasterVolumeLevelScalar(), endpoint.GetMute()) {
            publish_volume(hwnd_raw, shared, v.clamp(0.0, 1.0), m.as_bool(), false);
        }

        let callback: IAudioEndpointVolumeCallback = VolumeCallback {
            hwnd_raw,
            shared: Arc::clone(shared),
        }
        .into();

        if endpoint.RegisterControlChangeNotify(&callback).is_err() {
            return None;
        }

        Some(Self { endpoint, callback })
    }

    unsafe fn detach(self) {
        let _ = self.endpoint.UnregisterControlChangeNotify(&self.callback);
    }
}

unsafe fn worker_main(
    hwnd_raw: usize,
    shared: Arc<Mutex<SharedState>>,
    commands: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Command>,
) {
    let com_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();

    let enumerator: Option<IMMDeviceEnumerator> =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok();

    let device_callback: Option<IMMNotificationClient> = if let Some(ref e) = enumerator {
        let cb: IMMNotificationClient = DeviceNotifyCallback {
            commands: commands.clone(),
        }
        .into();
        if e.RegisterEndpointNotificationCallback(&cb).is_ok() {
            Some(cb)
        } else {
            None
        }
    } else {
        None
    };

    let mut audio_session = enumerator
        .as_ref()
        .and_then(|e| AudioSession::attach(e, hwnd_raw, &shared));

    while let Ok(command) = receiver.recv() {
        match command {
            Command::RefreshVolume { show_hud } => {
                // Give Windows a brief moment to apply the key press if needed.
                if show_hud {
                    thread::sleep(Duration::from_millis(25));
                }
                if let Some(ref session) = audio_session {
                    if let (Ok(v), Ok(m)) = (
                        session.endpoint.GetMasterVolumeLevelScalar(),
                        session.endpoint.GetMute(),
                    ) {
                        publish_volume(hwnd_raw, &shared, v.clamp(0.0, 1.0), m.as_bool(), show_hud);
                    }
                }
            }
            Command::RefreshBrightness => {
                // Small delay so the display driver finishes updating its level.
                thread::sleep(Duration::from_millis(60));
                if let Some(value) = read_brightness_on_worker() {
                    publish_brightness(hwnd_raw, &shared, value);
                }
            }
            Command::ReattachAudio => {
                if let Some(old) = audio_session.take() {
                    old.detach();
                }
                audio_session = enumerator
                    .as_ref()
                    .and_then(|e| AudioSession::attach(e, hwnd_raw, &shared));
            }
            Command::Shutdown => break,
        }
    }

    if let Some(session) = audio_session.take() {
        session.detach();
    }
    if let (Some(ref e), Some(ref cb)) = (&enumerator, &device_callback) {
        let _ = e.UnregisterEndpointNotificationCallback(cb);
    }
    drop(device_callback);
    drop(enumerator);

    if com_ok {
        CoUninitialize();
    }
}

/// On-demand brightness lookup (runs on the MTA worker thread):
/// WMI `WmiMonitorBrightness` for internal laptop panels, then DDC/CI for
/// external monitors.
unsafe fn read_brightness_on_worker() -> Option<f32> {
    read_brightness_wmi().or_else(|| read_brightness_ddc())
}

unsafe fn read_brightness_wmi() -> Option<f32> {
    (|| -> Result<f32> {
        let locator: IWbemLocator =
            CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let services: IWbemServices = locator.ConnectServer(
            &BSTR::from("root\\WMI"),
            &BSTR::new(),
            &BSTR::new(),
            &BSTR::new(),
            0,
            &BSTR::new(),
            None,
        )?;
        let _ = CoSetProxyBlanket(
            &services,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        );
        let query = services.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE"),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?;
        let _ = CoSetProxyBlanket(
            &query,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        );
        let mut objs = [None; 1];
        let mut returned = 0u32;
        query.Next(WBEM_INFINITE, &mut objs, &mut returned).ok()?;
        if returned == 0 {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(-1)));
        }
        let obj = objs[0]
            .take()
            .ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
        let mut value = VARIANT::default();
        obj.Get(windows::core::w!("CurrentBrightness"), 0, &mut value, None, None)?;
        let n = variant_to_f32(&value)
            .ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
        Ok((n / 100.0).clamp(0.0, 1.0))
    })()
    .ok()
}

unsafe fn read_brightness_ddc() -> Option<f32> {
    let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);

    let mut count: u32 = 0;
    if GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut count).is_err() || count == 0 {
        return None;
    }

    let mut monitors: Vec<PHYSICAL_MONITOR> = vec![std::mem::zeroed(); count as usize];
    if GetPhysicalMonitorsFromHMONITOR(hmon, &mut monitors).is_err() {
        return None;
    }

    let mut result = None;
    for m in &monitors {
        let mut min = 0u32;
        let mut cur = 0u32;
        let mut max = 0u32;
        let ok = GetMonitorBrightness(m.hPhysicalMonitor, &mut min, &mut cur, &mut max);
        if ok != 0 && max > min {
            result = Some(((cur - min) as f32 / (max - min) as f32).clamp(0.0, 1.0));
            break;
        }
    }

    let _ = DestroyPhysicalMonitors(&monitors);
    result
}

unsafe fn variant_to_f32(v: &VARIANT) -> Option<f32> {
    #[repr(C)]
    struct RawVariant {
        vt: u16,
        _r1: u16,
        _r2: u16,
        _r3: u16,
        val: u64,
        _rest: u64,
    }
    let raw: &RawVariant = std::mem::transmute(v);
    match raw.vt {
        17 => Some((raw.val as u8) as f32),        // VT_UI1
        2 => Some((raw.val as i16) as f32),        // VT_I2
        18 => Some((raw.val as u16) as f32),       // VT_UI2
        3 | 22 => Some((raw.val as i32) as f32),   // VT_I4 / VT_INT
        19 => Some((raw.val as u32) as f32),       // VT_UI4
        4 => Some(f32::from_bits(raw.val as u32)), // VT_R4
        5 => Some(f64::from_bits(raw.val) as f32), // VT_R8
        _ => None,
    }
}
