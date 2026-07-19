# EPUB reader + manga library (Phases DV–DW)

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

## The `Epub` kind

- **Detection** (`probe.rs`): `.epub` typed by EXTENSION, before ffprobe — a sibling to the `.stl` / `.3mf` branches (ffprobe can't type a zip container). Mime `application/epub+zip`, kind `MediaKind::Epub`. The enum gains `"epub"` in `as_str` / `from_str`; **no migration** — `media.kind` is a string column, so a new value needs no schema change.
- **Serving:** the existing `/media/file/<url_key>` byte route serves it verbatim — range-capable, `min_role`-gated (strictest-wins), `nosniff`. `application/epub+zip` is inert (a zip), so the active-content neutralizer (which force-DOWNLOADS svg/html/js) leaves it INLINE for foliate to fetch — pinned by a test, because "does the reader's fetch get the bytes or a download disposition" is exactly the kind of thing that silently breaks.

## The reader

- **Vendored** foliate-js (pinned version) as ES modules under `assets/vendor/foliate/`, served by `rust-embed` like threejs/katex — no build step. Its unzip is `fflate`, already vendored for the 3MF loader.
- **The embed** (`render_embed_html` Epub branch): a `<foliate-view>` mount + a boot splash that lifts on the reader's ready event (same pattern as the fab-gui editor), and a **no-JS fallback** that's a plain download-the-`.epub` link. Emitted with the `<span>`/flex wrappers the STL embed uses (a bare `![]()` sits inside a `<p>`, where a block `<div>` is invalid).
- **`epub-reader.js`:** fetch the gated byte URL as a Blob → hand it to foliate → mount the view. RTL is auto-detected from the OPF `page-progression-direction` (manga is RTL) — we don't guess. Controls: keyboard (←/→), tap-zones, swipe, single/spread toggle, fullscreen. Vanilla, `defer`'d, degrades to the download link without JS.
- **NO COOP/COEP.** Unlike the fab-gui WASM editor (which needs cross-origin isolation for SharedArrayBuffer), foliate renders EPUB content in same-origin blob-URL iframes and needs no isolation. There's no site-wide CSP today, so foliate's blob iframes aren't blocked — a test pins that the reader mounts + paints so a future CSP addition can't silently break it.

## Resume

Per-device `localStorage`, keyed `epub-loc:<ref>`, storing foliate's location (a CFI), applied when the reader is ready — the **same shape as `audio-player.js`'s `audio-pos:<ref>`**. Server-side cross-device sync is deferred exactly like the audiobook version (its Phase DF): localStorage is the honest v1, and a book resumes where you left it on the same device.

## Bulk ingest (→ Phase DW)

DV makes ONE volume work with manual authoring. A real series is 271 volumes at ~100 MB each ≈ 27 GB — which can't go through a browser upload. DW builds a shared stream-commit ingest core behind two front doors: (1) **filesystem-drop + admin trigger** (the primary — drop the `.epub`s on a mini drive, ingest server-side, no HTTP upload), and (2) a **browser multi-file drop** for small add-later batches. Volume number + `page_order` are parsed from the filename (`Series v012.epub` → Vol. 12); a content-hash already present under the series is skipped (idempotent re-run). Details in DW.

## Honest limits + deferred

- **Whole-file download.** foliate reads the entire zip; a 150 MB volume is a 150 MB fetch before the first page is interactive. The byte route supports range, and foliate/zip.js CAN range-fetch the central directory + entries, but the simple Blob path pulls the whole file — a loading state covers it, and **range-streaming the zip is the deferred optimization** if first-load latency hurts in dogfooding.
- **foliate integration is the phase's real risk.** Vendoring an ES-module engine + feeding it a gated Blob + mounting `<foliate-view>` in this stack is the uncertain part (module paths, the zip loader, the iframe sandbox). DV.3 vendors + boots it as a spike; if it doesn't fit, THIS doc gets revised before the rest of DV builds on it.
- **Headless e2e is scoped.** The browser e2e asserts the reader mounts + the first page paints + no fatal console error — not pixel-perfect rendering (headless foliate paint is timing-sensitive, like the fab-gui editor's splash-lift assertion).
- **Server resume sync** — deferred (shared with the audiobook DF item).
- **CBZ is nearly free** — foliate reads comic-book zips too; a `.cbz` → `Epub` (or a `Comic`) kind is a small add once the EPUB path works. Not in DV.
- **Reflowable-novel UX** (font size, themes, TOC) — foliate provides the hooks; DV wires the manga-first controls (page-turn, spread, RTL), and novel-specific polish is a later refinement.
