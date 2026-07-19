# EPUB reader + manga library (Phases DV–DW) [SHIPPED]

The next Library media type after audiobooks: **EPUB** — manga and reflowable novels — read in-browser, gated to Family, shelved as series → volume pages. DV builds the reader + a single volume end to end; DW builds the bulk path that makes a 271-volume series tractable.

Read this before touching `src/media/probe.rs` (the `Epub` kind), `web/features/media.rs` (the embed), `assets/scripts/epub-reader.js`, `assets/vendor/foliate/`, the child-index markdown widget in `web/markdown/`, `web/features/library.rs` (the `/library/manga` section), or `src/media/` bulk ingest.

## The two decisions that shape everything

1. **The reader is a client-side engine (foliate-js), NOT a server-side page extractor.** A manga EPUB is really just a zip of page images + a spine (order + reading direction), so the tempting lean move is to unzip at ingest and serve pages as plain images. Rejected on chris's call — because it's IMAGE-ONLY (no reflowable text novels) and it puts the format's complexity (spine, RTL, fixed-layout, 2-page spreads, mixed content) in our code. **foliate-js** (the engine behind the Foliate reader, MIT, ES modules, no build step) handles all of it and renders general EPUB. The cost, accepted: the whole `.epub` downloads to the client before/while reading — a manga volume is ~50–200 MB — and it's a heavier vendored JS dep than the site's others. That tradeoff IS the decision; the honest limits below live inside it.

2. **A series is a plain content-page subtree, and the volume "picker" is a reusable markdown widget — NOT a bespoke route or template.** `content_pages` is already a self-referential tree and `find_by_path` walks any depth, so a manga series is just `library → manga → <series> → <volume>` (flat: `<series>/1`, `/2`, `/3` — no range grouping; pagination + `?q=` search handle 271 volumes). The series page is a normal content page whose markdown carries a **child-index widget** (a ` ```children ` fence) that the renderer expands into a paginated, searchable grid of child cards. So "list this page's children as a picker" becomes a general capability — manga series are the first consumer, but any parent→children listing can use it later.

Everything else is plumbing hung off those two.

## The content model — series → volume, flat

```
library (special, min_role=Family)          — the DE-seeded gate
└── manga (authored section child)          — /library/manga code route + section index
    └── <series>  (authored content page)   — carries the ```children``` picker widget
        ├── 1     (volume, the foliate reader on volume 1's .epub)
        ├── 2
        └── …/271
```

- **`/library/manga`** is a code route in `library.rs` (mirrors `/library/audiobooks`). It STAYS a code route specifically for the **sign-in gate**: route names ship in the public source, so an insufficient viewer gets `library.rs::gate`'s state-aware "sign in" copy, not a cat-404 bookmark that reads as a support call. Its index lists SERIES (via the child-index widget, or a small template).
- **A series page** (`/pages/library/manga/<series>`) is a plain content page served by `get_page_path` — no bespoke route. It renders its volumes through the child-index widget. Because serving is depth-agnostic, nothing here cares that manga is 4 levels deep.
- **A volume page** (`/pages/library/manga/<series>/<n>`) is a plain content page whose markdown is one `![](/media/<ref>)` epub embed → the foliate reader. It has its own URL (bookmarkable "continue reading") and its own resume position.
- The `manga` child and each series are **authored, Family-gated** (inherit-on-create stamps `min_role` from the parent). Volumes inherit too. Gating is enforced exactly as everywhere else (`is_visible_to`, the strictest-wins byte gate).

## The child-index widget (the picker)

A general markdown widget: **render the current page's children as a picker grid.** The wrinkle is that `transform()` is a PURE, content-hash-cached function of the markdown string — it has no page identity and no DB. So the widget is a **two-stage** render, the same shape the media embed + diagram already use (a stable placeholder the handler resolves):

1. **Transform time (pure, cached):** the ` ```children ` fence (optional sort/page params in the fence body) becomes a STABLE sentinel element — `<div class="child-index" …></div>`. Same input → same sentinel → the render cache stays valid.
2. **Handler time (per-request):** `get_page` knows the page + holds the pool, so after `transform()` it fills the sentinel with the children grid via `listing.rs::paginate` (cards: title / cover / order, `?q=` + prev/next pager, each linking to the child page). The children can change between requests; the cached HTML can't go stale because the sentinel is static and the fill is live.

An empty/childless page renders an empty state, never a 500. This keeps `transform()`'s purity + the diagram `REGISTRY` coherence the render cache depends on (see `media-design.md` render-caching note) intact.

**As-built — it became THE listing mechanism (DV.6/DV.7).** chris pulled the whole audiobooks section onto this widget, so it's not manga-only: the fence takes `order=newest|manual` (encoded into a stable per-order sentinel), the card is rich (cover + title + excerpt + Scheduled/visibility badges), and it renders an admin "+ new child" form. The `show_audiobooks` handler + `audiobooks.html` are RETIRED — ONE generic `show_library_section` (`/library/{section}`) now serves audiobooks, manga, and any future section: gate → render the section's children through the same widget. The renderer (`child_index::render_children_grid`) splits **two bases** because a section route lists at `/library/<section>` while its children live at `/pages/library/<section>`: `list_base` builds the pager links (the URL the viewer is ON); `child_base` builds the card links + the new-child form action (the content-tree path). On a plain content page (a manga series) the two are equal, so the ````children` fence just passes one base twice. The audiobook/manga *players* on the DETAIL pages are untouched — this unified only the listing/selection.

**Gotcha found in the boot e2e:** `dominant_kind` (which decides a grouped item's kind) had no `Epub` arm, so a lone `.epub` fell through to `File` and rendered a download link instead of the reader. Fixed — `Epub` sits above `Image` in the precedence (a book grouped with its extracted cover is an Epub item, the image is its thumbnail — the same rule as audio/video). Pinned by `uploaded_epub_renders_the_foliate_reader_embed` + the browser boot e2e.

**Vendoring specifics:** foliate-js is pinned to a **commit SHA** (it has no npm release tags), copied under `assets/vendor/foliate/` MINUS `vendor/pdfjs/` (13 MB) + `pdf.js` (a dynamic import only a PDF file fires — we never open one) + the demo `reader.js`/`ui/`. `view.js`'s only static deps are 4 small modules; everything else (epub parser, zip loader, paginator) is a dynamic `import()`, so the excludes never break the EPUB path. Regen: re-clone at a new commit, re-copy the same whitelist, bump `FOLIATE-VERSION.txt`.

## The `Epub` + `Cbz` kinds

- **Detection** (`probe.rs`): `.epub` typed by EXTENSION, before ffprobe — a sibling to the `.stl` / `.3mf` branches (ffprobe can't type a zip container). Mime `application/epub+zip`, kind `MediaKind::Epub`. `.cbz` (comic-book zip, Phase DW.8) is the sibling branch: mime `application/vnd.comicbook+zip`, kind `MediaKind::Cbz`. The enum gains `"epub"`/`"cbz"` in `as_str` / `from_str`; **no migration** — `media.kind` is a string column, so a new value needs no schema change.
- **Serving:** the existing `/media/file/<url_key>` byte route serves both verbatim — range-capable, `min_role`-gated (strictest-wins), `nosniff`. `application/epub+zip` / `application/vnd.comicbook+zip` are inert (zips), so the active-content neutralizer (which force-DOWNLOADS svg/html/js) leaves them INLINE for foliate to fetch — pinned by a test, because "does the reader's fetch get the bytes or a download disposition" is exactly the kind of thing that silently breaks.
- **The reader is shared** (`render_embed_html` `Epub | Cbz` arm): the SAME `<foliate-view>` shell + `epub-reader.js` render both. The embed carries `data-kind="epub|cbz"`, and the reader wraps the fetched blob in a correctly-named/typed `File` so foliate's `makeBook` dispatches to the EPUB reader vs the vendored `comic-book.js` — foliate reads CBZ natively, so the only server work is the kind, the mime, and the cover.
- **CBZ cover (DW.9):** a CBZ has no OPF, so the cover is the FIRST image in the zip by sorted entry name (zero-padded page order → page 1). Extracted server-side with the `zip` crate (already in the tree via `epub`), stored as an image variant exactly like the EPUB OPF cover, so `cover_url_for` / the card thumbnail / `embedded_media_cover` all light up with no special-casing.

## The reader

- **Vendored** foliate-js (pinned version) as ES modules under `assets/vendor/foliate/`, served by `rust-embed` like threejs/katex — no build step. Its unzip is `fflate`, already vendored for the 3MF loader.
- **The embed** (`render_embed_html` Epub branch): a `<foliate-view>` mount + a boot splash that lifts on the reader's ready event (same pattern as the fab-gui editor), and a **no-JS fallback** that's a plain download-the-`.epub` link. Emitted with the `<span>`/flex wrappers the STL embed uses (a bare `![]()` sits inside a `<p>`, where a block `<div>` is invalid).
- **`epub-reader.js`:** fetch the gated byte URL as a Blob → hand it to foliate → mount the view. RTL is auto-detected from the OPF `page-progression-direction` (manga is RTL) — we don't guess. Controls: keyboard (←/→), tap-zones, swipe, single/spread toggle, fullscreen. Vanilla, `defer`'d, degrades to the download link without JS.
- **NO COOP/COEP.** Unlike the fab-gui WASM editor (which needs cross-origin isolation for SharedArrayBuffer), foliate renders EPUB content in same-origin blob-URL iframes and needs no isolation. There's no site-wide CSP today, so foliate's blob iframes aren't blocked — a test pins that the reader mounts + paints so a future CSP addition can't silently break it.

## Resume

Per-device `localStorage`, keyed `epub-loc:<ref>`, storing foliate's location (a CFI), applied when the reader is ready — the **same shape as `audio-player.js`'s `audio-pos:<ref>`**. Server-side cross-device sync is deferred exactly like the audiobook version (its Phase DF): localStorage is the honest v1, and a book resumes where you left it on the same device.

## Bulk ingest (Phase DW) [SHIPPED]

DV makes ONE volume work with manual authoring. A real series is 271 volumes at ~100 MB each ≈ 27 GB — which can't go through a browser upload. DW is `web/features/admin/manga_ingest.rs`: a shared stream-commit core behind two front doors, both funneling into ONE `ingest_volumes` so the item/page policy can't drift.

**Filename parse (DW.1, `parse_volume`).** Pure fn → `ParsedVolume { number, title }`. A token walk classifies a leading letter-run as a `v`/`vol`/`volume` (→ "Volume N") or `c`/`ch`/`chapter` (→ "Chapter N") marker — chris's Jujutsu Kaisen reads "Chapter 1", One Piece "Volume 12", so the LABEL is preserved, not flattened. A marker beats a stray number (a year): `Series 2020 v12` → 12, `Vol 12 (2020)` → 12. No marker + a bare trailing number → Volume N; no number at all → `None` (the caller orders by sorted position) + the cleaned stem as title. Unit-tested against the real shapes.

**The tree, auto-bootstrapped (DW.5, `resolve_or_create_series`).** `library → manga → <series> → <volume>`. The ingest find-or-creates the `manga` section (a child of `library`) and the `<series>` (a child of `manga`) if absent — each via `PageWrite::create_page`, so each **inherits its parent's gate** (`library` seeds `Family` → everything below is Family for free) and gets stamped with a ` ```children ` fence so a direct visit lists its children. The very first drop self-bootstraps the whole section; zero manual setup.

**The core (DW.2, `ingest_volumes` → `ingest_one`).** Per staged `.epub`: (1) **content-hash dedup** — skip if a child of the series already embeds a media item with this exact sha (idempotent re-run, `series_has_volume_with_sha`, sha-indexed so the child scan stays tiny); (2) reserve the volume page via `create_page` (inherits the series gate) — a **slug collision is a soft skip** (a same-numbered but DIFFERENT file, `SkippedExisting`), never a hard error; (3) ingest the EPUB media item through the shared `admin::media::ingest_stored_file` (probe → variant → OPF-cover extraction, `dominant_kind` → Epub); (4) fill the page with `![](/media/<ref>)` + `page_order` = the parsed number. A media-ingest failure **rolls back the empty page** so a corrupt file leaves no orphan. Best-effort per file — one bad EPUB is a `Failed` report entry, never an aborted batch. **Never all-in-memory:** each file streams to the content store (`StagedBlob`, O(chunk)); the filesystem path stages + ingests one file at a time, so pages appear incrementally and a 27 GB series never sits in RAM.

**Two front doors + the console (DW.3/DW.4, `GET /admin/library/manga`).** (1) **Filesystem** (`POST …/manga/ingest`, series + a server-side folder path) — the 271-volume path, no upload; the folder is CANONICALIZED (resolves `..`/symlinks) + `is_dir`-checked, then the ingest is **spawned** (detached like the coordinator backfills — a bad pass can't take the app down) and logs its tally to `/admin/logs`, so the log tail + the series page filling in ARE the progress view. (2) **Browser** (`POST …/manga/upload`, multipart) — SYNCHRONOUS for SMALL batches (`.epub`/`.cbz` parts only, others drained), returning the per-file report inline; an honest note steers a full series to the filesystem path. Both front doors accept EPUB and CBZ (`is_book_filename`), the `list_books` scan and the parse (`strip_book_ext`) handle both.

## Honest limits + deferred

- **Whole-file download.** foliate reads the entire zip; a 150 MB volume is a 150 MB fetch before the first page is interactive. The byte route supports range, and foliate/zip.js CAN range-fetch the central directory + entries, but the simple Blob path pulls the whole file — a loading state covers it, and **range-streaming the zip is the deferred optimization** if first-load latency hurts in dogfooding.
- **foliate integration is the phase's real risk.** Vendoring an ES-module engine + feeding it a gated Blob + mounting `<foliate-view>` in this stack is the uncertain part (module paths, the zip loader, the iframe sandbox). DV.3 vendors + boots it as a spike; if it doesn't fit, THIS doc gets revised before the rest of DV builds on it.
- **Headless e2e is scoped.** The browser e2e asserts the reader mounts + the first page paints + no fatal console error — not pixel-perfect rendering (headless foliate paint is timing-sensitive, like the fab-gui editor's splash-lift assertion).
- **Server resume sync** — deferred (shared with the audiobook DF item).
- **CBZ [SHIPPED, DW.8]** — foliate reads comic-book zips natively, so `.cbz` was the small add the doc predicted: a `MediaKind::Cbz`, the `application/vnd.comicbook+zip` mime, `data-kind` on the shared reader, and a first-image cover. Most of chris's manga is CBZ, so this is what makes the library usable, not a nice-to-have.
- **Reflowable-novel UX** (font size, themes, TOC) — foliate provides the hooks; DV wires the manga-first controls (page-turn, spread, RTL), and novel-specific polish is a later refinement.
- **Bulk-ingest folder validation is coarse (DW.3)** — canonicalize + `is_dir`, and it's admin-only (the `/admin` gate). Restricting the source to a configured drop-dir / the media roots is a deferred tightening; the operator is trusted here.
- **No live ingest progress bar (DW.3)** — the spawned filesystem ingest logs its per-file failures + final tally to `/admin/logs`, and the series page fills in as it goes. A shared status handle (a "running… / last run" badge like the dead-link scan's `DeadLinkScanState`) is the deferred nicety.
- **Range-grouping dropped (DW).** An earlier plan grouped a huge series into `<lo>-<hi>` range parent pages every N volumes. Cut — the flat `series → volume` tree + the drag-reorder + `?q=` search + the pager handle 271 volumes fine, and the depth-agnostic serving means it's a pure re-parent if it's ever wanted.
