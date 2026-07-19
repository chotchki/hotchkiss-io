#!/usr/bin/env bash
# Generate a tiny CBZ fixture for the comic-reader test (Phase DW.10). A CBZ is just
# a zip of page images (no manifest); the cover is the first image by sorted name.
# Two 2x2 PNG "pages" — 001.png sorts first, so it's the extracted cover.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$repo/tests/fixtures/manga/comic-v01.cbz"
tmp="$here/cbz"
rm -rf "$tmp" && mkdir -p "$tmp"
png='iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEUlEQVR4nGP8z8Dwn4EIwDiKAgB5ZwQBIsN9UwAAAABJRU5ErkJggg=='
printf '%s' "$png" | base64 -d > "$tmp/001.png"
printf '%s' "$png" | base64 -d > "$tmp/002.png"
rm -f "$out"
( cd "$tmp" && zip -X9 "$out" 001.png 002.png >/dev/null )
rm -rf "$tmp"
echo "wrote $out"
