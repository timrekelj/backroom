#!/bin/sh
# Renders every desktop scene to desktop/snapshots/*.png (headless, same renderer as the window,
# fake backend; `--at 1` lets the scroll-edge lines finish their fade). Run from anywhere: ./desktop/render.sh [extra rune flags, e.g. --scale 2]
set -e
cd "$(dirname "$0")/.."
cargo build -p thehideout-desktop
BIN=target/debug/thehideout-desktop
OUT=desktop/snapshots
mkdir -p "$OUT"
for mode in "" "-light"; do
  for scene in emoji-picker emoji-autocomplete reactions jumbo-emoji server-settings-emojis channel-icon-emoji-tabs; do
    $BIN --scene "$scene$mode" --size 1280x800 --at 1 --snap "$OUT/$scene$mode.png" "$@"
  done
  for scene in auth-login auth-signup-error main main-members home-dms home-empty toast empty-channel dialog-create-server reconnecting sections channel-settings icon-picker-search reorder-menu main-avatars server-settings-overview server-settings-roles server-settings-roles-limited server-settings-members profile-dialog admin-overview admin-servers admin-files admin-files-delete admin-users admin-audit admin-unavailable; do
    $BIN --scene "$scene$mode" --size 1280x800 --at 1 --snap "$OUT/$scene$mode.png" "$@"
  done
  for scene in attachments-composer attachments-messages attachments-images lightbox video-embed; do
    $BIN --scene "$scene$mode" --size 1280x800 --at 1 --snap "$OUT/$scene$mode.png" "$@"
  done
  # Embeds are tall: a taller window shows whole cards.
  for scene in embeds embeds-gallery embeds-collapsed; do
    $BIN --scene "$scene$mode" --size 1280x1300 --at 1 --snap "$OUT/$scene$mode.png" "$@"
  done
  $BIN --scene "main-small$mode" --size 800x600 --at 1 --snap "$OUT/main-small$mode.png" "$@"
done
# The Appearance page is tall with Density and Text size: a taller window shows all of it.
for mode in "" "-light"; do
  $BIN --scene "settings-appearance$mode" --size 1280x1250 --at 1 --snap "$OUT/settings-appearance$mode.png" "$@"
done
# Non-default looks, for review: accent x background x tint (see scenes.rs).
for scene in emoji-picker-ocean channel-settings-ocean main-compact main-compact-light main-text-125 main-text-125-light main-ocean-cool main-coral-paper-light main-graphite-deep; do
  $BIN --scene "$scene" --size 1280x800 --at 1 --snap "$OUT/$scene.png" "$@"
done
