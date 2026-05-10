CREATE TABLE IF NOT EXISTS request_log (
    id          INTEGER NOT NULL,
    ts          text    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    method      text    NOT NULL,
    path        text    NOT NULL,
    status      INTEGER NOT NULL,
    ip          text,
    user_agent  text,
    referer     text,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_request_log_ts ON request_log (ts);
