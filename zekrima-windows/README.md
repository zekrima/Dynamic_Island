# Zekrima untuk Windows 11 / 10

Dynamic Island native berbasis **Rust + Win32 + Direct2D + DirectWrite + WinRT + WASAPI** tanpa WebView/Electron/Tauri (~0,42 MB, `0.0%` CPU saat idle).

---

## 1. Persyaratan Sistem
- **OS**: Windows 10 (1903+) atau Windows 11 (disarankan).
- **Menjalankan binary portable**: tidak memerlukan runtime tambahan (langsung jalan).
- **Build dari source (opsional)**: [Rust Toolchain](https://rustup.rs/) (`cargo` + MSVC Build Tools).

---

## 2. Cara Instalasi

### Cara A — Langsung Pakai (Portable, Tanpa Install)
1. Buka folder `bin/` di dalam folder ini (`zekrima-windows\bin\zekrima.exe`), atau ekstrak `zekrima-windows-portable.zip`.
2. Pindahkan `zekrima.exe` ke folder permanen pilihanmu (misalnya `C:\Apps\Zekrima\zekrima.exe`).
3. Klik ganda **`zekrima.exe`** untuk menjalankannya.
4. *(Opsional)* Agar otomatis jalan setiap kali Windows dinyalakan:
   - Klik kanan ikon **Zekrima** di **System Tray** (pojok kanan bawah taskbar) → pilih **`Run at Windows Startup`**.

### Cara B — Build dari Source Code
Buka PowerShell di dalam folder `zekrima-windows/`:

```powershell
cargo build --release
```

Hasil binary release tersedia di:
```powershell
.\target\release\zekrima.exe
```

Untuk langsung menjalankan dari PowerShell:
```powershell
.\target\release\zekrima.exe
```

---

## 3. Mengaktifkan Fitur Notifikasi Windows (Gaya Pillar)
Zekrima memakai Windows `UserNotificationListener` untuk menampilkan toast notifikasi & membuka aplikasi pengirim saat diklik.

1. Klik kanan pill atau ikon System Tray → pilih **`Notification Settings`** (atau buka **Windows Settings → Privacy & security → Notifications**).
2. Pastikan **`Notification access`** dan **`Let desktop apps access your notifications`** dalam keadaan **ON**.
3. Zekrima akan otomatis mendeteksi izin tanpa perlu restart.

---

## 4. Panduan Penggunaan

### Interaksi Dasar
| Aksi | Fungsi |
| :--- | :--- |
| **Arahkan kursor (Hover) ke pill** | Pill mengembang mulus dengan animasi pegas (*spring*). |
| **Geser kursor keluar dari pill** | Pill menyusut kembali ke ukuran ringkas. |
| **Klik kiri pada pill** | Toggle mengembang/menyusut (tanpa mencuri fokus keyboard dari aplikasi aktif). |
| **Klik di area transparan luar pill** | Klik tembus (*click-through*) ke jendela di bawahnya. |
| **Klik kanan pada pill** | Menu cepat: `Notification Settings`, `Reset Position`, `Exit`. |
| **Tekan `Esc` saat kursor di atas pill** | Keluar dari Zekrima dengan cepat. |

### 5 Tab Produktivitas (Mode Expanded)
Saat pill mengembang (`440 × 162`), tersedia 5 tab di bagian atas:
- **`Media`**: kontrol musik (album art, judul/artis, Previous/Play-Pause/Next, visualizer FFT, jam) atau jam besar + volume saat idle.
- **`Timer`**: Focus / Pomodoro Timer dengan preset **`5m`**, **`15m`**, **`25m`**, **`45m`**, tombol **`Start`/`Pause`** & **`Reset`**, progress bar, indikator countdown di collapsed pill, serta toast notifikasi otomatis saat waktu habis.
- **`Clip`**: riwayat teks clipboard (`Ctrl+C`) berbasis `WM_CLIPBOARDUPDATE` (event-driven, 0% polling). Klik baris mana saja untuk copy ulang secara instan, atau klik **`Clear`**.
- **`Sys`**: System Monitor ringkas (**CPU %**, **Memory GB/%**, **Battery/Power**) dengan bar berkode warna — hanya di-sample saat tab `Sys` dibuka (`0.0%` idle CPU).
- **`Notifs`**: daftar notifikasi Windows bergaya Pillar.

### Jam & Status Idle
- **Mode Collapsed**: menampilkan jam **`HH:MM`** tebal dan jelas bersama indikator Zekrima (dan badge notifikasi jika ada).
- **Mode Expanded (tanpa media)**: menampilkan **jam besar + tanggal**, status notifikasi, serta bar level volume sistem secara real-time.

### Media & Bar Musik Mengikuti Irama
- Putar musik/video di **Spotify, YouTube (Chrome/Edge/Firefox), Apple Music, VLC**, atau pemutar lain yang mendukung Windows SMTC.
- **Mode Collapsed**: menampilkan album art mini, jam **`HH:MM`**, dan **5 bar equalizer real-time** yang bergerak mengikuti irama/beat lagu lewat **WASAPI Loopback + 512-point FFT**.
- **Mode Expanded**: menampilkan album art besar, judul lagu, nama artis, tombol **Previous / Play-Pause / Next**, bar visualizer real-time, serta jam di kanan bawah.
- Saat musik di-pause atau berhenti, capture audio WASAPI otomatis dimatikan agar penggunaan CPU kembali `0.0%`.

### Notifikasi (Gaya Pillar)
- **Incoming Toast**: saat notifikasi baru masuk, pill melebar menampilkan avatar warna aplikasi, nama aplikasi, judul, isi, tombol `×`, dan **progress bar countdown ~3,8 detik**.
  - **Klik isi toast**: membuka/membawa aplikasi pengirim ke depan (seperti Action Center) dan menutup toast.
  - **Klik tombol `×`**: menghapus notifikasi dari Zekrima dan Action Center.
- **Collapsed Badge**: setelah toast selesai, avatar warna aplikasi + badge merah angka (`1`–`9+`) tetap tampil di pill.
- **Panel Notifications**: klik tombol **`Notifs`** di pojok kanan atas saat expanded (atau langsung hover saat tidak ada media) untuk melihat daftar notifikasi, menghapus satu per satu, atau **`Clear all`**.

### Volume & Brightness HUD
- Tekan tombol **Volume Up / Down / Mute** (atau ubah volume dari slider taskbar/headset) → HUD Volume otomatis muncul selama ~1,8 detik (event-driven via Core Audio).
- Ubah kecerahan layar (**Brightness**) → HUD Brightness otomatis muncul via `WM_POWERBROADCAST` (dengan fallback WMI/DDC-CI untuk monitor eksternal).

### System Tray & Fullscreen Auto-Hide
- **System Tray** (klik kanan ikon Zekrima):
  - `Run at Windows Startup` — menambah/menghapus entri di `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
  - `Notification Settings` — membuka pengaturan izin notifikasi Windows.
  - `Reset Position` — memusatkan ulang pill di tengah atas monitor utama.
  - `Exit` — menutup aplikasi dengan bersih.
- **Fullscreen Auto-Hide**: saat menjalankan game atau video layar penuh, pill otomatis bersembunyi dan muncul kembali di posisi paling atas (`TOPMOST`) setelah keluar dari fullscreen.

---

## 5. Pemecahan Masalah (Troubleshooting)
- **Aplikasi tidak muncul saat diklik 2x**: Zekrima memakai *single-instance guard*. Cek System Tray di pojok kanan bawah apakah Zekrima sudah berjalan.
- **Notifikasi tidak masuk**: Pastikan opsi *Notification access* untuk aplikasi desktop aktif di Windows Settings (klik kanan tray → `Notification Settings`).
- **Bar musik tidak bergerak**: Pastikan audio keluar melalui perangkat output default Windows (*Default Audio Endpoint*) dan pemutar musik tidak dalam status *Paused*.
- **Posisi pill bergeser setelah ganti resolusi/skala DPI**: Klik kanan ikon tray → **`Reset Position`**.
