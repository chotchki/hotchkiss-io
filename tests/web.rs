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

/// Mirrors the runtime d2 resolver enough to gate the happy-path assertions on a
/// box without d2 (the server itself uses the resolver in `web::markdown::diagram`).
fn d2_present() -> bool {
    std::path::Path::new("/opt/homebrew/bin/d2").is_file()
        || std::path::Path::new("/usr/local/bin/d2").is_file()
        || std::process::Command::new("d2")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// Pull the diagram hash out of an `hx-get="/diagram/<hash>"` in a page body.
fn hash_from_body(body: &str) -> Option<String> {
    body.split("/diagram/")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .map(|s| s.to_string())
}

#[tokio::test]
async fn d2_fence_emits_source_and_swap_target() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("diagram-page", "# Title\n\n```d2\nx -> y -> z\n```\n")
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/pages/diagram-page"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    // The SERVED HTML carries the source (LLM/no-JS friendly) + the swap target.
    assert!(body.contains("hx-get=\"/diagram/"), "expected the HTMX swap target");
    assert!(
        body.contains("x -&gt; y"),
        "the d2 source should be in the served HTML: {body}"
    );
}

#[tokio::test]
async fn d2_fence_emits_swap_target_on_a_blog_post() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post("diagram-post", "```d2\nx -> y\n```\n")
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/blog/diagram-post"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("hx-get=\"/diagram/"));
}

#[tokio::test]
async fn diagram_endpoint_renders_registered_source() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("diagram-page2", "```d2\nx -> y\n```\n")
        .await
        .expect("seed");

    // Render the page first so the source registers, then follow the swap.
    let body = reqwest::get(server.url("/pages/diagram-page2"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let hash = hash_from_body(&body).expect("page should carry a diagram hash");

    let resp = reqwest::get(server.url(&format!("/diagram/{hash}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    if d2_present() {
        assert!(
            resp.text().await.unwrap().contains("data:image/svg+xml"),
            "the endpoint should return the rendered diagram"
        );
    }
}

#[tokio::test]
async fn diagram_endpoint_unknown_hash_is_error_block_not_500() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = reqwest::get(server.url("/diagram/deadbeefdeadbeefdeadbeefdeadbeef"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "unknown hash must not 500");
    assert!(resp.text().await.unwrap().contains("diagram-error"));
}

#[tokio::test]
async fn broken_d2_endpoint_is_error_block() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("broken-diagram", "```d2\nx -> -> -> {{{\n```\n")
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/pages/broken-diagram"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let hash = hash_from_body(&body).expect("page should carry a diagram hash");

    let resp = reqwest::get(server.url(&format!("/diagram/{hash}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "a broken diagram must not 500");
    if d2_present() {
        assert!(
            resp.text().await.unwrap().contains("diagram-error"),
            "the failure should be visible, not swallowed"
        );
    }
}

#[tokio::test]
async fn analytics_renders_chart_and_content_pages() {
    let server = spawn_test_server().await.expect("spawn");

    // Seed traffic: a content page (hit twice from one IP) + a static asset.
    for (path, ip) in [
        ("/pages/test-page", "1.1.1.1"),
        ("/pages/test-page", "1.1.1.1"),
        ("/styles/main.css", "1.1.1.1"),
    ] {
        sqlx::query("INSERT INTO request_log (method, path, status, ip) VALUES ('GET', ?, 200, ?)")
            .bind(path)
            .bind(ip)
            .execute(&server.pool)
            .await
            .unwrap();
    }

    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    let resp = admin
        .get(server.url("/admin/analytics?since=30"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("<svg"), "expected the views-per-day chart");
    assert!(body.contains("Unique visitors"), "expected the total/unique toggle");
    assert!(
        body.contains("/pages/test-page"),
        "the content page should appear in top pages"
    );
    assert!(body.contains("Top referrers"), "expected the referrers panel");
    assert!(body.contains("paths=all"), "expected the Content/All toggle");
}

/// Regression guard for the anonymous content-mutation bypass: the `/pages`
/// mutating handlers must reject anyone who isn't Admin. The old `if let
/// Authenticated(user) && role != Admin` idiom let Anonymous fall straight
/// through (an unauthenticated stranger could overwrite/create/delete pages).
#[tokio::test]
async fn only_admin_can_mutate_pages() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("Victim", "# original content")
        .await
        .expect("seed");

    let put_form = [
        ("page_category", ""),
        ("page_markdown", "DEFACED"),
        ("page_cover_attachment_id", ""),
        ("page_order", "0"),
    ];

    // --- Anonymous: every mutation is FORBIDDEN ---
    let anon = client();
    let resp = anon
        .post(server.url("/pages"))
        .form(&[("page_name", "hax")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "anon must not create top-level pages"
    );
    let resp = anon
        .put(server.url("/pages/Victim"))
        .form(&put_form)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "anon must not overwrite pages"
    );
    let resp = anon
        .delete(server.url("/pages/Victim"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "anon must not delete pages"
    );

    // --- Registered (logged in, not Admin): still FORBIDDEN ---
    let registered = client();
    registered
        .post(server.url("/test/login?role=Registered"))
        .send()
        .await
        .unwrap();
    let resp = registered
        .put(server.url("/pages/Victim"))
        .form(&put_form)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "registered must not overwrite pages"
    );

    // --- The page survived all of that, untouched ---
    let body = reqwest::get(server.url("/pages/Victim"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("original content"), "page must be unchanged");
    assert!(!body.contains("DEFACED"), "the defacement must NOT have landed");

    // --- Admin CAN mutate (the fix must not over-restrict) ---
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    let resp = admin
        .put(server.url("/pages/Victim"))
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# edited by admin"),
            ("page_cover_attachment_id", ""),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "admin PUT should succeed, got {}",
        resp.status()
    );
}
