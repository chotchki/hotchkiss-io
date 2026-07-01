-- CR.1 — covering indexes so the analytics GROUP-BY queries run INDEX-ONLY (no temp
-- b-tree for the grouping, no per-row row fetch), collapsing each from ~0.4-0.6s to
-- ~0.02-0.05s at 300k rows (measured). Column order is (grouping/leading key, ts for
-- the window filter, then the covering payload the query also reads). The ts prefix on
-- idx_request_log_ts_ip serves the per-day distinct-IP query (day comes from substr(ts)).
--
-- Write cost: every fire-and-forget insert now also maintains these; fine at
-- personal-site write rates (SQLite sustains thousands of index updates/sec). If the
-- write path ever contends, the batched-writer deferral (SPEC) is the lever, not
-- dropping indexes the read path depends on.
CREATE INDEX IF NOT EXISTS idx_request_log_path ON request_log (path, ts, status);
CREATE INDEX IF NOT EXISTS idx_request_log_ip ON request_log (ip, ts, status, path);
CREATE INDEX IF NOT EXISTS idx_request_log_referer ON request_log (referer, ts);
CREATE INDEX IF NOT EXISTS idx_request_log_user_agent ON request_log (user_agent, ts);
CREATE INDEX IF NOT EXISTS idx_request_log_ts_ip ON request_log (ts, ip);
