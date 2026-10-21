#!/usr/bin/env bash
# Install Gluco on Fedora.
# Builds the release binary from this source tree and installs it for all users.
set -euo pipefail

usage() {
    cat << 'EOF'
Usage: packaging/fedora/install.sh [--uninstall]

Installs the compiler and GUI libraries with dnf, builds Gluco, and copies
it to /usr/local. Run it as your normal user; it asks for sudo when it
changes the system.

  --uninstall   Remove the installed binary, menu entry, and icon.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

if [[ ! -r /etc/os-release ]]; then
    echo "This installer is for Fedora." >&2
    exit 1
fi
# shellcheck disable=SC1091
source /etc/os-release
if [[ "${ID:-}" != "fedora" && "${ID_LIKE:-}" != *fedora* ]]; then
    echo "This installer is for Fedora (detected ID=${ID:-unknown})." >&2
    echo "On Debian, Ubuntu, and their derivatives, install dist/gluco_*_amd64.deb instead." >&2
    exit 1
fi

if [[ "$(id -u)" -eq 0 ]]; then
    echo "Run this as your normal user. It will ask for sudo when it needs it." >&2
    exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

PREFIX="${PREFIX:-/usr/local}"
BIN_PATH="$PREFIX/bin/gluco"
DESKTOP_PATH="$PREFIX/share/applications/gluco.desktop"

remove_installed() {
    sudo rm -f "$BIN_PATH" "$DESKTOP_PATH"
    sudo rm -f "$PREFIX/share/icons/hicolor/"*"/apps/gluco.png"
    if command -v update-desktop-database >/dev/null 2>&1; then
        sudo update-desktop-database -q "$PREFIX/share/applications" || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        sudo gtk-update-icon-cache -q "$PREFIX/share/icons/hicolor" || true
    fi
}

if [[ "${1:-}" == "--uninstall" ]]; then
    remove_installed
    echo "Removed Gluco from $PREFIX"
    exit 0
fi
if [[ $# -gt 0 ]]; then
    usage >&2
    exit 1
fi

sudo dnf install -y \
    gcc \
    pkgconf-pkg-config \
    fontconfig-devel \
    freetype-devel \
    libxkbcommon-devel \
    libxkbcommon-x11-devel \
    libxcb-devel \
    libX11-devel \
    libXcursor-devel \
    libXi-devel \
    libXrender-devel \
    mesa-libGL-devel \
    mesa-libEGL-devel \
    wayland-devel \
    fontconfig \
    freetype \
    libxkbcommon \
    libxkbcommon-x11 \
    libxcb \
    libX11 \
    libX11-xcb \
    libXcursor \
    libXi \
    libXrender \
    mesa-libGL \
    mesa-libEGL \
    libwayland-client \
    libwayland-egl \
    ca-certificates \
    cargo \
    rust

# rust-toolchain.toml asks for Rust 1.92 or newer. Fedora's packaged compiler
# is sometimes older; rustup supplies a current stable toolchain for the build.
rustc_is_new_enough() {
    command -v rustc >/dev/null 2>&1 || return 1
    local version
    version="$(rustc --version | awk '{print $2}')"
    [[ "$(printf '%s\n%s\n' "1.92.0" "$version" | sort -V | head -n 1)" == "1.92.0" ]]
}

if ! rustc_is_new_enough; then
    if [[ ! -x "$HOME/.cargo/bin/rustup" ]]; then
        curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    fi
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
    rustup toolchain install stable
    rustup default stable
fi

if ! rustc_is_new_enough; then
    echo "Rust 1.92 or newer is required. rustc is: $(command -v rustc) ($(rustc --version 2>/dev/null || echo missing))" >&2
    exit 1
fi

# Inter is the face the window asks for. The package name varies by release.
sudo dnf install -y rsms-inter-fonts || sudo dnf install -y inter-fonts || true

# The copy on GitHub may be a header only, or it may list packages whose
# checksums do not match the crates. --locked refuses both. A fetch that
# succeeds means this lock file is safe to build with.
if grep -q '^\[\[package\]\]' Cargo.lock 2>/dev/null && cargo fetch --locked; then
    cargo build --release --locked
else
    echo "Cargo.lock in this checkout cannot be used. Cargo will write a new one."
    rm -f Cargo.lock
    cargo build --release
fi
bash packaging/icons/write-icons.sh
sudo install -D -m 0755 target/release/gluco "$BIN_PATH"
sudo install -D -m 0644 packaging/gluco.desktop "$DESKTOP_PATH"
for size in 48 64 128 256; do
    sudo install -D -m 0644 "packaging/icons/${size}x${size}/gluco.png" \
        "$PREFIX/share/icons/hicolor/${size}x${size}/apps/gluco.png"
done

if command -v update-desktop-database >/dev/null 2>&1; then
    sudo update-desktop-database -q "$PREFIX/share/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    sudo gtk-update-icon-cache -q "$PREFIX/share/icons/hicolor" || true
fi

echo "Installed $BIN_PATH"
echo "Open Gluco from the application menu, or run: gluco"
echo "Remove it later with: packaging/fedora/install.sh --uninstall"
