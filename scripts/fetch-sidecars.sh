#!/usr/bin/env bash
# Downloads the yt-dlp and ffmpeg binaries Lynk bundles, for one target, into
# src-tauri/binaries/ with the target-triple names Tauri expects.
#
#   bash scripts/fetch-sidecars.sh [target-triple]   (default: this machine)
#
# Versions are pinned and every download is checked against its SHA-256.
# The app keeps yt-dlp current by itself after install.
set -euo pipefail

TARGET="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/src-tauri/binaries"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$OUT"

YTDLP_VERSION="2026.08.19"
YTDLP_URL="https://github.com/yt-dlp/yt-dlp/releases/download/$YTDLP_VERSION"

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

fetch() { # url dest expected-sha256
  echo "↓ $1"
  curl -fL --retry 3 --silent --show-error -o "$2" "$1"
  local got; got="$(sha256 "$2")"
  if [ "$got" != "$3" ]; then
    echo "✗ checksum mismatch for $1" >&2
    echo "  expected $3" >&2
    echo "  got      $got" >&2
    exit 1
  fi
}

case "$TARGET" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    fetch "$YTDLP_URL/yt-dlp_macos" "$OUT/yt-dlp-$TARGET" \
      0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202
    if [ "$TARGET" = aarch64-apple-darwin ]; then
      fetch "https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip" "$TMP/ffmpeg.zip" \
        c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924
    else
      fetch "https://ffmpeg.martin-riedl.de/download/macos/amd64/1789931006_9.0.2/ffmpeg.zip" "$TMP/ffmpeg.zip" \
        7c6b4125b191cbf773832dc51f424cf2b6bb7da43007d1e066f95909e47cacd4
    fi
    unzip -q -o "$TMP/ffmpeg.zip" -d "$TMP/ffmpeg"
    cp "$(find "$TMP/ffmpeg" -type f -name ffmpeg | head -1)" "$OUT/ffmpeg-$TARGET"
    chmod +x "$OUT/yt-dlp-$TARGET" "$OUT/ffmpeg-$TARGET"
    xattr -c "$OUT/yt-dlp-$TARGET" "$OUT/ffmpeg-$TARGET" 2>/dev/null || true
    ;;
  x86_64-pc-windows-msvc)
    fetch "$YTDLP_URL/yt-dlp.exe" "$OUT/yt-dlp-$TARGET.exe" \
      66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a
    fetch "https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip" "$TMP/ffmpeg.zip" \
      60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba
    unzip -q -o "$TMP/ffmpeg.zip" -d "$TMP/ffmpeg"
    cp "$(find "$TMP/ffmpeg" -type f -name ffmpeg.exe | head -1)" "$OUT/ffmpeg-$TARGET.exe"
    ;;
  *)
    echo "Unsupported target: $TARGET" >&2
    exit 1
    ;;
esac

echo "✓ sidecars for $TARGET:"
ls -lh "$OUT" | grep -- "$TARGET"
