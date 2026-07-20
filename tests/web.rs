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

/// Media ingest shells out to ffprobe; the media tests skip where it's absent
/// (dev machines have it; some CI runners may not).
fn ffprobe_available() -> bool {
    std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A generic-binary multipart file part (octet-stream) — ffprobe can't type it,
/// so it ingests as `MediaKind::File` (no responsive/poster derivation → fast +
/// deterministic).
fn bin_part(bytes: Vec<u8>, name: &str) -> reqwest::multipart::Part {
    reqwest::multipart::Part::bytes(bytes)
        .file_name(name.to_string())
        .mime_str("application/octet-stream")
        .unwrap()
}

/// The `Location` header of a redirect response (the `client()` doesn't follow them).
fn location(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get("location")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
}

#[tokio::test]
async fn favicon_and_apple_icon_served_at_root() {
    // Browsers request /favicon.ico at the root by default (and iOS /apple-touch-icon.png)
    // regardless of the <link rel=icon> — they must 200, not 404 (the CR analytics finding:
    // 523 all-errored favicon hits because the root route was commented out).
    let server = spawn_test_server().await.expect("spawn");

    let resp = reqwest::get(server.url("/favicon.ico")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "/favicon.ico must serve, not 404");
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("icon") || ct.contains("image"),
        "favicon content-type should be an image: {ct}"
    );

    for icon in ["/apple-touch-icon.png", "/apple-touch-icon-precomposed.png"] {
        let resp = reqwest::get(server.url(icon)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{icon} must serve, not 404");
    }
}

#[tokio::test]
async fn analytics_requires_admin() {
    let server = spawn_test_server().await.expect("spawn");

    // anonymous → 401 (missing identity, DK.2)
    let resp = client()
        .get(server.url("/admin/analytics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // logged in but only Registered → still 403 (authenticated but insufficient)
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
async fn analytics_custom_range_overrides_preset_and_tolerates_garbage() {
    // Phase CT: a valid ?from/&to renders the custom-range branch; a bad or inverted
    // range degrades to the preset (never a 500).
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    // A valid past range → the custom-range branch, NOT the "Last N days" preset line.
    let body = admin
        .get(server.url("/admin/analytics?from=2020-01-01T00:00&to=2020-12-31T23:59"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("Custom range (UTC)"),
        "a valid from/to must render the custom-range branch"
    );
    assert!(
        !body.contains("Last 30 days"),
        "a custom range must not also show the preset line"
    );

    // Garbage bounds → graceful fall back to the preset, 200 not 500.
    let resp = admin
        .get(server.url("/admin/analytics?from=notadate&to=alsobad"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.text().await.unwrap().contains("Last 30 days"),
        "garbage bounds must degrade to the default preset"
    );

    // Inverted range (from > to) is invalid → preset fallback, still 200.
    let resp = admin
        .get(server.url("/admin/analytics?from=2030-01-01T00:00&to=2020-01-01T00:00"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.text().await.unwrap().contains("Last 30 days"),
        "an inverted range must degrade to the preset"
    );

    // A preset pre-fills the From picker with its resolved lower bound (so the fields
    // reflect the active window), while staying a preset — not flipping to a custom range.
    let body = admin
        .get(server.url("/admin/analytics?since=30"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Last 30 days"), "a preset stays a preset");
    let empty_from =
        "id=\"range-from\" name=\"from\" type=\"text\" data-flatpickr autocomplete=\"off\" value=\"\"";
    assert!(
        body.contains("id=\"range-from\"") && !body.contains(empty_from),
        "the From picker must be pre-filled with the preset's lower bound, not left empty"
    );
}

#[tokio::test]
async fn analytics_audience_toggle_renders_and_tolerates_garbage() {
    // CQ.2: the audience toggle + 3-chip honesty display render, and a bad ?audience
    // degrades to All instead of 500-ing the admin page.
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    let body = admin
        .get(server.url("/admin/analytics?audience=humans"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Audience"), "the audience toggle row renders");
    assert!(
        body.contains("Humans") && body.contains("Bots"),
        "all three audience chips render (the honesty display)"
    );

    // A bad ?audience must NOT 500 — Audience::parse falls back to All, never bubbles.
    let resp = admin
        .get(server.url("/admin/analytics?audience=not-a-real-value"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "garbage ?audience degrades to All, not a 500"
    );
}

#[tokio::test]
async fn analytics_surfaces_the_greylist_challenged_dimension() {
    // CY.2/CY.3/CY.7/CY.8: a tolled request (challenged=1, 429) + its clearance must surface as
    // the Challenged filter chip, the Greylist-toll stat block (tolls served + solve rate), and
    // the split-out 429 status bucket — and ?audience=challenged scopes without 500-ing.
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    // The wall (429 + challenged + is_bot) and its solve — the hard-vs-soft data.
    sqlx::query(
        "INSERT INTO request_log (method, path, status, ip, challenged, is_bot) \
         VALUES ('GET', '/wp-login.php', 429, '203.0.113.50', 1, 1)",
    )
    .execute(&server.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO greylist_clearance (ip, solve_ms, digest_version) \
         VALUES ('203.0.113.50', 640, 1)",
    )
    .execute(&server.pool)
    .await
    .unwrap();

    // Default (All) view: the toll block + the 429 bucket appear once something's been walled.
    let body = admin
        .get(server.url("/admin/analytics?since=90"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("Greylist toll"),
        "the toll stat block renders once something's been walled"
    );
    assert!(body.contains("tolls served"), "tolls-served stat present");
    assert!(
        body.contains("solve rate"),
        "the hard-vs-soft solve-rate stat present"
    );
    assert!(
        body.contains("Challenged 1"),
        "the Challenged filter chip shows the toll count"
    );
    assert!(
        body.contains("429 1"),
        "429 is split out of the generic 4xx bucket"
    );

    // The Challenged filter scopes like every other audience — never a 500.
    let resp = admin
        .get(server.url("/admin/analytics?audience=challenged&since=90"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "?audience=challenged scopes without erroring"
    );
    assert!(
        resp.text().await.unwrap().contains("challenged views"),
        "the headline reflects the challenged scope"
    );
}

#[tokio::test]
async fn ip_detail_admin_gated_and_validates() {
    // CQ.4: /admin/analytics/ip/{ip} is admin-gated; a valid-but-empty IP renders a
    // 200 empty-state; a garbage segment is a clean 400 (never a 500 / DB probe).
    let server = spawn_test_server().await.expect("spawn");

    // anonymous → 401 (missing identity; the /admin require_admin nest, DK.2)
    let resp = client()
        .get(server.url("/admin/analytics/ip/1.2.3.4"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    // valid IP, no rows → 200 empty-state
    let resp = admin
        .get(server.url("/admin/analytics/ip/8.8.8.8"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.text().await.unwrap().contains("8.8.8.8"));

    // garbage segment → 400
    let resp = admin
        .get(server.url("/admin/analytics/ip/not-an-ip"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_pages_editor_requires_admin() {
    let server = spawn_test_server().await.expect("spawn");

    // anonymous → 401 (GET is public site-wide, but /admin is require_admin-gated;
    // missing identity → 401, DK.2)
    let resp = client()
        .get(server.url("/admin/pages"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

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

    // anonymous POST → 401 (non-GET requires admin; reorder is not in the allowlist;
    // missing identity → 401, DK.2)
    let resp = client()
        .post(server.url("/admin/pages/reorder"))
        .form(&[("page_id", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

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
async fn logs_requires_admin_and_renders(/* Phase CO */) {
    let server = spawn_test_server().await.expect("spawn");

    // anonymous → 401 (GET is public site-wide, but /admin is require_admin-gated;
    // missing identity → 401, DK.2)
    let resp = client().get(server.url("/admin/logs")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Admin → 200, and the viewer renders (heading + the level-filter chips). The
    // log dir doesn't exist in the test harness, so this exercises the empty tail.
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    let resp = admin
        .get(server.url("/admin/logs?level=error"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Server Logs"));
    assert!(body.contains("/admin/logs?level=warn"), "level filter chips present");
}

#[tokio::test]
async fn logs_route_excluded_from_request_log(/* no self-feed */) {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    // View the EXCLUDED log page, then a NON-excluded admin page as a sentinel.
    admin.get(server.url("/admin/logs")).send().await.unwrap();
    admin.get(server.url("/admin/pages")).send().await.unwrap();

    // Poll until the sentinel (/admin/pages) is logged — that proves the
    // fire-and-forget request_log inserts have caught up, so the ABSENCE of an
    // /admin/logs row is real exclusion, not a timing artifact.
    let mut checked = false;
    for _ in 0..100 {
        let rows = sqlx::query("SELECT path FROM request_log")
            .fetch_all(&server.pool)
            .await
            .unwrap();
        let paths: Vec<String> = rows.iter().map(|r| r.get::<String, _>("path")).collect();
        if paths.iter().any(|p| p == "/admin/pages") {
            assert!(
                !paths.iter().any(|p| p == "/admin/logs"),
                "/admin/logs must be excluded from request_log (no self-feed)"
            );
            checked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(checked, "sentinel /admin/pages should have been logged");
}

#[tokio::test]
async fn base_layout_has_a11y_landmarks() {
    // Lighthouse's landmark audit wants page content wrapped in semantic regions.
    // base.html carries header (banner) / nav / main / footer — guard them so a
    // future template refactor can't silently drop back to bare <div>s.
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("Landmarks", "# probe")
        .await
        .expect("seed");
    let body = reqwest::get(server.url("/pages/Landmarks"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("<main"), "a <main> landmark");
    assert!(body.contains("<nav"), "a <nav> landmark");
    assert!(body.contains("<header"), "a <header> banner landmark");
    assert!(body.contains("<footer"), "a <footer> contentinfo landmark");
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
    assert!(body.contains("id=\"ts-chart\""), "expected the d3 chart container");
    assert!(body.contains("id=\"ts-data\""), "expected the JSON time-series island");
    assert!(body.contains("Humans"), "expected the audience 3-chip");
    assert!(
        body.contains("/pages/test-page"),
        "the content page should appear in top pages"
    );
    assert!(body.contains("Referrers (external)"), "expected the grouped referrers panel");
    assert!(body.contains("paths=all"), "expected the Content/All toggle");
    // CQ.7 new sections all render.
    assert!(body.contains("Status breakdown"), "expected the status breakdown");
    assert!(body.contains("Noisy IPs"), "expected the noisy-IPs leaderboard");
    assert!(body.contains("Server response time"), "expected the latency section");
    assert!(
        body.contains("NOT</strong> client page-load"),
        "latency must be honestly labeled server-side, not LCP"
    );
}

#[tokio::test]
async fn analytics_surfaces_scanners_and_latency() {
    // CQ.7: a scanner (many distinct 404s) is badged + linked to its drill-down, its
    // probe paths surface in the never-succeeded list, and a slow request shows up in
    // the latency section.
    let server = spawn_test_server().await.expect("spawn");

    for i in 0..6 {
        sqlx::query(
            "INSERT INTO request_log (method, path, status, ip, user_agent, duration_ms) \
             VALUES ('GET', ?, 404, '6.6.6.6', 'curl/8', 3)",
        )
        .bind(format!("/probe-{i}"))
        .execute(&server.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO request_log (method, path, status, ip, user_agent, duration_ms) \
         VALUES ('GET', '/pages/slow', 200, '1.1.1.1', 'Mozilla/5', 850)",
    )
    .execute(&server.pool)
    .await
    .unwrap();

    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();
    let body = admin
        .get(server.url("/admin/analytics?since=30"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        body.contains("/admin/analytics/ip/6.6.6.6"),
        "the scanner IP links to its drill-down"
    );
    assert!(
        body.contains("/probe-0"),
        "a never-succeeded probe path is listed"
    );
    assert!(
        body.contains("850"),
        "the slow request's duration renders in the latency section"
    );
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
        StatusCode::UNAUTHORIZED,
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
        StatusCode::UNAUTHORIZED,
        "anon must not overwrite pages"
    );
    let resp = anon
        .delete(server.url("/pages/Victim"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
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
        .header("HX-Request", "true")
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

/// DI.3: the page write routes content-negotiate off ONE handler — htmx gets the
/// HX-* header (unchanged), a JSON client gets the `{directive, page}` envelope, a
/// no-JS/native client gets a real 303. Same backend, three frontends.
#[tokio::test]
async fn write_route_content_negotiates_across_frontends() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("NegPage", "# start")
        .await
        .expect("seed");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    let form = [
        ("page_category", ""),
        ("page_markdown", "# body"),
        ("page_order", "0"),
    ];

    // htmx → HX-Refresh header, 200 (byte-identical to pre-DI.3).
    let r = admin
        .put(server.url("/pages/NegPage"))
        .header("HX-Request", "true")
        .form(&form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers().get("hx-refresh").unwrap(), "true");

    // JSON → 200 + the {directive, page} envelope.
    let r = admin
        .put(server.url("/pages/NegPage"))
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers().get("content-type").unwrap(), "application/json");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["directive"]["type"], "refresh");
    assert_eq!(v["page"]["slug"], "NegPage");

    // Native (no htmx, no json) → a real 303 to the page (POST-redirect-GET).
    let r = admin
        .put(server.url("/pages/NegPage"))
        .form(&form)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers().get("location").unwrap(), "/pages/NegPage");
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
        .header("HX-Request", "true")
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
        StatusCode::UNAUTHORIZED,
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
        StatusCode::UNAUTHORIZED,
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

/// The 403 for an authenticated-but-INSUFFICIENT viewer (DK.2) is a styled "How
/// about NO!" page on a full-page navigation, with a login link. (A MISSING
/// identity gets the 401 page — see `unauthorized_full_nav_renders_styled_page`.)
#[tokio::test]
async fn forbidden_full_nav_renders_styled_page() {
    let server = spawn_test_server().await.expect("spawn");
    // Registered = authenticated but not admin → the insufficient-identity 403.
    let registered = client();
    registered
        .post(server.url("/test/login?role=Registered"))
        .send()
        .await
        .unwrap();
    let resp = registered
        .get(server.url("/admin/analytics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = resp.text().await.unwrap();
    assert!(body.contains("How about NO!"), "styled 403 page: {body}");
    assert!(body.contains("/login"), "403 offers a login link: {body}");
}

/// The 401 for a MISSING identity (DK.2) is a styled "Who goes there?" page on a
/// full-page navigation, with a login link — distinct from the insufficient-viewer
/// 403 above.
#[tokio::test]
async fn unauthorized_full_nav_renders_styled_page() {
    let server = spawn_test_server().await.expect("spawn");
    let anon = client();
    let resp = anon
        .get(server.url("/admin/analytics"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Who goes there?"), "styled 401 page: {body}");
    assert!(body.contains("/login"), "401 offers a login link: {body}");
}

/// An anonymous HTMX mutation (session died mid-edit → MISSING identity) gets a
/// 401 (DK.2) with an HX-Redirect to /login instead of a full HTML doc swapped
/// into a fragment target.
#[tokio::test]
async fn unauthorized_htmx_request_gets_hx_redirect() {
    let server = spawn_test_server().await.expect("spawn");
    let anon = client();
    let resp = anon
        .post(server.url("/pages/anything"))
        .header("HX-Request", "true")
        .form(&[("page_title", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/login"),
        "an HTMX mutation 401 redirects to login"
    );
}

/// Phase CA: an `Authorization: Bearer hio_…` API key authenticates as its user,
/// so the fail-closed layer lets a mutation through; anon + bogus keys 401 (DK.2).
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
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "anon mutation blocked (missing identity → 401)"
    );

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
        StatusCode::UNAUTHORIZED,
        "a bogus key injects no identity → 401"
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

    // Icons are now inline SVGs (Phase CN), so key on the card LABEL — "Previous"
    // / "Next" appear only in this nav on a post page.
    assert!(body.contains("Previous"), "Previous card missing: {body}");
    assert!(body.contains("Next"), "Next card missing: {body}");
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
    assert!(newest.contains("Previous"), "newest should have a Previous card: {newest}");
    assert!(!newest.contains("Next"), "newest must NOT have a Next card: {newest}");
    assert!(newest.contains("href=\"/blog/second-post\""), "Previous → second: {newest}");

    // Oldest post: a Next card (→ second) only, no Previous.
    let oldest = reqwest::get(server.url("/blog/first-post"))
        .await.unwrap().text().await.unwrap();
    assert!(oldest.contains("Next"), "oldest should have a Next card: {oldest}");
    assert!(!oldest.contains("Previous"), "oldest must NOT have a Previous card: {oldest}");
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
    // The BX prev/next nav (<nav aria-label="Post navigation">) is blog-only.
    // Icons are generic <svg class="icon"> now (Phase CN), so anchor on the nav
    // landmark, not an icon class.
    assert!(
        !body.contains("Post navigation"),
        "a /pages page must not show the blog-only post nav: {body}"
    );
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

    // Management is admin-gated: anonymous GET + upload → 401 (missing identity, DK.2).
    assert_eq!(
        client().get(server.url("/admin/media")).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client().post(server.url("/media")).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
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
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "admin upload should succeed");
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["ref"].as_str().expect("ref in manifest").to_string();
    assert!(!media_ref.is_empty());

    // The library lists it.
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    assert!(lib.contains(&media_ref), "library should list {media_ref}");

    // Rename → the new title shows in the library (PUT /media/<ref> {title}, DR).
    let resp = admin
        .put(server.url(&format!("/media/{media_ref}")))
        .json(&serde_json::json!({ "title": "Bonnie Mugshot" }))
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
            .post(server.url(&format!("/media/{media_ref}/variants")))
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "add-encode (and dedup re-add) should be 201"
        );
    }

    // Per-stream delete: pull a variant's url_key from the library card and DELETE
    // it via the canonical surface (DELETE /media/<ref>/variants/<url_key>, DR).
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    let del_key = lib
        .split("data-url-key=\"")
        .nth(1)
        .expect("a variant delete control in the library")
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let resp = admin
        .delete(server.url(&format!("/media/{media_ref}/variants/{del_key}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "per-stream delete should succeed");

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
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "streaming upload should succeed");
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["ref"].as_str().expect("ref").to_string();

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

/// Phase DO/DQ: the fab-scad round-trip SAVE — `PUT /media/<ref>/variants`
/// COMPLETELY replaces an item's variant collection. The stable ref + title survive
/// (so `![](/media/<ref>)` embeds don't break), the new bytes serve, and the OLD
/// variant's url_key is GONE (a genuine replace, not an append). Returns the
/// manifest. Generic-file payloads keep it fast; needs ffprobe (the ingest probes).
#[tokio::test]
async fn media_patch_replaces_variant_set_in_place() {
    if !ffprobe_available() {
        eprintln!("skipping media replace-variants test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Distinct generic payloads — ffprobe types neither → MediaKind::File.
    let old_bytes: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(7)) as u8).collect();
    let new_bytes: Vec<u8> = (0..2048u32).map(|i| (i.wrapping_mul(13).wrapping_add(1)) as u8).collect();

    // Upload the original → item ref R.
    let form = reqwest::multipart::Form::new().part("file", bin_part(old_bytes, "model.old.bin"));
    let up: serde_json::Value = admin
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let media_ref = up["ref"].as_str().unwrap().to_string();

    // Title it, to prove the title survives the swap (PUT /media/<ref> {title}).
    admin
        .put(server.url(&format!("/media/{media_ref}")))
        .json(&serde_json::json!({ "title": "Widget" }))
        .send()
        .await
        .unwrap();

    // Capture the OLD variant's url_key from the download embed; the bytes serve now.
    let embed = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let old_key = embed
        .split("/media/file/")
        .nth(1)
        .expect("embed carries a file url")
        .split('"')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        reqwest::get(server.url(&format!("/media/file/{old_key}"))).await.unwrap().status(),
        StatusCode::OK
    );

    // PUT the SAME ref's variant collection with the new bytes → complete replace.
    let form = reqwest::multipart::Form::new().part("file", bin_part(new_bytes.clone(), "model.new.bin"));
    let resp = admin
        .put(server.url(&format!("/media/{media_ref}/variants")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "admin PUT /variants (replace-all) should succeed");
    let j: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(j["ref"].as_str().unwrap(), media_ref, "ref unchanged (the manifest)");
    let variants = j["variants"].as_array().expect("manifest variants array");
    assert_eq!(variants.len(), 1, "exactly the one new variant");
    let new_key = variants[0]["href"]
        .as_str()
        .unwrap()
        .strip_prefix("/media/file/")
        .expect("variant href is a byte URL")
        .to_string();
    assert_ne!(new_key, old_key, "the url_key changed with the bytes");

    // The new bytes serve byte-for-byte.
    let served = reqwest::get(server.url(&format!("/media/file/{new_key}"))).await.unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(
        served.bytes().await.unwrap().as_ref(),
        new_bytes.as_slice(),
        "new bytes served"
    );

    // The OLD variant is GONE — a complete replace, not an append.
    assert_eq!(
        reqwest::get(server.url(&format!("/media/file/{old_key}"))).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "old variant wiped"
    );

    // The stable ref + title survived → embeds keep working, now pointing at the new bytes.
    let embed2 = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(embed2.contains(&new_key), "embed resolves the new bytes via the SAME ref");
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    assert!(lib.contains("Widget"), "title preserved across the swap");
}

/// Regression (model-save 500): the fab-gui editor's SAVE (`PUT …/variants`) can send
/// BYTE-IDENTICAL file parts in ONE request (an unchanged export re-sent alongside
/// another). The replace loop must DEDUP them by content hash like `append_variants`
/// does — else the 2nd `create` violates `UNIQUE(media_id, sha256)` → a 500 on Save.
#[tokio::test]
async fn replace_variants_dedups_byte_identical_parts() {
    if !ffprobe_available() {
        eprintln!("skipping replace-variants dedup test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Upload an original → ref.
    let orig: Vec<u8> = (0..1024u32).map(|i| (i.wrapping_mul(3)) as u8).collect();
    let up: serde_json::Value = admin
        .post(server.url("/media"))
        .multipart(reqwest::multipart::Form::new().part("file", bin_part(orig, "m.bin")))
        .send().await.unwrap().json().await.unwrap();
    let media_ref = up["ref"].as_str().unwrap().to_string();

    // PUT the SAME bytes TWICE in one request (different filenames, identical content) —
    // the fab-gui duplicate-part case. Must succeed + collapse to ONE variant.
    let dup: Vec<u8> = (0..2048u32).map(|i| (i.wrapping_mul(11).wrapping_add(2)) as u8).collect();
    let form = reqwest::multipart::Form::new()
        .part("f1", bin_part(dup.clone(), "model.bin"))
        .part("f2", bin_part(dup.clone(), "model-copy.bin"));
    let resp = admin
        .put(server.url(&format!("/media/{media_ref}/variants")))
        .multipart(form)
        .send().await.unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "identical parts dedup, not a 500: {body}");
    let j: serde_json::Value = serde_json::from_str(&body).unwrap();
    let n = j["variants"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(n, 1, "the two byte-identical parts collapsed to ONE variant, got {n}");
}

/// Phase DO/DQ guards: `PUT /media/<ref>/variants` is Admin-only (inherited from the
/// mutation layer, no /media-specific rule), 404s an unknown ref, 400s an empty body
/// (a replace-to-nothing is a DELETE), and the freshly-minted variants INHERIT the
/// item's PRESERVED gate. Needs ffprobe.
#[tokio::test]
async fn media_patch_authz_guards_and_gate_preserved() {
    if !ffprobe_available() {
        eprintln!("skipping media replace-variants guard test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");

    // Anonymous PUT → 401 (missing identity): gated FOR FREE by the non-safe-method
    // admin fallback, no /media-specific rule.
    let anon = reqwest::Client::new()
        .put(server.url("/media/anything/variants"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Unknown ref → 404.
    let bytes: Vec<u8> = (0..1024u32).map(|i| i as u8).collect();
    let miss = admin
        .put(server.url("/media/does-not-exist/variants"))
        .multipart(reqwest::multipart::Form::new().part("file", bin_part(bytes.clone(), "x.bin")))
        .send()
        .await
        .unwrap();
    assert_eq!(miss.status(), StatusCode::NOT_FOUND);

    // Upload a GATED (Family) item → R.
    let form = reqwest::multipart::Form::new()
        .part("file", bin_part(bytes, "book.bin"))
        .text("min_role", "Family");
    let up: serde_json::Value = admin
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let media_ref = up["ref"].as_str().unwrap().to_string();

    // Empty PUT (no file parts) → 400, and it's rejected BEFORE the destructive
    // tx (a replace to zero variants is a DELETE).
    let empty = admin
        .put(server.url(&format!("/media/{media_ref}/variants")))
        .multipart(reqwest::multipart::Form::new().text("min_role", "Public"))
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST, "empty replace rejected");

    // Real PUT with new bytes → the new variant must INHERIT the Family gate.
    let new_bytes: Vec<u8> = (0..777u32).map(|i| (i.wrapping_mul(3)) as u8).collect();
    let j: serde_json::Value = admin
        .put(server.url(&format!("/media/{media_ref}/variants")))
        .multipart(reqwest::multipart::Form::new().part("file", bin_part(new_bytes, "book.v2.bin")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let new_key = j["variants"][0]["href"]
        .as_str()
        .unwrap()
        .strip_prefix("/media/file/")
        .expect("variant href")
        .to_string();

    // Anonymous fetch of the new bytes → 404 (gated, denied ≡ miss): the gate carried
    // onto the freshly-minted variant.
    assert_eq!(
        reqwest::get(server.url(&format!("/media/file/{new_key}"))).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "gate preserved onto the new variant"
    );
    // Admin reaches it.
    assert_eq!(
        admin.get(server.url(&format!("/media/file/{new_key}"))).send().await.unwrap().status(),
        StatusCode::OK,
        "admin reaches the gated new bytes"
    );
}

/// Phase DQ: the RESTful write-surface lifecycle — `POST /media` (create, 201 +
/// Location + manifest) → `POST …/variants` (add, 201) → `PUT /media/<ref>` (metadata,
/// absent field KEPT) → `DELETE …/variants/<key>` → `DELETE /media/<ref>`. Two POSTs
/// for the server-assigns-identity creates, PUT for replace/metadata, DELETE — zero
/// PATCH. Needs ffprobe.
#[tokio::test]
async fn media_write_surface_lifecycle() {
    if !ffprobe_available() {
        eprintln!("skipping DQ lifecycle test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // CREATE — POST /media → 201 + Location + manifest.
    let a: Vec<u8> = (0..2000u32).map(|i| i as u8).collect();
    let resp = admin
        .post(server.url("/media"))
        .multipart(
            reqwest::multipart::Form::new()
                .part("file", bin_part(a, "one.bin"))
                .text("title", "Doc"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "POST /media → 201 Created");
    let loc = location(&resp).expect("Location header");
    assert!(loc.starts_with("/media/"), "Location points at the new item: {loc}");
    let m: serde_json::Value = resp.json().await.unwrap();
    let r = m["ref"].as_str().unwrap().to_string();
    assert_eq!(m["self"].as_str().unwrap(), format!("/media/{r}"));
    assert_eq!(m["title"].as_str().unwrap(), "Doc");
    assert_eq!(m["controls"]["add"]["method"].as_str(), Some("POST"), "admin manifest carries controls");
    assert_eq!(m["variants"].as_array().unwrap().len(), 1);

    // ADD — POST /media/<ref>/variants → 201, now 2 variants.
    let b: Vec<u8> = (0..3000u32).map(|i| (i.wrapping_mul(7)) as u8).collect();
    let add = admin
        .post(server.url(&format!("/media/{r}/variants")))
        .multipart(reqwest::multipart::Form::new().part("file", bin_part(b, "two.bin")))
        .send()
        .await
        .unwrap();
    assert_eq!(add.status(), StatusCode::CREATED, "POST /variants → 201");
    let m2: serde_json::Value = add.json().await.unwrap();
    let variants = m2["variants"].as_array().unwrap().clone();
    assert_eq!(variants.len(), 2, "the added variant appears");

    // METADATA — PUT /media/<ref> {min_role: Family}; title ABSENT → KEEPS "Doc" (fail-safe).
    let put = admin
        .put(server.url(&format!("/media/{r}")))
        .json(&serde_json::json!({ "min_role": "Family" }))
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::OK);
    let m3: serde_json::Value = put.json().await.unwrap();
    assert_eq!(m3["min_role"].as_str(), Some("Family"), "gate set");
    assert_eq!(m3["title"].as_str(), Some("Doc"), "absent title KEPT (fail-safe, never silently clears)");

    // DELETE a variant — DELETE /media/<ref>/variants/<key> → 204; re-delete → 404.
    let key = variants[0]["href"]
        .as_str()
        .unwrap()
        .strip_prefix("/media/file/")
        .unwrap()
        .to_string();
    let del = admin.delete(server.url(&format!("/media/{r}/variants/{key}"))).send().await.unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT, "delete variant → 204");
    let del2 = admin.delete(server.url(&format!("/media/{r}/variants/{key}"))).send().await.unwrap();
    assert_eq!(del2.status(), StatusCode::NOT_FOUND, "re-delete a gone variant → 404");

    // DELETE the item — DELETE /media/<ref> → 204; then the item is gone.
    let ditem = admin.delete(server.url(&format!("/media/{r}"))).send().await.unwrap();
    assert_eq!(ditem.status(), StatusCode::NO_CONTENT, "delete item → 204");
    assert_eq!(
        admin.request(reqwest::Method::OPTIONS, server.url(&format!("/media/{r}"))).send().await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "the item is gone"
    );
}

/// Phase DQ.5/DQ.7: `GET /media` is admin-only (403 for a non-admin — the whole
/// library is an admin capability), and the OPTIONS manifest's write `controls` +
/// per-variant `remove` appear ONLY for an Admin (the HATEOAS mirror of the gate).
/// Needs ffprobe.
#[tokio::test]
async fn media_list_admin_only_and_manifest_controls_role_aware() {
    if !ffprobe_available() {
        eprintln!("skipping DQ list/controls test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let up: serde_json::Value = admin
        .post(server.url("/media"))
        .multipart(reqwest::multipart::Form::new().part("file", bin_part(vec![1, 2, 3, 4], "pub.bin")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = up["ref"].as_str().unwrap().to_string();

    // GET /media: admin → 200 + lists it; anon → 403 (not the DATA 404-oracle).
    let list = admin.get(server.url("/media")).send().await.unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let lj: serde_json::Value = list.json().await.unwrap();
    assert!(
        lj["items"].as_array().unwrap().iter().any(|i| i["ref"].as_str() == Some(r.as_str())),
        "admin list includes the item"
    );
    assert_eq!(
        reqwest::Client::new().get(server.url("/media")).send().await.unwrap().status(),
        StatusCode::FORBIDDEN,
        "anon GET /media → 403"
    );

    // OPTIONS a PUBLIC item: admin sees controls + remove; anon sees neither.
    let am: serde_json::Value = admin
        .request(reqwest::Method::OPTIONS, server.url(&format!("/media/{r}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(am["controls"].is_object(), "admin manifest has controls");
    assert!(am["variants"][0]["remove"].is_string(), "admin variant has a remove link");
    let anon_m: serde_json::Value = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, server.url(&format!("/media/{r}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(anon_m["controls"].is_null(), "anon manifest has NO controls");
    assert!(anon_m["variants"][0]["remove"].is_null(), "anon variant has NO remove link");
    assert!(anon_m["variants"][0]["href"].is_string(), "anon still sees the variant href");
}

/// Phase DQ: every write verb on the `/media` surface is Admin-gated FOR FREE by the
/// mutation layer (anon → 401), with NO /media-specific rule. No upload → no ffprobe.
#[tokio::test]
async fn media_write_surface_is_admin_gated() {
    let server = spawn_test_server().await.expect("spawn");
    let anon = reqwest::Client::new();
    for (method, path) in [
        (reqwest::Method::POST, "/media"),
        (reqwest::Method::PUT, "/media/x"),
        (reqwest::Method::DELETE, "/media/x"),
        (reqwest::Method::POST, "/media/x/variants"),
        (reqwest::Method::PUT, "/media/x/variants"),
        (reqwest::Method::DELETE, "/media/x/variants/deadbeef"),
    ] {
        let status = anon
            .request(method.clone(), server.url(path))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "anon {method} {path} → 401");
    }
}

/// Phase DQ (review fix): two byte-identical file parts in ONE multipart dedup to a
/// SINGLE variant — NOT a 500 from the `UNIQUE(media_id, sha256)` the 2nd insert would
/// hit + a partial append. Covers both create (`POST /media`) and add (`POST …/variants`).
/// Needs ffprobe.
#[tokio::test]
async fn media_dedups_identical_parts_within_one_upload() {
    if !ffprobe_available() {
        eprintln!("skipping dedup test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // CREATE with two IDENTICAL parts → 201 + exactly ONE variant (deduped, not a 500).
    let bytes: Vec<u8> = (0..1500u32).map(|i| (i.wrapping_mul(11)) as u8).collect();
    let form = reqwest::multipart::Form::new()
        .part("file", bin_part(bytes.clone(), "dup1.bin"))
        .part("file", bin_part(bytes.clone(), "dup2.bin"));
    let resp = admin.post(server.url("/media")).multipart(form).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "identical parts dedup, not a 500");
    let m: serde_json::Value = resp.json().await.unwrap();
    let r = m["ref"].as_str().unwrap().to_string();
    assert_eq!(m["variants"].as_array().unwrap().len(), 1, "two identical parts → ONE variant");

    // ADD an identical part again → 201, STILL one variant (idempotent content-dedup).
    let add = admin
        .post(server.url(&format!("/media/{r}/variants")))
        .multipart(reqwest::multipart::Form::new().part("file", bin_part(bytes, "dup3.bin")))
        .send()
        .await
        .unwrap();
    assert_eq!(add.status(), StatusCode::CREATED);
    let m2: serde_json::Value = add.json().await.unwrap();
    assert_eq!(m2["variants"].as_array().unwrap().len(), 1, "re-adding the same bytes is a no-op");
}

/// Phase DP: `GET /media/<ref>` content-negotiates (`?format=` > wildcard-free
/// `Accept` > largest) and `OPTIONS /media/<ref>` returns the hypermedia manifest.
/// The manifest's per-type `href`s are the ground truth the negotiation Locations are
/// checked against (no content-type dependency). Needs ffprobe (ingest probes).
#[tokio::test]
async fn media_get_negotiates_and_options_manifest() {
    if !ffprobe_available() {
        eprintln!("skipping DP negotiation test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // A grouped model item: a SMALL scad source + a LARGER 3mf mesh (dominant → Stl).
    let scad = bin_part(b"cube([10,10,10]);".to_vec(), "model.scad");
    let mesh = bin_part((0..8000u32).map(|i| i as u8).collect(), "model.3mf");
    let up: serde_json::Value = admin
        .post(server.url("/media"))
        .multipart(reqwest::multipart::Form::new().part("file", scad).part("file", mesh))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = up["ref"].as_str().unwrap().to_string();

    // OPTIONS → the manifest; map each variant type → its href (the ground truth).
    let opts = admin
        .request(reqwest::Method::OPTIONS, server.url(&format!("/media/{r}")))
        .send()
        .await
        .unwrap();
    assert_eq!(opts.status(), StatusCode::OK, "OPTIONS → 200 manifest");
    let m: serde_json::Value = opts.json().await.unwrap();
    assert_eq!(m["self"].as_str().unwrap(), format!("/media/{r}"));
    assert_eq!(m["ref"].as_str().unwrap(), r);
    let variants = m["variants"].as_array().unwrap();
    let href_of = |ty: &str| -> String {
        variants
            .iter()
            .find(|v| v["type"] == ty)
            .unwrap_or_else(|| panic!("manifest has no {ty} variant: {m}"))["href"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let scad_href = href_of("application/x-openscad");
    let mesh_href = href_of("model/3mf");

    // ?format=scad → 307 to the scad href, with Vary: Accept.
    let r1 = admin.get(server.url(&format!("/media/{r}?format=scad"))).send().await.unwrap();
    assert_eq!(r1.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(r1.headers().get("vary").and_then(|h| h.to_str().ok()), Some("Accept"));
    assert_eq!(location(&r1).as_deref(), Some(scad_href.as_str()), "?format=scad → the scad");

    // ?format=3mf → the mesh; ?format=mp4 (absent) → 406.
    let r2 = admin.get(server.url(&format!("/media/{r}?format=3mf"))).send().await.unwrap();
    assert_eq!(location(&r2).as_deref(), Some(mesh_href.as_str()), "?format=3mf → the mesh");
    assert_eq!(
        admin.get(server.url(&format!("/media/{r}?format=mp4"))).send().await.unwrap().status(),
        StatusCode::NOT_ACCEPTABLE,
        "a format the item lacks → 406"
    );

    // Accept: application/x-openscad (wildcard-FREE) → the scad (an honest preference).
    let ra = admin
        .get(server.url(&format!("/media/{r}")))
        .header("Accept", "application/x-openscad")
        .send()
        .await
        .unwrap();
    assert_eq!(location(&ra).as_deref(), Some(scad_href.as_str()), "wildcard-free Accept → scad");

    // Browser-ish Accept (carries */*) → largest (the mesh) — bare-link behavior UNCHANGED.
    let rw = admin
        .get(server.url(&format!("/media/{r}")))
        .header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .send()
        .await
        .unwrap();
    assert_eq!(location(&rw).as_deref(), Some(mesh_href.as_str()), "*/* → largest (unchanged)");

    // Accept: application/json → the item manifest (200), same self as OPTIONS.
    let js: serde_json::Value = admin
        .get(server.url(&format!("/media/{r}")))
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(js["ref"].as_str().unwrap(), r);
    assert_eq!(js["self"].as_str().unwrap(), format!("/media/{r}"));

    // Gate: OPTIONS on a Family item → anon 404 (≡ miss), admin 200.
    let gated: serde_json::Value = admin
        .post(server.url("/media"))
        .multipart(
            reqwest::multipart::Form::new()
                .part("file", bin_part(b"secret".to_vec(), "g.bin"))
                .text("min_role", "Family"),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let gr = gated["ref"].as_str().unwrap();
    assert_eq!(
        reqwest::Client::new()
            .request(reqwest::Method::OPTIONS, server.url(&format!("/media/{gr}")))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND,
        "anon OPTIONS on a gated item ≡ a miss (no oracle)"
    );
    assert_eq!(
        admin
            .request(reqwest::Method::OPTIONS, server.url(&format!("/media/{gr}")))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK,
        "admin OPTIONS on the gated item → 200"
    );
    // Unknown ref → 404.
    assert_eq!(
        admin
            .request(reqwest::Method::OPTIONS, server.url("/media/does-not-exist"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
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
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["ref"].as_str().unwrap().to_string();

    // The variant carries a non-NULL storage_root pointing at a real media root dir.
    let row = sqlx::query(
        "SELECT v.storage_root FROM media_variant v \
         JOIN media m ON m.media_id = v.media_id WHERE m.media_ref = ?1",
    )
    .bind(&media_ref)
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

/// Phase CJ/CK review fix (M2): the byte route never lets an uploaded file run as
/// active content on our canonical origin — `X-Content-Type-Options: nosniff`
/// always, and an executable mime (svg/html/js) is forced to download. Needs
/// ffprobe.
#[tokio::test]
async fn media_serve_neutralizes_active_content() {
    let has_ffprobe = std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_ffprobe {
        eprintln!("skipping active-content test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // An SVG carrying a script — ffprobe can't type it → MediaKind::File, mime
    // guessed from the .svg extension = image/svg+xml (an executable type).
    let svg =
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(svg)
            .file_name("evil.svg")
            .mime_str("image/svg+xml")
            .unwrap(),
    );
    let resp = admin
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["ref"].as_str().unwrap().to_string();

    let embed = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let url_key = embed
        .split("/media/file/")
        .nth(1)
        .expect("file url")
        .split('"')
        .next()
        .unwrap()
        .to_string();

    let served = reqwest::get(server.url(&format!("/media/file/{url_key}"))).await.unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(
        served
            .headers()
            .get("x-content-type-options")
            .map(|h| h.to_str().unwrap()),
        Some("nosniff"),
        "every served file gets nosniff",
    );
    assert!(
        served
            .headers()
            .get("content-disposition")
            .map(|h| h.to_str().unwrap().contains("attachment"))
            .unwrap_or(false),
        "an executable mime (svg) must be forced to download, not rendered inline",
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
async fn feed_conditional_request_304_then_200_after_change() {
    // Phase CS: the feed emits an ETag/Last-Modified validator and honors
    // If-None-Match with a 304 (skipping the expensive per-entry transform), then
    // returns a fresh 200 once the content set changes.
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post("cond-post", "Some body.")
        .await
        .expect("seed");

    // First fetch: 200 carrying the validators.
    let first = reqwest::get(server.url("/feed.xml")).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = first
        .headers()
        .get("etag")
        .and_then(|h| h.to_str().ok())
        .expect("feed must carry an ETag")
        .to_string();
    assert!(etag.starts_with("W/\""), "weak etag expected: {etag}");
    assert!(
        first.headers().contains_key("last-modified"),
        "feed must carry Last-Modified"
    );

    // Re-fetch echoing the ETag → 304, no body, validator preserved.
    let client = reqwest::Client::new();
    let cached = client
        .get(server.url("/feed.xml"))
        .header("If-None-Match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        cached.status(),
        StatusCode::NOT_MODIFIED,
        "an unchanged feed should 304"
    );
    assert_eq!(
        cached.headers().get("etag").and_then(|h| h.to_str().ok()),
        Some(etag.as_str()),
        "a 304 should echo the ETag"
    );
    assert!(
        cached.text().await.unwrap().is_empty(),
        "a 304 carries no body"
    );

    // Add a second entry → the entry count (and max modified_date) move the
    // validator → the SAME conditional request now misses and returns a 200.
    server
        .seed_blog_post("cond-post-2", "Another body.")
        .await
        .expect("seed 2");
    let after = client
        .get(server.url("/feed.xml"))
        .header("If-None-Match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        StatusCode::OK,
        "a new post must invalidate the client's cached ETag"
    );
    let new_etag = after
        .headers()
        .get("etag")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    assert_ne!(new_etag, etag, "the validator must change after content changes");
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

    // The middleware downgrades a deleted user to Anonymous → 401 (missing identity, DK.2).
    let denied = admin.get(server.url("/admin/analytics")).send().await.unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::UNAUTHORIZED,
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

    // Anonymous → 401 (missing identity, DK.2).
    assert_eq!(
        client().get(server.url("/admin/users")).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
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

// ───────────────────────── Phase CZ: Family role + honest sessions ─────────────────────────

/// Family is a READ tier, not a mutation tier (the role-scoped allowlist ships
/// EMPTY): a Family session reads public content like anyone, but every
/// mutation and every `/admin` GET stays admin-only — and the 403 comes from
/// the layer, BEFORE any handler (a Family DELETE on a nonexistent user is
/// still 403, never the handler's 404).
#[tokio::test]
async fn family_role_reads_but_cannot_mutate_or_reach_admin() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("FamilyProbe", "# hello family")
        .await
        .expect("seed");

    let family = client();
    let r = family
        .post(server.url("/test/login?role=Family"))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "the test seam must mint Family the moment the variant exists: {}",
        r.status()
    );

    // Reads: public content is 200 like any viewer.
    let resp = family
        .get(server.url("/pages/FamilyProbe"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The /admin nest gates its GETs above Family.
    for path in ["/admin/users", "/admin/analytics"] {
        assert_eq!(
            family.get(server.url(path)).send().await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "{path} must stay admin-only for Family"
        );
    }

    // Mutations: default-DENY holds (the role-scoped table is empty in CZ).
    let resp = family
        .put(server.url("/pages/FamilyProbe"))
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# defaced by family"),
            ("page_cover_attachment_id", ""),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "family must not mutate pages");

    // Layer-before-handler: 403 even for a target the handler would 404.
    let resp = family
        .delete(server.url("/admin/users/01980000-0000-7000-8000-000000000000"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "the authz layer must reject before the handler runs"
    );
}

/// The three-way /admin/users control: promote to Family, badge shows, and the
/// two CZ guards hold — `Anonymous` is not an assignable target (400), and the
/// last-admin protection also covers demote-to-Family (409).
#[tokio::test]
async fn admin_users_family_promotion_and_guards() {
    let server = spawn_test_server().await.expect("spawn");
    let alice = server.seed_user("alice", "Registered").await.unwrap();
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Registered → Family.
    let r = admin
        .post(server.url(&format!("/admin/users/{alice}/role")))
        .form(&[("role", "Family")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "promote to Family should succeed: {}", r.status());
    let role: String = sqlx::query("SELECT app_role FROM users WHERE display_name = 'alice'")
        .fetch_one(&server.pool).await.unwrap().get("app_role");
    assert_eq!(role, "Family");

    // The list renders the real role.
    let body = admin
        .get(server.url("/admin/users"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Family"), "user list should show the Family role");

    // Anonymous is a sentinel, not a target.
    let r = admin
        .post(server.url(&format!("/admin/users/{alice}/role")))
        .form(&[("role", "Anonymous")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "Anonymous must be rejected");
    let role: String = sqlx::query("SELECT app_role FROM users WHERE display_name = 'alice'")
        .fetch_one(&server.pool).await.unwrap().get("app_role");
    assert_eq!(role, "Family", "the rejected set must not have landed");

    // The last-admin guard keys on leaving Admin — a Family target is still a demotion.
    let id = admin_user_id(&server).await;
    let r = admin
        .post(server.url(&format!("/admin/users/{id}/role")))
        .form(&[("role", "Family")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT, "can't demote the last admin to Family");
}

/// The CZ.3 session touch: activity must push the `OnInactivity(1 day)` expiry
/// forward. Before the fix the session was written exactly once (at login), so
/// it died 24h after login regardless of activity. Reads the tower_sessions
/// row's expiry as epoch seconds (type-tolerant: the store binds an
/// OffsetDateTime, SQLite may hold it as TEXT or a number).
#[tokio::test]
async fn authenticated_activity_extends_session_expiry() {
    let server = spawn_test_server().await.expect("spawn");
    let user = client();
    user.post(server.url("/test/login?role=Family")).send().await.unwrap();

    let expiry_epoch = || async {
        sqlx::query(
            "SELECT CASE WHEN typeof(expiry_date) = 'text' THEN unixepoch(expiry_date) \
             ELSE CAST(expiry_date AS INTEGER) END AS e FROM tower_sessions",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap()
        .get::<i64, _>("e")
    };
    let at_login = expiry_epoch().await;

    // Whole-second expiry resolution → sleep past a second boundary.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    let resp = user.get(server.url("/")).send().await.unwrap();
    assert!(resp.status().is_success());

    let after_activity = expiry_epoch().await;
    assert!(
        after_activity > at_login,
        "an authenticated request must push the session expiry forward \
         (login: {at_login}, after: {after_activity})"
    );

    // The touch is THROTTLED (hourly): an immediate follow-up request must NOT
    // write again — otherwise static-asset/media-range fan-out would turn every
    // subresource GET into a tower_sessions write.
    let resp = user.get(server.url("/")).send().await.unwrap();
    assert!(resp.status().is_success());
    let after_second = expiry_epoch().await;
    assert_eq!(
        after_second, after_activity,
        "a request inside the touch interval must not re-save the session"
    );

    // And the anonymous path stays write-free: a fresh client with no session
    // must not create a session row.
    let rows_before: i64 = sqlx::query("SELECT COUNT(*) AS c FROM tower_sessions")
        .fetch_one(&server.pool).await.unwrap().get("c");
    client().get(server.url("/")).send().await.unwrap();
    let rows_after: i64 = sqlx::query("SELECT COUNT(*) AS c FROM tower_sessions")
        .fetch_one(&server.pool).await.unwrap().get("c");
    assert_eq!(rows_before, rows_after, "anonymous traffic must not mint sessions");
}

// ───────────────────────── Phase DA: min_role page visibility ─────────────────────────

/// Stamp `min_role` on a seeded page directly — the editor control arrives in
/// Phase DB; DA gates the read paths.
async fn stamp_min_role(pool: &sqlx::SqlitePool, page_name: &str, min_role: &str) {
    let n = sqlx::query("UPDATE content_pages SET min_role = ?1 WHERE page_name = ?2")
        .bind(min_role)
        .bind(page_name)
        .execute(pool)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1, "stamp must hit exactly one page ({page_name})");
}

/// The oracle rule (DA.5): an insufficient viewer gets the SAME cat-404 for a
/// role-gated page as for a page that does not exist — byte-identical body,
/// compared within the same session so nav login-state can't differ. Family and
/// Admin read the content.
#[tokio::test]
async fn gated_page_is_byte_identical_to_a_genuine_miss() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("SecretPage", "# family secrets inside")
        .await
        .expect("seed");
    stamp_min_role(&server.pool, "SecretPage", "Family").await;

    // Anonymous and Registered: gated page ≡ genuine miss, byte for byte.
    for role in [None, Some("Registered")] {
        let c = client();
        if let Some(r) = role {
            c.post(server.url(&format!("/test/login?role={r}"))).send().await.unwrap();
        }
        let gated = c.get(server.url("/pages/SecretPage")).send().await.unwrap();
        assert_eq!(gated.status(), StatusCode::NOT_FOUND, "viewer {role:?}");
        let gated_body = gated.text().await.unwrap();
        assert!(
            !gated_body.contains("family secrets"),
            "the 404 must not leak content ({role:?})"
        );

        let miss = c.get(server.url("/pages/NoSuchPage")).send().await.unwrap();
        assert_eq!(miss.status(), StatusCode::NOT_FOUND);
        let miss_body = miss.text().await.unwrap();
        assert_eq!(
            gated_body, miss_body,
            "gated page must be indistinguishable from a genuine miss for {role:?}"
        );
    }

    // Family and Admin: 200 with the content.
    for r in ["Family", "Admin"] {
        let c = client();
        c.post(server.url(&format!("/test/login?role={r}"))).send().await.unwrap();
        let resp = c.get(server.url("/pages/SecretPage")).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "viewer {r}");
        assert!(resp.text().await.unwrap().contains("family secrets inside"));
    }
}

/// A gated ANCESTOR hides its whole subtree — a public child under a Family
/// parent is a cat-404 to an insufficient viewer (a leaf-only gate would leak
/// the parent's title in the breadcrumb).
#[tokio::test]
async fn gated_ancestor_hides_the_subtree() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("GatedParent", "# parent")
        .await
        .expect("seed");
    sqlx::query(
        "INSERT INTO content_pages (parent_page_id, page_name, page_markdown) \
         SELECT page_id, 'PublicChild', '# child body' FROM content_pages WHERE page_name = 'GatedParent'",
    )
    .execute(&server.pool)
    .await
    .unwrap();
    stamp_min_role(&server.pool, "GatedParent", "Family").await;

    let resp = client()
        .get(server.url("/pages/GatedParent/PublicChild"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "subtree must inherit the gate");

    let fam = client();
    fam.post(server.url("/test/login?role=Family")).send().await.unwrap();
    let resp = fam
        .get(server.url("/pages/GatedParent/PublicChild"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.text().await.unwrap().contains("child body"));
}

/// Listing + crawler surfaces: a gated blog post is absent from /blog for an
/// insufficient viewer (and its direct URL is the cat-404), present for Family;
/// the feed and sitemap NEVER carry it — they are unconditionally Anonymous,
/// even when fetched with a Family session cookie (a per-viewer feed would also
/// break its 304 validator).
#[tokio::test]
async fn gated_post_hidden_from_listings_feed_and_sitemap() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post("open-post", "# Open Post\n\npublic words")
        .await
        .expect("seed");
    server
        .seed_blog_post("family-post", "# Family Post\n\nkin-only words")
        .await
        .expect("seed");
    stamp_min_role(&server.pool, "family-post", "Family").await;

    // Anonymous: index omits it, direct URL is a 404.
    let index = reqwest::get(server.url("/blog")).await.unwrap().text().await.unwrap();
    assert!(index.contains("Open Post"));
    assert!(!index.contains("Family Post"), "gated post must not list for anon");
    assert_eq!(
        reqwest::get(server.url("/blog/family-post")).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    // Family: listed + readable.
    let fam = client();
    fam.post(server.url("/test/login?role=Family")).send().await.unwrap();
    let index = fam.get(server.url("/blog")).send().await.unwrap().text().await.unwrap();
    assert!(index.contains("Family Post"), "family must see the gated post listed");
    let post = fam.get(server.url("/blog/family-post")).send().await.unwrap();
    assert_eq!(post.status(), StatusCode::OK);

    // Crawler surfaces stay anonymous even WITH the Family cookie.
    for path in ["/feed.xml", "/sitemap.xml"] {
        let body = fam.get(server.url(path)).send().await.unwrap().text().await.unwrap();
        assert!(
            !body.contains("family-post") && !body.contains("Family Post"),
            "{path} must never carry gated content, session or not"
        );
        assert!(body.contains("open-post"), "{path} still carries public content");
    }
}

/// The narrowed special-page exemption, end to end: a min_role on a SPECIAL row
/// darkens its ENTIRE section — the code route, the /pages redirect, the nav
/// tab, home's Latest band, the sitemap and the feed — for insufficient
/// viewers (its PUBLIC children included: ancestor gate), while Family still
/// enters everywhere. This is the seam the DE `library` row will stand on.
#[tokio::test]
async fn gated_special_page_darkens_its_whole_section() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_blog_post("kin-post", "# Kin Post\n\npublic child of a gated section")
        .await
        .expect("seed");
    stamp_min_role(&server.pool, "blog", "Family").await;

    // Anonymous: every surface is dark.
    let anon = client();
    for path in ["/blog", "/blog/kin-post"] {
        let resp = anon.get(server.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "anon {path} must be dark");
    }
    // /pages/<special> redirects even for an insufficient viewer (the DE
    // wrinkle: a special leaf's only possible failure is ROLE, and its route
    // name is public knowledge) — the TARGET route then stays dark (/blog
    // cat-404s above; /library shows its sign-in gate). DATA pages keep the
    // miss shape — /blog/kin-post above proves the subtree stays dark.
    let resp = anon.get(server.url("/pages/blog")).send().await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "anon /pages/blog redirects to the (dark) code route"
    );
    assert_eq!(
        resp.headers().get("location").unwrap().to_str().unwrap(),
        "/blog",
        "the redirect goes to the section route, never a data URL"
    );
    let home = anon.get(server.url("/")).send().await.unwrap().text().await.unwrap();
    assert!(!home.contains("/pages/blog"), "nav must drop the gated section's tab");
    assert!(!home.contains("Kin Post"), "home bands must drop the gated section's children");
    for path in ["/sitemap.xml", "/feed.xml"] {
        let body = anon.get(server.url(path)).send().await.unwrap().text().await.unwrap();
        assert!(
            !body.contains("/blog") && !body.contains("kin-post"),
            "{path} must not leak the gated section"
        );
    }

    // Family: the section opens — code route lists the post, the /pages leaf
    // redirects onward, the post itself serves.
    let fam = client();
    fam.post(server.url("/test/login?role=Family")).send().await.unwrap();
    let index = fam.get(server.url("/blog")).send().await.unwrap();
    assert_eq!(index.status(), StatusCode::OK);
    assert!(index.text().await.unwrap().contains("Kin Post"));
    let redirect = fam.get(server.url("/pages/blog")).send().await.unwrap();
    assert!(
        redirect.status().is_redirection(),
        "the special-leaf redirect must pass for a sufficient viewer: {}",
        redirect.status()
    );
    let post = fam.get(server.url("/blog/kin-post")).send().await.unwrap();
    assert_eq!(post.status(), StatusCode::OK);

    // Role-aware nav (DB.4): the gated section's tab shows FOR Family — the
    // other half of the tab-hide the anonymous assertions above pin.
    let home = fam.get(server.url("/")).send().await.unwrap().text().await.unwrap();
    assert!(
        home.contains("/pages/blog"),
        "the role-aware nav must show the gated tab to a sufficient viewer"
    );
}

// ──────────────────── Phase DC: media byte-gating ────────────────────

/// Upload a small generic file through the REAL multipart path (admin), with an
/// optional visibility field, returning its media_ref.
async fn upload_test_file(
    server: &hotchkiss_io::test_support::TestServer,
    admin: &reqwest::Client,
    min_role: Option<&str>,
) -> String {
    // DISTINCT bytes per visibility: identical bytes dedup to one sha → one
    // url_key, and the strictest-wins gate would then (correctly!) gate the
    // "public" copy too — the shared-sha rule the DAO unit test pins.
    let content = format!("stored bytes for {}", min_role.unwrap_or("public"));
    let mut form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(content.into_bytes())
            .file_name(format!("test-{}.zip", min_role.unwrap_or("public"))),
    );
    if let Some(r) = min_role {
        form = form.text("min_role", r.to_string());
    }
    let resp = admin
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "upload failed: {}", resp.status());
    resp.json::<serde_json::Value>().await.unwrap()["ref"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn url_key_for_ref(pool: &sqlx::SqlitePool, media_ref: &str) -> String {
    sqlx::query(
        "SELECT v.url_key FROM media_variant v JOIN media m ON m.media_id = v.media_id \
         WHERE m.media_ref = ?1 LIMIT 1",
    )
    .bind(media_ref)
    .fetch_one(pool)
    .await
    .unwrap()
    .get("url_key")
}

/// DS.1: on save, a pasted `/media/file/<url_key>` byte URL in page content is
/// rewritten to the stable `/media/<ref>` — the `![]()` embed AND the `[]()` link
/// forms — while an unresolvable byte URL is LEFT ALONE. So a variant re-encode
/// (which mints a new url_key) never strands a content link, and gated media stays
/// gate-correct (a `/media/<ref>` embed is fetched per viewer). Needs ffprobe (upload).
#[tokio::test]
async fn save_rewrites_media_byte_urls_to_stable_refs() {
    if !ffprobe_available() {
        eprintln!("skipping media-byte-url rewrite test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Upload → a real item (ref R) + resolve its per-save byte url_key K.
    let media_ref = upload_test_file(&server, &admin, None).await;
    let url_key = url_key_for_ref(&server.pool, &media_ref).await;
    // A valid-SHAPED (64 lowercase hex) but UNKNOWN key → must be left alone.
    let bogus_key = "0".repeat(64);

    server
        .seed_content_page("RewritePage", "# Rewrite Page\n\nseed")
        .await
        .expect("seed page");

    let markdown = format!(
        "![img](/media/file/{url_key})\n\n[download](/media/file/{url_key})\n\n![gone](/media/file/{bogus_key})"
    );
    let resp = admin
        .put(server.url("/pages/RewritePage"))
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", markdown.as_str()),
            ("page_cover_media_ref", ""),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "page PUT should succeed: {}", resp.status());

    // Re-open the editor → the STORED markdown is in the textarea.
    let edit = admin
        .get(server.url("/pages/RewritePage?edit=1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // Both resolvable byte URLs → the stable ref; the byte form is GONE.
    assert!(
        edit.contains(&format!("/media/{media_ref}")),
        "the resolvable byte URL(s) rewritten to the stable ref"
    );
    assert!(
        !edit.contains(&format!("/media/file/{url_key}")),
        "the per-save byte url_key must be gone from stored content"
    );
    // The unresolvable byte URL is untouched (typo-tolerant, like the cover-ref parse).
    assert!(
        edit.contains(&format!("/media/file/{bogus_key}")),
        "an unresolvable byte URL is left alone"
    );
}

/// DC end-to-end: an upload carrying min_role=Family mints a GATED item — anon
/// and Registered get miss-shaped denials on all three routes, Family gets the
/// bytes with PRIVATE caching; a plain upload stays public with shared caching.
#[tokio::test]
async fn gated_media_denies_anon_allows_family_caches_private() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let gated_ref = upload_test_file(&server, &admin, Some("Family")).await;
    let public_ref = upload_test_file(&server, &admin, None).await;
    let gated_key = url_key_for_ref(&server.pool, &gated_ref).await;
    let public_key = url_key_for_ref(&server.pool, &public_ref).await;

    // Anonymous + Registered: bytes and ref-download are the same 404 a bogus
    // key/ref gets; the embed is the same 200 error-span a bogus ref gets.
    for role in [None, Some("Registered")] {
        let c = client();
        if let Some(r) = role {
            c.post(server.url(&format!("/test/login?role={r}"))).send().await.unwrap();
        }
        assert_eq!(
            c.get(server.url(&format!("/media/file/{gated_key}"))).send().await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "bytes must deny {role:?}"
        );
        assert_eq!(
            c.get(server.url(&format!("/media/{gated_ref}"))).send().await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "ref-download must deny {role:?}"
        );
        let denied = c
            .get(server.url(&format!("/media/embed/{gated_ref}")))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::OK, "embed denial stays 200 (HTMX swap)");
        let denial_cc = denied.headers()[reqwest::header::CACHE_CONTROL].to_str().unwrap().to_string();
        assert!(denial_cc.contains("no-store"), "role-dependent embed must be no-store: {denial_cc}");
        let denied_body = denied.text().await.unwrap();
        let miss_body = c
            .get(server.url("/media/embed/no-such-ref"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(denied_body, miss_body, "embed denial ≡ bad-ref miss for {role:?}");
    }

    // Family: 302 on the ref, 200 + PRIVATE immutable caching on the bytes,
    // a real download button in the embed.
    let fam = client();
    fam.post(server.url("/test/login?role=Family")).send().await.unwrap();
    assert!(
        fam.get(server.url(&format!("/media/{gated_ref}"))).send().await.unwrap()
            .status()
            .is_redirection()
    );
    let bytes = fam
        .get(server.url(&format!("/media/file/{gated_key}")))
        .send()
        .await
        .unwrap();
    assert_eq!(bytes.status(), StatusCode::OK);
    let cc = bytes.headers()[reqwest::header::CACHE_CONTROL].to_str().unwrap().to_string();
    assert!(cc.contains("private") && cc.contains("immutable"), "gated bytes: {cc}");
    let embed = fam
        .get(server.url(&format!("/media/embed/{gated_ref}")))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(embed.contains("Download"), "family embed renders the element: {embed}");

    // The public item is untouched: anonymous 200 with SHARED caching.
    let pub_bytes = client()
        .get(server.url(&format!("/media/file/{public_key}")))
        .send()
        .await
        .unwrap();
    assert_eq!(pub_bytes.status(), StatusCode::OK);
    let cc = pub_bytes.headers()[reqwest::header::CACHE_CONTROL].to_str().unwrap().to_string();
    assert!(cc.contains("public") && cc.contains("immutable"), "public bytes: {cc}");
}

// ──────────────────── Phase DB: visibility authoring + role-aware nav ────────────────────

/// The full author→gate→deny loop through the REAL editor form: the Visibility
/// select gates a page, garbage or an absent field never loosens an existing
/// gate, and Public reopens it.
#[tokio::test]
async fn editor_visibility_select_authors_the_gate() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("VisPage", "# visible words")
        .await
        .expect("seed");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let put = |fields: Vec<(&'static str, &'static str)>| {
        let admin = &admin;
        let url = server.url("/pages/VisPage");
        async move {
            let r = admin.put(url).header("HX-Request", "true").form(&fields).send().await.unwrap();
            assert!(r.status().is_success(), "PUT should succeed: {}", r.status());
        }
    };
    let base = vec![
        ("page_category", ""),
        ("page_markdown", "# visible words"),
        ("page_order", "0"),
    ];

    // Gate it Family via the editor select.
    let mut f = base.clone();
    f.push(("min_role", "Family"));
    put(f).await;
    assert_eq!(
        client().get(server.url("/pages/VisPage")).send().await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "gated via the editor → anon 404"
    );
    let body = admin.get(server.url("/pages/VisPage")).send().await.unwrap().text().await.unwrap();
    assert!(body.contains("Family"), "admin reader view must badge the gate");

    // Garbage value → the gate is UNCHANGED (never silently loosened).
    let mut f = base.clone();
    f.push(("min_role", "Bogus"));
    put(f).await;
    assert_eq!(
        client().get(server.url("/pages/VisPage")).send().await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "a garbage select value must keep the existing gate"
    );

    // A PUT with NO min_role field (old client) also keeps the gate.
    put(base.clone()).await;
    assert_eq!(
        client().get(server.url("/pages/VisPage")).send().await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "an absent field must keep the existing gate"
    );

    // Public reopens it.
    let mut f = base.clone();
    f.push(("min_role", "Public"));
    put(f).await;
    assert_eq!(
        client().get(server.url("/pages/VisPage")).send().await.unwrap().status(),
        StatusCode::OK,
        "Public must clear the gate"
    );
}

/// Inherit-on-create (DB.3): a child created under a gated parent carries the
/// parent's gate from birth — belt and suspenders over the ancestor scan.
#[tokio::test]
async fn new_child_inherits_the_parent_gate() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_content_page("GatedTree", "# parent")
        .await
        .expect("seed");
    stamp_min_role(&server.pool, "GatedTree", "Family").await;

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let r = admin
        .post(server.url("/pages/GatedTree"))
        .header("HX-Request", "true")
        .form(&[("page_title", "Secret Child")])
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "child create should succeed: {}", r.status());

    let child_gate: Option<String> =
        sqlx::query("SELECT min_role FROM content_pages WHERE page_name = 'secret-child'")
            .fetch_one(&server.pool)
            .await
            .unwrap()
            .get("min_role");
    assert_eq!(child_gate.as_deref(), Some("Family"), "child must inherit the parent's gate");

    // And it behaves: anon 404, Family 200.
    assert_eq!(
        client().get(server.url("/pages/GatedTree/secret-child")).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let fam = client();
    fam.post(server.url("/test/login?role=Family")).send().await.unwrap();
    assert_eq!(
        fam.get(server.url("/pages/GatedTree/secret-child")).send().await.unwrap().status(),
        StatusCode::OK
    );
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
            .header("HX-Request", "true")
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

    // CQ.1.1: the panic-500 must ALSO be recorded. log_requests is layered OUTER to
    // CatchPanicLayer, so it observes the synthesized 500 — before the reorder the
    // panic unwound past the insert and the 500 never reached request_log (invisible
    // to the access log + analytics). Fire-and-forget insert → poll briefly.
    let mut logged = None;
    for _ in 0..100 {
        let rows =
            sqlx::query("SELECT status FROM request_log WHERE path = '/test/panic' ORDER BY id DESC")
                .fetch_all(&server.pool)
                .await
                .unwrap();
        if let Some(row) = rows.first() {
            logged = Some(row.get::<i64, _>("status"));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        logged,
        Some(500),
        "the panic-500 must land in request_log, not vanish past the log layer"
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
    // The editable jump-to-page form: a number input (current value + max) + "of N".
    assert!(
        p1.contains(r#"name="page""#) && p1.contains(r#"value="1""#) && p1.contains("of 2"),
        "editable pager present on page 1: {p1}"
    );
    assert!(p1.contains("Next"), "page 1 has a Next link");
    assert!(!p1.contains("Previous"), "page 1 has no Previous link");

    let p2 = reqwest::get(server.url("/blog?page=2")).await.unwrap().text().await.unwrap();
    assert_eq!(p2.matches("/blog/post-").count(), 2, "page 2 shows the remainder");
    assert!(
        p2.contains(r#"value="2""#) && p2.contains("of 2"),
        "the jump input reflects the current page"
    );
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
    assert!(s1.contains(r#"name="page""#) && s1.contains("of 2"), "filtered set paginates: {s1}");
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
    assert!(
        body.contains(r#"name="page""#) && body.contains("of 2"),
        "projects paginate via the shared machinery: {body}"
    );
    assert_eq!(
        body.matches("/pages/projects/proj-").count(),
        10,
        "page 1 shows PAGE_SIZE projects"
    );
}

/// The pager's "Page N of M" is an editable jump-to-page GET form (friendlier than
/// clicking Next 25× on a 26-page series): a number input bounded to [1, total] that
/// submits to base_path?page=N. No JS — native form GET. A search stays sticky via a
/// hidden q. And an out-of-range typed number is clamped by the handler, not a 500.
#[tokio::test]
async fn pager_is_an_editable_jump_to_page_form() {
    let server = spawn_test_server().await.expect("spawn");
    for i in 0..25 {
        server
            .seed_blog_post(&format!("jump-{i:02}"), "shared jumpword body")
            .await
            .unwrap();
    }
    // 25 posts, PAGE_SIZE 10 → 3 pages. The jump input is bounded to the total.
    let p1 = reqwest::get(server.url("/blog")).await.unwrap().text().await.unwrap();
    assert!(p1.contains(r#"<form method="get" action="/blog""#), "the pager is a GET form to base_path: {p1}");
    assert!(p1.contains(r#"type="number" name="page""#), "with a number page input");
    assert!(p1.contains(r#"max="3""#), "bounded to the total page count");
    assert!(p1.contains(r#"value="1""#), "defaulting to the current page");

    // Jumping straight to the last page works (what typing 3 + Enter navigates to).
    let p3 = reqwest::get(server.url("/blog?page=3")).await.unwrap().text().await.unwrap();
    assert_eq!(p3.matches("/blog/jump-").count(), 5, "page 3 shows the remainder (25 - 20)");
    assert!(p3.contains(r#"value="3""#), "the input reflects page 3");

    // An out-of-range jump is clamped to the last page, never a 500.
    let over = reqwest::get(server.url("/blog?page=999")).await.unwrap();
    assert_eq!(over.status(), StatusCode::OK, "an over-range page is clamped, not an error");

    // A search keeps its q sticky through the jump form (hidden field).
    let s = reqwest::get(server.url("/blog?q=jumpword")).await.unwrap().text().await.unwrap();
    assert!(
        s.contains(r#"type="hidden" name="q" value="jumpword""#),
        "the jump form preserves the active search: {s}"
    );
}

/// The cover field takes a "media ref", but the media library's only per-item copy
/// button hands you `![](/media/<ref>)` (and the other, `/media/file/<url_key>`).
/// Pasting either must SET the cover — the resolver has to extract the token, not
/// demand a bare ref (the old exact-match `find_by_ref` silently failed → cover
/// never set, and worse, wiped any existing one). Regression for "setting a cover
/// image for a project doesn't work".
#[tokio::test]
async fn setting_a_cover_accepts_the_copyable_media_forms() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_project("skylander", "# Skylander\n\nbody").await.unwrap();

    // A minimal media item + one image variant, as an upload would leave behind.
    let media_ref = "0190aaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let media_id: i64 = sqlx::query(
        "INSERT INTO media (media_ref, kind, title) VALUES (?1, 'Image', 'cover') RETURNING media_id",
    )
    .bind(media_ref)
    .fetch_one(&server.pool)
    .await
    .unwrap()
    .get("media_id");
    // A real 64-lowercase-hex url_key — what media_url_key emits, and what the DJ.4
    // UrlKey newtype gate requires (a non-hex /media/file/<key> paste is rejected).
    let url_key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    sqlx::query(
        "INSERT INTO media_variant (media_id, sha256, url_key, mime, bytes, width)
         VALUES (?1, 'sha', ?2, 'image/avif', 100, 480)",
    )
    .bind(media_id)
    .bind(url_key)
    .execute(&server.pool)
    .await
    .unwrap();

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let put = |cover: &str| {
        admin
            .put(server.url("/pages/projects/skylander"))
            .header("HX-Request", "true")
            .form(&[
                ("page_title", "Skylander"),
                ("page_category", ""),
                ("page_markdown", "# Skylander\n\nbody"),
                ("page_cover_media_ref", cover),
                ("page_order", "0"),
                ("page_creation_date", ""),
            ])
            .send()
    };
    let cover_id = || async {
        sqlx::query("SELECT page_cover_media_id FROM content_pages WHERE page_name = 'skylander'")
            .fetch_one(&server.pool)
            .await
            .unwrap()
            .get::<Option<i64>, _>("page_cover_media_id")
    };

    // The "Copy ![]()" button output — the natural thing to paste.
    let r = put(&format!("![](/media/{media_ref})")).await.unwrap();
    assert!(r.status().is_success(), "cover PUT: {}", r.status());
    assert_eq!(cover_id().await, Some(media_id), "markdown-embed form must set the cover");

    // The whole point: the /projects card now renders the cover image (this is the
    // user-visible symptom — before the fix the card fell back to the cubes icon).
    let index = reqwest::get(server.url("/projects")).await.unwrap().text().await.unwrap();
    assert!(
        index.contains(&format!("/media/file/{url_key}")),
        "the project card must show the cover image after it's set"
    );

    // Clearing the field removes the cover.
    assert!(put("").await.unwrap().status().is_success());
    assert_eq!(cover_id().await, None, "empty field clears the cover");

    // The bare "/media/<ref>" form and a bare ref both resolve too.
    put(&format!("/media/{media_ref}")).await.unwrap();
    assert_eq!(cover_id().await, Some(media_id), "/media/<ref> form must set the cover");
    put("").await.unwrap();
    put(media_ref).await.unwrap();
    assert_eq!(cover_id().await, Some(media_id), "a bare ref still resolves");

    // The "Copy link" button output — `/media/file/<url_key>` — resolves via the variant.
    put("").await.unwrap();
    put(&format!("/media/file/{url_key}")).await.unwrap();
    assert_eq!(cover_id().await, Some(media_id), "/media/file/<url_key> must set the cover");

    // A garbage non-empty ref must NOT wipe an existing cover (typo != clear).
    put(&format!("![](/media/{media_ref})")).await.unwrap(); // re-set
    put("![](/media/does-not-exist)").await.unwrap();
    assert_eq!(cover_id().await, Some(media_id), "an unresolvable ref preserves the cover");
}

// ───────────────────── Phase 13: featured landing page ─────────────────────

#[tokio::test]
async fn landing_page_serves_doors_and_auto_latest() {
    // `/` is now a real landing (not the old redirect): three pillar doors +
    // a self-maintaining "Latest" strip pulled from newest blog + project pages.
    let server = spawn_test_server().await.expect("spawn");
    server.seed_blog_post("hello-world", "# Hello World\n\nfirst post body").await.unwrap();
    server.seed_project("skylander", "# Skylander Mount\n\na printed bracket").await.unwrap();

    let resp = client().get(server.url("/")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "/ must render the landing, not redirect");
    let body = resp.text().await.unwrap();

    // The three live pillar doors.
    assert!(body.contains("href=\"/projects\""), "Projects door present");
    assert!(body.contains("href=\"/blog\""), "Writing door present");
    assert!(body.contains("href=\"/resume\""), "Résumé door present");

    // Above-the-fold connect links (13.5).
    assert!(body.contains("github.com/chotchki"), "GitHub link present");
    assert!(body.contains("mailto:chris@hotchkiss.io"), "Email link present");

    // Latest strip surfaces both sections, linking to the real detail routes.
    assert!(body.contains("Latest"), "Latest heading present");
    assert!(body.contains("Hello World"), "newest blog post shown in Latest");
    assert!(body.contains("/blog/hello-world"), "blog card links to the post");
    assert!(body.contains("Skylander Mount"), "newest project shown in Latest");
    assert!(
        body.contains("/pages/projects/skylander"),
        "project card links into the /pages tree, not a dead /projects/<slug>"
    );
}

/// Phase 13.8: the Pin button toggles the reserved `featured` category tag, and a
/// pinned page moves into the landing's Featured band (above Latest) and OUT of
/// Latest (no duplicate). Admin-gated.
#[tokio::test]
async fn pinning_features_a_page_on_the_landing() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_project("show-piece", "# Show Piece\n\nthe flagship build").await.unwrap();
    let page_id: i64 =
        sqlx::query("SELECT page_id FROM content_pages WHERE page_name = 'show-piece'")
            .fetch_one(&server.pool)
            .await
            .unwrap()
            .get("page_id");

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // The editor renders the Pin button, wired to the id-based toggle endpoint.
    let editor = admin
        .get(server.url("/pages/projects/show-piece?edit=1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        editor.contains(&format!("hx-post=\"/admin/pages/{page_id}/feature\"")),
        "editor shows the Pin button posting to the feature endpoint"
    );
    assert!(!editor.contains("Unpin"), "button reads 'Pin' (not 'Unpin') before featuring");

    // An anonymous caller cannot pin (fail-closed non-GET layer → 401, missing identity).
    let anon = client();
    let r = anon.post(server.url(&format!("/admin/pages/{page_id}/feature"))).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "anonymous pin must be unauthorized");

    let category = || async {
        sqlx::query("SELECT page_category FROM content_pages WHERE page_id = ?1")
            .bind(page_id)
            .fetch_one(&server.pool)
            .await
            .unwrap()
            .get::<Option<String>, _>("page_category")
    };

    // Pin: the `featured` tag lands in page_category.
    let r = admin.post(server.url(&format!("/admin/pages/{page_id}/feature"))).send().await.unwrap();
    assert!(r.status().is_success(), "pin: {}", r.status());
    assert_eq!(category().await.as_deref(), Some("featured"), "pin adds the featured tag");

    // The landing now shows it under Featured, and NOT under Latest (no dupe).
    let body = client().get(server.url("/")).send().await.unwrap().text().await.unwrap();
    let feat_idx = body.find("Featured").expect("Featured band present");
    let show_idx = body.find("Show Piece").expect("pinned page shown");
    assert!(feat_idx < show_idx, "the pinned card sits under the Featured heading");
    assert_eq!(body.matches("Show Piece").count(), 1, "pinned page shown once (Featured, not also Latest)");

    // Unpin: the tag is removed, the column clears to NULL, and Featured disappears.
    admin.post(server.url(&format!("/admin/pages/{page_id}/feature"))).send().await.unwrap();
    assert_eq!(category().await, None, "unpin clears the tag to NULL");
    let body = client().get(server.url("/")).send().await.unwrap().text().await.unwrap();
    assert!(!body.contains(">Featured<"), "no Featured band once nothing is pinned");
    assert!(body.contains("Show Piece"), "unpinned page falls back into Latest");
}

#[tokio::test]
async fn landing_page_empty_state_still_renders_doors() {
    // With no blog/project children, the Latest section is omitted but the doors
    // (and the page) still render — never a 500.
    let server = spawn_test_server().await.expect("spawn");
    let resp = client().get(server.url("/")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("href=\"/projects\""), "doors render with no content");
    assert!(!body.contains(">Latest<"), "no Latest heading when there's nothing to show");
}

/// Phase (3MF): the 3MF viewer loader + its fflate dependency must actually ship
/// and serve at the importmap paths — a broken import at the top of
/// `htmx-stl-view.js` would break EVERY STL viewer, not just 3MF. Guards the
/// vendoring (and the `../libs/fflate.module.js` relative path 3MFLoader uses).
#[tokio::test]
async fn threejs_3mf_loader_assets_are_served() {
    let server = spawn_test_server().await.expect("spawn");
    for path in [
        "/vendor/threejs/three.module.js",
        "/vendor/threejs/loaders/STLLoader.js",
        "/vendor/threejs/loaders/3MFLoader.js",
        "/vendor/threejs/libs/fflate.module.js",
    ] {
        let r = reqwest::get(server.url(path)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{path} must serve");
        let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(
            ct.contains("javascript"),
            "{path} should be served as JS, got {ct}"
        );
    }
}

/// Phase DV: the child-index widget — a ` ```children ` fence in a page's markdown
/// renders that page's CHILD pages as a card grid (with links), and the raw
/// sentinel is filled, not left in the output. The unified listing mechanism behind
/// manga series pages + the audiobooks section.
#[tokio::test]
async fn child_index_widget_lists_children() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // A parent "shelf" carrying the child-index fence, plus two children.
    admin.post(server.url("/pages")).form(&[("page_title", "Shelf")]).send().await.unwrap();
    admin
        .put(server.url("/pages/shelf"))
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# Shelf\n\n```children order=manual\n```\n"),
            ("page_cover_media_ref", ""),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    for t in ["Volume One", "Volume Two"] {
        admin.post(server.url("/pages/shelf")).form(&[("page_title", t)]).send().await.unwrap();
    }

    // Anonymous read → the widget lists the children as cards linking to their pages.
    let body = reqwest::get(server.url("/pages/shelf")).await.unwrap().text().await.unwrap();
    assert!(body.contains("Volume One"), "widget lists child 1");
    assert!(body.contains("Volume Two"), "widget lists child 2");
    assert!(
        body.contains("/pages/shelf/volume-one"),
        "a card links to the child page: {body}"
    );
    assert!(
        !body.contains("class=\"child-index\""),
        "the sentinel must be FILLED, not left raw in the output"
    );
}

/// Phase DV: an uploaded `.epub` becomes a `MediaKind::Epub` item (extension-typed,
/// no ffprobe; `dominant_kind` keeps a lone epub Epub — the regression that shipped
/// it as a File download), and its embed is the foliate READER shell carrying the
/// gated byte URL — NOT a plain download link.
#[tokio::test]
async fn uploaded_epub_renders_the_foliate_reader_embed() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let epub =
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test.epub")).unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(epub)
            .file_name("vol01.epub")
            .mime_str("application/epub+zip")
            .unwrap(),
    );
    let resp = admin.post(server.url("/media")).multipart(form).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "epub upload (no ffprobe needed)");
    let media_ref =
        resp.json::<serde_json::Value>().await.unwrap()["ref"].as_str().unwrap().to_string();

    let embed = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(embed.contains("class=\"epub-reader"), "epub renders the reader shell: {embed}");
    assert!(embed.contains("epub-splash"), "the reader shell carries the boot splash");
    assert!(embed.contains("/media/file/"), "carries the gated byte URL for the reader to fetch");
    assert!(!embed.contains("media-download"), "must NOT be a plain File download link");
}

/// Phase DV.12 + DZ.5: the reorder UI is the drag+position LIST in the EDITOR (`?edit`)
/// — the /admin/pages pattern — NOT the read-only card widget. The endpoint persists
/// `page_order`. The card grid (reader + preview) is read-only for everyone.
#[tokio::test]
async fn child_index_drag_reorder_persists_page_order() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // A parent carrying the manual-order fence + three children.
    admin.post(server.url("/pages")).form(&[("page_title", "Shelf")]).send().await.unwrap();
    admin
        .put(server.url("/pages/shelf"))
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# Shelf\n\n```children order=manual\n```\n"),
            ("page_cover_media_ref", ""),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    for t in ["Alpha", "Bravo", "Charlie"] {
        admin.post(server.url("/pages/shelf")).form(&[("page_title", t)]).send().await.unwrap();
    }
    let parent_id: i64 =
        sqlx::query_scalar("SELECT page_id FROM content_pages WHERE page_name = 'shelf'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let id = |name: &str| {
        let pool = server.pool.clone();
        let name = name.to_string();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT page_id FROM content_pages WHERE page_name = ?1")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let (a, b, c) = (id("alpha").await, id("bravo").await, id("charlie").await);

    // The EDITOR (?edit) carries the drag+position reorder list, targeting the children
    // endpoint with the parent id + a numeric position input per row (the long-list
    // mechanism). The READ-ONLY card widget (non-edit admin) has NO sortable.
    let editor = admin.get(server.url("/pages/shelf?edit=1")).send().await.unwrap().text().await.unwrap();
    assert!(editor.contains("class=\"sortable"), "the editor list is draggable");
    assert!(editor.contains("/admin/pages/reorder-children"), "posts to the children reorder");
    assert!(editor.contains(&format!("name=\"parent_id\" value=\"{parent_id}\"")), "carries the parent id");
    assert!(editor.contains("data-reorder-position"), "each row has a numeric position input");
    let reader = admin.get(server.url("/pages/shelf")).send().await.unwrap().text().await.unwrap();
    assert!(!reader.contains("class=\"sortable"), "the card widget is read-only (no reorder)");

    // Reorder to Charlie, Alpha, Bravo (repeated page_id keys → a Vec).
    let form: Vec<(&str, String)> = vec![
        ("parent_id", parent_id.to_string()),
        ("start", "0".to_string()),
        ("page_id", c.to_string()),
        ("page_id", a.to_string()),
        ("page_id", b.to_string()),
    ];
    let resp = admin.post(server.url("/admin/pages/reorder-children")).form(&form).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let order = |page_id: i64| {
        let pool = server.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT page_order FROM content_pages WHERE page_id = ?1")
                .bind(page_id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(order(c).await, 0, "Charlie moved first");
    assert_eq!(order(a).await, 1);
    assert_eq!(order(b).await, 2);

    // Anti-tamper: an id that isn't a child of the parent is rejected.
    let bad: Vec<(&str, String)> = vec![
        ("parent_id", parent_id.to_string()),
        ("page_id", parent_id.to_string()), // the parent itself isn't its own child
    ];
    let resp = admin.post(server.url("/admin/pages/reorder-children")).form(&bad).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "a non-child id is rejected");

    // A non-admin gets a plain list, no sortable form.
    let anon = reqwest::get(server.url("/pages/shelf")).await.unwrap().text().await.unwrap();
    assert!(!anon.contains("class=\"sortable"), "non-admin gets no drag handles");
    assert!(anon.contains("Alpha"), "but still sees the listing");
}

/// Phase DV.11: a child-index card whose page has NO explicit cover derives one from
/// the FIRST `![](/media/<ref>)` embed in the page's content → that media item's image
/// variant (here the EPUB's extracted OPF cover). So a book/volume auto-covers its card
/// with zero manual cover-setting.
#[tokio::test]
async fn child_card_cover_derives_from_embedded_media() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // An epub upload → an Epub item carrying an extracted OPF cover (image variant).
    let epub =
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test.epub")).unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(epub)
            .file_name("v1.epub")
            .mime_str("application/epub+zip")
            .unwrap(),
    );
    let media_ref = admin
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["ref"]
        .as_str()
        .unwrap()
        .to_string();

    // A series parent carrying the fence + a volume child that embeds the epub (no
    // explicit page cover set on the child).
    admin.post(server.url("/pages")).form(&[("page_title", "Series")]).send().await.unwrap();
    admin
        .put(server.url("/pages/series"))
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# Series\n\n```children order=manual\n```\n"),
            ("page_cover_media_ref", ""),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    admin.post(server.url("/pages/series")).form(&[("page_title", "Volume 1")]).send().await.unwrap();
    let vol_md = format!("# Volume 1\n\n![](/media/{media_ref})\n");
    admin
        .put(server.url("/pages/series/volume-1"))
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", vol_md.as_str()),
            ("page_cover_media_ref", ""),
            ("page_order", "1"),
        ])
        .send()
        .await
        .unwrap();

    // The series listing card carries a derived cover byte URL — not the placeholder.
    let body = reqwest::get(server.url("/pages/series")).await.unwrap().text().await.unwrap();
    assert!(
        body.contains("Volume 1") && body.contains("/media/file/"),
        "the card cover derives from the embedded media's image variant: {body}"
    );
}

/// Phase DV.10: an uploaded EPUB's embedded cover (declared in the OPF) is extracted
/// at ingest + stored as an image variant, so `cover_url_for` auto-populates the card
/// + hero — the EPUB analog of the audiobook `attached_pic` poster. The item stays an
/// Epub (the cover is its thumbnail, not its type).
#[tokio::test]
async fn epub_cover_is_extracted_as_an_image_variant() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let epub =
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test.epub")).unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(epub)
            .file_name("vol01.epub")
            .mime_str("application/epub+zip")
            .unwrap(),
    );
    let media_ref = admin
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["ref"]
        .as_str()
        .unwrap()
        .to_string();

    let img_variants: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_variant v JOIN media m ON m.media_id = v.media_id \
         WHERE m.media_ref = ?1 AND v.mime LIKE 'image/%'",
    )
    .bind(&media_ref)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert!(img_variants >= 1, "the OPF cover was extracted as an image variant");

    let kind: String = sqlx::query_scalar("SELECT kind FROM media WHERE media_ref = ?1")
        .bind(&media_ref)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(kind, "epub", "a book grouped with its cover stays an Epub item");

    // And the card cover resolves (cover_url_for picks the extracted image).
    let card =
        admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    assert!(card.contains(&media_ref), "the item shows in the library");
}

/// Phase DV: the vendored foliate-js EPUB reader engine serves as ES modules — the
/// entry (`view.js`), the EPUB parser it dynamically imports (`epub.js`), and the
/// zip loader under the relative `./vendor/` path view.js resolves against.
#[tokio::test]
async fn foliate_reader_assets_are_served() {
    let server = spawn_test_server().await.expect("spawn");
    for path in [
        "/vendor/foliate/view.js",
        "/vendor/foliate/epub.js",
        "/vendor/foliate/paginator.js",
        "/vendor/foliate/vendor/zip.js",
    ] {
        let r = reqwest::get(server.url(path)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{path} must serve");
        let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(ct.contains("javascript"), "{path} should be served as JS, got {ct}");
    }
}

/// `fab publish`'s Downloads links use `/media/<ref>` (the upload API returns a
/// media_ref, not the byte url_key). That must resolve — redirect to the item's
/// FULL-RES bytes — not 404 (the bowtie regression: the 3MF download link 404'd).
#[tokio::test]
async fn media_ref_download_redirects_to_full_res_bytes() {
    let server = spawn_test_server().await.expect("spawn");
    let media_ref = "019f2000-aaaa-bbbb-cccc-000000000001";
    let media_id: i64 = sqlx::query(
        "INSERT INTO media (media_ref, kind, title) VALUES (?1, 'Stl', 'Bowtie') RETURNING media_id",
    )
    .bind(media_ref)
    .fetch_one(&server.pool)
    .await
    .unwrap()
    .get("media_id");
    // A small preview + the full-res; the ref download must pick the LARGEST.
    for (key, bytes) in [("smallkey", 1000_i64), ("fullkey", 900_000_i64)] {
        sqlx::query(
            "INSERT INTO media_variant (media_id, sha256, url_key, mime, bytes) VALUES (?1, ?2, ?3, 'model/3mf', ?4)",
        )
        .bind(media_id)
        .bind(format!("sha-{key}"))
        .bind(key)
        .bind(bytes)
        .execute(&server.pool)
        .await
        .unwrap();
    }

    let r = client().get(server.url(&format!("/media/{media_ref}"))).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::TEMPORARY_REDIRECT, "ref download must redirect to bytes");
    let loc = r.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert_eq!(loc, "/media/file/fullkey", "redirects to the largest (full-res) variant, got {loc}");

    // Unknown ref → 404, not a redirect.
    let r = client().get(server.url("/media/019f2000-dead-0000-0000-000000000000")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

/// Featured is HAND-CURATED, so it must honor the manual `page_order` (the control
/// the /projects drag-reorder sets), NOT creation date like the auto Latest strip.
#[tokio::test]
async fn featured_section_respects_page_order() {
    let server = spawn_test_server().await.expect("spawn");
    // Pinned projects whose page_order deliberately differs from creation order.
    for (slug, order) in [("proj-a", 2_i64), ("proj-b", 0), ("proj-c", 1)] {
        server.seed_project(slug, &format!("# {slug}\n\nbody")).await.unwrap();
        sqlx::query("UPDATE content_pages SET page_category = 'featured', page_order = ?1 WHERE page_name = ?2")
            .bind(order)
            .bind(slug)
            .execute(&server.pool)
            .await
            .unwrap();
    }
    let body = client().get(server.url("/")).send().await.unwrap().text().await.unwrap();
    let region = &body[body.find(">Featured<").expect("Featured heading")..];
    let (pa, pb, pc) = (
        region.find("proj-a").expect("a"),
        region.find("proj-b").expect("b"),
        region.find("proj-c").expect("c"),
    );
    assert!(pb < pc && pc < pa, "Featured must order by page_order (b=0, c=1, a=2); got b={pb} c={pc} a={pa}");
}

/// The /library sign-in gate, all three viewer states (Phase DE). Code-defined
/// routes deliberately do NOT miss-shape — route names are public knowledge —
/// but the copy is state-aware and never names a tier, and everything
/// DATA-defined stays behind the cat-404 oracle (next test).
#[tokio::test]
async fn library_gate_states_and_family_entry() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_library_book("first-book", "# First Book\n\nA family read.")
        .await
        .expect("seed");

    // Anonymous → sign-in copy + a ?next login link, on BOTH code routes.
    let anon = client();
    for (path, next_href) in [
        ("/library", "/login?next=%2Flibrary"),
        ("/library/audiobooks", "/login?next=%2Flibrary%2Faudiobooks"),
    ] {
        let resp = anon.get(server.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "gate serves a page, not an error");
        let body = resp.text().await.unwrap();
        assert!(body.contains("Sign in"), "anon {path} must offer sign-in");
        assert!(body.contains(next_href), "{path} gate must carry its own ?next");
        assert!(!body.contains("First Book"), "{path} gate must not leak data");
    }

    // Authenticated-but-insufficient (a self-registered stranger) → neutral
    // restricted copy: no tier names, no sign-in loop, still no data.
    let reg = client();
    reg.post(server.url("/test/login?role=Registered")).send().await.unwrap();
    let body = reg
        .get(server.url("/library"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("restricted"), "insufficient copy is the neutral one");
    assert!(!body.contains("Sign in"), "no sign-in loop for an authenticated viewer");
    assert!(
        !body.contains("Family") && !body.contains("family"),
        "gate copy must never name the tier a stranger would need"
    );
    assert!(!body.contains("First Book"), "restricted gate must not leak data");

    // Family → doors index, book listing, and the book page itself.
    let fam = client();
    fam.post(server.url("/test/login?role=Family")).send().await.unwrap();
    let doors = fam
        .get(server.url("/library"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        doors.contains("/library/audiobooks"),
        "the audiobooks section door renders for Family"
    );
    let listing = fam
        .get(server.url("/library/audiobooks"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(listing.contains("First Book"), "the book card lists for Family");
    assert!(
        listing.contains("/pages/library/audiobooks/first-book"),
        "the card links into the content tree"
    );
    // The admin edit-section affordance (DE.7) never renders for Family.
    assert!(
        !doors.contains("Edit section") && !listing.contains("Edit section"),
        "edit-section links are admin-only"
    );
    let book = fam
        .get(server.url("/pages/library/audiobooks/first-book"))
        .send()
        .await
        .unwrap();
    assert_eq!(book.status(), StatusCode::OK, "Family reads the book page");

    // Nav: Family sees the tab; anonymous doesn't.
    assert!(doors.contains("/pages/library"), "Family nav carries the Library tab");
    let home = anon
        .get(server.url("/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!home.contains("/pages/library"), "anon nav must not show the tab");
}

/// DATA under /library stays oracle-shaped (Phase DE): a book page for an
/// insufficient viewer is byte-identical to a genuine miss, while the special
/// LEAF (/pages/library) redirects even when denied — its route name is
/// public, and the target shows the sign-in gate.
#[tokio::test]
async fn library_data_stays_miss_shaped_and_special_leaf_redirects() {
    let server = spawn_test_server().await.expect("spawn");
    server
        .seed_library_book("secret-book", "# Secret Book\n\nnot for strangers")
        .await
        .expect("seed");

    for role in [None, Some("Registered")] {
        let c = client();
        if let Some(r) = role {
            c.post(server.url(&format!("/test/login?role={r}"))).send().await.unwrap();
        }
        let real = c
            .get(server.url("/pages/library/audiobooks/secret-book"))
            .send()
            .await
            .unwrap();
        assert_eq!(real.status(), StatusCode::NOT_FOUND);
        let miss = c
            .get(server.url("/pages/library/audiobooks/no-such-book"))
            .send()
            .await
            .unwrap();
        assert_eq!(miss.status(), StatusCode::NOT_FOUND);
        let (real_body, miss_body) =
            (real.text().await.unwrap(), miss.text().await.unwrap());
        assert_eq!(
            real_body, miss_body,
            "role {role:?}: gated book vs genuine miss must be byte-identical"
        );

        // The special LEAF redirects to the code route (which gates) — the
        // one deliberate non-miss surface.
        let leaf = c.get(server.url("/pages/library")).send().await.unwrap();
        assert_eq!(leaf.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            leaf.headers().get("location").unwrap().to_str().unwrap(),
            "/library"
        );
    }
}

/// Login ?next end to end minus the passkey ceremony (Phase DE): a valid next
/// stashes in the session at /login and pops in the finish handler; the
/// open-redirect vectors never stash. The ceremony itself is covered by the
/// browser e2e — here the pop is exercised via the finish handlers' shared
/// helper by asserting the stash side: /login?next=X then the login page
/// renders (stash is invisible), and the WHATWG bypass strings are unit-vector
/// pinned in web::util::next_url.
#[tokio::test]
async fn login_next_param_accepts_paths_and_serves() {
    let server = spawn_test_server().await.expect("spawn");
    let c = client();
    for q in [
        "/login?next=%2Flibrary",
        "/login?next=%2F%5Cevil.com", // /\evil.com — rejected, page still serves
        "/login?next=%2F%2Fevil.com", // //evil.com — rejected, page still serves
        "/login",
    ] {
        let resp = c.get(server.url(q)).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{q} must serve the login page");
    }
}

/// The library authoring surface (Phase DE): /pages/library redirects for
/// everyone (special leaf), so section/book creation lives on the code index
/// pages as admin-only forms — the blog/projects pattern. The full loop:
/// create the section via /library's form target, a book via the section's,
/// and both land Family-gated (inherit-on-create from the seeded row).
#[tokio::test]
async fn library_admin_authoring_loop_creates_gated_pages() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Admin sees the create forms on the code index pages.
    let index = admin
        .get(server.url("/library"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(index.contains("hx-post=\"/pages/library\""), "New-section form renders");

    // Create the section + a book through the same form targets.
    admin
        .post(server.url("/pages/library"))
        .form(&[("page_title", "AudioBooks")])
        .send()
        .await
        .unwrap();
    let listing = admin
        .get(server.url("/library/audiobooks"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        listing.contains("hx-post=\"/pages/library/audiobooks\""),
        "New-book form renders once the section exists"
    );
    // Edit-section affordance (DE.7): the section's content page is shadowed
    // by the /library routes, so the SECTION index links its editor. The
    // doors page deliberately doesn't — it's the family-facing entry hall
    // (chris's call after seeing both).
    assert!(
        listing.contains("/pages/library/audiobooks?edit"),
        "the section index links the section editor for admin"
    );
    let doors = admin
        .get(server.url("/library"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !doors.contains("Edit section"),
        "the doors page stays free of admin edit links"
    );
    admin
        .post(server.url("/pages/library/audiobooks"))
        .form(&[("page_title", "Gated Book")])
        .send()
        .await
        .unwrap();

    // Inherit-on-create: both authored pages carry the parent's Family gate.
    for slug in ["audiobooks", "gated-book"] {
        let min_role: Option<String> = sqlx::query_scalar(
            "SELECT min_role FROM content_pages WHERE page_name = ?1",
        )
        .bind(slug)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert_eq!(min_role.as_deref(), Some("Family"), "{slug} inherits the gate");
    }

    // And the authored book stays miss-shaped for anonymous.
    let anon = client();
    let resp = anon
        .get(server.url("/pages/library/audiobooks/gated-book"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// DK.2: the auth split at the ONE global layer — a MISSING identity gets 401, an
/// authenticated-but-INSUFFICIENT identity gets 403 — and the 401 carries NO
/// `WWW-Authenticate` header (which would pop a browser basic-auth dialog AND
/// trigger an MCP client's OAuth discovery — the chase the DI design avoids).
#[tokio::test]
async fn auth_split_401_missing_403_insufficient_no_www_authenticate() {
    let server = spawn_test_server().await.expect("spawn");

    // Missing identity → 401, and crucially NO WWW-Authenticate challenge.
    let anon = client()
        .post(server.url("/admin/pages/reorder"))
        .form(&[("page_id", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED, "missing identity → 401");
    assert!(
        anon.headers().get("WWW-Authenticate").is_none(),
        "the 401 must NOT advertise a challenge (no OAuth chase / basic-auth popup)"
    );

    // Authenticated but insufficient (Registered) → 403.
    let registered = client();
    registered
        .post(server.url("/test/login?role=Registered"))
        .send()
        .await
        .unwrap();
    let insufficient = registered
        .post(server.url("/admin/pages/reorder"))
        .form(&[("page_id", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        insufficient.status(),
        StatusCode::FORBIDDEN,
        "authenticated but insufficient → 403"
    );
}

/// DK.1: a duplicate slug under the same parent is a 409 with an actionable
/// message over the editor's HTTP path — never the raw SQLite constraint text.
#[tokio::test]
async fn duplicate_slug_create_returns_409_over_http() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    // First create under /blog succeeds.
    let ok = admin
        .post(server.url("/pages/blog"))
        .form(&[("page_title", "Dup HTTP Post")])
        .send()
        .await
        .unwrap();
    // A native (non-HTMX) create is a 303 redirect to the new page in edit mode;
    // `client()` doesn't follow redirects, so accept the redirect as success.
    assert!(
        ok.status().is_redirection() || ok.status().is_success(),
        "first create ok: {}",
        ok.status()
    );

    // Same title → same slug → the UNIQUE(parent, slug) clash → 409, not a 500.
    let clash = admin
        .post(server.url("/pages/blog"))
        .form(&[("page_title", "Dup HTTP Post")])
        .send()
        .await
        .unwrap();
    assert_eq!(clash.status(), StatusCode::CONFLICT, "a slug clash is a 409");
    let body = clash.text().await.unwrap();
    assert!(body.contains("already exists"), "actionable message: {body}");
    assert!(
        !body.contains("UNIQUE") && !body.contains("content_pages"),
        "the raw SQLite constraint must not leak: {body}"
    );
}

// ---- Phase DM: auth flows fail loud (user + operator feedback) -------------

/// DM.4: the login page carries a `<noscript>` explainer, a server-fillable
/// error slot, and a native/htmx fallback that lands on `/login/js_required`
/// (not a silent reload or a 401 loop).
#[tokio::test]
async fn login_page_has_noscript_and_js_fallback() {
    let server = spawn_test_server().await.expect("spawn");
    let body = client()
        .get(server.url("/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("<noscript>"), "noscript explainer present");
    assert!(
        body.contains("/login/js_required"),
        "form falls back to the js_required route, not a silent reload"
    );
    assert!(
        body.contains("id=\"error_message\""),
        "the error slot the ceremony JS writes into is present"
    );
}

/// DM.4: the fallback route itself renders a real "you need JavaScript" message
/// at 200 — a public GET, so it never trips the mutation gate into a 401 loop.
#[tokio::test]
async fn js_required_fallback_renders_message() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = client()
        .get(server.url("/login/js_required"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("JavaScript"),
        "explicit message, not a silent reload: {}",
        &body[..body.len().min(400)]
    );
}

/// DM follow-up: the anonymous passkey-ceremony-failure BEACON is reachable without
/// auth (allowlisted in the mutation layer) and always `204`s (best-effort telemetry
/// — a garbage body never errors the client). Only POST is anonymous; another verb
/// hits the admin gate.
#[tokio::test]
async fn ceremony_error_beacon_is_anonymous_and_always_204() {
    let server = spawn_test_server().await.expect("spawn");

    // Anonymous POST with a real beacon body — NOT 401 (allowlisted), 204.
    let resp = client()
        .post(server.url("/login/ceremony_error"))
        .header("content-type", "application/json")
        .body(r#"{"action":"register","error_name":"NotAllowedError","error_message":"a request is already pending"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "anonymous beacon → 204, not 401");

    // A garbage body still 204s (telemetry is best-effort, never a 400/500).
    let junk = client()
        .post(server.url("/login/ceremony_error"))
        .body("not json at all")
        .send()
        .await
        .unwrap();
    assert_eq!(junk.status(), StatusCode::NO_CONTENT, "garbage beacon still 204");

    // Only POST is anonymous-allowed; a PUT hits the mutation gate → 401.
    let put = client()
        .request(reqwest::Method::PUT, server.url("/login/ceremony_error"))
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::UNAUTHORIZED, "only the POST beacon is anonymous");
}

/// DM.6: a taken display_name is rejected at `start_register` — BEFORE the
/// passkey is minted — with a real 409, and the raw PK-constraint text never
/// leaks (mirrors the duplicate-slug contract).
#[tokio::test]
async fn duplicate_display_name_rejected_before_ceremony() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_user("taken", "Registered").await.unwrap();

    let resp = client()
        .get(server.url("/login/start_register/taken"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a taken name is a 409 before the ceremony starts"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("already taken"), "actionable message: {body}");
    assert!(
        !body.to_uppercase().contains("UNIQUE") && !body.contains("users"),
        "the raw SQLite constraint / table must not leak: {body}"
    );
}

/// DM.6/DM.7: an AVAILABLE, valid name is NOT blocked — `start_register` returns
/// the real WebAuthn challenge (200). Guards against the pre-check over-blocking.
#[tokio::test]
async fn available_display_name_starts_registration() {
    let server = spawn_test_server().await.expect("spawn");
    let resp = client()
        .get(server.url("/login/start_register/brand-new-name"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("challenge") && body.contains("publicKey"),
        "returns the creation challenge: {}",
        &body[..body.len().min(200)]
    );
}

/// DM.7: an over-long name is a real 400 with a reason — not a 500, a silent
/// truncation, or a route 404.
#[tokio::test]
async fn invalid_display_name_rejected_with_reason() {
    let server = spawn_test_server().await.expect("spawn");
    let long = "x".repeat(65);
    let resp = client()
        .get(server.url(&format!("/login/start_register/{long}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.text().await.unwrap();
    assert!(body.contains("too long"), "reason surfaced: {body}");
}

/// DM follow-up: a STRANDED passkey — a valid assertion whose user handle has no
/// row (a registration whose `finish` failed, or a deleted account) — gets a
/// clean "not registered" 401 at login, NOT the old "User not found" 500
/// dead-end. `identify_discoverable_authentication` only reads the 16-byte
/// userHandle (no signature check), so a hand-crafted assertion with a
/// nonexistent handle hits the exact branch with no browser ceremony. The
/// userHandle here is 16 zero bytes (the nil UUID) — no such user in a fresh DB.
#[tokio::test]
async fn stranded_credential_login_is_a_clean_401_not_500() {
    let server = spawn_test_server().await.expect("spawn");
    let assertion = r#"{"id":"AAAA","rawId":"AAAA","response":{"authenticatorData":"AAAA","clientDataJSON":"AAAA","signature":"AAAA","userHandle":"AAAAAAAAAAAAAAAAAAAAAA"},"type":"public-key"}"#;
    let resp = client()
        .post(server.url("/login/finish_authentication"))
        .body(assertion)
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "stranded credential → clean 401, not a 500 or an extractor 4xx"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("register"), "actionable message: {body}");
}

// ---- Phase DN: open a model in the slicer ----------------------------------

/// A `.scad` upload is typed as OpenSCAD source, served with the right
/// Content-Type + CORP (so the COEP-isolated `/3d/editor` can fetch it), and its
/// embed offers the "Open in the slicer" button. Extension-typed, so no ffprobe.
#[tokio::test]
async fn scad_upload_serves_openscad_with_corp_and_slicer_embed() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"cube([10,10,10]);\n".to_vec())
            .file_name("bracket.scad")
            .mime_str("text/plain")
            .unwrap(),
    );
    let resp = admin
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "admin scad upload succeeds");
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["ref"].as_str().expect("ref").to_string();

    // The embed offers the slicer button; pull the served url_key from its href.
    let embed = admin
        .get(server.url(&format!("/media/embed/{media_ref}")))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(embed.contains("Open in the slicer"), "embed offers slicer: {embed}");
    // The slicer button now carries the round-trip URL (Phase DP): the STABLE ref +
    // ?format=scad, NOT the per-save url_key.
    assert!(
        embed.contains(&format!("/3d/editor?model=/media/{media_ref}?format=scad")),
        "slicer button carries ?model=/media/<ref>?format=scad: {embed}"
    );
    // Resolve the scad's byte url_key via the negotiated GET (?format=scad → 307) —
    // exercising the actual slicer load path.
    let redirect = admin
        .get(server.url(&format!("/media/{media_ref}?format=scad")))
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT, "?format=scad → 307");
    let url_key = location(&redirect)
        .expect("scad redirect Location")
        .strip_prefix("/media/file/")
        .expect("a byte URL")
        .to_string();

    // The byte route serves it as OpenSCAD source WITH CORP (public → the editor
    // can fetch it under COEP:require-corp).
    let bytes_resp = admin
        .get(server.url(&format!("/media/file/{url_key}")))
        .send()
        .await
        .unwrap();
    assert_eq!(bytes_resp.status(), StatusCode::OK);
    assert!(
        bytes_resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("application/x-openscad"),
        "served as OpenSCAD source: {:?}",
        bytes_resp.headers().get("content-type")
    );
    assert_eq!(
        bytes_resp
            .headers()
            .get("cross-origin-resource-policy")
            .and_then(|v| v.to_str().ok()),
        Some("cross-origin"),
        "CORP lets the isolated editor fetch it"
    );
}

// ───────────────────────── Phase DW: bulk manga ingest ─────────────────────────

/// An `.epub` multipart file part (typed so the probe reads it as Epub).
fn epub_part(bytes: Vec<u8>, name: &str) -> reqwest::multipart::Part {
    reqwest::multipart::Part::bytes(bytes)
        .file_name(name.to_string())
        .mime_str("application/epub+zip")
        .unwrap()
}

fn manga_fixture(n: u8) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/manga/series-v0{n}.epub",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Phase DW.2/DW.4/DW.5: the browser bulk-upload front door. A multi-file drop + a
/// series name creates the `library → manga → series` chain (Family-gated, inherited
/// from the seeded `library` row) and one ordered volume page per `.epub`, each
/// embedding its reader. Volumes order by the number PARSED from the filename, not by
/// upload order — so an out-of-order drop still reads 1..N.
#[tokio::test]
async fn manga_bulk_upload_creates_ordered_family_gated_volumes() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // The media library links to the import console (the discoverable entry point).
    let lib = admin.get(server.url("/admin/media")).send().await.unwrap().text().await.unwrap();
    assert!(
        lib.contains("/admin/media/import"),
        "the media library links to the bulk-import console"
    );

    // Drop OUT of order (3, 1, 2) to prove the parse drives ordering, not arrival.
    let form = reqwest::multipart::Form::new()
        .text("series", "One Piece")
        .part("f3", epub_part(manga_fixture(3), "series-v03.epub"))
        .part("f1", epub_part(manga_fixture(1), "series-v01.epub"))
        .part("f2", epub_part(manga_fixture(2), "series-v02.epub"));
    let resp = admin
        .post(server.url("/admin/media/import/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(body.contains("3</strong> created"), "report shows 3 created: {body}");

    // The whole chain inherits Family (library seeds it; each level inherits).
    let gate = |name: &str| {
        let pool = server.pool.clone();
        let name = name.to_string();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT min_role FROM content_pages WHERE page_name = ?1",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(gate("manga").await.as_deref(), Some("Family"), "manga section inherits Family");
    assert_eq!(gate("one-piece").await.as_deref(), Some("Family"), "series inherits Family");

    // The series' children are ordered volume-1..3 by the parsed number + Family-gated.
    let series_id: i64 =
        sqlx::query_scalar("SELECT page_id FROM content_pages WHERE page_name = 'one-piece'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT page_name, page_order, min_role FROM content_pages WHERE parent_page_id = ?1 ORDER BY page_order",
    )
    .bind(series_id)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    let names: Vec<&str> = rows.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, vec!["volume-1", "volume-2", "volume-3"], "ordered by parsed volume number");
    assert_eq!(rows[0].1, 1, "volume-1 order = parsed 1");
    assert_eq!(rows[2].1, 3, "volume-3 order = parsed 3");
    assert!(
        rows.iter().all(|(_, _, mr)| mr.as_deref() == Some("Family")),
        "every volume inherits the Family gate"
    );

    // The volume page serves the reader to an admin; an anon is 404'd (gated ancestor).
    let vol = admin
        .get(server.url("/pages/library/manga/one-piece/volume-1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        vol.contains("hx-get=\"/media/embed/"),
        "the volume page embeds its book (the reader loads via the embed route)"
    );
    let anon = reqwest::get(server.url("/pages/library/manga/one-piece/volume-1")).await.unwrap();
    assert_eq!(anon.status(), StatusCode::NOT_FOUND, "a gated volume is 404 to anon");

    // The series page lists its volumes (the stamped children fence is filled).
    let series_page = admin
        .get(server.url("/pages/library/manga/one-piece"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        series_page.contains("/pages/library/manga/one-piece/volume-1"),
        "the series page lists its volume cards"
    );
}

/// Phase DW.2: content-hash dedup makes a re-run idempotent — the same bytes under the
/// same series are skipped, not duplicated.
#[tokio::test]
async fn manga_bulk_upload_dedups_on_rerun() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let upload = |bytes: Vec<u8>| {
        let admin = admin.clone();
        let url = server.url("/admin/media/import/upload");
        async move {
            let form = reqwest::multipart::Form::new()
                .text("series", "Berserk")
                .part("f", epub_part(bytes, "series-v01.epub"));
            admin.post(url).multipart(form).send().await.unwrap().text().await.unwrap()
        }
    };

    let first = upload(manga_fixture(1)).await;
    assert!(first.contains("1</strong> created"), "first ingest creates the volume: {first}");
    let second = upload(manga_fixture(1)).await;
    assert!(second.contains("0</strong> created"), "re-run creates nothing");
    assert!(second.contains("1</strong> skipped"), "re-run skips the duplicate: {second}");

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM content_pages cp JOIN content_pages s ON cp.parent_page_id = s.page_id WHERE s.page_name = 'berserk'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "exactly one volume page — no duplicate");
}

/// Phase DW.3: the filesystem front door ingests a server-side folder of `.epub`s. It
/// SPAWNS (a real series copies tens of GB), so the POST returns "started" and the
/// volumes appear asynchronously — poll for them.
#[tokio::test]
async fn manga_filesystem_ingest_over_a_temp_dir() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    for n in 1..=3 {
        std::fs::write(dir.path().join(format!("series-v0{n}.epub")), manga_fixture(n)).unwrap();
    }
    let folder = dir.path().to_string_lossy().to_string();

    let resp = admin
        .post(server.url("/admin/media/import/filesystem"))
        .form(&[("series", "Naruto"), ("folder", folder.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.text().await.unwrap().contains("started"), "the spawned ingest reports started");

    // The spawn processes off-request — poll for all three volumes.
    let child_count = || {
        let pool = server.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM content_pages cp JOIN content_pages s ON cp.parent_page_id = s.page_id WHERE s.page_name = 'naruto'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let mut done = false;
    for _ in 0..80 {
        if child_count().await >= 3 {
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(done, "the filesystem ingest created 3 volumes under the series");

    // Ordered + Family-gated, same as the browser path.
    let series_id: i64 =
        sqlx::query_scalar("SELECT page_id FROM content_pages WHERE page_name = 'naruto'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT page_name, page_order, min_role FROM content_pages WHERE parent_page_id = ?1 ORDER BY page_order",
    )
    .bind(series_id)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    let names: Vec<&str> = rows.iter().map(|(n, _, _)| n.as_str()).collect();
    assert_eq!(names, vec!["volume-1", "volume-2", "volume-3"]);
    assert!(rows.iter().all(|(_, _, mr)| mr.as_deref() == Some("Family")), "volumes inherit Family");
    drop(dir);
}

/// Phase DW.8/DW.9/DW.10: a CBZ (comic zip) ingests as a Cbz item read by the SAME
/// foliate reader — the embed carries `data-kind="cbz"` so foliate's makeBook takes
/// the comic-book path — with its cover extracted from the first image in the zip.
#[tokio::test]
async fn manga_cbz_upload_renders_the_comic_reader_with_a_cover() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let cbz = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/manga/comic-v01.cbz"
    ))
    .unwrap();
    let form = reqwest::multipart::Form::new().text("series", "Comic Series").part(
        "f",
        reqwest::multipart::Part::bytes(cbz)
            .file_name("series-v01.cbz")
            .mime_str("application/vnd.comicbook+zip")
            .unwrap(),
    );
    let resp = admin
        .post(server.url("/admin/media/import/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.text().await.unwrap().contains("1</strong> created"),
        "the cbz ingest creates the volume"
    );

    // The item is typed Cbz (not File) with an extracted image cover variant.
    let kind: String = sqlx::query_scalar("SELECT kind FROM media LIMIT 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(kind, "cbz", "the item is typed Cbz, not a plain File download");
    let cover_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_variant mv JOIN media m ON mv.media_id = m.media_id WHERE m.kind = 'cbz' AND mv.mime LIKE 'image/%'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert!(cover_count >= 1, "the CBZ cover (first zip image) is stored as an image variant");

    // The embed (rendered by render_embed_html) is the shared reader shell, flagged as
    // a comic so foliate's makeBook takes the comic-book path.
    let media_ref: String = sqlx::query_scalar("SELECT media_ref FROM media WHERE kind = 'cbz'")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let embed = admin
        .get(server.url(&format!("/media/embed/{media_ref}")))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(embed.contains("class=\"epub-reader"), "renders the shared foliate reader shell");
    assert!(embed.contains("data-kind=\"cbz\""), "flagged as a comic so foliate picks the comic path");
    assert!(!embed.contains("media-download"), "not a plain File download");
}

/// Phase DW.12: a SERIES card (only a ` ```children ` fence — no cover, no embed of its
/// own) rolls up its first volume's cover, so the `/library/manga` tile isn't blank
/// after a volume-only import. The card's `<img>` cover comes one level down.
#[tokio::test]
async fn series_card_rolls_up_its_first_volume_cover() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // A CBZ carries a guaranteed first-image cover variant — ingest it as volume 1.
    let cbz = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/manga/comic-v01.cbz"
    ))
    .unwrap();
    let form = reqwest::multipart::Form::new().text("series", "Rollup Series").part(
        "f",
        reqwest::multipart::Part::bytes(cbz)
            .file_name("series-v01.cbz")
            .mime_str("application/vnd.comicbook+zip")
            .unwrap(),
    );
    let resp = admin
        .post(server.url("/admin/media/import/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The volume's cover url_key — the exact image the series card should borrow.
    let cover_key: String = sqlx::query_scalar(
        "SELECT mv.url_key FROM media_variant mv JOIN media m ON mv.media_id = m.media_id WHERE m.kind = 'cbz' AND mv.mime LIKE 'image/%' LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();

    // The manga SECTION lists SERIES cards. The series page has no cover/embed of its
    // own — the tile's `<img>` proves the roll-up borrowed volume 1's cover.
    let section = admin
        .get(server.url("/library/manga"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        section.contains("/pages/library/manga/rollup-series"),
        "the section lists the series card"
    );
    assert!(
        section.contains(&format!("/media/file/{cover_key}")),
        "the series card rolled up volume 1's cover: {section}"
    );
}

/// Breadcrumbs on a deep page are CLICKABLE ancestor links (chris's ask): a manga
/// volume at `/pages/library/manga/<series>/<volume>` shows `Manga › <Series>` as
/// links to their cumulative `/pages/…` URLs. The top section (library) is dropped
/// (the nav carries it) and the leaf (the volume you're on) is not a link.
#[tokio::test]
async fn breadcrumbs_are_clickable_ancestor_links() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Build a 4-deep tree via the manga ingest: library → manga → bread-series → volume-1.
    let form = reqwest::multipart::Form::new()
        .text("series", "Bread Series")
        .part("f1", epub_part(manga_fixture(1), "series-v01.epub"));
    admin
        .post(server.url("/admin/media/import/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    let vol = admin
        .get(server.url("/pages/library/manga/bread-series/volume-1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // The two ancestor crumbs are links to their cumulative paths.
    assert!(
        vol.contains(r#"<a href="/pages/library/manga""#),
        "the Manga crumb links to the section: {vol}"
    );
    assert!(
        vol.contains(r#"<a href="/pages/library/manga/bread-series""#),
        "the series crumb links to the series page"
    );
    // The leaf (the volume you're on) is NOT a link anywhere on its own page — the
    // breadcrumb omits it. (`/pages/library` is deliberately NOT asserted-absent: the
    // nav's Library tab links it, so its presence isn't a breadcrumb signal.)
    assert!(
        !vol.contains(r#"<a href="/pages/library/manga/bread-series/volume-1""#),
        "the current page is not a self-link in the trail"
    );
}

/// Phase DY: a nested volume page shows prev/next sibling buttons in page_order (the
/// volume reading order), each linking the sibling's FULL /pages path, with a side
/// omitted at the ends. The middle volume also emits the cross-page autoplay hook
/// (#autoplay-next → the next volume) that audio-player.js reads on `ended`.
#[tokio::test]
async fn nested_volume_shows_prev_next_and_autoplay_hook() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // library → manga → nav-series → volume-1..3 (drop out of order to prove ordering).
    let form = reqwest::multipart::Form::new()
        .text("series", "Nav Series")
        .part("f3", epub_part(manga_fixture(3), "series-v03.epub"))
        .part("f1", epub_part(manga_fixture(1), "series-v01.epub"))
        .part("f2", epub_part(manga_fixture(2), "series-v02.epub"));
    admin
        .post(server.url("/admin/media/import/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();

    let get = |slug: &str| {
        let admin = admin.clone();
        let url = server.url(&format!("/pages/library/manga/nav-series/{slug}"));
        async move { admin.get(url).send().await.unwrap().text().await.unwrap() }
    };

    // Middle volume: BOTH sides, full-path hrefs, + the autoplay hook → the next volume.
    let v2 = get("volume-2").await;
    assert!(v2.contains("Previous"), "middle has Previous: {v2}");
    assert!(v2.contains("Next"), "middle has Next");
    assert!(
        v2.contains(r#"href="/pages/library/manga/nav-series/volume-1""#),
        "Previous links volume-1 by full path"
    );
    assert!(
        v2.contains(r#"href="/pages/library/manga/nav-series/volume-3""#),
        "Next links volume-3 by full path"
    );
    assert!(
        v2.contains(r#"id="autoplay-next" data-href="/pages/library/manga/nav-series/volume-3""#),
        "the autoplay hook points at the next volume: {v2}"
    );

    // First volume: Next only (→ v2), no Previous.
    let v1 = get("volume-1").await;
    assert!(!v1.contains("Previous"), "first volume has no Previous: {v1}");
    assert!(v1.contains(r#"href="/pages/library/manga/nav-series/volume-2""#), "Next → v2");

    // Last volume: Previous only (→ v2), no Next, and NO autoplay hook (nothing after).
    let v3 = get("volume-3").await;
    assert!(!v3.contains("Next"), "last volume has no Next: {v3}");
    assert!(v3.contains(r#"href="/pages/library/manga/nav-series/volume-2""#), "Previous → v2");
    assert!(!v3.contains("autoplay-next"), "no autoplay hook on the last volume");
}

/// Phase DW.13: the cover roll-up descends MORE than one level, so a `series → season
/// → episode` tree covers the SERIES card from the first episode two levels down (the
/// season page is itself just a ` ```children ` fence). The single-level DW.12 roll-up
/// left the series card blank once a season layer was inserted.
#[tokio::test]
async fn series_card_rolls_up_a_cover_two_levels_deep() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // A cover-bearing media item (a CBZ auto-extracts a first-page cover).
    let cbz = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/manga/comic-v01.cbz"
    ))
    .unwrap();
    let up = admin
        .post(server.url("/media"))
        .multipart(reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(cbz)
                .file_name("ep.cbz")
                .mime_str("application/vnd.comicbook+zip")
                .unwrap(),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(up.status(), StatusCode::CREATED);
    let media_ref: String = sqlx::query_scalar("SELECT media_ref FROM media WHERE kind = 'cbz'")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let cover_key: String = sqlx::query_scalar(
        "SELECT mv.url_key FROM media_variant mv JOIN media m ON mv.media_id = m.media_id WHERE m.kind = 'cbz' AND mv.mime LIKE 'image/%' LIMIT 1",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();

    let fence = "```children order=manual\n```\n".to_string();
    let put = |path: &str, md: String| {
        let admin = admin.clone();
        let url = server.url(&format!("/pages/{path}"));
        async move {
            admin
                .put(url)
                .header("HX-Request", "true")
                .form(&[
                    ("page_category", ""),
                    ("page_markdown", md.as_str()),
                    ("page_cover_media_ref", ""),
                    ("page_order", "0"),
                ])
                .send()
                .await
                .unwrap();
        }
    };
    let create = |parent: &str, title: &str| {
        let admin = admin.clone();
        let url = if parent.is_empty() {
            server.url("/pages")
        } else {
            server.url(&format!("/pages/{parent}"))
        };
        let title = title.to_string();
        async move {
            admin.post(url).form(&[("page_title", title.as_str())]).send().await.unwrap();
        }
    };

    // Section (fence) → Show (fence) → Season (fence) → Episode (the media embed).
    create("", "Section").await;
    put("section", fence.clone()).await;
    create("section", "Show").await;
    put("section/show", fence.clone()).await;
    create("section/show", "Season").await;
    put("section/show/season", fence.clone()).await;
    create("section/show/season", "Episode").await;
    put("section/show/season/episode", format!("![](/media/{media_ref})")).await;

    // The Section grid renders the Show card. Show has only a fence (no cover/embed of
    // its own), so its card rolls up TWO levels — Show → Season → Episode — to the cover.
    let section = admin
        .get(server.url("/pages/section"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        section.contains("/pages/section/show"),
        "the section lists the show card: {section}"
    );
    assert!(
        section.contains(&format!("/media/file/{cover_key}")),
        "the show card rolled up the two-level-deep episode cover: {section}"
    );
}

/// Phase DZ.1: a ` ```children aspect=square ` fence renders the card cover boxes as
/// 1:1 tiles (square audiobook art fits with no crop); the default (no `aspect=`) stays
/// the 3:4 book portrait. The aspect rides the sentinel, so it survives the cache.
#[tokio::test]
async fn children_fence_aspect_controls_the_card_shape() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let put = |path: &str, md: &str| {
        let admin = admin.clone();
        let url = server.url(&format!("/pages/{path}"));
        let md = md.to_string();
        async move {
            admin
                .put(url)
                .header("HX-Request", "true")
                .form(&[
                    ("page_category", ""),
                    ("page_markdown", md.as_str()),
                    ("page_cover_media_ref", ""),
                    ("page_order", "0"),
                ])
                .send()
                .await
                .unwrap();
        }
    };

    // A square-aspect fence + one child (a coverless card still renders the aspect box).
    admin.post(server.url("/pages")).form(&[("page_title", "Squares")]).send().await.unwrap();
    put("squares", "```children order=newest aspect=square\n```\n").await;
    admin.post(server.url("/pages/squares")).form(&[("page_title", "Child A")]).send().await.unwrap();
    let sq = admin.get(server.url("/pages/squares")).send().await.unwrap().text().await.unwrap();
    assert!(sq.contains("aspect-square"), "square fence → square card boxes: {sq}");
    assert!(!sq.contains("aspect-[3/4]"), "square fence emits no portrait box");
    // DZ.6: the widget is `not-prose` — else the Typography plugin styles the card
    // cover <img> with article margins (whitespace above/below the cover) on the
    // `/pages/<…>` fence path (the grid renders inside `<div class="prose">`).
    assert!(sq.contains("not-prose"), "the widget opts out of prose img margins: {sq}");

    // Control: no aspect → the 3:4 portrait default.
    admin.post(server.url("/pages")).form(&[("page_title", "Portraits")]).send().await.unwrap();
    put("portraits", "```children order=newest\n```\n").await;
    admin.post(server.url("/pages/portraits")).form(&[("page_title", "Child B")]).send().await.unwrap();
    let pt = admin.get(server.url("/pages/portraits")).send().await.unwrap().text().await.unwrap();
    assert!(pt.contains("aspect-[3/4]"), "no aspect → portrait default: {pt}");
    assert!(!pt.contains("aspect-square"), "portrait emits no square box");
}

/// Phase DZ.2/DZ.3: the `/library/<section>` CODE route honors the section page's own
/// ` ```children ` fence aspect (instead of hardcoding), and the `/library` index
/// renders its sections through the SAME widget (cards linking into the content tree +
/// the admin "+ new section" form), not the old bespoke text doors.
#[tokio::test]
async fn library_uses_the_widget_and_honors_the_section_fence() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let put = |path: &str, md: &str| {
        let admin = admin.clone();
        let url = server.url(&format!("/pages/{path}"));
        let md = md.to_string();
        async move {
            admin
                .put(url)
                .header("HX-Request", "true")
                .form(&[
                    ("page_category", ""),
                    ("page_markdown", md.as_str()),
                    ("page_cover_media_ref", ""),
                    ("page_order", "0"),
                ])
                .send()
                .await
                .unwrap();
        }
    };

    // An audiobooks section under library with a SQUARE fence + a book child.
    admin.post(server.url("/pages/library")).form(&[("page_title", "AudioBooks")]).send().await.unwrap();
    put("library/audiobooks", "```children order=newest aspect=square\n```\n").await;
    admin.post(server.url("/pages/library/audiobooks")).form(&[("page_title", "Book One")]).send().await.unwrap();

    // The /library/<section> code route reads the section's fence → square cards.
    let section = admin.get(server.url("/library/audiobooks")).send().await.unwrap().text().await.unwrap();
    assert!(
        section.contains("aspect-square"),
        "the code route honors the section fence aspect: {section}"
    );

    // The /library index renders the section via the widget: a card into the content
    // tree + the admin "+ new section" form (both from render_children_grid).
    let index = admin.get(server.url("/library")).send().await.unwrap().text().await.unwrap();
    assert!(
        index.contains("/pages/library/audiobooks"),
        "the section is a widget card linking into the content tree: {index}"
    );
    assert!(
        index.contains(r#"hx-post="/pages/library""#),
        "the widget's new-section form is present"
    );
}

/// Phase DW.11: an EPUB with NO cover declared in the OPF (the Jujutsu Kaisen shape)
/// still gets a cover — the extractor falls back to the first image resource (page 1).
#[tokio::test]
async fn epub_without_opf_cover_falls_back_to_the_first_page() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let epub = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/manga/no-cover-v01.epub"
    ))
    .unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(epub)
            .file_name("Chapter 1.epub")
            .mime_str("application/epub+zip")
            .unwrap(),
    );
    let resp = admin.post(server.url("/media")).multipart(form).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Despite the OPF declaring no cover, an image variant (the first page) was added.
    let cover_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_variant mv JOIN media m ON mv.media_id = m.media_id WHERE m.kind = 'epub' AND mv.mime LIKE 'image/%'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(cover_count, 1, "the first-page fallback produced a cover variant");
}

/// Phase DW.11: the cover backfill gives a cover to a book that was ingested without
/// one (an image variant removed to simulate the pre-fallback import).
#[tokio::test]
async fn cover_backfill_covers_a_coverless_book() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let epub = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/manga/no-cover-v01.epub"
    ))
    .unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(epub)
            .file_name("Chapter 1.epub")
            .mime_str("application/epub+zip")
            .unwrap(),
    );
    admin.post(server.url("/media")).multipart(form).send().await.unwrap();

    // Simulate a pre-fallback import: strip the cover variant so the book is coverless.
    sqlx::query("DELETE FROM media_variant WHERE mime LIKE 'image/%'")
        .execute(&server.pool)
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_variant WHERE mime LIKE 'image/%'")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(before, 0, "book is coverless");

    // Trigger the backfill (spawned); poll for the cover to reappear.
    let resp = admin
        .post(server.url("/admin/media/import/backfill-covers"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut covered = false;
    for _ in 0..40 {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_variant WHERE mime LIKE 'image/%'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
        if n >= 1 {
            covered = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(covered, "the backfill re-extracted the cover");
}

/// Phase DW.11: a listing search skips the markdown body ONLY for an all-hex query —
/// so an `![](/media/<ref>)` embed's UUID hex can't pollute a numeric search (the
/// "80 → 35 results" bug) — while a PROSE body word still matches (body search kept).
#[tokio::test]
async fn listing_search_skips_body_only_for_hex_queries() {
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // A blog post whose BODY has a prose word ("zebra") AND a hex token ("deadbeef",
    // standing in for a media-ref UUID), but whose title/slug carry neither.
    admin.post(server.url("/pages")).form(&[("page_title", "Alpha")]).send().await.unwrap();
    admin
        .put(server.url("/pages/alpha"))
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# Alpha\n\nprose zebra and a hex token deadbeef here"),
            ("page_cover_media_ref", ""),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();

    // Move it under /blog so the listing search applies (children of the blog special page).
    let blog_id: i64 = sqlx::query_scalar("SELECT page_id FROM content_pages WHERE page_name = 'blog'")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE content_pages SET parent_page_id = ?1 WHERE page_name = 'alpha'")
        .bind(blog_id)
        .execute(&server.pool)
        .await
        .unwrap();

    // A prose body word still matches (body search preserved).
    let prose = reqwest::get(server.url("/blog?q=zebra")).await.unwrap().text().await.unwrap();
    assert!(prose.contains("/blog/alpha"), "a prose body word still matches (body search kept)");
    // An all-hex body token does NOT match (the pollution fix) — it's not in title/slug.
    let hex = reqwest::get(server.url("/blog?q=deadbeef")).await.unwrap().text().await.unwrap();
    assert!(!hex.contains("/blog/alpha"), "an all-hex query skips the body (no ref-UUID pollution)");
    // The title still matches regardless.
    let title = reqwest::get(server.url("/blog?q=Alpha")).await.unwrap().text().await.unwrap();
    assert!(title.contains("/blog/alpha"), "the title still matches");
}
