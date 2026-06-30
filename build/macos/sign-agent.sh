#!/bin/bash
# Phase CP — Developer ID signer agent.
#
# Runs in the GUI/Aqua session (LaunchAgent io.hotchkiss.signer, WatchPaths on the
# requests dir) — the ONE context where codesign can reach the login-keychain
# signing key. The push-receive hook runs in a non-GUI sshd session where codesign
# fails with errSecInternalComponent (a non-interactive session can't unlock the
# key, and `launchctl asuser` to hop into the GUI session needs root). So the hook
# drops a <token>.request file naming the app to sign; this agent signs it and
# writes a <token>.result the hook polls.
#
# Identity comes from the same off-repo config the hook reads
# (~/.config/hotchkiss-io/build.env: export CODESIGN_IDENTITY="Developer ID …").
# The login keychain must already have the cert + `security set-key-partition-list`
# run (one-time, see SETUP.md §9) so this signs without an interactive prompt.
set -uo pipefail

CFG="$HOME/.config/hotchkiss-io"
REQ_DIR="$CFG/sign/requests"
RES_DIR="$CFG/sign/results"
mkdir -p "$REQ_DIR" "$RES_DIR"

ID=""
# shellcheck disable=SC1091
[ -f "$CFG/build.env" ] && . "$CFG/build.env"
ID="${CODESIGN_IDENTITY:-}"

shopt -s nullglob
# Drain the queue, re-scanning a few times so a request that lands mid-run (a
# second deploy) still gets signed without waiting for its own WatchPaths fire.
for _ in 1 2 3 4 5; do
  reqs=("$REQ_DIR"/*.request)
  [ ${#reqs[@]} -eq 0 ] && break
  for req in "${reqs[@]}"; do
    token="$(basename "$req" .request)"
    app="$(cat "$req" 2>/dev/null)"
    res="$RES_DIR/$token.result"
    rm -f "$req"                      # claim it first so a re-fire can't double-sign

    if [ -z "$ID" ]; then
      printf 'FAIL: no CODESIGN_IDENTITY in build.env\n' > "$res"; continue
    fi
    if [ -z "$app" ] || [ ! -d "$app" ]; then
      printf 'FAIL: no app bundle at %s\n' "$app" > "$res"; continue
    fi
    exe_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist" 2>/dev/null)"
    exe="$app/Contents/MacOS/$exe_name"
    if [ -z "$exe_name" ] || [ ! -f "$exe" ]; then
      printf 'FAIL: no executable in %s\n' "$app" > "$res"; continue
    fi

    err="$RES_DIR/$token.err"
    if codesign --force --sign "$ID" --options runtime "$exe" 2>"$err"; then
      printf 'OK %s\n' "$ID" > "$res"
    else
      printf 'FAIL: codesign: %s\n' "$(tr '\n' ' ' < "$err" 2>/dev/null)" > "$res"
    fi
    rm -f "$err"
  done
done
