-- Dead-link checker (Phase DL). Two tables.
--
-- link_check: one row per DISTINCT url ever seen in content. `consecutive_failures`
-- is the memory that makes "confirmed dead" mean "dead for N daily passes", not
-- "flaked once" (confirm-before-alarm) — it persists across scans. `last_class` is
-- one of ok/dead/transient/blocked/unknown.
CREATE TABLE IF NOT EXISTS link_check (
    url                   text    NOT NULL,
    kind                  text    NOT NULL,   -- 'internal' | 'external'
    last_class            text    NOT NULL,   -- ok | dead | transient | blocked | unknown
    last_status           INTEGER,            -- HTTP status, or NULL (DNS/internal/transport)
    detail                text,               -- short human note (error kind, "HTTP 404")
    consecutive_failures  INTEGER NOT NULL DEFAULT 0,
    first_failed_at       text,               -- start of the current non-ok streak
    last_ok_at            text,
    last_checked_at       text    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (url)
);
CREATE INDEX IF NOT EXISTS idx_link_check_class ON link_check (last_class);

-- link_ref: which pages reference which url. REPLACED each scan (a url's referrers
-- change as content is edited), so it always reflects live content. Powers the
-- "broken links grouped by page" admin view. FK CASCADE so deleting a page drops
-- its refs.
CREATE TABLE IF NOT EXISTS link_ref (
    page_id  INTEGER NOT NULL REFERENCES content_pages(page_id) ON DELETE CASCADE,
    url      text    NOT NULL,
    PRIMARY KEY (page_id, url)
);
CREATE INDEX IF NOT EXISTS idx_link_ref_url ON link_ref (url);
