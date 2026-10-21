#!/usr/bin/env bash
# Build a binary .deb for Debian, Ubuntu, and close derivatives.
# The package contains the release binary, a menu entry, and the icon.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v dpkg-deb >/dev/null 2>&1 || ! command -v dpkg-shlibdeps >/dev/null 2>&1; then
    echo "dpkg-deb and dpkg-shlibdeps are required. Install them with:" >&2
    echo "  sudo apt install dpkg-dev" >&2
    exit 1
fi

# fontconfig is linked directly. The X11, Wayland, and GL libraries are loaded
# when the window opens, and their packages are added to Depends below.
if ! pkg-config --exists fontconfig; then
    echo "fontconfig development files are missing. Install them with:" >&2
    echo "  sudo apt install build-essential pkg-config libfontconfig1-dev" >&2
    exit 1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [[ -z "$VERSION" ]]; then
    echo "Could not read the version from Cargo.toml" >&2
    exit 1
fi

if command -v dpkg >/dev/null 2>&1; then
    ARCH="$(dpkg --print-architecture)"
else
    case "$(uname -m)" in
        x86_64) ARCH="amd64" ;;
        aarch64) ARCH="arm64" ;;
        *)
            echo "Unsupported architecture: $(uname -m)" >&2
            exit 1
            ;;
    esac
fi

echo "Building gluco ${VERSION} for ${ARCH}"
if grep -q '^\[\[package\]\]' Cargo.lock 2>/dev/null && cargo fetch --locked; then
    cargo build --release --locked
else
    echo "Cargo.lock in this checkout cannot be used. Cargo will write a new one."
    rm -f Cargo.lock
    cargo build --release
fi
bash packaging/icons/write-icons.sh

STAGE="$(mktemp -d)"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

install -D -m 0755 target/release/gluco "$STAGE/usr/bin/gluco"
install -D -m 0644 packaging/gluco.desktop "$STAGE/usr/share/applications/gluco.desktop"
for size in 48 64 128 256; do
    install -D -m 0644 "packaging/icons/${size}x${size}/gluco.png" \
        "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/gluco.png"
done
install -D -m 0644 packaging/debian/copyright "$STAGE/usr/share/doc/gluco/copyright"
install -D -m 0644 packaging/debian/README.Debian "$STAGE/usr/share/doc/gluco/README.Debian"
install -D -m 0644 LICENSE "$STAGE/usr/share/doc/gluco/LICENSE"
gzip -n -9 -c packaging/debian/README.Debian > "$STAGE/usr/share/doc/gluco/README.Debian.gz"
rm -f "$STAGE/usr/share/doc/gluco/README.Debian"

# dpkg-shlibdeps reads debian/control from the working directory. Give it a stub.
SHLIB_DIR="$(mktemp -d)"
mkdir -p "$SHLIB_DIR/debian"
printf 'Source: gluco\nPackage: gluco\n' > "$SHLIB_DIR/debian/control"
SHAREDLIBS="$(
    cd "$SHLIB_DIR"
    dpkg-shlibdeps -O -e "$STAGE/usr/bin/gluco" | sed -n 's/^shlibs:Depends=//p'
)"
rm -rf "$SHLIB_DIR"
if [[ -z "$SHAREDLIBS" ]]; then
    echo "dpkg-shlibdeps did not report shared library dependencies" >&2
    exit 1
fi
# The window opens these with dlopen, so they never appear in the NEEDED list.
# ca-certificates is required for the LibreLinkUp HTTPS calls.
RUNTIME_DEPENDS="ca-certificates, libegl1, libgl1, libwayland-client0, libwayland-egl1, libx11-6, libx11-xcb1, libxcursor1, libxi6, libxkbcommon-x11-0, libxkbcommon0, libxrender1"
DEPENDS="${SHAREDLIBS}, ${RUNTIME_DEPENDS}"

mkdir -p "$STAGE/DEBIAN"
INSTALLED_SIZE="$(du -sk "$STAGE/usr" | cut -f1)"
python3 - "$VERSION" "$ARCH" "$DEPENDS" "$INSTALLED_SIZE" "$STAGE/DEBIAN/control" << 'PY'
import pathlib, sys
version, arch, depends, size, dest = sys.argv[1:]
text = pathlib.Path("packaging/debian/control.in").read_text()
text = (
    text.replace("@VERSION@", version)
    .replace("@ARCH@", arch)
    .replace("@DEPENDS@", depends)
)
text += f"Installed-Size: {size}\n"
pathlib.Path(dest).write_text(text)
PY
install -m 0755 packaging/debian/postinst "$STAGE/DEBIAN/postinst"
install -m 0755 packaging/debian/postrm "$STAGE/DEBIAN/postrm"

mkdir -p dist
OUT="$ROOT/dist/gluco_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$STAGE" "$OUT"
(
    cd dist
    sha256sum "$(basename "$OUT")" > SHA256SUMS
)
echo "Wrote $OUT"
dpkg-deb --info "$OUT"
