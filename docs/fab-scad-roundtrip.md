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

## Save — the fab-gui side to build (Half 2, upstream)

To close the loop, fab-gui (when a logged-in Admin drives it) needs to:

1. **Export** from the current SCAD: the **low-res mesh** for the web viewer — a **low-res 3MF** for a
   multicolor model (color-preserving), or a low-res STL for single-color — the full-res **3MF**
   (color, print), plus the edited **SCAD text** itself. Keep low + high the SAME format (see
   *Color & resolution* above).
2. **Upload** all three to the site's media API, **authenticated by the ambient session cookie** —
   the editor is same-origin, so a logged-in Admin's POST carries the cookie automatically. No token
   plumbing, no OAuth.
3. **Target the EXISTING item**, not a new one. Carry the item's `media_ref` from load → save (the
   site puts it in the editor URL; fab-gui echoes it on upload). The site then **updates the item's
   variants IN PLACE** on the same `media_ref` — so every `![](/media/<ref>)` embed on the site stays
   valid with zero rewrite. (This is why we don't need a "superseded-by" pointer: the opaque ref never
   changes; only the bytes behind it do.)

## Site-side pieces Half 2 adds (so fab-gui has something to POST to)

- An **update-item-variants** endpoint (Admin): `POST` the three files targeting an existing
  `media_ref`, replacing the same-format variants. (Today `/admin/media/upload` MINTS a new item;
  `add_encode` adds a variant to an item by id — Half 2 wires the "update in place by ref" path.)
- The item ref threaded into the editor URL on load, and read back on save.

## Recommended authoring workflow

1. Upload the SCAD (fab-gui can generate the initial STL+3MF, or `fab publish` provides them) → one
   media item, ref `R`.
2. Embed `![](/media/R)` on the model page → the web gets the viewer (stl/3mf) + an **"Open in the
   slicer"** button.
3. **Edit:** click the button → fab-gui loads the SCAD (URL carries `R`). Tweak params / geometry.
4. **Save:** fab-gui exports STL+3MF+SCAD and POSTs them targeting `R` → variants updated in place.
   The page re-renders with the new mesh + source; the embed link is untouched.

## Open decisions for fab-gui to weigh in on

- **Export targets:** decimated-STL tri-count budget (small enough for a snappy web viewer), and 3MF
  color fidelity (standard basematerials/color-groups — the site's `3MFLoader` doesn't show slicer
  paint extensions like Bambu MMU).
- **Replace vs. version:** does a re-upload REPLACE the old same-format variant, or accumulate (free
  edit history + a per-item "current variant" marker)? Site-side call, but it shapes the endpoint.
- **Public source ⇒ free customizer:** because the SCAD is public and fab-gui is parametric, any
  visitor can already tweak params and re-slice via the load path. If that's desirable, nothing extra
  is needed; if not, that's a gating decision.
