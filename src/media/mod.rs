//! Content-addressed disk store for large media (video, big STLs) — files keyed
//! by SHA-256 under `Settings.media_path`, sharded so one directory never holds
//! thousands of entries. Kept OUT of SQLite so the daily backup + prod→beta
//! snapshot don't copy gigabytes (only the small metadata rows live in the DB).
//! Content-addressed buys three things: identical bytes dedupe, the hash IS the
//! cache key, and a stored file is immutable — safe to serve with a far-future
//! cache header and a `206` range response.

pub mod poster;
pub mod probe;

use anyhow::{Context, Result};
use openssl::sha::sha256;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

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

    #[allow(dead_code)] // store API; used by the BZ.8 orphan sweep
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

    /// Begin a STREAMING store — for uploads too large to hold in memory. Bytes
    /// are written chunk-by-chunk to a temp file under `<root>/.staging/` (same
    /// filesystem as the shards, so the final move is an atomic rename) and hashed
    /// incrementally; [`StagedBlob::commit`] finalizes the SHA-256 and renames into
    /// the content-addressed slot (or dedupes if already present). Memory stays
    /// O(chunk) regardless of file size — the fix for the old `store(&[u8])` path
    /// that buffered the whole upload in RAM.
    pub async fn stage(&self) -> Result<StagedBlob> {
        let staging = self.root.join(".staging");
        tokio::fs::create_dir_all(&staging)
            .await
            .with_context(|| format!("create media staging dir {staging:?}"))?;
        // Random temp name — the SHA isn't known until the stream ends.
        let tmp = staging.join(format!("up-{}", uuid::Uuid::now_v7().simple()));
        let file = tokio::fs::File::create(&tmp)
            .await
            .with_context(|| format!("create temp media file {tmp:?}"))?;
        Ok(StagedBlob {
            tmp,
            file: Some(file),
            hasher: Sha256::new(),
            len: 0,
            committed: false,
        })
    }
}

/// An in-progress streaming write into the [`MediaStore`]. Feed chunks with
/// [`write_chunk`](Self::write_chunk), then [`commit`](Self::commit). If it's
/// dropped WITHOUT committing (a failed/aborted upload), the temp file is
/// best-effort removed — a partial transfer never lingers in `.staging` or
/// half-populates the content-addressed store.
pub struct StagedBlob {
    tmp: PathBuf,
    file: Option<tokio::fs::File>,
    hasher: Sha256,
    len: u64,
    committed: bool,
}

impl StagedBlob {
    /// Append one chunk: hash it + write it through. O(chunk) memory.
    pub async fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        let file = self.file.as_mut().expect("write_chunk after commit");
        self.hasher.update(chunk);
        file.write_all(chunk)
            .await
            .with_context(|| format!("write temp media file {:?}", self.tmp))?;
        self.len += chunk.len() as u64;
        Ok(())
    }

    /// True until the first byte is written — lets a caller skip an empty file part
    /// without committing a (zero-byte) blob.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Finalize the SHA-256, fsync, then atomically rename the temp into the
    /// content-addressed slot — or, if identical content is already stored, drop
    /// the temp (dedupe). Returns `(sha_hex, total_bytes)`.
    pub async fn commit(mut self, store: &MediaStore) -> Result<(String, u64)> {
        if let Some(mut file) = self.file.take() {
            file.flush().await.context("flush temp media file")?;
            file.sync_all().await.context("fsync temp media file")?;
            // `file` drops here → the fd is closed before the rename.
        }
        let sha_hex: String = self
            .hasher
            .finalize_reset()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let dest = store.path_for(&sha_hex);
        if dest.is_file() {
            // identical content already present → dedupe, drop the temp.
            let _ = tokio::fs::remove_file(&self.tmp).await;
        } else {
            let dir = dest.parent().expect("path_for always has a parent");
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("create media shard dir {dir:?}"))?;
            tokio::fs::rename(&self.tmp, &dest)
                .await
                .with_context(|| format!("rename media into place {dest:?}"))?;
        }
        self.committed = true;
        Ok((sha_hex, self.len))
    }
}

impl Drop for StagedBlob {
    fn drop(&mut self) {
        if !self.committed {
            // Best-effort: an aborted/failed upload leaves nothing behind. Sync fs
            // here (Drop can't be async) — the temp is tiny to unlink.
            let _ = std::fs::remove_file(&self.tmp);
        }
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

    #[tokio::test]
    async fn store_stream_hashes_dedupes_and_cleans_up() {
        let dir = tempdir().unwrap();
        let store = MediaStore::new(dir.path().to_path_buf());
        let staging = dir.path().join(".staging");

        // streamed in chunks → one content-addressed file holding the exact bytes
        let mut staged = store.stage().await.unwrap();
        staged.write_chunk(b"hello ").await.unwrap();
        staged.write_chunk(b"world").await.unwrap();
        let (sha, len) = staged.commit(&store).await.unwrap();

        assert_eq!(len, 11);
        // CRITICAL: the streaming sha2 digest equals the openssl one-shot, so
        // content-addressed dedup is consistent across `store` and `stage`.
        assert_eq!(sha, hex_sha256(b"hello world"));
        assert!(store.exists(&sha));
        assert_eq!(fs::read(store.path_for(&sha)).unwrap(), b"hello world");

        // dedupe: the same content streamed again → same sha, no error
        let mut again = store.stage().await.unwrap();
        again.write_chunk(b"hello world").await.unwrap();
        let (sha_again, _) = again.commit(&store).await.unwrap();
        assert_eq!(sha, sha_again);

        // committed temps leave nothing in .staging
        let leftover: Vec<_> = fs::read_dir(&staging).unwrap().filter_map(|e| e.ok()).collect();
        assert!(leftover.is_empty(), "staging not clean after commit: {leftover:?}");

        // abort: dropping without commit removes the temp
        {
            let mut abandoned = store.stage().await.unwrap();
            abandoned.write_chunk(b"abandoned upload").await.unwrap();
            // dropped here without commit
        }
        let after_abort: Vec<_> = fs::read_dir(&staging).unwrap().filter_map(|e| e.ok()).collect();
        assert!(after_abort.is_empty(), "aborted temp not cleaned: {after_abort:?}");
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
