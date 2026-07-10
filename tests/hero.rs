//! Phase CV — cover hero images. A page with an image cover renders a stacked
//! hero banner at the top of its detail view (largest CN AVIF variant + srcset);
//! a page with no cover renders none.

use hotchkiss_io::test_support::spawn_test_server;
use reqwest::{redirect::Policy, StatusCode};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(Policy::none())
        .build()
        .unwrap()
}

fn has_ffprobe() -> bool {
    std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn page_without_cover_has_no_hero() {
    let server = spawn_test_server().await.expect("spawn");
    server.seed_content_page("NoCover", "# No Cover\n\nbody").await.expect("seed");

    let body = reqwest::get(server.url("/pages/NoCover"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !body.contains("data-hero"),
        "a page with no cover must render no hero: {body}"
    );
}

#[tokio::test]
async fn page_with_cover_renders_a_hero() {
    // Exercises the real pipeline: upload an image → set it as the page cover via
    // the editor PUT → the reader view renders the hero. Needs ffprobe (like the
    // media vertical test); skips where absent.
    if !has_ffprobe() {
        eprintln!("skipping hero-with-cover test: ffprobe not found");
        return;
    }

    let server = spawn_test_server().await.expect("spawn");
    server.seed_content_page("HeroPage", "# Hero Page\n\nbody").await.expect("seed");

    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    // Upload a real image → media_ref.
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

    // Set it as the page's cover through the editor save path. The editor is htmx,
    // so send HX-Request — the DI.3 responder returns a native 303 (not a 200) to a
    // no-JS form, and this simulates the real editor faithfully.
    let resp = admin
        .put(server.url("/pages/HeroPage"))
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# Hero Page\n\nbody"),
            ("page_cover_media_ref", media_ref.as_str()),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "cover PUT should succeed, got {}", resp.status());

    // The reader view now renders the hero banner pointing at a media byte URL.
    let body = reqwest::get(server.url("/pages/HeroPage"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("data-hero"), "a page with a cover must render a hero: {body}");
    assert!(
        body.contains("data-hero") && body.contains("/media/file/"),
        "the hero must point at a media byte URL"
    );

    // ...but NOT in the editor (?edit): the hero is a reader-view element.
    let edit_body = admin
        .get(server.url("/pages/HeroPage?edit=1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !edit_body.contains("data-hero"),
        "the hero must not render in the editor view"
    );
}
