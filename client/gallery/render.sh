#!/bin/sh
# Renders every gallery scene to gallery/snapshots/*.png (headless, same renderer as the window).
# Run from anywhere: ./gallery/render.sh [extra rune flags, e.g. --scale 2]
set -e
cd "$(dirname "$0")/.."
cargo build -p thehideout-gallery
BIN=target/debug/thehideout-gallery
OUT=gallery/snapshots
mkdir -p "$OUT"
for mode in dark light; do
  $BIN --scene "foundations-$mode" --size 1200x1800 --snap "$OUT/foundations-$mode.png" "$@"
  $BIN --scene "controls-$mode" --size 1200x1680 --snap "$OUT/controls-$mode.png" "$@"
  $BIN --scene "icons-$mode" --size 1200x2400 --snap "$OUT/icons-$mode.png" "$@"
  $BIN --scene "themes-$mode" --size 1200x1000 --snap "$OUT/themes-$mode.png" "$@"
  $BIN --scene "chat-$mode" --size 1200x4400 --snap "$OUT/chat-$mode.png" "$@"
  $BIN --scene "chat-dialog-$mode" --size 1200x1000 --snap "$OUT/chat-dialog-$mode.png" "$@"
done
