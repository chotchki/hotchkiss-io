# Plan archive

Completed phases, swept here from `PLAN.md` per the workflow rule (a phase exits when every box is ticked, e2e passes, docs updated → summarize → sweep). Newest first.

---

## Phase 4 — Bump `tray-wrapper` to 0.4.1 — DONE 2026-05-09

**Summary:** the user's upstream fixes to `tray-wrapper` landed as 0.4.1 (published 2026-05-09). Caret `^0.3.1` wouldn't accept it, so `Cargo.toml` was bumped to `"0.4.1"`, `cargo update -p tray-wrapper` re-locked it (and incidentally re-resolved the patched `cookie` fork to a newer commit on its branch — no behavior change). The 0.4 API turned out to be source-compatible — no call-site changes. `cargo build` / `clippy --all-targets` / `test` (19/19) all clean. Deployed to production as part of Phase 0.6.1's first automated push (commit `cde6085`); the running process came up clean against 0.4.1 (PID 76312, site serving), which validated the upgrade end-to-end. A visual tray-icon spot-check at the mini console is left to the user — the deploy only proves the process tree didn't break.

Per-task detail (all `[x]`): 4.1 version determined (0.4.1, manifest edit required) · 4.2 `Cargo.toml:112` bumped · 4.3 `cargo update -p tray-wrapper` · 4.4 build + clippy clean · 4.5 tests 19/19 · 4.6 validated via the 0.6.1 deploy · 4.7 shipped via `git push origin main`.

---

## Phase 0 — Direct push-to-deploy on the Mac mini — DONE 2026-05-10

**Goal (achieved):** replaced the tag-triggered `release.yml` (Developer ID signing + notarization on a hosted `macos-14` runner) → `install.yml` (download `.pkg`, `installer -target /` on a self-hosted runner) flow with a single `git push origin main` to a bare repo on the Mac mini, whose `post-receive` hook does `cargo build` → ad-hoc `codesign` → atomic `.app` swap into `/Applications` → `launchctl kickstart -k`. This eliminated Apple notarization, the Developer ID cert, the temp-keychain dance, the `.pkg` machinery, the hosted runner, the self-hosted runner, and both workflow files.

**Key decisions (kept for the record):**
- Tray icon stays — `tray-wrapper` is the up/down visual signal; running as a LaunchAgent in the user GUI session preserves it.
- **Sandbox dropped** (`com.apple.security.app-sandbox` removed). Eliminates `~/Library/Containers/.../Data/...` path translation; files now land in standard macOS locations. Defense-in-depth loss is marginal — the secrets worth stealing (Cloudflare token, ACME key, session-signing key) all live where the app must read them anyway, so the sandbox didn't compartmentalize them.
- Notarization unnecessary — the binary never leaves machines we control; ad-hoc signing is enough. (`spctl --add` wasn't even needed: launchd execs the binary directly via `ProgramArguments`, which doesn't trigger Gatekeeper.)
- Privileged port binding works without root because macOS Mojave+ allows non-root binds to ports <1024 when binding `INADDR_ANY` (axum's default). No `pf` redirect.
- No config CLI arg in the plist — `Settings::load` uses `NSHomeDirectory()`, which post-sandbox-removal returns the real `/Users/chotchki`, joining to the standard config path under `Library/Application Support`.
- `github` is a mirror remote; `origin` is the mini. `test_and_coverage.yml` keeps running on push for CI signal — informational, doesn't gate deploy.

**What shipped:**
- `build/macos/build.sh` trimmed from ~77 lines to ~30 — dropped `pkgbuild`/`productbuild`/`productsign`/`notarytool`/`stapler` and the four required Apple env vars; now ad-hoc-signs and prints `BUILT_APP=<abs path>`. Honors `CARGO_TARGET_DIR`.
- `build/macos/post-receive` — the deploy hook. Filters `refs/heads/main`, `git archive`s the pushed tree into `~/.cache/hotchkiss-io-build/src` (wiped per run), builds with `CARGO_TARGET_DIR=~/.cache/hotchkiss-io-build/target` (so incremental artifacts persist: cold ≈ 1m53s → warm ≈ 17–20s), atomic-ish swaps the `.app` (`mv` current → `.prev`, `mv` new in, `launchctl kickstart -k`, drop `.prev`), and bails before touching `/Applications` if the build fails. Sets `PATH` explicitly because sshd hands hooks a stripped env.
- `build/macos/io.hotchkiss.web.plist` — LaunchAgent: `Label=io.hotchkiss.web`, `ProgramArguments=[/Applications/Hotchkiss-IO.app/Contents/MacOS/hotchkiss-io]`, `RunAtLoad`, `KeepAlive`, `ThrottleInterval=10`, logs under `~/Library/Logs/io.hotchkiss.web/`. No `RootDirectory`.
- `build/macos/SETUP.md` — reproducible one-time mini bootstrap (toolchain, dirs, config, LaunchAgent, bare-repo init, dev-side `git remote set-url`, first-deploy, verification).
- `src/settings.rs` — `RawSettings` (private serde target) with `Option<String>` path fields; public `Settings` typed `PathBuf`; `Settings::resolve` fills omitted paths with `~/Library/Application Support/io.hotchkiss.web/data/database.sqlite`, `~/Library/Logs/io.hotchkiss.web`, `~/Library/Caches/io.hotchkiss.web`. Required fields shrank to `cloudflare_token` + `domain`. `make_config_path` now points at `~/Library/Application Support/io.hotchkiss.web/config.json`.
- `src/db/database_handle.rs` — `DatabaseHandle::create` takes `&Path` (was `&str`), uses `SqliteConnectOptions::new().filename(path)`.
- Deleted: `build/macos/entitlements.plist`, `build/macos/pkgbuild.plist`, `build/macos/Resources/`, `.github/workflows/release.yml`, `.github/workflows/install.yml`.
- Bare repo created at `~/repos/hotchkiss-io.git` on the mini (chosen over the stale 2025-era worktree that was sitting at `~/hotchkiss-io/repo`); dev `origin` repointed to `ssh://hotchkiss.io/Users/chotchki/repos/hotchkiss-io.git`.
- Mini migration: prod SQLite (`database.sqlite` + `-wal` + `-shm`) moved from the old sandbox container into the new standard path; old container (191 MB) and the root-owned PKG-installed `Hotchkiss-IO.app.prev` deleted; old self-hosted runner (`~/hotchkiss-io-runner/`) stopped, unregistered, removed.
- Docs: CLAUDE.md "Common commands" / release paragraph / "Configuration" / "Things to watch out for" all updated for the new flow + dropped sandbox; SPEC.md "Current site's pain" marks deployment-fragility solved.
- GitHub repo secrets deleted (all unused after the workflow removals): `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`, `MACOS_CERT_IDENTITY`, `KEYCHAIN_PASSWORD`, `KEYCHAIN`, `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID`. Only `CODECOV_TOKEN` remains.

**Validation:** 5 consecutive clean push-to-deploys (`cde6085`, `9978288`, `ed24ee3`, `d46c85d`, `8e5cfb5`) plus the 0.9.5 sweep push as a 6th. Two deliberate failure-path probes both handled gracefully: (a) the very first push hit a swap abort on the root-owned `.prev` — `set -e` stopped the hook before it touched `/Applications`, production kept serving; (b) an intentional `let _: () = ;` syntax error (commit `9174472`, reverted in `c9cee7e`) made the build fail — hook bailed pre-swap, prod PID unchanged. Also: killing the running PID confirmed `KeepAlive` respawns it past `ThrottleInterval`. `git push` exits 0 even when `post-receive` fails (standard git semantics — push status reflects the ref update, not the hook); the streamed compiler output is loud enough to notice, but if hard-fail-on-origin is ever wanted, a `pre-receive` hook would be the lever. `test_and_coverage.yml` green on the final push; `check_ip.yml` is schedule-triggered (not push) and informational.

**Deferred follow-ups (live in `PLAN.md`):** Phase 3 (swap `ifconfig.me` → Cloudflare `cdn-cgi/trace`) is now unblocked. Phase 5 (retire the `cookie-rs` fork via serde remote-derive) was opened during this work. `0.4.7` (`spctl --add`) was deliberately not done — launchd execs the binary directly so Gatekeeper isn't in the path; would only be needed if some future code opens the bundle via LaunchServices.
