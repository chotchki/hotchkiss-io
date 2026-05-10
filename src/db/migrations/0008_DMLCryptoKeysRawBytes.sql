-- crypto_keys.key_value now holds the raw 64-byte cookie::Key master key
-- instead of a JSON-serialized Key (the latter only existed to lean on a
-- forked `cookie` crate for serde impls). Existing rows hold JSON text
-- bytes, which are not a valid key to reuse — clear them so a fresh key
-- is generated on next boot.
--
-- Side effect: existing signed session cookies become invalid; users
-- re-authenticate (passkey tap). Acceptable for this site's user base.
DELETE FROM crypto_keys;
