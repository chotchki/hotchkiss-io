//! Content-addressed disk store for large media (video, big STLs) — files keyed
//! by SHA-256 under the configured `Settings.media_paths` roots (uploads fill
//! in order across drives), sharded so one directory never holds thousands of
//! entries. Kept OUT of SQLite so the daily backup + prod→beta snapshot don't
//! copy gigabytes (only the small metadata rows live in the DB; Backblaze covers
//! the drives at the filesystem level).
//! Content-addressed buys three things: identical bytes dedupe, the hash IS the
//! cache key, and a stored file is immutable — safe to serve with a far-future
//! cache header and a `206` range response.

pub mod poster;
pub mod probe;

use anyhow::{anyhow, bail, Context, Result};
use openssl::sha::sha256;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Debug)]
pub struct MediaStore {
    /// Ordered media roots (primary first). Reads check each for the SHA; writes
    /// fill in order — the first root with headroom.
    roots: Vec<PathBuf>,
    /// Don't write to a root with less than this much free space — fall to the
    /// next instead (the upload size isn't known up front when streaming).
    min_free_bytes: u64,
}

impl MediaStore {
    pub fn new(roots: Vec<PathBuf>, min_free_bytes: u64) -> Self {
        assert!(!roots.is_empty(), "MediaStore needs at least one root");
        Self {
            roots,
            min_free_bytes,
        }
    }

    /// `<root>/ab/cd/<full-64-hex>` for a GIVEN root — two levels of one-byte
    /// sharding keep any single directory small. Callers MUST pass a validated hash
    /// (see [`is_sha256_hex`]); the serve route validates the URL segment, and the
    /// store paths always hash internally.
    fn shard_path(root: &Path, sha_hex: &str) -> PathBuf {
        root.join(&sha_hex[0..2])
            .join(&sha_hex[2..4])
            .join(sha_hex)
    }

    /// Resolve the on-disk path for a stored SHA across the configured roots: try
    /// the `hint` root first (the recorded `storage_root` → O(1), no scan), then
    /// first-found across all roots (self-healing if the file moved or the hint is
    /// stale). `None` if no mounted root holds it.
    pub fn resolve_path(&self, sha_hex: &str, hint: Option<&str>) -> Option<PathBuf> {
        if let Some(h) = hint {
            let p = Self::shard_path(Path::new(h), sha_hex);
            if p.is_file() {
                return Some(p);
            }
        }
        self.roots
            .iter()
            .map(|r| Self::shard_path(r, sha_hex))
            .find(|p| p.is_file())
    }

    /// The root currently holding `sha_hex`, if any — for the `storage_root` hint
    /// on a dedup hit. Scan order = config order.
    fn root_containing(&self, sha_hex: &str) -> Option<&Path> {
        self.roots
            .iter()
            .find(|r| Self::shard_path(r, sha_hex).is_file())
            .map(|r| r.as_path())
    }

    #[allow(dead_code)] // store API; used by the BZ.8 orphan sweep
    pub fn exists(&self, sha_hex: &str) -> bool {
        self.resolve_path(sha_hex, None).is_some()
    }

    /// Probe a configured root WITHOUT creating anything. "Present" iff the root
    /// directory already exists OR its parent does — a cleanly-unmounted external
    /// volume is NEITHER (macOS removes the `/Volumes/<name>` entry on eject), so we
    /// never `create_dir_all` a phantom mountpoint chain onto the boot disk (the M1
    /// bug). Free/total are measured on the nearest existing path — the root, else
    /// its parent (the same volume the leaf would be created on). `(present, free,
    /// total)`.
    ///
    /// Residual: an UNCLEAN unmount can leave a stray `/Volumes/<name>` dir on boot,
    /// which reads as present → writes land on boot until the free-space margin trips
    /// and falls through. So configure a SUBDIR under each volume (not the mount
    /// root) and keep the margin sane — the margin is the backstop against filling
    /// the boot disk.
    fn probe_root(root: &Path) -> (bool, Option<u64>, Option<u64>) {
        let target = if root.exists() {
            Some(root.to_path_buf())
        } else {
            root.parent().filter(|p| p.exists()).map(Path::to_path_buf)
        };
        match target {
            Some(t) => (
                true,
                fs4::available_space(&t).ok(),
                fs4::total_space(&t).ok(),
            ),
            None => (false, None, None),
        }
    }

    /// Pick the write root: the first PRESENT root (its dir or parent exists — i.e.
    /// the drive is mounted) with more than `min_free_bytes` free (fill-in-order, so
    /// capacity spans drives). A not-present root (unmounted) or a per-root stat
    /// failure is SKIPPED — uploads fall through to the next healthy root rather than
    /// aborting (the L1 fix). The leaf dir is created downstream by the shard/staging
    /// write, only ever under an existing parent — never here, and never a phantom
    /// mountpoint. Errors only if NO root is ready with headroom.
    fn pick_write_root(&self) -> Result<PathBuf> {
        let mut mounted = 0;
        for root in &self.roots {
            let (present, free, _) = Self::probe_root(root);
            if !present {
                tracing::debug!("media root {root:?} not present (drive unmounted?) — skipping");
                continue;
            }
            mounted += 1;
            match free {
                Some(f) if f > self.min_free_bytes => return Ok(root.clone()),
                Some(_) => tracing::debug!(
                    "media root {root:?} below the {}-byte free margin — falling through",
                    self.min_free_bytes
                ),
                None => tracing::error!("media root {root:?} present but un-statted — skipping"),
            }
        }
        bail!(
            "no media root ready with > {} bytes free ({} configured, {mounted} mounted)",
            self.min_free_bytes,
            self.roots.len(),
        );
    }

    /// Store small IN-MEMORY bytes (e.g. a generated poster) content-addressed.
    /// Dedups across ALL roots; otherwise writes to the picked write root via an
    /// atomic temp+rename within it. Returns `(sha_hex, root)` — `root` is the
    /// `storage_root` hint. Sync fs IO — wrap large writes in `spawn_blocking`.
    pub fn store(&self, bytes: &[u8]) -> Result<(String, PathBuf)> {
        let sha_hex = hex_sha256(bytes);
        if let Some(root) = self.root_containing(&sha_hex) {
            return Ok((sha_hex, root.to_path_buf())); // dedupe (possibly another root)
        }
        let root = self.pick_write_root()?;
        let dest = Self::shard_path(&root, &sha_hex);
        let dir = dest.parent().expect("shard_path always has a parent");
        fs::create_dir_all(dir).with_context(|| format!("create media shard dir {dir:?}"))?;
        // temp + atomic rename WITHIN the chosen root (same filesystem) so a reader
        // never observes a partial file.
        let tmp = dir.join(format!(".tmp-{sha_hex}"));
        fs::write(&tmp, bytes).with_context(|| format!("write temp media file {tmp:?}"))?;
        fs::rename(&tmp, &dest).with_context(|| format!("rename media into place {dest:?}"))?;
        Ok((sha_hex, root))
    }

    /// Begin a STREAMING store — for uploads too large to hold in memory. Picks the
    /// write root UP FRONT (by free space) and stages a temp under that root's
    /// `.staging/`, so the commit rename stays intra-volume (a cross-volume rename
    /// is `EXDEV`). Hashes incrementally; [`StagedBlob::commit`] finalizes the SHA
    /// and renames into the slot (or dedupes across all roots). O(chunk) memory.
    pub async fn stage(&self) -> Result<StagedBlob> {
        // pick_write_root does blocking fs (create_dir_all + statvfs per root) —
        // keep it off the async runtime so an asleep drive can't pin a worker.
        let this = self.clone();
        let write_root = tokio::task::spawn_blocking(move || this.pick_write_root())
            .await
            .map_err(|e| anyhow!("pick_write_root task panicked: {e}"))??;
        let staging = write_root.join(".staging");
        tokio::fs::create_dir_all(&staging)
            .await
            .with_context(|| format!("create media staging dir {staging:?}"))?;
        // Random temp name — the SHA isn't known until the stream ends.
        let tmp = staging.join(format!("up-{}", uuid::Uuid::now_v7().simple()));
        let file = tokio::fs::File::create(&tmp)
            .await
            .with_context(|| format!("create temp media file {tmp:?}"))?;
        Ok(StagedBlob {
            write_root,
            tmp,
            file: Some(file),
            hasher: Sha256::new(),
            len: 0,
            committed: false,
        })
    }

    /// Report each configured root + its free space (for the admin storage panel —
    /// so multi-drive placement isn't silent). Uses the SAME `probe_root` the writer
    /// does, so the panel and `pick_write_root` always agree: a not-present root (an
    /// unmounted external — neither it nor its parent exists) reports
    /// `free_bytes: None` and is never the write target; the write target is the
    /// first present root with free space above the margin — exactly what an upload
    /// would pick. Creates nothing.
    pub fn roots_status(&self) -> Vec<RootStatus> {
        let mut write_target_taken = false;
        self.roots
            .iter()
            .map(|root| {
                let (present, free, total) = Self::probe_root(root);
                let below_margin = present && free.is_some_and(|f| f <= self.min_free_bytes);
                let is_write_target = !write_target_taken
                    && present
                    && free.is_some_and(|f| f > self.min_free_bytes);
                if is_write_target {
                    write_target_taken = true;
                }
                RootStatus {
                    path: root.clone(),
                    free_bytes: free,
                    total_bytes: total,
                    is_write_target,
                    below_margin,
                }
            })
            .collect()
    }
}

/// One row of [`MediaStore::roots_status`] — a configured root + its free/total
/// space and role. `free_bytes`/`total_bytes` are `None` when the root can't be
/// statted (missing or unmounted).
pub struct RootStatus {
    pub path: PathBuf,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub is_write_target: bool,
    pub below_margin: bool,
}

/// An in-progress streaming write into the [`MediaStore`]. Feed chunks with
/// [`write_chunk`](Self::write_chunk), then [`commit`](Self::commit). If it's
/// dropped WITHOUT committing (a failed/aborted upload), the temp file is
/// best-effort removed — a partial transfer never lingers in `.staging` or
/// half-populates the content-addressed store.
pub struct StagedBlob {
    /// The root picked at stage time — the temp lives under its `.staging/`, and
    /// the commit rename targets a shard under it (intra-volume, atomic).
    write_root: PathBuf,
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

    /// Finalize the SHA-256, fsync, then atomically rename the temp into the slot
    /// on the write root — or, if identical content is already on ANY root, drop the
    /// temp (dedupe). Returns `(sha_hex, total_bytes, root)`, where `root` is where
    /// the bytes live (the write root, or the existing root on a dedup hit) — the
    /// `storage_root` hint.
    pub async fn commit(mut self, store: &MediaStore) -> Result<(String, u64, PathBuf)> {
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
        // Dedupe across ALL roots — identical content shouldn't double up on a
        // second drive.
        if let Some(root) = store.root_containing(&sha_hex) {
            let root = root.to_path_buf();
            let _ = tokio::fs::remove_file(&self.tmp).await;
            self.committed = true;
            return Ok((sha_hex, self.len, root));
        }
        let dest = MediaStore::shard_path(&self.write_root, &sha_hex);
        let dir = dest.parent().expect("shard_path always has a parent");
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("create media shard dir {dir:?}"))?;
        tokio::fs::rename(&self.tmp, &dest)
            .await
            .with_context(|| format!("rename media into place {dest:?}"))?;
        self.committed = true;
        Ok((sha_hex, self.len, self.write_root.clone()))
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
/// `../` or a short slice past the shard-path indexing.
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
        let store = MediaStore::new(vec![dir.path().to_path_buf()], 0);

        let (sha, _root) = store.store(b"hello video").unwrap();
        assert_eq!(sha.len(), 64);
        assert!(is_sha256_hex(&sha));
        assert!(store.exists(&sha));

        // sharded path: <root>/<2>/<2>/<full hash>, holding the exact bytes
        let p = store.resolve_path(&sha, None).unwrap();
        assert!(p.is_file());
        assert!(p.ends_with(&sha));
        assert_eq!(p.parent().unwrap().file_name().unwrap(), &sha[2..4]);
        assert_eq!(fs::read(&p).unwrap(), b"hello video");

        // idempotent: same content → same hash, no second file, no temp left over
        let (sha_again, _) = store.store(b"hello video").unwrap();
        assert_eq!(sha, sha_again);

        // different content → different hash
        assert_ne!(sha, store.store(b"different bytes").unwrap().0);
    }

    #[tokio::test]
    async fn store_stream_hashes_dedupes_and_cleans_up() {
        let dir = tempdir().unwrap();
        let store = MediaStore::new(vec![dir.path().to_path_buf()], 0);
        let staging = dir.path().join(".staging");

        // streamed in chunks → one content-addressed file holding the exact bytes
        let mut staged = store.stage().await.unwrap();
        staged.write_chunk(b"hello ").await.unwrap();
        staged.write_chunk(b"world").await.unwrap();
        let (sha, len, _root) = staged.commit(&store).await.unwrap();

        assert_eq!(len, 11);
        // CRITICAL: the streaming sha2 digest equals the openssl one-shot, so
        // content-addressed dedup is consistent across `store` and `stage`.
        assert_eq!(sha, hex_sha256(b"hello world"));
        assert!(store.exists(&sha));
        assert_eq!(
            fs::read(store.resolve_path(&sha, None).unwrap()).unwrap(),
            b"hello world"
        );

        // dedupe: the same content streamed again → same sha, no error
        let mut again = store.stage().await.unwrap();
        again.write_chunk(b"hello world").await.unwrap();
        let (sha_again, _, _) = again.commit(&store).await.unwrap();
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

    #[tokio::test]
    async fn multi_root_resolve_hint_scan_dedup_and_full() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let roots = vec![a.path().to_path_buf(), b.path().to_path_buf()];

        // min_free 0 → writes fill the FIRST root; resolve finds it there.
        let store = MediaStore::new(roots.clone(), 0);
        let (sha_a, root_a) = store.store(b"primary bytes").unwrap();
        assert_eq!(root_a.as_path(), a.path(), "first write lands on the primary");
        assert!(store.resolve_path(&sha_a, None).unwrap().starts_with(a.path()));

        // Content living on the SECOND root (placed via a b-only store).
        let only_b = MediaStore::new(vec![b.path().to_path_buf()], 0);
        let (sha_b, _) = only_b.store(b"secondary bytes").unwrap();
        // resolved by SCAN (no hint) ...
        assert!(store.resolve_path(&sha_b, None).unwrap().starts_with(b.path()));
        // ... by correct HINT (O(1)) ...
        assert!(store
            .resolve_path(&sha_b, Some(b.path().to_str().unwrap()))
            .unwrap()
            .starts_with(b.path()));
        // ... and a STALE hint (points at a, where it isn't) falls back to scan.
        assert!(store
            .resolve_path(&sha_b, Some(a.path().to_str().unwrap()))
            .unwrap()
            .starts_with(b.path()));

        // dedup ACROSS roots: streaming bytes already on b returns b as the hint and
        // does NOT duplicate onto a.
        let mut staged = store.stage().await.unwrap();
        staged.write_chunk(b"secondary bytes").await.unwrap();
        let (sha2, _len, hint_root) = staged.commit(&store).await.unwrap();
        assert_eq!(sha2, sha_b);
        assert_eq!(hint_root.as_path(), b.path(), "deduped to the existing root");
        assert!(!MediaStore::shard_path(a.path(), &sha_b).is_file());

        // every root below the headroom margin → a clear error, not a silent misfile
        let full = MediaStore::new(roots, u64::MAX);
        assert!(full.store(b"nope").is_err());
        assert!(full.stage().await.is_err());
    }

    #[test]
    fn roots_status_reports_free_space_unavailable_and_write_target() {
        let a = tempdir().unwrap();
        // Simulate a cleanly-unmounted external root: BOTH the root and its parent
        // (the mount point) are absent, so probe_root reports it not-present.
        let missing = a.path().join("ghost-volume").join("media");
        // root[0] exists (real free space); root[1] is unmounted.
        let store = MediaStore::new(vec![a.path().to_path_buf(), missing], 0);
        let status = store.roots_status();
        assert_eq!(status.len(), 2);

        // existing root: free reported, it's the write target (margin 0), not full
        assert!(status[0].free_bytes.is_some());
        assert!(status[0].is_write_target);
        assert!(!status[0].below_margin);

        // missing/unmounted root: unavailable, never a write target
        assert!(status[1].free_bytes.is_none());
        assert!(!status[1].is_write_target);

        // an impossibly-high margin → the existing root reads as full + no write target
        let full = MediaStore::new(vec![a.path().to_path_buf()], u64::MAX);
        let s = full.roots_status();
        assert!(s[0].below_margin);
        assert!(!s[0].is_write_target);
    }

    #[test]
    fn pick_write_root_skips_unmounted_and_never_materializes_a_phantom_root() {
        let base = tempdir().unwrap();
        // root[0] = an "unmounted external": the mount point (parent) is absent too,
        // so the store must NEITHER pick it NOR create it on the host fs (M1).
        let unmounted = base.path().join("Volumes-Ext").join("media");
        // root[1] = a not-yet-created LOCAL dir whose PARENT exists (the default
        // bootstrap case) — present, so it's the fall-through target and is created
        // on first write.
        let local_parent = base.path().join("app-support");
        fs::create_dir_all(&local_parent).unwrap();
        let local = local_parent.join("media");

        let store = MediaStore::new(vec![unmounted.clone(), local.clone()], 0);

        // L1 + M1: falls through the unmounted root to the local one...
        let (_sha, root) = store.store(b"bytes").unwrap();
        assert_eq!(root.as_path(), local.as_path(), "fell through unmounted → local");
        // ...and NEVER materialized the unmounted mount point on the host fs.
        assert!(
            !base.path().join("Volumes-Ext").exists(),
            "an unmounted root must not be created on the host filesystem"
        );
        assert!(local.exists(), "the local fall-through root is auto-created on write");

        // roots_status agrees with the writer: unmounted unavailable, local writes.
        let st = store.roots_status();
        assert!(!st[0].is_write_target && st[0].free_bytes.is_none());
        assert!(st[1].is_write_target && st[1].free_bytes.is_some());
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
