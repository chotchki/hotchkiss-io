# Plan

## Phase 0 — Direct push-to-deploy on the Mac mini

**Goal:** replace the tag-triggered `release.yml` (Developer ID signing + notarization on hosted macos-14) → `install.yml` (download `.pkg` on self-hosted) flow with a single `git push mini main` to a bare repo on the mini, whose `post-receive` hook does cargo build → ad-hoc codesign → atomic `.app` swap into `/Applications` → `launchctl kickstart`. Eliminates Apple notarization, Developer ID, the keychain dance, the `.pkg` machinery, the macos-14 hosted runner, and `install.yml`.

**Key decisions (decided in design discussion, kept for context):**
- Tray icon stays — `tray-wrapper` is the up/down visual signal. LaunchAgent in the user GUI session preserves it.
- **Sandbox dropped.** Removing `com.apple.security.app-sandbox` from entitlements eliminates container path translation and lets files land in standard macOS locations (`~/Library/Application Support/io.hotchkiss.web/`, `~/Library/Logs/io.hotchkiss.web/`, `~/Library/Caches/io.hotchkiss.web/`). Defense-in-depth lost is marginal for this threat model — the secrets worth stealing (Cloudflare token, ACME key, session signing key) all live in places the app must access for normal operation, so sandbox doesn't compartmentalize them. Trade is deliberate.
- Notarization is unnecessary since the binary never leaves machines we control. `spctl --add` whitelists the ad-hoc-signed bundle once.
- Privileged port binding works without root because macOS Mojave+ allows non-root binds to ports <1024 when binding `INADDR_ANY` (axum default). No `pf` redirect needed.
- No config CLI arg in the plist — `Settings::load` uses `NSHomeDirectory()` which (post-sandbox-removal) returns `/Users/chotchki`, joining to the new standard config path under `Library/Application Support`.
- GitHub stays as origin/backup; `mini` is just an additional remote. `test_and_coverage.yml` keeps running on push for CI signal — purely informational, doesn't gate deploy.

### 0.1 Trim `build/macos/build.sh`, drop sandbox entitlements

- [x] 0.1.1 Drop the four required-env preamble: `SIGN_IDENTITY`, `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID`.
- [x] 0.1.2 Replace the `xcrun codesign --sign "$SIGN_IDENTITY" --timestamp ...` block with `codesign --force --sign - --options runtime <binary>`. Drop `--timestamp` (anchors to a TSA chain ad-hoc signatures don't have). Drop `--entitlements` — without sandbox, `network.server`/`network.client` aren't required either; macOS allows network freely for non-sandboxed apps.
- [x] 0.1.3 Delete `pkgbuild` / `productbuild` / `productsign` / `notarytool submit --wait` / `stapler staple` / final `mv` chain (lines ~54-76).
- [x] 0.1.4 Final script output: a signed `Hotchkiss-IO.app` at `$CARGO_TARGET_DIR/Hotchkiss-IO.app` (or `target/Hotchkiss-IO.app` if unset). Prints `BUILT_APP=<absolute-path>` on stdout for downstream consumers. *Scope note: also added `CARGO_TARGET_DIR` awareness so the upcoming 0.5 post-receive hook can reuse target/ across pushes — strictly necessary for 0.5 to work, picked up here while the script was being rewritten.*
- [x] 0.1.5 Delete `build/macos/pkgbuild.plist`, `build/macos/Resources/`, and `build/macos/entitlements.plist` (sandbox+network entitlements no longer apply).
- [x] 0.1.6 Run `./build/macos/build.sh` locally, verify it produces `Hotchkiss-IO.app`, confirm `codesign -dv --verbose=4` shows ad-hoc signature with no entitlement blob. *Confirmed: `Signature=adhoc`, `flags=0x10002(adhoc,runtime)`, `TeamIdentifier=not set`, empty entitlements dump.*

### 0.2 Simplify `Settings` for standard macOS paths

Code change in `src/settings.rs`. Today: all path fields are required `String`s, `make_config_path` joins `<home>/io.hotchkiss.web/config.json`. After: required fields shrink to `cloudflare_token` + `domain`, path fields become optional with `~/Library/...`-derived defaults, types tighten to `PathBuf`.

- [x] 0.2.1 Update `make_config_path` to push `Library/Application Support/io.hotchkiss.web` (instead of bare `io.hotchkiss.web`). Resolved path becomes `<home>/Library/Application Support/io.hotchkiss.web/config.json` — the location CLAUDE.md already claims.
- [x] 0.2.2 Refactor `Settings`: introduced private `RawSettings` (serde target) with `Option<String>` for `database_path`/`log_path`/`cache_path`. *No `omada_config` removal needed — it was never in the Rust struct, only in the on-disk JSON; deserialization silently ignored it.* Public `Settings` is typed `PathBuf` for path fields, `Serialize` derive dropped (no consumers). `Settings::load` calls `Settings::resolve(raw, &home)` to fill `None` paths with home-derived defaults.
- [x] 0.2.3 Existing test renamed `load_with_explicit_paths`, asserts `PathBuf` types. New test `defaults_resolve_against_stubbed_home` calls `Settings::resolve` directly with a stub home, verifies all three defaults land at `Library/Application Support/...`, `Library/Logs/...`, `Library/Caches/...`.
- [x] 0.2.4 Call-site verification: `lib.rs:33,35` pass `&settings.log_path` to `RollingFileAppender::new(_, impl AsRef<Path>, _)` — `&PathBuf` works unchanged. `service_coordinator.rs:30` passes `&settings.database_path` to `DatabaseHandle::create` — that signature was `&str`, so updated to `&Path` and switched the body to `SqliteConnectOptions::new().filename(path)` (avoids lossy `to_str` conversion). *Deviation from plan: plan assumed `DatabaseHandle::create` already took `AsRef<Path>`; it took `&str`. Updating the signature is consistent with the path-typing spirit of 0.2.* `cache_path` has zero readers; added `#[allow(dead_code)]` per plan ("keep the field").
- [x] 0.2.5 `cargo build` + `cargo clippy --all-targets` clean (only pre-existing warnings: dead `update` methods on DAOs, `page_path` field, markdown-transformer collapsible match). `cargo test` 19/19 passing (one transient ifconfig.me network flake on first run, passed on rerun — resolves via Phase 3).

### 0.3 LaunchAgent plist

- [x] 0.3.1 Wrote `build/macos/io.hotchkiss.web.plist` with: `Label=io.hotchkiss.web`, `ProgramArguments=[/Applications/Hotchkiss-IO.app/Contents/MacOS/hotchkiss-io]` (no config arg — binary uses default lookup which now resolves to the standard Application Support path), `RunAtLoad=true`, `KeepAlive=true`, `ThrottleInterval=10`, `WorkingDirectory=/Users/chotchki`, `StandardOutPath=/Users/chotchki/Library/Logs/io.hotchkiss.web/launchd-stdout.log`, `StandardErrorPath=/Users/chotchki/Library/Logs/io.hotchkiss.web/launchd-stderr.log`.
- [x] 0.3.2 No `RootDirectory` (chroot semantics from old plist conflict with absolute ProgramArguments — was either silent no-op or broken).

### 0.4 One-time mini migration + manual end-to-end

This step isolates the launchd/codesign/migration parts from the git-hook automation. Validate the deployable artifact + new path layout work *before* automating delivery.

- [x] 0.4.1 SIGTERM'd PID 50017 (the running prod process — was started manually via LaunchServices, no LaunchAgent to bootout). Confirmed all hotchkiss-io processes gone except the GitHub Actions self-hosted runner (different thing, left alone).
- [x] 0.4.2 Created `~/Library/Application\ Support/io.hotchkiss.web/data`, `~/Library/Logs/io.hotchkiss.web`, `~/Library/Caches/io.hotchkiss.web`. *Side-quest: discovered `~/Library/Application Support/io.hotchkiss.web` already existed from a prior abandoned non-sandbox attempt — config had the old (now-rotated) Cloudflare token, malformed `database_path` (shell-style escapes embedded in JSON), and an October 2025 SQLite from an unclean shutdown. Deleted with user approval.*
- [x] 0.4.3 Migrated `database.sqlite` (81920 B), `database.sqlite-shm` (32768 B), `database.sqlite-wal` (37112 B) from the container's `Data/io.hotchkiss.web/data/` to the new `Library/Application Support/io.hotchkiss.web/data/`. Non-empty WAL meant uncommitted data after last checkpoint — copying all three preserved consistency for SQLite's auto-recovery on next open.
- [x] 0.4.4 Wrote a placeholder `config.json` via SSH; user edited in their own session to inject the rotated token. Verified via `jq` (length>10, not the placeholder, domain=hotchkiss.io) without echoing the secret.
- [x] 0.4.5 `scp -r` the `.app` to `/tmp` on the mini, then `mv` to `/Applications/`. *First scp shipped the 0.1.6-era build, which crashed on launch because `make_config_path` was looking at the pre-0.2 path (`<home>/io.hotchkiss.web/config.json` — no Application Support join). Rebuilt locally (12.84s incremental thanks to cached cargo target), redeployed.*
- [x] 0.4.6 `cp build/macos/io.hotchkiss.web.plist ~/Library/LaunchAgents/`.
- [ ] 0.4.7 `spctl --add /Applications/Hotchkiss-IO.app` — *deliberately skipped*. When launchd execs the binary directly via ProgramArguments (vs. LaunchServices/Finder/`open`), Gatekeeper isn't checked. App started clean without it. Adding only if a future LSOpen path needs it.
- [x] 0.4.8 `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.hotchkiss.web.plist`. *Note: my one-liner had a bash-precedence bug (`bootstrap || echo ... && kickstart`) that always ran the kickstart, harmlessly cycling the freshly-bootstrapped process — accounted for one extra `runs=` increment.*
- [x] 0.4.9 Verified: `https://hotchkiss.io/` → 307 → `/pages/Resume` → 200, 7290 B HTML, 27ms. TLS handshake clean (cert valid). Database content intact (redirect target is DB-derived). User confirmed the site looks right in browser. Tray-icon visual check deferred — `tray-wrapper` fixes pending in Phase 4 mean the icon may not yet look right; not a deploy blocker.
- [x] 0.4.10 Killed PID 33187, slept 12s past ThrottleInterval, confirmed PID 33600 spawned (different PID), runs counter 2→3, `https://hotchkiss.io/` still serves. KeepAlive working as designed.
- [x] 0.4.11 Swapped `.app` while PID 33600 was running (mmap'd binary kept process alive across the swap, as expected), `launchctl kickstart -k`, slept 12s. Result: PID 33857, runs 3→4, exactly one process (no zombies), site still serves. Atomic-ish swap pattern verified for the post-receive hook in 0.5.
- [ ] 0.4.12 Once 0.4.9 confirms the new layout is fully working, the old container at `~/Library/Containers/io.hotchkiss.web/` can be deleted. *Wait until 0.6 (push-to-deploy validation) is also passing — keeping the container around is a free rollback target until then.* Also leave `/Applications/Hotchkiss-IO.app.prev` (the original PKG-installed root-owned bundle) until 0.6 for the same reason.

### 0.5 Bare repo + post-receive hook

- [x] 0.5.1 Added `build/macos/post-receive` (mode 100755 in index). Behavior matches plan: filter on `refs/heads/main`, `git archive` into `~/.cache/hotchkiss-io-build/src` (wiped each run), `CARGO_TARGET_DIR=~/.cache/hotchkiss-io-build/target`, hand off to `build/macos/build.sh`. On build failure exits before touching `/Applications`. Atomic-ish swap: rename current → `.prev`, rename new in, `launchctl kickstart -k gui/$(id -u)/io.hotchkiss.web`, drop `.prev`. PATH explicitly set at top so sshd's stripped env still finds cargo/rustup/launchctl. Build output tee'd to stderr.
- [x] 0.5.2 Added `build/macos/SETUP.md` covering toolchain, directory layout, config, LaunchAgent install, bare-repo init, dev-side `git remote set-url`, first-deploy bootstrap, and verification curl/launchctl commands.
- [x] 0.5.3 *Decision (2026-05-09): fresh bare repo at `~/repos/hotchkiss-io.git` — the existing `~/hotchkiss-io/repo` turned out to be a stale 2025-era working tree on `master` with `denyCurrentBranch=updateInstead`, not lower-friction once cleanup is factored in.* Initialized bare repo at `~/repos/hotchkiss-io.git` (HEAD on `refs/heads/main`, matching the deploy ref filter), copied `post-receive` to `hooks/post-receive` mode 0755.
- [x] 0.5.4 On dev: `git remote set-url origin ssh://hotchkiss.io/Users/chotchki/repos/hotchkiss-io.git`. Old `~/hotchkiss-io/repo` worktree gets deleted as part of 0.4.12 cleanup.

### 0.6 End-to-end push-to-deploy validation

*Ordering: do Phase 4 (tray-wrapper bump) before 0.6.1 — user confirmed 2026-05-09 that the upstream fix landed, so the first automated push doubles as the tray-wrapper smoke test.*

- [ ] 0.6.1 First automated push: `git push origin main`. Observe build streaming to terminal. On success, confirm new app version is live (check version in tray menu, timestamp of `/Applications/Hotchkiss-IO.app`, http response).
- [ ] 0.6.2 Second push (small change): confirm incremental build is fast (<60s wall clock — `CARGO_TARGET_DIR` reuse is working).
- [ ] 0.6.3 Failed-build push: introduce a syntax error, push, confirm hook exits non-zero, running app continues serving (no half-deployed state).
- [ ] 0.6.4 Push to a non-main branch: confirm hook no-ops (ref filter works).
- [ ] 0.6.5 After 0.6.1 lands, also push to `github` so the GitHub mirror tracks the same SHA as production.

### 0.7 Tear down GitHub-Actions release path

- [ ] 0.7.1 Delete `.github/workflows/release.yml`.
- [ ] 0.7.2 Delete `.github/workflows/install.yml`.
- [ ] 0.7.3 Delete the GitHub repo secrets used by the deleted workflows: Developer ID cert blob, `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID`, `SIGN_IDENTITY`. (The repo no longer references them, but secrets sitting around are exposure surface.)
- [ ] 0.7.4 Confirm `test_and_coverage.yml` and `check_ip.yml` still run cleanly on push — both are informational and not on the deploy path.

### 0.8 Docs

- [ ] 0.8.1 Update CLAUDE.md "Common commands" — `build/macos/build.sh` no longer needs the four Apple env vars; produces a signed `.app` (no `.pkg`).
- [ ] 0.8.2 Replace CLAUDE.md "Releases are tag-triggered..." paragraph with the new flow: `git push mini main` triggers the post-receive hook on the mini. Note that Apple notarization / `.pkg` distribution was retired because the binary only deploys to a single self-hosted Mac.
- [ ] 0.8.3 Update CLAUDE.md "Configuration" section — config now lives at `~/Library/Application Support/io.hotchkiss.web/config.json` (matches what the doc already claimed). List required fields (`cloudflare_token`, `domain`) and optional path fields with their default locations. Note the sandbox was dropped.
- [ ] 0.8.4 Update CLAUDE.md "Things to watch out for" — drop sandbox-related caveats if any get added during 0.2; ensure the file-layout section reflects standard `~/Library/...` paths.
- [ ] 0.8.5 Update SPEC.md "Current site's pain" — drop the "deployment is fragile / move to docker" line if Phase 0 holds up through ~5 deploys.

### 0.9 Exit criteria

- [x] 0.9.1 Cloudflare token + Omada password rotated (one-shot remediation for design-discussion leak — confirmed complete by user 2026-05-09).
- [ ] 0.9.2 All boxes above ticked.
- [ ] 0.9.3 Five consecutive pushes deploy cleanly without manual intervention.
- [ ] 0.9.4 At least one deliberate failure case (broken build, killed process) handled gracefully.
- [ ] 0.9.5 Sweep summary to PLAN_ARCHIVE.md per CLAUDE.md workflow rule.

Once 0.9 is green, Phase 3 (ifconfig.me swap) unblocks.

## Phase 1 — Fix `get_recs_by_name` hardcoded `type=A` filter

**Symptom:** ACME cert renewal hangs forever in `DnsValidator::ensure_not_existing` polling for a stale `_acme-challenge` TXT record that never disappears.

**Root cause:** `src/coordinator/dns/cloudflare_api.rs:146` pins the Cloudflare query to `type=A`. When `clean_proof` calls `get_recs_by_name` for the `_acme-challenge` domain, Cloudflare returns 0 results (no A records exist there), the delete loop is a no-op, and no TXT records are ever removed. `ensure_not_existing` then polls indefinitely.

- [x] 1.1 Add a record-type parameter to `CloudflareApi::get_recs_by_name` (`rec_type: &str`) and use it in the query string.
- [x] 1.2 Update `clean_proof` (`cloudflare_client.rs`) to pass `"TXT"`.
- [x] 1.3 Update `update_dns` (`cloudflare_client.rs`) to pass `"A"` (preserves current behavior; keeps `Ipv4Addr::from_str(&rec.content)` parsing safe).
- [x] 1.4 `cargo build` + `cargo clippy` clean (only pre-existing warnings remain).
- [x] 1.5 `cargo test` passes (18/18).
- [ ] 1.6 Manual e2e: trigger an ACME renewal in prod (or shorten the renewal window in dev) and confirm `clean_proof` deletes leftover TXT records before `create_proof` recreates them. *No automated e2e exists for the ACME path — this gap is tracked in Phase 2.*
- [ ] 1.7 Docs: no CLAUDE.md changes needed (behavior fix, no architectural shift). Confirm.

## Phase 2 — DNS module testability (deferred, tracked)

The DNS module has zero tests today. The bug in Phase 1 would have been caught by a unit test on `get_recs_by_name`'s URL construction. Worth fixing but out of scope for the immediate hotfix.

- [ ] 2.1 Extract URL-building from `CloudflareApi` methods into pure helpers.
- [ ] 2.2 Add unit tests covering: query string includes name + type for each call site; type is not hardcoded.
- [ ] 2.3 Decide on HTTP mocking strategy (`wiremock`, `mockito`, hand-rolled) for higher-level tests of `clean_proof` / `create_proof` / `update_dns`.
- [ ] 2.4 Add tests for `DnsValidator::ensure_exists` and `ensure_not_existing` that don't hit a real resolver (would have surfaced the infinite-loop behavior earlier).

## Phase 3 — Replace `ifconfig.me` with Cloudflare `cdn-cgi/trace` (parked, post build/deploy)

**Ordering:** parked until the build/deploy stabilization work lands. Self-contained one-file swap, but not the priority focus — kept here so it doesn't get lost.

**Motivation:** `ifconfig.me` is an external service that may go silently down; we already trust Cloudflare for DNS, so collapsing public-IP discovery into Cloudflare introduces no *new* dependency. `https://1.1.1.1/cdn-cgi/trace` returns `key=value\n` lines including `ip=<addr>`. Connecting to the IPv4 literal `1.1.1.1` forces an IPv4 path, which matches current behavior (`Ipv4Addr` only).

Current code: `src/coordinator/ip/ifconfig.rs` defines `IfconfigMe::public_ip() -> Result<Ipv4Addr>`; `src/coordinator/ip_provider_service.rs` is the only caller.

- [ ] 3.1 Add `src/coordinator/ip/cloudflare_trace.rs` with `CloudflareTrace::new()` + `public_ip() -> Result<Ipv4Addr>`. GET `https://1.1.1.1/cdn-cgi/trace`, split on `\n`, find the line starting with `ip=`, parse the suffix as `Ipv4Addr`. Bail clearly if `ip=` line is missing (Cloudflare changed format) so we notice instead of silently degrading.
- [ ] 3.2 Unit test: parse a captured sample response (hardcoded string with the full key=value block) and assert the extracted `Ipv4Addr`. Also test "missing ip= line" → error and "malformed ip= value" → error.
- [ ] 3.3 Integration test mirroring `ifconfig::tests::basic_run` (`#[tokio::test] async fn basic_run`) that hits the live endpoint and asserts `!addr.is_private()`.
- [ ] 3.4 Swap `IpProviderService::client` from `IfconfigMe` to `CloudflareTrace` in `src/coordinator/ip_provider_service.rs`. Update `super::ip::ifconfig::IfconfigMe` import.
- [ ] 3.5 Delete `src/coordinator/ip/ifconfig.rs` and remove its `pub mod ifconfig;` line in `src/coordinator/ip/mod.rs`. Add `pub mod cloudflare_trace;`.
- [ ] 3.6 Update CLAUDE.md "Runtime architecture" bullet — `IpProviderService` no longer polls `ifconfig.me`; it polls `1.1.1.1/cdn-cgi/trace`. Update SPEC.md "Self contained" external-deps list (drop ifconfig.me).
- [ ] 3.7 `cargo build` + `cargo clippy` clean; `cargo test` passes including the new unit + integration tests.
- [ ] 3.8 Manual e2e: run a non-debug build briefly, confirm the broadcasted IP matches what `curl https://1.1.1.1/cdn-cgi/trace | grep ^ip=` returns. (Debug builds short-circuit to `127.0.0.1` per existing logic — that path is untouched.)

## Phase 4 — Bump `tray-wrapper` once user's fixes publish (UNBLOCKED 2026-05-09)

**Ordering:** the upstream fix landed 2026-05-09. Per user direction, this runs between Phase 0.5 and Phase 0.6 so the first automated push (0.6.1) also validates the new tray icon end-to-end.

Current pin: `tray-wrapper = "0.3.1"` in `Cargo.toml:112` (caret semver).

- [x] 4.1 Latest stable on crates.io is 0.4.1 (published 2026-05-09). Caret `^0.3.1` won't accept 0.4.x — manifest edit required.
- [x] 4.2 `Cargo.toml:112` bumped `0.3.1 → 0.4.1`.
- [x] 4.3 `cargo update -p tray-wrapper` — `Cargo.lock` updated to 0.4.1 (the patched `cookie` crate also re-resolved to a newer commit on `serde_support`, which the `[patch.crates-io]` block tracks; not a behavior change).
- [x] 4.4 `cargo build` + `cargo clippy --all-targets` clean — only pre-existing warnings (dead `update` methods on DAOs, `page_path` field, collapsible-match in markdown transformer). No call-site updates required, the 0.4 API is source-compatible for our uses.
- [x] 4.5 `cargo test`: 19/19 passing.
- [ ] 4.6 Manual verification deferred to 0.6.1 — the first automated push to the mini will exercise the new tray-wrapper end-to-end on the production machine, which is the only place the tray icon actually shows up (debug builds short-circuit the IP service but the tray code is platform-gated, not debug-gated).
- [ ] 4.7 Subsumed by 0.6.1 — `git push origin main` is the deploy.

## Phase 5 — Drop the patched `cookie` fork (parked, post-Phase-0)

**Motivation:** Cookie 0.18.x still doesn't ship serde impls upstream (confirmed 2026-05-09). We currently maintain a fork (`chotchki/cookie-rs` `serde_support` branch) wired in via `[patch.crates-io]` in `Cargo.toml`. CLAUDE.md explicitly calls out the patch as a watch-out. Maintaining a fork to add a couple of trait impls is much heavier than serde's remote-derive pattern (https://serde.rs/remote-derive.html), which lets us provide `Serialize`/`Deserialize` for `cookie::Cookie` from our own crate without forking.

**Discovery:** the working tree only references `tower_sessions::cookie::Key` (the session-signing-key newtype) and never directly serializes `cookie::Cookie`. The patch may be dead — needed by a transitive dep that has since dropped the requirement. Check before assuming we need the workaround.

- [ ] 5.1 Try the no-op path first: comment out the `[patch.crates-io]` block in `Cargo.toml`, `cargo update -p cookie`, `cargo build`. If it builds, the patch was dead code — proceed to 5.5.
- [ ] 5.2 If 5.1 fails, locate the transitive consumer that wants `Cookie: Serialize/Deserialize` (`cargo tree -i cookie -e features` and read the build error). That tells us which crate's API forces the requirement.
- [ ] 5.3 Add `src/cookie_remote.rs` (or similar) with a `CookieDef` newtype, `#[serde(remote = "cookie::Cookie")]`, mirroring the public-field shape of `cookie::Cookie`. Annotate the consumer's call sites with `#[serde(with = "cookie_remote::CookieDef")]`.
- [ ] 5.4 If the transitive consumer is itself defining serde structs around `Cookie` (i.e. we can't reach the call site), the remote-derive escape hatch doesn't apply — at that point either the upstream crate needs a feature flag or we keep the fork. Document the finding and revert.
- [ ] 5.5 With the patch removed, drop `[patch.crates-io]` entirely from `Cargo.toml`, the corresponding lockfile entries, and the CLAUDE.md "Patched `cookie` crate" caveat.
- [ ] 5.6 `cargo build` + `cargo clippy --all-targets` clean; `cargo test` 19/19 passing.
