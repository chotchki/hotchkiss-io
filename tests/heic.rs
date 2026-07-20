//! EB.9 — HEIC ingest normalization. An iPhone HEIC upload keeps its ORIGINAL
//! bytes stored (source-of-truth) but ingest derives browser-renderable AVIF
//! rungs — the 480/960 ladder PLUS a capped "full" rung — via the ffmpeg decode
//! fallback (the `image` crate can't read HEIC). The embed then serves ONLY the
//! AVIFs; the heic url_key never appears in the rendered page.

use hotchkiss_io::test_support::spawn_test_server;
use reqwest::{redirect::Policy, StatusCode};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(Policy::none())
        .build()
        .unwrap()
}

fn tool_available(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn heic_upload_derives_avif_rungs_and_never_serves_heic() {
    // Ingest probes with ffprobe; the HEIC decode fallback shells ffmpeg.
    if !tool_available("ffprobe") || !tool_available("ffmpeg") {
        eprintln!("skipping: ffprobe/ffmpeg not installed");
        return;
    }
    let server = spawn_test_server().await.expect("spawn");
    let admin = client();
    admin
        .post(server.url("/test/login?role=Admin"))
        .send()
        .await
        .unwrap();

    let heic = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gradient-1200x900.heic"
    ))
    .expect("read heic fixture");
    let part = reqwest::multipart::Part::bytes(heic)
        .file_name("photo.heic")
        .mime_str("application/octet-stream")
        .unwrap();
    let resp = admin
        .post(server.url("/media"))
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "heic upload must ingest");
    let manifest: serde_json::Value = resp.json().await.unwrap();
    let media_ref = manifest["ref"].as_str().expect("ref").to_string();
    let kind: String = sqlx::query_scalar("SELECT kind FROM media WHERE media_ref = ?1")
        .bind(&media_ref)
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(kind, "image", "a HEIC still is an IMAGE");

    // Variants: the heic original + the derived AVIF rungs (480, 960, and the
    // full 1200 — under the 1920 cap, so full-size).
    let rows: Vec<(String, Option<i64>, String)> = sqlx::query_as(
        "SELECT mv.mime, mv.width, mv.url_key
         FROM media_variant mv JOIN media m ON m.media_id = mv.media_id
         WHERE m.media_ref = ?1",
    )
    .bind(&media_ref)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    let avif_widths: Vec<i64> = {
        let mut w: Vec<i64> = rows
            .iter()
            .filter(|(mime, _, _)| mime == "image/avif")
            .filter_map(|(_, width, _)| *width)
            .collect();
        w.sort();
        w
    };
    assert_eq!(avif_widths, vec![480, 960, 1200], "AVIF ladder incl. the full rung");
    let heic_key = rows
        .iter()
        .find(|(mime, _, _)| mime == "image/heic")
        .map(|(_, _, key)| key.clone())
        .expect("the heic ORIGINAL stays stored");

    // The rendered embed (the page's `media-embed` placeholder htmx-swaps in
    // GET /media/embed/<ref>) serves only AVIFs — the heic key appears NOWHERE.
    let html = reqwest::get(server.url(&format!("/media/embed/{media_ref}")))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let full_key = rows
        .iter()
        .find(|(mime, width, _)| mime == "image/avif" && *width == Some(1200))
        .map(|(_, _, key)| key.clone())
        .unwrap();
    assert!(
        html.contains(&format!("src=\"/media/file/{full_key}\"")),
        "embed src must be the full AVIF rung; got: {html}"
    );
    assert!(
        !html.contains(&heic_key),
        "the heic url_key must never reach the rendered embed: {html}"
    );
}
