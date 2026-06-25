# Plan archive

Completed phases, swept here from `PLAN.md` per the workflow rule (a phase exits when every box is ticked, e2e passes, docs updated → summarize → sweep). Newest first.

---

## Phase 9 — Tidy the Tailwind build pipeline; drop DaisyUI — DONE 2026-05-10

**Summary:** small prerequisite for the upcoming mobile-posting / editor facelift — a reproducible CSS build, minus the unused DaisyUI download.

- **9.1 Drop DaisyUI.** `build.rs` used to download three things into `$OUT_DIR` — the Tailwind CLI plus `daisyui.js` + `daisyui-theme.js` — but `styles/tailwind.css` never `@plugin "daisyui"`'d, so DaisyUI was fetched and never used. Removed the two `daisyui*` downloads (and the `HashMap` they lived in; the import too). `package.json` had no `daisyui` devDependency; `tailwind.css` had no `@plugin "daisyui"` — nothing else to remove. Decision (2026-05-10, user): the site is styled with hand-rolled Tailwind utilities and the facelift keeps doing that.
- **9.2 Pin the Tailwind CLI.** Was `…/releases/latest/download/tailwindcss-macos-arm64` (unpinned → non-reproducible; a Tailwind release could break the build silently). Now `const TAILWIND_VERSION = "v4.3.0"` in `build.rs`, fetched from `…/releases/download/v4.3.0/tailwindcss-macos-arm64`, cached at `$OUT_DIR/tailwindcli-v4.3.0` (version-keyed filename → bumping the const forces a re-download, no stale binary). Added `.error_for_status()` on the fetch so a bad pin fails loudly instead of writing a 404 page into the CLI file. `cargo clean -p hotchkiss-io && cargo build` confirmed: `assets/styles/main.css` regenerated, header `tailwindcss v4.3.0`, ~35 KB (comparable to before). The standalone CLI still resolves `@plugin "@tailwindcss/typography"` — unchanged.
- **9.3 Docs.** CLAUDE.md "Build-time machinery" point 2 rewritten (pinned CLI, version-keyed cache, DaisyUI removal note); the "Tailwind/DaisyUI build pipeline" Tech-debt item removed.

**Not done (deliberately):** arch/OS-awareness of the CLI download — still hardcoded `tailwindcss-macos-arm64`. Every place `build.rs` runs today is arm64 macOS (dev machines, the mini's post-receive build, `macos-latest` CI), so this is future-proofing, not a bug; revisit if a Linux/x86 build ever appears.

**Validation:** `cargo test` 40 green; `cargo clippy --all-targets` clean (5 standing pre-existing warnings); deployed via `git push origin main` — the prod build (release, on the mini) still produces a styled site.

---

## Phase 8 — Local / e2e test harness — DONE 2026-05-10

**Summary:** the running site is now testable without the prod machinery (no `:80`/`:443` bind, no IP/DNS/ACME coordinator, no passkey hardware). All-Rust — the e2e was prototyped with Playwright (TS) but, per user preference, redone with `chromiumoxide` so there's no Node toolchain.

- **8.1 In-process harness.** `src/test_support.rs` (a new `pub mod test_support;` in the lib — lives there, not `tests/common/`, so it can reach the crate-internal `create_router`/`AppState`/`DatabaseHandle` without making half the crate `pub`). `spawn_test_server() -> Result<TestServer>`: fresh tempfile SQLite via `DatabaseHandle::create` (same WAL/FK config as prod) → migrations; `SqliteStore::new(pool).migrate()`; `WebauthnBuilder::new("localhost", "http://localhost:<port>/")` (webauthn-rs accepts the http-localhost origin); `create_router(app_state)`; `axum::serve(TcpListener::bind("127.0.0.1:0"), router.into_make_service_with_connect_info::<SocketAddr>())` spawned. `TestServer { base_url, pool }` + `url(path)` + `seed_content_page(name, markdown)` + a `Drop` that aborts the server task and removes the temp DB(+wal/shm). *Side change:* `create_router` now sets the session layer's `Secure` flag from `!cfg!(debug_assertions)` — `Secure` cookies aren't sent over the harness's plain HTTP; prod (release) is unchanged (HTTPS-only, still `Secure`). Smoke test: `tests/server.rs::harness_boots_and_serves` (`/` → 307 thanks to the `0007` special-pages seed; a seeded content page renders).
- **8.2 Debug-only login seam.** `#[cfg(debug_assertions)] src/web/features/test_login.rs::test_router()` — `POST /test/login[?role=Admin|Registered]` (default `Admin`): direct `INSERT INTO users (...)` of a fresh user with that role (bypasses `UserDao::create`'s first-user→Admin override) then `SessionData::update_session(&session, &SessionData { auth_state: Authenticated(user) })`. Nested at `/test` in `create_router` behind `#[cfg(debug_assertions)]` (attribute on the `let router = ...` line — in release it vanishes); `#[cfg(debug_assertions)] pub mod test_login;` in `web/features/mod.rs`. Confirmed absent from the deployed prod release binary (`strings` → no `test/login`).
- **8.3 Rust integration tests.** `tests/web.rs`: `analytics_requires_admin` (anon → 403, `?role=Registered` → still 403, `?role=Admin` → 200 + the dashboard renders) and `request_log_middleware_records_requests` (`GET /pages/Probe`, then poll `request_log` via `server.pool` — asserts `status = 200`, `ip = 127.0.0.1`, i.e. `ConnectInfo` is wired). Each test gets a fresh DB via `spawn_test_server`; DB reads use runtime `sqlx::query(...)` (no `DATABASE_URL` needed for the `tests/` crate). Content-page rendering is covered by `tests/server.rs`. *(This closes the earlier "nothing tests the `require_admin` layer is wired" gap.)*
- **8.4 Browser e2e (pure Rust, `chromiumoxide`).** `tests/e2e_browser.rs` — `#[ignore]`d (needs Chrome installed; run via `cargo test --test e2e_browser -- --ignored`). Launches headless Chrome (`chromiumoxide`, default features = tokio runtime; a throwaway `user_data_dir` per launch so concurrent tests don't fight a shared `SingletonLock`), spawns the CDP event-drain task, attaches a CDP **virtual authenticator** (`WebAuthn.enable` + `WebAuthn.addVirtualAuthenticator` with `ctap2`/`internal`/`hasResidentKey`/`hasUserVerification`/`isUserVerified`/`automaticPresenceSimulation`), then drives the *real* passkey registration ceremony through `htmx-webauthn.js` (`/login` → fill `#username` → submit → `GET /login/start_register/<name>` → `navigator.credentials.create` → `POST /login/finish_register` → first user becomes Admin → `window.location.href = "/"`), waits for the URL to leave `/login`, then asserts `GET /admin/analytics` renders the dashboard. Plus `anonymous_forbidden_from_admin_dashboard` (the 403 body). *The `htmx-webauthn.js` registration path drove cleanly through the virtual authenticator — no footgun surfaced for that flow; the conditional-auth/autofill path is not yet exercised.* **Decision:** kept out of `cargo test`/CI (Chrome dependency); run manually when touching the login flow or the WebAuthn extension. *(History: a Playwright + CDP version was built first (`e2e/` dir, `tests/e2e_serve.rs` blocking-serve harness, `auth.spec.ts`) and both tests passed there — then ripped out and redone in `chromiumoxide` to drop the Node toolchain.)*
- **8.5 Docs.** CLAUDE.md "Common commands" — `cargo test` now includes the `tests/` integration tests on the in-process server; the debug-only `/test/login` seam; `cargo test --test e2e_browser -- --ignored` for the chromiumoxide e2e.

**Validation:** `cargo test` 40 green (37 lib + 1 `tests/server.rs` + 2 `tests/web.rs`; the 2 `tests/e2e_browser.rs` tests are `#[ignore]`d and pass when run with `--ignored`); `cargo clippy --all-targets` clean (5 standing pre-existing warnings, none new). Deps added (dev-only): `chromiumoxide = "0.9.1"`, `futures = "0.3"`.

---

## Phase 7 — Admin analytics dashboard — DONE 2026-05-10

**Summary:** an admin-only `/admin/analytics` page answering "who's hitting / scraping my site". Three slices, all shipped (commit `c252896`); the first use of a route-group auth layer in this codebase, and a deliberate *non*-use of the `special_page` mechanism (analytics is a real handler, not a redirect row).

- **7.1 Data layer.** Migration `0009_TableRequestLog` — `request_log (id, ts text NOT NULL DEFAULT CURRENT_TIMESTAMP, method, path, status, ip, user_agent, referer)` + `idx_request_log_ts` (SQLite stamps `ts` on insert, UTC `YYYY-MM-DD HH:MM:SS`; `substr(ts,1,10)` = the day; `datetime('now','-N days')` = windows — so the middleware never computes a timestamp). `RequestLogDao` (`src/db/dao/request_log.rs`): `insert(&NewRequestLog)`, `recent(limit)`, `count_since(days)`, `distinct_ip_count(days)`, `count_by_path(days,limit)`, `count_by_user_agent(days,limit)`, `count_by_day(days)`, `prune_before(retain_days)` — windows via a private `window(days) -> "-N days"`; aggregates ORDER/GROUP BY the underlying expression, *not* the sqlx column alias (sqlx's compile-check rejects alias refs there — that cost a debugging round). 3 `#[sqlx::test]` units. `web/middleware/request_log.rs::log_requests` — wired as the *outermost* layer in `create_router` via `from_fn_with_state(pool, ..)`: reads method/path + the client IP (from the `ConnectInfo<SocketAddr>` request extension) + `User-Agent`/`Referer` headers, runs `next`, then `tokio::spawn`s the INSERT (fire-and-forget — never adds latency to nor fails a response; `warn!` on insert error); `debug_assertions` builds skip `/tower-livereload`. `EndpointsProviderService` now serves HTTPS with `into_make_service_with_connect_info::<SocketAddr>()`, and runs a daily `RequestLogDao::prune_before(pool, 90)` task in its `JoinSet` alongside the session GC (first tick fires at startup).
- **7.2 Auth layer.** `web/middleware/require_admin.rs::require_admin` — `async fn(SessionData, Request, Next) -> Result<Response, (StatusCode, &'static str)>`: `403 "Admin only"` unless `session_data.auth_state.is_admin()`, else `next.run(req).await`. (`SessionData`'s extractor defaults to `Anonymous` with no session → unauthenticated gets a clean 403, no panic.) Applied via `.layer(from_fn(require_admin))` on `web/features/admin/admin_router()`, nested at `/admin` in `create_router` (inside the top-level session layer). The existing scattered per-handler `is_admin()` / `if let Authenticated(u) && u.role != Admin` checks were deliberately left alone (Tech debt). No router-level test for the *layer wiring* yet — `is_admin()` is unit-tested and `require_admin` is a one-liner over it; the wiring gap is closed by Phase 8's integration tests.
- **7.3 View.** `web/features/admin/analytics.rs::show_analytics` — `State<AppState>` + `SessionData` + `Query<{since: Option<i64>}>` (`?since=` → `unwrap_or(7).clamp(1,365)`), runs the `RequestLogDao` bundle (`count_since`, `distinct_ip_count`, `count_by_day`, `count_by_path(_,25)`, `count_by_user_agent(_,25)`, `recent(50)`), renders `AnalyticsTemplate`. No auth check in the handler — the layer owns it. `templates/analytics/dashboard.html` (`extends "base.html"`): `1d/7d/30d/90d` window pills, two big stats (requests, distinct IPs), then "Requests per day" / "Top paths" / "Top user agents" / "Recent requests" tables — plain HTML + the existing Tailwind classes, no JS charting. (Built a v1 directly rather than an ASCII-mock-first round — fine for an "easy feature" pass.) Conditional "Analytics" `<li>` added to the admin block of the nav in `base.html`.
- **7.4 Docs.** CLAUDE.md — "Runtime architecture" `EndpointsProviderService` bullet → `into_make_service_with_connect_info` + the prune task; "Web layer" Routing bullet → the `/admin` nest + the outer middleware stack incl. request-logging; the Authorization bullet → the `require_admin` layer is the *one* place auth is layer-enforced (rest still per-handler — see Tech debt). SPEC.md — "Analytics…" marked "v1 shipped 2026-05" with a Phase-7 pointer.

**Validation:** `cargo test` 37/37 (3 new); `cargo build` + `cargo clippy --all-targets` clean (only the 4 standing pre-existing warnings). In prod (`c252896`): migration `0009` applied; `request_log` filling with correct client IPs / methods / statuses / UAs (the steady background of `/wp-admin`, `/.env`, etc. — exactly the "who's scraping" signal); `GET /admin/analytics` → 403 unauthenticated; user signed off on the dashboard ("good MVP") logged in as admin.

**Follow-ons (Backlog):** status / "noise" view (paths that only ever 404); per-IP drill-down (scan fingerprint); referer breakdown (`referer` already recorded, just not surfaced); analytics→defense IP-blocklist (its own phase). **Recorded but not surfaced yet:** the dashboard shows visitor IPs/UAs in plaintext — fine for one's own admin eyes; the 90-day prune bounds the window; truncate/hash if that ever changes.

---

## Phase 2 — DNS module testability — DONE 2026-05-10

**Summary:** the DNS module had zero tests; the Phase 1 bug (a `type=A` pinned into `get_recs_by_name`'s query string) was the motivating example. Two concrete pieces landed; two follow-ups were deliberately deferred (now Phase 6).

- **2.1 — pure URL builders.** Extracted four private associated fns on `CloudflareApi`: `dns_records_url(zone)`, `dns_record_url(zone, rec)`, `zones_query_url(zone_name)`, `dns_records_query_url(zone, name, rec_type)` (the query-param ones now build via `Url::query_pairs_mut().append_pair(...)` instead of a hand-formatted string — same output for our inputs, but properly encoded and trivially testable). `create_record` / `create_txt_record` / `delete_record` / `get_zone_id` / `get_recs_by_name` all call the helpers; no behavior change.
- **2.2 — unit tests on the builders (5).** Pin the collection/single/zone-query URLs, assert `dns_records_query_url` emits exactly `name=` then `type=` over the right path, and — the regression guard for Phase 1 — `dns_records_query_type_is_a_parameter_not_hardcoded` loops `A`/`AAAA`/`TXT`/`CNAME` and checks the `type=` value tracks the argument.
- **2.3 — HTTP-mocking decision.** Don't build it now. Mocking `clean_proof`/`create_proof`/`update_dns` needs `BASE_URL` (a `LazyLock<Url>` const) to become a `CloudflareApi` field so a fake server URL can be injected; then `wiremock` (async-first, fits the codebase). The regression class that bit us is covered by 2.2's pure tests; the remaining untested logic in those methods is set arithmetic + sequencing, lower-risk. Recorded as Phase 6.1.
- **2.4 — testable `DnsValidator` decision logic.** Split the wait loops: `lookup_once` does one uncached resolver call and maps it to a `LookupOutcome` (`Found(Vec<RData>)` | `NoRecords`, with non-`NoRecordsFound` errors bubbling), and pure `exists_step(expected, outcome) -> WaitStep` / `not_existing_step(outcome) -> WaitStep` decide done-vs-keep-waiting. 7 unit tests: exact match, order-insensitive match, partial set → wait, wrong records → wait, no records → wait (for `exists`), and empty → done / leftovers → wait (for `not_existing`, the latter being the Phase 1 *symptom* — correct in isolation; the bug was the upstream deletion query). **Finding:** the `DnsValidator` timeout check is commented out *and* its condition was backwards (`timeout > Instant::now()` where `timeout = now + 300s` → true the whole window → would bail immediately). Left disabled (re-enabling changes the ACME path's runtime behavior — bounded retry would fail renewals on slow propagation), corrected the comment, tracked as Phase 6.2.

Suite went 22 → 34 tests, `cargo build` + `cargo clippy --all-targets` clean (only the standing pre-existing warnings). No docs change needed — internal refactor. No deploy needed for correctness, but it shipped on the next `git push origin main` anyway.

---

## Phase 5 — Drop the patched `cookie` fork — DONE 2026-05-10

**Original hypothesis (wrong on the specifics):** the plan assumed the `chotchki/cookie-rs` `serde_support` fork existed to get serde on `cookie::Cookie`, possibly dead code, fixable via serde remote-derive. **What it actually was:** the only consumer is `src/db/dao/crypto_key.rs`, which stored the session-signing key as `sqlx::types::Json<cookie::Key>` — and `Json<T>` needs `T: Serialize + DeserializeOwned` directly (no `#[serde(with)]` hook on a generic wrapper), so the fork's `serde` feature on `Key` was load-bearing. Removing the `[patch.crates-io]` block straight up failed to compile (`Serialize`/`Deserialize` not implemented for `tower_sessions::cookie::Key`).

**Better fix taken (user was open to it):** a `cookie::Key` is just a 64-byte master key — `Key::master()` gives the bytes, `Key::try_from(&[u8])` reconstructs it. Stuffing that into a `BLOB` column as JSON text was pointless. So `CryptoKey.key_value` changed from `sqlx::types::Json<Key>` to `Vec<u8>` (the raw master bytes), with a new `CryptoKey::key() -> Result<Key>` accessor; `web/router.rs` now calls `.with_signed(key.key()?)` instead of `.with_signed(key.key_value.0)`. No serde on `Key` needed, so **both** the `[patch.crates-io]` block *and* the direct `cookie = { git = ..., features = ["serde"] }` dependency in `Cargo.toml` were removed — the direct dep existed only to turn the `serde` feature on graph-wide via feature unification (the crate is never `use`d directly; `crypto_key.rs`/`router.rs` reach `Key` through `tower_sessions::cookie`). `cookie` re-resolves to crates.io `0.18.1` (a `cargo build` did the minimal lock re-resolve; a full `cargo update` would've churned ~80 unrelated crates, so that was reverted). CLAUDE.md "Patched `cookie` crate" caveat deleted.

**Migration `0008_DMLCryptoKeysRawBytes.sql`:** `DELETE FROM crypto_keys;` — the existing prod row held JSON-text bytes, not a real 64-byte key, so it's cleared and `get_or_create` regenerates a proper `Key::generate()` on next boot. **Side effect:** existing signed session cookies became invalid → everyone got logged out once and re-authenticated (passkey tap). Accepted given the tiny user base and that deploys already carry ~15s downtime.

**Validation:** `cargo clean -p hotchkiss-io` (migration change → sqlx macros re-validate against the rebuilt schema db), then `cargo build` + `cargo clippy --all-targets` clean (only the standing pre-existing warnings), `cargo test` 22/22. Deployed via `git push origin main` — migration `0008` ran on the prod SQLite, key regenerated, site stayed up.

Per-task mapping to the original checklist: 5.1 → patch removed, but it was *not* dead code (build failed) · 5.2 → consumer identified as our own `crypto_key.rs`, not a transitive dep · 5.3/5.4 → remote-derive shelved; the cleaner raw-bytes storage made it moot · 5.5 → `[patch.crates-io]` dropped, lockfile re-resolved, CLAUDE.md caveat removed · 5.6 → build/clippy/test green (22/22).

---

## Phase 3 — Replace `ifconfig.me` with Cloudflare `cdn-cgi/trace` — DONE 2026-05-10

**Summary:** public-IPv4 discovery moved off `ifconfig.me` (an external service that could go down silently) and onto `https://1.1.1.1/cdn-cgi/trace` — folded into the Cloudflare dependency we already have, so no *new* third party. New `src/coordinator/ip/cloudflare_trace.rs` defines `CloudflareTrace { client: reqwest::Client }` with `new()` (rustls) and `public_ip() -> Result<Ipv4Addr>`: GET the trace endpoint, `error_for_status`, then a pure private `parse_ip(&str)` that finds the `ip=` line via `strip_prefix` and parses it, `.context()`-ing a clear error if the line is missing (Cloudflare format change) or the value won't parse. Connecting to the IPv4 literal `1.1.1.1` forces a v4 path, matching the old `Ipv4Addr`-only behavior. `IpProviderService` now holds a `CloudflareTrace` instead of `IfconfigMe` — only the field type and one import changed; `server_ips()` is untouched. Old `src/coordinator/ip/ifconfig.rs` deleted; `src/coordinator/ip/mod.rs` is now just `pub mod cloudflare_trace;`. Tests: three new units (`parses_ip_from_sample` against a full captured key=value block → `203.0.113.42`, `missing_ip_line_errors`, `malformed_ip_value_errors`) plus `cloudflare_trace::tests::basic_run` (live endpoint, `!addr.is_private()`) replacing the old ifconfig integration test — suite 22/22 (was 19). Also retires the one transient test flake (the old `ifconfig.me` `basic_run` occasionally tripped on a network blip). Docs: CLAUDE.md "Runtime architecture" bullet + SPEC.md "Self contained" list updated. Shipped via `git push origin main` (commit `22242d4`); prod stayed up and non-crash-looping (a `public_ip()` error would `?`-propagate through `IpProviderService::start` → kill the coordinator → `KeepAlive` crash-loop), and `dig hotchkiss.io` (`174.21.221.87`) matched what `1.1.1.1/cdn-cgi/trace` reported from the mini — confirming the new path ran.

Per-task detail (all `[x]`): 3.1 new module · 3.2 three unit tests · 3.3 live `basic_run` · 3.4 `IpProviderService` swap · 3.5 old module deleted · 3.6 CLAUDE.md + SPEC.md · 3.7 build/clippy/test (22/22) · 3.8 prod e2e (dig matches trace, no crash-loop).

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

---

## 2026-06-22

## Phase 1 - Fix `get_recs_by_name` hardcoded `type=A` filter

**Symptom:** ACME cert renewal hangs forever in `DnsValidator::ensure_not_existing` polling for a stale `_acme-challenge` TXT record that never disappears.

**Root cause:** `CloudflareApi::get_recs_by_name` pinned the Cloudflare query to `type=A`. When `clean_proof` calls it for the `_acme-challenge` domain, Cloudflare returns 0 results (no A records exist there), the delete loop is a no-op, and no TXT records are ever removed. `ensure_not_existing` then polls indefinitely.

- [x] 1.1 - Add a record-type parameter to `CloudflareApi::get_recs_by_name` (`rec_type: &str`) and use it in the query string.
- [x] 1.2 - Update `clean_proof` (`cloudflare_client.rs`) to pass `"TXT"`.
- [x] 1.3 - Update `update_dns` (`cloudflare_client.rs`) to pass `"A"` (preserves current behavior; keeps `Ipv4Addr::from_str(&rec.content)` parsing safe).
- [x] 1.4 - `cargo build` + `cargo clippy` clean (only pre-existing warnings remain).
- [x] 1.5 - `cargo test` passes.
- [x] 1.6 - Manual e2e: confirm the next real ACME renewal in prod succeeds — `clean_proof` deletes any leftover `_acme-challenge` TXT records before `create_proof` recreates them. **Confirmed 2026-06-22**: cert rolled over in prod, renewal succeeded. (Phase 2 added unit coverage for the URL-construction class of bug; an automated ACME-path e2e is still a gap — tracked in Phase 6.)
- [x] 1.7 - Docs: no CLAUDE.md changes needed (behavior fix, no architectural shift). **Confirmed 2026-06-22** — none needed.


---

## 2026-06-23

## Phase 12 - Beta deployment on the mini

Same mini, alternate ports, snapshot-from-prod data on each beta deploy, inverted code flow: `main` → beta (always bleeding edge); `vX.Y.Z` tag → prod (deliberate promotion). Closes the prod-contamination risk that Phase 10/11 dogfooding hit, gives a real HTTPS surface for PWA install on the LAN, and separates "code lands" from "code ships to the public."

Beta is **public** (decided 2026-06-22 — chris is often off-LAN, so LAN-only wouldn't be reachable): a grey-cloud (DNS-only) `beta.hotchkiss.io` A record that beta's own `DnsProviderService` keeps pointed at the public IP (like prod's `hotchkiss.io`), reached on `:8443` via a router port-forward to the mini. Grey-cloud, not orange/proxied, so beta serves its own LE cert end-to-end (orange would put Cloudflare's cert in front and break the trusted-PWA-cert story). iCloud-Keychain passkeys from prod authenticate against beta because beta's WebAuthn rp_id is `hotchkiss.io` (the registrable parent) and the snapshot carries prod's `users` over. Beta data is intentionally ephemeral: prod's `database.sqlite` is snapshotted to beta on every `main` push, so any beta-only edits get blown away on the next deploy — that's the point. Beta stays a beta. **Public-beta safety:** WebAuthn server records are public keys, not secrets (can't forge auth without the authenticator's private key), and the snapshot is prod→beta one-way (beta registrations never reach prod), so a public beta exposes no forgeable credentials; the snapshot also scrubs `request_log` for visitor-IP privacy. Carrying prod's `users` (rather than preserving beta's own) keeps beta's user table non-empty, avoiding the first-user-becomes-admin land-grab a public, empty-table beta would invite.

- [x] 12.0 - Phase exit (met 2026-06-23 — all five criteria verified live; deploy hook hardened per adversarial review): `main` push → beta rebuilds + restarts with snapshotted prod data on `https://beta.hotchkiss.io:8443/`; iPhone installs the PWA from beta over real, publicly-trusted LE HTTPS (beta is a release build → LE prod → natively trusted, no profile install); existing prod passkey auths against beta; tag push → prod rebuilds + restarts on `https://hotchkiss.io/`; both run side-by-side on the mini.
- [x] 12.1 - `Settings`: add `webauthn_rp_id: Option<String>` (defaults to `domain` when absent). Update `EndpointsProviderService::create` to pass it to `WebauthnBuilder` instead of `settings.domain`. Beta uses `hotchkiss.io` so chris's existing prod passkey authenticates against beta too. (Done 2026-06-22: resolved to a concrete `String` in `Settings::resolve`; origin stays the served domain; unit test `load_with_webauthn_rp_id` + default-from-domain assertions.)
- [x] 12.2 - `build/macos/build.sh`: take a `--profile beta|prod` flag. Profile determines bundle name (`Hotchkiss-IO.app` vs `Hotchkiss-IO-Beta.app`), install path, LaunchAgent label. Today's path becomes the `prod` case; `prod` is the default if `--profile` is absent.
- [x] 12.3 - LaunchAgents: **kept prod as `io.hotchkiss.web` (no rename** — renaming a live agent is a bootout/bootstrap migration for zero functional gain); added `build/macos/io.hotchkiss.web.beta.plist` (label `io.hotchkiss.web.beta`, runs `Hotchkiss-IO-Beta.app` with an explicit beta config path as `argv[1]` — prod relies on the default config location, so beta must point at its own or it'd read prod's; beta launchd log dir `~/Library/Logs/io.hotchkiss.web.beta/`). Both `RunAtLoad`; the 12.4 post-receive kickstarts the matching label on swap. `plutil -lint` clean. SETUP.md notes the beta-agent prereqs.
- [x] 12.4 - `build/macos/post-receive`: dispatch by ref — `refs/heads/main` → `build.sh --profile beta` → swap `Hotchkiss-IO-Beta.app` → kickstart `io.hotchkiss.web.beta`; `refs/tags/v*` → `--profile prod` → swap `Hotchkiss-IO.app` → kickstart `io.hotchkiss.web` (= today's behavior, now gated on a version tag). Factored into a profile-parameterized `deploy()` with per-profile src/target dirs; beta-only 12.5 snapshot hook-point stubbed before the kickstart. `bash -n` + routing test clean. **Repo file only — the live cutover (re-copy the hook onto the mini) is part of the 12.8 sequence, not done by this edit.**
- [x] 12.5 - Beta DB snapshot in `post-receive` (`snapshot_prod_db_into_beta`, beta branch only): consistent online `sqlite3 .backup` of **live** prod (`~/Library/Application Support/io.hotchkiss.web/data/database.sqlite` — prod kept its un-suffixed dir, see 12.3) into beta's path — **`.backup`, not `cp`**, since prod may be mid-write (decided 2026-06-22). Then `DELETE FROM crypto_keys` (beta regenerates its own session-signing key on boot) + `DELETE FROM request_log` (visitor-IP privacy — beta is public) + `DELETE FROM tower_sessions` (sessions don't cross); users/passkeys carry over so chris's prod passkey authenticates on beta (and the table stays non-empty → no first-user-admin land-grab). **Cert preservation:** dump beta's `certificates` rows *before* the overwrite, drop prod's carried-over rows *after*, restore beta's — so beta never re-orders `beta.hotchkiss.io` from LE prod (the 5/week duplicate-cert limit would take beta HTTPS down). Runs before the kickstart. Functionally tested on throwaway DBs (steady-state + first-deploy); `bash -n` clean. Depends on macOS `/usr/bin/sqlite3`.
- [x] 12.6 - Beta config (mini-side done 2026-06-23: config placed on the mini, beta running on it). **Repo-side done:** committed template `build/macos/beta-config.sample.json` (`domain=beta.hotchkiss.io`, `webauthn_rp_id=hotchkiss.io`, `http_port=8080`, `https_port=8443`, beta `database_path`/`log_path`/`cache_path`, placeholder for the CF token (same as prod — see 12.7) — **no `static_ip`**: beta is public and discovers its IP like prod) + SETUP.md §8 beta bring-up runbook. JSON validated. **Mini-side pending (needs the 12.7 token):** copy the template to `~/Library/Application Support/io.hotchkiss.web.beta/config.json` and fill in the beta CF token + mini LAN IP.
- [x] 12.7 - Cloudflare + router (one-time): **reuse the prod CF token** for beta — CF can't scope narrower than the zone (decided 2026-06-22; only trades away independent revocation/audit). `beta.hotchkiss.io` A record exists (placeholder `127.0.0.1`, grey-cloud) — beta's `DnsProviderService` reconciles it to the live public IP on first boot (`update_dns` creates the public-IP record + deletes the placeholder; name-scoped, never touches prod's `hotchkiss.io`). Cert issuance is DNS-01, so it's A-record-independent anyway. Router forwards `:8443` **and** `:8080` → the mini (done). Grey-cloud, not orange/proxied, so beta serves its own end-to-end LE cert.
- [x] 12.8 - Bootstrap the inverted flow (done 2026-06-23 — hardened hook installed on the mini; bumped Cargo 0.0.42→0.0.44; tagged `v0.0.44` → prod rebuilt + deployed via the tag path in 4m31s; prod no longer auto-updates from main). Note: actual tag was `v0.0.44`, not `v0.0.42` (latest pre-existing tag was v0.0.43). cut `v0.0.42` (or the current Cargo version) from today's main. Tag-push to origin. Confirm the new post-receive routes the tag to a prod build/swap. After this commit, prod stops auto-updating from main — only tags promote.
- [x] 12.9 - CLAUDE.md update (done 2026-06-23: inverted flow, beta instance + snapshot lifecycle, rp_id, `--profile`, configurable-ports fix): document the prod+beta model, alternate ports, snapshot lifecycle, rp_id story, the inverted deploy flow (push main = beta, tag = prod). Absorbs Phase 11.8 (the `http_port`/`https_port`/`static_ip` settings docs).
- [x] 12.10 - Manual e2e on the iPhone (done 2026-06-23: PWA installed over beta's LE cert, no profile; prod passkey authenticated on beta; tag→prod round-trip exercised by 12.8): push a `main` commit → beta rebuilds with snapshotted prod data → install PWA from `https://beta.hotchkiss.io:8443/` (real LE prod cert, natively trusted — no profile) → existing chris passkey authenticates → edit a blog post on beta → tag-push the change to `v0.x.y+1` → prod deploys, post lands in prod's DB on next push-main → snapshot. Phase exit.
- [x] 12.11 - Retire Phase 11.3, 11.8, 11.9 (content absorbed into 12.6, 12.7, 12.9, 12.10). Update PLAN.md. (Phase 11 folded 2026-06-22; this box stays as the marker that the absorbed content actually lands in 12.6/12.9/12.10.)


---

## 2026-06-24

## Phase A - Diagram rendering (D2, source-in-HTML + HTMX swap)
- [x] A.0 - Phase exit: pages+blog embed ```d2; served HTML carries source (LLM/no-JS), HTMX swaps in the D2-rendered SVG; degrades gracefully; mobile
- [x] A.1 - Decide rendering point (RESOLVED: request-time from inline markdown fence)
- [x] A.2 - Diagram backend: D2 via brew-installed binary (shell out)
- [x] A.3 - Transformer: fenced d2 -> source placeholder + HTMX swap target
- [x] A.4 - Cache compiled SVG by source hash
- [x] A.5 - Broken d2 source fails visibly, never a 500
- [x] A.6 - Mobile: SVG scales within the column at 390px
- [x] A.7 - e2e + CLAUDE.md/SPEC update
- [x] A.8 - HTMX swap-by-hash delivery: source in HTML + GET /diagram/{hash}
- [x] A.9 - Diagram sizing: max-height cap + click-to-zoom lightbox

---

## 2026-06-25

## Phase B - Database backups (rolling daily)
- [x] B.0 - Phase exit: live DB backed up daily (consistent VACUUM INTO), 7-day rolling retention, into a Backblaze-synced dir; verified by tests
- [x] B.1 - Decide mechanism + location (VACUUM INTO in-process; Settings.backup_path)
- [x] B.2 - Daily backup task (mirror the request_log prune task)
- [x] B.3 - 7-day rolling retention (prune older backups)
- [x] B.4 - Fail-safe: a backup error logs + never crashes the coordinator
- [x] B.5 - Settings.backup_path config + default (Settings::resolve)
- [x] B.6 - Tests + CLAUDE.md/SPEC docs

---

## 2026-06-25

## Phase C - Analytics: views over time
- [x] C.0 - Phase exit: /admin/analytics summarizes views over time (design-approved by chris, then implemented)
- [x] C.1 - DESIGN (with chris): what to summarize
- [x] C.2 - DESIGN: data approach (aggregate query vs rollup)
- [x] C.3 - Implement the over-time summary + chart on /admin/analytics
- [x] C.4 - Tests + CLAUDE.md/SPEC docs (analytics)

---

## 2026-06-25

## Phase D - Analytics: referrers + attack-probe visibility
- [x] D.0 - Phase exit: analytics shows top external referrers + a Top-Pages Content/All toggle; live on prod
- [x] D.1 - Implement: referrers (count_by_referer) + Top-Pages content/all status toggle + tests/docs

---

## 2026-06-25

## Phase E - Fail-closed authz middleware layer
- [x] E.0 - Phase exit: one fail-closed authz layer (GET public; non-GET=admin by default; login/logout override); per-handler checks removed; live on prod
- [x] E.1 - Audit every route + finalize the anonymous-mutation allowlist
- [x] E.2 - Build the central fail-closed authz middleware (method-aware + allowlist override)
- [x] E.3 - Remove the now-redundant per-handler is_admin checks (audited)
- [x] E.4 - Tests: site-wide non-GET blocked for anon/registered; login ceremony + logout still work; admin works
- [x] E.5 - Docs: flip CLAUDE.md authz note + SPEC to the fail-closed layer

---

## 2026-06-25

## Phase F - Admin / authoring UX
- [x] F.0 - Phase exit: site is pleasant + usable logged-in as admin — clean reader view, login state visible, sane authoring flow, human titles (not slugs) shown publicly
- [x] F.1 - Title↔slug separation: add page_title, create-by-title with auto-slug, display title everywhere (fix the public hyphenated headline)
- [x] F.2 - Logged-in reader view: default to the clean page, an Edit toggle reveals the editor
- [x] F.3 - New-page creation redirects to the new page's editor (not htmx_refresh on the list)
- [x] F.4 - Login-state indicator + logout in the nav
- [x] F.5 - Restyle the page editor (raw textarea + unstyled form -> clean)
- [x] F.6 - Nav / admin-chrome cleanup: move the +/new-page box out of the nav, fix admin overflow
- [x] F.7 - F e2e + docs (content-model: page_title; admin-UX flows)
- [x] F.8 - Title rendering: one weighted H1 (display_title), strip the leading markdown H1, breadcrumb = ancestors only

