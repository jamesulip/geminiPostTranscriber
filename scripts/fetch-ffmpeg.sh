#!/usr/bin/env bash
# Downloads a static ffmpeg build for the host target into src-tauri/binaries/
# using Tauri's sidecar naming convention: ffmpeg-<target-triple>[.exe]
set -euo pipefail
cd "$(dirname "$0")/.."

TRIPLE="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
OUT_DIR="src-tauri/binaries"
mkdir -p "$OUT_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

case "$TRIPLE" in
  aarch64-apple-darwin)
    URL="https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip" ;;
  x86_64-apple-darwin)
    URL="https://ffmpeg.martin-riedl.de/redirect/latest/macos/amd64/release/ffmpeg.zip" ;;
  x86_64-unknown-linux-gnu)
    URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz" ;;
  x86_64-pc-windows-msvc)
    URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" ;;
  *) echo "Unsupported target: $TRIPLE" >&2; exit 1 ;;
esac

echo "Downloading ffmpeg for $TRIPLE"
curl -fL "$URL" -o "$TMP/pkg"
case "$URL" in
  *.zip) unzip -q "$TMP/pkg" -d "$TMP/x" ;;
  *.tar.xz) mkdir -p "$TMP/x" && tar -xJf "$TMP/pkg" -C "$TMP/x" ;;
esac

EXT=""; [[ "$TRIPLE" == *windows* ]] && EXT=".exe"
BIN="$(find "$TMP/x" -type f -name "ffmpeg$EXT" | head -n1)"
[ -n "$BIN" ] || { echo "ffmpeg binary not found in archive" >&2; exit 1; }
cp "$BIN" "$OUT_DIR/ffmpeg-$TRIPLE$EXT"
chmod +x "$OUT_DIR/ffmpeg-$TRIPLE$EXT"
echo "Installed $OUT_DIR/ffmpeg-$TRIPLE$EXT"
"$OUT_DIR/ffmpeg-$TRIPLE$EXT" -hide_banner -version | head -n1
