//! Web-layer integration tests against the in-process harness (Phase 8.3):
//! the `/admin` auth layer, and the request-logging middleware.

use hotchkiss_io::test_support::spawn_test_server;
use reqwest::{redirect::Policy, StatusCode};
use sqlx::Row;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn analytics_requires_admin() {
    let server = spawn_test_server().await.expect("spawn");

    // anonymous → 403
    let resp = client()
        .get(server.url("/admin/analytics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // logged in but only Registered → still 403
    let registered = client();
    let resp = registered
        .post(server.url("/test/login?role=Registered"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = registered
        .get(server.url("/admin/analytics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Admin → 200, and the dashboard renders
    let admin = client();
    let resp = admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = admin
        .get(server.url("/admin/analytics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.text().await.unwrap().contains("Analytics"));
}

#[tokio::test]
async fn request_log_middleware_records_requests() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("Probe", "# probe page")
        .await
        .expect("seed");

    let resp = reqwest::get(server.url("/pages/Probe")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // the INSERT is fire-and-forget (tokio::spawn) — poll briefly for it
    let mut found = None;
    for _ in 0..100 {
        let rows = sqlx::query("SELECT path, status, ip FROM request_log ORDER BY id DESC")
            .fetch_all(&server.pool)
            .await
            .unwrap();
        if let Some(row) = rows
            .iter()
            .find(|r| r.get::<String, _>("path") == "/pages/Probe")
        {
            found = Some((
                row.get::<i64, _>("status"),
                row.get::<Option<String>, _>("ip"),
            ));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let (status, ip) = found.expect("request_log should have a row for /pages/Probe");
    assert_eq!(status, 200);
    assert_eq!(ip.as_deref(), Some("127.0.0.1"));
}

#[tokio::test]
async fn blog_index_empty_state() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = reqwest::get(server.url("/blog")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("No posts yet"), "body was: {body}");
}

#[tokio::test]
async fn blog_index_lists_seeded_post() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post(
            "my-first-post",
            "Hello, world. This is the body of my first post.",
        )
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/blog"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("my-first-post"), "title missing: {body}");
    assert!(
        body.contains("Hello, world. This is the body of my first post."),
        "excerpt missing: {body}"
    );
}

#[tokio::test]
async fn blog_post_200_and_404() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post("real-post", "Body text.")
        .await
        .expect("seed");

    let resp = reqwest::get(server.url("/blog/real-post")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = reqwest::get(server.url("/blog/no-such-post"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blog_feed_serves_atom_with_entry() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post("feed-post", "Feed body content.")
        .await
        .expect("seed");

    let resp = reqwest::get(server.url("/blog/feed.xml")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("atom"), "unexpected content-type: {ct}");

    let body = resp.text().await.unwrap();
    assert!(body.starts_with("<?xml"), "not xml: {body}");
    assert!(body.contains("<feed"));
    assert!(body.contains("feed-post"));
    assert!(body.contains("Feed body content"));
}

#[tokio::test]
async fn manifest_webmanifest_served() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = reqwest::get(server.url("/manifest.webmanifest"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .map(|h| h.to_str().unwrap().to_string())
        .unwrap_or_default();
    // mime_guess maps .webmanifest → application/manifest+json on recent versions;
    // either that or application/json is acceptable. octet-stream would mean a regression.
    assert!(
        ct.contains("manifest+json") || ct.contains("json"),
        "unexpected content-type for manifest: {ct}"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("\"name\""), "manifest body wrong: {body}");
    assert!(body.contains("\"icons\""));
}
