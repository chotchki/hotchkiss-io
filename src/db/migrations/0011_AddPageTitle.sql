-- Separate the human display title from the URL slug (page_name).
-- Nullable: existing rows fall back to the markdown H1, then page_name, at
-- display time (see ContentPageDao::display_title), so no backfill is needed.
ALTER TABLE content_pages ADD COLUMN page_title text NULL;
