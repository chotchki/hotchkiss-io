# Hotchkiss-io

Meta Note: This project delivers the hotchkiss-io website so fundamentally this project and the site itself are intertwined.

## Goals

**This is a personal portfolio site.** Its job is to be the curated front door to chotchki's work — judged in seconds, then minutes — rather than a personal playground that happens to present me. Everything below is reprioritized around that reader.

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
- **Mini Blog** — v1 shipped (Phase 10); proof-of-life. **Editor facelift + admin UX shipped (Phase F):** title↔slug separation (`page_title`), create-by-title with auto-slug, reader-view-default with an `?edit` toggle, restyled editor, a dedicated admin bar, and a login-state indicator.
- **Analytics** — v1 shipped 2026-05 (`/admin/analytics`, admin-only). See PLAN_ARCHIVE Phase 7. **v2 (Phase C):** views-over-time — date-range chips (7/30/90, default 30), a server-rendered inline-SVG views/day chart with a total↔unique-visitors toggle, top pages with a Content↔All toggle (Content hides 404 scanner probes; All surfaces them — static assets always excluded), and top external referrers (directional only — referrers are spoofable/often-stripped). On-the-fly aggregation over `request_log` (90-day window, no rollup). A richer interactive (d3) dashboard is backlogged — it's an internal tool, basic is fine.
- **Backups** — more content → more worth protecting. v1 shipped: daily on-disk DB snapshots, 7-day rolling window (see "Database backups" below).
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

## Content images & links (Phase BU)

Dogfooded out of the first image blog post.

- **Images render capped + click-to-zoom.** Every non-`.stl` markdown image (`![]()`) becomes a height-capped (480px) `<img class="content-image … cursor-zoom-in" data-zoomable>` in `transformer.rs` — the same treatment as a diagram, so a tall screenshot doesn't dominate the page. It reuses the diagram lightbox: `diagram-zoom.js` now binds **any** `img[data-zoomable]` (diagrams + content images), and the full-resolution `src` loads in-flow (CSS-capped), so the zoom shows it at full size.
- **Same-site links go relative on save.** `web/markdown/links.rs::rewrite_site_links` runs in `put_page_path`: absolute links + image `src`s pointing at the site's own **registrable** host (`webauthn_rp_id` — `hotchkiss.io` on both prod and beta, via `AppState.site_host`; the rp-id and not the served `domain`, so beta's `beta.hotchkiss.io` still relativizes the canonical `hotchkiss.io` links in its snapshot — plus `www.`, any scheme/port) are rewritten to root-relative, preserving query + fragment. **Why on save, not render:** the stored markdown becomes the canonical portable form — it works on prod, beta, and any future host, and the Atom feed inherits it. It edits only the matched URL substrings (longest-first, so a bare-domain match can't corrupt a longer path URL), leaving the author's formatting otherwise byte-for-byte intact — not a full AST reflow.

## Typeset math + code highlighting (Phase BV)

The lecture-style content (the recon-gen deep-dive) wants real math + readable code.

- **Math** is authored as `$$…$$` — single `$` stays literal so prose prices ("$200") don't parse as math. The transformer enables markdown-rs's math constructs and emits each math node as a source-carrying `.math` element (the TeX stays in the served HTML — no-JS / crawler / LLM reads it); **KaTeX** (vendored, client-side) typesets them on load + after HTMX swaps. Same source-in-HTML philosophy as the d2 diagrams.
- **Code** keeps its fenced `language-*` class; **highlight.js** (vendored, client-side) highlights it, excluding the d2 diagram source. Authoring convention: **deep-link the real code to GitHub at exact lines AND show the important snippet inline** in a highlighted block — the permalink keeps it honest (the canonical source), the inline snippet saves a click for the bit under discussion. The snippet is copied from the real code, never invented; large code is not reproduced wholesale.
- **Tables** are GFM (`| … |`). Since `mdast_util_to_markdown` can't re-serialize a Table node, each table is rendered from its original markdown slice and emitted as HTML (Phase BW). That same phase fixed a walk-depth bug so math / images / diagrams nested in a list or heading convert too — previously only top-level ones did.

## Database backups

All site content lives in one SQLite DB, so a daily local snapshot is the cheapest meaningful protection against accidental deletion / corruption.

- **Mechanism:** `VACUUM INTO` run in-process through the existing sqlx pool — a consistent point-in-time copy that doesn't block writers and needs no external `sqlite3` binary. (The mini's beta-snapshot path still shells out to `sqlite3 .backup`; this is the in-app equivalent.)
- **Schedule:** a long-lived task in `EndpointsProviderService` (alongside session GC + request_log prune) fires daily, first tick at startup.
- **Files:** `database-YYYY-MM-DD.sqlite` (UTC date) under `Settings::backup_path` (default `~/Library/Application Support/io.hotchkiss.web/backups`, created if missing). VACUUM INTO won't overwrite, so a same-day re-run refreshes the file.
- **Retention:** rolling **7 days** — after each snapshot, dated backups beyond the newest 7 are deleted.
- **Off-site:** the whole server is backed up by **Backblaze**, so these on-disk snapshots are the local recovery tier; no upload logic lives in the app.
- **Failure isolation:** the backup loop matches + logs every fallible step and never returns, so a backup failure logs an error and is retried next tick — it can't crash the coordinator (whose `try_join!` would otherwise take the whole app down).
- **Testable units:** `coordinator/backup.rs` exposes `run_backup(pool, dir)` and `prune_old_backups(dir, keep)` so the snapshot + rotation are unit-tested without the full coordinator.

## Analytics — signal vs noise (Phase CQ)

Traffic is up and the dashboard can't answer the question that actually matters anymore: as more hits arrive, what's a real reader vs a scanner, where's it coming from, and what's slow? Today's `/admin/analytics` aggregates by day / path / UA / referer but never separates signal from noise. Phase CQ answers all three — audience, sources, performance — with everything behind the `require_admin` gate, off the public LCP path, and NO speculative infrastructure (no rollup table, no write-path rework, no stored classification column — every one of those is trigger-gated and named in the deferral list below). Three new cuts on the same `request_log` table plus ONE honest capture addition.

### Two orthogonal axes — status is factual, agent is inferred

The core move, and the thing that stops the ugly misclassification: **status** (2xx / 3xx / 403 / 404 / 4xx / 5xx) is ground truth, zero heuristic, zero maintenance. **Agent** (human vs bot) is inferred from a spoofable User-Agent ruleset and is LABELED as such. A human clicking a dead link is a 404 AND a real reader — so status NEVER feeds the agent classifier. The behavioral catch for UA-spoofing scanners is a separate per-IP 404-fanout leaderboard, not the headline audience filter. Bot classification is a zero-storage SQL VIEW (`request_log_view.ua_class` via `CAST(CASE … AS TEXT)`), computed at QUERY time — the ruleset stays reversible/tunable against all 90 days of history instead of frozen into a column at capture. **Headline numbers default to All (factual)** with an always-visible All / Humans / Bots 3-chip (they sum to All by construction — seeing all three at once IS the honesty mechanism) plus a toggle; a spoofable heuristic never silently governs the primary KPI.

### Sources — kill the IP-literal referer pollution

The shipped `count_by_referer` groups by the FULL referer URL and self-filters with `NOT LIKE '%hotchkiss.io%'` — which also wrongly swallows a spoofed `hotchkiss.io.evil.com`. Replace it with a pure `normalize_referer` on the `url` crate (already a dep): `url::Host::{Ipv4,Ipv6}` IS the free spec-correct IP-literal test (dotted-quad AND bracketed v6, incl. WHATWG-normalized forms) — those referers are the pollution chris flagged. Group by registrable-ish host (strip `www.`/`m.`/`amp.`, NO `psl` dep), bucket by category (search / social / aggregator / referral), and COUNT the junk rather than silently drop it ("N polluting referers hidden"). Derived at query time — no migration, works on the existing 90-day window day one. Honest caveat: referer is spoofable / often stripped — directional, not authoritative.

### Performance — server-handler latency, NOT client page-load (decided scope)

`request_log` captures no timing today; add ONE nullable `duration_ms` column, stamped in the fire-and-forget middleware. Be LOUD about what it is: **server-handler processing time** (the inner stack + handler, measured at the outermost log layer), NOT client LCP. It catches a slow d2 / weasyprint / ffprobe subprocess, an asleep-external-drive stat, the feed transforming every post, the session+role-refresh DB floor. It does NOT catch TLS / network / download, and it under-counts streaming bodies (`ServeFile` returns before the last byte). Real field Core Web Vitals are already free from Search Console / CrUX with zero code, and a client RUM beacon would fight the no-public-JS ethos Phase CN fought FOR — so that's a separate future phase, explicitly out of scope here, not bolted on. SQLite has no percentile function, so percentiles compute Rust-side (nearest-rank over a single windowed sample). LEAD with two tables — slowest routes (p50/p95/max) and slowest individual raw requests — those are the bottleneck-finders; the latency line chart is nice-to-have. p99 is computed but not displayed (≈ max at personal-site sample counts). Routes bucket through a `normalize_route` mirror of the axum router — a hand-maintained mirror with real drift risk, so it's pinned by unit tests that fail loudly when a new id-bearing route needs a rule. The latency exclusion set KEEPS `/diagram` + `/media` (unlike top-pages, which folds them out) — those subprocess / external-drive routes are the highest-value latency targets.

### Per-IP drill-down — the scan fingerprint

The dashboard never aggregates by IP, yet that's the one fingerprint separating a scanner from a human: a single client walking a wordlist of dead paths. `noisy_ips` returns per-IP total / distinct-paths / distinct-404-paths / errors, sorted by VOLUME (so a high-volume 200-only scraper or LLM crawler surfaces too, not just wordlist-walkers), with a `distinct_404 >= 5` scanner BADGE as a secondary sort. `WHERE ip IS NOT NULL` is non-negotiable — a NULL in the set is the classic SQL NULL-poison that silently zeroes the whole leaderboard. It groups on the EXISTING `ip` column: the server is verified IPv4-only (binds `Ipv4Addr::UNSPECIFIED`, publishes A records only, no v6 listener), so any `/64`-grouping column would equal `ip` for 100% of rows — the whole v6 apparatus is cut. `noisy_ips` takes a window CUTOFF (not a `days` int) precisely so the deferred blocklist phase's "N 404s in M MINUTES" rate variant is a caller change, not a new function — this query is that phase's reuse seam. A `/admin/analytics/ip/{ip}` drill-down (gated for free under the `/admin` nest) shows one IP's path+status / UA / recent-request detail.

### Dashboard — vendored d3 line charts (decided)

The dashboard is deliberately server-rendered inline SVG with no chart lib — a PUBLIC-page ethos that's defensible to break HERE (admin-only, gated, non-indexed, off LCP). **Decided: port recon-gen's d3** (`renderLineChart`) — vendor d3@7 UMD (~88 KB gzip, immutable-cached, admin-only) like htmx/katex already are, no bundler, no build step. The headline line chart overlays Total + Unique (the GAP between them is the signal-vs-noise story), and every future bot / status / latency chart rides the one renderer. Two things are fixed regardless of the renderer:
- **XSS boundary.** Attacker-controlled path / UA / referer strings STAY in auto-escaped askama tables — NEVER a JSON island, where `serde_json` won't escape `</script>`. The island carries ONLY numeric date+count data, `\uXXXX`-escaped unconditionally, from a TYPED serde struct (not an ad-hoc `Value`).
- **Control-model fix.** Today `since` drives the chart AND every table AND both stat numbers; a per-chart swap would leave tables stale while the pushed URL lies. Swap the WHOLE `#analytics-content` wrapper (`hx-target`/`hx-select` + `hx-push-url`) so chart + tables + stats + chips refresh together and the URL matches what's shown.
- **Self-feed guard.** Exclude `/admin/analytics` from the request-log skip-prefix (mirrors the existing `/admin/logs` exclusion) — the dashboard's own views currently pollute the very table it visualizes.

### Shared foundation + sequencing

Build the foundation FIRST: migration `0019` (`duration_ms`) + `0020` (`request_log_view`), both metadata-only and transactional-safe. Do them together so the OUT_DIR `schema.db` rebuild (`cargo clean -p hotchkiss-io`, the documented sqlx gotcha) happens ONCE. `duration_ms` on `NewRequestLog` turns the INSERT 6→7 columns and touches the `entry()`/`seed()` test helpers; view passthrough columns infer nullable through sqlx, so budget `as "col!: T"` overrides. Then the read-side query surface — status/noise, per-IP, referer, latency — is independent DAO work that proceeds in parallel. The dashboard consumer is the only piece that touches the d3 render; the data/query tasks land regardless.

### Decided / deferred

- **Decided OUT — geo/ASN enrichment.** MaxMind GeoLite2 is a licensed ~60 MB asset that can't live in the now-public GitHub mirror, and an online IP-geo API ships visitor IPs offsite (contradicts the beta-scrub-for-privacy stance). If it ever earns it: ASN-only (datacenter-vs-residential is the real bot signal) at read time, no schema hook — so skipping now costs nothing later.
- **Deferred — IP-blocklist ENFORCEMENT** (its own phase, per the standing steer). This phase builds the seam (`noisy_ips` window-cutoff), ships NO early-middleware drop layer. Reconcile distinct-404-fanout (this dashboard's axis) vs per-minute-rate (enforcement's axis) before wiring — they're not drop-in for each other.
- **Deferred — rollup/summary table.** Until a section's own dogfooded `duration_ms` crosses ~150–300ms on prod OR 90-day rows cross ~500k–1M (status/audience) / ~2M (per-IP). CHEAP pre-rollup step first: a SQL latency histogram + a covering index on the specific slow GROUP BY — don't jump straight to a rollup.
- **Deferred — batched mpsc writer.** Until sustained >~20 req/s OR a `SQLITE_BUSY`/failed-to-record warn OR user-facing GET p99 climbing (pool starvation, the real risk — WAL readers are lock-free). Fully specified; not built now because `try_send`-drop makes logging LOSSY, a behavior change to opt into.
- **Deferred — stored+indexed `referer_host`.** Until a referrer-spam campaign blows up distinct-referer cardinality (SUDDEN, not gradual). The pure `normalize_referer` fn is written to drop straight onto the write path when it lands.
- **Panic-500s ARE now logged (CQ.1.1).** The prior blind spot — a handler PANIC never reached `request_log` because the log layer sat INNER to `CatchPanicLayer`, so the panic unwound past its post-`next.run` insert — is closed by a one-line reorder: `log_requests` now layers OUTER to `CatchPanicLayer`, so the catch's synthesized 500 flows back out through the log layer and is recorded like any other response. (Folded in mid-phase at chris's call — it was a simple reorder, not the separate item it was first backlogged as.)
- **Remaining known limit, stated not glossed:** beta scrubs `request_log` wholesale, so every new column/view ships EMPTY on beta and all of CQ dark-launches there — test coverage carries the confidence the beta dogfood normally would.
