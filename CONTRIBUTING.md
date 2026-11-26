# Contributing to Zekrima

Terima kasih tertarik berkontribusi atau mengembangkan **Zekrima**!

## Struktur Repositori
- `zekrima-windows/` — Aplikasi native Windows 11/10 (Win32, Direct2D, WinRT, WASAPI).
- `zekrima-arch-linux/` — Aplikasi native Arch Linux / CachyOS (GTK4, `gtk4-layer-shell`, Cairo/Pango, PipeWire, MPRIS2, D-Bus).

## Prinsip Pengembangan
1. **Ringan & Native**: Hindari WebView, Electron, atau runtime berat. Jaga konsumsi idle CPU tetap **`0.0%`** (event-driven, tanpa continuous polling loop).
2. **Jangan Mencuri Fokus**: Interaksi dengan pill tidak boleh mengganggu jendela kerja aktif pengguna.
3. **Zero Warnings**: Pastikan `cargo check --all-targets` dan `cargo build --release` bersih tanpa warning.

## Menjalankan Build Lokal
- **Windows**:
  ```powershell
  cd zekrima-windows
  cargo check --all-targets
  cargo build --release
  ```
- **Arch Linux / CachyOS**:
  ```bash
  cd zekrima-arch-linux
  cargo check --all-targets
  makepkg -si
  ```
