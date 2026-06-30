#!/bin/bash
# Massive thanks to @dylanwh for the original approach
# https://github.com/dylanwh/lilguy/blob/main/macos/build.sh
#
# Builds an ad-hoc-signed Hotchkiss-IO[-Beta].app for direct deployment on a
# self-hosted Mac (no notarization / .pkg — the binary never leaves machines we
# control). The DEPLOY then RE-SIGNS the app with a stable Developer ID via a
# GUI-session signer agent (post-receive -> io.hotchkiss.signer -> sign-agent.sh,
# Phase CP) so the macOS Full Disk Access (TCC) grant survives deploys instead of
# dropping on every ad-hoc cdhash change. Signing is delegated there because
# codesign can't reach the keychain from the headless push hook
# (errSecInternalComponent — a non-GUI session can't unlock the signing key).
#
# Honors CARGO_TARGET_DIR so the post-receive deploy hook can persist
# incremental build artifacts across pushes.
set -euo pipefail

# Profile selection: --profile beta|prod (default prod). Determines the app
# bundle name + identifier so the beta and prod .apps coexist in /Applications
# and register as distinct LaunchServices entries. The install path and
# LaunchAgent label are the post-receive hook's concern, not build.sh's.
PROFILE="prod"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      [[ $# -ge 2 ]] || { echo "build.sh: --profile requires a value ('beta' or 'prod')" >&2; exit 2; }
      PROFILE="$2"; shift 2 ;;
    --profile=*) PROFILE="${1#*=}"; shift ;;
    *) echo "build.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$PROFILE" in
  prod)
    BUNDLE_NAME="Hotchkiss-IO"
    BUNDLE_ID="io.hotchkiss.web"
    APP_BASENAME="Hotchkiss-IO.app"
    ;;
  beta)
    BUNDLE_NAME="Hotchkiss-IO Beta"
    BUNDLE_ID="io.hotchkiss.web.beta"
    APP_BASENAME="Hotchkiss-IO-Beta.app"
    ;;
  *)
    echo "build.sh: --profile must be 'beta' or 'prod', got '$PROFILE'" >&2
    exit 2
    ;;
esac

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
APP="$TARGET_DIR/$APP_BASENAME"

rustup target add aarch64-apple-darwin

cargo build --locked --target aarch64-apple-darwin --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$TARGET_DIR/aarch64-apple-darwin/release/$EXE" "$APP/Contents/MacOS/$EXE"
sed -e "s;%VERSION%;$VERSION;g" \
    -e "s;%BUNDLE_NAME%;$BUNDLE_NAME;g" \
    -e "s;%BUNDLE_ID%;$BUNDLE_ID;g" \
    build/macos/Info.plist > "$APP/Contents/Info.plist"
cp build/macos/HotchkissLogox1024.icns "$APP/Contents/Resources/"

# Ad-hoc sign so the binary RUNS (arm64 requires at least an ad-hoc signature).
# The deploy then re-signs with a stable Developer ID via the GUI-session signer
# agent — codesign can't reach the keychain from the headless push hook, and a
# stable identity is what keeps the Full Disk Access (TCC) grant alive across
# deploys (Phase CP). A local/dev build just stays ad-hoc, which is fine.
codesign --force --sign - --options runtime "$APP/Contents/MacOS/$EXE"

ABSOLUTE_APP="$(cd "$(dirname "$APP")" && pwd)/$(basename "$APP")"
echo "BUILT_APP=$ABSOLUTE_APP"
echo "PROFILE=$PROFILE"
