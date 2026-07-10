-- Dead-link checker (DL.9 dogfood): a manual "ignore" flag. A link the checker
-- can't verify but the operator confirmed by hand — a content-negotiating SPA that
-- always 200s (crates.io), an IP/login-walled host that works in a browser — gets
-- dismissed so it stops showing as a problem. A browser-side auto-recheck is
-- CORS-blocked (opaque cross-origin responses), so the human's judgment IS the
-- check. The daily scan still records the url; the admin view moves ignored rows
-- out of the confirmed/failing/review buckets into a separate collapsed list.
ALTER TABLE link_check ADD COLUMN ignored INTEGER NOT NULL DEFAULT 0;
