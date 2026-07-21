//! DL.3 — resolve an internal (root-relative) link STRUCTURALLY against the DB.
//!
//! No HTTP. A role-gated or scheduled page correctly 404s an anonymous fetch, so a
//! self-fetch would false-positive a live-but-gated page as dead — we resolve
//! EXISTENCE against the content tree + media, never visibility. `find_by_path` is
//! deliberately date/role-blind (it's the shared mutation lookup), which is exactly
//! what we want: the row exists ⇒ the link isn't dead, gate or no gate.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::db::dao::content_pages::ContentPageDao;
use crate::db::dao::media::{MediaDao, MediaVariantDao};

/// The verdict for an internal link. `Unknown` is a SOFT signal ("I don't
/// recognize this route — review it"), distinct from `Dead` (the route exists and
/// the target is genuinely missing): the route map here is hand-maintained and can
/// drift from the real router, so an unrecognized `/…` must never read as a
/// confident "broken".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalVerdict {
    Ok,
    Dead,
    Unknown,
}

/// Exact routes that always exist: home, the special-page section indexes, the
/// static endpoints. (Detail pages under these resolve via the content tree below.)
const OK_EXACT: &[&str] = &[
    "/",
    "/blog",
    "/projects",
    "/3d",
    // The WASM slicer/placer editor (Phase CW) — a CODE route, not a content
    // page; without this it read as "review" (and the /3d dead-shape below
    // would upgrade that to a false "dead").
    "/3d/editor",
    "/library",
    "/library/audiobooks",
    "/resume",
    "/resume.pdf",
    "/login",
    "/feed.xml",
    "/blog/feed.xml",
    "/sitemap.xml",
    "/robots.txt",
];

/// Resolve `raw_path` (a root-relative internal link, query/fragment tolerated).
pub async fn resolve_internal(pool: &SqlitePool, raw_path: &str) -> Result<InternalVerdict> {
    // Existence doesn't depend on the query or fragment.
    let path = raw_path.split(['?', '#']).next().unwrap_or("");
    // Normalize a trailing slash, but keep root "/".
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };

    if OK_EXACT.contains(&path) {
        return Ok(InternalVerdict::Ok);
    }

    // The known DEAD-shape (Phase CD): project DETAIL pages live at
    // /pages/projects/<slug>, NOT /projects/<slug>. /projects is the index (Ok
    // above); a link to /projects/<anything> is the exact bug CD fixed in the feed.
    if let Some(rest) = path.strip_prefix("/projects/")
        && !rest.is_empty()
    {
        return Ok(InternalVerdict::Dead);
    }
    // Same shape for the 3d gallery: model DETAIL pages live at
    // /pages/3d/<slug>; /3d is the index and /3d/editor the tool (both Ok
    // above), so any other /3d/<x> is the CD class of bug.
    if let Some(rest) = path.strip_prefix("/3d/")
        && !rest.is_empty()
    {
        return Ok(InternalVerdict::Dead);
    }

    // Media byte URL: /media/file/<url_key>.
    if let Some(url_key) = path.strip_prefix("/media/file/") {
        return Ok(hit_or_dead(
            MediaVariantDao::find_by_url_key(pool, url_key).await?.is_some(),
        ));
    }
    // Author-ref media: /media/embed/<ref> and /media/<ref> both resolve the ref
    // (the embed prefix must be tried FIRST or "/media/embed/X" reads ref="embed/X").
    if let Some(media_ref) = path
        .strip_prefix("/media/embed/")
        .or_else(|| path.strip_prefix("/media/"))
        && !media_ref.is_empty()
    {
        return Ok(hit_or_dead(
            MediaDao::find_by_ref(pool, media_ref).await?.is_some(),
        ));
    }

    // Content tree: /blog/<slug…> resolves under the `blog` special page;
    // /pages/<seg…> walks from the root by page_name.
    let segments: Option<Vec<&str>> = if let Some(rest) = path.strip_prefix("/blog/") {
        Some(std::iter::once("blog").chain(split_segments(rest)).collect())
    } else {
        path.strip_prefix("/pages/")
            .map(|rest| split_segments(rest).collect())
    };
    if let Some(segs) = segments {
        if segs.is_empty() {
            return Ok(InternalVerdict::Unknown);
        }
        // find_by_path returns the full ancestor chain on success, empty on any
        // miss — a partial walk (len < segments) is a dead leaf under a live parent.
        let found = ContentPageDao::find_by_path(pool, &segs).await?;
        return Ok(hit_or_dead(found.len() == segs.len()));
    }

    // Unrecognized internal route → surface for review, never a hard "dead".
    Ok(InternalVerdict::Unknown)
}

fn hit_or_dead(hit: bool) -> InternalVerdict {
    if hit {
        InternalVerdict::Ok
    } else {
        InternalVerdict::Dead
    }
}

fn split_segments(s: &str) -> impl Iterator<Item = &str> {
    s.split('/').filter(|seg| !seg.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn known_static_routes_are_ok(pool: SqlitePool) {
        for path in [
            "/",
            "/blog",
            "/projects",
            "/resume",
            "/resume.pdf",
            "/feed.xml",
            "/sitemap.xml",
            "/robots.txt",
            "/library",
            "/library/audiobooks",
        ] {
            assert_eq!(
                resolve_internal(&pool, path).await.unwrap(),
                InternalVerdict::Ok,
                "{path} should be a known-live route"
            );
        }
        // Trailing slash + a query/fragment must not change existence.
        assert_eq!(
            resolve_internal(&pool, "/blog/").await.unwrap(),
            InternalVerdict::Ok
        );
        assert_eq!(
            resolve_internal(&pool, "/resume?x=1#top").await.unwrap(),
            InternalVerdict::Ok
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn projects_slug_is_the_dead_shape(pool: SqlitePool) {
        // /projects is the index (Ok); /projects/<slug> is the CD dead-shape —
        // detail pages live at /pages/projects/<slug>.
        assert_eq!(
            resolve_internal(&pool, "/projects").await.unwrap(),
            InternalVerdict::Ok
        );
        assert_eq!(
            resolve_internal(&pool, "/projects/some-piece").await.unwrap(),
            InternalVerdict::Dead
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn three_d_editor_is_ok_and_slug_is_the_dead_shape(pool: SqlitePool) {
        // /3d/editor is a CODE route (the WASM slicer) — chris's false-positive
        // report; /3d/<anything else> is the same dead-shape as /projects/<slug>.
        assert_eq!(
            resolve_internal(&pool, "/3d/editor").await.unwrap(),
            InternalVerdict::Ok
        );
        assert_eq!(
            resolve_internal(&pool, "/3d/benchy").await.unwrap(),
            InternalVerdict::Dead
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn content_tree_hit_and_miss(pool: SqlitePool) {
        // A top-level page → /pages/<slug> resolves; a missing one is Dead.
        ContentPageDao::create(&pool, None, "about".to_string(), None, "# About".to_string(), None)
            .await
            .unwrap();
        assert_eq!(
            resolve_internal(&pool, "/pages/about").await.unwrap(),
            InternalVerdict::Ok
        );
        assert_eq!(
            resolve_internal(&pool, "/pages/nope").await.unwrap(),
            InternalVerdict::Dead
        );

        // A blog post is a child of the (migration-seeded) `blog` special page.
        let blog = ContentPageDao::find_by_name(&pool, None, "blog")
            .await
            .unwrap()
            .expect("blog special page seeded by migration 0010");
        ContentPageDao::create(
            &pool,
            Some(blog.page_id),
            "hello-world".to_string(),
            None,
            "# Hello".to_string(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            resolve_internal(&pool, "/blog/hello-world").await.unwrap(),
            InternalVerdict::Ok
        );
        assert_eq!(
            resolve_internal(&pool, "/blog/ghost-post").await.unwrap(),
            InternalVerdict::Dead
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn media_miss_is_dead(pool: SqlitePool) {
        // No media seeded → any ref / url_key is a miss = Dead.
        assert_eq!(
            resolve_internal(&pool, "/media/0190-nonexistent-ref").await.unwrap(),
            InternalVerdict::Dead
        );
        assert_eq!(
            resolve_internal(&pool, "/media/file/deadbeef").await.unwrap(),
            InternalVerdict::Dead
        );
        assert_eq!(
            resolve_internal(&pool, "/media/embed/also-missing").await.unwrap(),
            InternalVerdict::Dead
        );
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn unrecognized_route_is_unknown(pool: SqlitePool) {
        // A `/…` matching no known prefix is SOFT "review", not a hard "dead" — the
        // route map can drift from the real router.
        assert_eq!(
            resolve_internal(&pool, "/some/mystery/route").await.unwrap(),
            InternalVerdict::Unknown
        );
    }
}
