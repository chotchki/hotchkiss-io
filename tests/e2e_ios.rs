//! iOS Simulator e2e — drives a real iPhone Sim's Safari (WebKit, not
//! Blink) via `ios-inspect` (path-dep on `../skylander-portal-controller/
//! tools/ios-inspect`). These catch iOS-specific rendering quirks the
//! chromiumoxide tests in `e2e_browser.rs` miss — Dynamic Type, WebKit
//! font metrics, viewport behavior, etc.
//!
//! Macos-only (the underlying `xcrun simctl` + `ios-webkit-debug-proxy` only
//! exist there) AND gated behind the `ios_e2e` Cargo feature — booting a sim
//! takes 10–60 s, so it's out of the default `cargo test`. It is NOT ignore-gated
//! (ignored tests rot silently; see the `lint_no_ignored_tests` guard): the mini
//! runs `cargo test --features ios_e2e` to exercise these.
//!
//! Prereqs:
//!  - Xcode + at least one iPhone iOS runtime (Xcode → Settings → Platforms).
//!  - `brew install ios-webkit-debug-proxy`.
//!  - `../skylander-portal-controller` checked out next to this repo.
//!
//! Run with (one sim at a time):
//!   `cargo test --features ios_e2e --test e2e_ios -- --test-threads=1`

#![cfg(all(target_os = "macos", feature = "ios_e2e"))]

use std::time::Duration;

use hotchkiss_io::test_support::{spawn_test_server, TestServer};
use ios_inspect::state::DeviceState;

/// Drop guard that schedules `shutdown_all` on test exit (success or panic),
/// matching the skylander harness pattern. `block_in_place` requires
/// `flavor = "multi_thread"` on the `#[tokio::test]`.
struct TeardownGuard {
    device_name: String,
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        let label = self.device_name.clone();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(ios_inspect::shutdown_all())
        });
        if let Err(e) = result {
            eprintln!("teardown for {label}: shutdown_all errored: {e}");
        }
    }
}

/// Boot one auto-picked iPhone sim and return the device handle + a teardown
/// guard. Caller must keep the guard alive until the end of the test.
async fn boot_iphone() -> (DeviceState, TeardownGuard) {
    let session = ios_inspect::boot_devices(&[])
        .await
        .expect("boot iPhone sim — Xcode + ios-webkit-debug-proxy installed?");
    let device = session
        .devices
        .first()
        .cloned()
        .expect("at least one device booted");
    let guard = TeardownGuard {
        device_name: device.device_name.clone(),
    };
    (device, guard)
}

async fn js_i64(device: &DeviceState, expr: &str) -> i64 {
    let v = ios_inspect::eval_js(device, expr)
        .await
        .unwrap_or_else(|e| panic!("eval_js `{expr}` failed: {e}"));
    v.as_i64()
        .unwrap_or_else(|| panic!("eval_js `{expr}` returned non-integer: {v:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ios_blog_no_horizontal_scroll() {
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (device, _teardown) = boot_iphone().await;

    let url = server
        .lan_url("/blog")
        .expect("resolve LAN URL for the iOS sim");
    ios_inspect::open_url(&device, &url)
        .await
        .expect("open /blog on sim");

    ios_inspect::wait_for_selector(&device, "h1", Duration::from_secs(20))
        .await
        .expect("/blog never rendered an <h1> on the sim");

    let scroll_width = js_i64(&device, "document.documentElement.scrollWidth").await;
    let inner_width = js_i64(&device, "window.innerWidth").await;
    eprintln!("  iOS /blog: scrollWidth={scroll_width} innerWidth={inner_width}");

    assert!(
        scroll_width <= inner_width,
        "/blog overflows on iOS Sim in portrait: scrollWidth={scroll_width}, innerWidth={inner_width}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ios_top_nav_no_overflow() {
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (device, _teardown) = boot_iphone().await;

    let url = server
        .lan_url("/blog")
        .expect("resolve LAN URL for the iOS sim");
    ios_inspect::open_url(&device, &url)
        .await
        .expect("open /blog on sim");

    ios_inspect::wait_for_selector(&device, "ul.flex.flex-row", Duration::from_secs(20))
        .await
        .expect("top nav <ul> never appeared on the sim");

    let nav_scroll = js_i64(
        &device,
        "document.querySelector('ul.flex.flex-row').scrollWidth",
    )
    .await;
    let nav_client = js_i64(
        &device,
        "document.querySelector('ul.flex.flex-row').clientWidth",
    )
    .await;
    eprintln!("  iOS nav: scrollWidth={nav_scroll} clientWidth={nav_client}");

    assert!(
        nav_scroll <= nav_client,
        "top nav overflows on iOS Sim: scrollWidth={nav_scroll}, clientWidth={nav_client}",
    );
}
