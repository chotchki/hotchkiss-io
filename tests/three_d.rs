//! Phase CW (gallery half) — the `/3d` gallery scaffold: the special-page tab, the
//! index (Featured band + grid), model pages nesting under `/pages/3d/<slug>`, and
//! the guarantee that 3D never leaks onto the home page.

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
async fn three_d_tab_and_empty_index() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = reqwest::get(server.url("/3d")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("No models yet"), "empty state: {body}");
    // The 3d nav tab renders (links to /pages/3d, which the special page redirects to /3d).
    assert!(body.contains("href=\"/pages/3d\""), "3d nav tab present: {body}");
}

#[tokio::test]
async fn three_d_model_nests_and_renders() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_3d_model("widget", "# Widget\n\nA printed widget.")
        .await
        .expect("seed");

    let index = reqwest::get(server.url("/3d")).await.unwrap().text().await.unwrap();
    assert!(
        index.contains("href=\"/pages/3d/widget\""),
        "model card links the detail page: {index}"
    );
    assert!(index.contains("Widget"), "model title on the card: {index}");

    // Detail page nests under the content tree, served by the EXISTING /pages route.
    let detail = reqwest::get(server.url("/pages/3d/widget")).await.unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert!(
        detail.text().await.unwrap().contains("A printed widget."),
        "the model detail page renders"
    );
}

#[tokio::test]
async fn three_d_models_stay_off_home() {
    // A 3D model — even featured/pinned — must NOT appear on the home page (3d
    // doesn't belong in /). Home only fetches blog+projects, so the reused
    // `featured` tag can't leak a model into the home Featured band.
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_3d_model("offhome-model", "# Off Home\n\nbody")
        .await
        .expect("seed");
    // Pin it — the same tag that surfaces posts/projects on the home Featured band.
    sqlx::query("UPDATE content_pages SET page_category = 'featured' WHERE page_name = 'offhome-model'")
        .execute(&server.pool)
        .await
        .unwrap();

    let home = reqwest::get(server.url("/")).await.unwrap().text().await.unwrap();
    assert!(
        !home.contains("offhome-model"),
        "a pinned 3D model must NOT appear on home: {home}"
    );

    // ...but it IS featured on the 3D index (the tag reuse works, scoped).
    let three_d = reqwest::get(server.url("/3d")).await.unwrap().text().await.unwrap();
    assert!(
        three_d.contains("offhome-model"),
        "the pinned model shows on the 3D index: {three_d}"
    );
    assert!(three_d.contains("Featured"), "the 3D index shows a Featured band");
}

#[tokio::test]
async fn three_d_create_form_admin_only() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let body = admin.get(server.url("/3d")).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("New model"), "admin sees the create form: {body}");

    let anon = reqwest::get(server.url("/3d")).await.unwrap().text().await.unwrap();
    assert!(!anon.contains("New model"), "anon must not see the create form");
}

#[tokio::test]
async fn editor_route_is_cross_origin_isolated_and_confined() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = reqwest::get(server.url("/3d/editor")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers().clone();
    assert_eq!(
        h.get("cross-origin-opener-policy").and_then(|v| v.to_str().ok()),
        Some("same-origin"),
        "editor document carries COOP"
    );
    assert_eq!(
        h.get("cross-origin-embedder-policy").and_then(|v| v.to_str().ok()),
        Some("require-corp"),
        "editor document carries COEP"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("/3d/editor/fab_web.js"), "document loads the glue: {body}");

    // The isolation must NOT bleed onto the rest of the site.
    let home = reqwest::get(server.url("/")).await.unwrap();
    assert!(
        home.headers().get("cross-origin-embedder-policy").is_none(),
        "the home page must NOT be cross-origin isolated"
    );
    let gallery = reqwest::get(server.url("/3d")).await.unwrap();
    assert!(
        gallery.headers().get("cross-origin-embedder-policy").is_none(),
        "the 3D gallery index must NOT be cross-origin isolated"
    );
}

#[tokio::test]
async fn editor_serves_glue_and_wasm() {
    let server = spawn_test_server().await.expect("spawn");

    let js = reqwest::get(server.url("/3d/editor/fab_web.js")).await.unwrap();
    assert_eq!(js.status(), StatusCode::OK);
    assert!(
        js.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("javascript"),
        "glue is served as javascript"
    );

    let wasm = reqwest::get(server.url("/3d/editor/fab_web_bg.wasm")).await.unwrap();
    assert_eq!(wasm.status(), StatusCode::OK);
    assert_eq!(
        wasm.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/wasm"),
        "wasm carries the exact MIME instantiateStreaming needs"
    );
    let bytes = wasm.bytes().await.unwrap();
    assert!(
        bytes.len() > 1_000_000,
        "wasm body is substantial (got {} bytes)",
        bytes.len()
    );
}
