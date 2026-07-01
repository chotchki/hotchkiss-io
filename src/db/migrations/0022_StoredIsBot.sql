-- CR.2 — stored is_bot (0/1) so the audience filter + the 3-chip become cheap indexed
-- counts instead of request_log_view's per-row 25-LIKE ua_class (the ~1.2s → ~0.05s
-- win at 300k rows). The classification single-source is now the Rust fn
-- `request_log::is_bot(user_agent)`, used at write (the middleware insert) AND by
-- `reclassify_bots` (the idempotent startup backfill + the admin recompute command) —
-- so the ruleset stays RETUNABLE despite being stored (run the recompute after editing
-- the rules), it's just no longer re-derived on every query. The VIEW is dropped.
--
-- Nullable: a row is is_bot NULL until the startup backfill (or the next recompute)
-- stamps it; the audience queries treat NULL as "neither" (a transient undercount only
-- during the first-boot backfill, which is fast).
ALTER TABLE request_log ADD COLUMN is_bot INTEGER;
DROP VIEW IF EXISTS request_log_view;
CREATE INDEX IF NOT EXISTS idx_request_log_ts_is_bot ON request_log (ts, is_bot);
