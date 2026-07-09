-- Media visibility (Phase DC): the MINIMUM role that may fetch this item's
-- bytes / 302 / embed — the same NULL-only-public, fail-closed semantics as
-- content_pages.min_role (0025). Enforcement is strictest-wins across items
-- sharing a url_key (content-addressed dedup makes the url_key index
-- deliberately NON-unique, so a LIMIT 1 lookup alone could resolve to the
-- loosest owner and leak silently).
ALTER TABLE media ADD COLUMN min_role TEXT;
