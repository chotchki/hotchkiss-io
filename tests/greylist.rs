//! Integration tests for greylist ENFORCEMENT (Phase CX.5). Seeds the server's in-memory
//! snapshot via `server.greylist.insert(...)` (the test client connects as `127.0.0.1`) and
//! asserts the toll gates the right traffic while letting exempt / cleared / authenticated
//! requests through.

use hotchkiss_io::test_support::{solve_challenge, spawn_test_server};
use reqwest::redirect::Policy;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .cookie_store(true)
        .redirect(Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn non_greylisted_ip_is_served_normally() {
    let s = spawn_test_server().await.unwrap();
    let r = reqwest::get(format!("{}/", s.base_url)).await.unwrap();
    assert_eq!(r.status(), 200, "a normal visitor is never tolled");
}

#[tokio::test]
async fn greylisted_ip_gets_the_toll() {
    let s = spawn_test_server().await.unwrap();
    s.greylist.insert("127.0.0.1");
    let r = reqwest::get(format!("{}/", s.base_url)).await.unwrap();
    assert_eq!(r.status(), 429);
    assert!(r.text().await.unwrap().contains("Dimes not accepted"));
}

#[tokio::test]
async fn exempt_paths_pass_but_content_is_tolled() {
    let s = spawn_test_server().await.unwrap();
    s.greylist.insert("127.0.0.1");
    // The toll's own endpoint must stay reachable (or it could never be solved).
    assert_eq!(
        reqwest::get(format!("{}/challenge/new", s.base_url))
            .await
            .unwrap()
            .status(),
        200
    );
    // A content path is NOT exempt.
    assert_eq!(
        reqwest::get(format!("{}/pages/anything", s.base_url))
            .await
            .unwrap()
            .status(),
        429
    );
}

#[tokio::test]
async fn a_cleared_client_is_waved_through() {
    let s = spawn_test_server().await.unwrap();
    s.greylist.insert("127.0.0.1");
    let c = client();

    // Tolled before clearance.
    assert_eq!(
        c.get(format!("{}/", s.base_url)).send().await.unwrap().status(),
        429
    );

    // Solve → the verify GET sets the clearance cookie in this client's jar.
    let verify = solve_challenge(&s.base_url, "/").await.unwrap();
    let v = c
        .get(format!("{}{}", s.base_url, verify))
        .send()
        .await
        .unwrap();
    assert_eq!(v.status(), 302);

    // Now the same client (carrying hio_toll) passes.
    assert_eq!(
        c.get(format!("{}/", s.base_url)).send().await.unwrap().status(),
        200,
        "a valid clearance cookie bypasses the toll"
    );
}

#[tokio::test]
async fn an_authenticated_user_is_not_tolled() {
    let s = spawn_test_server().await.unwrap();
    let c = client();
    // Log in BEFORE greylisting (the debug test-login seam), then greylist this IP.
    let login = c
        .post(format!("{}/test/login?role=Admin", s.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    s.greylist.insert("127.0.0.1");

    assert_eq!(
        c.get(format!("{}/", s.base_url)).send().await.unwrap().status(),
        200,
        "an authenticated user bypasses the toll regardless of greylist"
    );
}

#[tokio::test]
async fn a_challenged_request_is_logged_as_challenged_and_bot() {
    let s = spawn_test_server().await.unwrap();
    s.greylist.insert("127.0.0.1");

    let r = reqwest::get(format!("{}/pages/x", s.base_url)).await.unwrap();
    assert_eq!(r.status(), 429);

    // The log insert is fire-and-forget — poll for the stamped row.
    let mut count = 0i64;
    for _ in 0..100 {
        count = sqlx::query_scalar(
            "SELECT COUNT(*) FROM request_log WHERE challenged = 1 AND is_bot = 1",
        )
        .fetch_one(&s.pool)
        .await
        .unwrap();
        if count > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(count >= 1, "the toll is logged as challenged + bot");
}

#[tokio::test]
async fn admin_pin_and_release_manage_the_greylist() {
    let s = spawn_test_server().await.unwrap();
    let c = client();
    c.post(format!("{}/test/login?role=Admin", s.base_url))
        .send()
        .await
        .unwrap();

    // Pin an IP — updates the DB AND the in-memory snapshot immediately.
    let pin = c
        .post(format!("{}/admin/greylist/pin", s.base_url))
        .form(&[("ip", "203.0.113.7")])
        .send()
        .await
        .unwrap();
    assert!(pin.status().is_success());
    assert!(
        s.greylist.is_greylisted("203.0.113.7"),
        "a manual pin tolls immediately, without waiting for a sweep"
    );

    // It's listed on the panel, badged as a pin.
    let body = c
        .get(format!("{}/admin/greylist", s.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("203.0.113.7"));
    assert!(body.contains("pinned"));

    // Release it — clears the DB row AND the snapshot.
    c.post(format!("{}/admin/greylist/203.0.113.7/release", s.base_url))
        .send()
        .await
        .unwrap();
    assert!(
        !s.greylist.is_greylisted("203.0.113.7"),
        "a release un-tolls immediately"
    );
}

#[tokio::test]
async fn admin_greylist_rejects_bad_ip_and_gates_anonymous() {
    let s = spawn_test_server().await.unwrap();

    // Anonymous can't see the panel (the /admin require_admin layer).
    assert_eq!(
        reqwest::get(format!("{}/admin/greylist", s.base_url))
            .await
            .unwrap()
            .status(),
        403
    );

    let c = client();
    c.post(format!("{}/test/login?role=Admin", s.base_url))
        .send()
        .await
        .unwrap();
    let bad = c
        .post(format!("{}/admin/greylist/pin", s.base_url))
        .form(&[("ip", "not-an-ip")])
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400, "a garbage IP is rejected");
}

#[tokio::test]
async fn run_sweep_now_greylists_a_scanner_on_demand() {
    let s = spawn_test_server().await.unwrap();
    // A scanner's signature probes (a public IP, so detection evaluates it).
    for p in ["/wp-login.php", "/xmlrpc.php", "/.env"] {
        sqlx::query("INSERT INTO request_log (method, path, status, ip) VALUES ('GET', ?, 404, '203.0.113.99')")
            .bind(p)
            .execute(&s.pool)
            .await
            .unwrap();
    }
    assert!(!s.greylist.is_greylisted("203.0.113.99"));

    let c = client();
    c.post(format!("{}/test/login?role=Admin", s.base_url))
        .send()
        .await
        .unwrap();
    let r = c
        .post(format!("{}/admin/greylist/run-sweep", s.base_url))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    assert!(
        s.greylist.is_greylisted("203.0.113.99"),
        "the on-demand sweep greylisted the scanner via R1 (no wait for the timer)"
    );
}

#[tokio::test]
async fn candidate_signatures_surface_novel_dead_paths() {
    let s = spawn_test_server().await.unwrap();
    let c = client();
    c.post(format!("{}/test/login?role=Admin", s.base_url))
        .send()
        .await
        .unwrap();

    // Pin an IP, then have it probe a NOVEL dead path R1 doesn't match (never succeeds for anyone).
    c.post(format!("{}/admin/greylist/pin", s.base_url))
        .form(&[("ip", "203.0.113.7")])
        .send()
        .await
        .unwrap();
    for _ in 0..3 {
        sqlx::query("INSERT INTO request_log (method, path, status, ip) VALUES ('GET', '/some-new-cms-probe', 404, '203.0.113.7')")
            .execute(&s.pool)
            .await
            .unwrap();
    }

    let body = c
        .get(format!("{}/admin/greylist", s.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("/some-new-cms-probe"),
        "a novel dead path probed by a greylisted IP surfaces as a candidate signature"
    );
}

#[tokio::test]
async fn challenge_ceremony_is_excluded_from_the_request_log() {
    let s = spawn_test_server().await.unwrap();
    // The toll's own endpoint (excluded) + a sentinel content path (logged).
    reqwest::get(format!("{}/challenge/new", s.base_url)).await.unwrap();
    reqwest::get(format!("{}/sentinel-not-a-page", s.base_url)).await.unwrap();

    // Poll for the sentinel — proves the fire-and-forget writer processed requests AFTER the
    // /challenge one, so if the ceremony WERE logged it would be present by now.
    let mut sentinel = 0i64;
    for _ in 0..100 {
        sentinel =
            sqlx::query_scalar("SELECT COUNT(*) FROM request_log WHERE path = '/sentinel-not-a-page'")
                .fetch_one(&s.pool)
                .await
                .unwrap();
        if sentinel > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(sentinel >= 1, "the sentinel request was logged");

    let ceremony: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM request_log WHERE path LIKE '/challenge%'")
            .fetch_one(&s.pool)
            .await
            .unwrap();
    assert_eq!(ceremony, 0, "the /challenge ceremony is excluded from the access log");
}
