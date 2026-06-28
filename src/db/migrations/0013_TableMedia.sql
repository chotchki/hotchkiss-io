-- The unified large-media model (Phase BZ). Every binary asset (image, video,
-- STL, other file) is ONE `media` row keyed by a stable `media_ref` used in
-- markdown as `![alt](/media/<media_ref>)`; the transformer dispatches on `kind`
-- at render. Each stored encoding is a `media_variant` (a video's AV1 + HEVC, an
-- image's single original; a video poster is just a later image-mime variant —
-- so EVERY servable blob is a variant with one url_key). The BYTES live in the
-- content-addressed disk store (Settings.media_path), keyed by `sha256` — NOT in
-- the DB, so the daily backup + prod→beta snapshot stay small.
CREATE TABLE IF NOT EXISTS media (
    media_id    INTEGER PRIMARY KEY AUTOINCREMENT,
    media_ref   text    NOT NULL,           -- stable URL/author key (slug)
    kind        text    NOT NULL,           -- 'image' | 'video' | 'stl' | 'file'
    title       text,                       -- author label / default alt text
    width       INTEGER,                    -- px (image/video), nullable
    height      INTEGER,                    -- px, nullable
    duration_ms INTEGER,                    -- video length, nullable
    created_at  text    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (media_ref)
);

-- One row per stored encoding. `sha256` keys the disk store. The PUBLIC URL is
-- `url_key` = HMAC-SHA256(server key, sha256), NOT the bare sha: the bytes URL
-- must not be a file-existence oracle (a content hash is guessable by anyone
-- holding a copy, so a raw-sha URL would let them probe whether we host a known
-- file). HMAC is unforgeable without the server key, so only a published item —
-- whose url_key is already in its page HTML — is reachable. Deterministic in the
-- sha, so identical content → one stable, cacheable URL. `codecs` carries the
-- `<source type … codecs="…">` string for video (av01… / hvc1), null otherwise.
CREATE TABLE IF NOT EXISTS media_variant (
    variant_id  INTEGER PRIMARY KEY AUTOINCREMENT,
    media_id    INTEGER NOT NULL,
    sha256      text    NOT NULL,           -- content-store key (never exposed)
    url_key     text    NOT NULL,           -- HMAC-SHA256(server key, sha256), the public token
    mime        text    NOT NULL,           -- 'video/mp4', 'image/avif', 'model/stl'…
    codecs      text,                       -- 'av01.0.12M.08' / 'hvc1' for video; null otherwise
    bytes       INTEGER NOT NULL,
    UNIQUE (media_id, sha256),
    FOREIGN KEY (media_id) REFERENCES media (media_id)
        ON DELETE CASCADE ON UPDATE CASCADE
);

-- Serve route looks the variant up by its url_key. NOT unique: the same content
-- (sha) dedupes to one url_key but may be a variant of more than one media item.
CREATE INDEX IF NOT EXISTS idx_media_variant_url_key ON media_variant (url_key);
