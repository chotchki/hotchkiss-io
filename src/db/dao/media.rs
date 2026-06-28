use anyhow::{anyhow, Result};
use sqlx::{prelude::FromRow, query, query_as, SqliteExecutor};

/// What a media item IS — drives the render-time dispatch (image → `<img>`,
/// video → `<video>` multi-source, stl → `<object class="stl-view">`, file → a
/// download link). Stored as TEXT; constructed typed, read back via [`MediaDao::kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Stl,
    File,
}

impl MediaKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Stl => "stl",
            MediaKind::File => "file",
        }
    }

    pub fn parse(s: &str) -> Result<MediaKind> {
        match s {
            "image" => Ok(MediaKind::Image),
            "video" => Ok(MediaKind::Video),
            "stl" => Ok(MediaKind::Stl),
            "file" => Ok(MediaKind::File),
            other => Err(anyhow!("unknown media kind {other:?}")),
        }
    }
}

/// A logical media item — one row, N [`MediaVariantDao`]. The bytes live in the
/// disk store; this is just metadata.
#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct MediaDao {
    pub media_id: i64,
    pub media_ref: String,
    pub kind: String,
    pub title: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}

impl MediaDao {
    /// The typed kind (errors only if the DB holds an unknown string).
    pub fn kind(&self) -> Result<MediaKind> {
        MediaKind::parse(&self.kind)
    }

    pub async fn create(
        executor: impl SqliteExecutor<'_>,
        media_ref: String,
        kind: MediaKind,
        title: Option<String>,
        width: Option<i64>,
        height: Option<i64>,
        duration_ms: Option<i64>,
    ) -> Result<MediaDao> {
        let kind_str = kind.as_str();
        let row = query!(
            r#"
            INSERT INTO media (media_ref, kind, title, width, height, duration_ms)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            RETURNING media_id as "media_id!", created_at as "created_at!"
            "#,
            media_ref,
            kind_str,
            title,
            width,
            height,
            duration_ms,
        )
        .fetch_one(executor)
        .await?;

        Ok(MediaDao {
            media_id: row.media_id,
            media_ref,
            kind: kind_str.to_string(),
            title,
            width,
            height,
            duration_ms,
            created_at: row.created_at,
        })
    }

    /// Lookup by the markdown ref — the transformer's render-time dispatch key.
    pub async fn find_by_ref(
        executor: impl SqliteExecutor<'_>,
        media_ref: &str,
    ) -> Result<Option<MediaDao>> {
        let media = query_as!(
            MediaDao,
            r#"
            SELECT media_id as "media_id!", media_ref, kind, title,
                   width, height, duration_ms, created_at
            FROM media
            WHERE media_ref = ?1
            "#,
            media_ref
        )
        .fetch_optional(executor)
        .await?;

        Ok(media)
    }

    /// Newest-first — the media library listing.
    #[allow(dead_code)] // wired by the media library UI (BZ.7)
    pub async fn find_all(executor: impl SqliteExecutor<'_>) -> Result<Vec<MediaDao>> {
        let media = query_as!(
            MediaDao,
            r#"
            SELECT media_id as "media_id!", media_ref, kind, title,
                   width, height, duration_ms, created_at
            FROM media
            ORDER BY media_id DESC
            "#
        )
        .fetch_all(executor)
        .await?;

        Ok(media)
    }

    /// Edit the display title (the URL `media_ref` is the stable key and is NOT
    /// renamed here — that would break existing `![](/media/<ref>)` embeds).
    /// An empty title clears it (display falls back to the ref).
    pub async fn update_title(
        executor: impl SqliteExecutor<'_>,
        media_id: i64,
        title: &str,
    ) -> Result<()> {
        let trimmed = title.trim();
        let title_opt = (!trimmed.is_empty()).then_some(trimmed);
        query!(
            r#"UPDATE media SET title = ?1 WHERE media_id = ?2"#,
            title_opt,
            media_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// ON DELETE CASCADE drops the variant rows; the disk bytes are swept
    /// separately (content-addressed → a sha may still be referenced elsewhere).
    pub async fn delete_by_id(executor: impl SqliteExecutor<'_>, media_id: i64) -> Result<()> {
        query!(r#"DELETE FROM media WHERE media_id = ?1"#, media_id)
            .execute(executor)
            .await?;
        Ok(())
    }
}

/// One stored encoding of a media item. `sha256` keys the disk store (never
/// exposed); `url_key` is the public HMAC token the serve route resolves.
#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct MediaVariantDao {
    pub variant_id: i64,
    pub media_id: i64,
    pub sha256: String,
    pub url_key: String,
    pub mime: String,
    pub codecs: Option<String>,
    pub bytes: i64,
}

impl MediaVariantDao {
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        executor: impl SqliteExecutor<'_>,
        media_id: i64,
        sha256: String,
        url_key: String,
        mime: String,
        codecs: Option<String>,
        bytes: i64,
    ) -> Result<MediaVariantDao> {
        let row = query!(
            r#"
            INSERT INTO media_variant (media_id, sha256, url_key, mime, codecs, bytes)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            RETURNING variant_id as "variant_id!"
            "#,
            media_id,
            sha256,
            url_key,
            mime,
            codecs,
            bytes,
        )
        .fetch_one(executor)
        .await?;

        Ok(MediaVariantDao {
            variant_id: row.variant_id,
            media_id,
            sha256,
            url_key,
            mime,
            codecs,
            bytes,
        })
    }

    /// All encodings of a media item — what the transformer turns into `<source>`s.
    pub async fn find_by_media_id(
        executor: impl SqliteExecutor<'_>,
        media_id: i64,
    ) -> Result<Vec<MediaVariantDao>> {
        let variants = query_as!(
            MediaVariantDao,
            r#"
            SELECT variant_id as "variant_id!", media_id, sha256, url_key, mime, codecs, bytes
            FROM media_variant
            WHERE media_id = ?1
            ORDER BY variant_id
            "#,
            media_id
        )
        .fetch_all(executor)
        .await?;

        Ok(variants)
    }

    /// Delete one stored encoding. The disk bytes are NOT swept here — the same
    /// sha may back another media item (content-addressed dedup); an orphan sweep
    /// handles unreferenced files (BZ.8).
    pub async fn delete_by_id(executor: impl SqliteExecutor<'_>, variant_id: i64) -> Result<()> {
        query!(
            r#"DELETE FROM media_variant WHERE variant_id = ?1"#,
            variant_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// The serve route's lookup: public HMAC token → the variant (mime + sha).
    /// Not unique (dedup'd content shares a url_key), so take the first.
    #[allow(dead_code)] // wired by the range serve route (BZ.2)
    pub async fn find_by_url_key(
        executor: impl SqliteExecutor<'_>,
        url_key: &str,
    ) -> Result<Option<MediaVariantDao>> {
        let variant = query_as!(
            MediaVariantDao,
            r#"
            SELECT variant_id as "variant_id!", media_id, sha256, url_key, mime, codecs, bytes
            FROM media_variant
            WHERE url_key = ?1
            LIMIT 1
            "#,
            url_key
        )
        .fetch_optional(executor)
        .await?;

        Ok(variant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn media_with_variants_roundtrip(pool: SqlitePool) -> Result<()> {
        // A video media item with the two encodes chris produces.
        let media = MediaDao::create(
            &pool,
            "skylander-intro".to_string(),
            MediaKind::Video,
            Some("Skylander intro".to_string()),
            Some(1728),
            Some(1116),
            Some(44_908),
        )
        .await?;
        assert_eq!(media.kind()?, MediaKind::Video);

        let av1 = MediaVariantDao::create(
            &pool,
            media.media_id,
            "av1sha".to_string(),
            "av1urlkey".to_string(),
            "video/mp4".to_string(),
            Some("av01.0.12M.08".to_string()),
            1_000,
        )
        .await?;
        let hevc = MediaVariantDao::create(
            &pool,
            media.media_id,
            "hevcsha".to_string(),
            "hevcurlkey".to_string(),
            "video/mp4".to_string(),
            Some("hvc1".to_string()),
            900,
        )
        .await?;

        // Lookup by ref (the transformer's dispatch key) returns the item.
        let found = MediaDao::find_by_ref(&pool, "skylander-intro").await?.unwrap();
        assert_eq!(found, media);

        // Both encodes come back, in insert order → <source> order.
        let variants = MediaVariantDao::find_by_media_id(&pool, media.media_id).await?;
        assert_eq!(variants, vec![av1.clone(), hevc]);

        // Serve route resolves the public token → the right variant (never the sha).
        let by_key = MediaVariantDao::find_by_url_key(&pool, "av1urlkey").await?.unwrap();
        assert_eq!(by_key, av1);
        assert!(MediaVariantDao::find_by_url_key(&pool, "nope").await?.is_none());

        // CASCADE: deleting the media drops its variants.
        MediaDao::delete_by_id(&pool, media.media_id).await?;
        assert!(MediaDao::find_by_ref(&pool, "skylander-intro").await?.is_none());
        assert!(MediaVariantDao::find_by_media_id(&pool, media.media_id)
            .await?
            .is_empty());

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn media_ref_is_unique(pool: SqlitePool) -> Result<()> {
        MediaDao::create(&pool, "dup".to_string(), MediaKind::Image, None, None, None, None)
            .await?;
        let second = MediaDao::create(
            &pool,
            "dup".to_string(),
            MediaKind::Image,
            None,
            None,
            None,
            None,
        )
        .await;
        assert!(second.is_err(), "duplicate media_ref must be rejected");
        Ok(())
    }
}
