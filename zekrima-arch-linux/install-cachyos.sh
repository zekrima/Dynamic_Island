#!/usr/bin/env bash
set -euo pipefail

echo "==> Installing build & runtime dependencies for Zekrima on CachyOS / Arch Linux..."
sudo pacman -S --needed --noconfirm \
  base-devel rust cargo pkgconf \
  gtk4 gtk4-layer-shell pango cairo gdk-pixbuf2 \
  playerctl pipewire-pulse wireplumber dbus curl

echo "==> Building and installing Zekrima Arch package via makepkg..."
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
makepkg -si --noconfirm

echo ""
echo "==> Selesai! Zekrima sudah terinstall di /usr/bin/zekrima"
echo "    Jalankan dengan: zekrima &"
echo "    (Klik kanan pada pill untuk mengaktifkan 'Run at Login')"
