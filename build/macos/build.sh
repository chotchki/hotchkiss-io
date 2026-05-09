#!/bin/bash
# Massive thanks to @dylanwh for the original approach
# https://github.com/dylanwh/lilguy/blob/main/macos/build.sh
#
# Builds a signed Hotchkiss-IO.app for direct deployment on a self-hosted
# Mac. Ad-hoc signed (no Apple Developer ID, no notarization, no .pkg) —
# the binary never leaves machines we control, so spctl --add on the target
# Mac is enough.
#
# Honors CARGO_TARGET_DIR so the post-receive deploy hook can persist
# incremental build artifacts across pushes.
set -euo pipefail

# Resolve VERSION: env override → CI tag → git describe → dev placeholder.
# Always strip a leading 'v' so artifact filenames stay numeric
# (e.g. tag v0.0.43 → 0.0.43).
if [[ -z "${VERSION:-}" ]]; then
  if [[ -n "${GITHUB_REF_NAME:-}" ]]; then
    VERSION="${GITHUB_REF_NAME}"
  elif git_desc=$(git describe --tags --always --dirty 2>/dev/null); then
    VERSION="${git_desc}"
  else
    VERSION="0.0.0-dev"
  fi
fi
VERSION="${VERSION#v}"

EXE="hotchkiss-io"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
APP="$TARGET_DIR/Hotchkiss-IO.app"

rustup target add aarch64-apple-darwin

cargo build --locked --target aarch64-apple-darwin --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$TARGET_DIR/aarch64-apple-darwin/release/$EXE" "$APP/Contents/MacOS/$EXE"
sed -e "s;%VERSION%;$VERSION;g" build/macos/Info.plist > "$APP/Contents/Info.plist"
cp build/macos/HotchkissLogox1024.icns "$APP/Contents/Resources/"

codesign --force --sign - --options runtime "$APP/Contents/MacOS/$EXE"

ABSOLUTE_APP="$(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")"
echo "BUILT_APP=$ABSOLUTE_APP"
