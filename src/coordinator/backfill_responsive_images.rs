//! One-shot startup backfill (Phase CN): generate the width-stepped AVIF variants
//! for image media that was uploaded BEFORE the responsive pipeline existed, and
//! stamp the original variant's width so it joins the srcset.
//!
//! Purely ADDITIVE — a legacy image already serves its full-resolution original
//! via the render's single-`src` fallback — so this runs DETACHED in the
//! background after boot. It is never part of the coordinator's `try_join!`, so a
//! failure can't take the app down, and it does NOT delay serving. Idempotent: an
//! image that already has a width-carrying variant is skipped, so a restart
//! mid-run resumes cleanly and steady-state boots are a single cheap query.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use sqlx::SqlitePool;

use crate::coordinator::backup::run_backup;
use crate::db::dao::crypto_key::CryptoKey;
use crate::db::dao::media::{MediaDao, MediaKind, MediaVariantDao};
use crate::media::resize::{responsive_avif_variants, ResizeResult};
use crate::media::{media_url_key, MediaStore};
use crate::settings::Settings;

/// `crypto_keys` id 2 — the media-URL HMAC key (same one the upload path uses).
const MEDIA_HMAC_KEY_ID: i64 = 2;

/// Spawn the backfill as a detached background task. It logs its own outcome and
/// never bubbles — the caller (the coordinator) does not await it.
pub fn spawn(pool: SqlitePool, settings: Arc<Settings>) {
    tokio::spawn(async move {
        if let Err(e) = run(&pool, &settings).await {
            tracing::error!("responsive-image backfill aborted: {e:?}");
        }
    });
}

async fn run(pool: &SqlitePool, settings: &Settings) -> Result<()> {
    // An image needs backfill if NO image variant carries a width yet (a new
    // upload arrives complete, so we only ever touch the legacy backlog).
    let mut todo = Vec::new();
    for m in MediaDao::find_all(pool).await? {
        if m.kind().map(|k| k == MediaKind::Image).unwrap_or(false) {
            let variants = MediaVariantDao::find_by_media_id(pool, m.media_id).await?;
            let needs = variants.iter().any(|v| v.mime.starts_with("image/"))
                && !variants.iter().any(|v| v.width.is_some());
            if needs {
                todo.push(m);
            }
        }
    }
    if todo.is_empty() {
        tracing::info!("responsive-image backfill: nothing to do");
        return Ok(());
    }
    tracing::info!("responsive-image backfill: {} image(s) to process", todo.len());

    // Back the DB up FIRST (mirrors the retired media migration): a backup failure
    // DEFERS the run rather than risk an un-backed-up mutation. Only reached when
    // there's real work, so steady-state boots never trigger a backup.
    run_backup(pool, &settings.backup_path)
        .await
        .map_err(|e| anyhow!("pre-backfill backup failed, deferring: {e}"))?;

    let store = MediaStore::new(settings.media_paths.clone(), settings.media_min_free_bytes);
    let hmac_key = CryptoKey::get_or_create(pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;

    let (mut ok, mut failed) = (0u32, 0u32);
    for m in todo {
        match backfill_one(pool, &store, &hmac_key, &m).await {
            Ok(n) => {
                ok += 1;
                tracing::debug!("backfilled {n} variant(s) for media {}", m.media_id);
            }
            Err(e) => {
                failed += 1;
                tracing::warn!("backfill failed for media {}: {e:?}", m.media_id);
            }
        }
    }
    tracing::info!("responsive-image backfill done: {ok} processed, {failed} failed");
    Ok(())
}

/// Backfill one image: stamp the original variant's width + add the downscaled
/// AVIF variants. Returns how many resized variants were added.
async fn backfill_one(
    pool: &SqlitePool,
    store: &MediaStore,
    hmac_key: &[u8],
    m: &MediaDao,
) -> Result<usize> {
    let variants = MediaVariantDao::find_by_media_id(pool, m.media_id).await?;
    let Some(original) = variants.iter().find(|v| v.mime.starts_with("image/")) else {
        return Ok(0); // no image bytes to resize
    };

    let store_clone = store.clone();
    let sha = original.sha256.clone();
    let edit = m.meta().edit.unwrap_or_default();
    let resized = tokio::task::spawn_blocking(move || -> Result<ResizeResult> {
        let path = store_clone
            .resolve_path(&sha, None)
            .ok_or_else(|| anyhow!("source bytes not found in any media root"))?;
        responsive_avif_variants(&path, &edit)
    })
    .await
    .map_err(|e| anyhow!("resize task panicked: {e}"))??;

    // Stamp the original with the TRUE decoded dims so it's the largest srcset
    // entry (+ the no-srcset src). The `media.width` column is NULL for
    // attachment-migrated covers — reading it was exactly what skipped them.
    MediaVariantDao::set_dimensions(
        pool,
        original.variant_id,
        Some(resized.source_width as i64),
        Some(resized.source_height as i64),
    )
    .await?;

    let mut added = 0;
    for r in resized.variants {
        let store_clone = store.clone();
        let bytes = r.avif;
        let len = bytes.len() as i64;
        let (sha, root) = tokio::task::spawn_blocking(move || store_clone.store(&bytes))
            .await
            .map_err(|e| anyhow!("store task panicked: {e}"))??;
        let url_key = media_url_key(hmac_key, &sha)?;
        MediaVariantDao::create(
            pool,
            m.media_id,
            sha,
            url_key,
            "image/avif".to_string(),
            None,
            len,
            Some(root.to_string_lossy().into_owned()),
            Some(r.width as i64),
            Some(r.height as i64),
        )
        .await?;
        added += 1;
    }
    Ok(added)
}
