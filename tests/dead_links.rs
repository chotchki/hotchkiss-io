//! Dead-link admin surface (Phase DL.7) integration tests: the `/admin/dead-links`
//! report is admin-gated, groups broken links by the page that cites them, and its
//! run-scan / re-check mutations are gated too. The `deadlinks` module is
//! crate-private, so the report row is seeded via raw SQL through `server.pool`.

use hotchkiss_io::test_support::spawn_test_server;
use reqwest::{redirect::Policy, StatusCode};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn dead_links_page_is_admin_gated() {
    let server = spawn_test_server().await.expect("spawn");

    // Anonymous → 401 (missing identity, DK.2).
    let r = client().get(server.url("/admin/dead-links")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Registered → 403 (authenticated but insufficient).
    let reg = client();
    reg.post(server.url("/test/login?role=Registered")).send().await.unwrap();
    assert_eq!(
        reg.get(server.url("/admin/dead-links")).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // Admin → 200, empty state (no scan has run in the test harness).
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let r = admin.get(server.url("/admin/dead-links")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body = r.text().await.unwrap();
    assert!(body.contains("Dead links"), "renders the heading: {body}");
    assert!(
        body.contains("No broken links"),
        "empty state when nothing's been scanned: {body}"
    );
}

#[tokio::test]
async fn dead_links_view_groups_a_broken_link_under_its_page() {
    let server = spawn_test_server().await.expect("spawn");
    let page = server
        .seed_content_page("Rotted Post", "# Rotted Post\n\n[gone](https://gone.example/)")
        .await
        .expect("seed");

    // Seed a confirmed-dead link_check row + a ref to the page (the DAO is
    // crate-private, so go through raw SQL on the shared pool).
    sqlx::query(
        "INSERT INTO link_check (url, kind, last_class, last_status, detail, consecutive_failures, last_checked_at) \
         VALUES (?1, 'external', 'dead', 404, 'HTTP 404', 3, datetime('now'))",
    )
    .bind("https://gone.example/")
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO link_ref (page_id, url) VALUES (?1, ?2)")
        .bind(page.page_id)
        .bind("https://gone.example/")
        .execute(&server.pool)
        .await
        .unwrap();

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let body = admin
        .get(server.url("/admin/dead-links"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(body.contains("https://gone.example/"), "the dead url shows: {body}");
    assert!(body.contains("Confirmed dead"), "the confirmed-dead badge shows: {body}");
    // display_title() = page_title ?? first H1 ?? page_name → the H1 "Rotted Post".
    assert!(body.contains("Rotted Post"), "grouped under its page title: {body}");
    assert!(body.contains("?edit=1"), "offers an edit-page link: {body}");
    assert!(body.contains("HTTP 404"), "shows the status detail: {body}");
}

#[tokio::test]
async fn run_scan_and_recheck_are_admin_gated() {
    let server = spawn_test_server().await.expect("spawn");

    // Anonymous mutations → 401.
    assert_eq!(
        client().post(server.url("/admin/dead-links/run-scan")).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client()
            .post(server.url("/admin/dead-links/recheck"))
            .form(&[("url", "/blog")])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Run-scan spawns a background pass (the seeded content has no external links,
    // so it's DB-only + offline) and returns a refresh immediately.
    let r = admin.post(server.url("/admin/dead-links/run-scan")).send().await.unwrap();
    assert!(r.status().is_success(), "run-scan → {}", r.status());

    // Re-check an internal route (resolves in-DB, no network) → success + records it.
    let r = admin
        .post(server.url("/admin/dead-links/recheck"))
        .form(&[("url", "/blog")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "recheck → {}", r.status());

    // The re-checked internal route is now tracked as ok.
    let class: Option<String> =
        sqlx::query_scalar("SELECT last_class FROM link_check WHERE url = '/blog'")
            .fetch_optional(&server.pool)
            .await
            .unwrap();
    assert_eq!(class.as_deref(), Some("ok"), "recheck recorded /blog as ok");
}
