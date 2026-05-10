# Plan

Completed phases are in `PLAN_ARCHIVE.md` (most recent: Phase 7 — admin analytics dashboard; Phase 2 — DNS module testability; Phase 5 — dropped the `cookie-rs` fork; Phase 3 — `ifconfig.me` → Cloudflare `cdn-cgi/trace`; Phase 0 — push-to-deploy on the Mac mini; Phase 4 — `tray-wrapper` 0.4.1 bump).

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

## Phase 8 — Local / e2e test harness

**Goal:** make the running site testable without the prod machinery. Today you can't easily exercise it locally (the dev loop binds `:80`/`:443` and spins up the IP/DNS/ACME coordinator) and admin routes need a passkey. This delivers: **8.1** an in-process helper that boots just the axum app + a fresh DB on an ephemeral local port, **8.2** a debug-only login seam so tests (and Claude, poking around) can be "admin", **8.3** `reqwest`-based Rust integration tests, **8.4** a Playwright + CDP virtual-authenticator e2e that exercises the *real* passkey flow, **8.5** docs.

**Design decisions (resolved 2026-05-10):**
- **No production-reachable test server.** The boot logic lives in `tests/common/mod.rs::spawn_test_server()` — a fresh tempfile SQLite (`MIGRATOR.run`), `SqliteStore::new(...).migrate()`, a `WebauthnBuilder` on an `http://localhost:<port>` origin, `create_router(app_state)`, served via plain `axum::serve(TcpListener::bind("127.0.0.1:0"), router.into_make_service_with_connect_info::<SocketAddr>())`. No coordinator. Used in-process by the Rust integration tests. *(Verify `webauthn-rs` accepts an `http://localhost` origin; if it's https-only even for localhost, the helper needs a self-signed local TLS listener for the Playwright case — decide then.)*
- **Playwright needs a launchable server.** Resolved without a prod-reachable binary/flag: a `cargo test`-launched **blocking serve test** (`tests/e2e_serve.rs`, `#[ignore]`d so normal `cargo test` skips it) calls the same `spawn_test_server` on a *fixed* port and parks; Playwright's `webServer` runs `cargo test --test e2e_serve -- --ignored`. One helper, two consumers.
- **Login seam gating:** `#[cfg(debug_assertions)]` (attribute form — the route + handler literally don't exist in `--release`, which is what prod ships). If stricter gating is ever wanted, swap for a `test-harness` cargo feature — noted, not done.

### 8.1 In-process test server helper

- [x] 8.1.1 `src/test_support.rs::spawn_test_server() -> Result<TestServer { base_url, pool, .. }>` — *deviation from the plan: lives in the lib as `pub mod test_support;`, not `tests/common/`. A `tests/` module can only use the crate's public API, and exposing `create_router`/`AppState`/`DatabaseHandle` via `lib.rs` is a much wider surface than one `pub mod`.* Boots: fresh tempfile SQLite via `DatabaseHandle::create` (same WAL/FK config as prod) → migrations run; `SqliteStore::new(pool).migrate()`; `WebauthnBuilder::new("localhost", &Url::parse("http://localhost:{port}/"))?.build()?` (webauthn-rs accepts the http-localhost origin — confirmed by the passing test); `create_router(app_state)`; `TcpListener::bind("127.0.0.1:0")`; `tokio::spawn(axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()))`. `TestServer` has `url(path)`, `seed_content_page(name, markdown)`, `pool`, and a `Drop` that aborts the server task + removes the temp DB (+ `-wal`/`-shm`). *Unplanned but needed:* `create_router` now sets `.with_secure(!cfg!(debug_assertions))` on the session layer — `Secure` cookies aren't sent over the harness's plain HTTP; prod (release) still gets `Secure`.
- [x] 8.1.2 `tests/server.rs::harness_boots_and_serves` — `/` → 307 (the `0007` special-pages seed means `/` always redirects, *not* a 404 as the plan guessed); then `seed_content_page("HarnessSmoke", "# hello harness")` → `GET /pages/HarnessSmoke` → 200, body contains the rendered text. `cargo test` 37 lib + 1 integration green; `cargo clippy --all-targets` clean (5 standing pre-existing warnings, none new).

### 8.2 Debug-only login seam

- [x] 8.2.1 `#[cfg(debug_assertions)] src/web/features/test_login.rs::test_router()` — `POST /login` taking `?role=` (`Option<Role>`, strum variant names; defaults to `Admin`). Does a direct `INSERT INTO users (...) VALUES (...)` (a fresh user each call — bypasses `UserDao::create`'s first-user→Admin logic so the requested role is honored) then `SessionData::update_session(&session, &SessionData { auth_state: Authenticated(user) })`. (`Session` extractor works — `/test` nests inside the top-level session layer.)
- [x] 8.2.2 `create_router`: `#[cfg(debug_assertions)] let router = router.nest("/test", crate::web::features::test_login::test_router());` (attribute on the `let`, so in release the line vanishes and `router` is unchanged); `#[cfg(debug_assertions)] pub mod test_login;` in `web/features/mod.rs`. *Absence from release confirmed against the deployed prod binary (no `test/login` string).*
- [x] 8.2.3 Covered by `tests/web.rs::analytics_requires_admin` — `POST /test/login?role=Admin` (cookie-store client) → `GET /admin/analytics` → 200 + body contains "Analytics"; without login → 403.

### 8.3 Rust integration tests

- [x] 8.3.1 `tests/web.rs`: `analytics_requires_admin` (anon → 403, `?role=Registered` → still 403, `?role=Admin` → 200 + the dashboard renders); `request_log_middleware_records_requests` (`GET /pages/Probe`, then poll `request_log` via `server.pool` for the row — asserts `status = 200`, `ip = 127.0.0.1`, i.e. `ConnectInfo` is being read). Content-page rendering is already covered by `tests/server.rs::harness_boots_and_serves`. Each test gets a fresh DB via `spawn_test_server`. (DB reads in tests use the runtime `sqlx::query(...)` — not `query!` — so no `DATABASE_URL` needed when compiling the `tests/` crate.)
- [x] 8.3.2 `cargo test` 40 green (37 lib + 1 `tests/server.rs` + 2 `tests/web.rs`); `cargo clippy --all-targets` clean (5 standing pre-existing warnings, none new).

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
- **Staging / beta deployment.** A real deployed instance serving `beta.hotchkiss.io` (a DNS entry that already exists, used for Let's Encrypt testing) — complements Phase 8's local harness ("does it survive in the wild"). `Settings` already supports this with no code change: a beta `config.json` with `domain = beta.hotchkiss.io`, its own `database_path`, and its own Cloudflare token (separate token = independent revocation / rate-limit / audit — *not* blast-radius isolation, since CF tokens scope per-zone and `beta.` is in the `hotchkiss.io` zone; real isolation would need a delegated zone). The actual blocker is the hardcoded `:80`/`:443` (can't coexist with prod on the mini) → make ports configurable; then it's "second machine, or port-mapped local instance". Bundle: configurable ports + beta config/token + the second-instance story.
