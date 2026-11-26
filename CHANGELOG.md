# Changelog

All notable changes to **Zekrima** will be documented in this file.

## [0.1.0] - 2026-11-26

### Added
- **Windows 11 / 10 Native Edition (`zekrima-windows/`)**:
  - Pure native Win32 + Direct2D + DirectWrite layered overlay (`~0.42 MB` release binary, `0.0%` idle CPU).
  - Damped harmonic spring physics, per-pixel transparency, click-through hit-testing (`WM_NCHITTEST`), and zero focus stealing (`WS_EX_NOACTIVATE`).
  - Clear bold clock (`HH:MM`) across collapsed pill, expanded idle hero card (+ date), and expanded media card.
  - Real-time 5-band audio-reactive music equalizer using WASAPI Loopback (`AUDCLNT_STREAMFLAGS_LOOPBACK`) + pure-Rust 512-point FFT.
  - Event-driven WinRT SMTC media controls (album art with hash caching, title, artist, Previous / Play-Pause / Next).
  - Pillar-style Windows notifications (`UserNotificationListener` with real-time `NotificationChanged`, fallback polling, colored app avatar, incoming toast countdown bar, collapsed badge, expanded list, click-to-activate app via AUMID, and Windows Settings shortcut).
  - Event-driven Volume HUD (`IAudioEndpointVolumeCallback` + `IMMNotificationClient`) and Brightness HUD (`WM_POWERBROADCAST` + WMI/DDC-CI fallback).
  - Native System Tray (`Run at Windows Startup`, `Notification Settings`, `Reset Position`, `Exit`), single-instance mutex, and fullscreen auto-hide (`SHQueryUserNotificationState`).
- **Arch Linux / CachyOS Native Edition (`zekrima-arch-linux/`)**:
  - Native Rust + GTK4 + `gtk4-layer-shell` (Wayland `Layer::Overlay` + X11 fallback) + Cairo/PangoCairo renderer.
  - Dynamic click-through input region (`GdkSurface::set_input_region`) and `KeyboardMode::None` non-activating overlay.
  - Real-time 5-band PipeWire `@DEFAULT_MONITOR@` FFT music visualizer (`parec`).
  - Event-driven MPRIS2 media controls (`playerctl --follow`), Volume HUD (`pactl subscribe` + `wpctl`), Brightness HUD (`udevadm monitor` + `/sys/class/backlight`), and Pillar-style D-Bus notification mirroring.
  - Native Arch `PKGBUILD`, `zekrima.desktop`, and one-command `install-cachyos.sh`.
- **GitHub Automation**:
  - CI build workflow for Windows and Arch Linux.
  - Automated GitHub Release workflow triggered on `v*` tags.
