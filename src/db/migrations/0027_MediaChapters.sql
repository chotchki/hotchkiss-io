-- Audiobook chapters (Phase DD): JSON `[{"start_ms": N, "title": "…"}]`,
-- stamped at ingest for MediaKind::Audio from ffprobe's -show_chapters.
-- NULL for everything else (and for a chapterless audio file).
ALTER TABLE media ADD COLUMN chapters TEXT;
