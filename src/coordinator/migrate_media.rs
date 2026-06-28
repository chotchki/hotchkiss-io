//! One-shot startup migration (Phase BZ.8): copy SQLite-BLOB attachments into the
//! content-addressed disk store (SHA-256) + `media` rows, rewrite page markdown
//! `/attachments/…` → `/media/<ref>`, and re-home page covers. Idempotent (skips
//! attachments already migrated). Fires a DB backup FIRST — it rewrites real
//! content — and DEFERS (no mutation) if that backup can't be taken. NON-
//! destructive: the `attachments` table + the `/attachments` route stay until
//! Stage 2 retires them, after beta verifies every image still renders.

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::path::Path;
use tracing::{info, warn};

use crate::coordinator::backup::run_backup;
use crate::db::dao::crypto_key::CryptoKey;
use crate::db::dao::media::{MediaDao, MediaKind, MediaVariantDao};
use crate::media::{media_url_key, MediaStore};

/// CryptoKey row id for the media-URL HMAC secret (matches the admin handler).
const MEDIA_HMAC_KEY_ID: i64 = 2;

pub async fn migrate_attachments_to_media(
    pool: &SqlitePool,
    store: &MediaStore,
    backup_dir: &Path,
) -> Result<()> {
    let pending: i64 =
        sqlx::query_scalar!(r#"SELECT COUNT(*) FROM attachments WHERE migrated_media_id IS NULL"#)
            .fetch_one(pool)
            .await?;
    if pending == 0 {
        return Ok(());
    }
    info!("BZ.8: {pending} attachment(s) to migrate into the media store");

    // SAFETY: snapshot the DB before rewriting content. If it fails, DEFER — don't
    // mutate without a restore point (the migration retries next boot).
    match run_backup(pool, backup_dir).await {
        Ok(p) => info!("BZ.8: pre-migration backup written to {}", p.display()),
        Err(e) => {
            warn!("BZ.8: pre-migration backup FAILED — deferring migration to next boot: {e:?}");
            return Ok(());
        }
    }

    let hmac_key = CryptoKey::get_or_create(pool, MEDIA_HMAC_KEY_ID)
        .await?
        .key_value;

    // 1. Copy each un-migrated attachment → store (SHA-256) + media row + variant.
    let attachments = sqlx::query!(
        r#"SELECT attachment_id as "attachment_id!", attachment_name, mime_type, attachment_content
           FROM attachments WHERE migrated_media_id IS NULL"#
    )
    .fetch_all(pool)
    .await?;

    for a in &attachments {
        let kind = attachment_kind(&a.attachment_name, &a.mime_type);
        let sha = store
            .store(&a.attachment_content)
            .context("storing attachment blob into the media store")?;
        let media = MediaDao::create(
            pool,
            uuid::Uuid::now_v7().simple().to_string(),
            kind,
            Some(strip_ext(&a.attachment_name)),
            None,
            None,
            None,
        )
        .await?;
        let url_key = media_url_key(&hmac_key, &sha)?;
        MediaVariantDao::create(
            pool,
            media.media_id,
            sha,
            url_key,
            a.mime_type.clone(),
            None,
            a.attachment_content.len() as i64,
        )
        .await?;
        sqlx::query!(
            r#"UPDATE attachments SET migrated_media_id = ?1 WHERE attachment_id = ?2"#,
            media.media_id,
            a.attachment_id
        )
        .execute(pool)
        .await?;
    }

    // 2. The full attachment → media map (every attachment is migrated now).
    let map = sqlx::query!(
        r#"SELECT a.attachment_id as "attachment_id!", a.page_id, a.attachment_name,
                  m.media_id as "media_id!", m.media_ref
           FROM attachments a JOIN media m ON m.media_id = a.migrated_media_id"#
    )
    .fetch_all(pool)
    .await?;

    // 3. Rewrite page markdown (both URL forms) + re-home covers.
    let pages = sqlx::query!(
        r#"SELECT page_id as "page_id!", page_markdown, page_cover_attachment_id, page_cover_media_id
           FROM content_pages"#
    )
    .fetch_all(pool)
    .await?;

    let mut rewritten = 0;
    let mut covers = 0;
    for p in &pages {
        let mut md = p.page_markdown.clone();
        for r in &map {
            let media_url = format!("/media/{}", r.media_ref);
            md = md.replace(
                &format!("/attachments/{}/{}", r.page_id, r.attachment_name),
                &media_url,
            );
            md = md.replace(&format!("/attachments/id/{}", r.attachment_id), &media_url);
        }
        if md != p.page_markdown {
            sqlx::query!(
                r#"UPDATE content_pages SET page_markdown = ?1 WHERE page_id = ?2"#,
                md,
                p.page_id
            )
            .execute(pool)
            .await?;
            rewritten += 1;
        }
        // Re-home the cover once (page_cover_attachment_id → page_cover_media_id).
        if p.page_cover_media_id.is_none()
            && let Some(att_id) = p.page_cover_attachment_id
            && let Some(r) = map.iter().find(|r| r.attachment_id == att_id)
        {
            sqlx::query!(
                r#"UPDATE content_pages SET page_cover_media_id = ?1 WHERE page_id = ?2"#,
                r.media_id,
                p.page_id
            )
            .execute(pool)
            .await?;
            covers += 1;
        }
    }

    info!(
        "BZ.8: migrated {} attachment(s); rewrote {rewritten} page(s); re-homed {covers} cover(s)",
        attachments.len()
    );
    Ok(())
}

/// Attachment kind from its name + declared mime (STL by extension; ffprobe isn't
/// run here — dims/duration stay null, which the embed renders fine).
fn attachment_kind(name: &str, mime: &str) -> MediaKind {
    if name.to_ascii_lowercase().ends_with(".stl") {
        MediaKind::Stl
    } else if mime.starts_with("image/") {
        MediaKind::Image
    } else if mime.starts_with("video/") {
        MediaKind::Video
    } else {
        MediaKind::File
    }
}

/// Filename minus its extension → a human title.
fn strip_ext(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::dao::attachments::AttachmentDao;
    use crate::db::dao::content_pages::ContentPageDao;
    use tempfile::tempdir;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn migrates_rewrites_rehomes_and_is_idempotent(pool: sqlx::SqlitePool) -> Result<()> {
        // A page whose markdown references one attachment by BOTH URL forms, with
        // that attachment also set as the page cover.
        let mut page =
            ContentPageDao::create(&pool, None, "test".to_string(), None, String::new(), None)
                .await?;
        let attach = AttachmentDao::create(
            &pool,
            page.page_id,
            "cat.jpg".to_string(),
            "image/jpeg".to_string(),
            b"cat-bytes".to_vec(),
        )
        .await?;
        page.page_markdown = format!(
            "see ![a](/attachments/{}/cat.jpg) and ![b](/attachments/id/{})",
            page.page_id, attach.attachment_id
        );
        page.page_cover_attachment_id = Some(attach.attachment_id);
        page.update(&pool).await?;

        let dir = tempdir()?;
        let store = MediaStore::new(dir.path().join("media"));
        let backups = dir.path().join("backups");
        migrate_attachments_to_media(&pool, &store, &backups).await?;

        // A pre-migration backup was written.
        assert!(
            std::fs::read_dir(&backups)?.next().is_some(),
            "a backup snapshot should exist"
        );

        // The attachment is marked migrated → one media item + variant.
        let media_id: i64 = sqlx::query_scalar!(
            r#"SELECT migrated_media_id FROM attachments WHERE attachment_id = ?1"#,
            attach.attachment_id
        )
        .fetch_one(&pool)
        .await?
        .expect("attachment marked migrated");
        let media_ref: String =
            sqlx::query_scalar!(r#"SELECT media_ref FROM media WHERE media_id = ?1"#, media_id)
                .fetch_one(&pool)
                .await?;
        let variants = MediaVariantDao::find_by_media_id(&pool, media_id).await?;
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].mime, "image/jpeg");
        assert!(store.exists(&variants[0].sha256), "blob is in the store");

        // Both URL forms rewrote to /media/<ref>; no /attachments/ left.
        let md: String = sqlx::query_scalar!(
            r#"SELECT page_markdown FROM content_pages WHERE page_id = ?1"#,
            page.page_id
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            md,
            format!("see ![a](/media/{media_ref}) and ![b](/media/{media_ref})")
        );

        // Cover re-homed to the media id.
        let cover: Option<i64> = sqlx::query_scalar!(
            r#"SELECT page_cover_media_id FROM content_pages WHERE page_id = ?1"#,
            page.page_id
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(cover, Some(media_id));

        // Idempotent: a second run creates no new media.
        migrate_attachments_to_media(&pool, &store, &backups).await?;
        let count: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) FROM media"#)
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 1, "re-run must not duplicate media");

        Ok(())
    }
}
