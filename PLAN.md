# Plan

Completed phases are in `PLAN_ARCHIVE.md` (most recent: Phase 12 — beta deployment on the mini (inverted `main`→beta / `v*`tag→prod flow); Phase 1 — `get_recs_by_name` `type=A` filter fix (ACME renewal hang); Phase 9 — Tailwind cleanup / dropped DaisyUI; Phase 8 — local/e2e test harness; Phase 7 — admin analytics dashboard; Phase 2 — DNS module testability; Phase 5 — dropped the `cookie-rs` fork; Phase 3 — `ifconfig.me` → Cloudflare `cdn-cgi/trace`; Phase 0 — push-to-deploy on the Mac mini; Phase 4 — `tray-wrapper` 0.4.1 bump).

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

### Tech debt
- **Routing model is "too clever" (the `special_page` fallout).** `content_pages` is a self-referential tree that simultaneously (a) serves nested rendered-Markdown content, (b) carries `special_page` rows whose `page_markdown` is a *redirect target URL*, not content, and (c) is dispatched by a top-level router that special-cases the redirect rows while *also* breaking out to dedicated application routers (`/login`, `/projects`, soon `/admin`). Three concerns — content node / routing redirect / dedicated app page — conflated in one table + one dispatch path. A cleaner design separates them (content pages stay a tree; "special"/app routes become plain axum routes, not DB rows). Touches `redirect_to_first_page`, `pages/mod.rs` dispatch, `ContentPageDao::find_by_path`, the `0007` seed migration, `projects.rs`.
- **Authorization is per-handler and inconsistent.** Two idioms in the tree: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` (`preview.rs`, `attachments.rs`) and `if let AuthenticationState::Authenticated(u) = … && u.role != Role::Admin { return FORBIDDEN }` (`pages/mod.rs::delete_page_path`). No route-group enforcement anywhere. Phase 7 introduces a `require_admin` layer for the new `/admin` nest; the follow-up is to audit every existing mutating route and either move it behind a layer or a uniform `AdminUser` extractor, and converge on one idiom. (CLAUDE.md explicitly warns: audit every route first.)
- **`SessionData::from_request_parts` has a load-bearing `.unwrap()`** (carrying a `//Unsure how to do this without an unwrap` comment) on the session-store read — a transient SQLite error there panics the request instead of degrading. Map it to `Ok(SessionData::default())` (treat a read failure as "no session") or surface it as a 500 via the rejection type.
- **Version single source of truth (Phase 12 review).** Prod tag deploys stamp `CFBundleVersion=0.0.0-dev` (the hook builds a `.git`-less archived tree and passes no `VERSION`), and the runtime/log/tray version is `CARGO_PKG_VERSION` (kept in lockstep by a manual `Cargo.toml` bump per tag — drift-prone; it slipped once at v0.0.43). Fix: `post-receive` derives `VERSION` from the tag and exports it to `build.sh`; `build.rs` forwards it as a `rustc-env`; `lib.rs` prefers it over `CARGO_PKG_VERSION`. One tag-derived version for both the bundle and the runtime.
- **Separate beta Cloudflare token (Phase 12 review).** Beta reuses prod's CF token (CF can't scope a token narrower than the `hotchkiss.io` zone, so a beta-specific token has identical access). A *distinct* same-scope token would let beta be revoked/rotated independently of prod if beta is compromised. Deferred by choice — independent revocation/audit only.
- **Optional: scrub non-admin users from the beta snapshot (Phase 12 review).** Beta carries prod's full `users` table (passkey records are public keys with no network-reachable leak path; chris-as-admin on beta is intentional via shared rp_id). Pure defense-in-depth: `DELETE FROM users WHERE app_role != 'Admin'` in `snapshot_prod_db_into_beta` keeps only the admin row. Not required.

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
- **Publish `ios-inspect` as a crates.io crate.** The iOS-simulator inspection tool used by `tests/e2e_ios.rs` now lives in its own repo `github.com/chotchki/ios-inspect` (split out 2026-06-22 from `skylander-portal-controller/tools/ios-inspect`: the original sibling-repo *path* dep silently broke every non-dev-machine build incl. the mini's prod deploy, and a git-dep on the skylander repo dragged in its giant `rpcs3` submodule). It's now a clean **git** dev-dependency on the standalone repo, pinned via `Cargo.lock`. Publishing it to crates.io would make it a plain versioned registry dep (no git/branch pin) and is independently reusable — its own small project.
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
## Phase BW - GFM tables + fix nested-element rendering (BV walk-depth regression)
- [ ] BW.0 - Phase exit: content pages render GFM tables + math/images/diagrams nested in lists/headings/blockquotes (BV walk-depth regression fixed); live on prod
- [x] BW.1 - Fix transformer walk to descend into ALL containers (lists/headings/blockquotes/emphasis/links), not just Root/Paragraph — math/images/diagrams nested anywhere now convert; test
- [x] BW.2 - GFM tables: enable gfm_table in to_mdast + the to_html re-parse so | a | b | renders as a table; test
- [ ] BW.3 - Docs (CLAUDE.md/SPEC) + deploy beta → verify → tag vX.Y.Z (prod)

## Backlog (not yet phased)

- **Add Biome for first-party JS/CSS lint (augment Prettier)** — added 2026-06-24.
- **Richer interactive analytics dashboard (port recon-gen's d3 pipeline)** — added 2026-06-25.
- **Embed widgets: live-demo iframe + source-code iframe (deep-link line ranges, no copy-paste)** — added 2026-06-26. Two forms. (1) **Page iframe** embeds a live demo app inline (e.g. `recon-gen-spec.hotchkiss.io`) so a project page shows the REAL thing, not a screenshot — surfaced from the recon-gen draft's "I'd really like an Iframe to the demo app instead" note. (2) **Source-code iframe** takes a deep-link WITH a line range (the recon-gen page already links `…/schema.py#L2934-L2953`) and renders the actual source at those lines inline, so a snippet is never copy-pasted into a page. DRY — the repo is the single source, the pasted copy is what drifts (the recon-gen draft today pastes the SQL AND deep-links it, exactly the duplication this kills). Both reusable across every project page (extends the "deep-link + show the snippet" pattern). Open Qs: render target (GitHub's own embed vs raw-fetch + highlight.js into our own block vs an iframe to a `/source/<repo>/<path>?lines=` route on the site), pinning the ref so line ranges don't rot, caching and private-repo handling.
