use anyhow::{anyhow, Result};
use sqlx::{prelude::FromRow, query, query_as, SqliteExecutor};

use super::roles::Role;

/// What a media item IS — drives the render-time dispatch (image → `<img>`,
/// video → `<video>` multi-source, stl → `<object class="stl-view">`, file → a
/// download link). Stored as TEXT; constructed typed, read back via [`MediaDao::kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Stl,
    Audio,
    File,
}

impl MediaKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Stl => "stl",
            MediaKind::Audio => "audio",
            MediaKind::File => "file",
        }
    }

    pub fn parse(s: &str) -> Result<MediaKind> {
        match s {
            "image" => Ok(MediaKind::Image),
            "video" => Ok(MediaKind::Video),
            "stl" => Ok(MediaKind::Stl),
            "audio" => Ok(MediaKind::Audio),
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
    /// Minimum role that may fetch this item's bytes/302/embed (Phase DC).
    /// `None` is the ONLY public spelling; decode via `min_role_rank` — never
    /// branch on the raw string (same rule as `content_pages.min_role`).
    pub min_role: Option<String>,
    /// Audiobook chapters (Phase DD): JSON `[{"start_ms": N, "title": "…"}]`,
    /// stamped at ingest for `Audio`. `None` elsewhere / when chapterless.
    pub chapters: Option<String>,
}

impl MediaDao {
    /// The typed kind (errors only if the DB holds an unknown string).
    pub fn kind(&self) -> Result<MediaKind> {
        MediaKind::parse(&self.kind)
    }

    /// Fail-closed decode of `min_role` — the exact twin of
    /// `ContentPageDao::min_role_rank` and the SQL CASE in
    /// `find_by_url_key_with_required_rank`. Unknown non-NULL values rank as
    /// TOP-of-ladder (`Role::Admin.rank()`), never public.
    pub fn min_role_rank(&self) -> u8 {
        match self.min_role.as_deref() {
            None => 0,
            Some("Registered") => 1,
            Some("Family") => 2,
            Some(_) => Role::Admin.rank(),
        }
    }

    /// May `viewer` fetch this item's bytes / 302 / embed? Media has no
    /// scheduling axis — the gate is the role clause alone.
    pub fn is_visible_to(&self, viewer: Role) -> bool {
        viewer.rank() >= self.min_role_rank()
    }

    /// The library badge / selector label, from the fail-closed decode (a
    /// garbage value reads as "Admin-only", never as its own text).
    pub fn visibility_label(&self) -> Option<&'static str> {
        match self.min_role_rank() {
            0 => None,
            1 => Some("Registered"),
            2 => Some("Family"),
            _ => Some("Admin-only"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        executor: impl SqliteExecutor<'_>,
        media_ref: String,
        kind: MediaKind,
        title: Option<String>,
        width: Option<i64>,
        height: Option<i64>,
        duration_ms: Option<i64>,
        min_role: Option<String>,
        chapters: Option<String>,
    ) -> Result<MediaDao> {
        let kind_str = kind.as_str();
        let row = query!(
            r#"
            INSERT INTO media (media_ref, kind, title, width, height, duration_ms, min_role, chapters)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            RETURNING media_id as "media_id!", created_at as "created_at!"
            "#,
            media_ref,
            kind_str,
            title,
            width,
            height,
            duration_ms,
            min_role,
            chapters,
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
            min_role,
            chapters,
        })
    }

    /// Set just the visibility gate — the library UI's per-item control
    /// (`POST /admin/media/{id}/visibility`). `None` = public.
    pub async fn set_min_role(
        executor: impl SqliteExecutor<'_>,
        media_id: i64,
        min_role: Option<String>,
    ) -> Result<()> {
        query!(
            r#"UPDATE media SET min_role = ?1 WHERE media_id = ?2"#,
            min_role,
            media_id
        )
        .execute(executor)
        .await?;
        Ok(())
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
                   width, height, duration_ms, created_at, min_role, chapters
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
                   width, height, duration_ms, created_at, min_role, chapters
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
    /// Which configured media root holds the bytes — a HINT the serve route tries
    /// first (O(1)) before falling back to a first-found scan across all roots.
    /// `None` for legacy rows / unknown → always resolved by scan.
    pub storage_root: Option<String>,
    /// Pixel dimensions of THIS encoding (Phase CN) — drive the srcset `Nw`
    /// descriptor for an image's width-stepped variants. `None` for video / stl /
    /// file / legacy variants (omitted from the srcset).
    pub width: Option<i64>,
    pub height: Option<i64>,
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
        storage_root: Option<String>,
        width: Option<i64>,
        height: Option<i64>,
    ) -> Result<MediaVariantDao> {
        let row = query!(
            r#"
            INSERT INTO media_variant (media_id, sha256, url_key, mime, codecs, bytes, storage_root, width, height)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            RETURNING variant_id as "variant_id!"
            "#,
            media_id,
            sha256,
            url_key,
            mime,
            codecs,
            bytes,
            storage_root,
            width,
            height,
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
            storage_root,
            width,
            height,
        })
    }

    /// Stamp this variant's pixel dimensions (Phase CN backfill). A legacy image's
    /// original variant predates the width column; setting it to the item's dims
    /// puts it in the srcset as the largest entry.
    pub async fn set_dimensions(
        executor: impl SqliteExecutor<'_>,
        variant_id: i64,
        width: Option<i64>,
        height: Option<i64>,
    ) -> Result<()> {
        query!(
            r#"UPDATE media_variant SET width = ?1, height = ?2 WHERE variant_id = ?3"#,
            width,
            height,
            variant_id
        )
        .execute(executor)
        .await?;
        Ok(())
    }

    /// All encodings of a media item — what the transformer turns into `<source>`s.
    pub async fn find_by_media_id(
        executor: impl SqliteExecutor<'_>,
        media_id: i64,
    ) -> Result<Vec<MediaVariantDao>> {
        let variants = query_as!(
            MediaVariantDao,
            r#"
            SELECT variant_id as "variant_id!", media_id, sha256, url_key, mime, codecs, bytes, storage_root, width, height
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
            SELECT variant_id as "variant_id!", media_id, sha256, url_key, mime, codecs, bytes, storage_root, width, height
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

    /// The byte route's lookup + gate in ONE query (Phase DC): the variant plus
    /// the STRICTEST-WINS required rank across ALL media rows sharing this
    /// `url_key`. The url_key is deterministic in the sha and its index is
    /// deliberately NON-unique (identical bytes dedup across items), so gating
    /// by `find_by_url_key`'s arbitrary `LIMIT 1` owner could resolve to the
    /// LOOSEST item and leak silently. `MAX(rank)` can only over-restrict —
    /// which breaks visibly (a public embed 404s) instead of leaking. The CASE
    /// is the same fail-closed ladder as `min_role_rank` / the content_pages
    /// queries: NULL 0 / Registered 1 / Family 2 / ELSE top.
    pub async fn find_by_url_key_with_required_rank(
        executor: impl SqliteExecutor<'_>,
        url_key: &str,
    ) -> Result<Option<(MediaVariantDao, i64)>> {
        let row = query!(
            r#"
            SELECT v.variant_id as "variant_id!", v.media_id, v.sha256, v.url_key, v.mime,
                   v.codecs, v.bytes, v.storage_root, v.width, v.height,
                   (SELECT MAX(CASE WHEN m.min_role IS NULL THEN 0
                                    WHEN m.min_role = 'Registered' THEN 1
                                    WHEN m.min_role = 'Family' THEN 2
                                    ELSE 3 END)
                    FROM media_variant v2
                    JOIN media m ON m.media_id = v2.media_id
                    WHERE v2.url_key = v.url_key) as "required_rank!: i64"
            FROM media_variant v
            WHERE v.url_key = ?1
            LIMIT 1
            "#,
            url_key
        )
        .fetch_optional(executor)
        .await?;

        Ok(row.map(|r| {
            (
                MediaVariantDao {
                    variant_id: r.variant_id,
                    media_id: r.media_id,
                    sha256: r.sha256,
                    url_key: r.url_key,
                    mime: r.mime,
                    codecs: r.codecs,
                    bytes: r.bytes,
                    storage_root: r.storage_root,
                    width: r.width,
                    height: r.height,
                },
                r.required_rank,
            )
        }))
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
            None,
            None,
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
            None,
            None,
            None,
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
            Some("/Volumes/big/media".to_string()),
            None,
            None,
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

    /// DC.2's core risk: two items sharing identical bytes (same sha → same
    /// url_key, index deliberately non-unique). The gate must be the STRICTEST
    /// owner — a public item + a Family item sharing a sha gate at Family;
    /// loosest-wins would leak the family copy through the public one's row.
    /// Garbage min_role on a third owner escalates to top (fail-closed).
    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn shared_sha_gates_strictest_wins(pool: SqlitePool) -> Result<()> {
        let public = MediaDao::create(
            &pool, "pub-item".into(), MediaKind::File, None, None, None, None, None, None,
        )
        .await?;
        let family = MediaDao::create(
            &pool, "fam-item".into(), MediaKind::File, None, None, None, None,
            Some("Family".to_string()), None,
        )
        .await?;
        // Identical bytes → identical sha + url_key on both items' variants.
        for m in [&public, &family] {
            MediaVariantDao::create(
                &pool, m.media_id, "sharedsha".into(), "sharedkey".into(),
                "application/zip".into(), None, 10, None, None, None,
            )
            .await?;
        }
        let (_, rank) = MediaVariantDao::find_by_url_key_with_required_rank(&pool, "sharedkey")
            .await?
            .expect("variant resolves");
        assert_eq!(rank, 2, "NULL + Family sharing a sha must gate at Family");

        // A public-only key gates at 0.
        MediaVariantDao::create(
            &pool, public.media_id, "solosha".into(), "solokey".into(),
            "application/zip".into(), None, 10, None, None, None,
        )
        .await?;
        let (_, rank) = MediaVariantDao::find_by_url_key_with_required_rank(&pool, "solokey")
            .await?
            .unwrap();
        assert_eq!(rank, 0);

        // Garbage min_role on a third shared owner → top-of-ladder, never public.
        let garbage = MediaDao::create(
            &pool, "junk-item".into(), MediaKind::File, None, None, None, None,
            Some("Bogus".to_string()), None,
        )
        .await?;
        MediaVariantDao::create(
            &pool, garbage.media_id, "sharedsha".into(), "sharedkey".into(),
            "application/zip".into(), None, 10, None, None, None,
        )
        .await?;
        let (_, rank) = MediaVariantDao::find_by_url_key_with_required_rank(&pool, "sharedkey")
            .await?
            .unwrap();
        assert_eq!(rank as u8, crate::db::dao::roles::Role::Admin.rank());

        // An unknown key stays a miss.
        assert!(
            MediaVariantDao::find_by_url_key_with_required_rank(&pool, "nokey")
                .await?
                .is_none()
        );
        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn media_ref_is_unique(pool: SqlitePool) -> Result<()> {
        MediaDao::create(&pool, "dup".to_string(), MediaKind::Image, None, None, None, None, None, None)
            .await?;
        let second = MediaDao::create(
            &pool,
            "dup".to_string(),
            MediaKind::Image,
            None,
            None,
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
