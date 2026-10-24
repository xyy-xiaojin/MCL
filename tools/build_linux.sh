#!/usr/bin/env bash
# MCL on Linux: one-shot build.
#
# Produces ./MCL (release binary, frontend embedded) and, if the Tauri CLI
# works, a .deb under src-tauri/target/release/bundle/.
#
# Usage:
#   bash tools/build_linux.sh            # release binary only
#   bash tools/build_linux.sh --bundle   # also build the .deb package
#
# Notes:
#   - No Smart App Control on Linux, so plain `cargo build --release` works.
#   - Do NOT drop the custom-protocol feature: without it the binary tries to
#     load http://localhost:1420 and shows a blank window.
set -euo pipefail

cd "$(dirname "$0")/.."

BUNDLE=0
for arg in "$@"; do
  case "$arg" in
    --bundle) BUNDLE=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing command: $1" >&2
    echo "see docs/BUILD-LINUX.md for the dependency list" >&2
    exit 1
  }
}

echo "==> checking toolchain"
need cargo
need node
need npm

# webkit2gtk-4.1 is what Tauri 2 links against; 4.0 (Tauri 1) is not enough.
if command -v pkg-config >/dev/null 2>&1; then
  if ! pkg-config --exists webkit2gtk-4.1; then
    echo "!! webkit2gtk-4.1 not found." >&2
    echo "   Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-dev" >&2
    echo "   Fedora:        sudo dnf install webkit2gtk4.1-devel" >&2
    echo "   Arch:          sudo pacman -S webkit2gtk-4.1" >&2
    exit 1
  fi
fi

echo "==> building frontend (vue-tsc + vite)"
npm ci
npm run build

echo "==> building backend (release)"
# Cargo.toml lives in src-tauri/, not at the repo root.
# Release keeps the default optimizations; there is no build-script blocking
# here (Smart App Control is Windows-only).
cargo build --release --features custom-protocol -j 1 \
  --manifest-path src-tauri/Cargo.toml

cp src-tauri/target/release/mcl-tauri ./MCL
chmod +x ./MCL
echo "==> binary: $(pwd)/MCL ($(du -h ./MCL | cut -f1))"

# Sanity check: the frontend must be embedded, otherwise the window is blank.
if grep -aq 'index-' ./MCL; then
  echo "==> frontend assets embedded: ok"
else
  echo "!! frontend assets missing from the binary (blank window expected)" >&2
  exit 1
fi

if [ "$BUNDLE" -eq 1 ]; then
  echo "==> building .deb"
  # Reuses the release artifacts from the step above; the CLI only adds packaging.
  npm run tauri build -- --bundles deb
  ls -1 src-tauri/target/release/bundle/deb/ 2>/dev/null || true
fi

echo "done. run ./MCL"
