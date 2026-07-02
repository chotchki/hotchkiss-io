//! Smoke test for the test harness itself (Phase 8.1.2): the in-process server
//! boots, migrations run (incl. the special-pages seed), the `/` landing page
//! renders (Phase 13), and a seeded content page renders.

use hotchkiss_io::test_support::spawn_test_server;
use reqwest::StatusCode;

#[tokio::test]
async fn harness_boots_and_serves() {
    let server = spawn_test_server().await.expect("spawn test server");

    // `/` serves the featured landing (Phase 13 — no longer a redirect): 200 with
    // the pillar doors routing to the special pages the migrations seeded.
    let resp = reqwest::get(server.url("/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("href=\"/projects\""),
        "landing should render the pillar doors; body starts: {}",
        &body[..body.len().min(200)]
    );

    // a seeded content page renders its markdown
    server
        .seed_content_page("HarnessSmoke", "# hello harness")
        .await
        .expect("seed");
    let resp = reqwest::get(server.url("/pages/HarnessSmoke")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("hello harness"),
        "rendered page should contain the markdown text; body starts: {}",
        &body[..body.len().min(200)]
    );
    // The jumbotron portrait links home (Phase 13 — the route back to the landing).
    assert!(
        body.contains("aria-label=\"Home\""),
        "the header portrait should link home on every page"
    );
}

/// End-to-end proof of the Phase CN build-time icon codegen: the 404 cat page
/// calls `icons::house()`, a build.rs-generated askama macro emitting an inline
/// `<svg class="icon">`. A real render exercises the whole pipeline (vendored SVG
/// → codegen → macro → page) and confirms FontAwesome is gone.
#[tokio::test]
async fn build_time_icon_codegen_renders_inline_svg() {
    let server = spawn_test_server().await.expect("spawn test server");

    // Any unmatched route renders the 404 cat page.
    let resp = reqwest::get(server.url("/no-such-route-xyz")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("<svg class=\"icon\""),
        "404 page should carry a build-time inline SVG icon; body starts: {}",
        &body[..body.len().min(400)]
    );
    assert!(
        !body.contains("fa-solid") && !body.contains("fontawesome"),
        "FontAwesome should be fully gone from the rendered page"
    );
}
