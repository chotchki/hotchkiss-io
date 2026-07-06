#!/usr/bin/env bash
# Regenerate the greylist toll image (Phase CX) from a source art file.
#
# The bot-challenge interstitial paints this image pixel-by-pixel AS the proof-of-work:
# its pixel count IS the hash-iteration count, so its dimensions ARE the difficulty knob
# (see docs/greylist-challenge-design.md — 320 high × ~570 wide ≈ 182k iterations, a
# ~1s solve). The server decodes THIS committed PNG at boot, forces every pixel fully
# opaque, and hashes + ships the raw RGBA — so this script only has to resize to the
# target height; alpha and color-profile normalization are the server's job, not this
# script's. Rerun it whenever the art changes, then commit assets/greylist/toll.png.
#
# Usage: build/greylist/make-toll-image.sh <source-image> [target-height]
#   target-height defaults to 320.
#
# Uses `sips` (built into macOS — this repo is macOS-only). ImageMagick equivalent:
#   magick "$SRC" -resize "x${H}" -strip "$OUT"

set -euo pipefail

SRC="${1:?usage: make-toll-image.sh <source-image> [target-height]}"
H="${2:-320}"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$REPO_ROOT/assets/greylist/toll.png"

mkdir -p "$(dirname "$OUT")"

# --resampleHeight fixes the height and scales width proportionally; -s format png emits
# a lossless PNG (the server re-decodes it deterministically, so lossless in matters).
sips --resampleHeight "$H" -s format png "$SRC" --out "$OUT" >/dev/null

W="$(sips -g pixelWidth "$OUT" | awk '/pixelWidth/{print $2}')"
HH="$(sips -g pixelHeight "$OUT" | awk '/pixelHeight/{print $2}')"
echo "wrote $OUT (${W}x${HH}, ~$(( W * HH / 1000 ))k pixels = ~$(( W * HH / 1000 ))k hash iterations)"
