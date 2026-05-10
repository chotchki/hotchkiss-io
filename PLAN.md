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

## Tech debt (untriaged — triage into phases when picked up)

- **Routing model is "too clever" (the `special_page` fallout).** `content_pages` is a self-referential tree that simultaneously (a) serves nested rendered-Markdown content, (b) carries `special_page` rows whose `page_markdown` is a *redirect target URL*, not content, and (c) is dispatched by a top-level router that special-cases the redirect rows while *also* breaking out to dedicated application routers (`/login`, `/projects`, soon `/admin`). Three concerns — content node / routing redirect / dedicated app page — conflated in one table + one dispatch path. A cleaner design separates them (content pages stay a tree; "special"/app routes become plain axum routes, not DB rows). Touches `redirect_to_first_page`, `pages/mod.rs` dispatch, `ContentPageDao::find_by_path`, the `0007` seed migration, `projects.rs`.
- **Authorization is per-handler and inconsistent.** Two idioms in the tree: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` (`preview.rs`, `attachments.rs`) and `if let AuthenticationState::Authenticated(u) = … && u.role != Role::Admin { return FORBIDDEN }` (`pages/mod.rs::delete_page_path`). No route-group enforcement anywhere. Phase 7 introduces a `require_admin` layer for the new `/admin` nest; the follow-up is to audit every existing mutating route and either move it behind a layer or a uniform `AdminUser` extractor, and converge on one idiom. (CLAUDE.md explicitly warns: audit every route first.)
- **Tailwind/DaisyUI build pipeline is non-portable + non-reproducible.** `build.rs` downloads `tailwindcss-macos-arm64` (hardcoded OS+arch — fails on Linux/x86, e.g. any non-self-hosted CI runner) and DaisyUI from `…/releases/latest/…` (unpinned → builds aren't reproducible). Also `styles/tailwind.css` only `@plugin "@tailwindcss/typography"` — no `@plugin "daisyui"` — so DaisyUI may be downloaded but unused; confirm and either wire it in or stop fetching it. Fixes: pin versions, make the CLI fetch arch/OS-aware (or vendor it, or use the npm `@tailwindcss/cli` package), resolve the DaisyUI question.
- **`SessionData::from_request_parts` has a load-bearing `.unwrap()`** (carrying a `//Unsure how to do this without an unwrap` comment) on the session-store read — a transient SQLite error there panics the request instead of degrading. Map it to `Ok(SessionData::default())` (treat a read failure as "no session") or surface it as a 500 via the rejection type.
