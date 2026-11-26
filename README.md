# Zekrima — Native Dynamic Island Overlay

![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)
![Windows](https://img.shields.io/badge/Windows-11%20%2F%2010-0078D6?logo=windows11)
![Arch Linux](https://img.shields.io/badge/Arch%20Linux-CachyOS-1793D1?logo=archlinux)
![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)

**Zekrima** adalah aplikasi **Dynamic Island native** super ringan yang dibangun murni dengan **Rust** — tanpa WebView, Electron, atau Tauri. Dirancang untuk menemani alur kerja harian di **Windows 11/10** maupun **Arch Linux / CachyOS** dengan konsumsi `0.0%` idle CPU, animasi *damped harmonic spring*, serta integrasi sistem yang cepat dan tidak mengganggu fokus kerja.

> Proyek ini **terus dikembangkan secara aktif** dengan fitur-fitur produktivitas baru untuk mempermudah pekerjaan sehari-hari.

---

## Fitur Saat Ini (`v0.1.0`)

- **Tampilan Jam Tegas & Jelas**
  - Jam **`HH:MM`** tebal dan mudah dibaca di mode collapsed pill (`16.5px` Bold), jam besar **`32px` Bold + tanggal** di mode expanded idle, serta jam **`18.5px` Bold** di pojok kanan kartu media.
- **Equalizer Musik Mengikuti Irama Audio Asli (Real-Time FFT)**
  - 5 band frekuensi musikal (Sub-bass, Bass, Vokal, Melodi, Treble) menggunakan **512-point FFT pure Rust**:
    - **Windows**: WASAPI Loopback (`AUDCLNT_STREAMFLAGS_LOOPBACK`).
    - **Arch Linux / CachyOS**: PipeWire `@DEFAULT_MONITOR@` (`parec`).
  - Capture audio & timer animasi otomatis berhenti saat musik di-pause (`0.0%` idle CPU).
- **Kontrol Media Native**
  - Menampilkan album art, judul lagu, artis, dan tombol **Previous / Play-Pause / Next** (WinRT SMTC di Windows, MPRIS2 `playerctl --follow` di Linux).
- **Notifikasi Desktop Bergaya Pillar**
  - **Incoming Toast**: warna aksen otomatis per aplikasi, avatar inisial, judul, isi, tombol dismiss `×`, dan **progress bar countdown ~3,8 detik**.
  - **Collapsed Pill Badge**: avatar warna aplikasi + badge merah jumlah notifikasi (`1`–`9+`).
  - **Expanded Notifications Panel**: daftar notifikasi, klik untuk membuka aplikasi pengirim (Windows AUMID), dismiss satuan, serta **`Clear all`**.
- **Volume & Brightness HUD Event-Driven**
  - Bar HUD sementara saat mengubah volume, mute, atau kecerahan layar tanpa polling terus-menerus.
- **Click-Through, Non-Focus-Stealing & Fullscreen Auto-Hide**
  - Klik di luar area pill selalu tembus ke jendela kerja di bawahnya; klik pill tidak mencuri fokus keyboard.
  - Di Windows, pill otomatis bersembunyi saat aplikasi/game fullscreen dan muncul kembali di lapisan teratas setelah keluar fullscreen.
- **System Tray & Autostart**
  - Dukungan autostart saat login (`HKCU\...\Run` di Windows, XDG `~/.config/autostart/zekrima.desktop` / Hyprland `exec-once` di Linux).

---

## Struktur Proyek

| Platform | Folder | Stack Native | Panduan Lengkap |
| :--- | :--- | :--- | :--- |
| **Windows 11 / 10** | [`zekrima-windows/`](zekrima-windows/) | Win32, Direct2D, DirectWrite, WinRT SMTC, WASAPI | [Panduan Windows](zekrima-windows/README.md) |
| **Arch Linux / CachyOS** | [`zekrima-arch-linux/`](zekrima-arch-linux/) | GTK4, `gtk4-layer-shell` (Wayland), Cairo/Pango, PipeWire, MPRIS2, D-Bus | [Panduan Arch / CachyOS](zekrima-arch-linux/README.md) |

---

## Quick Start

### Windows 11 / 10
- **Portable**: unduh `zekrima-windows-portable.zip` dari halaman **Releases**, ekstrak, lalu jalankan `zekrima.exe`.
- **Build dari source**:
  ```powershell
  cd zekrima-windows
  cargo build --release
  .\target\release\zekrima.exe
  ```
Detail instalasi, izin notifikasi Windows, & pemakaian lengkap: [zekrima-windows/README.md](zekrima-windows/README.md).

### Arch Linux / CachyOS
- **Instalasi otomatis (1 perintah)**:
  ```bash
  cd zekrima-arch-linux
  chmod +x install-cachyos.sh
  ./install-cachyos.sh
  zekrima &
  ```
- **Manual via `makepkg`**:
  ```bash
  cd zekrima-arch-linux
  makepkg -si
  ```
Detail instalasi Hyprland / KDE Plasma 6 Wayland, autostart, & uninstall: [zekrima-arch-linux/README.md](zekrima-arch-linux/README.md).

---

## Roadmap Pengembangan (Aktif)

Fitur-fitur produktivitas yang direncanakan untuk rilis berikutnya agar semakin membantu pekerjaan harian:

- [ ] **Focus / Pomodoro Timer** — timer kerja langsung di pill dengan progress bar & pengingat selesai.
- [ ] **Quick Clipboard & Scratchpad** — akses cepat riwayat clipboard & catatan singkat tanpa pindah jendela.
- [ ] **System Monitor Ringkas** — indikator CPU, RAM, suhu, & jaringan saat expanded.
- [ ] **Battery & Bluetooth Status** — notifikasi pengisian daya, baterai rendah, dan status perangkat audio Bluetooth.
- [ ] **Calendar & Meeting Reminder** — hitung mundur jadwal meeting berikutnya langsung di collapsed pill.
- [ ] **Kustomisasi Tema & Multi-Monitor** — pengaturan warna aksen, posisi monitor, dan file konfigurasi `config.toml`.
- [ ] **Auto-Update & AUR Package** — paket AUR (`zekrima-bin` / `zekrima-git`) untuk Arch Linux & CachyOS.

---

## CI/CD & Rilis Otomatis
- **CI** ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)): otomatis mengecek dan mem-build versi **Windows** dan **Arch Linux** pada setiap push/PR ke `main`.
- **Release** ([`.github/workflows/release.yml`](.github/workflows/release.yml)): otomatis mem-build dan menerbitkan aset `zekrima.exe`, `zekrima-windows-portable.zip`, dan `zekrima-arch-cachyos.tar.gz` ke **GitHub Releases** setiap kali tag versi (`v*`) di-push:
  ```bash
  git tag v0.1.0
  git push origin v0.1.0
  ```

## Lisensi
Dirilis di bawah lisensi [MIT](LICENSE).
