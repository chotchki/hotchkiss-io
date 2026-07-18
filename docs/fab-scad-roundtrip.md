# fab-gui ↔ hotchkiss.io: the model round-trip contract

A model on the site is ONE media item (`media_ref`, stable) carrying up to three variants,
each with a distinct consumer:

| variant | MIME | consumer |
|---|---|---|
| **source** | `application/x-openscad` | fab-gui — the thing it slices/edits |
| **low-res mesh** | `model/3mf` (multicolor) — or `model/stl` (single-color) | the site's three.js web viewer (fast preview) |
| **full-res mesh** | `model/3mf` | download / print (color) |

All three are **public** (same `min_role` as each other). Only the **write** (edit → re-upload) is
Admin-gated. There is no `model/scad` — that MIME isn't IANA-registered; `application/x-openscad`
is the de-facto and is what the site stores/serves.

### Color & resolution — the low-res mesh format matters

STL carries **no color**, so a MULTICOLOR model can't use a low-res STL for the web viewer. It ships
**low-res 3MF + high-res 3MF** (both color). A single-color model can use STL (smaller).

The site already handles this — `render_embed_html` selects **viewer = the SMALLEST `model/3mf`**
(color + fast) and **download = the LARGEST mesh**. So a multicolor set (low 3MF + high 3MF) renders
the low 3MF in the viewer and the high 3MF for download, with zero site change.

**One export rule to respect:** keep the low + high mesh the **SAME format**. Don't mix a low-res STL
with a high-res 3MF — the selector prefers *any* 3MF for the viewer (for color), so it would load the
*big* 3MF. Multicolor → both 3MF; single-color → both STL.

## Load — SHIPPED (Phase DN), zero fab-scad change

The site opens the editor at:

    /3d/editor?model=/media/file/<scad-url-key>

fab-gui already reads `?model=` and `fetch_text`s it. The URL is **same-origin** and the site sends
`Cross-Origin-Resource-Policy: cross-origin` on the byte route, so the COEP:require-corp editor can
fetch it. It's served `application/x-openscad`, which fab-gui reads as text. Nothing to build here.

## Save — the site SAVE TARGET, SHIPPED (Phase DO)

The site now exposes the update-in-place endpoint fab-gui POSTs to. The contract is **FROZEN** — build
the fab-gui side against exactly this:

    PATCH /media/<ref>

- **Verb + URL:** `PATCH` on the item's OWN ref URL — the same identity the editor loaded from. Not a
  new `/admin/…` route: the ref IS the resource, and the fail-closed mutation layer gates any non-GET
  to Admin automatically (the WebAuthn + role allowlists are POST-only, so a PATCH can't slip past the
  admin fallback — no bespoke guard, and it can't be forgotten).
- **Auth:** the ambient **session cookie**. The editor is same-origin, so a logged-in Admin's PATCH
  carries it — no token, no OAuth. A missing identity is a `401`, insufficient a `403`, from the
  global layer.
- **Body:** `multipart/form-data`, the SAME shape as `/admin/media/upload` — one **file part per file**
  (the edited SCAD + the meshes). The site types each by its filename EXTENSION, so send correct ones:
  `.scad` → `application/x-openscad`, `.3mf` → `model/3mf`, `.stl` → `model/stl`. Non-file fields are
  ignored (the gate is preserved, never re-set here). Body size is unlimited (streamed to disk, not
  buffered).
- **Semantics — COMPLETE replacement:** the uploaded set BECOMES the item's entire variant set, wiped +
  re-inserted in ONE transaction. The item's identity — `media_ref`, `title`, and `min_role` gate — is
  preserved, so every `![](/media/<ref>)` embed and the gate survive with zero rewrite; the new variants
  INHERIT the item's gate. Anything NOT re-uploaded (e.g. a hand-added render-image thumbnail) is
  DROPPED — the uploaded set is authoritative. Replaced blobs go cold on disk (content-addressed,
  Backblaze-backed; no in-line sweep, same as delete).
- **Response:** `200` JSON `{ "media_ref": "…", "kind": "stl", "variants": [ { "url_key": "…", "mime":
  "model/3mf", "bytes": 12345 }, … ] }` — the final variant set, so fab-gui can confirm the swap. `404`
  unknown ref, `400` empty body (a replace-to-nothing is a DELETE, not a PATCH).

**One boundary to design around:** the byte `url_key` is `HMAC(sha)`, so replacing bytes CHANGES it —
any author-pasted `/media/file/<url_key>` link goes stale, but `![](/media/<ref>)` embeds resolve live
and stay valid. Embed by ref; let raw byte URLs be URLs.

## Save — the fab-gui side still to build (upstream)

To close the loop, fab-gui (when a logged-in Admin drives it) needs to:

1. **Export** from the current SCAD: the **low-res mesh** for the web viewer — a **low-res 3MF** for a
   multicolor model (color-preserving), or a low-res STL for single-color — the full-res **3MF**
   (color, print), plus the edited **SCAD text** itself. Keep low + high the SAME format (see
   *Color & resolution* above).
2. **PATCH** all of them to `/media/<ref>` as multipart file parts (correct extensions per the contract
   above), **authenticated by the ambient session cookie** — the editor is same-origin, so a logged-in
   Admin's PATCH carries it automatically. No token plumbing, no OAuth.
3. **Carry the item's `media_ref`** from load → save. The site puts it in the editor URL on load; fab-gui
   echoes it into the PATCH URL. The site updates the variants IN PLACE on the same ref — so every
   `![](/media/<ref>)` embed stays valid with zero rewrite. (This is why we don't need a "superseded-by"
   pointer: the opaque ref never changes; only the bytes behind it do.)

## Recommended authoring workflow

1. Upload the SCAD (fab-gui can generate the initial STL+3MF, or `fab publish` provides them) → one
   media item, ref `R`.
2. Embed `![](/media/R)` on the model page → the web gets the viewer (stl/3mf) + an **"Open in the
   slicer"** button.
3. **Edit:** click the button → fab-gui loads the SCAD (URL carries `R`). Tweak params / geometry.
4. **Save:** fab-gui exports STL+3MF+SCAD and PATCHes them to `/media/R` → variants replaced in place.
   The page re-renders with the new mesh + source; the embed link is untouched.

## Open decisions for fab-gui to weigh in on

- **Export targets:** decimated-STL tri-count budget (small enough for a snappy web viewer), and 3MF
  color fidelity (standard basematerials/color-groups — the site's `3MFLoader` doesn't show slicer
  paint extensions like Bambu MMU).
- **Replace vs. version:** DECIDED — **complete replace** (Phase DO). A PATCH wipes the whole variant
  set and re-inserts the uploaded one; the item identity (ref/title/gate) is preserved. No edit history,
  no "current variant" marker. If versioning is ever wanted, a `media_variant.superseded_at` + a current
  marker is an additive change that doesn't break this contract.
- **Public source ⇒ free customizer:** because the SCAD is public and fab-gui is parametric, any
  visitor can already tweak params and re-slice via the load path. If that's desirable, nothing extra
  is needed; if not, that's a gating decision.
