-- Page visibility, second axis (Phase DA): the MINIMUM role that may read the
-- page, TEXT role name beside the scheduling gate. NULL is the ONLY public
-- spelling — both the Rust gate (ContentPageDao::min_role_rank) and the SQL
-- CASE in the paged queries decode any unrecognized non-NULL value as
-- Admin-only, so a value this binary doesn't know (manual DB edit, a future
-- role after a rollback) hides content instead of leaking it.
ALTER TABLE content_pages ADD COLUMN min_role TEXT;
