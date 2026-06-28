//! Content-addressed disk store for large media (video, big STLs) — files keyed
//! by SHA-256 under `Settings.media_path`, sharded so one directory never holds
//! thousands of entries. Kept OUT of SQLite so the daily backup + prod→beta
//! snapshot don't copy gigabytes (only the small metadata rows live in the DB).
//! Content-addressed buys three things: identical bytes dedupe, the hash IS the
//! cache key, and a stored file is immutable — safe to serve with a far-future
//! cache header and a `206` range response.

pub mod probe;

use anyhow::{Context, Result};
use openssl::sha::sha256;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct MediaStore {
    root: PathBuf,
}

impl MediaStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `<root>/ab/cd/<full-64-hex>` — two levels of one-byte sharding keep any
    /// single directory small even with thousands of files. Caller MUST pass a
    /// validated hash (see [`is_sha256_hex`]); `store` always does, and the serve
    /// route validates the URL path segment before calling.
    pub fn path_for(&self, sha_hex: &str) -> PathBuf {
        self.root
            .join(&sha_hex[0..2])
            .join(&sha_hex[2..4])
            .join(sha_hex)
    }

    pub fn exists(&self, sha_hex: &str) -> bool {
        self.path_for(sha_hex).is_file()
    }

    /// Store bytes content-addressed; returns the SHA-256 hex. Idempotent: if the
    /// content is already present it's a no-op (dedupe). Writes to a temp file in
    /// the same shard dir then renames into place, so a concurrent reader never
    /// sees a half-written file. Sync fs IO — callers in an async context should
    /// wrap large writes in `spawn_blocking`.
    pub fn store(&self, bytes: &[u8]) -> Result<String> {
        let sha_hex = hex_sha256(bytes);
        let dest = self.path_for(&sha_hex);
        if dest.is_file() {
            return Ok(sha_hex); // already stored → dedupe
        }
        let dir = dest.parent().expect("path_for always has a parent");
        fs::create_dir_all(dir).with_context(|| format!("create media shard dir {dir:?}"))?;
        // temp + atomic rename within the same shard dir (same filesystem) so a
        // reader never observes a partial file.
        let tmp = dir.join(format!(".tmp-{sha_hex}"));
        fs::write(&tmp, bytes).with_context(|| format!("write temp media file {tmp:?}"))?;
        fs::rename(&tmp, &dest).with_context(|| format!("rename media into place {dest:?}"))?;
        Ok(sha_hex)
    }
}

/// SHA-256 of `bytes`, lowercase hex (64 chars).
pub fn hex_sha256(bytes: &[u8]) -> String {
    sha256(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256(`hmac_key`, `sha_hex`) → lowercase hex (64 chars). The PUBLIC
/// media URL token: unforgeable without the server key, so `/media/file/<token>`
/// can't be used as an existence oracle (a content hash is guessable by anyone
/// holding a copy; a raw-sha URL would let them probe whether we host it). The
/// `hmac_key` is a server secret generated + persisted once (CryptoKey, like the
/// session signing key). Deterministic in the sha → identical content yields one
/// stable, cacheable token.
pub fn media_url_key(hmac_key: &[u8], sha_hex: &str) -> Result<String> {
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::sign::Signer;

    let pkey = PKey::hmac(hmac_key).context("building HMAC key")?;
    let mut signer = Signer::new(MessageDigest::sha256(), &pkey).context("HMAC signer")?;
    signer.update(sha_hex.as_bytes()).context("HMAC update")?;
    let mac = signer.sign_to_vec().context("HMAC sign")?;
    Ok(mac.iter().map(|b| format!("{b:02x}")).collect())
}

/// True iff `s` is exactly 64 lowercase hex chars — i.e. a well-formed SHA-256.
/// The serve route gates the URL path segment on this so a request can't slip a
/// `../` or a short slice past [`MediaStore::path_for`]'s indexing.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn store_is_content_addressed_shards_and_dedupes() {
        let dir = tempdir().unwrap();
        let store = MediaStore::new(dir.path().to_path_buf());

        let sha = store.store(b"hello video").unwrap();
        assert_eq!(sha.len(), 64);
        assert!(is_sha256_hex(&sha));
        assert!(store.exists(&sha));

        // sharded path: <root>/<2>/<2>/<full hash>, holding the exact bytes
        let p = store.path_for(&sha);
        assert!(p.is_file());
        assert!(p.ends_with(&sha));
        assert_eq!(p.parent().unwrap().file_name().unwrap(), &sha[2..4]);
        assert_eq!(fs::read(&p).unwrap(), b"hello video");

        // idempotent: same content → same hash, no second file, no temp left over
        let sha_again = store.store(b"hello video").unwrap();
        assert_eq!(sha, sha_again);

        // different content → different hash
        assert_ne!(sha, store.store(b"different bytes").unwrap());
    }

    #[test]
    fn media_url_key_is_deterministic_keyed_and_unforgeable() {
        let key = b"a-64-byte-ish-server-secret-from-the-crypto-keys-table-xxxxxxxxxx";
        let sha = "a".repeat(64);
        let token = media_url_key(key, &sha).unwrap();

        // deterministic + 64 lowercase hex (HMAC-SHA256 = 32 bytes)
        assert_eq!(token, media_url_key(key, &sha).unwrap());
        assert!(is_sha256_hex(&token));

        // a different SERVER KEY → different token (so you can't precompute it
        // for a known content sha without the secret)
        assert_ne!(token, media_url_key(b"a-different-server-secret", &sha).unwrap());
        // different CONTENT → different token
        assert_ne!(token, media_url_key(key, &"b".repeat(64)).unwrap());
    }

    #[test]
    fn is_sha256_hex_rejects_garbage_and_traversal() {
        assert!(is_sha256_hex(&"a1".repeat(32))); // 64 lowercase hex
        assert!(!is_sha256_hex("xyz")); // too short
        assert!(!is_sha256_hex(&"A".repeat(64))); // uppercase not allowed
        assert!(!is_sha256_hex(&"g".repeat(64))); // non-hex letter
        assert!(!is_sha256_hex("../../etc/passwd")); // path traversal
    }
}
