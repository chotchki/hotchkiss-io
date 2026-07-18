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
        .post(server.url("/media"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "admin upload should succeed");
    let j: serde_json::Value = resp.json().await.unwrap();
    let media_ref = j["ref"].as_str().expect("ref in manifest").to_string();

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

/// Upload a real image via `POST /media`, returning its `media_ref`.
async fn upload_image(
    server: &hotchkiss_io::test_support::TestServer,
    admin: &reqwest::Client,
    filename: &str,
) -> String {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/images/404")
        .join(filename);
    let bytes = std::fs::read(&fixture).expect("read avif fixture");
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str("image/avif")
            .unwrap(),
    );
    let resp = admin.post(server.url("/media")).multipart(form).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "image upload should succeed");
    resp.json::<serde_json::Value>().await.unwrap()["ref"].as_str().unwrap().to_string()
}

async fn set_cover(admin: &reqwest::Client, url: String, cover_ref: &str) {
    let resp = admin
        .put(url)
        .header("HX-Request", "true")
        .form(&[
            ("page_category", ""),
            ("page_markdown", "# Cover Stable\n\nbody"),
            ("page_cover_media_ref", cover_ref),
            ("page_order", "0"),
        ])
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "cover PUT should succeed: {}", resp.status());
}

/// The first `/media/file/<url_key>` byte-URL key in the page HTML (the hero src).
fn hero_byte_key(html: &str) -> String {
    html.split("/media/file/")
        .nth(1)
        .expect("a hero byte URL")
        .split(['"', ' ', '?'])
        .next()
        .unwrap()
        .to_string()
}

/// DS.2 ref-stability LOCK: a cover set from EITHER the `/media/<ref>` form OR the
/// `/media/file/<url_key>` byte form resolves to the SAME stored `media_id`, and the
/// hero re-renders with FRESH bytes after the item's variants are replaced. So
/// covers need NO save-rewrite (unlike content links, DS.1): `page_cover_media_id`
/// is stable + the byte URL is recomputed each render, never stale. Needs ffprobe.
#[tokio::test]
async fn cover_is_ref_stable_across_a_variant_replace() {
    if !has_ffprobe() {
        eprintln!("skipping cover ref-stability test: ffprobe not found");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin.post(server.url("/test/login?role=Admin")).send().await.unwrap();

    let media_ref = upload_image(&server, &admin, "blame_bonnie.avif").await;
    let media_id: i64 = sqlx::query_scalar("SELECT media_id FROM media WHERE media_ref = ?1")
        .bind(&media_ref)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    let url_key: String = sqlx::query_scalar(
        "SELECT v.url_key FROM media_variant v WHERE v.media_id = ?1 AND v.mime LIKE 'image/%' LIMIT 1",
    )
    .bind(media_id)
    .fetch_one(&server.pool)
    .await
    .unwrap();

    server
        .seed_content_page("CoverStable", "# Cover Stable\n\nbody")
        .await
        .expect("seed page");
    let page_url = server.url("/pages/CoverStable");

    // BOTH cover forms must resolve to the SAME media_id.
    for cover_ref in [format!("/media/{media_ref}"), format!("/media/file/{url_key}")] {
        set_cover(&admin, page_url.clone(), &cover_ref).await;
        let stored: Option<i64> =
            sqlx::query_scalar("SELECT page_cover_media_id FROM content_pages WHERE page_name = 'CoverStable'")
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!(stored, Some(media_id), "cover form {cover_ref:?} resolves to the item's media_id");
    }

    // The reader view renders a hero pointing at a live variant byte URL.
    let before = reqwest::get(page_url.clone()).await.unwrap().text().await.unwrap();
    assert!(before.contains("data-hero"), "a cover renders a hero");
    let key_before = hero_byte_key(&before);

    // Replace the item's ENTIRE variant set with a DIFFERENT image → new url_keys,
    // old ones wiped (a genuine round-trip SAVE).
    let hobbes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/images/404/blame_hobbes.avif"),
    )
    .unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(hobbes)
            .file_name("hobbes.avif")
            .mime_str("image/avif")
            .unwrap(),
    );
    let resp = admin
        .put(server.url(&format!("/media/{media_ref}/variants")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "variant replace should succeed: {}", resp.status());

    // The cover is UNCHANGED (media_id is stable — covers aren't rewritten), and the
    // hero now points at a FRESH, currently-live variant (never the wiped old key).
    let stored_after: Option<i64> =
        sqlx::query_scalar("SELECT page_cover_media_id FROM content_pages WHERE page_name = 'CoverStable'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(stored_after, Some(media_id), "the cover media_id survives a variant replace");

    let after = reqwest::get(page_url).await.unwrap().text().await.unwrap();
    assert!(after.contains("data-hero"), "the hero still renders after the replace");
    let key_after = hero_byte_key(&after);
    assert_ne!(key_before, key_after, "the hero byte URL is recomputed to the fresh variant");
    // And that fresh key is a LIVE variant of the item (not a dangling stale URL).
    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_variant WHERE media_id = ?1 AND url_key = ?2",
    )
    .bind(media_id)
    .bind(&key_after)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(live, 1, "the hero resolves to a current variant, never a stale byte URL");
}
