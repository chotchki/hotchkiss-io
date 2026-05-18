# Plan

Completed phases are in `PLAN_ARCHIVE.md` (most recent: Phase 9 — Tailwind cleanup / dropped DaisyUI; Phase 8 — local/e2e test harness; Phase 7 — admin analytics dashboard; Phase 2 — DNS module testability; Phase 5 — dropped the `cookie-rs` fork; Phase 3 — `ifconfig.me` → Cloudflare `cdn-cgi/trace`; Phase 0 — push-to-deploy on the Mac mini; Phase 4 — `tray-wrapper` 0.4.1 bump).

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

## Phase 10 — Mini Blog v1

Slice (a) of the mini blog + mobile-posting arc — see SPEC.md "Mini Blog (v1)" for what & why. Slice (b) (editor facelift) is a separate phase, expected to start when dogfooding slice (a) from a phone surfaces concrete editor pain.

- [ ] 10.0 Phase exit: blog live in prod, dogfooded from a phone, slice (b) SPEC'd from real complaints.
- [x] 10.1 Migration `0010_DMLBlogSpecialPage.sql` — seed the `blog` special_page (page_markdown="/blog", page_order chosen to match site nav intent). Mirror the `projects` row in 0007.
- [x] 10.2 `ContentPageDao::find_blog_posts()` — children of the `blog` special_page ordered by `page_creation_date DESC`. Unit tests via `#[sqlx::test]`. (Implemented as generic `find_by_parent_newest_first`.)
- [x] 10.3 Excerpt helper: first paragraph of markdown → plaintext → ~200 chars. Pure function (likely under `web/markdown/`). Unit tests for edge cases: empty, leading image-only, very long, leading code block, leading heading.
- [x] 10.4 `/blog` nested router + `templates/blog/index.html` — cards (cover or fallback, date, title, excerpt), newest first, hand-rolled Tailwind, mobile-first responsive grid, empty state.
- [x] 10.5 `/blog/<slug>` route — reuse the existing get_page rendering by delegating to `find_by_path(&["blog", slug])`. 404 if not found. (Required absolute-path PUT in the editor form so saves work from either URL.)
- [x] 10.6 `/blog/feed.xml` Atom feed (latest 50, `application/atom+xml` content-type) + `<link rel="alternate" type="application/atom+xml">` in the layout head. Hand-written XML.
- [x] 10.7 PWA icon set: render 192×192, 512×512, 180×180 (apple-touch-icon), 512×512 maskable PNGs from `HotchkissLogo.svg`. Commit under `assets/images/`. Regen recipe in `build/regen-icons.sh`.
- [x] 10.8 `assets/manifest.webmanifest` (name, short_name, start_url="/", display="standalone", theme/background colors matching the site palette, icons array) + `<link rel="manifest">` and `<link rel="apple-touch-icon">` in the layout head. rust-embed serves it with `application/manifest+json` via mime_guess.
- [x] 10.9 Add `capture="environment"` to the attachment upload `<input type="file">` in `templates/pages/list_attachments.html`. One-attribute change — the only editor change in this phase.
- [x] 10.10 Integration tests (`tests/web.rs` style via `spawn_test_server`): `/blog` empty state, `/blog` with a seeded post (verify card content), `/blog/<slug>` returns 200/404, `/blog/feed.xml` returns Atom with the seeded entry, `/manifest.webmanifest` served with the right MIME.
- [x] 10.11 CLAUDE.md update: added `/blog` to the routing list under "Web layer"; noted the `blog` special_page mirrors `projects`.
- [x] 10.12 Compressed the PLAN.md backlog "Mini blog + mobile-posting editor facelift" line — slice (a) is now Phase 10; reduced to a one-liner pointing at SPEC for slice (b).
- [ ] 10.13 Deploy + manual phone e2e: install to home screen on iOS, post via the existing editor with `capture="environment"`, verify camera flow, verify the post shows up on `/blog`. Capture concrete editor pain as the slice (b) SPEC seed.

## Tech debt (untriaged — triage into phases when picked up)

- **Routing model is "too clever" (the `special_page` fallout).** `content_pages` is a self-referential tree that simultaneously (a) serves nested rendered-Markdown content, (b) carries `special_page` rows whose `page_markdown` is a *redirect target URL*, not content, and (c) is dispatched by a top-level router that special-cases the redirect rows while *also* breaking out to dedicated application routers (`/login`, `/projects`, soon `/admin`). Three concerns — content node / routing redirect / dedicated app page — conflated in one table + one dispatch path. A cleaner design separates them (content pages stay a tree; "special"/app routes become plain axum routes, not DB rows). Touches `redirect_to_first_page`, `pages/mod.rs` dispatch, `ContentPageDao::find_by_path`, the `0007` seed migration, `projects.rs`.
- **Authorization is per-handler and inconsistent.** Two idioms in the tree: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` (`preview.rs`, `attachments.rs`) and `if let AuthenticationState::Authenticated(u) = … && u.role != Role::Admin { return FORBIDDEN }` (`pages/mod.rs::delete_page_path`). No route-group enforcement anywhere. Phase 7 introduces a `require_admin` layer for the new `/admin` nest; the follow-up is to audit every existing mutating route and either move it behind a layer or a uniform `AdminUser` extractor, and converge on one idiom. (CLAUDE.md explicitly warns: audit every route first.)
- **`SessionData::from_request_parts` has a load-bearing `.unwrap()`** (carrying a `//Unsure how to do this without an unwrap` comment) on the session-store read — a transient SQLite error there panics the request instead of degrading. Map it to `Ok(SessionData::default())` (treat a read failure as "no session") or surface it as a 500 via the rejection type.

## Backlog (ideas — promote to a phase when ready)

- **Analytics: status / "noise" view.** Status-code breakdown (200 vs 404 vs 403 …) and a "paths that only ever 404" table — effectively a scanner-signature list. Small; extends the `request_log` queries + the dashboard template.
- **Analytics: per-IP drill-down.** Group by IP; flag IPs hitting many distinct 404 paths (classic scan fingerprint) vs. real visitors. A new query + a sub-page or section.
- **Analytics: referer breakdown.** `referer` is already recorded by the logging middleware, just not surfaced — add a `count_by_referer` query + a table.
- **Analytics → defense: dumb IP blocklist.** Derive a blocklist from `request_log` (N 404s in M minutes → drop the IP for a while), enforced by an early middleware layer. Bigger — its own phase (blocklist storage + decay, the enforcing layer, an admin view/override, false-positive handling).
- **e2e: exercise the conditional-auth / autofill login path.** Phase 8.4's browser e2e drives the passkey *registration* ceremony cleanly, but the `webauthn-autofill` flow in `htmx-webauthn.js` (conditional `navigator.credentials.get()` on page load → `/login/get_auth_opts` → `/login/finish_authentication`) isn't tested — and per the original author that's where hidden footguns lurk. Add an e2e that, with a pre-registered virtual-authenticator credential, opens `/login` in a fresh context and verifies the autofill auth completes and lands logged-in. May surface bugs in the extension → could spin off its own phase.
- **Editor facelift** — slice (b) of the mini blog + mobile-posting arc. Slice (a) shipped as Phase 10. SPEC writeup waits on Phase 10 dogfooding to surface real pain (see 10.13). Constraints still apply: stay HTMX-first (Leptos islands only if HTMX hits a real wall on live-preview / drag-reorder / complex client state); Tailwind cleanup (Phase 9) is done; DaisyUI dropped; the "easier project loader" remains explicitly out of scope — bigger and deferred.
- **Staging / beta deployment.** A real deployed instance serving `beta.hotchkiss.io` (a DNS entry that already exists, used for Let's Encrypt testing) — complements Phase 8's local harness ("does it survive in the wild"). `Settings` already supports this with no code change: a beta `config.json` with `domain = beta.hotchkiss.io`, its own `database_path`, and its own Cloudflare token (separate token = independent revocation / rate-limit / audit — *not* blast-radius isolation, since CF tokens scope per-zone and `beta.` is in the `hotchkiss.io` zone; real isolation would need a delegated zone). The actual blocker is the hardcoded `:80`/`:443` (can't coexist with prod on the mini) → make ports configurable; then it's "second machine, or port-mapped local instance". Bundle: configurable ports + beta config/token + the second-instance story.
