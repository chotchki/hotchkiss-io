<!-- plan-bridge:phase-high-water=DR -->
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

## Phase CW - Fab / 3D section — host the WASM slicer/placer editor

- [ ] CW.0 - Phase exit: the Fab/3D tab hosts the live WASM slicer/placer editor (cross-origin isolated, own route, consuming the pinned fab-scad release) with models nested; tests + docs; shipped
- [x] CW.1 - Nail the consume-contract with fab-scad (GATING, cross-repo): the GitHub-release asset shape + pinned-download mechanism + confirm SAB/threads
- [x] CW.2 - build.rs: download the pinned fab-scad WASM release into OUT_DIR (mirror the Tailwind-CLI download) + stage for rust-embed
- [x] CW.3 - Dedicated editor route serving the bundle with COOP+COEP scoped to that route only
- [>] CW.4 - Add Cross-Origin-Resource-Policy to the media byte route so the isolated editor can fetch models
- [x] CW.5 - The fab/3d special page + nav tab; models nest as its content-page children
- [ ] CW.6 - Models gallery under the tab: reparent/curate the Phase-15 hand-picked models (existing STL/3MF viewer + fab-publish)
- [ ] CW.7 - Tests (route serves bundle + COOP/COEP scoped; media CORP; models render) + CLAUDE.md + deploy

- [x] CW.8 - Migrate editor bundle fab-web → fab-gui: rename + drop OpenSCAD side-module + boot-splash/flex-column reframe + safe-by-construction wasm drop; pin web-v0.12.0
- [x] CW.9 - Build + test against the published web-v0.12.0: confirm the version pin matches the real tag; cargo test the editor suite (incl. the geom identity guard); boot /3d/editor — tool renders + splash lifts on fab-gui:ready
- [x] CW.10 - Editor under the real site nav: full site header that scrolls away, sticky full-viewport fab-gui tool pinned on scroll (reuse nav partials, COOP/COEP retained)
## Phase DE - The family Library section

- [ ] DE.0 - Phase exit: family members use the Library tab end-to-end (first real audiobook live on prod); everyone else sees only the sign-in gate on code-defined routes
- [x] DE.1 - Migration: seed the library special page ('library','/library',-1,true) mirroring 0023, with min_role='Family' stamped on the row
- [x] DE.2 - /library route (section doors from children, /3d shape) + /library/audiobooks (paginated book cards via listing.rs paginate + search/pager partials); detail pages stay on get_page_path
- [x] DE.3 - Sign-in gate on ALL code-defined /library routes: logged-out copy ("sign in" + ?next link) vs authenticated-insufficient copy ("restricted", no tier names); get_page_path still redirects a special leaf whose ONLY failure is role (nav links /pages/library); oracle tests for both states + data stays miss-shaped
- [x] DE.4 - Login ?next: validated (leading /, second char not / or \, NO backslash anywhere — test vectors /\evil.com + //evil.com) and SESSION-stashed at login_page/get_auth_opts/start_register; finish handlers pop + redirect; htmx-webauthn.js navigates to response.url instead of hardcoded "/"
- [x] DE.5 - Add the /library index (exact path, not subtree) to greylist EXEMPT_PREFIXES beside /login — a greylisted logged-out family member must reach the sign-in gate, and the gate serves nothing scrapable
- [ ] DE.6 - Author the first real audiobook end-to-end on beta (upload gated m4b → book page → family listen) then prod tag
- [x] DE.7 - Browser e2e (register 2nd virtual-authenticator user, promote via server.pool): Family sees Library tab + book page renders; anonymous: no tab, cat-404 book, sign-in gate on /library
- [x] DE.8 - CLAUDE.md major update: role ladder, min_role axes, Library section, audio kind, the new request_log exclusions

## Phase DF - Listening progress sync

- [ ] DF.0 - Phase exit: cross-device resume works (phone→iPad), saves survive screen-off listening, no analytics/greylist pollution. Deferrable phase.
- [ ] DF.1 - Migration: playback_progress (PK user_id+media_id, media_id FK ON DELETE CASCADE matching media_variant; user rows wiped in delete_user's tx alongside api_keys)
- [ ] DF.2 - POST /library/progress {media_ref, position_ms} via the role-scoped allowlist (Family) + handler re-checks the media's min_role; GET returns the session user's row with Cache-Control: no-store (cached position would defeat the phone→iPad handoff)
- [ ] DF.3 - Player server-resume swap (localStorage stays as fallback); saves ~30s + pause + visibilitychange→hidden + pagehide beacon (throttle is the real guarantee on iOS); save rejection → "session expired, sign in" prompt, never a silent stall
- [ ] DF.4 - Exclude /library/progress from request_log (machine telemetry, ~120 rows/listening-hour — same self-feed logic as /challenge; keeps it out of greylist R3)
- [ ] DF.5 - Tests + CLAUDE.md delta + phone re-check: progress saves continue with the screen off
## Phase DG - Series playlist
- [ ] DG.0 - Phase exit: a multi-volume series page auto-plays through volumes with the screen off on a real iPhone; standalone books (single embed) unchanged
- [x] DG.1 - Player playlist mode: a page's audio embeds (document order) form an ordered playlist — on `ended` advance to the next volume
- [x] DG.2 - MediaSession volume skip: nexttrack/prevtrack map to next/previous volume; lock-screen metadata (title + artwork) updates per volume
- [ ] DG.3 - Up-next affordance: on page open, highlight/scroll to the last-listened volume (per-ref saved positions decide it)
- [ ] DG.4 - Tests + CLAUDE.md delta + real-phone validation: screen-off auto-advance, lock-screen volume skip, series authoring convention documented
- [x] DG.5 - Audio embed: cover-art + title header replaces the download button

## Phase DN - Open a model in the slicer (public load widget)

*Half 1 of the fab-scad model round-trip. fab-gui is a SCAD SLICER — it already loads `?model=<scad-url>` as TEXT on boot (agent-verified in the pinned web-v0.16.x); STL/3MF mesh-import is a hard-stub on the wasm worker. So this half is the PUBLIC READ path: store the SCAD (public, `application/x-openscad`), surface an "Open in the slicer" button that hands fab-gui the SCAD URL. The EDIT round-trip (fab-scad export of STL/3MF + authenticated same-origin upload + update-in-place on the STABLE `media_ref`, so content links never go stale) is chris's UPSTREAM fab-scad work + a later phase — NOT this one.*

- [ ] DN.0 - Phase exit: a public "Open in the slicer" button on a model page opens fab-gui at `/3d/editor?model=/media/file/<scad-url_key>` and the model loads + slices; SCAD stored as `application/x-openscad`; the three.js mesh viewer is unaffected by the scad variant; tests + docs; shipped.
- [x] DN.1 - `ModelFormat` enum (`Scad`/`Stl`/`ThreeMf`) + `from_mime` + `is_mesh()` — the typed discriminator replacing the fragile `mime == "model/3mf"` / `starts_with("model/")` string-matching in the embed dispatch (strong-type-from-the-start). No migration — reads the existing `media_variant.mime` column.
- [x] DN.2 - Ingest: probe `.scad` (by extension, like `.stl`/`.3mf`) → mime `application/x-openscad`, kind `MediaKind::File` (a standalone scad is a downloadable source; grouped with a mesh, `dominant_kind` keeps the item `Stl`). Unit test beside the `.stl`/`.3mf` probe test.
- [x] DN.3 - Byte route: serve `application/x-openscad` as-is (fab-gui reads text; a browser downloads the source), + add `cross-origin-resource-policy: cross-origin` so the COEP:require-corp editor can fetch it (resurrects the deferred CW.4; `cross-origin` keeps public media hotlinkable — NOT `same-origin`; CORP doesn't bypass the `min_role` gate, which is enforced separately). Verify the editor fetch end-to-end.
- [x] DN.4 - Embed dispatch (`render_embed_html`): the `Stl` arm's mesh selection goes through `ModelFormat::is_mesh` (scad excluded by its `application/` prefix anyway), and when the item has a scad variant, render an **"Open in the slicer"** button → `/3d/editor?model=/media/file/<scad-url_key>`; the `File` arm renders the slicer button (not a plain download) when the file IS a scad. New `open_in_slicer_button()` helper beside `download_button`.
- [x] DN.5 - Tests + e2e + docs: unit (`ModelFormat::from_mime`; `.scad` probe); embed (slicer button for a scad-carrying `Stl` item AND a standalone scad `File`; mesh-viewer selection unaffected by the scad variant; button URL correct); e2e (`/3d/editor?model=/media/file/<key>` boots + fab-gui loads the scad); CLAUDE.md delta.

## Phase DO - Update a model in place (fab-scad round-trip, Half 2 site-side)

*Half 2 of the fab-scad model round-trip — the SITE side. DN shipped the LOAD (open the SCAD in the slicer); this closes the loop with the SAVE TARGET so a logged-in Admin's fab-gui edit re-homes onto the SAME media item. The verb is **`PATCH /media/<ref>`** — the item's stable ref IS its identity, and the fail-closed mutation layer gates any non-GET to Admin FOR FREE (the WebAuthn + role-scoped allowlists are both POST-only, so a PATCH structurally can't slip past the admin fallback — no new authz wiring, and it can't be forgotten). Semantics: **COMPLETE replacement** (chris's call) — the uploaded multipart file set BECOMES the item's entire variant set (old variants wiped in one tx), the item row (ref/title/`min_role`) preserved so every `![](/media/<ref>)` embed stays valid with zero rewrite; a render-image thumbnail not re-uploaded is dropped (the uploaded set is authoritative). Replace-not-version: the old blobs go cold on disk (content-addressed, Backblaze-backed — the delete path already doesn't sweep). The fab-gui export + same-origin authenticated upload is chris's UPSTREAM fab-scad work (currently BLOCKED on this landing); this phase gives it a POST target + freezes the contract in `docs/fab-scad-roundtrip.md`.*

- [x] DO.0 - Phase exit: `PATCH /media/<ref>` replaces a model item's variants in place (Admin-only, complete replace), ref/title/gate preserved and embeds unbroken; the contract doc marks the site side SHIPPED; tests green; shipped.
- [x] DO.1 - DAO: `MediaVariantDao::delete_all_for_media` + `MediaDao::update_facts` (re-derive `kind`/dims/`duration_ms`/`chapters` from the new primary), both executor-generic so the handler runs wipe→insert→re-derive in ONE transaction. Unit test: the variant set replaces in place while the item identity (ref/title/`min_role`) stays untouched.
- [x] DO.2 - Extract the streaming-ingest helper from `upload_media` (stage→commit→probe each file part + parse the text fields) so upload (mint) and PATCH (replace) share ONE O(chunk)-memory ingest path; `upload_media` refactored onto it, behavior byte-identical (its existing media-vertical test still passes).
- [x] DO.3 - Handler `patch_media_by_ref`: resolve the ref (404 unknown), ingest the file set (400 if empty — a replace-to-nothing is a DELETE, not a PATCH), tx-replace the variant set + re-derive facts (ref/title/gate preserved, new variants INHERIT the item's `min_role`), best-effort poster/responsive parity with upload, `200` JSON `{media_ref, kind, variants:[{url_key,mime,bytes}]}`.
- [x] DO.4 - Route: `.patch(patch_media_by_ref)` on the public `/{media_ref}` route + `DefaultBodyLimit::disable()` (multi-GB model set); Admin-gating INHERITED from `require_admin_for_mutations` (no allowlist entry — a non-safe method hits the admin fallback).
- [x] DO.5 - Tests: authz (anonymous PATCH → 401), unknown ref → 404, empty → 400, happy replace (old `url_key` 404s, new serves, ref + title preserved), gate preserved (upload `Family` → PATCH → new variant still gated: anon denied / admin 200).
- [x] DO.6 - Docs: `docs/fab-scad-roundtrip.md` — mark the site side SHIPPED + freeze the contract (`PATCH /media/<ref>`, cookie-auth Admin, multipart file parts typed by extension, complete-replace, cold blobs, response shape); CLAUDE.md media-section delta.

## Phase DP - Media resource: content negotiation + discovery (read half)

*The READ half of the HATEOAS media resource (`docs/media-design.md` §5). `/media/<ref>` becomes content-negotiated + self-describing: a caller STATES what representation it wants and DISCOVERS what's there, instead of the server heuristically redirecting to the largest variant. Independent of the write re-verb (DQ) — this is the external read/discovery contract fab-gui + any client loads against.*

- [ ] DP.0 - Phase exit: `GET /media/<ref>` negotiates (`?format=` > `Accept` > largest), `OPTIONS /media/<ref>` returns the role-aware hypermedia manifest, the slicer button loads via `?model=/media/<ref>?format=scad`; tests + docs; shipped.
- [ ] DP.1 - `GET /media/<ref>` negotiation: `?format=<token>` (scad/stl/3mf/avif/mp4 → mime; 406 on no-match) > `Accept` (largest acceptable; `*/*` → largest; specific-unsatisfiable → 406) > largest. `Vary: Accept` + `Content-Location`; `min_role`-gated (denied ≡ 404); `Accept: application/json` → the item state.
- [ ] DP.2 - `OPTIONS /media/<ref>` → the manifest `{ref, self, kind, title, min_role, variants:[{type,bytes,width?,href,remove?}], controls:{…}}`. ROLE-AWARE (write controls + per-variant `remove` only for an Admin caller); safe-method-public + `min_role`-gated; wired on the existing `/{media_ref}` method-router.
- [ ] DP.3 - ONE shared, typed variant-SELECTOR (by-type / by-size over `ModelFormat` + `bytes`/`width`) used by the negotiation (DP.1), the manifest (DP.2), AND the embed (§8 — replacing its hand-rolled smallest-3mf / largest-mesh / srcset picks). No duplicate selection logic.
- [ ] DP.4 - Slicer button → `?model=/media/<ref>?format=scad` (ref in the PATH = the SAVE target, derivable by dropping the query; format EXPLICIT — no implied state). Replaces the `?model=/media/file/<url_key>` form.
- [ ] DP.5 - Tests (negotiation precedence + 406 + Vary; manifest shape + role-aware controls + gating; selector unit; slicer URL) + `media-design.md` [TARGET]→[SHIPPED] flips.

## Phase DQ - Media resource: the write surface (RESTful, zero PATCH)

*The WRITE half (`docs/media-design.md` §5). The canonical REST surface every writer (fab-gui + the admin UI) targets: two POSTs for the server-assigns-identity CREATES, PUT for every idempotent replace, DELETE for removal — NO PATCH. RE-VERBS the shipped DO endpoint (`PATCH /media/<ref>` → `PUT /media/<ref>/variants`) — safe, it's inert (no fab-gui pin).*

- [ ] DQ.0 - Phase exit: the full `/media[/<ref>/variants]` write surface is live — `POST /media` (create, 201+Location), `POST …/variants` (add), `PUT …/variants` (replace-all), `PUT /media/<ref>` (metadata), `DELETE` item + variant, admin-gated `GET /media` (list); DO's PATCH re-verbed; tests + docs; shipped.
- [ ] DQ.1 - `PUT /media/<ref>/variants` — re-verb DO's `patch_media_by_ref` (identical ingest / tx / complete-replace body) onto PUT of the variant collection; metadata untouched BY CONSTRUCTION. Retire `PATCH /media/<ref>`.
- [ ] DQ.2 - `POST /media` — create an item (multipart initial variants + optional title/min_role) → `201` + `Location: /media/<ref>` + the manifest. The RESTful home of `upload_media`.
- [ ] DQ.3 - `POST /media/<ref>/variants` — add ONE variant to an existing item (the `add_encode` semantics, by ref not id) → `201` + `Location`; content-dedup = idempotent no-op.
- [ ] DQ.4 - `PUT /media/<ref>` (json `{title, min_role}`) — replace item metadata (the `rename` + `visibility` merge); `DELETE /media/<ref>` (item, CASCADE) + `DELETE /media/<ref>/variants/<url_key>` (one variant).
- [ ] DQ.5 - `GET /media` — the admin-only item listing behind its OWN `require_admin` (the safe-method default would leak the whole library — §4a); JSON collection.
- [ ] DQ.6 - Tests (each verb + the 201/Location + the `GET /media` admin gate + the re-verb preserves DO's behavior) + docs flips.

## Phase DR - Migrate the admin media UI onto the canonical surface

*Fold the parallel `/admin/media/*` vocabulary onto the `/media[/<ref>/variants]` REST surface so the library UI and fab-gui share ONE contract (`docs/media-design.md` §11). Behind the DQ surface — the external contract ships first, the admin swap lands after, no fab-scad blocker.*

- [ ] DR.0 - Phase exit: the admin library drives the canonical `/media` surface (upload→`POST /media`, add→`POST …/variants`, rename+visibility→`PUT /media/<ref>`, delete→`DELETE`, per-variant→`DELETE …/variants/<key>`); the `/admin/media/*` mutation routes retire; tests + docs; shipped.
- [ ] DR.1 - Templates + `media-upload.js` + `editor-support.js` → the new verbs/URLs (upload progress + drop-group + inline editor insert unchanged in behavior).
- [ ] DR.2 - Retire the `/admin/media/{upload,encode,rename,visibility,delete,variant}` handlers (thin shims first if needed, then drop); keep `GET /admin/media` as the library PAGE (HTML), distinct from `GET /media` (JSON collection).
- [ ] DR.3 - Tests (the admin UI still round-trips through the new surface) + docs.

## Backlog (not yet phased)

### Tech debt

- **Admin forms swallow non-2xx responses silently (HTMX).** *(→ promoted to DM.8.)* Every admin `hx-post`/`hx-delete` form relies on `htmx_refresh()` firing on success; a defense-in-depth rejection (the 409 last-admin guard, the CZ 400 not-assignable guard, any 404) triggers `htmx:responseError` with no swap — the admin clicks, nothing visibly happens, and they assume it worked. Pre-existing pattern (the old Demote 409 had it too), surfaced in the CZ review. Fix is a small site-wide `htmx:responseError` listener that renders the response text as a toast/inline error — one snippet in base.html covers every admin form.
- **Routing model is "too clever" (the `special_page` fallout).** `content_pages` is a self-referential tree that simultaneously (a) serves nested rendered-Markdown content, (b) carries `special_page` rows whose `page_markdown` is a *redirect target URL*, not content, and (c) is dispatched by a top-level router that special-cases the redirect rows while *also* breaking out to dedicated application routers (`/login`, `/projects`, soon `/admin`). Three concerns — content node / routing redirect / dedicated app page — conflated in one table + one dispatch path. A cleaner design separates them (content pages stay a tree; "special"/app routes become plain axum routes, not DB rows). Touches `redirect_to_first_page`, `pages/mod.rs` dispatch, `ContentPageDao::find_by_path`, the `0007` seed migration, `projects.rs`.
- **Authorization is per-handler and inconsistent.** Two idioms in the tree: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` (`preview.rs`, `attachments.rs`) and `if let AuthenticationState::Authenticated(u) = … && u.role != Role::Admin { return FORBIDDEN }` (`pages/mod.rs::delete_page_path`). No route-group enforcement anywhere. Phase 7 introduces a `require_admin` layer for the new `/admin` nest; the follow-up is to audit every existing mutating route and either move it behind a layer or a uniform `AdminUser` extractor, and converge on one idiom. (CLAUDE.md explicitly warns: audit every route first.)
- **`SessionData::from_request_parts` has a load-bearing `.unwrap()`** (carrying a `//Unsure how to do this without an unwrap` comment) on the session-store read — a transient SQLite error there panics the request instead of degrading. Map it to `Ok(SessionData::default())` (treat a read failure as "no session") or surface it as a 500 via the rejection type.
- **Version single source of truth (Phase 12 review).** Prod tag deploys stamp `CFBundleVersion=0.0.0-dev` (the hook builds a `.git`-less archived tree and passes no `VERSION`), and the runtime/log/tray version is `CARGO_PKG_VERSION` (kept in lockstep by a manual `Cargo.toml` bump per tag — drift-prone; it slipped once at v0.0.43). Fix: `post-receive` derives `VERSION` from the tag and exports it to `build.sh`; `build.rs` forwards it as a `rustc-env`; `lib.rs` prefers it over `CARGO_PKG_VERSION`. One tag-derived version for both the bundle and the runtime.
- **Separate beta Cloudflare token (Phase 12 review).** Beta reuses prod's CF token (CF can't scope a token narrower than the `hotchkiss.io` zone, so a beta-specific token has identical access). A *distinct* same-scope token would let beta be revoked/rotated independently of prod if beta is compromised. Deferred by choice — independent revocation/audit only.
- **Optional: scrub non-admin users from the beta snapshot (Phase 12 review).** Beta carries prod's full `users` table (passkey records are public keys with no network-reachable leak path; chris-as-admin on beta is intentional via shared rp_id). Pure defense-in-depth: `DELETE FROM users WHERE app_role != 'Admin'` in `snapshot_prod_db_into_beta` keeps only the admin row. Not required.

### Ideas

- **Analytics expansion (d3 dashboard / status-noise / per-IP / referer-grouping / referer-breakdown / site-performance)** — all six folded into **Phase CQ** (SPEC.md "Analytics — signal vs noise"), designed 2026-06-30.
- **Analytics → defense: dumb IP blocklist.** Derive a blocklist from `request_log` (N 404s in M minutes → drop the IP for a while), enforced by an early middleware layer. Still its own phase (blocklist storage + decay, the enforcing layer, an admin view/override, false-positive handling). **Phase CQ builds the reuse seam** — `noisy_ips(window_cutoff, …)` takes a window cutoff not a `days` int, so the "N 404s in M minutes" rate variant is a caller change, not a new fn; reconcile distinct-404-fanout (CQ's axis) vs per-minute-rate (enforcement's axis) before wiring.
- **e2e: exercise the conditional-auth / autofill login path.** Phase 8.4's browser e2e drives the passkey *registration* ceremony cleanly, but the `webauthn-autofill` flow in `htmx-webauthn.js` (conditional `navigator.credentials.get()` on page load → `/login/get_auth_opts` → `/login/finish_authentication`) isn't tested — and per the original author that's where hidden footguns lurk. Add an e2e that, with a pre-registered virtual-authenticator credential, opens `/login` in a fresh context and verifies the autofill auth completes and lands logged-in. May surface bugs in the extension → could spin off its own phase.
- **Editor facelift** — slice (b) of the mini blog + mobile-posting arc. Slice (a) shipped as Phase 10. SPEC writeup waits on Phase 10 dogfooding to surface real pain (see 10.13). Constraints still apply: stay HTMX-first (Leptos islands only if HTMX hits a real wall on live-preview / drag-reorder / complex client state); Tailwind cleanup (Phase 9) is done; DaisyUI dropped; the "easier project loader" remains explicitly out of scope — bigger and deferred.
- **Staging / beta deployment.** A real deployed instance serving `beta.hotchkiss.io` (a DNS entry that already exists, used for Let's Encrypt testing) — complements Phase 8's local harness ("does it survive in the wild"). `Settings` already supports this with no code change: a beta `config.json` with `domain = beta.hotchkiss.io`, its own `database_path`, and its own Cloudflare token (separate token = independent revocation / rate-limit / audit — *not* blast-radius isolation, since CF tokens scope per-zone and `beta.` is in the `hotchkiss.io` zone; real isolation would need a delegated zone). The actual blocker is the hardcoded `:80`/`:443` (can't coexist with prod on the mini) → make ports configurable (which Phase 11 does); then it's "second machine, or port-mapped local instance". **Promoted to Phase 12 (drafted).**
- **Warn when main has commits past the latest prod tag.** With Phase 12's inverted code flow (`main` → beta, `vX.Y.Z` tag → prod), it'd be easy to forget to tag and let prod silently lag. A small pre-push hook or `git status`-style helper that prints "main is N commits ahead of v0.x.y" would close the gap. Could also surface in the analytics dashboard ("running version vs latest commit"). Small follow-up to Phase 12.
- **Beta-only registration → prod-usable passkey (rp_id design implication).** With Phase 12's `webauthn_rp_id = hotchkiss.io` on beta, *any* passkey registered against beta is implicitly authorized for prod too (and vice versa) — they share an rp_id. For chris-as-sole-admin that's the goal (existing prod passkey works on beta). If users ever register on beta, their credentials would also work on prod after the next snapshot. Mitigations to consider before opening registration on beta: (a) disable public registration on beta (admin-only gate via config), (b) split rp_ids and accept that prod passkeys won't work on beta (need iCloud sync or re-register), (c) snapshot strips non-admin credentials on the way in.
- **Publish `ios-inspect` as a crates.io crate.** The iOS-simulator inspection tool used by `tests/e2e_ios.rs` now lives in its own repo `github.com/chotchki/ios-inspect` (split out 2026-06-22 from `skylander-portal-controller/tools/ios-inspect`: the original sibling-repo *path* dep silently broke every non-dev-machine build incl. the mini's prod deploy, and a git-dep on the skylander repo dragged in its giant `rpcs3` submodule). It's now a clean **git** dev-dependency on the standalone repo, pinned via `Cargo.lock`. Publishing it to crates.io would make it a plain versioned registry dep (no git/branch pin) and is independently reusable — its own small project.

  - *Dogfood findings (Phase 10 phone testing, running list):*
    - Top nav (`templates/base.html` `<ul class="list-none flex flex-row">`) overflows the viewport on mobile — no `flex-wrap`, `px-8` per tab, ~5 tabs busts a ~390px iPhone viewport. Whole-site issue, not blog-specific. Likely fix: wrap + tighter mobile padding, possibly a hamburger at xs.
    - `post_page_path` / `post_top_level_page_path` reject non-URI-safe `page_name` (spaces, etc) with a 400 — but htmx swallows non-2xx responses, so submissions silently no-op. The blog "+ New post" form now slugifies on input as a local fix, but the top-nav admin "Create New Page" form and the editor's child-create form (`templates/pages/get_page.html`) still have the silent-fail. Whole-site fix: either slugify server-side in the handlers (any `page_name` → lowercase/hyphenated), or apply the same client-side slugify everywhere, or render an inline error message on 400.
    - Page minimum width exceeds an iPhone portrait viewport (~390px) — `templates/base.html` has the jumbotron as `flex flex-row` (image `size-40` = 160px + name/tagline text alongside) which never wraps, and the un-wrapped nav `<ul>` from the first finding contributes too. User has to rotate to landscape. Likely fix: jumbotron becomes `flex-col sm:flex-row` (or similar) so it stacks on narrow screens; nav fix from finding #1 helps here too.
    - On the phone, can't reach the editor — user reports "not logged in to the website to edit." Need to confirm symptom precisely (no editor chrome / 403 / redirect to login / save fails / something else) and whether (a) PWA cookie scope is separate from Safari, (b) session expired silently (1-day inactivity), or (c) the login passkey ceremony itself doesn't complete on iOS in some path.

- ~~**Add Biome for first-party JS/CSS lint (augment Prettier)** — added 2026-06-24.~~ **DONE 2026-07-08, as REPLACE not augment:** brew-installed Biome binary + `biome.json` scoped to `assets/scripts/` + `styles/` (tailwindDirectives parser on, style-war rules off); Prettier/npm/package.json removed entirely after the whole-tree-reformat + markdown-corruption incident. First `biome check` caught 4 real bugs (un-interpolated `${…}` in htmx-webauthn.js error logs) + dead code. Remaining gap: templates lost Tailwind class-sorting (Biome HTML is experimental — revisit).

- **Embed widgets: live-demo iframe + source-code iframe (deep-link line ranges, no copy-paste)** — added 2026-06-26. Two forms. (1) **Page iframe** embeds a live demo app inline (e.g. `recon-gen-spec.hotchkiss.io`) so a project page shows the REAL thing, not a screenshot — surfaced from the recon-gen draft's "I'd really like an Iframe to the demo app instead" note. (2) **Source-code iframe** takes a deep-link WITH a line range (the recon-gen page already links `…/schema.py#L2934-L2953`) and renders the actual source at those lines inline, so a snippet is never copy-pasted into a page. DRY — the repo is the single source, the pasted copy is what drifts (the recon-gen draft today pastes the SQL AND deep-links it, exactly the duplication this kills). Both reusable across every project page (extends the "deep-link + show the snippet" pattern). Open Qs: render target (GitHub's own embed vs raw-fetch + highlight.js into our own block vs an iframe to a `/source/<repo>/<path>?lines=` route on the site), pinning the ref so line ranges don't rot, caching and private-repo handling.
- **Optional: re-parallelize the browser e2e via a shared WebAuthn fixture. They currently serialize on E2E_LOCK (test isolation) because concurrent passkey ceremonies race. A pre-registered-credential fixture (one virtual authenticator + seeded admin reused across tests) could let independent assertions run in parallel again. Only worth it if the serial ~6s run becomes a bottleneck.** — added 2026-06-28.
- **Backlog: client-side source hash (File API) → pre-flight dedup + integrity** — added 2026-06-29.
- CR.4 - Trim redundant scans (measure-gated): fold count_since into audience_counts.all + combine any queries where one windowed scan yields multiple aggregates; only if the diagnostic still shows it matters after CR.1-CR.3 *(deferred from phase `CR` on 2026-07-01)*
- **Add Cross-Origin-Resource-Policy to the media byte route so the isolated editor can fetch models** — deferred from CW.4 on 2026-07-03.
- DJ.2 - PageId / MediaId (i64 newtypes) through the DAOs + call sites *(deferred from phase `DJ` on 2026-07-10)*
- **Route `deadlinks/internal.rs`'s `/media/` resolve through the canonical `media_ref::parse_cover_reference`** — added 2026-07-10 (DJ.4 follow-up). The dead-link internal resolver has its own `strip_prefix("/media/embed/"|"/media/"|"/media/file/")` → `find_by_ref`/`find_by_url_key` shape, independent of the one canonical media-token parse DJ.4 established. Consolidating it is a real DRY win (one parse path for every `/media/*` URL), but it's a link-CHECK path (not hot/security) and its shape differs (it handles `/media/embed/` + roots the whole path), so it was scoped OUT of DJ.4 to keep that refactor's blast radius to `media_ref.rs` + two spots. Low priority.
