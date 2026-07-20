//! EB.8 — host-aware identity icons: a non-canonical (beta) host serves the
//! color-inverted favicon/apple-touch-icon so the two pinned PWAs are tellable
//! apart on a home screen. The harness host (127.0.0.1) counts as canonical,
//! mirroring the robots.txt rule.

use hotchkiss_io::test_support::spawn_test_server;

#[tokio::test]
async fn beta_host_serves_inverted_icons() {
    let server = spawn_test_server().await.expect("spawn");

    let client = reqwest::Client::new();
    for (path, header_host) in [
        ("/favicon.ico", "beta.hotchkiss.io:8443"),
        ("/apple-touch-icon.png", "beta.hotchkiss.io"),
        ("/images/apple-touch-icon.png", "beta.hotchkiss.io"),
    ] {
        // Per-path canonical baseline (the harness host counts as canonical).
        let canonical = reqwest::get(server.url(path))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let resp = client
            .get(server.url(path))
            .header("host", header_host)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{path} must serve on beta host");
        let beta = resp.bytes().await.unwrap();
        assert_ne!(
            canonical, beta,
            "{path} on a non-canonical host must serve DIFFERENT (inverted) bytes"
        );
    }
}
