#!/usr/bin/env bash
# Phase CP — install the GUI-session Developer ID signer on the deploy Mac.
#
# codesign can't reach the keychain from the non-GUI push-receive hook
# (errSecInternalComponent), so signing is delegated to a LaunchAgent in the
# logged-in GUI session (io.hotchkiss.signer). This installs that agent + writes
# build.env + re-installs the hook, then END-TO-END tests signing through it.
#
# PREREQ (one-time, NOT done here): the Developer ID cert is in the login keychain
# and `security set-key-partition-list -S apple-tool:,apple: -s -k <login-pw>
# ~/Library/Keychains/login.keychain-db` has been run, so the agent signs without
# an interactive prompt. The script verifies the identity is present and bails
# with instructions if not.
#
# Run from a checkout on the DEV machine:
#   bash build/macos/install-signer-agent.sh [mini-host]
set -euo pipefail

HOST="${1:-hotchkiss.io}"
IDENTITY="Developer ID Application: Christopher Hotchkiss (G53N9PU948)"
HERE="$(cd "$(dirname "$0")" && pwd)"

for f in sign-agent.sh io.hotchkiss.signer.plist post-receive; do
  [ -f "$HERE/$f" ] || { echo "missing $HERE/$f (run from a checkout)" >&2; exit 1; }
done

echo "→ staging files on $HOST ..."
ssh "$HOST" 'mkdir -p ~/.config/hotchkiss-io ~/Library/LaunchAgents ~/Library/Logs/io.hotchkiss.web'
scp -q "$HERE/sign-agent.sh"             "$HOST:.config/hotchkiss-io/sign-agent.sh"
scp -q "$HERE/io.hotchkiss.signer.plist" "$HOST:Library/LaunchAgents/io.hotchkiss.signer.plist"
scp -q "$HERE/post-receive"              "$HOST:repos/hotchkiss-io.git/hooks/post-receive"

echo "→ configuring + bootstrapping the signer agent on $HOST ..."
ssh "$HOST" "IDENTITY='$IDENTITY' bash -se" <<'REMOTE'
set -uo pipefail
CFG="$HOME/.config/hotchkiss-io"

# 0. Prereq: the identity must be in the keychain (cert + ACL are a one-time step).
if ! security find-identity -v -p codesigning | grep -q "$IDENTITY"; then
  echo "ERROR: '$IDENTITY' not in codesigning identities." >&2
  echo "Import the cert + run set-key-partition-list first (SETUP.md section 9)." >&2
  exit 1
fi

# 1. build.env: identity only (supersedes the abandoned dedicated-keychain try).
echo "export CODESIGN_IDENTITY=\"$IDENTITY\"" > "$CFG/build.env"
chmod 600 "$CFG/build.env"

# 2. Executable bits + the sign queue dirs.
chmod +x "$CFG/sign-agent.sh" "$HOME/repos/hotchkiss-io.git/hooks/post-receive"
mkdir -p "$CFG/sign/requests" "$CFG/sign/results"

# 3. (Re)bootstrap the signer LaunchAgent into the GUI session.
launchctl bootout "gui/$(id -u)/io.hotchkiss.signer" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/io.hotchkiss.signer.plist"
echo "signer agent bootstrapped into gui/$(id -u)."

# 4. Clean up the abandoned dedicated-keychain experiment (best-effort).
security delete-keychain "$HOME/Library/Keychains/hio-signing.keychain-db" 2>/dev/null || true
rm -f "$CFG/devid-import.p12" 2>/dev/null || true

# 5. End-to-end test: ad-hoc-sign a throwaway .app (like build.sh), hand it to the
#    agent via the queue, and verify it returns Developer-ID-signed — the exact
#    deploy path (request dropped from THIS non-GUI session, signed in the GUI one).
work="$(mktemp -d)"; app="$work/SignerSelfTest.app"
mkdir -p "$app/Contents/MacOS"
cat > "$app/Contents/Info.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>selftest</string></dict></plist>
PL
cp /usr/bin/true "$app/Contents/MacOS/selftest"
codesign --force --sign - "$app/Contents/MacOS/selftest" 2>/dev/null || true

token="selftest.$$"
rm -f "$CFG/sign/results/$token.result"
printf '%s' "$app" > "$CFG/sign/requests/$token.request"   # WatchPaths fires the agent
for i in $(seq 1 30); do
  [ -f "$CFG/sign/results/$token.result" ] && break
  [ $((i % 5)) -eq 0 ] && launchctl kickstart "gui/$(id -u)/io.hotchkiss.signer" 2>/dev/null
  sleep 1
done
res="$(cat "$CFG/sign/results/$token.result" 2>/dev/null || true)"
team="$(codesign -dvv "$app/Contents/MacOS/selftest" 2>&1 | grep TeamIdentifier || true)"
rm -rf "$work" "$CFG/sign/results/$token.result" "$CFG/sign/requests/$token.request"

echo "--- signer self-test ---"
echo "result: ${res:-<TIMEOUT - agent did not respond>}"
echo "${team:-TeamIdentifier=not set}"
case "$res" in
  OK*) [ "$team" = "TeamIdentifier=G53N9PU948" ] && { echo "PASS"; } || { echo "FAIL: signed but wrong/no team"; exit 1; } ;;
  *)   echo "FAIL"; exit 1 ;;
esac
REMOTE

echo
echo "✓ Signer agent installed + self-test PASSED on $HOST."
echo "  Re-deploy (push main → beta, or a v* tag → prod); the build log shows:"
echo "    post-receive: Developer ID re-signed via signer agent"
