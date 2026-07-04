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
    assert!(body.contains("/fab_web.js"), "document loads the glue: {body}");
    assert!(
        body.contains("id=\"fab-web\""),
        "document provides the bind canvas the app requires: {body}"
    );

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

    // Discover the version-pathed glue URL from the document (cache-bust: the path
    // carries the bundle version, so glue + wasm cache immutable + version-consistent).
    let doc = reqwest::get(server.url("/3d/editor"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let glue = doc
        .split("import init from '")
        .nth(1)
        .and_then(|s| s.split('\'').next())
        .expect("document imports the glue");
    assert!(
        glue.starts_with("/3d/editor/") && glue.ends_with("/fab_web.js"),
        "glue URL is version-pathed: {glue}"
    );
    // The document declares data-base = the mount dir so the app resolves lazy
    // openscad/ fetches against the versioned bundle dir, not the document URL.
    let base = glue.trim_end_matches("fab_web.js");
    assert!(
        doc.contains(&format!("data-base=\"{base}\"")),
        "document declares data-base = the mount dir: {doc}"
    );

    let js = reqwest::get(server.url(glue)).await.unwrap();
    assert_eq!(js.status(), StatusCode::OK);
    assert!(
        js.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("javascript"),
        "glue is served as javascript"
    );
    assert!(
        js.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("immutable"),
        "version-pathed glue caches immutable"
    );

    // The wasm sits alongside the glue (the glue resolves it relative to its path).
    let wasm_url = glue.replace("fab_web.js", "fab_web_bg.wasm");
    let wasm = reqwest::get(server.url(&wasm_url)).await.unwrap();
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

#[tokio::test]
async fn editor_serves_openscad_tree() {
    // Regression for the tree bug: the 0.8.0+ bundle includes an openscad/ worker +
    // wasm + libs.json the app fetches at runtime — they must serve, not 404.
    let server = spawn_test_server().await.expect("spawn");
    let doc = reqwest::get(server.url("/3d/editor")).await.unwrap().text().await.unwrap();
    let glue = doc
        .split("import init from '")
        .nth(1)
        .and_then(|s| s.split('\'').next())
        .expect("glue");
    let base = glue.trim_end_matches("fab_web.js"); // /3d/editor/<ver>/

    let worker = reqwest::get(server.url(&format!("{base}openscad/openscad-worker.js")))
        .await
        .unwrap();
    assert_eq!(worker.status(), StatusCode::OK, "openscad worker must serve, not 404");
    assert!(
        worker
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("javascript"),
        "worker served as javascript"
    );
    // The worker loads into the COEP:require-corp editor, so it must itself carry
    // require-corp (+ CORP) or the load is blocked.
    assert_eq!(
        worker
            .headers()
            .get("cross-origin-embedder-policy")
            .and_then(|v| v.to_str().ok()),
        Some("require-corp"),
        "worker script carries COEP require-corp"
    );
    assert_eq!(
        worker
            .headers()
            .get("cross-origin-resource-policy")
            .and_then(|v| v.to_str().ok()),
        Some("same-origin"),
        "worker script carries CORP"
    );

    let ow = reqwest::get(server.url(&format!("{base}openscad/openscad.wasm")))
        .await
        .unwrap();
    assert_eq!(ow.status(), StatusCode::OK, "openscad wasm must serve");
    assert_eq!(
        ow.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/wasm")
    );

    let libs = reqwest::get(server.url(&format!("{base}openscad/libs.json")))
        .await
        .unwrap();
    assert_eq!(libs.status(), StatusCode::OK, "libs.json must serve");
}

#[tokio::test]
async fn editor_wasm_identity_for_no_encoding_client() {
    // Issue-3 fix: a client accepting no compression gets the RAW wasm (gunzipped
    // from the .gz), never a mislabeled compressed blob.
    let server = spawn_test_server().await.expect("spawn");
    let doc = reqwest::get(server.url("/3d/editor")).await.unwrap().text().await.unwrap();
    let glue = doc
        .split("import init from '")
        .nth(1)
        .and_then(|s| s.split('\'').next())
        .unwrap();
    let wasm_url = glue.replace("fab_web.js", "fab_web_bg.wasm");

    let resp = reqwest::Client::new()
        .get(server.url(&wasm_url))
        .header("accept-encoding", "identity")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "identity response carries no Content-Encoding"
    );
    assert_eq!(
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/wasm")
    );
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(&bytes[..4], b"\0asm", "identity body is a raw wasm module");
}
