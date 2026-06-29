-- Per-variant pixel dimensions (Phase CN). An image's width-stepped AVIF variants
-- each record their own width so the render can emit a srcset `Nw` descriptor and
-- the browser pulls an appropriately-sized file instead of the full-resolution
-- original (PSI "improve image delivery"). `media.width`/`height` is the ORIGINAL
-- item's dims; this is per-ENCODING. Nullable: video/stl/file variants + legacy
-- rows leave it NULL and are simply omitted from the srcset.
ALTER TABLE media_variant ADD COLUMN width INTEGER;
ALTER TABLE media_variant ADD COLUMN height INTEGER;
