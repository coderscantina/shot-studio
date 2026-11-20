#!/usr/bin/env bash
# Regenerates packaging/macos/AppIcon.icns from packaging/macos/AppIcon.svg. The SVG holds the
# logo on the macOS icon grid: a 824 px rounded square inset 100 px on a 1024 px canvas.
# Needs rsvg-convert (`brew install librsvg`) and iconutil, which ships with macOS.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

SRC=packaging/macos/AppIcon.svg
OUT=packaging/macos/AppIcon.icns

iconset=$(mktemp -d)/AppIcon.iconset
mkdir -p "$iconset"
trap 'rm -rf "$(dirname "$iconset")"' EXIT

for size in 16 32 128 256 512; do
  rsvg-convert -w "$size" -h "$size" "$SRC" -o "$iconset/icon_${size}x${size}.png"
  rsvg-convert -w $((size * 2)) -h $((size * 2)) "$SRC" -o "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o "$OUT"
