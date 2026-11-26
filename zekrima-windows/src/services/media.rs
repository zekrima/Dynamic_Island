//! Event-driven Windows System Media Transport Controls (SMTC) integration.
//!
//! The WinRT session manager is owned by a dedicated MTA worker thread. The
//! worker never polls: it waits on a channel and wakes up when Windows raises
//! `CurrentSessionChanged`, `MediaPropertiesChanged` or `PlaybackInfoChanged`,
//! or when the UI submits a control command (play/pause, next, previous).
//!
//! Each refresh copies the small metadata snapshot into an `Arc<Mutex<_>>` and
//! posts `WM_MEDIA_UPDATED` to the island window. Album art is decoded on the
//! worker into premultiplied BGRA bytes, so the UI thread only has to upload a
//! ready-to-use bitmap to Direct2D.

use std::ffi::c_void;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use windows::core::Interface;
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::{
    DataReader, IInputStream, IRandomAccessStream, IRandomAccessStreamReference,
    IRandomAccessStreamWithContentType,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::{
    IWICBitmapSource, IWICImagingFactory, CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{CoCreateInstance, IStream, CLSCTX_INPROC_SERVER};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Custom message posted by the media worker after the shared state changed.
pub const WM_MEDIA_UPDATED: u32 = 0x8001;

/// Refuse absurd thumbnails so a broken provider cannot exhaust memory.
const MAX_THUMBNAIL_BYTES: u64 = 24 * 1024 * 1024;
/// Album art is downscaled before being shared with the UI.
const MAX_ART_EDGE: u32 = 256;
/// A decoded source larger than this is treated as corrupt.
const MAX_SOURCE_EDGE: u32 = 16384;

/// Decoded, premultiplied BGRA album art ready for `ID2D1Bitmap`.
pub struct AlbumArt {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Small snapshot copied between the media worker and the UI thread.
#[derive(Clone, Default)]
pub struct MediaInfo {
    pub has_session: bool,
    pub title: String,
    pub artist: String,
    pub is_playing: bool,
    pub album_art: Option<Arc<AlbumArt>>,
    /// Changes only when new album art has been decoded.
    pub art_revision: u64,
}

/// The three transport controls rendered inside the expanded pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaControl {
    Previous,
    PlayPause,
    Next,
}

enum Command {
    Refresh,
    Control(MediaControl),
    Shutdown,
}

/// Handle to the SMTC worker thread.
pub struct MediaService {
    shared: Arc<Mutex<MediaInfo>>,
    commands: mpsc::Sender<Command>,
    worker: Option<JoinHandle<()>>,
}

impl MediaService {
    /// Starts the media worker. If the thread cannot be created the service
    /// degrades to a no-op and the island keeps running without media.
    pub fn start(hwnd: HWND) -> Self {
        let shared = Arc::new(Mutex::new(MediaInfo::default()));
        let (commands, receiver) = mpsc::channel();

        let worker_shared = Arc::clone(&shared);
        let worker_commands = commands.clone();
        let hwnd = hwnd.0 as usize;

        let worker = thread::Builder::new()
            .name("zekrima-media".to_owned())
            .spawn(move || unsafe {
                worker_main(hwnd, worker_shared, worker_commands, receiver);
            })
            .ok();

        Self {
            shared,
            commands,
            worker,
        }
    }

    /// Copies the latest metadata snapshot.
    pub fn snapshot(&self) -> MediaInfo {
        let guard = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clone()
    }

    /// Sends a transport command to the worker. Never blocks the UI thread.
    pub fn control(&self, control: MediaControl) {
        let _ = self.commands.send(Command::Control(control));
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct SessionHooks {
    media: EventRegistrationToken,
    playback: EventRegistrationToken,
}

struct Worker {
    hwnd: HWND,
    shared: Arc<Mutex<MediaInfo>>,
    commands: mpsc::Sender<Command>,
    manager: GlobalSystemMediaTransportControlsSessionManager,
    session: Option<GlobalSystemMediaTransportControlsSession>,
    hooks: Option<SessionHooks>,
    wic: Option<IWICImagingFactory>,
    art_revision: u64,
    last_art_hash: Option<u64>,
    last_art: Option<Arc<AlbumArt>>,
}

impl Worker {
    unsafe fn refresh(&mut self) {
        let current = self.manager.GetCurrentSession().ok();

        if self.session != current {
            self.detach_session();
            self.session = current;
            if let Some(session) = self.session.clone() {
                self.attach_session(&session);
            }
        }

        let Some(session) = self.session.clone() else {
            if self.last_art.is_some() {
                self.last_art = None;
                self.last_art_hash = None;
                self.art_revision = self.art_revision.wrapping_add(1);
            }
            self.publish(MediaInfo {
                art_revision: self.art_revision,
                ..Default::default()
            });
            return;
        };

        let is_playing = session
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .map(|status| status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            .unwrap_or(false);

        let mut next = MediaInfo {
            has_session: true,
            is_playing,
            ..Default::default()
        };

        if let Ok(properties) = session
            .TryGetMediaPropertiesAsync()
            .and_then(|operation| operation.get())
        {
            next.title = properties
                .Title()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();
            next.artist = properties
                .Artist()
                .map(|value| value.to_string_lossy())
                .unwrap_or_default();

            let mut decoded_art: Option<Arc<AlbumArt>> = None;
            if let Ok(thumbnail) = properties.Thumbnail() {
                if let Some(bytes) = read_thumbnail_bytes(&thumbnail) {
                    let hash = fnv1a_hash(&bytes);
                    if self.last_art_hash == Some(hash) && self.last_art.is_some() {
                        decoded_art = self.last_art.clone();
                    } else if let Some(art) = self.decode_bytes(&bytes) {
                        let arc = Arc::new(art);
                        self.last_art_hash = Some(hash);
                        self.last_art = Some(Arc::clone(&arc));
                        self.art_revision = self.art_revision.wrapping_add(1);
                        decoded_art = Some(arc);
                    }
                }
            }

            if decoded_art.is_none() && self.last_art.is_some() {
                self.last_art = None;
                self.last_art_hash = None;
                self.art_revision = self.art_revision.wrapping_add(1);
            }
            next.album_art = decoded_art;
        } else {
            next.album_art = self.last_art.clone();
        }

        next.art_revision = self.art_revision;
        self.publish(next);
    }

    unsafe fn attach_session(&mut self, session: &GlobalSystemMediaTransportControlsSession) {
        let media_commands = self.commands.clone();
        let media_handler = TypedEventHandler::new(move |_sender, _args| {
            let _ = media_commands.send(Command::Refresh);
            Ok(())
        });

        let playback_commands = self.commands.clone();
        let playback_handler = TypedEventHandler::new(move |_sender, _args| {
            let _ = playback_commands.send(Command::Refresh);
            Ok(())
        });

        let media = session.MediaPropertiesChanged(&media_handler).ok();
        let playback = session.PlaybackInfoChanged(&playback_handler).ok();

        if let (Some(media), Some(playback)) = (media, playback) {
            self.hooks = Some(SessionHooks { media, playback });
        }
    }

    fn detach_session(&mut self) {
        if let (Some(session), Some(hooks)) = (&self.session, self.hooks.take()) {
            let _ = session.RemoveMediaPropertiesChanged(hooks.media);
            let _ = session.RemovePlaybackInfoChanged(hooks.playback);
        }
    }

    fn publish(&mut self, next: MediaInfo) {
        let changed = {
            let mut guard = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            let changed = guard.has_session != next.has_session
                || guard.title != next.title
                || guard.artist != next.artist
                || guard.is_playing != next.is_playing
                || guard.art_revision != next.art_revision
                || guard.album_art.is_some() != next.album_art.is_some();

            if changed {
                *guard = next;
            }
            changed
        };

        if changed {
            unsafe {
                let _ = PostMessageW(self.hwnd, WM_MEDIA_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn send_control(&mut self, control: MediaControl) {
        let Some(session) = self.session.clone() else {
            return;
        };

        unsafe {
            match control {
                MediaControl::PlayPause => {
                    let _ = session.TryTogglePlayPauseAsync().and_then(|op| op.get());
                }
                MediaControl::Next => {
                    let _ = session.TrySkipNextAsync().and_then(|op| op.get());
                }
                MediaControl::Previous => {
                    let _ = session.TrySkipPreviousAsync().and_then(|op| op.get());
                }
            }

            // Reflect the new state immediately; the SMTC event that follows
            // simply confirms it (publish deduplicates identical snapshots).
            self.refresh();
        }
    }

    unsafe fn decode_bytes(&mut self, bytes: &[u8]) -> Option<AlbumArt> {
        if self.wic.is_none() {
            self.wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
        }
        let factory = self.wic.as_ref()?;

        let wic_stream = factory.CreateStream().ok()?;
        wic_stream.InitializeFromMemory(bytes).ok()?;

        let stream: IStream = wic_stream.cast().ok()?;
        let decoder = factory
            .CreateDecoderFromStream(
                &stream,
                std::ptr::null(),
                WICDecodeMetadataCacheOnLoad,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let source: IWICBitmapSource = frame.cast().ok()?;

        let mut source_width = 0u32;
        let mut source_height = 0u32;
        source
            .GetSize(&mut source_width, &mut source_height)
            .ok()?;
        if source_width == 0
            || source_height == 0
            || source_width > MAX_SOURCE_EDGE
            || source_height > MAX_SOURCE_EDGE
        {
            return None;
        }

        let scale =
            (MAX_ART_EDGE as f32 / source_width.max(source_height) as f32).min(1.0);
        let scaled_width = ((source_width as f32 * scale).round() as u32).max(1);
        let scaled_height = ((source_height as f32 * scale).round() as u32).max(1);

        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(
                &source,
                scaled_width,
                scaled_height,
                WICBitmapInterpolationModeFant,
            )
            .ok()?;
        let scaled: IWICBitmapSource = scaler.cast().ok()?;

        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &scaled,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;

        let mut width = 0u32;
        let mut height = 0u32;
        converter.GetSize(&mut width, &mut height).ok()?;
        if width == 0 || height == 0 || width > MAX_ART_EDGE || height > MAX_ART_EDGE {
            return None;
        }

        let stride = width.checked_mul(4)?;
        let length = (stride as usize).checked_mul(height as usize)?;
        let mut pixels = vec![0u8; length];
        converter
            .CopyPixels(std::ptr::null(), stride, &mut pixels)
            .ok()?;

        apply_rounded_corners(&mut pixels, width, height);

        Some(AlbumArt {
            width,
            height,
            pixels,
        })
    }
}

unsafe fn worker_main(
    hwnd: usize,
    shared: Arc<Mutex<MediaInfo>>,
    commands: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Command>,
) {
    if RoInitialize(RO_INIT_MULTITHREADED).is_err() {
        return;
    }

    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|operation| operation.get())
    {
        Ok(manager) => manager,
        Err(_) => {
            RoUninitialize();
            return;
        }
    };

    let wake = commands.clone();
    let manager_handler = TypedEventHandler::new(move |_sender, _args| {
        let _ = wake.send(Command::Refresh);
        Ok(())
    });
    let manager_token = manager.CurrentSessionChanged(&manager_handler).ok();

    let mut worker = Worker {
        hwnd: HWND(hwnd as *mut c_void),
        shared,
        commands,
        manager: manager.clone(),
        session: None,
        hooks: None,
        wic: None,
        art_revision: 0,
        last_art_hash: None,
        last_art: None,
    };

    worker.refresh();

    while let Ok(command) = receiver.recv() {
        match command {
            Command::Refresh => worker.refresh(),
            Command::Control(control) => worker.send_control(control),
            Command::Shutdown => break,
        }
    }

    worker.detach_session();
    if let Some(token) = manager_token {
        let _ = manager.RemoveCurrentSessionChanged(token);
    }
    drop(manager_handler);
    drop(worker);

    RoUninitialize();
}

unsafe fn read_thumbnail_bytes(thumbnail: &IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = thumbnail.OpenReadAsync().ok()?.get().ok()?;
    let bytes = read_stream(&stream);
    let _ = stream.Close();
    bytes.filter(|b| !b.is_empty())
}

fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

unsafe fn read_stream(stream: &IRandomAccessStreamWithContentType) -> Option<Vec<u8>> {
    let random_access: IRandomAccessStream = stream.cast().ok()?;
    let size = random_access.Size().ok()?;
    if size == 0 || size > MAX_THUMBNAIL_BYTES {
        return None;
    }

    let input: IInputStream = stream.cast().ok()?;
    let reader = DataReader::CreateDataReader(&input).ok()?;
    reader.LoadAsync(size as u32).ok()?.get().ok()?;

    let mut bytes = vec![0u8; size as usize];
    let result = reader.ReadBytes(&mut bytes);
    let _ = reader.Close();
    result.ok()?;

    Some(bytes)
}

/// Masks the bitmap corners at decode time so the same bitmap can be drawn at
/// both the 20 px and 72 px sizes without a Direct2D layer per frame.
fn apply_rounded_corners(pixels: &mut [u8], width: u32, height: u32) {
    let short_edge = width.min(height) as f32;
    let radius = (short_edge * 0.2).max(2.0).min(short_edge * 0.5);
    let width_f = width as f32;
    let height_f = height as f32;

    for y in 0..height {
        let py = y as f32 + 0.5;
        let dy = ((py - height_f / 2.0).abs() - (height_f / 2.0 - radius)).max(0.0);

        for x in 0..width {
            let px = x as f32 + 0.5;
            let dx = ((px - width_f / 2.0).abs() - (width_f / 2.0 - radius)).max(0.0);
            let distance = (dx * dx + dy * dy).sqrt() - radius;
            let coverage = (0.5 - distance).clamp(0.0, 1.0);

            if coverage >= 1.0 {
                continue;
            }

            let index = ((y * width + x) * 4) as usize;
            // Premultiplied BGRA: scale colour and alpha together.
            for channel in 0..4 {
                pixels[index + channel] =
                    (pixels[index + channel] as f32 * coverage).round() as u8;
            }
        }
    }
}
