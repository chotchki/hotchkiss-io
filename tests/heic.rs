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

#[tokio::test]
async fn rotated_heic_bakes_orientation_into_the_avif() {
    // EB.10: an orientation-6 HEIC (300x200 raw pixels, rotate-90 display
    // matrix) must come out PORTRAIT in the derived AVIF — ffmpeg's autorotate
    // bakes the rotation during the fallback decode.
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
        "/tests/fixtures/rot-300x200-or6.heic"
    ))
    .expect("read rotated heic fixture");
    let part = reqwest::multipart::Part::bytes(heic)
        .file_name("rotated.heic")
        .mime_str("application/octet-stream")
        .unwrap();
    let resp = admin
        .post(server.url("/media"))
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let manifest: serde_json::Value = resp.json().await.unwrap();
    let media_ref = manifest["ref"].as_str().expect("ref").to_string();

    // 200 wide → no 480/960 rungs; exactly the one full rung, PORTRAIT.
    let avif: Vec<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT mv.width, mv.height
         FROM media_variant mv JOIN media m ON m.media_id = mv.media_id
         WHERE m.media_ref = ?1 AND mv.mime = 'image/avif'",
    )
    .bind(&media_ref)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    assert_eq!(
        avif,
        vec![(Some(200), Some(300))],
        "the derived AVIF must be upright portrait (rotation baked in)"
    );
}

#[tokio::test]
async fn rederive_drops_and_reminting_avif_rungs() {
    // ED.1: POST /admin/media/{ref}/rederive drops the derived rungs and
    // re-runs the ingest derivation from the stored source. Content-addressing
    // means identical bytes re-mint identical shas — the assertion is that the
    // full ladder EXISTS again after the spawned re-derive completes.
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
    .unwrap();
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let manifest: serde_json::Value = resp.json().await.unwrap();
    let media_ref = manifest["ref"].as_str().unwrap().to_string();

    let avif_count = |pool: sqlx::SqlitePool, r: String| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM media_variant mv JOIN media m ON m.media_id = mv.media_id
             WHERE m.media_ref = ?1 AND mv.mime = 'image/avif'",
        )
        .bind(r)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(avif_count(server.pool.clone(), media_ref.clone()).await, 3);

    // Anonymous is denied before anything happens.
    let anon = reqwest::Client::new();
    let resp = anon
        .post(server.url(&format!("/admin/media/{media_ref}/rederive")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Admin re-derive: 200, then the spawned derivation restores the ladder.
    let resp = admin
        .post(server.url(&format!("/admin/media/{media_ref}/rederive")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let n = avif_count(server.pool.clone(), media_ref.clone()).await;
        if n == 3 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "re-derive never restored the ladder (count {n})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
