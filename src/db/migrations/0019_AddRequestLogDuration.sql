-- CQ.1 — server-handler processing time per request.
-- Nullable + NO index: legacy rows (pre-CQ) and beta-scrubbed rows stay NULL, and
-- the column is read only by the on-demand admin latency views (never a hot GROUP BY
-- that would earn an index). This is SERVER-handler time, not client page-load/LCP.
ALTER TABLE request_log ADD COLUMN duration_ms INTEGER;
