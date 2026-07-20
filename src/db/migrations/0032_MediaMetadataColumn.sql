-- media.metadata (Phase ED): ONE extensible JSON bag instead of column sprawl.
-- The 0027 `chapters` column folds in as `metadata.chapters`; the image edit
-- params (Phase ED rotate/crop — applied at rung DERIVATION, the original bytes
-- never touched) land at `metadata.edit`. Typed decode is `MediaDao::meta()`
-- (fail-soft). DROP COLUMN needs SQLite 3.35+ — the sqlx-bundled library is
-- far past that; `chapters` is a plain TEXT column (no index/FK/generated
-- reference), so the drop is legal.
ALTER TABLE media ADD COLUMN metadata TEXT;

UPDATE media
SET metadata = json_object('chapters', json(chapters))
WHERE chapters IS NOT NULL;

ALTER TABLE media DROP COLUMN chapters;
