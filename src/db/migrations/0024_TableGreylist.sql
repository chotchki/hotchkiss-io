-- CX.1 — greylist + clearance tables for the behavioral bot challenge (design +
-- rationale in docs/greylist-challenge-design.md). The detection sweep upserts abusive
-- IPs into `greylist`; auto (behavioral) rows carry a SLIDING `expires_at` (extended on
-- every re-trip, lapsed after quiet), a manual pin is `manual = 1` + NULL expiry (never
-- lapses until released). The request path reads an IN-MEMORY snapshot of the active set
-- (refreshed by the sweep), so this table is NEVER on the hot path — the datetime()
-- compares here run on the timer, not per request.
--
-- `greylist_clearance` records every solved toll: the "passing is a signal" data (a
-- cleared client actually ran the JS) AND the feed for the deferred clear-then-scan
-- escalation. The clearance COOKIE is a non-IP-bound bearer token (see the design doc);
-- this table still records which IP solved, for analytics + escalation only.
--
-- `request_log.challenged` (0/1, nullable like `is_bot`) marks a request that got the 429
-- toll, so the analytics can split challenged traffic out of the human numbers honestly.

CREATE TABLE IF NOT EXISTS greylist (
    id          INTEGER NOT NULL,
    ip          text    NOT NULL,
    reason      text    NOT NULL,
    evidence    text,
    manual      INTEGER NOT NULL DEFAULT 0,
    created_at  text    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at  text    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at  text,
    PRIMARY KEY (id)
);

-- One row per IP — the sweep upserts (ON CONFLICT(ip)) to slide the expiry.
CREATE UNIQUE INDEX IF NOT EXISTS idx_greylist_ip ON greylist (ip);
CREATE INDEX IF NOT EXISTS idx_greylist_expires_at ON greylist (expires_at);

CREATE TABLE IF NOT EXISTS greylist_clearance (
    id             INTEGER NOT NULL,
    ip             text    NOT NULL,
    cleared_at     text    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    solve_ms       INTEGER,
    digest_version INTEGER,
    user_agent     text,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_greylist_clearance_ip ON greylist_clearance (ip);
CREATE INDEX IF NOT EXISTS idx_greylist_clearance_cleared_at ON greylist_clearance (cleared_at);

ALTER TABLE request_log ADD COLUMN challenged INTEGER;
CREATE INDEX IF NOT EXISTS idx_request_log_ts_challenged ON request_log (ts, challenged);
