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
async fn admin_pages_editor_requires_admin() {
    let server = spawn_test_server().await.expect("spawn");

    // anonymous → 403 (GET is public site-wide, but /admin is require_admin-gated)
    let resp = client()
        .get(server.url("/admin/pages"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Admin → 200, the dedicated page-management view renders
    let admin = client();
    let resp = admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = admin
        .get(server.url("/admin/pages"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Manage Pages"));
    // On the admin page the Admin nav tab renders active (no content tab is named
    // "admin", so the only active-yellow indicator is the Admin tab itself).
    assert!(body.contains("border-b-yellow"));
}

#[tokio::test]
async fn reorder_pages_requires_admin() {
    let server = spawn_test_server().await.expect("spawn");

    // anonymous POST → 403 (non-GET requires admin; reorder is not in the allowlist)
    let resp = client()
        .post(server.url("/admin/pages/reorder"))
        .form(&[("page_id", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Admin → 200; the seeded special pages (ids 1,2) reorder cleanly
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    let resp = admin
        .post(server.url("/admin/pages/reorder"))
        .form(&[("page_id", "2"), ("page_id", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // an id that isn't a current top-level page → 400 (the write is scoped)
    let resp = admin
        .post(server.url("/admin/pages/reorder"))
        .form(&[("page_id", "999999")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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

/// 14.6: the /projects index renders project cards (mirroring the blog index) —
/// display title + excerpt, linking to the project page.
#[tokio::test]
async fn projects_index_lists_seeded_project() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_project(
            "recon-gen",
            "# Recon Gen\n\nAn open-source financial-validation platform.",
        )
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/projects"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("href=\"/pages/projects/recon-gen\""),
        "card must link the project page: {body}"
    );
    assert!(
        body.contains("Recon Gen"),
        "card must show the display title (not the slug): {body}"
    );
    assert!(
        body.contains("An open-source financial-validation platform."),
        "card must show the excerpt: {body}"
    );
}

/// BV: a content page carries math + code SOURCE in the served HTML (no-JS /
/// crawler / LLM readable) and loads the KaTeX + highlight.js assets that
/// typeset/highlight them client-side.
#[tokio::test]
async fn content_page_carries_math_and_code_source_plus_assets() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page(
            "MathPage",
            "Drift is $$d = b - c$$ inline.\n\n```rust\nlet x = 1;\n```\n",
        )
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/pages/MathPage"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // math source-in-HTML
    assert!(
        body.contains("class=\"math math-inline\""),
        "math span missing: {body}"
    );
    assert!(
        body.contains("d = b - c"),
        "the TeX source must be in the served HTML: {body}"
    );
    // fenced code keeps its language class for highlight.js
    assert!(
        body.contains("language-rust"),
        "code language class missing: {body}"
    );
    // the typeset + highlight assets load
    assert!(
        body.contains("/vendor/katex/katex.min.css"),
        "katex css not loaded: {body}"
    );
    assert!(
        body.contains("/scripts/katex-render.js"),
        "katex init not loaded: {body}"
    );
    assert!(
        body.contains("/vendor/highlightjs/highlight.min.js"),
        "highlight.js not loaded: {body}"
    );
    assert!(
        body.contains("/scripts/code-highlight.js"),
        "code-highlight init not loaded: {body}"
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

/// Phase BU: saving a page rewrites absolute same-site links to root-relative
/// (the seam that hid the beta bug — `AppState.site_host` wiring), while leaving
/// external links untouched. The test server's `site_host` is "hotchkiss.io".
#[tokio::test]
async fn save_relativizes_same_site_links() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("RelTest", "# placeholder")
        .await
        .expect("seed");

    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    let resp = admin
        .put(server.url("/pages/RelTest"))
        .form(&[
            ("page_category", ""),
            (
                "page_markdown",
                "[post](https://hotchkiss.io/blog/foo) and [ext](https://github.com/chotchki/recon-gen)",
            ),
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

    let body = reqwest::get(server.url("/pages/RelTest"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // The same-site link was relativized on save...
    assert!(
        body.contains("href=\"/blog/foo\""),
        "same-site link should be root-relative: {body}"
    );
    assert!(
        !body.contains("https://hotchkiss.io/blog/foo"),
        "the absolute same-site URL must be gone: {body}"
    );
    // ...and the external link was left untouched.
    assert!(
        body.contains("href=\"https://github.com/chotchki/recon-gen\""),
        "external link must stay absolute: {body}"
    );
}

/// Phase E: the fail-closed layer gates non-GET site-wide — not just /pages —
/// now that the per-handler is_admin checks are gone; and it lets the anonymous
/// WebAuthn ceremony POSTs through (the exact-tuple allowlist).
#[tokio::test]
async fn mutation_layer_gates_all_nests_and_allows_auth_ceremony() {
    let server = spawn_test_server().await.expect("spawn");
    let anon = client();

    // A different nest (attachments) with no per-handler check anymore.
    let resp = anon
        .delete(server.url("/attachments/1/whatever.png"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "anon attachment delete must be blocked by the layer"
    );

    // An exotic verb (PATCH) — proves default-DENY (allow safe methods), not a
    // deny-list of {POST,PUT,DELETE} that would let PATCH slip.
    let resp = anon
        .patch(server.url("/pages/preview"))
        .form(&[("page_markdown", "# x")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "anon PATCH preview must be blocked by the layer"
    );

    // The WebAuthn ceremony POST is allowlisted: the layer must NOT block it. It
    // reaches its handler (which then fails for lack of ceremony state) — the
    // point is it is not the layer's 'Admin only' 403.
    let resp = anon
        .post(server.url("/login/finish_authentication"))
        .body("{}")
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "the login ceremony must not be gated by the admin layer"
    );
}

/// The auth 403 is a styled "How about NO!" page on a full-page navigation (e.g.
/// hitting an admin page after the session died), with a login link.
#[tokio::test]
async fn forbidden_full_nav_renders_styled_page() {
    let server = spawn_test_server().await.expect("spawn");
    let anon = client();
    let resp = anon
        .get(server.url("/admin/analytics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = resp.text().await.unwrap();
    assert!(body.contains("How about NO!"), "styled 403 page: {body}");
    assert!(body.contains("/login"), "403 offers a login link: {body}");
}

/// An HTMX mutation that 403s (session died mid-edit) gets an HX-Redirect to
/// /login instead of a full HTML doc swapped into a fragment target.
#[tokio::test]
async fn forbidden_htmx_request_gets_hx_redirect() {
    let server = spawn_test_server().await.expect("spawn");
    let anon = client();
    let resp = anon
        .post(server.url("/pages/anything"))
        .header("HX-Request", "true")
        .form(&[("page_title", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        resp.headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/login"),
        "an HTMX mutation 403 redirects to login"
    );
}

/// Phase CA: an `Authorization: Bearer hio_…` API key authenticates as its user,
/// so the fail-closed layer lets a mutation through; anon + bogus keys still 403.
#[tokio::test]
async fn api_key_authenticates_a_mutation() {
    let server = spawn_test_server().await.expect("spawn");
    let key = server.seed_admin_api_key("ci").await.expect("seed key");
    let c = client();

    // Anon mutation → blocked by the fail-closed layer.
    let resp = c
        .post(server.url("/pages/nope"))
        .form(&[("page_title", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "anon mutation blocked");

    // Same mutation WITH the key → the layer lets it through (reaches the handler,
    // which 404s on the missing parent — the point is it is NOT the 403).
    let resp = c
        .post(server.url("/pages/nope"))
        .header("Authorization", format!("Bearer {key}"))
        .form(&[("page_title", "x")])
        .send()
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "the API key authorizes the mutation"
    );

    // A bogus key injects nothing → still blocked.
    let resp = c
        .post(server.url("/pages/nope"))
        .header("Authorization", "Bearer hio_bogus")
        .form(&[("page_title", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a bogus key is not authorized"
    );
}

/// Phase F authoring flow: create-by-title auto-slugs the URL; an admin lands on
/// the clean reader view with an Edit toggle; ?edit reveals the editor; anon sees
/// neither.
#[tokio::test]
async fn admin_authoring_flow_title_slug_and_edit_toggle() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    // Create by TITLE — the server auto-slugs the URL from it.
    let resp = admin
        .post(server.url("/pages"))
        .form(&[("page_title", "Hello World Post")])
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().as_u16() < 400,
        "create should succeed, got {}",
        resp.status()
    );

    // The auto-slugged page exists; display_title renders as the page H1; and the
    // admin sees the clean reader view (Edit toggle), NOT the editor.
    let resp = admin
        .get(server.url("/pages/hello-world-post"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "auto-slugged page should exist");
    let body = resp.text().await.unwrap();
    assert!(body.contains("Hello World Post"), "title should render");
    assert!(body.contains("Edit this page"), "admin sees the edit toggle");
    assert!(
        !body.contains("Page Editor"),
        "editor must be hidden in the reader view"
    );

    // ?edit reveals the editor.
    let body = admin
        .get(server.url("/pages/hello-world-post?edit=1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Page Editor"), "?edit reveals the editor");
    assert!(
        body.contains("name=\"page_markdown\""),
        "editor has the markdown field"
    );

    // Anonymous sees the title but neither the editor nor the toggle.
    let body = reqwest::get(server.url("/pages/hello-world-post"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Hello World Post"), "anon sees the title");
    assert!(
        !body.contains("Edit this page"),
        "anon must not see the edit toggle"
    );
}

/// BX: a blog post in the middle of the timeline shows BOTH a Previous (older)
/// and a Next (newer) card linking the adjacent posts. Posts are seeded
/// oldest→newest, so the newest-first order is [third, second, first].
#[tokio::test]
async fn blog_post_middle_shows_prev_and_next() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("first-post", "the oldest").await.expect("seed");
    server.seed_blog_post("second-post", "the middle").await.expect("seed");
    server.seed_blog_post("third-post", "the newest").await.expect("seed");

    let body = reqwest::get(server.url("/blog/second-post"))
        .await.unwrap().text().await.unwrap();

    assert!(body.contains("fa-arrow-left"), "Previous card missing: {body}");
    assert!(body.contains("fa-arrow-right"), "Next card missing: {body}");
    assert!(
        body.contains("href=\"/blog/first-post\""),
        "Previous should link the older post: {body}"
    );
    assert!(
        body.contains("href=\"/blog/third-post\""),
        "Next should link the newer post: {body}"
    );
}

/// BX: the newest post has no Next, the oldest has no Previous — a side is
/// omitted at each end.
#[tokio::test]
async fn blog_post_ends_omit_one_side() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("first-post", "the oldest").await.expect("seed");
    server.seed_blog_post("second-post", "the middle").await.expect("seed");
    server.seed_blog_post("third-post", "the newest").await.expect("seed");

    // Newest post: a Previous card (→ second) only, no Next.
    let newest = reqwest::get(server.url("/blog/third-post"))
        .await.unwrap().text().await.unwrap();
    assert!(newest.contains("fa-arrow-left"), "newest should have a Previous card: {newest}");
    assert!(!newest.contains("fa-arrow-right"), "newest must NOT have a Next card: {newest}");
    assert!(newest.contains("href=\"/blog/second-post\""), "Previous → second: {newest}");

    // Oldest post: a Next card (→ second) only, no Previous.
    let oldest = reqwest::get(server.url("/blog/first-post"))
        .await.unwrap().text().await.unwrap();
    assert!(oldest.contains("fa-arrow-right"), "oldest should have a Next card: {oldest}");
    assert!(!oldest.contains("fa-arrow-left"), "oldest must NOT have a Previous card: {oldest}");
    assert!(oldest.contains("href=\"/blog/second-post\""), "Next → second: {oldest}");
}

/// BX: the next/previous nav is blog-only — a regular /pages page (same
/// template) shows neither card.
#[tokio::test]
async fn regular_page_has_no_post_nav() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_content_page("about", "# About\n\nJust a page.").await.expect("seed");

    let body = reqwest::get(server.url("/pages/about"))
        .await.unwrap().text().await.unwrap();
    assert!(!body.contains("fa-arrow-left"), "no Previous card on a /pages page: {body}");
    assert!(!body.contains("fa-arrow-right"), "no Next card on a /pages page: {body}");
}

/// Phase 16.3: /resume renders the résumé content (the newest child of the
/// `resume` special page) with the title from its leading H1 + a PDF download link.
#[tokio::test]
async fn resume_page_renders_with_pdf_link() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_resume("# Christopher Hotchkiss\n\nSoftware architect and systems engineer.")
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/resume"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Christopher Hotchkiss"), "résumé title (from H1): {body}");
    assert!(body.contains("Software architect"), "résumé body: {body}");
    assert!(
        body.contains("href=\"/resume.pdf\""),
        "the PDF download link must be present: {body}"
    );
}

/// Phase 16.4: /resume.pdf generates a real PDF from the same markdown via
/// weasyprint. weasyprint is a real dependency (like d2) — skip the strict check
/// if it isn't installed so the suite still passes on a box without it.
#[tokio::test]
async fn resume_pdf_is_a_real_pdf() {
    let have_weasyprint = std::process::Command::new("weasyprint")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || ["/opt/homebrew/bin/weasyprint", "/usr/local/bin/weasyprint"]
            .iter()
            .any(|p| std::path::Path::new(p).exists());
    if !have_weasyprint {
        eprintln!("weasyprint not installed — skipping /resume.pdf assertion");
        return;
    }

    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_resume("# Christopher Hotchkiss\n\nSoftware architect and systems engineer.")
        .await
        .expect("seed");

    let resp = reqwest::get(server.url("/resume.pdf")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.contains("application/pdf"), "content-type was: {ct}");
    let bytes = resp.bytes().await.unwrap();
    assert!(
        bytes.starts_with(b"%PDF"),
        "body should be a PDF (got {} bytes)",
        bytes.len()
    );
}

/// CA: the two 404 code-paths — an unmatched route (global fallback) AND a
/// missing `/pages/<slug>` (the dead `/pages/Resume` link that prompted this) —
/// both render the shared "blame the cat" page with a real 404 status.
#[tokio::test]
async fn not_found_renders_cat_page_on_both_paths() {
    let server = spawn_test_server().await.expect("spawn");

    for path in [
        "/definitely-not-a-real-route",
        "/pages/this-page-does-not-exist",
    ] {
        let resp = reqwest::get(server.url(path)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path} should be a 404");
        let body = resp.text().await.unwrap();
        assert!(
            body.contains("Which one is guilty"),
            "{path} should render the cat 404: {body}"
        );
        assert!(
            body.contains("blame_hobbes.avif"),
            "{path} should show the suspect lineup: {body}"
        );
        // the orange-cat verdict is the punchline — confirm the quips render
        assert!(
            body.contains("Not Guilty Due to Orange Cat"),
            "{path} should carry the verdicts: {body}"
        );
    }
}

/// BZ: the media upload → serve → embed vertical. Admin uploads an image; it's
/// stored content-addressed + ffprobe'd, listed in the library, and renders as
/// an `<img>` whose bytes serve from the HMAC-keyed `url_key` (never the sha).
/// Needs ffprobe; skips where absent.
#[tokio::test]
async fn media_upload_serve_and_embed_vertical() {
    let has_ffprobe = std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_ffprobe {
        eprintln!("skipping media vertical test: ffprobe not found");
        return;
    }

    let server = spawn_test_server().await.expect("spawn");

    // Management is admin-gated: anonymous GET + upload → 403.
    assert_eq!(
        client().get(server.url("/admin/media")).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client().post(server.url("/admin/media/upload")).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // Admin uploads a real image (the committed cat AVIF).
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/images/404/blame_bonnie.avif");
    let bytes = std::fs::read(&fixture).expect("read avif fixture");
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(bytes)
            .file_name("bonnie.avif")
            .mime_str("image/avif")
            .unwrap(),
    );
    let resp = admin
        .post(server.url("/admin/media/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin upload should succeed");
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["media_ref"].as_str().expect("media_ref in response").to_string();
    assert!(!media_ref.is_empty());

    // The library lists it.
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    assert!(lib.contains(&media_ref), "library should list {media_ref}");

    // Rename → the new title shows in the library.
    let media_id = j["media_id"].as_i64().expect("media_id in response");
    let resp = admin
        .post(server.url(&format!("/admin/media/{media_id}/rename")))
        .form(&[("title", "Bonnie Mugshot")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    assert!(lib.contains("Bonnie Mugshot"), "renamed title should show");

    // Add-encode a second file (separate upload) → 200; re-adding the same bytes
    // dedups (still 200, idempotent — not a 400).
    let hobbes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/images/404/blame_hobbes.avif"),
    )
    .unwrap();
    for _ in 0..2 {
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(hobbes.clone())
                .file_name("hobbes.avif")
                .mime_str("image/avif")
                .unwrap(),
        );
        let resp = admin
            .post(server.url(&format!("/admin/media/{media_id}/encode")))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "add-encode (and dedup re-add) should be OK");
    }

    // Per-stream delete: pull a variant delete link from the library and use it.
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    let variant_id = lib
        .split("/admin/media/variant/")
        .nth(1)
        .expect("a variant delete link in the library")
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let resp = admin
        .delete(server.url(&format!("/admin/media/variant/{variant_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "per-stream delete should succeed");

    // The embed renders an <img> pointing at /media/file/<url_key>.
    let embed = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(embed.contains("<img"), "image embeds an <img>: {embed}");
    let url_key = embed
        .split("/media/file/")
        .nth(1)
        .expect("embed carries a file url")
        .split('"')
        .next()
        .unwrap()
        .to_string();

    // The bytes serve from the HMAC token.
    let served = reqwest::get(server.url(&format!("/media/file/{url_key}"))).await.unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    assert!(!served.bytes().await.unwrap().is_empty(), "served bytes non-empty");

    // Range + gzip: media must serve 206 and must NOT be compressed — gzipping a
    // range response corrupts the byte ranges the browser seeks with (the cause
    // of jerky video). This guards the serve route end-to-end.
    let ranged = reqwest::Client::new()
        .get(server.url(&format!("/media/file/{url_key}")))
        .header("Range", "bytes=0-9")
        .header("Accept-Encoding", "gzip, br")
        .send()
        .await
        .unwrap();
    assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT, "range request → 206");
    assert!(
        ranged.headers().get("content-encoding").is_none(),
        "media must not be compressed (breaks ranges)"
    );
    let cr = ranged
        .headers()
        .get("content-range")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cr.starts_with("bytes 0-9/"), "content-range was: {cr}");
    assert_eq!(ranged.bytes().await.unwrap().len(), 10, "exactly the requested 10 bytes");

    // A junk token is a clean 404 — no existence oracle.
    let bad = reqwest::get(server.url(&format!("/media/file/{}", "0".repeat(64))))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::NOT_FOUND);
}

/// Phase CI: a LARGE, non-A/V upload streams to disk (multi-chunk, never buffered
/// whole), ingests as `MediaKind::File` (ffprobe can't type it → graceful fallback,
/// not a rejection), and serves back BYTE-FOR-BYTE from the HMAC share link. The
/// ~3 MB size just exercises the chunked path — the RAM ceiling is the same code at
/// any size. Needs ffprobe (the ingest probes); skips where absent.
#[tokio::test]
async fn media_streams_large_generic_file_and_serves_it_back() {
    let has_ffprobe = std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_ffprobe {
        eprintln!("skipping streaming-upload test: ffprobe not found");
        return;
    }

    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // ~3 MB of deterministic non-media bytes — not an image/video/STL, so ffprobe
    // can't type it and it must ingest as a generic File.
    let payload: Vec<u8> = (0..3_000_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(payload.clone())
            .file_name("demo-build.bin")
            .mime_str("application/octet-stream")
            .unwrap(),
    );
    let resp = admin
        .post(server.url("/admin/media/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "streaming upload should succeed");
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["media_ref"].as_str().expect("media_ref").to_string();

    // It ingested as a generic File → the embed is a download <a>, not an img/video.
    let embed = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(embed.contains("<a "), "generic file embeds a download link: {embed}");
    let url_key = embed
        .split("/media/file/")
        .nth(1)
        .expect("embed carries a file url")
        .split('"')
        .next()
        .unwrap()
        .to_string();

    // The share link serves the bytes back BYTE-FOR-BYTE (streaming write +
    // ServeFile round-trip).
    let served = reqwest::get(server.url(&format!("/media/file/{url_key}"))).await.unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    let got = served.bytes().await.unwrap();
    assert_eq!(got.len(), payload.len(), "served length matches uploaded");
    assert_eq!(got.as_ref(), payload.as_slice(), "served bytes identical to uploaded");

    // The library offers a "Copy link" affordance carrying that share url_key.
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    assert!(lib.contains("copy-link"), "library offers a copy-link button");
    assert!(lib.contains(&url_key), "library card carries the share url_key");
}

/// Phase CJ: an uploaded variant records WHICH media root holds its bytes
/// (the storage_root hint), and the serve route resolves through it end-to-end.
/// The multi-root resolve / dedup / free-space-fallback logic is unit-tested in
/// src/media; this checks the hint is persisted + served. Needs ffprobe.
#[tokio::test]
async fn media_records_storage_root_hint() {
    let has_ffprobe = std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_ffprobe {
        eprintln!("skipping storage_root hint test: ffprobe not found");
        return;
    }

    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"storage-root hint bytes".to_vec())
            .file_name("hint.bin")
            .mime_str("application/octet-stream")
            .unwrap(),
    );
    let resp = admin
        .post(server.url("/admin/media/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_id = j["media_id"].as_i64().unwrap();
    let media_ref = j["media_ref"].as_str().unwrap().to_string();

    // The variant carries a non-NULL storage_root pointing at a real media root dir.
    let row = sqlx::query("SELECT storage_root FROM media_variant WHERE media_id = ?1")
        .bind(media_id)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let storage_root: Option<String> = row.get("storage_root");
    let root = storage_root.expect("storage_root hint persisted on the variant");
    assert!(
        std::path::Path::new(&root).is_dir(),
        "storage_root hint points at a real media root dir: {root}"
    );

    // And it serves back through that hint.
    let embed = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let url_key = embed
        .split("/media/file/")
        .nth(1)
        .expect("embed carries a file url")
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let served = reqwest::get(server.url(&format!("/media/file/{url_key}"))).await.unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(
        served.bytes().await.unwrap().as_ref(),
        b"storage-root hint bytes"
    );
}

// ───────────────────────── Phase CB: unified feed + SEO ─────────────────────────

#[tokio::test]
async fn unified_feed_includes_blog_and_project() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post("feed-post", "A blog body.")
        .await
        .expect("seed post");
    server
        .seed_project("the-widget", "A project body.")
        .await
        .expect("seed project");

    let resp = reqwest::get(server.url("/feed.xml")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("atom"), "unexpected content-type: {ct}");

    let body = resp.text().await.unwrap();
    assert!(body.starts_with("<?xml"), "not xml: {body}");
    assert!(
        body.contains("<title>Christopher Hotchkiss</title>"),
        "feed retitled to the unified site title"
    );
    // Blog post entry, linked under /blog.
    assert!(body.contains("/blog/feed-post"), "blog entry url missing");
    assert!(
        body.contains("<category term=\"blog\""),
        "blog entry category missing"
    );
    // Project page entry, linked at the real /pages/projects/<slug> route.
    assert!(
        body.contains("/pages/projects/the-widget"),
        "project entry url must be /pages/projects/<slug>: {body}"
    );
    assert!(
        body.contains("<category term=\"projects\""),
        "project entry category missing"
    );
}

#[tokio::test]
async fn blog_feed_alias_still_serves_unified_feed() {
    // /blog/feed.xml is kept for back-compat and now serves the SAME unified feed,
    // so a project page shows up there too.
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_project("legacy-alias", "body")
        .await
        .expect("seed project");

    let body = reqwest::get(server.url("/blog/feed.xml"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("/pages/projects/legacy-alias"),
        "the /blog/feed.xml alias should carry projects too: {body}"
    );
}

#[tokio::test]
async fn sitemap_lists_home_pages_blog_and_projects() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("about-me", "# About\n\nbody")
        .await
        .expect("seed page");
    server
        .seed_blog_post("hello-world", "post body")
        .await
        .expect("seed post");
    server
        .seed_project("the-widget", "project body")
        .await
        .expect("seed project");

    let resp = reqwest::get(server.url("/sitemap.xml")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("xml"), "unexpected content-type: {ct}");

    let body = resp.text().await.unwrap();
    assert!(body.contains("<urlset"), "not a sitemap: {body}");
    // Home + the section indexes + the seeded leaves are all present.
    assert!(body.contains("<loc>http://localhost"), "absolute locs");
    assert!(body.contains("/pages/about-me</loc>"), "content page");
    assert!(body.contains("/blog/hello-world</loc>"), "blog post");
    assert!(body.contains("/pages/projects/the-widget</loc>"), "project page");
    assert!(body.contains("<lastmod>"), "lastmod present");
    // Special redirect rows are NOT exposed as /pages/<slug>.
    assert!(
        !body.contains("/pages/blog</loc>"),
        "special pages must not leak as /pages/<slug>"
    );
}

#[tokio::test]
async fn robots_txt_canonical_host_has_sitemap_directive() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = reqwest::get(server.url("/robots.txt")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.contains("text/plain"), "unexpected content-type: {ct}");

    let body = resp.text().await.unwrap();
    assert!(body.contains("Sitemap:"), "missing Sitemap directive: {body}");
    assert!(body.contains("/sitemap.xml"), "sitemap url missing");
    assert!(body.contains("Disallow: /admin/"), "admin should be hidden");
    assert!(body.contains("Allow: /"), "canonical host should allow crawling");
}

#[tokio::test]
async fn robots_txt_deindexes_non_canonical_beta_host() {
    // A request whose Host isn't the canonical site host (e.g. beta.hotchkiss.io)
    // must get a blanket Disallow so the ephemeral beta copy isn't indexed.
    let server = spawn_test_server().await.expect("spawn");
    let body = client()
        .get(server.url("/robots.txt"))
        .header("Host", "beta.hotchkiss.io")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Disallow: /"), "beta should be fully disallowed: {body}");
    assert!(
        !body.contains("Sitemap:"),
        "a disallowed host shouldn't advertise a sitemap"
    );
}

#[tokio::test]
async fn content_page_carries_seo_meta() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("about-me", "# About Me\n\nI build robust, typed systems.")
        .await
        .expect("seed");

    let body = reqwest::get(server.url("/pages/about-me"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("<meta name=\"description\""),
        "per-page meta description missing"
    );
    // Canonical is the CANONICAL host (hotchkiss.io), not the served localhost.
    assert!(
        body.contains("<link rel=\"canonical\" href=\"https://hotchkiss.io/pages/about-me\""),
        "canonical url wrong/missing: {body}"
    );
    assert!(
        body.contains("property=\"og:title\""),
        "OpenGraph title missing"
    );
    assert!(
        body.contains("name=\"twitter:card\""),
        "twitter card missing"
    );
    // The description should reflect the page body (excerpt), not just the default.
    assert!(
        body.contains("I build robust, typed systems."),
        "description should derive from the page excerpt: {body}"
    );
}

// ───────────────── Phase CC: live role enforcement (cookie sessions) ─────────────────

#[tokio::test]
async fn role_change_takes_effect_on_live_session() {
    // An admin's cookie session stores a snapshot of their role; the
    // refresh_session_role middleware must re-check the DB each request so a
    // demote bites WITHOUT a re-login.
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    // The gate is open while they're Admin.
    let ok = admin.get(server.url("/admin/analytics")).send().await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    // Demote them in the DB (out of band, same as another admin would).
    sqlx::query("UPDATE users SET app_role = 'Registered' WHERE display_name = 'test-Admin'")
        .execute(&server.pool)
        .await
        .unwrap();

    // Same cookie, next request → the live recheck sees Registered → 403.
    let denied = admin.get(server.url("/admin/analytics")).send().await.unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::FORBIDDEN,
        "a demoted admin must lose access on their live session"
    );
}

#[tokio::test]
async fn deleted_user_loses_access_on_live_session() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        admin
            .get(server.url("/admin/analytics"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // Delete the user out from under the live session.
    sqlx::query("DELETE FROM users WHERE display_name = 'test-Admin'")
        .execute(&server.pool)
        .await
        .unwrap();

    // The middleware downgrades a deleted user to Anonymous → 403.
    let denied = admin.get(server.url("/admin/analytics")).send().await.unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::FORBIDDEN,
        "a deleted user must lose access on their live session"
    );
}

#[tokio::test]
async fn session_cookie_is_http_only_and_samesite() {
    // The session cookie (the only cookie the app sets) must be HttpOnly +
    // SameSite. Secure isn't asserted here — the test harness is plain HTTP, so
    // Secure is off by design in debug (on in release).
    let server = spawn_test_server().await.expect("spawn");
    let resp = client()
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .expect("login sets a session cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set_cookie.to_lowercase().contains("httponly"),
        "session cookie must be HttpOnly: {set_cookie}"
    );
    assert!(
        set_cookie.to_lowercase().contains("samesite"),
        "session cookie must set SameSite: {set_cookie}"
    );
}

// ───────────────────────── Phase CC: /admin/users management ─────────────────────────

/// The display_name `/test/login?role=Admin` seeds — used to grab the live
/// admin's id from the DB.
async fn admin_user_id(server: &hotchkiss_io::test_support::TestServer) -> String {
    sqlx::query("SELECT id FROM users WHERE display_name = 'test-Admin'")
        .fetch_one(&server.pool)
        .await
        .unwrap()
        .get::<String, _>("id")
}

#[tokio::test]
async fn admin_users_list_requires_admin() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_user("alice", "Registered").await.unwrap();

    // Anonymous → 403.
    assert_eq!(
        client().get(server.url("/admin/users")).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // Registered → 403.
    let reg = client();
    reg.post(server.url("/test/login?role=Registered")).send().await.unwrap();
    assert_eq!(
        reg.get(server.url("/admin/users")).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // Admin → 200, listing both alice and the test admin.
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let resp = admin.get(server.url("/admin/users")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("alice"), "list should show seeded user");
    assert!(body.contains("test-Admin"), "list should show the admin");
}

#[tokio::test]
async fn admin_can_promote_and_demote() {
    let server = spawn_test_server().await.expect("spawn");
    let alice = server.seed_user("alice", "Registered").await.unwrap();
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Promote alice → Admin.
    let r = admin
        .post(server.url(&format!("/admin/users/{alice}/role")))
        .form(&[("role", "Admin")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "promote should succeed: {}", r.status());
    let role: String = sqlx::query("SELECT app_role FROM users WHERE display_name = 'alice'")
        .fetch_one(&server.pool).await.unwrap().get("app_role");
    assert_eq!(role, "Admin");

    // Demote alice → Registered (test-Admin keeps the floor, so it's allowed).
    let r = admin
        .post(server.url(&format!("/admin/users/{alice}/role")))
        .form(&[("role", "Registered")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "demote should succeed: {}", r.status());
    let role: String = sqlx::query("SELECT app_role FROM users WHERE display_name = 'alice'")
        .fetch_one(&server.pool).await.unwrap().get("app_role");
    assert_eq!(role, "Registered");
}

#[tokio::test]
async fn last_admin_protected_from_demote_and_delete() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let id = admin_user_id(&server).await;

    // Demoting the sole admin → 409.
    let r = admin
        .post(server.url(&format!("/admin/users/{id}/role")))
        .form(&[("role", "Registered")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT, "can't demote the last admin");

    // Deleting the sole admin → 409.
    let r = admin.delete(server.url(&format!("/admin/users/{id}"))).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT, "can't delete the last admin");

    // Still admin, still here.
    let role: String = sqlx::query("SELECT app_role FROM users WHERE display_name = 'test-Admin'")
        .fetch_one(&server.pool).await.unwrap().get("app_role");
    assert_eq!(role, "Admin");
}

#[tokio::test]
async fn admin_can_delete_a_user() {
    let server = spawn_test_server().await.expect("spawn");
    let alice = server.seed_user("alice", "Registered").await.unwrap();
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let r = admin.delete(server.url(&format!("/admin/users/{alice}"))).send().await.unwrap();
    assert!(r.status().is_success(), "delete should succeed: {}", r.status());

    let count: i64 = sqlx::query("SELECT COUNT(*) as c FROM users WHERE display_name = 'alice'")
        .fetch_one(&server.pool).await.unwrap().get("c");
    assert_eq!(count, 0, "alice should be gone");
}

// ───────────────────────── Phase CE: editable post date (backdating) ─────────────────────────

#[tokio::test]
async fn admin_can_backdate_a_page() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("old-post", "# Old Post\n\nfrom way back")
        .await
        .expect("seed");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let put = |date: &str| {
        admin
            .put(server.url("/pages/old-post"))
            .form(&[
                ("page_title", "Old Post"),
                ("page_category", ""),
                ("page_markdown", "# Old Post\n\nfrom way back"),
                ("page_cover_media_ref", ""),
                ("page_order", "0"),
                ("page_creation_date", date),
            ])
            .send()
    };

    // Backdate it to 2014.
    let r = put("2014-03-15T10:30:00").await.unwrap();
    assert!(r.status().is_success(), "backdate PUT: {}", r.status());
    let date: String =
        sqlx::query("SELECT page_creation_date FROM content_pages WHERE page_name = 'old-post'")
            .fetch_one(&server.pool)
            .await
            .unwrap()
            .get("page_creation_date");
    assert!(date.starts_with("2014-03-15"), "should be backdated to 2014, got {date}");

    // A subsequent save with NO date override keeps the (2014) date — doesn't
    // stamp it back to today.
    let r = put("").await.unwrap();
    assert!(r.status().is_success(), "empty-date PUT: {}", r.status());
    let date: String =
        sqlx::query("SELECT page_creation_date FROM content_pages WHERE page_name = 'old-post'")
            .fetch_one(&server.pool)
            .await
            .unwrap()
            .get("page_creation_date");
    assert!(date.starts_with("2014-03-15"), "empty override must keep the date, got {date}");
}

// ───────────────────── Phase CG: panic hardening ─────────────────────

#[tokio::test]
async fn handler_panic_becomes_a_500_not_a_dropped_connection() {
    // A handler panic must surface as a styled 500 via the CatchPanicLayer — NOT a
    // reset connection (the `000` an uncaught panic gives, which also took the feed
    // down when one post's content crashed the markdown transform).
    let server = spawn_test_server().await.expect("spawn");
    let resp = client()
        .get(server.url("/test/panic"))
        .send()
        .await
        .expect("a panic must yield a response, not a dropped connection");
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        resp.text().await.unwrap().contains("tripped over the cable"),
        "the styled 500 should render"
    );
}

#[tokio::test]
async fn blog_post_page_shows_its_date() {
    // The post detail page (not just the index card) must show the post date.
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("dated-post", "the body").await.expect("seed");
    // Backdate it so the rendered date is deterministic.
    sqlx::query("UPDATE content_pages SET page_creation_date = '2014-03-15 10:00:00' WHERE page_name = 'dated-post'")
        .execute(&server.pool)
        .await
        .unwrap();
    let body = reqwest::get(server.url("/blog/dated-post"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("March 15, 2014"), "post page must show its date: {body}");
}

// ───────────────────── Phase CH: blog/projects pagination + search ─────────────────────

#[tokio::test]
async fn blog_paginates_at_page_size() {
    let server = spawn_test_server().await.expect("spawn");
    for i in 0..12 {
        server
            .seed_blog_post(&format!("post-{i:02}"), &format!("body number {i}"))
            .await
            .unwrap();
    }
    let p1 = reqwest::get(server.url("/blog")).await.unwrap().text().await.unwrap();
    assert_eq!(p1.matches("/blog/post-").count(), 10, "page 1 shows PAGE_SIZE cards");
    assert!(p1.contains("Page 1 of 2"), "pager present: {p1}");
    assert!(p1.contains("Next"), "page 1 has a Next link");
    assert!(!p1.contains("Previous"), "page 1 has no Previous link");

    let p2 = reqwest::get(server.url("/blog?page=2")).await.unwrap().text().await.unwrap();
    assert_eq!(p2.matches("/blog/post-").count(), 2, "page 2 shows the remainder");
    assert!(p2.contains("Page 2 of 2"));
    assert!(p2.contains("Previous"), "page 2 has a Previous link");
    assert!(!p2.contains("Next"), "page 2 has no Next link");
}

#[tokio::test]
async fn blog_search_filters_and_composes_with_pagination() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("rust-post", "all about rustlang internals").await.unwrap();
    server.seed_blog_post("python-post", "all about serpents").await.unwrap();

    let hit = reqwest::get(server.url("/blog?q=rustlang")).await.unwrap().text().await.unwrap();
    assert!(hit.contains("/blog/rust-post"), "matching post shown");
    assert!(!hit.contains("/blog/python-post"), "non-matching post hidden");
    assert!(hit.contains("1 result for"), "result count shown: {hit}");

    let miss = reqwest::get(server.url("/blog?q=zzznotathing")).await.unwrap().text().await.unwrap();
    assert!(miss.contains("No posts match your search"), "empty-search message: {miss}");

    // A filtered set that is itself larger than a page: the search must paginate AND
    // the next link must carry the query.
    for i in 0..12 {
        server.seed_blog_post(&format!("widget-{i:02}"), "shared widgetword body").await.unwrap();
    }
    let s1 = reqwest::get(server.url("/blog?q=widgetword")).await.unwrap().text().await.unwrap();
    assert!(s1.contains("12 results for"), "filtered count: {s1}");
    assert!(s1.contains("Page 1 of 2"), "filtered set paginates");
    assert!(
        s1.contains("q=widgetword") && s1.contains("page=2"),
        "next link composes q + page"
    );
}

#[tokio::test]
async fn projects_listing_uses_the_shared_search_and_pager() {
    let server = spawn_test_server().await.expect("spawn");
    for i in 0..12 {
        server.seed_project(&format!("proj-{i:02}"), &format!("project {i} body")).await.unwrap();
    }
    let body = reqwest::get(server.url("/projects")).await.unwrap().text().await.unwrap();
    assert!(body.contains("action=\"/projects\""), "shared search box wired on projects: {body}");
    assert!(body.contains("Page 1 of 2"), "projects paginate via the shared machinery");
    assert_eq!(
        body.matches("/pages/projects/proj-").count(),
        10,
        "page 1 shows PAGE_SIZE projects"
    );
}
