-- Phase BZ.8 (attachments → media). Two nullable columns added ahead of the
-- one-shot DATA migration (run once at startup, in Rust — it stores BLOBs to the
-- disk media store + rewrites page markdown, which SQL can't do):
--   * content_pages.page_cover_media_id — the cover re-homed onto a media item.
--     Populated by the migration; page_cover_attachment_id is dropped in Stage 2
--     once the blog/project cards render from this instead.
--   * attachments.migrated_media_id — marks an attachment already copied to the
--     media store, so the migration is idempotent (skip re-copying).
-- Plain INTEGER, not a declared FK: SQLite ALTER TABLE ADD COLUMN can't add a
-- REFERENCES constraint, and the migration owns the integrity.
ALTER TABLE content_pages ADD COLUMN page_cover_media_id INTEGER;
ALTER TABLE attachments ADD COLUMN migrated_media_id INTEGER;
