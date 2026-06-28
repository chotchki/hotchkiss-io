-- The unified large-media model (Phase BZ). Every binary asset (image, video,
-- STL, other file) is ONE `media` row keyed by a stable `media_ref` used in
-- markdown as `![alt](/media/<media_ref>)`; the transformer dispatches on `kind`
-- at render. Each stored encoding is a `media_variant` (a video's AV1 + HEVC, an
-- image's single original). The BYTES live in the content-addressed disk store
-- (Settings.media_path), keyed by `sha256` — NOT in the DB, so the daily backup
-- + prod→beta snapshot stay small (only this metadata is in SQLite).
CREATE TABLE IF NOT EXISTS media (
    media_id    INTEGER PRIMARY KEY AUTOINCREMENT,
    media_ref   text    NOT NULL,           -- stable URL/author key (slug)
    kind        text    NOT NULL,           -- 'image' | 'video' | 'stl' | 'file'
    title       text,                       -- author label / default alt text
    width       INTEGER,                    -- px (image/video), nullable
    height      INTEGER,                    -- px, nullable
    duration_ms INTEGER,                    -- video length, nullable
    poster_sha  text,                       -- video poster (an image in the store), nullable
    created_at  text    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (media_ref)
);

-- One row per stored encoding of a media item. `sha256` is the key into the
-- content-addressed disk store; `codecs` carries the `<source type codecs="…">`
-- string for video (av01… / hvc1) and is null for single-codec kinds.
CREATE TABLE IF NOT EXISTS media_variant (
    variant_id  INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id    INTEGER NOT NULL,
    sha256      text    NOT NULL,           -- content-store key
    mime        text    NOT NULL,           -- 'video/mp4', 'image/avif', 'model/stl'…
    codecs      text,                       -- 'av01…' / 'hvc1' for video; null otherwise
    bytes       INTEGER NOT NULL,
    UNIQUE (media_id, sha256),
    FOREIGN KEY (media_id) REFERENCES media (media_id)
        ON DELETE CASCADE ON UPDATE CASCADE
);
