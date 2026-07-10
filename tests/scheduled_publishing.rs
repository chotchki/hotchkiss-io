//! Phase CU — scheduled / timed publishing. A content page dated in the FUTURE
//! (`page_creation_date > now`) is hidden from non-admins on every public read
//! path (direct URL 404s through the SAME cat page a genuine miss returns, and it
//! is absent from the blog/project indexes, the feed, the sitemap, the home bands
//! and the nav), while an Admin sees it inline, badged. No new column, no cron —
//! the flip is evaluated at read time against `datetime('now')` / `Utc::now()`.
//!
//! Tests future-date a seeded row with a raw `UPDATE` (space-form text, which the
//! `datetime()`-normalized SQL gate + the Rust-side `DateTime<Utc>` decode both
//! read correctly — the mixed-format-column point).

use hotchkiss_io::test_support::spawn_test_server;
use reqwest::{redirect::Policy, StatusCode};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(Policy::none())
        .build()
        .unwrap()
}

/// A `page_creation_date` far in the future — reads as scheduled everywhere.
const FUTURE: &str = "2999-01-01 00:00:00";
/// A `page_creation_date` in the past — published.
const PAST: &str = "2000-01-01 00:00:00";

async fn set_creation_date(pool: &sqlx::SqlitePool, page_name: &str, date: &str) {
    sqlx::query("UPDATE content_pages SET page_creation_date = ?1 WHERE page_name = ?2")
        .bind(date)
        .bind(page_name)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn scheduled_blog_post_hidden_from_anon_visible_to_admin() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("live-post", "Live body.").await.expect("seed");
    server.seed_blog_post("future-post", "Future body.").await.expect("seed");
    set_creation_date(&server.pool, "future-post", FUTURE).await;

    // --- Anonymous: hidden everywhere ---
    // Direct URL → 404 through the cat page.
    let resp = reqwest::get(server.url("/blog/future-post")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(
        resp.text().await.unwrap().contains("Which one is guilty"),
        "a scheduled post must 404 through the cat page"
    );

    let blog = reqwest::get(server.url("/blog")).await.unwrap().text().await.unwrap();
    assert!(blog.contains("live-post"), "the live post lists");
    assert!(!blog.contains("future-post"), "the scheduled post is hidden from /blog");

    let feed = reqwest::get(server.url("/feed.xml")).await.unwrap().text().await.unwrap();
    assert!(feed.contains("live-post"));
    assert!(!feed.contains("future-post"), "the scheduled post is hidden from the feed");

    let sitemap = reqwest::get(server.url("/sitemap.xml")).await.unwrap().text().await.unwrap();
    assert!(sitemap.contains("/blog/live-post"));
    assert!(!sitemap.contains("/blog/future-post"), "the scheduled post is hidden from the sitemap");

    // --- Admin: visible + badged ---
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let resp = admin.get(server.url("/blog/future-post")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin can view a scheduled post");
    assert!(
        resp.text().await.unwrap().contains("Scheduled"),
        "admin sees the Scheduled badge on the post"
    );
    let blog = admin.get(server.url("/blog")).send().await.unwrap().text().await.unwrap();
    assert!(blog.contains("future-post"), "admin sees the scheduled post in /blog");
    assert!(blog.contains("Scheduled"), "admin sees the Scheduled badge on the card");
}

#[tokio::test]
async fn scheduled_and_missing_return_identical_404() {
    // A scheduled post and a truly-missing slug must return the SAME 404 body, or
    // the difference is an oracle telling a non-admin the scheduled slug exists.
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("hidden-post", "Secret.").await.expect("seed");
    set_creation_date(&server.pool, "hidden-post", FUTURE).await;

    let scheduled = reqwest::get(server.url("/blog/hidden-post")).await.unwrap();
    let missing = reqwest::get(server.url("/blog/no-such-post")).await.unwrap();
    assert_eq!(scheduled.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(
        scheduled.text().await.unwrap().contains("Which one is guilty"),
        "scheduled → cat 404"
    );
    assert!(
        missing.text().await.unwrap().contains("Which one is guilty"),
        "missing → cat 404"
    );
}

#[tokio::test]
async fn scheduled_project_hidden_from_index_and_sitemap() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_project("live-proj", "# Live Project\n\nShipped.").await.expect("seed");
    server.seed_project("future-proj", "# Future Project\n\nNot yet.").await.expect("seed");
    set_creation_date(&server.pool, "future-proj", FUTURE).await;

    // Anon: direct URL 404, absent from /projects and the sitemap.
    let resp = reqwest::get(server.url("/pages/projects/future-proj")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "a scheduled project must 404 for anon");
    let projects = reqwest::get(server.url("/projects")).await.unwrap().text().await.unwrap();
    assert!(projects.contains("live-proj"), "the live project lists");
    assert!(!projects.contains("future-proj"), "the scheduled project is hidden from /projects");
    let sitemap = reqwest::get(server.url("/sitemap.xml")).await.unwrap().text().await.unwrap();
    assert!(sitemap.contains("/pages/projects/live-proj"));
    assert!(
        !sitemap.contains("/pages/projects/future-proj"),
        "sitemap must exclude the scheduled project"
    );

    // Admin sees it in the index, badged.
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let projects = admin.get(server.url("/projects")).send().await.unwrap().text().await.unwrap();
    assert!(projects.contains("future-proj"), "admin sees the scheduled project");
    assert!(projects.contains("Scheduled"), "admin sees the Scheduled badge");
}

#[tokio::test]
async fn scheduled_resume_hidden_from_anon_and_pdf_gated() {
    // The résumé's newest child is scheduled → anon gets nothing (page AND pdf),
    // admin sees it. Guards the show_resume_pdf no-session leak.
    let server = spawn_test_server().await.expect("spawn");
    server.seed_resume("# Draft Resume\n\nNot ready.").await.expect("seed");
    // Future-date EVERY résumé child (the 0012 empty placeholder + our seed) so no
    // published child remains — else /resume falls back to the placeholder.
    sqlx::query(
        "UPDATE content_pages SET page_creation_date = ?1 \
         WHERE parent_page_id = (SELECT page_id FROM content_pages \
                                 WHERE page_name = 'resume' AND parent_page_id IS NULL)",
    )
    .bind(FUTURE)
    .execute(&server.pool)
    .await
    .unwrap();

    let resp = reqwest::get(server.url("/resume")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "a scheduled résumé must not show to anon");
    let resp = reqwest::get(server.url("/resume.pdf")).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "/resume.pdf must be gated too (the no-session leak fix)"
    );

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let body = admin.get(server.url("/resume")).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("Draft Resume"), "admin sees the scheduled résumé: {body}");
    assert!(body.contains("Scheduled"), "admin sees the Scheduled badge");
}

#[tokio::test]
async fn resume_falls_back_to_newest_published_child() {
    // A newer SCHEDULED résumé sits in front of an older PUBLISHED one. Anon must
    // fall back to the published child (the dropped LIMIT 1), not 404.
    let server = spawn_test_server().await.expect("spawn");
    // Published, newest-AUTHORED résumé (seed_resume creates the "main" child, dated now).
    server.seed_resume("# Live Resume\n\nPUBLISHEDBODY").await.expect("seed");
    // A SEPARATE, newer SCHEDULED child (seed_resume can't — it reuses the name "main").
    // Dated 2999, so it's the newest child; the gate must skip it to the published one.
    sqlx::query(
        "INSERT INTO content_pages \
             (parent_page_id, page_name, page_markdown, page_order, special_page, page_creation_date) \
         SELECT page_id, 'draft', ?1, 0, false, ?2 \
         FROM content_pages WHERE page_name = 'resume' AND parent_page_id IS NULL",
    )
    .bind("# Draft Resume\n\nSCHEDULEDBODY")
    .bind(FUTURE)
    .execute(&server.pool)
    .await
    .unwrap();

    let body = reqwest::get(server.url("/resume")).await.unwrap().text().await.unwrap();
    assert!(body.contains("Live Resume"), "anon falls back to the published résumé: {body}");
    assert!(!body.contains("Draft Resume"), "the scheduled résumé is hidden from anon");

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let body = admin.get(server.url("/resume")).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("Draft Resume"), "admin sees the newest (scheduled) résumé: {body}");
}

#[tokio::test]
async fn feed_excludes_future_and_flip_busts_304() {
    // CU.5 + CU.6: the feed excludes a future post; when it goes live (date → past,
    // simulating the wall-clock flip / Publish-now) the published entry count moves
    // and the stale ETag no longer 304s — the crawler re-fetches.
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("flip-post", "Flip body content.").await.expect("seed");
    set_creation_date(&server.pool, "flip-post", FUTURE).await;

    let c = client();
    let resp = c.get(server.url("/feed.xml")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let etag1 = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("feed carries an ETag")
        .to_string();
    let body1 = resp.text().await.unwrap();
    assert!(!body1.contains("flip-post"), "a future-dated post is excluded from the feed");

    // Nothing changed → the same ETag 304s.
    let resp = c
        .get(server.url("/feed.xml"))
        .header("If-None-Match", &etag1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED, "a stable feed 304s");

    // Publish it (the flip).
    set_creation_date(&server.pool, "flip-post", PAST).await;

    // The old ETag no longer matches → 200, and the post now appears.
    let resp = c
        .get(server.url("/feed.xml"))
        .header("If-None-Match", &etag1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the go-live flip must bust the stale 304");
    assert!(
        resp.text().await.unwrap().contains("flip-post"),
        "the published post now appears in the feed"
    );
}

#[tokio::test]
async fn scheduled_top_level_page_hidden_from_nav_and_direct_url() {
    // A non-special top-level page that's scheduled is hidden from the global nav
    // (unconditionally) and 404s on its direct URL for anon; admin reaches it.
    let server = spawn_test_server().await.expect("spawn");
    server.seed_content_page("secretpage", "# Secret Page\n\nfuture").await.expect("seed");
    set_creation_date(&server.pool, "secretpage", FUTURE).await;

    // Anon: absent from the nav (rendered on every page — check the home page) and
    // 404 on the direct URL.
    let home = reqwest::get(server.url("/")).await.unwrap().text().await.unwrap();
    assert!(!home.contains("secretpage"), "a scheduled top-level page must not appear in the nav");
    let resp = reqwest::get(server.url("/pages/secretpage")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "scheduled top-level page 404s for anon");

    // Admin: reaches it directly, badged.
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let resp = admin.get(server.url("/pages/secretpage")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin can view a scheduled top-level page");
    assert!(resp.text().await.unwrap().contains("Scheduled"), "admin sees the Scheduled badge");
}

#[tokio::test]
async fn pages_root_redirect_skips_scheduled_first_page() {
    // GET /pages redirects to the first top-level page; it must SKIP a scheduled one,
    // else the Location header leaks the draft slug's existence (an oracle).
    let server = spawn_test_server().await.expect("spawn");
    server.seed_content_page("aaa-scheduled", "# Draft").await.expect("seed");
    server.seed_content_page("bbb-live", "# Live").await.expect("seed");
    // Order the scheduled page ahead of everything, then future-date it.
    sqlx::query("UPDATE content_pages SET page_order = -100 WHERE page_name = 'aaa-scheduled'")
        .execute(&server.pool)
        .await
        .unwrap();
    set_creation_date(&server.pool, "aaa-scheduled", FUTURE).await;

    let resp = client().get(server.url("/pages")).send().await.unwrap();
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        !loc.contains("aaa-scheduled"),
        "the /pages redirect must not point an anon at the scheduled slug: {loc}"
    );
}

#[tokio::test]
async fn publish_now_and_unpublish_buttons_flip_visibility() {
    // CU.9: the Publish-now / Unpublish admin endpoints flip a page's visibility.
    let server = spawn_test_server().await.expect("spawn");
    let page = server.seed_content_page("togglepage", "# Toggle\n\nbody").await.expect("seed");
    set_creation_date(&server.pool, "togglepage", FUTURE).await;

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Scheduled → anon 404.
    assert_eq!(
        reqwest::get(server.url("/pages/togglepage")).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    // Publish now → anon sees it.
    let resp = admin
        .post(server.url(&format!("/admin/pages/{}/publish", page.page_id)))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "publish_now should succeed: {}", resp.status());
    assert_eq!(
        reqwest::get(server.url("/pages/togglepage")).await.unwrap().status(),
        StatusCode::OK,
        "after Publish-now the page is public"
    );

    // Unpublish → anon 404 again (back to a draft).
    let resp = admin
        .post(server.url(&format!("/admin/pages/{}/unpublish", page.page_id)))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "unpublish should succeed: {}", resp.status());
    assert_eq!(
        reqwest::get(server.url("/pages/togglepage")).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "after Unpublish the page is a hidden draft again"
    );

    // The publish/unpublish endpoints are admin-gated; anonymous → 401 (missing
    // identity, DK.2).
    assert_eq!(
        client()
            .post(server.url(&format!("/admin/pages/{}/publish", page.page_id)))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED,
        "publish_now must be admin-gated"
    );
}
