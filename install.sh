#!/usr/bin/env bash
# Build Paperlight, install it for the current user and make it the default PDF viewer.
# Undo: ./install.sh --uninstall
set -euo pipefail
cd "$(dirname "$0")"

PDFIUM_TAG="chromium/8111"
PDFIUM_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_TAG}/pdfium-linux-x64.tgz"
PDFIUM_TGZ_SHA256="bdc60126399f39fe768566a18d4be055a7b3681bc9e248e8a6656b4f6cae0aa2"
PDFIUM_SO_SHA256="dca41ec4a2496ea7378b144cc4bad2d9e050692aaac75b9cc22411c1835f7f6e"
BIN="$HOME/.local/bin/paperlight"
LIB_DIR="$HOME/.local/lib/paperlight"
APPS="$HOME/.local/share/applications"
DESKTOP="$APPS/paperlight.desktop"
ICONS="$HOME/.local/share/icons/hicolor"
SIZES=(16 32 48 64 128 256 512)
FALLBACK="org.gnome.Evince.desktop"

step() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

refresh_caches() {
    update-desktop-database "$APPS" 2>/dev/null || true
    gtk-update-icon-cache -q -t -f "$ICONS" 2>/dev/null || true
}

uninstall() {
    if [[ "$(xdg-mime query default application/pdf)" == paperlight.desktop ]]; then
        if [[ -f "/usr/share/applications/$FALLBACK" ]]; then
            step "Restoring $FALLBACK as the default PDF viewer"
            xdg-mime default "$FALLBACK" application/pdf
        else
            warn "/usr/share/applications/$FALLBACK not found; default PDF viewer left as is"
        fi
    fi
    step "Removing files"
    rm -f "$BIN" "$DESKTOP" "$ICONS/scalable/apps/paperlight.svg"
    for s in "${SIZES[@]}"; do
        rm -f "$ICONS/${s}x${s}/apps/paperlight.png"
    done
    rm -rf "$LIB_DIR"
    refresh_caches
    echo "Removed Paperlight; default PDF viewer: $(xdg-mime query default application/pdf)"
    exit 0
}

case "${1:-}" in
    "") ;;
    --uninstall) uninstall ;;
    *) die "usage: $0 [--uninstall]" ;;
esac

step "Checking prerequisites"
for cmd in cargo curl tar sha256sum xdg-mime pkg-config; do
    command -v "$cmd" >/dev/null || die "missing command: $cmd"
done
pkg-config --exists wayland-client || die "Missing wayland-devel: sudo dnf install wayland-devel"

if [[ -f "$LIB_DIR/libpdfium.so" ]] && echo "$PDFIUM_SO_SHA256  $LIB_DIR/libpdfium.so" | sha256sum -c --quiet - &>/dev/null; then
    step "libpdfium ($PDFIUM_TAG) already installed"
else
    step "Downloading libpdfium ($PDFIUM_TAG)"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL --retry 3 -o "$tmp/pdfium.tgz" "$PDFIUM_URL"
    echo "$PDFIUM_TGZ_SHA256  $tmp/pdfium.tgz" | sha256sum -c --quiet -
    tar -xzf "$tmp/pdfium.tgz" -C "$tmp" lib/libpdfium.so
    echo "$PDFIUM_SO_SHA256  $tmp/lib/libpdfium.so" | sha256sum -c --quiet -
    install -Dm644 "$tmp/lib/libpdfium.so" "$LIB_DIR/libpdfium.so"
fi

step "Building release binary"
cargo build --release --locked
install -Dm755 target/release/paperlight "$BIN"

step "Installing icons"
install -Dm644 assets/icon.svg "$ICONS/scalable/apps/paperlight.svg"
for s in "${SIZES[@]}"; do
    install -Dm644 "assets/icon-$s.png" "$ICONS/${s}x${s}/apps/paperlight.png"
done

step "Installing desktop entry"
mkdir -p "$APPS"
sed "s|^Exec=paperlight |Exec=$BIN |" paperlight.desktop > "$DESKTOP"
chmod 644 "$DESKTOP"
if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate "$DESKTOP" || warn "desktop-file-validate reported problems"
fi

step "Setting Paperlight as the default PDF viewer"
refresh_caches
# xdg-mime writes mimeapps.list there but does not create the directory.
mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}"
xdg-mime default paperlight.desktop application/pdf

[[ "$(xdg-mime query default application/pdf)" == paperlight.desktop ]] \
    || die "xdg-mime did not accept paperlight.desktop as the PDF default"
echo "Installed:"
echo "  $BIN"
echo "  $LIB_DIR/libpdfium.so"
echo "  $DESKTOP"
echo "  $ICONS/{scalable,NxN}/apps/paperlight.{svg,png}"
