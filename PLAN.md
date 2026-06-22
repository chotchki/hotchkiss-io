# Plan

Completed phases are in `PLAN_ARCHIVE.md` (most recent: Phase 1 — `get_recs_by_name` `type=A` filter fix (ACME renewal hang); Phase 9 — Tailwind cleanup / dropped DaisyUI; Phase 8 — local/e2e test harness; Phase 7 — admin analytics dashboard; Phase 2 — DNS module testability; Phase 5 — dropped the `cookie-rs` fork; Phase 3 — `ifconfig.me` → Cloudflare `cdn-cgi/trace`; Phase 0 — push-to-deploy on the Mac mini; Phase 4 — `tray-wrapper` 0.4.1 bump).

## Phase 6 - DNS follow-ups (parked)

Two items deferred out of Phase 2 — neither urgent, both real.

- [ ] 6.1 - Higher-level tests for `CloudflareClient::clean_proof` / `create_proof` / `update_dns` (the set-arithmetic + sequencing logic). Requires `BASE_URL` in `cloudflare_api.rs` to stop being a `LazyLock<Url>` const and become a `CloudflareApi` field so a mock server URL can be injected; then use `wiremock` (decided in Phase 2.3 — `mockito` is fine too, `wiremock`'s async-first API fits the codebase better). Not built yet because the regression class that actually bit us (a pinned query param) is now covered by the pure URL-builder unit tests, and this is mostly orchestration.
- [ ] 6.2 - Re-evaluate `DnsValidator`'s disabled timeout. The `//if … timeout …` blocks in `ensure_exists` / `ensure_not_existing` are commented out, so both are unbounded retry loops today. The original condition was also backwards (`timeout > Instant::now()` where `timeout = now + 300s` is true for the whole window → would bail on iteration 1; the corrected form `Instant::now() > timeout` is now in the comment). Re-enabling it is a *runtime behavior change* to the ACME path — a slow DNS propagation would make a renewal *fail* after 5 min instead of eventually succeeding. Decide whether bounded-with-bail or unbounded is what we want before flipping it on; if bounded, the `WaitStep` decision split from Phase 2.4 makes the loop easy to test with a short timeout.

## Phase 10 - Mini Blog v1

Slice (a) of the mini blog + mobile-posting arc — see SPEC.md "Mini Blog (v1)" for what & why. Slice (b) (editor facelift) is a separate phase, expected to start when dogfooding slice (a) from a phone surfaces concrete editor pain.

- [ ] 10.0 - Phase exit: blog live in prod, dogfooded from a phone, slice (b) SPEC'd from real complaints.
- [x] 10.1 - Migration `0010_DMLBlogSpecialPage.sql` — seed the `blog` special_page (page_markdown="/blog", page_order chosen to match site nav intent). Mirror the `projects` row in 0007.
- [x] 10.2 - `ContentPageDao::find_blog_posts()` — children of the `blog` special_page ordered by `page_creation_date DESC`. Unit tests via `#[sqlx::test]`. (Implemented as generic `find_by_parent_newest_first`.)
- [x] 10.3 - Excerpt helper: first paragraph of markdown → plaintext → ~200 chars. Pure function (likely under `web/markdown/`). Unit tests for edge cases: empty, leading image-only, very long, leading code block, leading heading.
- [x] 10.4 - `/blog` nested router + `templates/blog/index.html` — cards (cover or fallback, date, title, excerpt), newest first, hand-rolled Tailwind, mobile-first responsive grid, empty state.
- [x] 10.5 - `/blog/<slug>` route — reuse the existing get_page rendering by delegating to `find_by_path(&["blog", slug])`. 404 if not found. (Required absolute-path PUT in the editor form so saves work from either URL.)
- [x] 10.6 - `/blog/feed.xml` Atom feed (latest 50, `application/atom+xml` content-type) + `<link rel="alternate" type="application/atom+xml">` in the layout head. Hand-written XML.
- [x] 10.7 - PWA icon set: render 192×192, 512×512, 180×180 (apple-touch-icon), 512×512 maskable PNGs from `HotchkissLogo.svg`. Commit under `assets/images/`. Regen recipe in `build/regen-icons.sh`.
- [x] 10.8 - `assets/manifest.webmanifest` (name, short_name, start_url="/", display="standalone", theme/background colors matching the site palette, icons array) + `<link rel="manifest">` and `<link rel="apple-touch-icon">` in the layout head. rust-embed serves it with `application/manifest+json` via mime_guess.
- [x] 10.9 - Add `capture="environment"` to the attachment upload `<input type="file">` in `templates/pages/list_attachments.html`. One-attribute change — the only editor change in this phase.
- [x] 10.10 - Integration tests (`tests/web.rs` style via `spawn_test_server`): `/blog` empty state, `/blog` with a seeded post (verify card content), `/blog/<slug>` returns 200/404, `/blog/feed.xml` returns Atom with the seeded entry, `/manifest.webmanifest` served with the right MIME.
- [x] 10.11 - CLAUDE.md update: added `/blog` to the routing list under "Web layer"; noted the `blog` special_page mirrors `projects`.
- [x] 10.12 - Compressed the PLAN.md backlog "Mini blog + mobile-posting editor facelift" line — slice (a) is now Phase 10; reduced to a one-liner pointing at SPEC for slice (b).
- [ ] 10.13 - Deploy + manual phone e2e: install to home screen on iOS, post via the existing editor with `capture="environment"`, verify camera flow, verify the post shows up on `/blog`. Capture concrete editor pain as the slice (b) SPEC seed.

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

## Phase 12 - Beta deployment on the mini

Same mini, alternate ports, snapshot-from-prod data on each beta deploy, inverted code flow: `main` → beta (always bleeding edge); `vX.Y.Z` tag → prod (deliberate promotion). Closes the prod-contamination risk that Phase 10/11 dogfooding hit, gives a real HTTPS surface for PWA install on the LAN, and separates "code lands" from "code ships to the public."

Beta is **public** (decided 2026-06-22 — chris is often off-LAN, so LAN-only wouldn't be reachable): a grey-cloud (DNS-only) `beta.hotchkiss.io` A record that beta's own `DnsProviderService` keeps pointed at the public IP (like prod's `hotchkiss.io`), reached on `:8443` via a router port-forward to the mini. Grey-cloud, not orange/proxied, so beta serves its own LE cert end-to-end (orange would put Cloudflare's cert in front and break the trusted-PWA-cert story). iCloud-Keychain passkeys from prod authenticate against beta because beta's WebAuthn rp_id is `hotchkiss.io` (the registrable parent) and the snapshot carries prod's `users` over. Beta data is intentionally ephemeral: prod's `database.sqlite` is snapshotted to beta on every `main` push, so any beta-only edits get blown away on the next deploy — that's the point. Beta stays a beta. **Public-beta safety:** WebAuthn server records are public keys, not secrets (can't forge auth without the authenticator's private key), and the snapshot is prod→beta one-way (beta registrations never reach prod), so a public beta exposes no forgeable credentials; the snapshot also scrubs `request_log` for visitor-IP privacy. Carrying prod's `users` (rather than preserving beta's own) keeps beta's user table non-empty, avoiding the first-user-becomes-admin land-grab a public, empty-table beta would invite.

- [ ] 12.0 - Phase exit: `main` push → beta rebuilds + restarts with snapshotted prod data on `https://beta.hotchkiss.io:8443/`; iPhone installs the PWA from beta over real, publicly-trusted LE HTTPS (beta is a release build → LE prod → natively trusted, no profile install); existing prod passkey auths against beta; tag push → prod rebuilds + restarts on `https://hotchkiss.io/`; both run side-by-side on the mini.
- [x] 12.1 - `Settings`: add `webauthn_rp_id: Option<String>` (defaults to `domain` when absent). Update `EndpointsProviderService::create` to pass it to `WebauthnBuilder` instead of `settings.domain`. Beta uses `hotchkiss.io` so chris's existing prod passkey authenticates against beta too. (Done 2026-06-22: resolved to a concrete `String` in `Settings::resolve`; origin stays the served domain; unit test `load_with_webauthn_rp_id` + default-from-domain assertions.)
- [x] 12.2 - `build/macos/build.sh`: take a `--profile beta|prod` flag. Profile determines bundle name (`Hotchkiss-IO.app` vs `Hotchkiss-IO-Beta.app`), install path, LaunchAgent label. Today's path becomes the `prod` case; `prod` is the default if `--profile` is absent.
- [x] 12.3 - LaunchAgents: **kept prod as `io.hotchkiss.web` (no rename** — renaming a live agent is a bootout/bootstrap migration for zero functional gain); added `build/macos/io.hotchkiss.web.beta.plist` (label `io.hotchkiss.web.beta`, runs `Hotchkiss-IO-Beta.app` with an explicit beta config path as `argv[1]` — prod relies on the default config location, so beta must point at its own or it'd read prod's; beta launchd log dir `~/Library/Logs/io.hotchkiss.web.beta/`). Both `RunAtLoad`; the 12.4 post-receive kickstarts the matching label on swap. `plutil -lint` clean. SETUP.md notes the beta-agent prereqs.
- [x] 12.4 - `build/macos/post-receive`: dispatch by ref — `refs/heads/main` → `build.sh --profile beta` → swap `Hotchkiss-IO-Beta.app` → kickstart `io.hotchkiss.web.beta`; `refs/tags/v*` → `--profile prod` → swap `Hotchkiss-IO.app` → kickstart `io.hotchkiss.web` (= today's behavior, now gated on a version tag). Factored into a profile-parameterized `deploy()` with per-profile src/target dirs; beta-only 12.5 snapshot hook-point stubbed before the kickstart. `bash -n` + routing test clean. **Repo file only — the live cutover (re-copy the hook onto the mini) is part of the 12.8 sequence, not done by this edit.**
- [x] 12.5 - Beta DB snapshot in `post-receive` (`snapshot_prod_db_into_beta`, beta branch only): consistent online `sqlite3 .backup` of **live** prod (`~/Library/Application Support/io.hotchkiss.web/data/database.sqlite` — prod kept its un-suffixed dir, see 12.3) into beta's path — **`.backup`, not `cp`**, since prod may be mid-write (decided 2026-06-22). Then `DELETE FROM crypto_keys` (beta regenerates its own session-signing key on boot) + `DELETE FROM request_log` (visitor-IP privacy — beta is public) + `DELETE FROM tower_sessions` (sessions don't cross); users/passkeys carry over so chris's prod passkey authenticates on beta (and the table stays non-empty → no first-user-admin land-grab). **Cert preservation:** dump beta's `certificates` rows *before* the overwrite, drop prod's carried-over rows *after*, restore beta's — so beta never re-orders `beta.hotchkiss.io` from LE prod (the 5/week duplicate-cert limit would take beta HTTPS down). Runs before the kickstart. Functionally tested on throwaway DBs (steady-state + first-deploy); `bash -n` clean. Depends on macOS `/usr/bin/sqlite3`.
- [ ] 12.6 - Beta config. **Repo-side done:** committed template `build/macos/beta-config.sample.json` (`domain=beta.hotchkiss.io`, `webauthn_rp_id=hotchkiss.io`, `http_port=8080`, `https_port=8443`, beta `database_path`/`log_path`/`cache_path`, placeholder for the CF token (same as prod — see 12.7) — **no `static_ip`**: beta is public and discovers its IP like prod) + SETUP.md §8 beta bring-up runbook. JSON validated. **Mini-side pending (needs the 12.7 token):** copy the template to `~/Library/Application Support/io.hotchkiss.web.beta/config.json` and fill in the beta CF token + mini LAN IP.
- [x] 12.7 - Cloudflare + router (one-time): **reuse the prod CF token** for beta — CF can't scope narrower than the zone (decided 2026-06-22; only trades away independent revocation/audit). `beta.hotchkiss.io` A record exists (placeholder `127.0.0.1`, grey-cloud) — beta's `DnsProviderService` reconciles it to the live public IP on first boot (`update_dns` creates the public-IP record + deletes the placeholder; name-scoped, never touches prod's `hotchkiss.io`). Cert issuance is DNS-01, so it's A-record-independent anyway. Router forwards `:8443` **and** `:8080` → the mini (done). Grey-cloud, not orange/proxied, so beta serves its own end-to-end LE cert.
- [ ] 12.8 - Bootstrap the inverted flow: cut `v0.0.42` (or the current Cargo version) from today's main. Tag-push to origin. Confirm the new post-receive routes the tag to a prod build/swap. After this commit, prod stops auto-updating from main — only tags promote.
- [ ] 12.9 - CLAUDE.md update: document the prod+beta model, alternate ports, snapshot lifecycle, rp_id story, the inverted deploy flow (push main = beta, tag = prod). Absorbs Phase 11.8 (the `http_port`/`https_port`/`static_ip` settings docs).
- [ ] 12.10 - Manual e2e on the iPhone: push a `main` commit → beta rebuilds with snapshotted prod data → install PWA from `https://beta.hotchkiss.io:8443/` (real LE prod cert, natively trusted — no profile) → existing chris passkey authenticates → edit a blog post on beta → tag-push the change to `v0.x.y+1` → prod deploys, post lands in prod's DB on next push-main → snapshot. Phase exit.
- [ ] 12.11 - Retire Phase 11.3, 11.8, 11.9 (content absorbed into 12.6, 12.7, 12.9, 12.10). Update PLAN.md. (Phase 11 folded 2026-06-22; this box stays as the marker that the absorbed content actually lands in 12.6/12.9/12.10.)

## Phase 13 - Landing page + portfolio spine

See SPEC.md "Portfolio — the three pillars". The landing page is the connective tissue: orient a visitor in seconds, route to the three pillars (Software / 3D / Resume).

- [ ] 13.0 - Phase exit: a visitor hitting `/` understands who chotchki is within seconds and can reach all three pillars (Software / 3D / Resume); the layout is clean on a 390px phone.
- [ ] 13.1 - Decide landing-page IA: hero (name + one-line value prop + what I do), three pillar doors (Software / 3D / Resume), links out (GitHub, contact/email). Wireframe it in SPEC.
- [ ] 13.2 - Implement the home page: replace the `/`→first-content-page redirect with a real landing template (or designate a landing content_page). Hand-rolled Tailwind, mobile-first.
- [ ] 13.3 - Top-nav surfaces the three pillars; verify it doesn't overflow at 390px (the Phase-10 dogfood nav fix — confirm it already shipped or land it here).
- [ ] 13.4 - Identity/jumbotron block stacks on narrow screens (confirm the dogfood min-width fix is shipped or land it here).
- [ ] 13.5 - Clear contact + GitHub links and a "resume / hire me" call-to-action above the fold.
- [ ] 13.6 - e2e (`tests/e2e_browser.rs`, mobile viewport): `/` renders the three pillar doors and has no horizontal scroll at 390px.
- [ ] 13.7 - CLAUDE.md + SPEC update: document the real landing page replacing the `/` redirect.

## Phase 14 - Software project showcase

See SPEC.md Pillar 1. The side projects are the *verifiable* half of the pitch — public, clickable proof of range that the less-visible background work can't show.

- [ ] 14.0 - Phase exit: `/projects` presents 3–5 curated software projects, each with its own page (what it does / why / problem & role / media / link-out); "this site" is one entry, not the headline.
- [ ] 14.1 - Curate the lead set: pick 3–5 projects (GitHub + unposted), a one-sentence hook each, decide order. List candidates in SPEC.
  - [ ] 14.1.1 - Curation criterion: prioritize range/breadth and publicly clickable/verifiable projects over "most polished" — these compensate for the less-visible background work.
- [ ] 14.2 - Define the project-page shape: title, summary, problem/role, tech, screenshots/demo, GitHub link. Decide plain content_page vs a richer template.
- [ ] 14.3 - Decide curation mechanism — hand-authored content_pages vs GitHub-API pull. Recommend hand-curate the lead set; defer auto-listing.
- [ ] 14.4 - Author the lead project pages (content + media).
- [ ] 14.5 - "This site" project page — surface the existing engineering (self-hosted Rust, DNS/ACME, passkeys/HTMX, push-deploy, tray app) as one entry, not the headline.
- [ ] 14.6 - `/projects` index polish: cards, ordering, mobile layout, empty/fallback state.
- [ ] 14.7 - e2e coverage for `/projects` + a project page; CLAUDE.md/SPEC update.

## Phase 15 - 3D printing / OpenSCAD gallery

See SPEC.md Pillar 2. Tangible range in a different medium. The bulk loader is deferred — ship ~5 hand-picked pieces first.

- [ ] 15.0 - Phase exit: a 3D pillar shows ~5 hand-picked prints with model viewer + photos + description; STL + OpenSCAD source downloadable where applicable; the bulk loader is explicitly deferred.
- [ ] 15.1 - Hand-pick ~5 showpiece prints/models; list in SPEC with a one-line why each.
- [ ] 15.2 - Confirm the existing STL viewer (`.stl`→`<object class="stl-view">` rewrite) works for gallery use; verify it renders on mobile.
- [ ] 15.3 - Define the 3D-piece page shape: render/photo, description, STL viewer, downloadable STL + OpenSCAD source.
- [ ] 15.4 - Auto-generate a lower-res STL (SPEC goal) — decide build-time vs on-upload; may defer.
- [ ] 15.5 - Author the 5 gallery entries (photos + descriptions + files).
- [ ] 15.6 - e2e coverage for the 3D gallery; CLAUDE.md/SPEC update.

## Phase 16 - Resume / background capture

See SPEC.md Pillar 3. The substance and the long pole: making less-visible work credible, not just recording it.

## Backlog (not yet phased)
### Tech debt
- **Routing model is "too clever" (the `special_page` fallout).** `content_pages` is a self-referential tree that simultaneously (a) serves nested rendered-Markdown content, (b) carries `special_page` rows whose `page_markdown` is a *redirect target URL*, not content, and (c) is dispatched by a top-level router that special-cases the redirect rows while *also* breaking out to dedicated application routers (`/login`, `/projects`, soon `/admin`). Three concerns — content node / routing redirect / dedicated app page — conflated in one table + one dispatch path. A cleaner design separates them (content pages stay a tree; "special"/app routes become plain axum routes, not DB rows). Touches `redirect_to_first_page`, `pages/mod.rs` dispatch, `ContentPageDao::find_by_path`, the `0007` seed migration, `projects.rs`.
- **Authorization is per-handler and inconsistent.** Two idioms in the tree: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` (`preview.rs`, `attachments.rs`) and `if let AuthenticationState::Authenticated(u) = … && u.role != Role::Admin { return FORBIDDEN }` (`pages/mod.rs::delete_page_path`). No route-group enforcement anywhere. Phase 7 introduces a `require_admin` layer for the new `/admin` nest; the follow-up is to audit every existing mutating route and either move it behind a layer or a uniform `AdminUser` extractor, and converge on one idiom. (CLAUDE.md explicitly warns: audit every route first.)
- **`SessionData::from_request_parts` has a load-bearing `.unwrap()`** (carrying a `//Unsure how to do this without an unwrap` comment) on the session-store read — a transient SQLite error there panics the request instead of degrading. Map it to `Ok(SessionData::default())` (treat a read failure as "no session") or surface it as a 500 via the rejection type.
### Ideas
- **Analytics: status / "noise" view.** Status-code breakdown (200 vs 404 vs 403 …) and a "paths that only ever 404" table — effectively a scanner-signature list. Small; extends the `request_log` queries + the dashboard template.
- **Analytics: per-IP drill-down.** Group by IP; flag IPs hitting many distinct 404 paths (classic scan fingerprint) vs. real visitors. A new query + a sub-page or section.
- **Analytics: referer breakdown.** `referer` is already recorded by the logging middleware, just not surfaced — add a `count_by_referer` query + a table.
- **Analytics → defense: dumb IP blocklist.** Derive a blocklist from `request_log` (N 404s in M minutes → drop the IP for a while), enforced by an early middleware layer. Bigger — its own phase (blocklist storage + decay, the enforcing layer, an admin view/override, false-positive handling).
- **e2e: exercise the conditional-auth / autofill login path.** Phase 8.4's browser e2e drives the passkey *registration* ceremony cleanly, but the `webauthn-autofill` flow in `htmx-webauthn.js` (conditional `navigator.credentials.get()` on page load → `/login/get_auth_opts` → `/login/finish_authentication`) isn't tested — and per the original author that's where hidden footguns lurk. Add an e2e that, with a pre-registered virtual-authenticator credential, opens `/login` in a fresh context and verifies the autofill auth completes and lands logged-in. May surface bugs in the extension → could spin off its own phase.
- **Editor facelift** — slice (b) of the mini blog + mobile-posting arc. Slice (a) shipped as Phase 10. SPEC writeup waits on Phase 10 dogfooding to surface real pain (see 10.13). Constraints still apply: stay HTMX-first (Leptos islands only if HTMX hits a real wall on live-preview / drag-reorder / complex client state); Tailwind cleanup (Phase 9) is done; DaisyUI dropped; the "easier project loader" remains explicitly out of scope — bigger and deferred.
- **Staging / beta deployment.** A real deployed instance serving `beta.hotchkiss.io` (a DNS entry that already exists, used for Let's Encrypt testing) — complements Phase 8's local harness ("does it survive in the wild"). `Settings` already supports this with no code change: a beta `config.json` with `domain = beta.hotchkiss.io`, its own `database_path`, and its own Cloudflare token (separate token = independent revocation / rate-limit / audit — *not* blast-radius isolation, since CF tokens scope per-zone and `beta.` is in the `hotchkiss.io` zone; real isolation would need a delegated zone). The actual blocker is the hardcoded `:80`/`:443` (can't coexist with prod on the mini) → make ports configurable (which Phase 11 does); then it's "second machine, or port-mapped local instance". **Promoted to Phase 12 (drafted).**
- **Warn when main has commits past the latest prod tag.** With Phase 12's inverted code flow (`main` → beta, `vX.Y.Z` tag → prod), it'd be easy to forget to tag and let prod silently lag. A small pre-push hook or `git status`-style helper that prints "main is N commits ahead of v0.x.y" would close the gap. Could also surface in the analytics dashboard ("running version vs latest commit"). Small follow-up to Phase 12.
- **Beta-only registration → prod-usable passkey (rp_id design implication).** With Phase 12's `webauthn_rp_id = hotchkiss.io` on beta, *any* passkey registered against beta is implicitly authorized for prod too (and vice versa) — they share an rp_id. For chris-as-sole-admin that's the goal (existing prod passkey works on beta). If users ever register on beta, their credentials would also work on prod after the next snapshot. Mitigations to consider before opening registration on beta: (a) disable public registration on beta (admin-only gate via config), (b) split rp_ids and accept that prod passkeys won't work on beta (need iCloud sync or re-register), (c) snapshot strips non-admin credentials on the way in.
- [ ] 16.0 - Phase exit: `/resume` renders a clean, current resume; a downloadable PDF is one click away; the background is captured in a reusable, structured form.
- [ ] 16.1 - Capture the raw history — interview/brain-dump the background (roles, scope, impact, highlights). chotchki + Claude drafting session. The long pole; start in parallel now.
  - [ ] 16.1.1 - Mine the less-visible work for public-safe signal — architecture/problem writeups, scope/scale/impact stated at a safe level, and anything already public (talks, patents, OSS, conference work).
- [ ] 16.2 - Decide resume structure + narrative (chronological vs impact-led), what to lead with, and public vs gated.
  - [ ] 16.2.1 - Narrative strategy: meet the "lots of depth, little public proof" skeptic head-on (depth + progression), and cross-link the résumé to the side projects as the tangible evidence of range.
- [ ] 16.3 - Resume page template + content at `/resume`.
- [ ] 16.4 - Downloadable PDF: decide mechanism (committed static PDF vs generated from a single source of truth).
- [ ] 16.5 - Tie the contact/CTA into the landing page (Phase 13).
- [ ] 16.6 - e2e coverage for `/resume` + PDF download; CLAUDE.md/SPEC update.





  - *Dogfood findings (Phase 10 phone testing, running list):*
    - Top nav (`templates/base.html` `<ul class="list-none flex flex-row">`) overflows the viewport on mobile — no `flex-wrap`, `px-8` per tab, ~5 tabs busts a ~390px iPhone viewport. Whole-site issue, not blog-specific. Likely fix: wrap + tighter mobile padding, possibly a hamburger at xs.
    - `post_page_path` / `post_top_level_page_path` reject non-URI-safe `page_name` (spaces, etc) with a 400 — but htmx swallows non-2xx responses, so submissions silently no-op. The blog "+ New post" form now slugifies on input as a local fix, but the top-nav admin "Create New Page" form and the editor's child-create form (`templates/pages/get_page.html`) still have the silent-fail. Whole-site fix: either slugify server-side in the handlers (any `page_name` → lowercase/hyphenated), or apply the same client-side slugify everywhere, or render an inline error message on 400.
    - Page minimum width exceeds an iPhone portrait viewport (~390px) — `templates/base.html` has the jumbotron as `flex flex-row` (image `size-40` = 160px + name/tagline text alongside) which never wraps, and the un-wrapped nav `<ul>` from the first finding contributes too. User has to rotate to landscape. Likely fix: jumbotron becomes `flex-col sm:flex-row` (or similar) so it stacks on narrow screens; nav fix from finding #1 helps here too.
    - On the phone, can't reach the editor — user reports "not logged in to the website to edit." Need to confirm symptom precisely (no editor chrome / 403 / redirect to login / save fails / something else) and whether (a) PWA cookie scope is separate from Safari, (b) session expired silently (1-day inactivity), or (c) the login passkey ceremony itself doesn't complete on iOS in some path.
