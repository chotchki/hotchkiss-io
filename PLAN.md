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

- [ ] 7.1.1 Migration `0009_TableRequestLog.sql`: `request_log (id INTEGER PRIMARY KEY, ts TEXT NOT NULL, method TEXT NOT NULL, path TEXT NOT NULL, status INTEGER NOT NULL, ip TEXT, user_agent TEXT, referer TEXT)` + an index on `ts`. ISO-8601 text timestamps (SQLite-comparable; matches the session store's convention). `ip`/`user_agent`/`referer` nullable. Run `cargo clean -p hotchkiss-io` afterward so the sqlx macros re-validate against the rebuilt schema db.
- [ ] 7.1.2 `RequestLogDao` (`src/db/dao/request_log.rs`): `insert(executor, &RequestLogRow)` plus the dashboard queries — `recent(executor, limit)`, `count_by_path(executor, since, limit)`, `count_by_day(executor, since)`, `count_by_user_agent(executor, since, limit)`, `distinct_ip_count(executor, since)`, `prune_before(executor, cutoff)`. `#[sqlx::test(...)]` units covering insert + each aggregate + prune against seeded rows.
- [ ] 7.1.3 Request-logging middleware — an `axum::middleware::from_fn` closure capturing a cloned `SqlitePool`: capture `method` + `path` (and the client IP per the ConnectInfo decision) before `next.run(req)`, the response `status` after, pull `User-Agent` / `Referer` headers, then `tokio::spawn` the INSERT (fire-and-forget — logging never adds latency to nor fails the response; on insert error, `warn!` and move on). In `debug_assertions` builds, skip the livereload long-poll path (noise).
- [ ] 7.1.4 Wire the middleware into `create_router`'s `ServiceBuilder` stack so it sees *all* routes — static assets, 404s, everything (scrapers hit weird paths). It doesn't need the session, so it can sit outside the session layer; decide exact ordering vs. compression/trace.
- [ ] 7.1.5 Prune task: periodically `DELETE FROM request_log WHERE ts < cutoff` (retention window per the decision above), located per the decision above. `cargo build` + `cargo clippy --all-targets` clean; `cargo test` green.

### 7.2 Admin route nest + auth layer

- [ ] 7.2.1 `require_admin` middleware: `async fn require_admin(session_data: SessionData, req: Request, next: Next) -> Result<Response, (StatusCode, &'static str)>` — `403 FORBIDDEN` unless `session_data.auth_state.is_admin()`, else `next.run(req).await`. (`SessionData`'s extractor already defaults to `Anonymous` when there's no session, so unauthenticated → 403, no panic.) Lives in `src/web/features/admin/mod.rs` (or `src/web/middleware/require_admin.rs`).
- [ ] 7.2.2 `admin_router() -> Router<AppState>`: nests the analytics route(s), applies `.layer(axum::middleware::from_fn(require_admin))` so every route under it is gated *by construction* — no per-handler checks inside. `create_router` adds `.nest("/admin", admin_router())` (inside the session layer, which is already the outermost layer there).
- [ ] 7.2.3 Tests: an integration test that `GET /admin/analytics` is 403 for anonymous + registered sessions and 200 for an admin session. If standing up a `Router` with a seeded admin session is heavy, fall back to unit-testing a pure `fn admin_allowed(&AuthenticationState) -> bool` helper and note the integration-test gap.
- [ ] 7.2.4 **Scope guard:** do NOT migrate the existing scattered `is_admin()` / `if let Authenticated(u) && u.role != Admin` checks (`pages`, `attachments`, `preview`) behind this layer in Phase 7 — that needs a full route audit (CLAUDE.md). It's in Tech debt.

### 7.3 Analytics dashboard view

- [ ] 7.3.1 Layout mock — sketch the page (ASCII or a short written spec): top-bar, a time-window selector, then "Requests (last N days): N", "Distinct IPs: N", a "Top paths" table, a "Top user-agents" table, a "Requests per day" table (date → count, maybe a CSS bar). Plain HTML + existing Tailwind classes; **no JS charting library**. Get a thumbs-up before building the template.
- [ ] 7.3.2 `GET /admin/analytics` handler (in `admin_router`): reads `?since=` (default per the decision), runs the `RequestLogDao` query bundle, renders askama `templates/analytics/dashboard.html`. Returns `Result<Response, AppError>`. No `is_admin()` check in the handler — the layer owns it.
- [ ] 7.3.3 `templates/analytics/dashboard.html` — extends the base layout; fields: `top_bar`, `auth_state`, the window, and the aggregates. Tables only.
- [ ] 7.3.4 Nav: conditional "Analytics" link in the top-bar partial gated on `auth_state.is_admin()` (or the chosen admin-dropdown form).
- [ ] 7.3.5 `cargo build` + `cargo clippy --all-targets` clean; `cargo test` green. Deploy via `git push origin main`; on the mini, hit `/admin/analytics` logged in as admin, generate traffic, confirm rows accumulate (and trust the 7.1.2 unit test for pruning, or check after the retention window).

### 7.4 Docs + exit

- [ ] 7.4.1 CLAUDE.md: `request_log` table + logging middleware under "Runtime architecture" (or a short "Analytics" blurb); the `/admin` nest + `require_admin` layer under "Web layer", noting it's the *one* place auth is layer-enforced (the rest is still per-handler — see Tech debt).
- [ ] 7.4.2 SPEC.md: mark the "Analytics, who is scraping my site?" line as in-progress/done.
- [ ] 7.4.3 Sweep to PLAN_ARCHIVE.md once 7.1–7.4 are ticked and it's been live a few days.

## Tech debt (untriaged — triage into phases when picked up)

- **Routing model is "too clever" (the `special_page` fallout).** `content_pages` is a self-referential tree that simultaneously (a) serves nested rendered-Markdown content, (b) carries `special_page` rows whose `page_markdown` is a *redirect target URL*, not content, and (c) is dispatched by a top-level router that special-cases the redirect rows while *also* breaking out to dedicated application routers (`/login`, `/projects`, soon `/admin`). Three concerns — content node / routing redirect / dedicated app page — conflated in one table + one dispatch path. A cleaner design separates them (content pages stay a tree; "special"/app routes become plain axum routes, not DB rows). Touches `redirect_to_first_page`, `pages/mod.rs` dispatch, `ContentPageDao::find_by_path`, the `0007` seed migration, `projects.rs`.
- **Authorization is per-handler and inconsistent.** Two idioms in the tree: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` (`preview.rs`, `attachments.rs`) and `if let AuthenticationState::Authenticated(u) = … && u.role != Role::Admin { return FORBIDDEN }` (`pages/mod.rs::delete_page_path`). No route-group enforcement anywhere. Phase 7 introduces a `require_admin` layer for the new `/admin` nest; the follow-up is to audit every existing mutating route and either move it behind a layer or a uniform `AdminUser` extractor, and converge on one idiom. (CLAUDE.md explicitly warns: audit every route first.)
- **Tailwind/DaisyUI build pipeline is non-portable + non-reproducible.** `build.rs` downloads `tailwindcss-macos-arm64` (hardcoded OS+arch — fails on Linux/x86, e.g. any non-self-hosted CI runner) and DaisyUI from `…/releases/latest/…` (unpinned → builds aren't reproducible). Also `styles/tailwind.css` only `@plugin "@tailwindcss/typography"` — no `@plugin "daisyui"` — so DaisyUI may be downloaded but unused; confirm and either wire it in or stop fetching it. Fixes: pin versions, make the CLI fetch arch/OS-aware (or vendor it, or use the npm `@tailwindcss/cli` package), resolve the DaisyUI question.
- **`SessionData::from_request_parts` has a load-bearing `.unwrap()`** (carrying a `//Unsure how to do this without an unwrap` comment) on the session-store read — a transient SQLite error there panics the request instead of degrading. Map it to `Ok(SessionData::default())` (treat a read failure as "no session") or surface it as a 500 via the rejection type.
