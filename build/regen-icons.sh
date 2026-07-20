#!/usr/bin/env bash
# Regenerate the PWA icon set in assets/images/ from HotchkissLogox1024.png.
# Run from the repo root; requires `sips` (macOS built-in).
#
# - icon-192.png / icon-512.png: downscaled logo, transparent background.
# - apple-touch-icon.png: 180px (iOS Home Screen).
# - icon-maskable-512.png: logo at 80% on navy (#14213D) so circular masks don't clip.

set -euo pipefail

SRC="assets/images/HotchkissLogox1024.png"
OUT="assets/images"
NAVY="14213D"  # matches --color-navy in styles/tailwind.css

sips -z 192 192 "$SRC" --out "$OUT/icon-192.png" >/dev/null
sips -z 512 512 "$SRC" --out "$OUT/icon-512.png" >/dev/null
sips -z 180 180 "$SRC" --out "$OUT/apple-touch-icon.png" >/dev/null

TMP="$(mktemp -t logo-410).png"
sips -z 410 410 "$SRC" --out "$TMP" >/dev/null
sips -p 512 512 --padColor "$NAVY" "$TMP" --out "$OUT/icon-maskable-512.png" >/dev/null
rm -f "$TMP"

echo "regenerated: icon-192, icon-512, icon-maskable-512, apple-touch-icon"

# Beta identity variants (EB.8): color-negated favicon + apple-touch-icon so the
# beta PWA pin is tellable from prod on a home screen. Served host-aware by
# static_content.rs on any non-canonical host. Requires ImageMagick (brew).
magick "$OUT/favicon.ico" -channel RGB -negate "$OUT/favicon-beta.ico"
magick "$OUT/apple-touch-icon.png" -channel RGB -negate "$OUT/apple-touch-icon-beta.png"
