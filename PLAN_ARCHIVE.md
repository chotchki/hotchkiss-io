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

---

## 2026-06-25

## Phase G - Reliability hardening (launchd respawns, so crash-loop/correctness focus)

- [x] G.0 - Phase exit: coordinator + cert/DNS path self-heal; no single transient error or panic can wedge/kill the live site; AVIF + session-unwrap correctness bugs fixed
- [x] G.1 - Coordinator loops self-heal (ACME/DNS/session-GC: match-log-continue like backup.rs)
- [x] G.2 - Replace the todo!() LE-authz panic with bail! + cap the unbounded order_cert loop
- [x] G.3 - Timeouts on the cert/DNS path (reqwest timeouts + re-enable DnsValidator deadline)
- [x] G.4 - Fix the AVIF resize integer-division bug (blank landscape thumbnails) + test
- [x] G.5 - SQLite busy_timeout + remove the session-read unwrap()
- [x] G.6 - Reliability tail: TLS-config reload, build.rs migrations rerun-path, CertificateDao tests, WARN-log hygiene

---

## 2026-06-25

## Phase BT - Deferred polish

- [x] BT.1 - Tab redesign (chris's wife to opine on look + behavior)
- [x] BT.2 - Dedicated /admin/pages editor + nav rework (+ icon, fold admin into hamburger, fix tab styling)

---

## 2026-06-26

## Phase BU - Blog image UX + relative-link rewrite (dogfooding the first image post)

- [x] BU.0 - Phase exit: page/blog images render capped + click-to-zoom (diagram lightbox); site-absolute links + image srcs rewritten relative on save; live on prod
- [x] BU.1 - rewrite_site_links(markdown, domain): Link + Image URLs matching the site host → relative (preserve path/query/fragment); unit tests
- [x] BU.2 - Add domain to AppState (from Settings); wire the link rewrite into the page-save path (put_page_path) on save
- [x] BU.3 - Images capped + zoomable: transformer non-.stl image → sized+zoomable <img> (mirror diagram wrap); broaden diagram-zoom.js selector; update image_link test
- [x] BU.4 - Tests (unit + web) green + CLAUDE.md/SPEC docs update
- [x] BU.5 - Deploy: push main (beta) → verify on beta → tag vX.Y.Z (prod)

---

## 2026-06-26

## Phase BV - Content rendering — typeset math (KaTeX) + code syntax highlighting

- [x] BV.0 - Phase exit: content pages render typeset math (KaTeX) + syntax-highlighted code; TeX/code source stays in the served HTML (no-JS/LLM-readable); live on prod
- [x] BV.1 - Typeset math: enable markdown-rs math_text/math_flow + emit TeX-carrying spans in transformer; vendor KaTeX (CSS/JS + autorender) on content+blog pages; render client-side incl HTMX-swapped; tests
- [x] BV.2 - Code syntax highlighting: vendor highlight.js (CSS/JS), auto-highlight <pre><code class=language-*> on load + HTMX swap; site-matching theme
- [x] BV.3 - Tests (web: math + highlighted code render, source-in-HTML) + CLAUDE.md/SPEC docs
- [x] BV.4 - Deploy: push main (beta) → verify → tag vX.Y.Z (prod)

---

## 2026-06-26

## Phase BW - GFM tables + fix nested-element rendering (BV walk-depth regression)

- [x] BW.0 - Phase exit: content pages render GFM tables + math/images/diagrams nested in lists/headings/blockquotes (BV walk-depth regression fixed); live on prod
- [x] BW.1 - Fix transformer walk to descend into ALL containers (lists/headings/blockquotes/emphasis/links), not just Root/Paragraph — math/images/diagrams nested anywhere now convert; test
- [x] BW.2 - GFM tables: enable gfm_table in to_mdast + the to_html re-parse so | a | b | renders as a table; test
- [x] BW.3 - Docs (CLAUDE.md/SPEC) + deploy beta → verify → tag vX.Y.Z (prod)

---

## 2026-06-27

## Phase BX - Blog post next/previous navigation cards

- [x] BX.0 - Phase exit: blog posts show next/previous cards to adjacent posts; omitted at the ends; absent on /pages; live on prod
- [x] BX.1 - PostNavCard + Option prev/next on GetPageTemplate; render nav section in get_page.html (blog-only, compact, omit a side at the ends)
- [x] BX.2 - show_post computes older/newer siblings + builds the cards; get_page_path passes None
- [x] BX.3 - Tests (web) + CLAUDE.md docs
- [x] BX.4 - Deploy beta → verify → tag vX.Y.Z (prod)

---

## 2026-06-27

## Phase BY - Graceful deploy restart (kill the mini crash dialogs)

- [x] BY.0 - Phase exit: deploys leave no "quit unexpectedly" dialogs on the mini (graceful SIGTERM restart); verified on the mini
- [x] BY.1 - post-receive: graceful SIGTERM restart instead of kickstart -k (SIGKILL); re-copy the hook to the mini
- [x] BY.2 - Verify on the mini; add an app SIGTERM handler if the tray app doesn't exit cleanly

---

## 2026-06-28

## Phase CA - Custom cat 404 page

- [x] CA.0 - Phase exit: a 404 (unmatched route OR missing /pages/* path) renders the 3-cat "Which one is guilty?!" page; tap-to-blame quip overlay; back-home link; web-optimized AVIF; both paths tested
- [x] CA.1 - Re-encode the 3 cat photos: resize to a uniform web size + AVIF (avifenc); drop the raw JPEGs from embedded assets
- [x] CA.2 - templates/404.html: 3 cat cards, "Which one is guilty?!" header, per-cat quip overlay slots, back-to-/ link; mobile-first
- [x] CA.3 - Shared render_not_found helper; route global fallback (move before with_state) + get_page_path None branch through it
- [x] CA.4 - Pure-Tailwind tap-to-blame reveal (native <details> + group-open:, NO JS, NO custom CSS); chris's real quips inline with a clear edit spot
- [x] CA.5 - Integration tests: both 404 paths (unmatched route + /pages/<missing>) → 404 + cat/quip markers; CLAUDE.md updated. (Reveal is native <details> — no custom JS to e2e.)

---

## 2026-06-28

## Phase BZ - Self-hosted large-media store (disk + HTTP range)

- [x] BZ.0 - Phase exit: UNIFIED media — disk content store + media/variant schema; uniform ![](/media/<ref>) authoring + render-time polymorphic dispatch (img/video/stl); range route; ffprobe-typed media library UI; existing attachments migrated off BLOBs + /attachments retired; backup-correct; beta→prod verified
- [x] BZ.1 - MediaStore content-addressed disk primitive (sharded ab/cd/<sha>, atomic write, dedup, traversal-guarded) + Settings.media_path
- [x] BZ.2 - Range byte route /media/file/<url_key> (206/Accept-Ranges, immutable cache, HMAC token, mime from variant)
- [x] BZ.3 - Backup/ops: media dir excluded from the beta snapshot, added to daily backup + Backblaze; tests + docs
- [x] BZ.4 - media + media_variant schema (migration) + typed MediaDao (kind enum, variants); unit tests
- [x] BZ.5 - Ingest: ffprobe-derive codec/mime/dims/duration per upload, store variants; auto-poster (ffmpeg frame→AVIF)
- [x] BZ.6 - Transformer dispatch: ![](/media/<ref>) → /media/embed HTMX swap → <img>/<video multi-source>/<object stl> (diagram-style, low-risk vs transform() refactor)
- [x] BZ.7 - Central media library /admin/media (drag-drop grouped upload, codec chips, copy-ref, delete; admin-gated; JSON ingest endpoint)
- [x] BZ.8 - One-shot migration: attachment BLOBs → store + media rows + rewrite page refs /attachments→/media + re-home page_cover_attachment_id → media; verify on beta; retire /attachments + drop table
- [x] BZ.9 - Inline editor media upload (async drag-drop onto the textarea + toolbar button, insert ![](/media/ref) at cursor, NO refresh — fixes the save_attachments lose-edits bug)
- [x] BZ.10 - Merge / add-encode: associate a separately-uploaded video encode into an EXISTING media item (today merge needs all encodes in one simultaneous drop). Per-video "add encode" action and/or prompt-to-merge on video upload.
- [x] BZ.11 - Thumbnails / posters: auto frame-grab a video poster (ffmpeg → AVIF) shown on the library card AND as <video poster=…>; a real thumbnail for every kind in the library grid.
- [x] BZ.12 - Rename media: edit title freely; ref rename is riskier (breaks existing ![](/media/oldref) embeds) — gate it (ref editable only until referenced, or rewrite references like the BZ.8 migration).

---

## 2026-06-28

## Phase CB - Prod feedback: mobile analytics, unified feed, SEO

- [x] CB.0 - Phase exit: /admin/analytics usable on a 390px phone (tables + widgets visible, no page-wide horizontal scroll); /feed.xml carries blog posts AND project pages newest-first; /sitemap.xml + a Sitemap-directive robots.txt live with per-page meta description/canonical/OpenGraph; beta de-indexed; verified on beta then prod
- [x] CB.1 - Analytics dashboard mobile-responsive: wrap every data table in overflow-x-auto, wrap the stat + range/metric/paths chip rows (flex-wrap), break long UA/referer strings; confirm the SVG chart scales
- [x] CB.2 - Unified /feed.xml (blog posts + project pages, newest-first, retitled to chris's name); /blog/feed.xml keeps serving it (back-compat); base.html link rel=alternate → /feed.xml; factor feed render into web/features/feed.rs
- [x] CB.3 - SEO discovery: dynamic /sitemap.xml (home + top-level pages + blog posts + projects + /resume, lastmod from page_modified_date, absolute URLs from request host) + dynamic /robots.txt (Sitemap: directive; Disallow non-canonical hosts e.g. beta)
- [x] CB.4 - Per-page meta: base.html baseline meta description + canonical + OpenGraph/Twitter via an askama seo macro; enrich get_page (pages/blog/resume), blog index, projects list with page description (excerpt), absolute canonical URL, og:image (cover or site photo)
- [x] CB.5 - Tests + docs: e2e mobile-analytics + integration feed/sitemap/robots/meta; CLAUDE.md update (feed/sitemap/robots/meta + beta de-index); flag Google Search Console resubmission as chris's manual step

---

## 2026-06-28

## Phase CC - User management admin screen

- [x] CC.0 - Phase exit: an admin can list users (role + passkey/API-key counts) at /admin/users, promote/demote Registered↔Admin, and delete a user; the last admin can never be removed/demoted; role + delete take effect IMMEDIATELY on a live cookie session (per-request recheck); tested + documented; beta→prod verified
- [x] CC.1 - UserDao methods: list_summaries (display_name, id, role, passkey_count = json_array_length(keys), api_key_count = live api_keys), count_admins, set_role(id, role), delete(id) (DELETE the user's api_keys first then the row, in a tx — FK). Unit tests incl. delete cascade + count_admins
- [x] CC.2 - Live enforcement: refresh_session_role middleware (from_fn_with_state, layered INNER to api_key_auth) — for a cookie session that is Authenticated and has no api-key injection, re-load the user by id and inject the refreshed SessionData (updated role), or inject Anonymous if the user was deleted. Makes demote/delete immediate. Integration test: demote/delete reflected without re-login
- [x] CC.3 - /admin/users UI: web/features/admin/users.rs + templates/admin/users.html — list table (name, role badge, passkey/key counts), promote/demote (POST), delete (hx-delete + confirm); admin-gated under the /admin nest; last-admin guard enforced in handlers (block demote/delete of the final admin with a clear message); hub link from /admin/pages
- [x] CC.4 - Tests + docs: integration tests (list, promote/demote, last-admin guard blocks, delete cascades api_keys, deleted/demoted user loses access immediately, /admin/users admin-gated); CLAUDE.md user-management section; deploy beta→prod
- [x] CC.5 - Cookie hardening: make the session cookie's HttpOnly + Secure explicit on SessionManagerLayer (HttpOnly always; Secure in release — debug/test is plain HTTP so it stays off there by design), set SameSite explicitly; integration test asserts Set-Cookie carries HttpOnly + SameSite. The session cookie is the only cookie the app sets (tower-sessions)

---

## 2026-06-28

## Phase CD - SEO regressions: HTTP/2 host + project URLs

- [x] CD.0 - Phase exit: prod sitemap/robots/feed emit the REAL host over HTTP/2 (not localhost) and project entries link the working /pages/projects/<slug>; beta de-indexes under h2; verified on prod over both h1.1 + h2
- [x] CD.1 - HTTP/2 host detection: web/util/host.rs request_host(headers, uri) = Host header ?? uri :authority ?? localhost (h2 puts the host in :authority, not a Host header) + request_scheme(); use it in feed.rs + seo.rs sitemap/robots so prod over h2 emits the real host. Unit-test the helper (Host header, :authority fallback, neither)
- [x] CD.2 - Project detail URL fix: feed + sitemap link project entries at /pages/projects/<slug> (the real route), NOT /projects/<slug> (404); blog stays /blog/<slug>, /projects index stays. Fix the two CB tests that enshrined the wrong URL
- [x] CD.3 - Verify + deploy: full test suite green; ship beta→prod (vX); confirm on prod over BOTH --http1.1 and --http2 that sitemap/robots/feed emit hotchkiss.io + project links are /pages/projects/<slug> + 200; CLAUDE.md note on the h2 :authority gotcha

---

## 2026-06-28

## Phase CE - Editable post date (backdating)

- [x] CE.0 - Phase exit: a page's post date (page_creation_date) is editable in the editor + over the API, so a Wayback-recovered post can be backdated 10+ years and lands at its real chronological spot on /blog with its real date; modified-date stays auto; tested; beta→prod
- [x] CE.1 - Make page_creation_date editable: ContentPageDao::update writes page_creation_date (+ creation_date_input() helper formatting it for datetime-local); PutPageForm gains optional page_creation_date (parse YYYY-MM-DDTHH:MM:SS, empty/unparseable → keep existing); put_page_path applies it before update. Integration test: a PUT backdates a page (and reorders /blog)
- [x] CE.2 - Editor UI: a "Posted" datetime-local input (step=1) in get_page.html prefilled with the current creation date; CLAUDE.md note that post date is editable (backdating); deploy beta→prod

---

## 2026-06-28

## Phase CF - Wayback blog import

- [x] CF.0 - Phase exit: 16 selected Wayback posts (technical 2,3,7,8,9,11,15,16,17 + personal 1,4,5,6,10,12,13) are live on /blog, each backdated to its REAL byline date (day precision), body converted from the archived HTML to clean markdown; chris polishes text on-site after
- [x] CF.1 - Scrape + convert: for each selected post, fetch the Wayback raw capture (id_), extract title (h2#single-title), real date (byline "on Month Dayth, Year"), and body (between byline and post-bottom-meta) → pandoc to markdown. Save per-post {title, date, markdown}; validate the netcat sample + the title/date table before publishing
- [x] CF.2 - Publish each post via the API: POST /pages/blog (title → slug) then PUT /pages/blog/<slug> with the markdown + page_creation_date set to the real byline date (backdated). Verify all 16 land on /blog dated correctly (oldest at the bottom)

---

## 2026-06-28

## Phase CG - Harden against content-triggered panics

- [x] CG.0 - Phase exit: content can't crash a request or the feed — transform() catches panics + degrades to escaped source; the markdown table-slice is char-boundary-safe (markdown-rs offsets are CHAR offsets, not bytes); a CatchPanicLayer turns any handler panic into a 500 (not a 000 connection reset); regression tests; beta→prod
- [x] CG.1 - transform() hardening: char-safe table slice (markdown.chars().skip(s).take(e-s), NOT byte &markdown[s..e]) + wrap transform in catch_unwind → on panic, log + return escaped-source fallback so a page/feed degrades, never crashes. Regression tests: a table after a smart-quote line + the netcat perl content → transform returns Ok
- [x] CG.2 - CatchPanicLayer on the router (outermost) → any handler panic returns a 500 (styled if cheap) instead of a dropped connection; integration test (a debug-only panic route → 500, not a reset). CLAUDE.md note. Deploy beta→prod

---

## 2026-06-28

## Phase CH - Blog pagination + search

- [x] CH.0 - Phase exit: /blog paginates (N newest per page, prev/next, no-JS, mobile) AND supports text search (?q= over title + body); the two compose (?q=…&page=N); server-rendered; tested; beta→prod
- [x] CH.1 - Pagination: ContentPageDao gains count_children(parent) + a paged find (LIMIT/OFFSET on find_by_parent_newest_first); /blog?page=N (1-indexed, PAGE_SIZE const ~10); prev/next links at the bottom (no-JS, mobile, omitted at the ends); blog index handler + template. The Atom feed stays full (newest 50) — feed paging is out of scope
- [x] CH.2 - Search: a GET search box on /blog (?q=) filtering posts by title + markdown. Decide mechanism — LIKE %q% is adequate at this scale (recommend); SQLite FTS5 (virtual table + sync triggers, ranked) is the noted upgrade if the corpus grows. Echo the query + a result count + a clear-search link; results render as the same cards; composes with ?page=N
- [x] CH.3 - Tests + docs + deploy: integration tests (page boundaries + prev/next presence/omission, search hit/miss/empty-q, ?q=&page=N composition), an e2e mobile check that /blog paginates with no horizontal scroll; CLAUDE.md blog section update; beta→prod

---

## 2026-06-29

## Phase CL - Media review hardening

- [x] CL.0 - CL.0 - Phase exit: media review hardening (M1-M3, L1) shipped beta→prod
- [x] CL.1 - CL.1 - M2: byte route nosniff + force-download active-content mimes (XSS)
- [x] CL.2 - CL.2 - M3: resolve_path/pick_write_root off the async runtime (spawn_blocking)
- [x] CL.3 - CL.3 - M1/L1: pick_write_root readiness (no phantom mountpoint) + per-root fall-through; roots_status shares the probe
- [x] CL.4 - CL.4 - Tests + docs + ship beta→prod

---

## 2026-06-29

## Phase CM - CM - Scrub career-private docs + rewrite history for public re-mirror

- [x] CM.0 - Phase exit: docs de-personalized + history rewritten clean for public re-mirror
- [x] CM.1 - De-personalize SPEC.md + PLAN.md (remove employment-context; keep product spec); commit
- [x] CM.2 - Rewrite history (git-filter-repo replace-text + replace-message); verify clean + code unchanged
- [x] CM.3 - Push gate: public-mirror branch/tag scope + mini reconciliation (deploy-aware)

---

## 2026-06-30

## Phase CP - CP - Stable code signing (Developer ID) for TCC/FDA persistence

- [x] CP.0 - Phase exit: prod + beta are signed with chris's Apple Developer ID (a stable identity), so the Full Disk Access (TCC) grant survives every deploy — grant FDA once, never re-grant after a push; verified across two consecutive deploys with MediaStorage4-hosted media serving throughout (no re-grant, no 25s hang). Signing-IDENTITY only — Apple notarization + .pkg stay retired
- [x] CP.1 - build.sh: sign with the Developer ID Application cert instead of ad-hoc `codesign -s -`. Resolve the identity from $SIGN_IDENTITY (the "Developer ID Application: … (TEAMID)" string) with ad-hoc as the FALLBACK when it's unset/absent (so a cert-less dev or CI build still works). Both --profile beta + prod sign with the same cert. Minimal — no hardened runtime / notarization (not needed without public download distribution; this is for TCC identity stability only)
- [x] CP.2 - One-time mini keychain setup (build/macos/SETUP.md): import the Developer ID Application cert + private key into the keychain the post-receive build uses, and make NON-INTERACTIVE codesign work from the ssh/post-receive context (no GUI) — unlock the keychain + `security set-key-partition-list -S apple-tool:,apple: -s -k <pw> <keychain>` so codesign signs without a prompt. The real gotcha: a `git push` build runs OUTSIDE the GUI session, so the signing key must be reachable + ACL-allowed headless
- [x] CP.3 - Grant Full Disk Access ONCE to the Developer-ID-signed app, then verify the grant PERSISTS across a deploy: push twice and confirm MediaStorage4-hosted media keeps serving with no re-grant + no 25s hang. Docs: CLAUDE.md + SETUP.md (signing is now Developer ID for durable TCC; notarization/.pkg still retired; FDA is a one-time grant). Ship beta→prod

---

## 2026-06-30

## Phase CO - CO - Admin log viewer (prod ops visibility)

- [x] CO.0 - Phase exit: an admin reads recent server logs at /admin/logs (tail of Settings.log_path, level filter, manual refresh) WITHOUT the page feeding its own log (no infinite loop); the backfill-style silent no-op would now be one click to see; verified beta→prod
- [x] CO.1 - Log tail reader: a bounded tail of the newest log file under Settings.log_path (last ~N lines / cap the bytes read — never slurp a multi-GB log), newest-first, run in spawn_blocking so a log on a slow/asleep disk can't pin a tokio worker. Handle tracing's rotation (read the current/most-recent file). Returns the lines + a level filter applied
- [x] CO.2 - /admin/logs page: admin-gated under the /admin nest's require_admin, server-rendered <pre> of the tail + a level filter (?level=error|warn|all) + a MANUAL refresh link (no aggressive auto-poll). NO INFINITE LOOP: manual refresh is the primary defense, AND exclude /admin/logs from the request_log middleware + drop the route's own access lines from the displayed tail, so viewing the log never feeds the log. Hub link from /admin/pages
- [x] CO.3 - Tests + docs + deploy: integration (admin-gated 403 for anon; renders the tail; the /admin/logs route is excluded from request_log so a self-view doesn't appear in the analytics/tail; level filter narrows). CLAUDE.md note (admin log viewer + the no-self-feed design). Ship beta→prod

---

## 2026-07-01

## Phase 11 - Better local testing

The Phase 10 dogfood loop revealed two structural gaps: (1) mobile/responsive bugs slipped through because no test exercised a phone-sized viewport, and (2) every fix required a push-to-deploy round-trip because the dev server isn't reachable from a real phone with HTTPS (hardcoded `:80`/`:443`, no LAN story). Closing both.

Dev-HTTPS strategy: dev runs as a debug build, which already routes ACME at LE staging (`instant_acme.rs:42-47`). A separate Cloudflare token scoped to the `hotchkiss.io` zone gives the dev box DNS-01 access for `beta.hotchkiss.io`; the beta A record is grey-clouded at the dev box's LAN IP. One-time iPhone setup: install the LE staging root CA profile so iOS trusts the cert (PWA install requires a fully trusted cert, not just a bypassed warning). `ServiceCoordinator`'s `IpProviderService` gets a `static_ip` option so dev doesn't broadcast 127.0.0.1 (its current debug default).

**Folded into Phase 12 (2026-06-22).** The reusable code shipped — 11.1–11.2 (configurable ports + `static_ip`) and 11.4–11.7 (mobile-viewport e2e). The dev-box-on-LAN + LE-staging-cert + iPhone-profile path (11.3 / 11.8 / 11.9) is superseded by Phase 12's beta-on-the-mini approach (real LE-prod cert, natively trusted, no profile install) and absorbed into 12.6 / 12.9 / 12.10 — see 12.11.

- [x] 11.0 - Phase exit (superseded): code portions shipped; the dev-box/LE-staging/profile path is replaced by Phase 12's beta-on-mini.
- [x] 11.1 - Make `:80` / `:443` configurable in `Settings` (`http_port` / `https_port`, defaulting to 80/443). Update `endpoints_provider_service.rs` to bind the configured ports.
- [x] 11.2 - Add a `static_ip` option to `Settings`. When set, `IpProviderService` skips the cdn-cgi/trace poll and broadcasts the static IP immediately, overriding the existing `debug_assertions` 127.0.0.1 default.
- [x] 11.3 - (Folded into 12.6 / 12.7 / 12.9.) `docs/dev-https.md`: one-time Cloudflare API token scoped to the `hotchkiss.io` zone, grey-cloud `beta.hotchkiss.io` A record at the dev box's LAN IP, sample `dev-config.json` (`domain="beta.hotchkiss.io"`, dev CF token, `static_ip`, dev ports), how to install the LE staging root CA profile on iPhone for PWA install. Note: debug build automatically uses LE staging — no code knob to flip.
- [x] 11.4 - `tests/e2e_browser.rs` mobile-viewport setup: launch Chrome with 390×844 viewport, retina device-scale-factor. Factor out into a helper so individual tests can opt into mobile vs desktop.
- [x] 11.5 - e2e: `/blog` and a representative content page have no horizontal scrollbar at 390px wide (`document.documentElement.scrollWidth <= window.innerWidth`). Captures the page-min-width-exceeds-portrait regression.
- [x] 11.6 - e2e: top nav `<ul>` doesn't overflow at 390px. Captures the nav-cut-off regression.
- [x] 11.7 - e2e: admin sees the "+ New post" form on `/blog`; submitting a slug with spaces succeeds (no silent 400) and the slug input live-slugifies as expected (typing "Hello world" leaves "hello-world" in the field, spacebar isn't eaten). Captures both Phase 10 dogfood fixes.
- [x] 11.8 - (Folded into 12.9.) CLAUDE.md update: document the new `http_port`/`https_port`/`static_ip` settings; note the dev-HTTPS recipe; mention the existing debug→LE-staging routing so dev never burns prod cert quota.
- [x] 11.9 - (Folded into 12.10.) Manual e2e: bring up a phone-reachable HTTPS surface, install the PWA from a phone, edit a template, refresh phone, confirm the change is live without a deploy.

## Phase CA - API key authentication

- [x] CA.0 - Phase exit: a user can generate/revoke API keys in /admin; an `Authorization: Bearer hio_…` key authenticates as that user (HMAC-pepper hashed) across all routes; tested + documented
- [x] CA.1 - api_keys schema + ApiKeyDao + HMAC-pepper hashing (crypto_keys id 3) + key generation (hio_<base64url>); unit tests
- [x] CA.2 - Auth resolution in SessionData extractor: Authorization: Bearer (axum-extra TypedHeader) → live-key lookup → Authenticated(user) + stamp last_used; session fallback; integration test
- [x] CA.3 - Admin UI /admin/api-keys: generate (label → key shown ONCE), list (label/created/last-used), revoke; admin-gated; CLAUDE.md docs

## Phase CI - Large-file streaming upload + shareable link

- [x] CI.0 - Phase exit: large-file streaming upload + shareable link
- [x] CI.1 - MediaStore::store_stream (chunks → temp → atomic rename, incremental SHA-256)
- [x] CI.2 - Stream the upload handlers (drop the Vec<u8> buffering)
- [x] CI.3 - Graceful generic-file ingest → MediaKind::File
- [x] CI.4 - Share-link UX in the media library
- [x] CI.5 - Tests + docs + deploy

## Phase CJ - Multi-drive media storage

- [x] CJ.0 - Phase exit: multi-drive media storage
- [x] CJ.1 - Config: media_paths ordered list + free-space headroom
- [x] CJ.2 - Schema: media_variant.storage_root hint column (migration 0017)
- [x] CJ.3 - MediaStore multi-root: resolve (hint+scan), pick-write-root by free space
- [x] CJ.4 - Wire ingest + serve + all path_for callers through resolve_path
- [x] CJ.5 - Beta mirror: rsync iterates roots
- [x] CJ.6 - Tests + docs + deploy
- [x] CJ.7 - Storage panel: show media roots + free space

## Phase CK - Upload progress

- [x] CK.0 - Phase exit: media uploads show real progress
- [x] CK.1 - media-upload.js: XHR upload + progress bar (drop-zone + add-encode)
- [x] CK.2 - editor-support.js: XHR upload + progress for inline media drop
- [x] CK.3 - CK tests + docs + deploy

## Phase CN - CN - Performance: render path + responsive images

- [x] CN.0 - Phase exit: mobile LCP < 2.5s; PSI render-blocking (~1.9s) + image-delivery (~434 KiB) + font-display (~90ms) insights cleared — FontAwesome dropped for build-time-generated inline SVG, htmx deferred, @font-face font-display:swap; content images served width-stepped AVIF via srcset (existing images backfilled); versioned static assets cached a year; verified beta→prod via fresh PSI run
- [x] CN.1 - Drop FontAwesome via build-time icon codegen: a build.rs step takes a declared list of the ~19 used icons (arrow-left/right, bars, cloud-arrow-up, cube, cubes-stacked, file, file-pdf, floppy-disk, grip-vertical, house, image, link, magnifying-glass, pen, pen-nib, photo-film, plus, trash-can) and generates inline SVG (an icon("pen") macro / fn returning <svg>), sourced from FA Free solid SVGs (vendored or pin-downloaded like the Tailwind CLI). Replace <i class="fa-…"> across the 9 templates; remove fontawesome.css + solid.css from base.html <head> and drop the FA webfonts from assets. Kills the 154.71 KiB fa-solid-900.woff2 + ~18 KiB CSS + two critical-chain hops — biggest single LCP lever. Declarative list → tool generates, no hand-copied SVG blobs
- [x] CN.2 - defer htmx: add defer to base.html:30 script so it leaves the critical path (16.7 KiB / ~760ms). Verify nothing in {% block head %} or inline scripts calls htmx.* at parse time — the vendor inits (webauthn, katex-render, code-highlight, diagram-zoom, htmx-stl-view) are event-driven so deferring is safe; confirm load order with the other deferred scripts
- [x] CN.3 - font-display: swap on the @font-face for Oswald + Quattrocento (styles/tailwind.css:15-22) so text paints immediately instead of FOIT (~90ms, no invisible text). Optional/deferred: a build-time Latin subset of the two woff2s to shave 31+38 KiB off the critical chain
- [x] CN.4 - Static-asset cache TTL: in static_content.rs serve versioned (?cb=) requests Cache-Control: public, max-age=31536000, immutable; keep a short TTL (or the current 86400) for un-versioned paths (favicon, manifest, apple-touch-icon). Gate on uri.query().is_some(). Honest note: a repeat-visit win — Lighthouse measures cold load so it won't move the PSI score, but it clears uses-long-cache-ttl and cuts real return-visit bytes
- [x] CN.5 - Migration 0018: media_variant.width/height (nullable INTEGER) so each image variant records its own pixel width for a srcset Nw descriptor. (media.width/height already holds the item's ORIGINAL dims; per-variant width is what srcset needs and is new.)
- [x] CN.6 - Resize/transcode on image ingest: generate width-stepped AVIF variants (e.g. 480/960/1440, skipping any width >= source) reusing poster.rs's resize + ImageFormat::Avif path (image crate; if in-process rav1e is too slow on multi-MB uploads, shell to the existing ffmpeg like the poster frame-grab does). Store each as a media_variant with width/height recorded; keep the original variant as the fallback. Dedup by sha across roots like every other variant
- [x] CN.7 - Responsive render: the /media/embed Image branch (media.rs render_media_embed) emits <img srcset="…480w,…960w,…1440w" sizes="…"> from the width-stepped AVIF variants, original as the src fallback, keeping data-zoomable + the 480px max-height. Same for blog/project card covers (cover_url_for). No-JS still gets a working single src
- [x] CN.8 - Backfill: a one-shot startup task (coordinator, modeled on the retired migrate_media.rs) generates the width-stepped AVIF variants + records widths for EXISTING image media. Idempotent (dedup by sha, skip if variants already present), backup-first, per-item non-fatal (log + continue so one bad image can't abort boot or trip the coordinator try_join)
- [x] CN.9 - Tests + docs (deploy/PSI verify → CN.10): unit (resize target_widths + real AVIF downscale, srcset render, cache_control policy), integration (icon-codegen inline <svg> on the 404 page; BX prev/next tests re-anchored off the removed fa- classes), CLAUDE.md (build-time icon codegen, cache TTL + htmx defer + font-display, responsive-image pipeline + 0018 + backfill). Full suite green, clippy clean (bar one pre-existing api_key_auth warning)
- [x] CN.10 - Ship CN beta→prod + verify: git push origin main (→ beta), eyeball beta.hotchkiss.io, then tag vX.Y.Z + push (→ prod). Re-run PageSpeed on hotchkiss.io (mobile) to confirm LCP < 2.5s + render-blocking (~1.9s) / image-delivery (~434 KiB) / font-display (~90ms) insights cleared. HELD for chris: keyless PSI is quota-0 (needs your read of the report), and a prod deploy is a deliberate tag push. The background responsive-image backfill runs on first CN boot (logged); give it a few minutes before judging image delivery
- [x] CN.11 - FIX (CN regression): the responsive backfill skipped every attachment-migrated cover because media.width is NULL — backfill_one bailed on `let Some(src_w) = m.width`. Derive the source width from the DECODED image instead (responsive_avif_variants reads img.width(), returns ResizeResult{source_width,source_height,variants}); stamp the original variant from those true dims. Re-ship beta→prod; covers then serve the 480 AVIF (was 60–166 KB JPEG/PNG)
- [x] CN.12 - Harden the media byte route with a timeout: a flaky/blocked media root (external volume unmounted, TCC-blocked, or wedged) currently HANGS the serve for 25s+ (the v0.0.81 MediaStorage4 incident — AVIFs on the primary SSD root were unreadable by the LaunchAgent). resolve_path is already off the tokio worker (M3 spawn_blocking), but the request still waits. Wrap the resolve (and consider the ServeFile body) in a tokio::time::timeout → fast 503/404 instead of a hang. Note: a timed-out spawn_blocking keeps running (can't cancel a blocking stat), so a permanently-wedged root could still leak blocking threads — log loudly so the log viewer surfaces it
- [x] CN.13 - Right-size the static page images PSI still flags (embedded assets, NOT /media — outside the responsive pipeline): the jumbotron Photo.avif is 640×640 but renders at 160px (size-40), ~2x oversized. Resize to ~320px AVIF (clean 2x for a 160px display; bump if 3x crispness wanted) — same approach as the 404 cats (Phase 39). HotchkissLogo is SVG (fine). Saves ~25-30 KB on every page + helps LCP (the headshot is an early-paint element)

---

## 2026-07-01

## Phase CQ - Analytics: separating signal from noise (sources + performance)

- [x] CQ.0 - Phase exit: audience (human/bot) + status/noise + per-IP + referer + latency all live on /admin/analytics behind require_admin; d3 line chart shipped; cargo test green (unit + reqwest integration + chromiumoxide e2e); CLAUDE.md updated; swept to PLAN_ARCHIVE
- [x] CQ.1 - Shared foundation: migration 0019 duration_ms (nullable INTEGER, no index) + capture it in the fire-and-forget middleware (Instant before next.run, saturating i64 ms after); NewRequestLog/insert 6→7 cols, recent() projection gains it; fix entry()/seed() helpers; cargo clean -p hotchkiss-io for sqlx; docstring HONESTLY (server-handler time NOT client LCP, under-counts streaming bodies)
  - [x] CQ.1.1 - Fold the panic-logging fix into capture: reorder log_requests OUTER to CatchPanicLayer in router.rs so a caught handler-panic 500 is observed + recorded (it was invisible — the panic unwound past the post-next.run insert); extend handler_panic_becomes_a_500 test to assert the /test/panic 500 lands in request_log
- [x] CQ.2 - Bot/human view + audience toggle: migration 0020 request_log_view (ua_class via CAST(CASE ua null/empty/known-bot-substr THEN 'bot' ELSE 'human')); Audience enum (All/Humans/Bots); thread ?audience (bound OR-of-constant so the ts index still bounds the scan) through count_since/distinct_ip_count/count_by_day/distinct_ip_by_day/count_by_content_path; audience_counts(days)→{all,humans,bots} 3-chip; parse ?audience via clamp/unwrap (never ?-bubble → no 500 on bad param); test humans+bots==all
- [x] CQ.3 - Status + noise queries: count_by_status_bucket (2xx/3xx/403/404/4xx/5xx, 403+404 split out); status_by_day series; noisy_ips(window_cutoff, min_distinct_404, limit) GROUP BY ip WHERE ip IS NOT NULL → total/distinct_paths/distinct_404/errors, ORDER BY total (VOLUME default), SCAN_DISTINCT_404_THRESHOLD=5 badge + secondary sort, window CUTOFF not days (blocklist reuse seam); never_succeeded_paths (HAVING SUM(status<400)=0); annotate conditional aggregates for sqlx; test NULL-ip poison + threshold boundaries
- [x] CQ.4 - Per-IP drill-down: GET /admin/analytics/ip/{ip} under the require_admin nest; ~4 scoped queries (header from noisy_ips, path+status, UA, recent) + derive 404-wordlist/status-mix in Rust; parse ip→400 on garbage via Ok((BAD_REQUEST,_)), no rows→200 empty-state (never ?-bubble); templates/analytics/ip_detail.html; leaderboard rows link in; integration test (200 valid / 400 garbage / anon rejected)
- [x] CQ.5 - Referer normalization + smart grouping: web/util/referer.rs normalize_referer(raw, site_host) via url::Url::host → Direct/Malformed/IpLiteral(Host::Ipv4|Ipv6)/Internal(site+www+beta)/Domain(host_key, category); strip www./m./amp., NO psl dep; RefererCategory suffix→category slice (amend-here like the content-path exclusion set); group_referers fold → top_external (trunc 25), by_category chip, noise_count, direct_count (COUNT WHERE referer IS NULL); referer_urls_since(days) UNBOUNDED (accurate noise count); RETIRE count_by_referer; tests: hotchkiss.io.evil.com→External (fixes the LIKE bug), IP-literal/mailto/garbage quarantined
- [x] CQ.6 - Latency analytics (server-processing time, honest label): web/util/route.rs normalize_route longest-prefix-first mirror of the axum router (documented drift risk, pinned unit tests); latency_samples(days) WHERE duration_ms NOT NULL with an exclusion set that KEEPS /diagram + /media; slowest_requests(days,limit); nearest-rank percentile helper over &[i64] (n=1/even/odd/empty tested) → route/count/p50/p95/max sorted by p95 (p99 computed NOT displayed); 'Server response time' section, honest 'not client page-load/LCP' copy + graceful 'no timing data' empty-state
- [x] CQ.7 - Dashboard consumer + d3 line chart + control-model fix (DECIDED: port d3): vendor d3@7 UMD (assets/vendor, deferred, dashboard head only) + port renderLineChart/JSON-island/hydrate from recon-gen; shape_timeseries(total,unique,since) TYPED serde struct, UTC zero-fill continuous daily axis, Total+Unique overlay, \uXXXX-escaped island (XSS boundary — attacker path/UA/referer strings stay in auto-escaped askama TABLES); swap whole #analytics-content wrapper (hx-target/hx-select + hx-push-url), drop the separate /timeseries endpoint; exclude /admin/analytics from the request_log skip-prefix (self-feed guard); new sections: 3 audience chips (default All), status-bucket table, noisy-IPs leaderboard (drill links + scanner badge), referrers table (category chip + 'N polluting referers hidden'), latency tables; MEASURED-vs-INFERRED labels
- [x] CQ.8 - Tests: unit (shape_timeseries UTC zero-fill/two-series alignment/empty; normalize_route pinned; percentile nearest-rank edge cases); reqwest integration tests/web.rs (/test/login Admin → GET /admin/analytics renders new sections; audience toggle + /admin/analytics/ip/{ip} admin-gated, anon rejected); chromiumoxide e2e (admin login via virtual authenticator → GET /admin/analytics → assert path.linechart-line rendered via Runtime.evaluate)
- [x] CQ.9 - Docs: CLAUDE.md analytics section — new columns/view (duration_ms, request_log_view/ua_class), two-axis model (factual status / inferred agent), latency = server-processing-NOT-LCP semantics; per-IP noisy_ips window-cutoff blocklist-reuse seam, geo/ASN skip rationale, referer derive-at-query + the LIKE-bug fix; the named deferral triggers (batcher / rollup+histogram / stored referer_host / ip_group-on-v6) and the panic-not-logged known limit

---

## 2026-07-01

## Phase CR - Analytics performance — indexes, stored is_bot, parallel queries

- [x] CR.0 - Phase exit: /admin/analytics loads in well under ~0.5s at 300k+ rows (verified via the diagnostic harness); cargo test green; CLAUDE.md + SPEC updated; swept to PLAN_ARCHIVE
- [x] CR.1 - Covering indexes (migration 0021) — the biggest win: (path,ts,status), (ip,ts), (referer,ts), (user_agent,ts) so the GROUP-BY queries go index-only (no temp b-tree); confirm via EXPLAIN QUERY PLAN they use COVERING INDEX; note the write-amplification (4 more indexes on the fire-and-forget insert — acceptable at personal-site write rates)
- [x] CR.2 - Stored is_bot column (migration 0022) + single-source Rust classifier: port the 25 bot-UA substrings to a fn is_bot(ua)->bool used at write (NewRequestLog/insert) AND an idempotent startup backfill for existing NULL rows; switch the audience-threaded queries + audience_counts from request_log_view.ua_class to is_bot (SUM); add (ts,is_bot) or a covering index so audience_counts is index-only; DROP the now-unused request_log_view
  - [x] CR.2.1 - Admin recompute command: re-run is_bot(ua) over ALL rows on demand so the ruleset stays retunable despite being stored (addresses CQ's frozen-classification concern) — a POST /admin/analytics/reclassify-bots or equivalent, admin-gated, in spawn_blocking
- [x] CR.3 - Parallelize show_analytics: try_join!/join_all the independent read queries (WAL + pool≤10 → concurrent readers) so wall-clock ≈ slowest query, not the sum of ~15; mind the shared-pool connection budget (don't starve concurrent site requests)
- [>] CR.4 - Trim redundant scans (measure-gated): fold count_since into audience_counts.all + combine any queries where one windowed scan yields multiple aggregates; only if the diagnostic still shows it matters after CR.1-CR.3
- [x] CR.5 - Verify + tests + docs + deploy: re-run the diagnostic harness to confirm <0.5s at 300k rows; unit/integration (is_bot classify parity with the old view rules, backfill idempotent, recompute, parallel path correctness); CLAUDE.md + SPEC update (indexes, stored is_bot + recompute, query parallelization, view dropped); ship beta→prod

## Phase 16 - Resume / background capture — DONE (résumé live at /resume + /resume.pdf; reconciled 2026-07-01)

See SPEC.md Pillar 3. The substance and the long pole: making less-visible work credible, not just recording it. Shipped: the résumé is authored + live, rendered from single-source markdown for both the web view and the weasyprint-generated PDF. (Was structurally orphaned in PLAN.md's Backlog by the original layout; reconciled + swept here.)

- [x] 16.0 - Phase exit: `/resume` renders a clean, current resume; a downloadable PDF is one click away; the background is captured in a reusable, structured form.
- [x] 16.1 - Capture the raw history — interview/brain-dump the background (roles, scope, impact, highlights). DONE — the résumé chris authors at `/resume?edit` is the captured form.
  - [x] 16.1.1 - Mine the less-visible work for public-safe signal — architecture/problem writeups, scope/scale/impact at a safe level, anything already public.
- [x] 16.2 - Decide resume structure + narrative (chronological vs impact-led), what to lead with, public vs gated.
  - [x] 16.2.1 - Narrative strategy: meet the "lots of depth, little public proof" skeptic head-on + cross-link the résumé to the side projects as tangible evidence of range.
- [x] 16.3 - `/resume` as a content_page rendering the résumé markdown — the SINGLE source for both the web view and the PDF. SHIPPED (`web/features/resume.rs`, source-in-HTML so ATS / AI screeners parse it).
- [x] 16.4 - Downloadable PDF — GENERATE `/resume.pdf` from the same source server-side. SHIPPED (weasyprint, resolved `$WEASYPRINT_BIN`→brew→PATH, content-hash cached, fail-visible-not-500; `<base href>` so PDF links resolve absolute).
  - [x] 16.4.1 - Pick + vendor the HTML→PDF binary — `weasyprint` (installed on dev + mini + CI, like d2).
- [>] 16.5 - Tie the contact/CTA into the landing page (Phase 13) — FOLDED into Phase 13's 13.5 (the same "resume / hire me" landing-page CTA); lands with the landing page, not the résumé.
- [x] 16.6 - e2e coverage for `/resume` + PDF download; CLAUDE.md/SPEC update.

---

## 2026-07-01

## Phase CS - Feed + page render caching

- [x] CS.0 - Phase exit: /feed.xml served from a warm transform cache (~1.3s → sub-100ms), shared by all page renders; feed emits ETag/Last-Modified + honors conditional 304
- [x] CS.1 - render-cache module: content-hash-keyed in-memory transform (+ excerpt) cache, coherent with the diagram REGISTRY (process lifetime); unit tests for hit/miss/determinism
- [x] CS.2 - Wire cached transform/excerpt into feed.rs, pages/mod.rs, blog.rs, resume.rs, projects.rs index, seo.rs Meta
- [x] CS.3 - Fix cover-date bug: page_cover_media_id update must bump page_modified_date (route through update() or stamp the raw SQL)
- [x] CS.4 - ETag/Last-Modified + conditional 304 on /feed.xml (validator = host + max(page_modified_date) + entry count; skip body build on match)
- [x] CS.5 - Tests: transform-cache unit tests, feed 304 conditional-request integration test, cover-date bump test; existing feed test still green
- [x] CS.6 - CLAUDE.md update: document the transform/excerpt cache, feed ETag/304, and the cover-date fix

---

## 2026-07-01

## Phase CT - Analytics custom date-range picker

- [x] CT.0 - Phase exit: /admin/analytics supports a custom from/to datetime range (native datetime-local) threaded through all queries with an upper bound; presets still work; verified isolating a post-deploy window
- [x] CT.1 - Introduce Window{from,to} (concrete UTC SQLite-datetime bounds); refactor request_log.rs queries + noisy_ips from since_days/cutoff to ts >= ?from AND ts < ?to; keep ts-index usage
- [x] CT.2 - Handler: parse ?from=&to= (+ tz offset) in AnalyticsQuery, compute the Window (custom overrides preset; preset = now-Ndays..now); graceful fallback on bad input (never 500)
- [x] CT.3 - UI: datetime-local from/to GET form in dashboard.html within the CQ.7 control model (hidden paths/audience + tz offset; native submit = no-JS fallback; htmx swaps #analytics-content); show active range + clear-to-presets
- [x] CT.4 - Tests: Window bound predicate (request inside vs outside the range), custom overrides preset, bad from/to → default, existing analytics tests green
- [x] CT.5 - CLAUDE.md update: document the custom range picker + Window refactor + tz handling

---

## 2026-07-02

## Phase 13 - Landing page + portfolio spine

See SPEC.md "Portfolio — the three pillars". The landing page is the connective tissue: orient a visitor in seconds, route to the three pillars (Software / 3D / Resume).

- [x] 13.0 - Phase exit: a visitor grasps chotchki + reaches all three pillars
- [x] 13.1 - Decide landing-page IA: hero (name + one-line value prop + what I do), three pillar doors (Software / 3D / Resume), links out (GitHub, contact/email). Wireframe it in SPEC.
- [x] 13.2 - Implement the home page: replace the `/`→first-content-page redirect with a real landing template (or designate a landing content_page). Hand-rolled Tailwind, mobile-first.
- [x] 13.3 - Top-nav surfaces the three pillars; verify it doesn't overflow at 390px (the Phase-10 dogfood nav fix — confirm it already shipped or land it here).
- [x] 13.4 - Identity/jumbotron block stacks on narrow screens (confirm the dogfood min-width fix is shipped or land it here).
- [x] 13.5 - Clear contact + GitHub links and a "resume / hire me" call-to-action above the fold.
- [x] 13.6 - e2e (`tests/e2e_browser.rs`, mobile viewport): `/` renders the three pillar doors and has no horizontal scroll at 390px.
- [x] 13.7 - CLAUDE.md + SPEC update: document the real landing page replacing the `/` redirect.

- [x] 13.8 - Featured pinning via a category tag + Featured band above Latest

---

## 2026-07-03

## Phase CU - Scheduled / timed publishing

- [x] CU.0 - Phase exit: future-dated pages hidden from non-admins on every public read path; admin sees them inline + badged; Publish-now/Unpublish work; tests green; docs updated
- [x] CU.1 - Shared visibility predicate: is_scheduled/is_published on ContentPageDao + is_visible(page, is_admin) helper
- [x] CU.2 - Direct-serve gates (security-critical): get_page_path scans whole pages_path; show_post gates leaf + filters future prev/next siblings
- [x] CU.3 - Résumé gates: newest_resume_child picks newest VISIBLE child (drop LIMIT 1); add session_data to show_resume_pdf
- [x] CU.4 - Paginated list gates (SQL, count-consistent): viewer_is_admin + datetime()-normalized predicate through count_children + find_children_*_paged
- [x] CU.5 - Feed + sitemap gates (unconditional, no session): filter collect_entries at the chokepoint; guard sitemap's three content loops
- [x] CU.6 - Feed 304 correctness: fold the publish instant into Last-Modified so If-Modified-Since-only crawlers catch the go-live flip
- [x] CU.7 - Home + nav gates (role-conditional, admin sees inline): show_home retains published-or-admin before partition; TopBar filters top-level tabs
- [x] CU.8 - set_creation_date targeted DAO setter (stamps modified_date, unlike Pin) + unit test
- [x] CU.9 - Publish-now + Unpublish buttons, cloned from the Pin/feature button
- [x] CU.10 - Scheduled badges (cards + editor + admin reader view) + UTC Posted-field relabel/echo
- [x] CU.11 - Tests: unit (predicate boundary, set_creation_date, count-consistency) + integration (anon 404/absence, admin sees, résumé fallback, PDF gate, feed flip)
- [x] CU.12 - Docs + deploy: CLAUDE.md gate writeup; version bump; main->beta, verify, tag->prod

---

## 2026-07-03

## Phase CV - Hero images on post + project pages

added 2026-07-03.

- [x] CV.0 - Phase exit: any page with a cover renders a stacked hero on its detail view (largest AVIF variant + srcset); tests + docs; shipped
- [x] CV.1 - cover_hero_for helper: largest image variant of a page's cover + srcset (contrast cover_url_for's smallest) + CoverHero struct
- [x] CV.2 - Render stacked hero in get_page.html + wire GetPageTemplate.hero through the handlers
- [x] CV.3 - Tests + docs + deploy for the hero

---

## 2026-07-06

## Phase CX - Greylist — behavioral bot challenge (cat toll)

Design + rationale (the decisions, the honest limits): [docs/greylist-challenge-design.md](docs/greylist-challenge-design.md).

- [x] CX.0 - Phase exit: abusive IPs auto-greylisted on behavior (FCrDNS-verified crawlers exempt), served a snarky 429 PoW challenge; pass mints a 7d bearer clearance (NOT IP-bound) + records the signal; challenged traffic stamped challenged+is_bot in analytics; /admin/greylist manage panel; tests + docs; shipped to beta
- [x] CX.1 - Migration 0024 (greylist + greylist_clearance tables, request_log.challenged column) + GreylistDao with #[sqlx::test] coverage (auto-upsert/sliding-expiry, manual pin no-expiry, release, active set, record/list clearances)
- [x] CX.2 - Detection sweep: pure ip_features + named rules (R1 signature-probe ≥400-only UA-blind, R2 distinct-404 burst, R3 flood) over request_log; periodic detached coordinator task; loopback/RFC1918 guard; pluggable score()->Verdict; unit tests at threshold edges
- [x] CX.3 - FCrDNS crawler verification at sweep time — reuse the async ACME DNS resolver (PTR reverse + forward A confirm), timeout, suffix allowlist const, verdict cache, DNS-failure = skip-tick fail-safe, injected resolver for tests — verified crawlers never auto-greylisted, exemptions recorded
- [x] CX.4 - Challenge core: STATELESS signed-seed token — seed=HMAC(crypto_keys id 4, inner_seed‖ts‖digest_version), NO per-challenge store; image_digest computed at boot from committed assets/greylist/toll.png (version = content hash, no runtime rotation); verify = recompute seed + short freshness-window check + constant-time compare (CVE-2025-24369-style regression test: reject any answer not recomputed); clearance = signed bearer cookie HMAC(server_key, expiry), NOT IP-bound, HttpOnly+Secure+SameSite, 7d — pure fns + unit tests
- [x] CX.5 - Enforcement middleware: shared in-memory active set; skips = authenticated session/API key, valid clearance, exempt paths (/challenge*, /robots.txt, /.well-known/*, embedded static chrome; /media stays tolled); 429+Retry-After interstitial short-circuit; response extension → request_log stamps challenged=1 + is_bot=1; wired inner to refresh_session_role
- [x] CX.6 - Interstitial + verify endpoints: GET /challenge/new (O(1) token issue, no rate-limit v1); GET /challenge/verify (recompute seed, freshness window, version digest lookup, constant-time compare, same-origin path-only redir, 302 + Set-Cookie clearance w/ solve time); self-contained static cat-customs interstitial (image = cached static asset by version, worker JS + inline/same-origin SHA-256, progress bar, JS+cookies-required copy per voice profile)
- [x] CX.7 - Admin: /admin/greylist page (active entries w/ reason+evidence, challenges served, clearances, release, manual pin) + "Greylist this IP" on the IP drill-down; dashboard surfaces challenged counts
- [x] CX.8 - Integration + e2e tests: seeded-entry challenge flow, exempt paths, clearance passes, authenticated skip, verify-endpoint abuse cases (bad nonce, replay, foreign redir); chromiumoxide e2e solves the real PoW and lands on content
- [x] CX.9 - Refinement panel: candidate signatures = paths probed by greylisted IPs ∩ never-succeeded, promote-to-ruleset button; clearance-then-kept-scanning escalation documented as deferred
- [x] CX.10 - CLAUDE.md + docs (beta dark-launch caveat: request_log scrubbed → auto-detection empty on beta; deferred: escalation, batched writer, fitted model) + beta deploy + watch
- [x] CX.11 - Bespoke challenge design chat — align on the canvas pixel-write + keyed-order hashing kernel (stock Anubis PoW is solver-coded in the wild; decide image pipeline, mutation model + verify kernel before building CX.4/CX.6)
- [x] CX.12 - Beta trip-test affordance: admin "Run sweep now" button (release-safe, no debug seam) to force detection on demand; document the recipe (hit /wp-login.php ×2 → run sweep → greylisted → toll); manual pin covers instant challenge-flow test

---

## 2026-07-06

## Phase CY - Analytics — align on the greylist/challenged dimension

- [x] CY.0 - Phase exit: the analytics dashboard reports the greylist/challenged dimension — tolls-served surfaced, is_bot-vs-challenged disambiguated, 429-tolls read as "greylist working" not mystery errors, /challenge ceremony out of the noise; tested + shipped
- [x] CY.1 - Model DECIDED (chris confirmed): challenged is a subset of is_bot=1 → a "tolls served" headline sub-metric + a `?audience=challenged` FILTER value (scopes all graphs/tables), NOT a 4th All/Humans/Bots partition chip; chip styled as "a slice of Bots"
- [x] CY.2 - Surface the toll on the dashboard: a "challenged / tolls served" stat over the window + a challenged series on the traffic-per-day chart, so the greylist's impact is visible
- [x] CY.3 - Status-bucket clarity: split the 429 tolls out of s4xx (like 403/404 are split) or annotate, so a 4xx rise reads as "greylist working" not mystery errors
- [x] CY.4 - Exclude the greylist's own ceremony (/challenge/*) from request_log (mirror the /admin/analytics self-exclusion) so /challenge/new|image|verify don't pollute top-paths — the challenged=1 stamp on the ORIGINAL path is preserved
- [x] CY.5 - Cross-check existing views under the new dimension: audience counts, top-pages Content↔All, noisy-IPs, never-succeeded, referrers — a greylisted-then-cleared IP reports correctly, no double-count/misattribution
- [x] CY.6 - Tests (challenged-aware count queries + a challenged fixture) + CLAUDE.md/docs update + deploy beta→prod
- [x] CY.7 - Challenged FILTER: add `Audience::Challenged` (→ challenged=1 predicate) threaded through all ~15 windowed count/GROUP-BY queries + the chart JSON island + the chip row, reusing the CQ.7 hx-get control model, so every graph/table scopes to tolled traffic; a bad value degrades to All (never 500)
- [x] CY.8 - Toll outcome / block-hardness: challenged (walls hit) vs greylist_clearance (got through) over the window → a solve-rate stat (LOW = hard block, scrapers bounce; HIGH = soft, solving-through or a false positive) + per-IP challenged-vs-cleared so a repeatedly-solving IP is visible. No new schema — data's already recorded.
- [x] CY.9 - Whole-page design review of /admin/analytics at end of CY: has it gotten too confusing/cluttered with all the dimensions (audience + challenged filter, status buckets w/ 429, tolls/solve-rate, top-pages, noisy IPs, referrers, latency, chart)? Assess info hierarchy, propose + apply simplification

---

## 2026-07-09

## Phase CZ - Family role + honest sessions (Library/Home foundation)

**Summary:** the role ladder the Library phases (DA–DF) build on, shipped INERT (no migration, no anonymous/registered behavior change) — commit `ecf6eff`, beta-validated on `1f97616` same day. `Role::Family` + explicit `rank()` (a `Role::iter()`-pinned test guards the ladder; `derive(Ord)` forbidden — alphabetical variants would rank Admin below Anonymous); `AuthenticationState::role()` with `is_admin()` delegating; the `/admin/users` three-way control validating targets against a POSITIVE `ASSIGNABLE_ROLES` allowlist (itself iter-pinned; `Anonymous` → 400); the role-scoped mutation allowlist in `require_admin_for_mutations` shipped EMPTY with a pinned test (DF adds `POST /library/progress`). **The session-touch fix took two rounds:** a re-`insert` of the same `SessionData` silently never saves (tower-sessions dedups unchanged values — the integration test caught the no-op), and the review then caught that an UNCONDITIONAL touch writes `tower_sessions` per static-asset/media-range GET (a save tripping the 5s `busy_timeout` 500s the response — tower-sessions replaces it) → final form is a `touched_at` stamp throttled to one write/session/hour, with the throttle-skip pinned in the test. Accepted residuals, documented in code: the last-write-wins logout race inherent to activity-refresh sessions (bounded by the throttle; deleted/demoted users re-derive per request), and admin HTMX forms swallowing non-2xx (pre-existing → Tech debt). Rode along: the Biome swap (npm/Prettier REMOVED after a whole-tree-reformat + markdown-corruption incident; first `biome check` caught 4 un-interpolated `${…}` bugs in htmx-webauthn.js error logs), commit `1f97616`.

**Validation:** cargo test 348 green (unit + integration + browser-e2e passkey ceremonies through the changed webauthn JS); `biome check` clean; beta deploy verified live (/, 403 on /admin/users anon, login renders) and chris signed off on the three-way UI. The >24h-session check continues passively on chris's phone (mechanics pinned by `authenticated_activity_extends_session_expiry`).

- [x] CZ.0 - Phase exit: Role::Family exists end-to-end (rank ladder, admin UI, test seam), sessions actually refresh on activity, role-scoped mutation allowlist shipped EMPTY; tests + CLAUDE.md; inert on prod. Design: docs/library-design.md
- [x] CZ.1 - Role::Family + explicit rank() (Anonymous 0 < Registered 1 < Family 2 < Admin 3), Role::iter()-pinned test + doc comment forbidding derive(Ord) (variants are alphabetical — Admin would rank below Anonymous)
- [x] CZ.2 - AuthenticationState::role() -> Role helper beside is_admin()/is_authenticated(); all new viewer-role derivations go through it
- [x] CZ.3 - Session-touch fix: refresh_session_role re-saves authenticated sessions so OnInactivity(1 day) means inactivity — today the session is only written at login and dies 24h later regardless of activity (likely explains the Phase-10 "not logged in on the phone" dogfood finding)
- [x] CZ.4 - /admin/users rework: UserRow carries the real role (not is_admin bool), three-way promote control, reject role=Anonymous as a target; tests
- [x] CZ.5 - require_admin_for_mutations: role-scoped exact-match allowlist table (Method, path, min Role) checked by rank() — shipped EMPTY; conventions: ids in request body, per-resource checks in handlers; unit tests
- [x] CZ.6 - Integration gate tests via /test/login?role=Family + CLAUDE.md delta + beta validation (promote a beta user to Family; session survives >24h of activity)


---

## 2026-07-09

## Phase DA - Page visibility - the min_role predicate

**Summary:** the security-critical phase of the library arc — `content_pages.min_role` (migration `0025`, NULL = the only public spelling) gates every read path, fail-closed + oracle-safe. Commit `c3540fb`, beta-validated live same day. `is_visible_to(viewer: Role)` = (special_page ‖ Admin ‖ !scheduled) AND `rank() >= min_role_rank()`; the paged trio applies the byte-identical SQL CASE with `viewer: Role` replacing the `viewer_is_admin` bool end-to-end (`paginate` included); ~12 read paths threaded; feed/sitemap/redirect/nav pinned `Role::Anonymous`. **Review catches that mattered:** (1) the SECTION half-gate — a `min_role` on a special row hid the nav tab + `/pages` redirect but `/blog`/`/projects`/`/3d`/`/resume`(+`.pdf`), home's bands, the feed and the sitemap's special branch still served the section (the split itself an oracle) → all seven surfaces now honor the ancestor gate, pinned by the `gated_special_page_darkens_its_whole_section` e2e; (2) the fail-closed catch-all was a literal `3` → now `Role::Admin.rank()` + the `admin_is_the_top_rank` pin (a future mid-ladder insert must not turn "unknown value" into a middle-tier leak); (3) the parity test compared cardinalities and had no teeth for an unlearned variant (both ladders fail closed identically and cross-agree) → row-SET comparison + a positive per-variant pin (a row gated at exactly R must admit viewer R). Oracle tests: gated page ≡ genuine miss byte-for-byte per-session for Anonymous AND Registered; gated ancestor hides its subtree; feed/sitemap never carry gated content even fetched WITH a session. Deliberately NOT fixed: TopBar stays Anonymous until DB.4 (fail-safe), `/diagram/<hash>` = design-doc acceptance, `redirect_to_first_page` unconditional (CU precedent). Rollback caveat documented: a pre-DA binary serves gated rows publicly.

**Validation:** 355 tests green (5 new integration, 3 new unit; clippy at the 5-warning baseline). Beta (`c3540fb`): migration applied, zero rows gated; live matrix on a snapshot post stamped `Family` via sqlite3 — anon direct 404 + absent from /blog, home, sitemap; Bearer-Admin key (chris-minted) read it everywhere while feed/sitemap stayed anonymous-clean WITH the key attached; the one feed grep hit was an escaped cross-link inside another post's body (link-404s, not a leak — the designed property); stamp reverted, post public again.

- [x] DA.0 - Phase exit: content_pages.min_role gates every read path oracle-safely and fail-closed; parity tests green; inert (no rows gated); CLAUDE.md updated
- [x] DA.1 - Migration: content_pages.min_role TEXT NULL (NULL = the only public spelling); thread the column through ~8 query_as! sites + struct + test constructors (cargo clean for sqlx re-validation)
- [x] DA.2 - is_visible_to(viewer: Role): special-page exemption narrowed to scheduling only (role clause applies to special pages); fail-closed parse (unknown non-NULL min_role → Admin-only)
- [x] DA.3 - Fail-closed SQL CASE (WHEN NULL 0 / Registered 1 / Family 2 / ELSE 3) in count_children + both paged fetches; parity tests: count/fetch predicates identical + CASE↔rank() via Role::iter()
- [x] DA.4 - Thread the ~12 read paths: get_page_path ancestor scan, show_post + sibling retain, home bands, resume, /3d, paginate (viewer_is_admin bool → Role); feed/sitemap/redirect_to_first_page stay unconditionally Anonymous
- [x] DA.5 - Oracle tests: gated content page returns the byte-identical cat-404 for Anonymous AND Registered; Family/Admin get 200; rollback caveat documented (pre-gate binary serves gated rows publicly)
- [x] DA.6 - CLAUDE.md delta + beta validation: sqlite3-stamp min_role on a throwaway beta page, verify all four listing surfaces + direct-serve deny/allow


---

## 2026-07-09

## Phase DB - Page visibility - authoring + nav surface

**Summary:** the authoring surface over DA's predicate — commits `869eca7` + `e0a91ab` (select styling), beta-validated by chris same day (author→gate→deny loop through the real editor: gate via select → logged-out 404 → Public reopens). Editor **Visibility select** (5th metadata cell) rides `PutPageForm.min_role` → `update()` (stamps `page_modified_date` → feed/sitemap validators bust on a flip); the write rule mirrors the cover-typo rule — `"Public"` clears, a known role sets, ABSENT/unrecognized keeps the gate (bad input never silently LOOSENS visibility; the absent-field case protects old-client PUTs). **Inherit-on-create** stamps a child with its parent's gate at birth (top-level pages explicitly born public); no retroactive downward propagation — the ancestor scan stays the enforcement. **Visibility pills** beside every Scheduled pill (4 card templates + editor header + reader/hero), always from `visibility_label()` — the fail-closed decode, never the raw string. **Role-aware nav:** `TopBar::create(…, viewer: Role)` at 17 call sites. **Review catch that mattered:** the first nav cut used `is_visible_to(viewer)` wholesale, which rendered an admin's future-dated DRAFTS as unbadged live-looking tabs — the semantics are now the DAO's `is_nav_visible_to` (role viewer-aware, scheduling hidden-from-everyone) living beside `is_visible_to` so the two gates can't drift, pinned by `Role::iter()` tests both ways. Accepted: no retroactive gate propagation (documented), visibility pills visible to entitled non-admins (tier names only on content you can already read), garbage-gate→'Admin' normalization on resave (fail-closed, unpinned). Styling: the native `<select>` chrome needed `appearance-none` + the input border + a vendored FA `chevron-down` (new icon) — chris's beta catch.

**Validation:** 359 tests green (editor-loop integration incl. garbage/absent-field keep-the-gate, inherit e2e, nav role/schedule units); clippy at baseline; beta `e0a91ab` live with chris's sign-off on the functional loop.

- [x] DB.0 - Phase exit: visibility is authorable in the editor, badged everywhere Scheduled is, nav is role-aware; beta author→gate→deny loop verified
- [x] DB.1 - Editor Visibility select (Public/Registered/Family/Admin-only) as the 5th metadata-grid cell, through PutPageForm → update() (stamps page_modified_date so feed/sitemap validators bust on a visibility flip)
- [x] DB.2 - Visibility badge everywhere the Scheduled badge renders (4 card templates + editor header + reader) — admin-facing by construction
- [x] DB.3 - Inherit-on-create: new child pages default min_role to the parent's (post_page_path has the parent row in hand) — belt+suspenders over the ancestor scan
- [x] DB.4 - Role-aware TopBar::create (viewer Role param, ~15 call sites) — Family sees gated tabs, Anonymous doesn't; tests
- [x] DB.5 - Tests + CLAUDE.md delta + beta validation: full author→gate→deny loop through the editor on beta


---

## 2026-07-09

## Phase DC - Media visibility - gating the bytes

**Summary:** media bytes now actually gate — commit `e7535cb`, live-validated on beta same day via a chris-minted API key (gated upload → anon 404 on bytes+302 with the embed denial BYTE-IDENTICAL to a bogus-ref miss, key-read 200 with `private` caching, public control unchanged, test items deleted after). `media.min_role` (migration `0026`, the pages' NULL-only-public fail-closed decode) enforced at all three routes; the byte route resolves variant + gate in ONE query (`find_by_url_key_with_required_rank`) with **strictest-wins** MAX-rank across every item sharing the url_key — content-addressed dedup makes that index deliberately non-unique, so the LIMIT-1 owner could be the loosest and leak silently; MAX only over-restricts, visibly (the e2e proved it by ACCIDENT: its fixture uploads shared bytes and the "public" one correctly gated until the fixtures were made distinct). **Review catches:** the role-dependent embed HTML had no Cache-Control → ALL embed responses now `no-store` (a cached Family embed would leak a gated url_key to anon, and a miss/denial HEADER difference would itself be an oracle — asserted in the e2e); `render_embed_html` carries a gate-invariant comment (byte-URL references ONLY, never inlined bytes — the one path that could bypass strictest-wins); the editor drop-upload's gate-inheritance selector scoped to the editor's own form. **Leak sweep verdict: zero byte-leak paths** — gated URLs can surface in public HTML (gated cover on a public page, author-pasted byte URLs baked into cached HTML/feed) but every fetch re-gates: the documented authoring rule (public page ⇒ public cover; embed gated media via `![](/media/<ref>)` only). Accepted by design: over-restriction on deduped bytes (fail-closed, visible), browser-cache retention after logout (`private,immutable` — the no-DRM family-trust model), per-card hx-post swallow (pre-existing tech-debt). Authoring defaults: upload `min_role` field (`fab publish` sends nothing → public), editor drop-uploads inherit the page's gate, library badge + per-item selector + drop-zone default.

**Validation:** 361 tests green (shared-sha strictest-wins unit incl. garbage→top, the full upload→deny/allow/cache e2e, embed no-store assert); clippy at baseline; beta `e7535cb` matrix all-green live.

- [x] DC.0 - Phase exit: media.min_role enforced on bytes/302/embed with strictest-wins dedup + private caching; authorable with safe defaults; beta-verified
- [x] DC.1 - Migration: media.min_role TEXT NULL (same NULL-only-public, fail-closed semantics) + DAO threading
- [x] DC.2 - Byte route gate: SessionData extractor + NEW scalar-aggregate strictest-wins query (MAX rank across ALL media rows sharing the url_key — find_by_url_key/MediaVariantDao untouched); shared-sha unit test (NULL + Family → Family wins); denied → 404
- [x] DC.3 - /media/{ref} 302 + /media/embed/{ref} gates via the media row both already load (embed denial = the existing 200 error-span miss shape)
- [x] DC.4 - Cache-Control on gated bytes: private, max-age=31536000, immutable (public media unchanged); header test
- [x] DC.5 - Authoring defaults: upload_media min_role multipart field; editor-support.js sends the current page's visibility as default (drop-on-gated-page must NOT mint public media); library UI selector + prominent badge + default control (POST /admin/media/{id}/visibility)
- [x] DC.6 - Tests + CLAUDE.md delta + beta validation: gate a beta media item — anonymous 404, Family 200, private header


