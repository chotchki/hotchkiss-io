# Plan

Completed phases are in `PLAN_ARCHIVE.md` (most recent: Phase 2 — DNS module testability; Phase 5 — dropped the `cookie-rs` fork; Phase 3 — `ifconfig.me` → Cloudflare `cdn-cgi/trace`; Phase 0 — push-to-deploy on the Mac mini; Phase 4 — `tray-wrapper` 0.4.1 bump).

## Phase 1 — Fix `get_recs_by_name` hardcoded `type=A` filter

**Symptom:** ACME cert renewal hangs forever in `DnsValidator::ensure_not_existing` polling for a stale `_acme-challenge` TXT record that never disappears.

**Root cause:** `CloudflareApi::get_recs_by_name` pinned the Cloudflare query to `type=A`. When `clean_proof` calls it for the `_acme-challenge` domain, Cloudflare returns 0 results (no A records exist there), the delete loop is a no-op, and no TXT records are ever removed. `ensure_not_existing` then polls indefinitely.

- [x] 1.1 Add a record-type parameter to `CloudflareApi::get_recs_by_name` (`rec_type: &str`) and use it in the query string.
- [x] 1.2 Update `clean_proof` (`cloudflare_client.rs`) to pass `"TXT"`.
- [x] 1.3 Update `update_dns` (`cloudflare_client.rs`) to pass `"A"` (preserves current behavior; keeps `Ipv4Addr::from_str(&rec.content)` parsing safe).
- [x] 1.4 `cargo build` + `cargo clippy` clean (only pre-existing warnings remain).
- [x] 1.5 `cargo test` passes.
- [ ] 1.6 Manual e2e: confirm the next real ACME renewal in prod succeeds — `clean_proof` deletes any leftover `_acme-challenge` TXT records before `create_proof` recreates them. The fix is live on the mini (shipped with the Phase 0 deploys), so this is "watch the next renewal", not "deploy and test". (Phase 2 added unit coverage for the URL-construction class of bug; an automated ACME-path e2e is still a gap — tracked in Phase 6.)
- [ ] 1.7 Docs: no CLAUDE.md changes needed (behavior fix, no architectural shift). Confirm and tick.

## Phase 6 — DNS follow-ups (parked)

Two items deferred out of Phase 2 — neither urgent, both real.

- [ ] 6.1 Higher-level tests for `CloudflareClient::clean_proof` / `create_proof` / `update_dns` (the set-arithmetic + sequencing logic). Requires `BASE_URL` in `cloudflare_api.rs` to stop being a `LazyLock<Url>` const and become a `CloudflareApi` field so a mock server URL can be injected; then use `wiremock` (decided in Phase 2.3 — `mockito` is fine too, `wiremock`'s async-first API fits the codebase better). Not built yet because the regression class that actually bit us (a pinned query param) is now covered by the pure URL-builder unit tests, and this is mostly orchestration.
- [ ] 6.2 Re-evaluate `DnsValidator`'s disabled timeout. The `//if … timeout …` blocks in `ensure_exists` / `ensure_not_existing` are commented out, so both are unbounded retry loops today. The original condition was also backwards (`timeout > Instant::now()` where `timeout = now + 300s` is true for the whole window → would bail on iteration 1; the corrected form `Instant::now() > timeout` is now in the comment). Re-enabling it is a *runtime behavior change* to the ACME path — a slow DNS propagation would make a renewal *fail* after 5 min instead of eventually succeeding. Decide whether bounded-with-bail or unbounded is what we want before flipping it on; if bounded, the `WaitStep` decision split from Phase 2.4 makes the loop easy to test with a short timeout.

## Phase 7 — Admin analytics dashboard

**Goal:** an admin-only `/admin/analytics` page answering "who is scraping my site / what's getting hit" (SPEC.md "Analytics"). Three slices: **7.1** request-logging data layer, **7.2** an `/admin` route nest behind an auth layer, **7.3** the dashboard view, then **7.4** docs/exit. Deliberately the first use of a *route-group auth layer* (vs. the per-handler `is_admin()` checks scattered today — see Tech debt) and a deliberate *non*-use of the `special_page` mechanism (that's for redirects; analytics is a real handler — see Tech debt). Also the project's first request-logging table, first prune task patterned on the existing `tower-sessions-sqlx-store` GC, and first "dashboard-y" template.

**Design decisions (resolved 2026-05-10):**
- **IP source — plumb it through.** The mini is hit directly (Cloudflare is DNS-only — it manages A records + ACME DNS-01), so the connecting socket addr *is* the real client IP. Add `.into_make_service_with_connect_info::<SocketAddr>()` where `axum_server` serves the app (`EndpointsProviderService`) if it isn't already; the middleware reads `ConnectInfo<SocketAddr>`. `X-Forwarded-For` / `X-Real-IP` only become relevant if a reverse proxy ever appears.
- **Prune task — co-located with session GC.** Lives alongside the existing `tower-sessions-sqlx-store` GC task in `EndpointsProviderService`.
- **Retention + window.** 90-day retention; dashboard defaults to the last 7 days with a `?since=` override.
- **Nav — conditional top-bar link** gated on `auth_state.is_admin()` (templates already receive `auth_state`; askama can call the method). Additive change to the top-bar *partial*, not the `TopBar` type. If the admin nav grows, revisit (dropdown).

### 7.1 Request-logging data layer

- [x] 7.1.1 `0009_TableRequestLog.sql`: `request_log (id, ts text NOT NULL DEFAULT CURRENT_TIMESTAMP, method, path, status, ip, user_agent, referer)` + `idx_request_log_ts`. Followed `content_pages`' convention — SQLite stamps `ts` on insert (`CURRENT_TIMESTAMP`, UTC, `YYYY-MM-DD HH:MM:SS`), so the middleware doesn't compute it; `substr(ts,1,10)` gives the day; `datetime('now','-N days')` gives windows. `cargo clean -p hotchkiss-io` done so sqlx re-validated.
- [x] 7.1.2 `RequestLogDao` (`src/db/dao/request_log.rs`): `insert(&NewRequestLog)`, `recent(limit)`, `count_since(days)`, `distinct_ip_count(days)`, `count_by_path(days, limit)`, `count_by_user_agent(days, limit)`, `count_by_day(days)`, `prune_before(retain_days)` — windows via a private `window(days) -> "-N days"` helper; aggregates ORDER/GROUP BY the underlying expr (not the sqlx column alias — sqlx's compile-check rejects alias refs in ORDER/GROUP BY). 3 `#[sqlx::test]` units: insert+recent, all aggregates, prune.
- [x] 7.1.3 `web/middleware/request_log.rs::log_requests` — `from_fn_with_state(pool, log_requests)`: captures method/path + IP (from the `ConnectInfo<SocketAddr>` extension, `None` if absent) + `User-Agent`/`Referer` headers, runs `next`, then `tokio::spawn`s the INSERT (fire-and-forget; `warn!` on error). `debug_assertions` builds skip `/tower-livereload`.
- [x] 7.1.4 Wired as the *outermost* layer in `create_router`'s `ServiceBuilder` (sees every request incl. static/404s + the final status). Doesn't need the session, so it sits outside the session layer.
- [x] 7.1.5 Prune task: in `EndpointsProviderService::start`, a `JoinSet` task next to the session GC — daily `RequestLogDao::prune_before(pool, 90)` (`tokio::time::interval`, first tick fires at startup). `cargo build` + `cargo clippy --all-targets` clean (only the 4 standing pre-existing warnings); `cargo test` 37/37.

### 7.2 Admin route nest + auth layer

- [x] 7.2.1 `web/middleware/require_admin.rs::require_admin` — `async fn(SessionData, Request, Next) -> Result<Response, (StatusCode, &'static str)>`: `403` ("Admin only") unless `session_data.auth_state.is_admin()`, else `next.run(req).await`. (`SessionData`'s extractor defaults to `Anonymous` when there's no session → unauthenticated gets a clean 403, no panic.)
- [x] 7.2.2 `web/features/admin/mod.rs::admin_router() -> Router<AppState>` — `route("/analytics", get(show_analytics)).layer(from_fn(require_admin))`; `create_router` adds `.nest("/admin", admin_router())` (inside the top-level session layer).
- [x] 7.2.3 Decision: took the plan's fallback. `AuthenticationState::is_admin()` is already unit-tested (`authentication_state.rs::admin_check`), and `require_admin` is a one-line wrapper over it; a full router+seeded-session integration test (needs `AppState` = `Webauthn` + `SqliteStore` + migrated pool + cookie plumbing) was disproportionate for a "get used to building features" pass. **Gap recorded:** no test asserts the `require_admin` *layer* is actually wired onto `/admin` — if someone drops the `.layer(...)` in `admin_router`, nothing catches it. → Tech debt / a future `wiremock`-style web-test harness (cf. Phase 6.1's `BASE_URL`-injectable groundwork).
- [x] 7.2.4 Scope guard held — the existing scattered `is_admin()` / `if let Authenticated(u) && u.role != Admin` checks were left alone; the converge-them-behind-a-layer-or-extractor work stays in Tech debt.

### 7.3 Analytics dashboard view

- [x] 7.3.1 Built a v1 directly instead of an ASCII mock first (reasonable for an "easy feature" pass — show the deployed page and iterate). Layout: top-bar, `1d/7d/30d/90d` window pills, two big numbers (requests, distinct IPs), then "Requests per day", "Top paths", "Top user agents", and a "Recent requests" table (when/method/status/path/ip/user-agent). Plain HTML + the existing Tailwind classes; no JS charting.
- [x] 7.3.2 `web/features/admin/analytics.rs::show_analytics` — `State<AppState>` + `SessionData` + `Query<AnalyticsQuery>` (`?since=` → `unwrap_or(7).clamp(1,365)`), runs the `RequestLogDao` bundle (`count_since`, `distinct_ip_count`, `count_by_day`, `count_by_path(_,25)`, `count_by_user_agent(_,25)`, `recent(50)`), renders `AnalyticsTemplate`. No auth check — the layer owns it.
- [x] 7.3.3 `templates/analytics/dashboard.html` — `extends "base.html"`; fields `top_bar`, `auth_state`, `since_days`, `total_requests`, `distinct_ips`, `by_day`, `by_path`, `by_user_agent`, `recent`. Tables only.
- [x] 7.3.4 Added a conditional "Analytics" `<li>` to the admin block of the nav in `templates/base.html` (gated on `auth_state.is_admin()`).
- [ ] 7.3.5 `cargo build` + `cargo clippy --all-targets` clean; `cargo test` 37/37. Deployed via `git push origin main`. **Still to confirm in prod:** log in as admin, open `/admin/analytics`, generate some traffic, see rows accumulate (prune is covered by the 7.1.2 unit test). Anon `GET /admin/analytics` → 403 is verified.

### 7.4 Docs + exit

- [x] 7.4.1 CLAUDE.md: "Runtime architecture" `EndpointsProviderService` bullet now notes `into_make_service_with_connect_info` + the daily `request_log` prune task; "Web layer" Routing bullet notes the `/admin` nest + the outer middleware stack incl. request-logging; the Authorization bullet notes the `require_admin` layer is the *one* place auth is layer-enforced (rest still per-handler — see Tech debt).
- [x] 7.4.2 SPEC.md: "Analytics, who is scraping my site?" marked "v1 shipped 2026-05" with a Phase-7 pointer.
- [ ] 7.4.3 Sweep to PLAN_ARCHIVE.md once 7.3.5 is confirmed in prod and it's been live a few days.

## Phase 8 — Local / e2e test harness

**Goal:** make the running site testable without the prod machinery. Today you can't easily exercise it locally (the dev loop binds `:80`/`:443` and spins up the IP/DNS/ACME coordinator) and admin routes need a passkey. This delivers: **8.1** an in-process helper that boots just the axum app + a fresh DB on an ephemeral local port, **8.2** a debug-only login seam so tests (and Claude, poking around) can be "admin", **8.3** `reqwest`-based Rust integration tests, **8.4** a Playwright + CDP virtual-authenticator e2e that exercises the *real* passkey flow, **8.5** docs.

**Design decisions (resolved 2026-05-10):**
- **No production-reachable test server.** The boot logic lives in `tests/common/mod.rs::spawn_test_server()` — a fresh tempfile SQLite (`MIGRATOR.run`), `SqliteStore::new(...).migrate()`, a `WebauthnBuilder` on an `http://localhost:<port>` origin, `create_router(app_state)`, served via plain `axum::serve(TcpListener::bind("127.0.0.1:0"), router.into_make_service_with_connect_info::<SocketAddr>())`. No coordinator. Used in-process by the Rust integration tests. *(Verify `webauthn-rs` accepts an `http://localhost` origin; if it's https-only even for localhost, the helper needs a self-signed local TLS listener for the Playwright case — decide then.)*
- **Playwright needs a launchable server.** Resolved without a prod-reachable binary/flag: a `cargo test`-launched **blocking serve test** (`tests/e2e_serve.rs`, `#[ignore]`d so normal `cargo test` skips it) calls the same `spawn_test_server` on a *fixed* port and parks; Playwright's `webServer` runs `cargo test --test e2e_serve -- --ignored`. One helper, two consumers.
- **Login seam gating:** `#[cfg(debug_assertions)]` (attribute form — the route + handler literally don't exist in `--release`, which is what prod ships). If stricter gating is ever wanted, swap for a `test-harness` cargo feature — noted, not done.

### 8.1 In-process test server helper

- [ ] 8.1.1 `tests/common/mod.rs::spawn_test_server() -> TestServer { base_url, pool, _shutdown }` — fresh tempfile SQLite + migrations, `SqliteStore::migrate()`, `WebauthnBuilder::new("localhost", &Url::parse(&format!("http://localhost:{port}/"))?)?.build()?` (resolve the http-localhost question here), `create_router(app_state)`, bind `127.0.0.1:0`, `tokio::spawn(axum::serve(...))`. `reqwest` is already a dep; add as a dev-dep if the existing one isn't usable from tests.
- [ ] 8.1.2 Smoke: a `tests/` test does `spawn_test_server()` → `reqwest::get("{base}/")`. A fresh DB has no content pages → `/` → 404 "No pages found"; so the helper (or each test) seeds at least one `ContentPageDao` first, then asserts the 307 → `/pages/<name>` → 200. Decide where the seeding lives (probably a `TestServer::seed_*` convenience).

### 8.2 Debug-only login seam

- [ ] 8.2.1 `#[cfg(debug_assertions)] src/web/features/test_login.rs`: `test_router() -> Router<AppState>` with `POST /login` taking `role` (query or form) → `UserDao::find_by_display_name`-or-create with that `Role`, build `SessionData { auth_state: Authenticated(user) }`, `SessionData::update_session(&session, &data).await`, return 200. (`Session` extractor works — `/test` nests inside the top-level session layer.)
- [ ] 8.2.2 In `create_router`: `#[cfg(debug_assertions)] { router = router.nest("/test", test_login::test_router()); }`. Confirm it's absent from a `--release` build.
- [ ] 8.2.3 Sanity test: `POST {base}/test/login?role=admin` (capture the session cookie) → `GET {base}/admin/analytics` with the cookie → 200, body contains the dashboard heading; without the cookie → 403.

### 8.3 Rust integration tests

- [ ] 8.3.1 `tests/web.rs` (split as needed): analytics auth (403 anon, 403 registered, 200 admin + body checks); the request-log middleware records (hit a path, then query `request_log` via `TestServer::pool` and assert the row, incl. the `ConnectInfo` IP being `127.0.0.1`); a content page renders (seed a page, GET it, assert rendered markdown in the body). Each test gets its own DB via `spawn_test_server`.
- [ ] 8.3.2 `cargo test` green incl. the new `tests/` integration tests; `cargo clippy --all-targets` clean.

### 8.4 Playwright + virtual-authenticator e2e

- [ ] 8.4.1 `tests/e2e_serve.rs` — an `#[ignore]`d test that `spawn_test_server`s on a fixed (env-overridable) port and parks (`std::future::pending::<()>().await`); seeds a content page so `/` works. This is what Playwright launches.
- [ ] 8.4.2 `e2e/`: `package.json` (Playwright dev-dep), `playwright.config.ts` (`webServer: { command: "cargo test --test e2e_serve -- --ignored", url: "http://localhost:<port>", reuseExistingServer: true }`), `e2e/auth.spec.ts` — enable CDP `WebAuthn`, add a virtual authenticator, walk the real passkey *registration* flow (first registered user → Admin), then in a fresh context walk the *authentication* flow with the same credential, then assert `/admin/analytics` renders. `e2e/README.md` with run instructions (`npx playwright test`).
- [ ] 8.4.3 Decide CI vs. manual: Node + browser download in `test_and_coverage.yml` is real cost, and the Rust integration tests cover the non-passkey paths — lean toward keeping the Playwright e2e a local/manual check for now. Note the decision.

### 8.5 Docs

- [ ] 8.5.1 CLAUDE.md "Common commands": `cargo test` now includes `tests/` integration tests on an in-process server; the debug-only `/test/login` seam exists in non-release builds; `npx playwright test` (from `e2e/`) runs the browser e2e against a `cargo test`-launched server.
- [ ] 8.5.2 Sweep to PLAN_ARCHIVE.md once 8.1–8.5 are ticked.

## Tech debt (untriaged — triage into phases when picked up)

- **Routing model is "too clever" (the `special_page` fallout).** `content_pages` is a self-referential tree that simultaneously (a) serves nested rendered-Markdown content, (b) carries `special_page` rows whose `page_markdown` is a *redirect target URL*, not content, and (c) is dispatched by a top-level router that special-cases the redirect rows while *also* breaking out to dedicated application routers (`/login`, `/projects`, soon `/admin`). Three concerns — content node / routing redirect / dedicated app page — conflated in one table + one dispatch path. A cleaner design separates them (content pages stay a tree; "special"/app routes become plain axum routes, not DB rows). Touches `redirect_to_first_page`, `pages/mod.rs` dispatch, `ContentPageDao::find_by_path`, the `0007` seed migration, `projects.rs`.
- **Authorization is per-handler and inconsistent.** Two idioms in the tree: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` (`preview.rs`, `attachments.rs`) and `if let AuthenticationState::Authenticated(u) = … && u.role != Role::Admin { return FORBIDDEN }` (`pages/mod.rs::delete_page_path`). No route-group enforcement anywhere. Phase 7 introduces a `require_admin` layer for the new `/admin` nest; the follow-up is to audit every existing mutating route and either move it behind a layer or a uniform `AdminUser` extractor, and converge on one idiom. (CLAUDE.md explicitly warns: audit every route first.)
- **Tailwind/DaisyUI build pipeline is non-portable + non-reproducible.** `build.rs` downloads `tailwindcss-macos-arm64` (hardcoded OS+arch — fails on Linux/x86, e.g. any non-self-hosted CI runner) and DaisyUI from `…/releases/latest/…` (unpinned → builds aren't reproducible). Also `styles/tailwind.css` only `@plugin "@tailwindcss/typography"` — no `@plugin "daisyui"` — so DaisyUI may be downloaded but unused; confirm and either wire it in or stop fetching it. Fixes: pin versions, make the CLI fetch arch/OS-aware (or vendor it, or use the npm `@tailwindcss/cli` package), resolve the DaisyUI question.
- **`SessionData::from_request_parts` has a load-bearing `.unwrap()`** (carrying a `//Unsure how to do this without an unwrap` comment) on the session-store read — a transient SQLite error there panics the request instead of degrading. Map it to `Ok(SessionData::default())` (treat a read failure as "no session") or surface it as a 500 via the rejection type.
- **No test asserts the `require_admin` layer is actually wired onto `/admin`.** `is_admin()` is unit-tested and `require_admin` is a one-liner over it, but if someone drops the `.layer(...)` in `admin_router()` nothing catches it. Closed once Phase 8's integration tests land (8.2.3 / 8.3.1 hit `/admin/analytics` as anon → 403).

## Backlog (ideas — promote to a phase when ready)

- **Analytics: status / "noise" view.** Status-code breakdown (200 vs 404 vs 403 …) and a "paths that only ever 404" table — effectively a scanner-signature list. Small; extends the `request_log` queries + the dashboard template.
- **Analytics: per-IP drill-down.** Group by IP; flag IPs hitting many distinct 404 paths (classic scan fingerprint) vs. real visitors. A new query + a sub-page or section.
- **Analytics: referer breakdown.** `referer` is already recorded by the logging middleware, just not surfaced — add a `count_by_referer` query + a table.
- **Analytics → defense: dumb IP blocklist.** Derive a blocklist from `request_log` (N 404s in M minutes → drop the IP for a while), enforced by an early middleware layer. Bigger — its own phase (blocklist storage + decay, the enforcing layer, an admin view/override, false-positive handling).
