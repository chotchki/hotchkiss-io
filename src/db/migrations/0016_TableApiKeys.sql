-- Phase CA: user-managed API keys that DELEGATE a user's access — a key carries
-- the minting user's role (full delegation), so an `Authorization: Bearer hio_…`
-- request authenticates as that user across every route. The key is shown ONCE at
-- creation and never stored; only its HMAC-SHA256(server pepper = crypto_keys
-- id 3, key) hex hash lives here, so a DB leak alone can neither recover a key nor
-- verify one offline (the pepper isn't in the DB row). Keys are live until
-- revoked (revoked_at set); last_used_at is stamped on each authenticated request.
CREATE TABLE IF NOT EXISTS api_keys (
    id           INTEGER PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES users (id),
    key_hash     TEXT NOT NULL UNIQUE,
    label        TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at   TEXT
);
