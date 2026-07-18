# Media subsystem — design & contract

THE single source of truth for hotchkiss.io's media. This ABSORBS the former
`docs/fab-scad-roundtrip.md` and the CLAUDE.md "Media" paragraph — read it before
touching `src/media/`, `web/features/media.rs`, `web/features/admin/media.rs`, or
the `media` / `media_variant` schema.

Each section is tagged **[SHIPPED]** (in prod), **[TARGET]** (the design we're
rationalizing toward — not built yet), or plain (background). The gap between
SHIPPED and TARGET is the [Rationalization backlog](#11-rationalization-backlog).

---

## 1. Model & principles

ALL binary media — images, video, audio, STLs/3MF, OpenSCAD, arbitrary files —
lives in a content-addressed **disk store**, NOT SQLite BLOBs. One logical **item**
(`media`) carries N **variants** (`media_variant`) — a video's HEVC + AV1 encodes +
an auto-poster; an image's responsive AVIF ladder; a model's SCAD source + low/high
meshes; an audiobook's AAC track + cover art. The bytes live on disk sharded by
`sha256`; the DB rows are metadata.

Two IDENTIFIERS, both unguessable, each with a job:
- **`media_ref`** — the opaque **UUIDv7** author token. It IS the item's stable
  identity: `![](/media/<ref>)` embeds, `/media/<ref>` the resource. NEVER changes,
  even when the bytes behind it do (this is why we need no "superseded-by" pointer).
- **`url_key`** — `HMAC-SHA256(crypto_keys id 2, sha)`, per-VARIANT. The byte route
  `/media/file/<url_key>` key. Deterministic in the content, so identical bytes
  dedup to one key; it CHANGES when the bytes change (a re-encode / a round-trip
  save mints new keys — embed by `ref`, never by pasted `url_key`).

**Organizing principle — HATEOAS.** `/media/<ref>` is a RESOURCE that describes its
own representations and controls. A caller (the browser, fab-gui, a script, a future
SPA) discovers what variants exist and where to write from the resource itself —
never from a hardcoded URL shape or a memorized format vocabulary. The site owns its
URLs; clients follow the `href`s they're handed. This is the frame the whole
subsystem rationalizes toward: uniform negotiation on `/media/<ref>` for EVERY kind,
with kind-specific logic confined to the presentation layer (the embed).

---

## 2. Storage — `MediaStore` (`src/media/`)  [SHIPPED]

Files under the configured `Settings.media_paths` roots, sharded `ab/cd/<sha256>`,
atomic temp+rename, dedup by content.

- **Streaming ingest.** `MediaStore::stage()` → `StagedBlob`: upload bytes stream
  chunk-by-chunk to a temp in `<root>/.staging`, hashed incrementally (`sha2`), then
  atomic-renamed into `ab/cd/<sha>` (or dedup'd + dropped). So a multi-GB upload is
  disk-bound, not RAM-bound (the old `field.bytes()→Vec<u8>` OOM'd near free-RAM).
  An aborted upload self-cleans on `Drop`. `store(&[u8])` is the in-memory twin
  (posters, resizes) — identical digest, so dedup is consistent across both paths.
- **Multi-drive (CJ).** `media_paths` is an ORDERED list; `pick_write_root` fills
  the first root with `media_min_free_bytes` headroom UP FRONT (so the commit rename
  stays intra-volume — a cross-volume rename is `EXDEV`) and falls through to the
  next when a drive drops below the margin. A root is a write target ONLY if PRESENT
  (its dir or parent exists = the drive is mounted): a cleanly-unmounted external
  root is skipped, never `create_dir_all`'d onto the boot disk (the M1 bug). Configure
  a SUBDIR under each volume, not the mount root, so the present-check is meaningful.
- **Resolve.** Each variant records a `storage_root` HINT (nullable); `resolve_path`
  tries the hint first (O(1)) then first-found-scans all roots — self-healing if a
  file moved. An unmounted root → that variant 404s. Backblaze covers off-site backup
  at the filesystem level, so the app never copies media for backup.
- The `.staging` dir is `--exclude`d from the prod→beta media rsync.
- **Defaults + beta/prod:** the default single root is `app_support/media`; `media_min_free_bytes`
  headroom defaults to 10 GiB. Prod uses the default unless drives are ADDED to `media_paths`.
  **Rename beta's `media_path` → `media_paths` (a one-element array) before deploying** or it
  silently falls back to the default (= prod's) dir. The prod→beta snapshot PRESERVES
  `crypto_keys` id 2 (so beta's HMAC `url_key`s match prod's — WHY a prod/beta key mismatch
  still serves, §6) and rsyncs prod media into beta's OWN dir (beta config points
  `media_paths`/`backup_path` under `io.hotchkiss.web.beta/`, since `app_support` is hardcoded
  to `io.hotchkiss.web`). A per-root stat error falls THROUGH to the next root (L1), never
  aborting the whole upload.

---

## 3. Schema

- **`media`** — `media_id` PK, `media_ref` UNIQUE, `kind` (image/video/stl/audio/file),
  `title`, `width`/`height`/`duration_ms` (the item's dims), `created_at`,
  **`min_role`** (NULL = public; the visibility gate, §4), **`chapters`** (JSON
  `[{start_ms,title}]`, Audio only).
- **`media_variant`** — `variant_id` PK, `media_id` FK (CASCADE), `sha256`, `url_key`
  (NON-unique index — content dedup means many rows share a key; see §4 strictest-wins),
  `mime`, `codecs`, `bytes`, `storage_root` (hint), `width`/`height` (this ENCODING's
  pixels, for an image's srcset).
- Migrations of note: `0014` `content_pages.page_cover_media_id` (covers) · `0016` api_keys
  · `0017` storage_root · `0018` variant width/height · `0026` media.min_role · `0027`
  chapters. `0015` emptied the retired `attachments` table — the one-shot
  `coordinator/migrate_media.rs` (now removed) copied every BLOB → store + `media` rows,
  rewrote `/attachments/…`→`/media/<ref>` (both URL forms), re-homed covers (backup-first,
  defer-on-fail), then deleted the `/attachments` route + `AttachmentDao` (Phase BZ.8, prod
  v0.0.63→v0.0.64). The now-empty `attachments` table + the dead `page_cover_attachment_id`
  column intentionally remain (circular FK — a future `-- no-transaction` rebuild drops them).
- **DJ.4 newtype BOUNDARY:** the DAO signatures / struct fields stay `&str`/`String` and the
  `Path<String>` extractors stay UNTYPED on purpose — a `Path<UrlKey>` deserialize-reject
  would be a `400` (an existence oracle) vs the required identical-to-miss `404`; generation
  sites stay `String` (trusted-by-construction). The newtypes gate at the PARSE boundary
  (`UrlKey::parse` = the 64-hex gate), not everywhere.
- **Typed tokens (DJ.4):** `MediaRef<'a>` (`[A-Za-z0-9_-]`, never UUID-shaped so legacy
  slug refs resolve) and `UrlKey<'a>` (whose constructor `UrlKey::parse` IS the 64-hex
  gate). `ModelFormat` (`Scad`/`Stl`/`ThreeMf` + `from_mime`/`is_mesh`) replaced fragile
  `mime == "model/3mf"` matching. `MediaKind` (image/video/stl/audio/file).

---

## 4. Authorization  [SHIPPED]

Two ORTHOGONAL gates. Both fail closed. Both are enforced OUTSIDE the media handlers
(the handlers assume the gate already ran) — EXCEPT the read-visibility check, which
each read path applies because it's per-item data.

### 4a. Mutation gate — who may WRITE (`require_admin_for_mutations`, Phase E)

A single fail-closed layer, site-wide, inner to the session layer. **`GET`/`HEAD`/
`OPTIONS` are public everywhere** (safe, side-effect-free — an allowlist of SAFE
methods, NOT a deny-list of known-mutating verbs, so a new route/verb is gated by
default). **Every other method requires an authenticated Admin.** Decision order:
safe methods → anonymous WebAuthn ceremony allowlist → role-scoped allowlist
(rank-checked, currently just `POST /library/progress`@Family) → admin fallback →
deny. A MISSING identity denies **401** (`unauthorized_response`); an
authenticated-but-INSUFFICIENT caller denies **403** (`forbidden_response`); the 401
carries NO `WWW-Authenticate` (deliberate — no basic-auth dialog, no MCP OAuth chase).

So for the media resource, with ZERO bespoke wiring:
- **Safe reads** — `GET /media/<ref>` (negotiated), `GET /media/file/<key>`, `GET
  /media/embed/<ref>`, `GET /media/<ref>/variants`, **`OPTIONS /media/<ref>`** — PUBLIC,
  then read-gated per 4b.
- **Writes** — `PUT` / `POST` / `DELETE` on `/media`, `/media/<ref>`, and
  `/media/<ref>/variants` (create, add, replace-all, metadata, delete) — non-safe → admin
  fallback → **Admin only**, automatically. They can't slip past: the WebAuthn +
  role-scoped allowlists are exact `(method, path)` and none of these is listed.
- **The ONE exception — `GET /media` (list ALL items) is a NON-public GET** the safe-method
  default would wrongly expose. Enumerating the whole library is an ADMIN capability (the
  opaque `ref` / HMAC `url_key` design exists precisely to stop non-admins discovering
  media). It needs its OWN `require_admin` on that route — the same pattern `/admin/analytics`
  uses. This is the single spot where "GET is public" and the media model disagree; the
  rationalization MUST gate it explicitly (a public `GET /media` listing would be a
  library-wide leak). The manifest's ROLE-AWARE controls (§5) are the HATEOAS mirror of
  this: a non-admin `OPTIONS` sees no write controls.
Identity is resolved by `refresh_session_role` (cookie session, live role) OR
`api_key_auth` (`Authorization: Bearer hio_…`, full role delegation), injected before
the gate reads `SessionData`.

### 4b. Visibility gate — who may READ (`min_role`, Phase DC)

`media.min_role TEXT NULL`, decoded fail-closed by `MediaDao::min_role_rank`
(NULL→0 / Registered→1 / Family→2 / **everything else → Admin/top**, NEVER the raw
string). Gates all read paths:
- **Byte route** resolves the variant AND a **STRICTEST-WINS** required rank in ONE
  query (`find_by_url_key_with_required_rank`: `MAX(CASE…)` across every item sharing
  the `url_key` — content dedup makes the key index non-unique, so a LIMIT-1 owner
  could be the LOOSEST and leak; MAX can only over-restrict, which breaks VISIBLY).
- **`/media/<ref>` (GET + OPTIONS) + embed** apply `is_visible_to(viewer)` on
  the item. **Denied ≡ the unknown-ref/key 404** — no existence oracle. A denied
  `/media/embed/<ref>` returns the byte-identical bad-ref error-span at 200 (HTMX still
  swaps; the per-viewer embed fetch means the content-keyed render_cache never captures
  a role decision).
- **Embed HTTP `Cache-Control: no-store` is a SECURITY invariant** (`embed_response`, on
  the element AND the miss/error/denial spans): the embed HTML is ROLE-DEPENDENT, so a
  shared/browser cache must never hand one viewer's embed to another — a cached Family
  embed would leak its gated `url_key` to anon (an oracle), a cached anon miss would blank
  a Family view. HTMX refetches per load anyway. The miss and denial spans carry the SAME
  no-store so a header difference isn't itself the oracle.
- **`render_embed_html` bytes-by-`url_key`-ONLY invariant:** the embed/302 handlers gate
  on the item's OWN (possibly LOOSER) `min_role`, while the byte route re-gates
  strictest-wins across `url_key`-sharing items — so the embed must reference bytes ONLY
  via `/media/file/<url_key>` URLs and NEVER inline a `data:` URI or server-read content,
  or it would leak deduped gated bytes through a public item. The looser embed gate is
  safe precisely BECAUSE the byte fetch re-gates.
- **Gated bytes** ship `Cache-Control: private, …, immutable` (browser + range cache
  stay; shared caches cut out); public media unchanged.
- New variants INHERIT the item's gate (they carry no `min_role` of their own — create,
  add-variant, and replace-all all rely on this). Covers gate at the PAGE level; authoring
  rule: public page ⇒ public cover.

### 4c. Executable-content hardening (CL)

The byte route always sends `X-Content-Type-Options: nosniff` and forces
`Content-Disposition: attachment` on executable mimes (`is_active_content_mime`:
`text/html`/`xhtml`/`image/svg+xml`/`*+xml`/`application|text/xml`/`javascript`/
`ecmascript`) — a `MediaKind::File` carries a filename-guessed mime on a public
same-origin route, so an admin-uploaded `.svg`/`.html` must not run as active script
(stored XSS). Probe-verified image/video/stl/audio kinds never hit that set → render
inline. **`Cross-Origin-Resource-Policy: cross-origin` is sent UNCONDITIONALLY** on every
byte response (inserted BEFORE the active-mime check), so ALL public media — images,
video, models, files — is cross-origin hotlinkable/embeddable; this is what lets the
COEP-`require-corp` `/3d/editor` fetch a model's SCAD/mesh bytes (Phase DN / CW.4). CORP
does NOT bypass `min_role` — the gate is enforced ABOVE via `required_rank`, so a denied
request never reaches these headers.

---

## 5. The resource — collection, item, variants  [SHIPPED, Phases DP + DQ]

The read + write surface, uniform across every kind. FOUR resources, and a verb
discipline that keeps it HATEOAS-clean: **every write is an idempotent PUT (replace)
except the two server-assigns-identity creates (POST) — and there is NO PATCH.**

```
/media                            the item collection
/media/<ref>                      an item — metadata + a variant collection
/media/<ref>/variants             the item's variant collection
/media/<ref>/variants/<url_key>   one variant, as a collection member
/media/file/<url_key>             a variant's BYTES (content-addressed, shared; §6)
```

| operation | verb + URL | why this verb | replaces |
|---|---|---|---|
| **create an item** | `POST /media` | server mints the UUIDv7 `ref` → `201` + `Location: /media/<ref>` | `POST /admin/media/upload` |
| list items | `GET /media` | admin-only (a non-public GET; see §4a) | `GET /admin/media` |
| read a representation | `GET /media/<ref>` (negotiated) | safe | `serve_media_by_ref` |
| item state (JSON) | `GET /media/<ref>` + `Accept: application/json` | GET/PUT symmetry | — |
| discover controls | `OPTIONS /media/<ref>` | safe → manifest | — |
| **edit metadata** | `PUT /media/<ref>` (JSON) | replace the item's writable representation (title, min_role) — idempotent | `rename` + `visibility` |
| delete item | `DELETE /media/<ref>` | | `delete_media` |
| **add a variant** | `POST /media/<ref>/variants` | server mints the content-addressed `url_key` | `add_encode` |
| **replace all variants** | `PUT /media/<ref>/variants` | replace the collection — idempotent (fab-gui's SAVE) | DO's `PATCH /media/<ref>` |
| list variants | `GET /media/<ref>/variants` | = the manifest's `variants` | — |
| remove a variant | `DELETE /media/<ref>/variants/<url_key>` | `url_key` is unambiguous WITHIN a ref | `delete_variant` |

**POST vs PUT — the one rule:** POST when the SERVER assigns the URI (create item →
UUIDv7 `ref`; add variant → `HMAC(sha)` `url_key` the client can't derive), PUT when the
CLIENT owns the URI it's replacing (metadata; the whole variant collection). Two POSTs,
both genuine creates; every other write a PUT. No overloaded verb, no PATCH merge-format.

**GET/PUT symmetry on the item.** `GET /media/<ref>` + `Accept: application/json` returns
the item's state; `PUT /media/<ref>` (JSON) replaces its writable fields (title,
min_role). Same shape in and out. Media BYTES are negotiated alternates
(`Accept: image/avif` → 302). `OPTIONS` owns pure control-discovery — it carries the
`controls` block; `GET(json)` carries state.

### The manifest — `OPTIONS /media/<ref>`  [SHIPPED, Phase DP (read) + DQ.7 (controls)]

Every variant a followable link + the controls the caller may use. `min_role`-gated
(denied ≡ 404). `build_manifest(media, variants, viewer)` — served by `OPTIONS`, by
`GET … Accept: application/json` (the item state), and as the `201` create /
`200` write response body. **ROLE-AWARE** (DQ.7): a non-admin sees only
`{ref, self, kind, title, min_role, variants:[{type, bytes, width?, height?, href}]}`;
an Admin ALSO sees each variant's `remove` link + the `controls` block (`add` /
`replace-all` / `metadata` / `delete`) — the HATEOAS mirror of the mutation gate
(`serde skip_serializing_if` drops both for a non-admin).

```json
{
  "ref": "<media_ref>",
  "self": "/media/<ref>",
  "kind": "stl",
  "title": "…",
  "min_role": null,
  "variants": [
    { "type": "application/x-openscad", "bytes": 210,     "href": "/media/file/<k1>", "remove": "/media/<ref>/variants/<k1>" },
    { "type": "model/3mf",              "bytes": 120000,  "href": "/media/file/<k2>", "remove": "/media/<ref>/variants/<k2>" },
    { "type": "model/3mf",              "bytes": 2400000, "href": "/media/file/<k3>", "remove": "/media/<ref>/variants/<k3>" }
  ],
  "controls": {
    "add":         { "href": "/media/<ref>/variants", "method": "POST",   "accepts": "multipart/form-data" },
    "replace-all": { "href": "/media/<ref>/variants", "method": "PUT",    "accepts": "multipart/form-data" },
    "metadata":    { "href": "/media/<ref>",          "method": "PUT",    "accepts": "application/json" },
    "delete":      { "href": "/media/<ref>",          "method": "DELETE" }
  }
}
```
Image variants also carry `width`/`height`; A/V variants `codecs` when known. The client
follows links — no URL construction, no format vocabulary to hardcode. outputSchema root
is an object (not a bare array).

### `GET /media/<ref>` — read, content-negotiated  [SHIPPED, Phase DP]

Precedence **`?format=` (explicit) > `Accept` (preference) > largest (default)**:
- **`?format=<token>`** — `scad`/`stl`/`3mf`/`avif`/`mp4`/… → mime → the LARGEST variant
  of that mime. Unknown token or absent format → **406** (OPTIONS to discover).
- **`Accept: <mime>`** — the largest ACCEPTABLE variant. A browser's `…,*/*` matches
  everything → largest overall, so a plain download link is UNCHANGED (no implied-state
  surprise — this is what killed the scad-first heuristic). A specific, unsatisfiable
  Accept with no `*/*` → **406**. `Accept: application/json` → the item state (§ above).
- **neither** → largest.
Redirects with **HTTP 307** (`Redirect::temporary`, NOT 302) to the chosen
`/media/file/<url_key>`; `Vary: Accept` + `Content-Location`. A zero-variant item →
`404` "no downloadable file for this media". SHIPPED in Phase DP — `serve_media_by_ref`
delegates to `media_select::negotiate` (the ONE selector, §5 / DP.3), which the embed
(§8) and the manifest also use.

### `PUT /media/<ref>/variants` — replace all (the round-trip SAVE)  [SHIPPED, Phase DQ.1 (re-verbed from DO's PATCH)]

The fab-scad round-trip SAVE + general update-in-place. `multipart/form-data`, SAME shape
as create — one file part per file, typed by EXTENSION (`.scad`→`application/x-openscad`,
`.3mf`→`model/3mf`, `.stl`→`model/stl`), streamed to disk (`DefaultBodyLimit::disable()`),
sharing `ingest_multipart` so it can't drift from create. **COMPLETE replacement:** the
uploaded set BECOMES the whole collection, wiped + re-inserted in ONE transaction
(`delete_all_for_media` → `create` each → `update_facts` re-derives kind/dims). The item's
metadata (`title`/`min_role`/`ref`) is untouched BY CONSTRUCTION — it lives on the PARENT
`/media/<ref>`, not this collection (exactly why the collection sub-resource beats DO's
PATCH-that-remembers-to-preserve-metadata). New variants inherit the gate; un-re-uploaded
variants (a render thumbnail) are DROPPED; replaced blobs go cold (no in-line sweep).
`200`/manifest; `400` empty body (a replace-to-nothing is a DELETE). Versioning deferred
(an additive `superseded_at` doesn't break this). **SHIPPED today as `PATCH /media/<ref>`
(Phase DO); DP re-verbs to `PUT …/variants` — inert, no consumer breaks.**

### `POST /media` — create item  ·  `POST /media/<ref>/variants` — add one  [SHIPPED, Phase DQ.2/DQ.3]

Both mint a SERVER-assigned identity → both POST → `201` + `Location`. `POST /media`
creates an ITEM (mints the UUIDv7 `ref`, initial variants in the multipart); `POST
/media/<ref>/variants` adds ONE variant to an existing item (mints its `HMAC(sha)`
`url_key`) WITHOUT re-uploading the rest — the admin curation path, distinct from
replace-all (adding one codec to a big video mustn't force re-sending every encode). Same
multipart ingest as the PUT; content-dedup makes a repeat add an idempotent no-op (bonus,
not contract). `POST /media` takes an optional `min_role`/`title`; a variant inherits the
item's gate.

### `PUT /media/<ref>` — edit metadata  ·  `DELETE …` — remove  [SHIPPED, Phase DQ.4]

`PUT /media/<ref>` (JSON `{title, min_role}`) REPLACES the item's writable metadata — the
honest home for the old `rename` + `visibility` POSTs, idempotent (no PATCH).
`DELETE /media/<ref>` drops the item (CASCADE its variants);
`DELETE /media/<ref>/variants/<url_key>` drops one variant. All Admin-gated by §4a.

---

## 6. Byte route `/media/file/<url_key>`  [SHIPPED]

Streams a variant's bytes via `tower_http::ServeFile` (HTTP range/206, `Accept-Ranges`,
immutable cache). Looked up by the STORED `url_key` (so a prod/beta key mismatch still
serves — resolves by sha). `UrlKey::parse` gates the 64-hex format at the door (junk ≡
404, no oracle). Path resolve runs in `spawn_blocking` with a 5s timeout → 503 on a
wedged/asleep drive (abandons the uncancellable stat, logs loudly; the file READ only
runs once the stat passed). Gating + nosniff + CORP per §4b/4c. EXCLUDED from
`request_log` (a streaming range-storm would self-greylist a listening household via R3
+ swamp the Humans signal). The `CompressionLayer` excludes `video/`/`audio/`/`model/`/
`application/octet-stream` (gzipping a range response corrupts it).

---

## 7. Ingest — the media library (`GET /admin/media` page + the canonical `/media` surface)  [SHIPPED; admin UI migrated onto `/media` in DR]

`GET /admin/media` is the HTML library PAGE; every MUTATION goes through the canonical
`/media` REST surface (§5) — the admin JS (`media-upload.js`) drives `POST /media`,
`POST …/variants`, `PUT`/`DELETE /media/<ref>`, `DELETE …/variants/<key>` (DR). The
shared ingest core `ingest_new_item` (behind `POST /media`) streams each file part through
`ingest_multipart` → stage → commit → `ffprobe` (never trusting the filename:
`.stl`/`.3mf`/`.scad` by ext, image-vs-video by `format.duration`, audio by its first
non-`attached_pic` AUDIO stream). All parts in one upload GROUP into one item; the item
kind is the **DOMINANT** kind (`dominant_kind`: a model/video/audio beats an image — so a
render grouped with a model stays a viewer, order-independent). Then best-effort derived
variants:
- **Poster** (video + audio): ffmpeg frame-grab → AVIF (video thumbnail; audio pulls the
  `attached_pic` cover art → library thumb + lock-screen artwork).
- **Responsive ladder** (image, CN): width-stepped AVIF downscales (480/960, skip ≥
  source), each a content-addressed `image/avif` variant carrying its pixel width.
`append_variants` (behind `POST /media/<ref>/variants`) adds a variant to an existing item
(another codec, or a poster). Uploads POST via `XMLHttpRequest` for a native `<progress>`
bar (CK). A file ffprobe can't type → `MediaKind::File` (mime by extension, octet-stream
fallback) — a graceful download, not a rejection; but a MISSING ffprobe errors loudly
(deploy misconfig). Codec policy: video sources ordered HEVC-before-AV1 (Safari AV1
`<video>` is jerky); audio UNIVERSAL-only (aac→audio/mp4, mp3→audio/mpeg, flac→audio/flac;
opus/vorbis/alac bail → File — AAC m4b is canonical). Visibility: ingest takes a `min_role`
multipart field (known gate roles only; absent/garbage → public — `fab publish` sends
nothing); the editor drop sends the page's current visibility.

**Shipped ingest contracts + asymmetries:**
- **Response bodies (consumers depend on these):** the server-mints-identity creates
  (`POST /media`, `POST …/variants`) → `201` + `Location: /media/<ref>` + the item manifest
  (`{ref, title, min_role, variants:[{href,mime,bytes,…}], controls}`); `PUT /media/<ref>`
  (metadata) + `PUT …/variants` (replace-all) → `200` + the manifest; `DELETE …` → `204`.
  The retired `/admin/media/{upload,encode,rename,visibility,delete,variant}` htmx handlers
  (which returned `htmx_refresh()` / an ad-hoc `{media_id, media_ref, markdown}`) are GONE
  (DR.2) — the library reloads on the REST responses instead.
- **`append_variants` is APPEND-ONLY (DQ.3 decision):** unlike upload/replace it does NOT
  call `add_derived_variants` (an added image gets NO responsive ladder; a video/audio gets
  NO poster) and does NO tx / NO `update_facts` re-derive — you're adding a SPECIFIC variant
  (a codec, a poster, a mesh LOD), not re-ingesting. Use `PUT …/variants` to replace + re-derive.
- **Title fallback:** `title` field → a field literally named `media_ref` used as a TITLE
  candidate (NOT the ref — the ref is always a fresh UUIDv7) → filename via
  `strip_media_suffixes` (drops ext + a trailing codec tag). Empty file parts are silently
  skipped.
- **Audio-classification incident (DD):** an m4b/mp3 whose cover art ships as an mjpeg/png
  stream was pre-DD misread as an unsupported VIDEO and silently degraded to a download
  button; the fix classifies by the first AUDIO stream while EXCLUDING `disposition.attached_pic`.
- **Responsive backfill:** `coordinator/backfill_responsive_images.rs` — DETACHED, idempotent
  startup backfill (never in `try_join!`, backup-first, per-item non-fatal) generating the
  AVIF ladder + stamping widths for pre-CN images.
- **Storage panel:** the admin library renders `roots_status` per configured root (humanized
  free/total, `is_write_target`, `below_margin`) so multi-drive placement isn't silent —
  shares `probe_root` with `pick_write_root`, so the panel and the writer always agree.
- **Tooling:** `ffprobe`/`ffmpeg` must be installed (dev + mini, like `d2`/`weasyprint`); the
  probe KAT runs real ffprobe against `tests/fixtures/chapters.m4b` (aac + attached_pic cover
  + 2 chapters, ffmpeg-generated).

---

## 8. Embed (presentation) `/media/embed/<ref>`  [SHIPPED]

The KIND-SPECIFIC layer — deliberately the opposite of §5's uniform resource. The
markdown transformer rewrites `![](/media/<ref>)` → a `<span hx-get="/media/embed/<ref>"
hx-trigger="load" hx-swap="outerHTML">`; `render_embed_html` dispatches by kind:
- **image** → `<img data-zoomable srcset="…480w,…960w,…origw" sizes>` (`cover_url_for`
  = smallest for a card thumbnail; `cover_hero_for` = largest for the hero).
- **video** → `<video>` multi-`<source>` (HEVC before AV1) + poster.
- **audio** → native `<audio>` + cover-art/title header + `audio-player.js` (chapters,
  ±30s, rate, MediaSession, resume; series playlist auto-advance via track adoption, DG).
- **stl/3mf** → the three.js viewer (`stl_viewer_block`), sized `max-w-2xl h-96` with a
  fullscreen toggle; VIEWER = smallest `model/3mf` (color+fast, else smallest mesh),
  DOWNLOAD = largest mesh, `data-format` (stl|3mf) branches the loader. A scad variant
  adds the **"Open in the slicer"** button (§10). Selection is over `ModelFormat::is_mesh`
  variants only (an image variant is thumbnail, never mis-loaded as mesh).
- **file** → a styled `download_button` (glyph + name + size, `download` attr).

**Selector sharing (DP.3 / DR.4):** the embed's Stl arm (`media_select::viewer_mesh` /
`largest_mesh`), Image arm (`media_select::image_ladder`), AND the COVER helpers
`cover_url_for` (smallest image thumbnail) / `cover_hero_for` (largest image + srcset) all
delegate to the ONE shared selector — so the negotiation (§5), the manifest, the embed, and
covers pick variants the same way. `cover_url_for` = `image_ladder().first()` (smallest),
`cover_hero_for` = `image_ladder().last()` (largest) + the ≥2-sizes srcset, each with a
first-image fallback for a legacy unresized cover (DR.4 retired their hand-rolled SQL).

**Authored references normalize to the stable `media_ref` (→ Phase DS).** On SAVE,
`rewrite_site_links` (already relativizes site links) ALSO rewrites any
`/media/file/<url_key>` in the content → `/media/<ref>` (resolve `url_key` → owning item →
ref; an unresolvable key is left alone, typo-tolerant like the cover-ref parse). Why: the
library's "Copy link" hands out a `/media/file/<url_key>`, and a pasted byte URL bakes a
PER-SAVE key into the content-hash-cached HTML + the feed — it goes STALE the moment that
variant is re-encoded / round-trip-replaced (a `PUT …/variants` mints new `url_key`s) AND
it can't per-viewer-gate (a `![](/media/<ref>)` embed is fetched per viewer; a baked byte
URL is shared for everyone, and for gated media it just 404s from cache). **Covers are
already ref-stable** — stored as a `page_cover_media_id` (`media_id`) via
`parse_cover_reference`, resolved FRESH at render (the hero `<img srcset>` byte URLs are
computed each render, never baked). Content markdown is the gap DS closes; the editor's
drop-upload already inserts `![](/media/<ref>)`, and the save-rewrite backstops a pasted
byte URL. **The rule:** an author never bakes a `url_key`; every authored reference
resolves through the stable ref.

**Shipped embed specifics (the real-device-hardened details):**
- **Audio player** (`audio-player.js`, first-party, `defer`, re-scans on `htmx:afterSettle`):
  a `hidden`-class chapter toggle — NOT `<details>` (invalid inside the embed's `<p>`) —
  ±30s skips, a rate cycle 1/1.25/1.5/1.6/2× persisted GLOBALLY (`localStorage audio-rate`,
  re-asserted on first play — iOS resets it at stream load), `localStorage audio-pos:<ref>`
  resume applied at `loadedmetadata` AND re-asserted once on first play (iOS drops
  pre-metadata seeks), never autoplay on load, degrades to bare native `<audio controls>`
  without JS. MediaSession lock-screen controls fetch gated artwork via a **credentialed-
  fetch→blob** fallback (the lock screen's own fetch may go out cookieless → a gated cover
  would 401).
- **Series playlist (DG) track ADOPTION** — phone-proven necessary: iOS keeps a gesture-less
  `<audio>` MUTED until unlock, so `next.play()` advances silently. Screen-VISIBLE `ended`
  plays the real next element (starting any player pauses the rest). Screen-HIDDEN advance
  ADOPTS the finished element (owner of the live audio session): swaps its `src` to the next
  book, files saves under the ADOPTED book's resume key, presents adopted lock-screen metadata
  (per-ref blob cache); lock-screen next/prev adopt in BOTH directions while hidden. On
  `visibilitychange`→visible, playback hands back to the real per-book player at the carried
  position — **pause-BEFORE-clearing-adopted is load-bearing** (the pause-save must file under
  the adopted key), and if iOS blocks the gesture-less hand-back play the player sits paused
  (one tap resumes). Validated on a real iPhone as an installed **PWA** — Safari-tab vs PWA
  have SEPARATE cookie jars, so the PWA is the supported mode for gated audiobooks.
- **STL/3MF viewer:** color via a vendored `3MFLoader` + `fflate` matched to three.js **r173**;
  a colorless STL defaults its material to the site yellow `#ffc935` (overridable via
  `data-color`). Emitted with `<span>` block/flex wrappers, NOT `<div>` (a standalone
  `![](x.stl)` sits inside a `<p>`, where a block `<div>` is invalid HTML).
- **Covers are media** (migration `0014` `content_pages.page_cover_media_id`): `cover_url_for`
  = smallest thumbnail, `cover_hero_for` = largest hero. A pasted cover field runs through
  `parse_cover_reference` → `resolve_cover_media_id` (tolerates markdown-embed / bare-ref /
  full-URL / bare-token); empty clears, a non-empty-UNRESOLVABLE ref is LEFT ALONE (a typo
  can't wipe the cover — the old exact-match `find_by_ref` on the raw string silently wiped it).
- **Inline editor upload** (`editor-support.js`): drop on the markdown box (or the 🎞 button)
  → async `POST /media` → reads the manifest `ref` → inserts `![](/media/<ref>)` at the cursor
  with NO page refresh (DR; the old attachment upload `htmx_refresh()`'d + ate unsaved edits).

---

## 9. Kinds — the variant profile

| kind  | variants | negotiation example |
|---|---|---|
| image | responsive AVIF ladder (+ original) | `?format=avif` / `Accept: image/avif` → largest; per-variant `width` picks a size by `href` |
| video | HEVC + AV1 `video/mp4` + poster AVIF | `mp4` → largest; poster is `image/avif` |
| audio | AAC (`audio/mp4`) + cover art AVIF | `m4a`/`aac` → the audio; art is `image/avif` |
| stl   | scad + low/high mesh (3mf/stl) | `scad` → source, `3mf`/`stl` → largest mesh |
| file  | the one file | its own type |

`bytes` is the fidelity key everywhere (decimation/downscale is monotonic).

**Producer contract + selection load-bearers:**
- `fab publish`'s Downloads links use `/media/<ref>` (the upload API returns a `media_ref`,
  not a `url_key`) — a bare `GET /media/<ref>` used to 404 (the "bowtie" regression), which
  is WHY the one-segment ref route resolves to bytes at all (§5).
- The site does NOT decimate — `fab publish` provides the low-res mesh (grouped item or
  `POST …/variants`), like it provides video encodes. STL variants carry no `width`, so `bytes`
  is the fidelity key, and because `variant_id` fetch order is insertion order, the render's
  `sort_by_key(bytes)` is LOAD-BEARING for the viewer(smallest) / download(largest) picks.
- **`fab publish` initial-upload contract** (distinct from §10's round-trip export): upload
  the full + decimated STL as ONE item (grouped drop or item+encode); ship 3MF / OpenSCAD
  source as SEPARATE `MediaKind::File` items, each its own download button, by `title`.
- **Bambu PLATES stay download buttons** NOT via kind detection but because `fab` lists them
  as plain markdown LINKS — only an `![](/media/<ref>)` EMBED hits the kind dispatch (where
  intent is always "show it"); a plain link never does.

---

## 10. The 3D round-trip (Phases DN → DO → DP)

A model is one item carrying its SCAD source + low/high meshes. fab-gui (the WASM SCAD
slicer at `/3d/editor`) both LOADS the scad and SAVES edits back — all on the uniform
resource, nothing model-special:

- **Load [SHIPPED, Phase DP]:** the embed's "Open in the slicer" button →
  `?model=/media/<ref>?format=scad`. fab-gui `fetch_text`s the model URL → the negotiated
  GET 307-redirects `?format=scad` to the scad source; the `ref` lives in the URL's PATH,
  so the SAVE target is derivable by dropping the query (`PUT /media/<ref>/variants`, DQ)
  or via the OPTIONS manifest. Emits the STABLE ref, never the per-save `url_key`.
- **Save [SHIPPED, Phase DQ.1 — `PUT /media/<ref>/variants`]:**
  replace the whole variant collection (§5). The opaque ref never changes, so every
  `![](/media/<ref>)` embed stays valid across edits — no supersession pointer.
- **fab-gui side (upstream):** export low-res mesh (3MF multicolor / STL single-color,
  keep low+high SAME format) + full-res 3MF + the edited SCAD; `PUT` all three to
  `/media/<ref>/variants` (multipart file parts, correct extensions), authenticated by the
  ambient same-origin session cookie. INERT until the fab-gui pin bump — the shipped bundle
  doesn't drive save yet.
- Color: STL carries none; a multicolor model ships low+high 3MF (`3MFLoader` shows
  standard basematerials/color-groups, NOT Bambu MMU paint). `.scad` mime is the
  community `application/x-openscad` (its `application/` prefix keeps it out of the
  `model/*` mesh glob for free).
- Open decision: public SCAD + parametric fab-gui = any visitor can re-slice via the load
  path (a free customizer) — desirable or a gating call.

---

## 11. Rationalization backlog

The deltas between SHIPPED and TARGET — the code-alignment work (Phase DP), scoped:

- **✓ SHIPPED (DP): `GET /media/<ref>` negotiation** — `?format=`/`Accept` (precedence,
  `Vary`, `Content-Location`, 406) via `media_select::negotiate`; a `*/*`-bearing Accept →
  largest (bare-link behavior unchanged). The scad-first heuristic idea (implied state)
  stayed killed.
- **✓ SHIPPED (DP, read shape): `OPTIONS /media/<ref>` manifest** — the HATEOAS entry
  point (`build_manifest`, `.options()` on the `/{media_ref}` method-router;
  safe-method-public + `min_role`-gated). **DQ adds** the ROLE-AWARE `controls` block +
  per-variant `remove` once the write surface exists.
- **✓ SHIPPED (DQ): write surface** — the full HATEOAS surface (§5): `POST /media`
  (create, `201`+`Location`+manifest), `POST /media/<ref>/variants` (add, append-only),
  `PUT /media/<ref>/variants` (replace-all — re-verbed from DO's `PATCH /media/<ref>`),
  `PUT /media/<ref>` (metadata, absent-keeps), `DELETE /media/<ref>` + `…/variants/<key>`.
  **Zero PATCH.** The create + add-variant handlers share the ingest cores
  (`ingest_new_item` / `append_variants`), so the surface can't drift.
- **✓ SHIPPED (DQ.5): `GET /media` admin gate** — the list-all self-gates to Admin
  (`403` for a non-admin — the safe-method default would leak the whole library, §4a).
- **✓ SHIPPED (DR): admin-route migration** — the admin library (`media.html` +
  `media-upload.js`) + the inline editor upload (`editor-support.js`) drive the canonical
  `/media` surface; the `/admin/media/{upload,encode,rename,visibility,delete,variant}`
  htmx handlers + routes are RETIRED (DR.2), and the covers adopt the shared selector
  (DR.4). `GET /admin/media` stays as the library PAGE. Covered by HTTP tests + real-Chrome
  e2e (`admin_media_library_full_crud_in_browser`, `inline_editor_upload_inserts_media_ref_in_browser`).
  Kept-as-JS-not-htmx is deliberate — revisit at htmx v4 (see the decision note).
- **✓ SHIPPED (DP): the slicer button** emits `?model=/media/<ref>?format=scad` (ref in
  the path, format explicit) instead of the per-save `url_key`.
- **✓ SHIPPED (DP): shared variant selector** — `web/features/media_select.rs` (format
  token→mime, `largest`/`largest_of_mime`, `viewer_mesh`/`largest_mesh`, Accept parsing +
  `negotiate`) is the ONE selector used by the negotiated GET, the manifest, AND the embed
  (§8's Stl arm now delegates to `viewer_mesh`/`largest_mesh`).
- **✓ DONE: docs** — this file is the source; `fab-scad-roundtrip.md` + the CLAUDE.md media
  paragraph are pointers here.
- Deferred: size-within-format negotiation (`?width=`/`sizes`); a `?format=` token↔mime
  table vs. suffix-matching; whether OPTIONS also answers a `GET … Accept: application/json`.
