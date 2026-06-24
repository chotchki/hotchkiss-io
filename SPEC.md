# Hotchkiss-io

Meta Note: This project delivers the hotchkiss-io website so fundamentally this project and the site itself are intertwined.

## Goals

**This is a personal portfolio site.** Its job is to be the curated front door to chotchki's work, rather than a personal playground that happens to present me. Everything below is reprioritized around that reader.

- **Audience:** a technical visitor evaluating chotchki's work. The landing page orients them fast and routes them to proof.
- **Thesis:** Much of chotchki's depth comes from work that isn't publicly visible, so it can't be shown directly. The GitHub + 3D **side projects are the tangible, clickable proof of range** that compensates: public deliverables anyone can actually verify. The site's job is to make the two reinforce each other — depth from the background, evidence from the projects. The site itself is just one of those side projects: a decent artifact, not the headline.
- **Why my own site?** Content's scattered across GitHub and a pile of unposted projects. I want to share it without someone else owning the experience.
- **Still true, still differentiators** (built — see PLAN_ARCHIVE): self-hosted on my own hardware; self-contained, minimal external deps — Let's Encrypt (certs) and Cloudflare (dynamic DNS + public-IPv4 discovery via `1.1.1.1/cdn-cgi/trace`).

## Portfolio — the three pillars

Each pillar is a PLAN phase (14–16), fronted by a landing page (Phase 13). The hard problem is called out per pillar because none of these is "build a page" — the hard part is curation and capture, not code. The pillars are not independent: the résumé carries the *weight* (the depth of the background) but lacks public evidence; the side projects carry the *evidence* but not the weight. The win condition is each covering the other's gap.

### Pillar 1 — Software / GitHub  → Phase 14
- A curated `/projects` front door to 3–5 *lead* projects. Each gets its own page: what it does, why it's interesting, the problem and my role, media (screenshot/demo), link out to GitHub. Linking out is fine — I just want the front door to be mine.
- **Role in the pitch:** these side projects are the *verifiable* half of the story — public, clickable deliverables that prove range the less-visible background work can't show. Curate for **breadth and clickability**, not just polish.
- **This site** is one entry here, *not* the lead — it surfaces engineering that's currently invisible (self-hosted Rust binary; self-managed DNS/ACME; passkeys over HTMX; push-to-deploy; macOS tray app).
- **Hard problem — curation, not code:** which 3–5 lead, in what order, and the one-sentence hook for each. Hand-curate the lead set; defer any GitHub-API auto-listing.

### Pillar 2 — 3D printing / OpenSCAD  → Phase 15
- A gallery of hand-picked prints/models: model viewer (the STL viewer already exists), photos, description, downloadable STL + OpenSCAD source, ideally an auto-generated lower-res STL.
- **Role in the pitch:** tangible range in a different medium — physical things I designed and made. Hardware/CAD breadth most software portfolios don't have.
- **Hard problem — ingestion:** countless prints, no easy bulk-load. **Don't let that block shipping 5 great ones by hand.** The bulk loader is deferred (backlog / earn-it — build it only if hand-loading the lead set proves too slow).

### Pillar 3 — Resume / background capture  → Phase 16
- A clean, current `/resume` page + a one-click downloadable PDF. Table stakes.
- **Hard problem — making less-visible work *credible*, not just recording it:** the work can't be clicked into and verified the way the side projects can. The capture (a writing problem, best as a drafting partnership) has to extract scope, scale, impact, and decisions — and find what can be **written up in a public-safe form**. The narrative leans on the side projects as the tangible proof of range. Host the result — do **not** build a resume CMS. Decide public vs gated, and what to lead with.

### Supporting content (lower priority)
- **Mini Blog** — v1 shipped (Phase 10); proof-of-life. No editor facelift now (slice (b) parked) unless cadence demands it.
- **Analytics** — v1 shipped 2026-05 (`/admin/analytics`, admin-only). See PLAN_ARCHIVE Phase 7.
- **Backups** — more content → more worth protecting. (Backlog.)
- **Family / approved-people-only features** — I run non-public services; gated content is a later want. (Backlog.)

## Current site's pain
- ~~deployment is fragile, unsure if I should finally move to docker~~ — **solved 2026-05**: `git push origin main` → post-receive hook on the mini builds, atomic-swaps the `.app`, restarts the LaunchAgent. No docker, no copying stuff around. (See PLAN.md Phase 0.)
- ~~What should be the landing page? that's always hard~~ — **answered 2026-06**: with the audience pinned to a technical visitor, the landing page is identity + a one-line value prop + three pillar doors (Software / 3D / Resume). See Phase 13.
- ~~No mini blog~~ — **solved 2026-05**: `/blog` shipped (Phase 10).
- Mobile posting is too hard, I am very open to enabling a PWA version to enable easier posting
  - easier == I can add an annoucement, attach a couple photos from a phone with a nice interface
- too experimental? I'm mixed on this because this site is also a source of experiments for me
  - I'm proud of passkeys with htmx
  - I like sqlite as a storage mechanism for content but I know it won't scale if I start really loading content

## Mini Blog (v1)

**Goal.** A `/blog` surface that lists posts as cards (cover + date + title + excerpt), so chotchki has somewhere to put short writing without committing to a posting cadence. Slice (a) of the "mini blog + mobile-posting editor facelift" arc — the editor facelift is slice (b), separate SPEC pass later.

**Why now.** The site's "self-hosted, own the experience" thesis is undermined every time there's something to say and it lands on someone else's platform because posting here is too painful. Slice (a) creates the smallest real surface to dogfood the facelift against — the expectation is that testing slice (a) on a phone will surface the editor pain that justifies slice (b).

### Model
- Posts are `content_pages` rows whose parent is a new `blog` special_page (mirrors `projects`; new migration `0010_DMLBlogSpecialPage.sql`).
- Live on save — no `published` column. Saved = published. "Drafts" are unsaved work in the editor.
- `page_creation_date` is the canonical post date; `page_modified_date` is editorial.
- `page_cover_attachment_id` is the card cover.

### URLs
- `/blog` — index, newest first.
- `/blog/<slug>` — single post; `page_name` is the slug (uniqueness via `UNIQUE(parent_page_id, page_name)`).
- `/pages/blog/<slug>` — unchanged, still works via the content_pages tree walk.
- `/blog/feed.xml` — Atom feed.

### Index UX
- Cards: cover image (with a sensible fallback when absent), date, title, excerpt.
- Excerpt = first paragraph of `page_markdown`, formatting stripped, truncated to ~200 chars.
- Empty state: "No posts yet."

### Feed
- Atom 1.0, posts ordered by `page_creation_date` desc, capped at the 50 most recent.
- `<link rel="alternate" type="application/atom+xml">` in the layout head on `/blog` and post pages.

### PWA (minimal)
- `manifest.webmanifest`: `name`, `short_name`, `start_url=/`, `display=standalone`, theme + background colors.
- Icon set under `assets/images/`, pre-rendered from `HotchkissLogo.svg` and committed: 192×192, 512×512, 180×180 (apple-touch-icon), 512×512 maskable.
- No service worker. No offline. "Add to Home Screen" works on iOS Safari and Chrome — that's the install story.
- `<link rel="manifest">` and `<link rel="apple-touch-icon">` in the layout head.

### Editor (single change in this phase)
- Add `capture="environment"` to the attachment upload `<input type="file">` so phones offer Camera-or-Library. One attribute. Every other editor change is slice (b).

### Out of scope (deliberate)
- Editor facelift / autosave / toolbar / drag-paste — slice (b).
- Comments, reactions, social sharing — site ethos is one-way publishing.
- Drafts / scheduled posts — revisit if cadence demands.
- Tags / categories beyond the existing (unused) `page_category` — punt until a real need shows up.
- Heavier PWA (service worker, offline compose, queued attachment upload, push, install prompts) — likely revisited as part of slice (b). The editor is the probable forcing function, not connectivity: a mobile compose flow that wants background save / queued uploads / native-feeling install is what pushes past a static manifest.

## Diagrams (Phase A)

Diagrams are first-class content: they carry relations faster + denser than prose. Authored INLINE in page markdown as a fenced ` ```d2 ` block — the source stays in the markdown (diffable, LLM-parsable, edited from the same editor).

### Renderer: D2 (`brew install d2`)
D2 over Graphviz DOT — chris compared both and D2's output is clearly nicer, which matters for a portfolio showcase. A pure-Rust DOT crate (`layout-rs`) was built + working first, but D2 won on looks; chris is fine owning the `d2` install on dev + mini + CI. d2 is shelled out to (`d2 - -`, stdin→stdout), resolved via `$D2_BIN` → `/opt/homebrew/bin` → `/usr/local/bin` → PATH (the mini's LaunchAgent PATH excludes homebrew). Not self-contained, but the app already needs the network to boot, and diagrams **degrade gracefully** to a visible error block if d2 is absent.

### Delivery: source-in-HTML + HTMX swap (more LLM-friendly than baking)
The served page carries the **d2 source**, not an opaque image — friendlier to LLMs / crawlers / no-JS readers, and pure progressive enhancement.
- At page-render the fence becomes a one-line placeholder: the source in a `<pre>` + `hx-get="/diagram/<hash>" hx-trigger="load" hx-swap="outerHTML"`. (One line + source newlines as `&#10;` so it survives the markdown AST round-trip.)
- On load HTMX GETs `/diagram/{hash}`; the server renders the SVG and returns it for the swap. No JS → the reader just sees the source.
- The endpoint renders **only sources the server itself emitted** (registered by hash at page-render), so it is NOT an open "compile arbitrary d2" surface (no DoS/abuse). Uses the HTMX already shipped site-wide.

### Hashing (a page may have many diagrams)
The id is a **content hash of the source bytes only** (SHA-256, 128-bit hex) — content-addressed. Two different diagrams can't collide; two identical ones dedupe harmlessly. Nothing page- or position-specific goes into the hash.

### Behavior
- The swapped SVG is embedded as a base64 `data:` URI `<img>` — isolated (no id/font collisions across diagrams).
- **Sizing:** the natural SVG size is injected so the `<img>` has intrinsic dimensions, then `max-width:100%` + a `max-height` cap keep it from dominating the page (responsive on a 390px phone). The full diagram is reachable via **click-to-zoom**: a zero-dependency pan/zoom lightbox (`assets/scripts/diagram-zoom.js`, pattern borrowed from recon-gen's `qs-lightbox`), bound by event delegation so it catches HTMX-swapped-in diagrams.
- Render output cached in-memory by hash (rebuilt after a restart; mirrors the on-the-fly AVIF precedent).
- A bad source or a stale/unknown hash returns a visible error block at HTTP 200 (so HTMX still swaps), never a 500 — surface the failure, don't swallow it.
