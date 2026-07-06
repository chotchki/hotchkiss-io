//! Integration tests for the greylist bot-toll challenge endpoints (Phase CX.6). Drives the
//! real server via `spawn_test_server`; `solve_challenge` runs the same chain + HMAC the browser
//! worker does, so this is a Rust-level end-to-end without a headless browser (the browser path
//! is covered by the chromiumoxide e2e in CX.8).

use hotchkiss_io::test_support::{solve_challenge, spawn_test_server};
use reqwest::redirect::Policy;

#[derive(serde::Deserialize)]
struct Tok {
    seed: String,
    inner_seed: String,
    ts: i64,
    version: String,
    width: u32,
    height: u32,
    image_url: String,
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn challenge_new_returns_a_signed_token() {
    let s = spawn_test_server().await.unwrap();
    let tok: Tok = reqwest::get(format!("{}/challenge/new", s.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!tok.seed.is_empty() && !tok.inner_seed.is_empty() && !tok.version.is_empty());
    assert!(tok.width > 0 && tok.height > 0);
    assert_eq!(tok.image_url, format!("/challenge/image/{}", tok.version));
}

#[tokio::test]
async fn challenge_image_serves_rgba_and_404s_wrong_version() {
    let s = spawn_test_server().await.unwrap();
    let tok: Tok = reqwest::get(format!("{}/challenge/new", s.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let img = reqwest::get(format!("{}{}", s.base_url, tok.image_url))
        .await
        .unwrap();
    assert_eq!(img.status(), 200);
    let bytes = img.bytes().await.unwrap();
    assert_eq!(
        bytes.len() as u32,
        tok.width * tok.height * 4,
        "raw RGBA buffer = width*height*4"
    );

    let bad = reqwest::get(format!("{}/challenge/image/deadbeef", s.base_url))
        .await
        .unwrap();
    assert_eq!(bad.status(), 404, "unknown version 404s");
}

#[tokio::test]
async fn solving_the_toll_clears_and_redirects_with_a_bearer_cookie() {
    let s = spawn_test_server().await.unwrap();
    let verify = solve_challenge(&s.base_url, "/pages/projects").await.unwrap();

    let resp = no_redirect_client()
        .get(format!("{}{}", s.base_url, verify))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "a correct solve redirects");
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/pages/projects",
        "back to the page the client was trying to reach"
    );
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("hio_toll="), "mints the clearance cookie");
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));

    // The recorded clearance shows up (the "passing is a signal" data).
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM greylist_clearance")
        .fetch_one(&s.pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "the solve was recorded");
}

#[tokio::test]
async fn bogus_answer_reserves_the_toll_without_a_cookie() {
    let s = spawn_test_server().await.unwrap();
    let tok: Tok = reqwest::get(format!("{}/challenge/new", s.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // A well-formed but WRONG answer: base64url of 32 zero bytes (the CVE-2025-24369 class).
    let bogus = "A".repeat(43);
    let resp = no_redirect_client()
        .get(format!(
            "{}/challenge/verify?inner_seed={}&ts={}&version={}&answer={}&redir=%2F",
            s.base_url, tok.inner_seed, tok.ts, tok.version, bogus
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302);
    assert!(
        resp.headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("/challenge"),
        "a failed solve re-serves the toll, never a dead end"
    );
    assert!(
        resp.headers().get("set-cookie").is_none(),
        "no clearance on a failed solve"
    );
}

#[tokio::test]
async fn foreign_redir_falls_back_to_root() {
    let s = spawn_test_server().await.unwrap();
    let verify = solve_challenge(&s.base_url, "https://evil.example.com/x")
        .await
        .unwrap();

    let resp = no_redirect_client()
        .get(format!("{}{}", s.base_url, verify))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/",
        "open-redirect blocked — a valid solve to a foreign redir still only goes home"
    );
}

#[tokio::test]
async fn interstitial_is_429_noindex_with_the_copy() {
    let s = spawn_test_server().await.unwrap();
    let resp = reqwest::get(format!("{}/challenge", s.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);
    assert_eq!(
        resp.headers().get("x-robots-tag").unwrap(),
        "noindex, nofollow"
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("Dimes not accepted"), "chris's copy");
    assert!(body.contains("Blazing Saddles"), "the film credit");
    assert!(body.contains("/scripts/challenge.js"), "loads the solver");
}
