//! Browser e2e (Phase 8.4) — pure Rust via `chromiumoxide` (CDP). Drives a real
//! headless Chrome against the in-process harness (`spawn_test_server`), using a
//! CDP **virtual authenticator** so the WebAuthn/passkey ceremony completes with
//! no hardware or human.
//!
//! These run as part of plain `cargo test` (no longer ignore-gated — ignored
//! tests rot silently; see the `lint_no_ignored_tests` guard). They need Google
//! Chrome installed. Every test SERIALIZES on `E2E_LOCK`: each launches its own
//! Chrome + WebAuthn virtual authenticator, and running the passkey ceremonies
//! concurrently races (resource contention blows the 20s registration deadline) —
//! test isolation (each test wants its own server + DB, so a shared fixture
//! doesn't fit yet).

use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, MutexGuard};

use chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams;
use chromiumoxide::cdp::browser_protocol::emulation::{
    MediaFeature, SetDeviceMetricsOverrideParams, SetEmulatedMediaParams,
};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::web_authn::{
    AddVirtualAuthenticatorParams, AuthenticatorProtocol, AuthenticatorTransport, EnableParams,
    VirtualAuthenticatorOptions,
};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use hotchkiss_io::test_support::{spawn_test_server, TestServer};

/// Serializes the browser e2e — see the module docs. Acquire at the top of every
/// test (`let _e2e = e2e_lock().await;`) so only one Chrome + WebAuthn ceremony
/// runs at a time.
static E2E_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

async fn e2e_lock() -> MutexGuard<'static, ()> {
    E2E_LOCK.lock().await
}

/// Launch headless Chrome with a throwaway profile dir (so concurrent test
/// instances don't fight over a shared `SingletonLock`). Returns the browser,
/// the join handle of the task that drains its CDP event stream (must be kept
/// alive for the session to work), and the profile dir to clean up.
async fn launch() -> (Browser, tokio::task::JoinHandle<()>, std::path::PathBuf) {
    let profile = std::env::temp_dir().join(format!("hotchkiss-e2e-chrome-{}", uuid::Uuid::new_v4()));
    let (browser, mut handler) = Browser::launch(
        BrowserConfig::builder()
            .user_data_dir(&profile)
            .build()
            .expect("build BrowserConfig"),
    )
    .await
    .expect("launch chrome — is Google Chrome installed?");
    let handle = tokio::spawn(async move { while handler.next().await.is_some() {} });
    (browser, handle, profile)
}

/// Override the page's viewport to an iPhone-14-Pro-ish frame (390×844).
/// Call after `new_page` and before any `goto` so layout is computed at the
/// mobile size. Tests that don't call this get Chrome's default viewport.
///
/// Note: `mobile=false` is deliberate — Chrome's `mobile=true` mode applies
/// text autosizing that masks real layout overflow. We want a strict 390px
/// CSS viewport with no accommodations so overflow surfaces the way it does
/// on iOS Safari.
async fn use_mobile_viewport(page: &Page) {
    let params = SetDeviceMetricsOverrideParams::builder()
        .width(390)
        .height(844)
        .device_scale_factor(3.0)
        .mobile(false)
        .build()
        .expect("SetDeviceMetricsOverrideParams");
    page.execute(params)
        .await
        .expect("Emulation.setDeviceMetricsOverride");
}

/// Attach a software platform authenticator that auto-completes ceremonies.
async fn add_virtual_authenticator(page: &Page) {
    page.execute(EnableParams::default())
        .await
        .expect("WebAuthn.enable");
    let opts = VirtualAuthenticatorOptions::builder()
        .protocol(AuthenticatorProtocol::Ctap2)
        .transport(AuthenticatorTransport::Internal)
        .has_resident_key(true)
        .has_user_verification(true)
        .is_user_verified(true)
        .automatic_presence_simulation(true)
        .build()
        .expect("VirtualAuthenticatorOptions");
    page.execute(AddVirtualAuthenticatorParams::new(opts))
        .await
        .expect("WebAuthn.addVirtualAuthenticator");
}

/// Poll the page URL until it no longer contains `/login` (or time out).
async fn wait_until_left_login(page: &Page) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            Instant::now() < deadline,
            "registration never navigated away from /login"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(Some(url)) = page.url().await
            && !url.contains("/login")
        {
            return;
        }
    }
}

#[tokio::test]
async fn passkey_registration_then_admin_dashboard() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    add_virtual_authenticator(&page).await;

    // The login page's WebAuthn HTMX extension drives the real ceremony:
    //   GET /login/start_register/<name> → navigator.credentials.create(...)
    //   POST /login/finish_register      → the first registered user is Admin
    // then `window.location.href = "/"`.
    page.goto(server.url("/login")).await.expect("goto /login");
    let username = page.find_element("#username").await.expect("#username");
    username.click().await.expect("focus #username");
    username.type_str("e2e-admin").await.expect("type username");
    page.find_element("button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");

    wait_until_left_login(&page).await;

    // The resulting session is Admin → the layer-gated dashboard renders.
    page.goto(server.url("/admin/analytics"))
        .await
        .expect("goto /admin/analytics");
    let html = page.content().await.expect("page content");
    assert!(
        html.contains("Analytics"),
        "admin dashboard should render; first 300 chars: {}",
        &html[..html.len().min(300)]
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

/// DM.1: a cancelled/failed passkey ceremony must surface a VISIBLE error — the
/// exact incident (a dismissed sheet was a silent unhandled rejection, no DOM
/// write, no console on a phone). We simulate the cancel by patching
/// `navigator.credentials.create` to reject with `NotAllowedError` (the register
/// extension reads it at call time, so a post-load patch takes effect); the
/// ceremony's `.catch` must then fill `#error_message` and must NOT navigate away
/// as if it succeeded. No virtual authenticator needed — create() never runs for
/// real.
#[tokio::test]
async fn cancelled_registration_surfaces_a_visible_error() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    page.goto(server.url("/login")).await.expect("goto /login");

    // Simulate the user dismissing the passkey sheet (or a timeout).
    let _: bool = js(
        &page,
        "(() => { navigator.credentials.create = () => \
         Promise.reject(Object.assign(new Error('x'), { name: 'NotAllowedError' })); \
         return true; })()",
    )
    .await;

    let username = page.find_element("#username").await.expect("#username");
    username.click().await.expect("focus #username");
    username.type_str("e2e-cancel").await.expect("type username");
    page.find_element("button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");

    // The .catch must write the error slot (pre-DM this stayed empty forever).
    let deadline = Instant::now() + Duration::from_secs(10);
    let msg = loop {
        assert!(
            Instant::now() < deadline,
            "no visible error after a cancelled ceremony — DM.1 .catch regressed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let m: String =
            js(&page, "document.getElementById('error_message').textContent").await;
        if !m.trim().is_empty() {
            break m;
        }
    };
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("cancel") || lower.contains("try again"),
        "error should explain the cancel; got {msg:?}"
    );

    // And we did NOT falsely navigate to a logged-in page.
    let url = page.url().await.ok().flatten().unwrap_or_default();
    assert!(url.contains("/login"), "stayed on /login after failure; url={url}");

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

/// DM follow-up: a CLIENT-side registration ceremony failure must now BEACON the
/// server (`POST /login/ceremony_error`) so a phone-only failure is no longer
/// invisible to the operator. We override `navigator.sendBeacon` to capture the
/// call, patch `credentials.create` to reject (the concurrent-`create` "a request
/// is already pending" shape — the Android theory), submit register, and assert
/// the beacon fired with the action + error name in its body (AND the visible
/// error still renders). No virtual authenticator — create() never runs for real.
#[tokio::test]
async fn cancelled_registration_beacons_the_failure() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    page.goto(server.url("/login")).await.expect("goto /login");

    // Capture sendBeacon calls; reject create() with a "pending"-shaped error.
    let _: bool = js(
        &page,
        "(() => { \
           window.__beacons = []; \
           navigator.sendBeacon = function (url, data) { \
             const rec = { url: url, body: null }; window.__beacons.push(rec); \
             try { if (data && data.text) { data.text().then(function (t) { rec.body = t; }); } } catch (e) {} \
             return true; \
           }; \
           navigator.credentials.create = () => Promise.reject(\
             Object.assign(new Error('a request is already pending'), { name: 'NotAllowedError' })); \
           return true; })()",
    )
    .await;

    let username = page.find_element("#username").await.expect("#username");
    username.click().await.expect("focus #username");
    username.type_str("e2e-beacon").await.expect("type username");
    page.find_element("button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");

    // Poll for the ceremony_error beacon body (the .catch fires it after the reject).
    let deadline = Instant::now() + Duration::from_secs(10);
    let body = loop {
        assert!(
            Instant::now() < deadline,
            "no /login/ceremony_error beacon after a failed ceremony — the DM-followup beacon regressed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let b: String = js(
            &page,
            "(() => { const b = (window.__beacons || []).find(x => x.url && x.url.indexOf('/login/ceremony_error') >= 0); return b && b.body ? b.body : ''; })()",
        )
        .await;
        if !b.trim().is_empty() {
            break b;
        }
    };
    assert!(body.contains("\"action\":\"register\""), "beacon carries the action: {body}");
    assert!(body.contains("NotAllowedError"), "beacon carries the error name: {body}");

    // The visible error still renders (the beacon is additive, not a replacement).
    let msg: String = js(&page, "document.getElementById('error_message').textContent").await;
    assert!(!msg.trim().is_empty(), "the user still sees an error message");

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

/// Evaluate a JS expression on the page and return it as a JSON value.
async fn js<T: serde::de::DeserializeOwned>(page: &Page, expr: &str) -> T {
    page.evaluate(expr)
        .await
        .unwrap_or_else(|e| panic!("evaluate `{expr}`: {e}"))
        .into_value()
        .unwrap_or_else(|e| panic!("into_value `{expr}`: {e}"))
}

// ── Admin media library e2e helpers (DR.3) ──────────────────────────────────

/// ffprobe gate — the media ingest shells out to it even for a generic file (it
/// types the upload, falling back to `MediaKind::File`). Absent → skip, like the
/// HTTP-level media tests. NOT `#[ignore]` (the `no_ignored_tests` guard holds).
fn ffprobe_available() -> bool {
    std::process::Command::new("ffprobe")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write a throwaway fixture file — `DOM.setFileInputFiles` needs a real path on
/// disk. Distinct bytes per call, so two uploads don't content-address-dedup.
fn write_temp_fixture(prefix: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("{prefix}-{}.bin", uuid::Uuid::new_v4()));
    std::fs::write(&p, bytes).expect("write temp fixture");
    p
}

/// Register the first user via the REAL passkey ceremony → Admin (the first
/// registrant is auto-promoted). Leaves the browser on `/` with an Admin session.
async fn register_first_admin(page: &Page, server: &TestServer, username: &str) {
    page.goto(server.url("/login")).await.expect("goto /login");
    let u = page.find_element("#username").await.expect("#username");
    u.click().await.expect("focus #username");
    u.type_str(username).await.expect("type username");
    page.find_element("button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");
    wait_until_left_login(page).await;
}

/// Set a file input's files over CDP (`DOM.setFileInputFiles`, keyed by the
/// element's backend node id) — the browser-faithful way to drive an upload.
/// Chrome fires the input's own `change` event, which is what the media JS listens
/// on; we deliberately do NOT dispatch a second one — the drop-zone handler POSTs a
/// fresh item per change, so a double-fire would mint two items.
async fn set_file_input(page: &Page, selector: &str, paths: &[&std::path::Path]) {
    let el = page
        .find_element(selector)
        .await
        .unwrap_or_else(|e| panic!("find {selector}: {e}"));
    let files = paths.iter().map(|p| p.to_string_lossy().into_owned());
    page.execute(
        SetFileInputFilesParams::builder()
            .files(files)
            .backend_node_id(el.backend_node_id)
            .build()
            .expect("build SetFileInputFilesParams"),
    )
    .await
    .expect("DOM.setFileInputFiles");
}

/// Non-panicking evaluate — tolerant of the transient "context destroyed" an
/// evaluate hits mid-navigation (each media mutation reloads the page). Poll on
/// these so a reload in flight is a retry, not a spurious failure.
async fn try_bool(page: &Page, expr: &str) -> bool {
    page.evaluate(expr).await.ok().and_then(|r| r.into_value().ok()).unwrap_or(false)
}
async fn try_string(page: &Page, expr: &str) -> String {
    page.evaluate(expr).await.ok().and_then(|r| r.into_value().ok()).unwrap_or_default()
}

/// Poll a boolean JS expression until true (or panic at the 20s deadline).
async fn wait_true(page: &Page, expr: &str, ctx: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for: {ctx}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        if try_bool(page, expr).await {
            return;
        }
    }
}

/// Poll a string JS expression until non-empty (or panic at the 20s deadline).
async fn wait_nonempty(page: &Page, expr: &str, ctx: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for: {ctx}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let v = try_string(page, expr).await;
        if !v.trim().is_empty() {
            return v;
        }
    }
}

/// Click the first element matching `selector` (re-found fresh — element handles
/// go stale across the reloads each mutation triggers).
async fn click_selector(page: &Page, selector: &str) {
    page.find_element(selector)
        .await
        .unwrap_or_else(|e| panic!("find {selector} to click: {e}"))
        .click()
        .await
        .unwrap_or_else(|e| panic!("click {selector}: {e}"));
}

#[tokio::test]
async fn blog_no_horizontal_scroll_on_mobile() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    use_mobile_viewport(&page).await;

    page.goto(server.url("/blog")).await.expect("goto /blog");
    // Let layout settle.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let scroll_width: i64 = js(&page, "document.documentElement.scrollWidth").await;
    let inner_width: i64 = js(&page, "window.innerWidth").await;

    // Smoke check — fires if some future change introduces literal viewport
    // overflow (a wide fixed-width element, `whitespace-nowrap` on the wrong
    // thing, etc). Does *not* reproduce the iOS rendering quirks (Dynamic
    // Type, font metrics) the user hit during Phase 10 dogfooding; real-iOS
    // testing per 11.9 is still the source of truth for "looks right on a
    // phone." See PLAN.md dogfood findings for the gap.
    assert!(
        scroll_width <= inner_width,
        "/blog has horizontal scroll on a 390px viewport: scrollWidth={scroll_width}, innerWidth={inner_width}",
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
async fn top_nav_no_overflow_on_mobile() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    use_mobile_viewport(&page).await;

    page.goto(server.url("/blog")).await.expect("goto /blog");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Below `lg` the nav is a native `<details>` hamburger (no JS) — the old
    // single-row `<ul>` is `hidden` on mobile. It must exist, and crucially must
    // still fit the viewport when fully EXPANDED (the old nav-cut-off regression).
    let has_hamburger: bool = js(&page, "!!document.querySelector('details > summary')").await;
    assert!(has_hamburger, "mobile hamburger <details><summary> not found");

    page.find_element("details > summary")
        .await
        .expect("hamburger summary")
        .click()
        .await
        .expect("open hamburger");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let scroll_width: i64 = js(&page, "document.documentElement.scrollWidth").await;
    let inner_width: i64 = js(&page, "window.innerWidth").await;
    assert!(
        scroll_width <= inner_width,
        "expanded mobile nav overflows a 390px viewport: scrollWidth={scroll_width}, innerWidth={inner_width}",
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
async fn admin_create_page_server_slugifies_title() {
    // Page creation moved off /blog to the admin hub (`/admin/pages`) in Phase F,
    // and slugification moved from a client-side oninput handler to the SERVER
    // (`post_top_level_page_path` → `slugify`). This is the current flow: a title
    // with spaces creates a page and redirects to its slugified URL — no silent
    // 400 (the regression that originally motivated this test).
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    use_mobile_viewport(&page).await;

    add_virtual_authenticator(&page).await;

    // Register the first user — automatically promoted to Admin (UserDao::create).
    page.goto(server.url("/login")).await.expect("goto /login");
    let username = page.find_element("#username").await.expect("#username");
    username.click().await.expect("focus #username");
    username.type_str("e2e-admin").await.expect("type username");
    page.find_element("button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");
    wait_until_left_login(&page).await;

    page.goto(server.url("/admin/pages"))
        .await
        .expect("goto /admin/pages");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let title_input = page
        .find_element("form[hx-post='/pages'] input[name='page_title']")
        .await
        .expect("new-page title input");
    title_input.click().await.expect("focus title");
    title_input
        .type_str("Hello World E2E")
        .await
        .expect("type title");
    page.find_element("form[hx-post='/pages'] button[type=submit]")
        .await
        .expect("create button")
        .click()
        .await
        .expect("click create");

    // The handler htmx-redirects to `/pages/<slug>?edit=1`; poll the URL until the
    // server-slugified path appears (spaces → hyphens, lowercased).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < deadline,
            "create never navigated to the slugified page (silent 400?)"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(Some(url)) = page.url().await
            && url.contains("/pages/hello-world-e2e")
        {
            break;
        }
    }

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
async fn anonymous_unauthorized_from_admin_dashboard() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    page.goto(server.url("/admin/analytics"))
        .await
        .expect("goto /admin/analytics");
    let html = page.content().await.expect("page content");
    // A full-page (non-HTMX) anonymous nav to a gated route renders the styled 401
    // page (DK.2) — "Who goes there?" (missing identity), not a bare status.
    assert!(
        html.contains("Who goes there"),
        "anonymous request should hit the styled 401 page; first 300 chars: {}",
        &html[..html.len().min(300)]
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
async fn analytics_usable_on_mobile() {
    // Regression guard for the prod report: the analytics dashboard "didn't even
    // look like a table, none of the widgets show" on a phone — a wide unwrapped
    // table forced the document past 390px and Safari mangled the whole layout.
    // The fix wraps every table in overflow-x-auto so nothing exceeds the
    // viewport; this asserts no page-wide horizontal scroll AND that the widgets
    // (chart, stat numbers, top-pages row) actually render.
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");

    // Seed traffic incl. a deliberately LONG user-agent — the overflow-prone cell
    // that used to blow out the page width on mobile.
    let long_ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1 SomeVeryLongTrackingTokenThatGoesOnAndOnAndOn";
    for _ in 0..3 {
        sqlx::query("INSERT INTO request_log (method, path, status, ip, user_agent) VALUES ('GET', '/pages/mobile-test', 200, '1.1.1.1', ?)")
            .bind(long_ua)
            .execute(&server.pool)
            .await
            .unwrap();
    }

    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    use_mobile_viewport(&page).await;
    add_virtual_authenticator(&page).await;

    // Register the first user (→ Admin) via the real passkey ceremony.
    page.goto(server.url("/login")).await.expect("goto /login");
    let username = page.find_element("#username").await.expect("#username");
    username.click().await.expect("focus #username");
    username.type_str("e2e-admin").await.expect("type username");
    page.find_element("button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");
    wait_until_left_login(&page).await;

    page.goto(server.url("/admin/analytics"))
        .await
        .expect("goto /admin/analytics");
    // The d3 line chart renders client-side (vendored d3 + analytics-chart.js) — poll
    // for its two overlaid series paths (total + unique). CQ.7: assert the REAL rendered
    // chart, not just any inline <svg> (the nav icons are <svg> too). d3 is ~280KB, so
    // give it a beat to load + draw.
    let mut chart_lines = 0i64;
    for _ in 0..40 {
        chart_lines = js(&page, "document.querySelectorAll('path.linechart-line').length").await;
        if chart_lines >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        chart_lines >= 2,
        "the d3 line chart should render both series (total + unique); got {chart_lines}",
    );

    // The core fix: no element forces the document wider than the phone viewport —
    // checked AFTER the chart renders so the SVG is included in the layout.
    let scroll_width: i64 = js(&page, "document.documentElement.scrollWidth").await;
    let inner_width: i64 = js(&page, "window.innerWidth").await;
    assert!(
        scroll_width <= inner_width,
        "/admin/analytics has horizontal scroll on a 390px viewport: scrollWidth={scroll_width}, innerWidth={inner_width}",
    );

    // The other widgets are present (not collapsed/hidden).
    let html = page.content().await.expect("page content");
    assert!(html.contains("total views"), "stat widgets should render");
    assert!(
        html.contains("/pages/mobile-test"),
        "the top-pages table should render the seeded path"
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
async fn landing_doors_render_no_horizontal_scroll_on_mobile() {
    // Phase 13: `/` is the featured landing. On a 390px viewport the three pillar
    // doors must render and nothing may force the document wider than the phone
    // (13.3/13.4 ride the base.html hamburger + `w-full min-w-0` content cap).
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    // Seed content so the Latest strip (cards + a long title) is in the layout too.
    server
        .seed_blog_post(
            "a-rather-long-post-title",
            "# A Rather Long Post Title That Could Push The Layout Wide\n\nbody",
        )
        .await
        .unwrap();
    server.seed_project("printed-bracket", "# Printed Bracket\n\nbody").await.unwrap();

    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    use_mobile_viewport(&page).await;

    page.goto(server.url("/")).await.expect("goto /");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The three live pillar doors are present.
    for href in ["/projects", "/blog", "/resume"] {
        let present: bool =
            js(&page, &format!("!!document.querySelector('a[href=\"{href}\"]')")).await;
        assert!(present, "pillar door for {href} should render on the landing");
    }

    let scroll_width: i64 = js(&page, "document.documentElement.scrollWidth").await;
    let inner_width: i64 = js(&page, "window.innerWidth").await;
    assert!(
        scroll_width <= inner_width,
        "/ (landing) has horizontal scroll on a 390px viewport: scrollWidth={scroll_width}, innerWidth={inner_width}",
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

/// The full toll path in a REAL browser: a greylisted client hits the site, the interstitial's
/// worker solves the proof-of-work over the toll image, `/challenge/verify` mints the clearance
/// cookie, and the 302 lands back on real site content (Phase CX.8).
#[tokio::test]
async fn greylisted_browser_pays_the_toll_and_reaches_content() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    // The browser connects over loopback — greylist both forms so whichever it uses is tolled.
    server.greylist.insert("127.0.0.1");
    server.greylist.insert("::1");

    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    // Greylisted → the home page serves the toll interstitial first.
    // Greylisted → the home page serves the toll. The worker solves the PoW and ENABLES the
    // Continue button (no auto-redirect — the visitor clicks through so they can see the toll).
    page.goto(server.url("/")).await.expect("goto /");
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        assert!(
            Instant::now() < deadline,
            "the toll's Continue button never enabled — the browser didn't solve the PoW"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        // The button exists ONLY on the toll page, and enables only once the solve finishes.
        let ready: bool = js(
            &page,
            "!!document.getElementById('toll-continue') && !document.getElementById('toll-continue').disabled",
        )
        .await;
        if ready {
            break;
        }
    }

    // Click Continue → /challenge/verify → 302 → the home page (which renders the base.html nav).
    page.find_element("#toll-continue")
        .await
        .expect("continue button")
        .click()
        .await
        .expect("click continue");

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            Instant::now() < deadline,
            "clicking Continue never landed on site content"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let html = page.content().await.unwrap_or_default();
        if html.contains("aria-label=\"Primary\"") && !html.contains("Dimes not accepted") {
            break; // cleared: real site chrome is showing, toll is gone
        }
    }

    // Prove it went THROUGH the toll (not served the site directly): a clearance was recorded on
    // solve, and the first hit was stamped as challenged.
    let cleared: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM greylist_clearance")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert!(cleared >= 1, "the browser actually solved the toll (clearance recorded)");
    let challenged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_log WHERE challenged = 1")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert!(challenged >= 1, "the first request was served the toll");

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

/// The family-library journey (Phase DE), full-stack: anonymous sees no tab +
/// the sign-in gate + a miss-shaped book page; registering through
/// `/login?next=/library` runs the REAL passkey ceremony and the JS lands on
/// /library via the finish response's URL (the session-stashed ?next); a
/// pool-side promote to Family (the e2e Family mint — registration alone only
/// produces Admin/Registered, and refresh_session_role picks the new role up
/// on the next request) opens the doors, the listing and the book itself.
#[tokio::test]
async fn family_library_gate_login_next_and_entry() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    server
        .seed_library_book("e2e-book", "# E2E Book\n\na family read")
        .await
        .expect("seed book");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    add_virtual_authenticator(&page).await;

    // Anonymous: no Library tab, the sign-in gate, and a miss-shaped book.
    page.goto(server.url("/")).await.expect("goto /");
    let home = page.content().await.expect("home content");
    assert!(!home.contains("/pages/library"), "anon nav must not show the tab");
    page.goto(server.url("/library")).await.expect("goto /library");
    let gate = page.content().await.expect("gate content");
    assert!(gate.contains("Sign in"), "anon /library shows the sign-in gate");
    page.goto(server.url("/pages/library/audiobooks/e2e-book"))
        .await
        .expect("goto book");
    let miss = page.content().await.expect("book content");
    assert!(!miss.contains("E2E Book"), "anon book page must stay miss-shaped");

    // Register through /login?next=/library — the stashed next must carry the
    // browser to /library after the ceremony (first user lands Admin, which
    // passes the gate).
    page.goto(server.url("/login?next=%2Flibrary"))
        .await
        .expect("goto /login?next");
    let username = page.find_element("#username").await.expect("#username");
    username.click().await.expect("focus #username");
    username.type_str("e2e-family").await.expect("type username");
    page.find_element("button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");
    wait_until_left_login(&page).await;
    let landed = page.url().await.expect("url").expect("some url");
    assert!(
        landed.ends_with("/library"),
        "?next must land the ceremony on /library, got {landed}"
    );

    // Promote to Family via the pool; live role refresh applies it next request.
    sqlx::query("UPDATE users SET app_role = 'Family'")
        .execute(&server.pool)
        .await
        .expect("promote to Family");

    page.goto(server.url("/library")).await.expect("re-goto /library");
    let doors = page.content().await.expect("doors content");
    assert!(
        doors.contains("/library/audiobooks"),
        "Family sees the audiobooks door"
    );
    assert!(doors.contains("/pages/library"), "Family nav carries the tab");
    page.goto(server.url("/pages/library/audiobooks/e2e-book"))
        .await
        .expect("goto book as family");
    let book = page.content().await.expect("book content");
    assert!(book.contains("E2E Book"), "Family reads the book page");

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

/// Phase DV: the foliate-js EPUB reader BOOTS in a real browser — the de-risk the
/// direct tests can't give. Setup over HTTP (upload a real `.epub` as PUBLIC media +
/// embed it on a public page), then headless Chrome loads the page: the embed
/// hx-swaps in, `epub-reader.js` fetches the `.epub` Blob, mounts a `<foliate-view>`,
/// opens the book, and lifts the boot splash. Asserts the view mounts + the splash
/// lifts (the book opened + first page painted) + no fatal console error. Gating is
/// proved server-side; this proves the reader RUNS.
#[tokio::test]
async fn epub_reader_boots_in_browser() {
    use chromiumoxide::cdp::browser_protocol::log::{
        EnableParams as LogEnableParams, EventEntryAdded, LogEntryLevel,
    };
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");

    // Setup over reqwest: upload the fixture .epub as PUBLIC media + embed it on a
    // public page (so the browser loads it anonymously — no ceremony needed here).
    let http = reqwest::Client::builder().cookie_store(true).build().unwrap();
    http.post(server.url("/test/login?role=Admin")).send().await.unwrap();
    let epub =
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/test.epub")).unwrap();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(epub)
            .file_name("test.epub")
            .mime_str("application/epub+zip")
            .unwrap(),
    );
    let up: serde_json::Value =
        http.post(server.url("/media")).multipart(form).send().await.unwrap().json().await.unwrap();
    let media_ref = up["ref"].as_str().expect("media ref").to_string();
    http.post(server.url("/pages")).form(&[("page_title", "Reader Test")]).send().await.unwrap();
    let md = format!("# Reader Test\n\n![](/media/{media_ref})\n");
    http.put(server.url("/pages/reader-test"))
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

    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    // Capture console errors so a foliate boot failure is visible in CI output.
    page.execute(LogEnableParams::default()).await.expect("Log.enable");
    let mut entries = page.event_listener::<EventEntryAdded>().await.expect("log listener");
    let errors: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = errors.clone();
    let log_task = tokio::spawn(async move {
        while let Some(ev) = entries.next().await {
            if matches!(ev.entry.level, LogEntryLevel::Error) {
                sink.lock().unwrap().push(ev.entry.text.clone());
            }
        }
    });

    page.goto(server.url("/pages/reader-test")).await.expect("goto reader page");

    // Wait for the reader to mount + the splash to lift (book opened + painted).
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut booted = false;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let ok: bool = page
            .evaluate(
                "!!document.querySelector('.epub-reader foliate-view') \
                 && !document.querySelector('.epub-splash')",
            )
            .await
            .ok()
            .and_then(|r| r.into_value().ok())
            .unwrap_or(false);
        if ok {
            booted = true;
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await; // flush late error logs

    let view_present: bool = page
        .evaluate("!!document.querySelector('.epub-reader foliate-view')")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or(false);
    let splash_text: String = page
        .evaluate("(function(){var s=document.querySelector('.epub-splash');return s?s.textContent:'';})()")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or_default();

    let errs = errors.lock().unwrap().clone();
    let fatal: Vec<&String> = errs
        .iter()
        .filter(|e| {
            let t = e.to_lowercase();
            t.contains("epub-reader:") || t.contains("uncaught") || t.contains("is not a function")
        })
        .collect();

    browser.close().await.ok();
    handle.await.ok();
    log_task.abort();
    let _ = std::fs::remove_dir_all(&profile);

    assert!(view_present, "the <foliate-view> reader element mounted");
    assert!(
        fatal.is_empty(),
        "fatal console errors during foliate boot: {fatal:?}"
    );
    assert!(
        booted,
        "the reader never booted (splash never lifted); splash still reads {splash_text:?}, errors: {errs:?}"
    );
    drop(server);
}

// ── Editor boot e2e (fab-gui migration, CW.9) ────────────────────────────────
/// Loads `/3d/editor` in real headless Chrome and asserts the fab-gui WASM app
/// BOOTS end-to-end — coverage the direct-fetch integration tests (three_d.rs)
/// can't give: a real browser resolving the version-pathed bundle subresources
/// relative to the glue, instantiating the wasm, and enforcing COOP/COEP. Hard
/// asserts: every editor subresource loads (no 404), the `#fab-gui` bind canvas
/// exists, the context is actually cross-origin isolated (the isolation TOOK, not
/// just that headers were sent), no fatal wasm error hit the console, and the boot
/// splash LIFTS on the app's `fab-gui:ready` event (the "~8.7 MiB download isn't a
/// blank page" contract). The 3D render runs on headless SwiftShader; the
/// splash-lift is the only GPU/timing-sensitive assertion — the rest are deterministic.
#[tokio::test]
async fn editor_boots_in_browser() {
    use chromiumoxide::cdp::browser_protocol::log::{
        EnableParams as LogEnableParams, EventEntryAdded, LogEntryLevel, LogEntrySource,
    };
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    page.execute(LogEnableParams::default()).await.expect("Log.enable");
    let mut entries = page
        .event_listener::<EventEntryAdded>()
        .await
        .expect("log listener");
    let collected: std::sync::Arc<
        std::sync::Mutex<Vec<(LogEntrySource, LogEntryLevel, String, Option<String>)>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = collected.clone();
    let log_task = tokio::spawn(async move {
        while let Some(ev) = entries.next().await {
            let e = &ev.entry;
            sink.lock()
                .unwrap()
                .push((e.source.clone(), e.level.clone(), e.text.clone(), e.url.clone()));
        }
    });

    page.goto(server.url("/3d/editor")).await.expect("goto /3d/editor");

    // Wait for the splash to lift on `fab-gui:ready` (the app booted + painted).
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut booted = false;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let hidden: bool = page
            .evaluate("(function(){var s=document.getElementById('splash');return !!(s&&s.classList.contains('hide'));})()")
            .await
            .ok()
            .and_then(|r| r.into_value().ok())
            .unwrap_or(false);
        if hidden {
            booted = true;
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await; // flush late error logs

    let canvas_present: bool = page
        .evaluate("!!document.getElementById('fab-gui')")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or(false);
    let cross_origin_isolated: bool = page
        .evaluate("window.crossOriginIsolated === true")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or(false);
    // CW.10: the editor renders under the real site nav — confirm it's actually in
    // the DOM (the nav's main.css + fonts also exercise that same-origin subresources
    // survive COEP, which crossOriginIsolated above would go false on if they didn't).
    let nav_present: bool = page
        .evaluate("!!document.querySelector('nav[aria-label=\"Primary\"]')")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or(false);

    // CW.10 (scroll-fix): at LOAD the page must stay at the TOP — site header visible,
    // tool NOT pinned. The app focuses its canvas and the browser would scroll it into
    // view (jumping past the header); the preventScroll + reset guards keep it at top.
    // Give the boot-time reset nudges (ready + 50/300ms) a beat to settle, then check.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let initial_scroll_y: f64 = page
        .evaluate("window.scrollY")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or(-1.0);
    let starts_at_top = (0.0..5.0).contains(&initial_scroll_y);

    // CW.10 scroll behavior: scroll to the bottom, then the tool region must be PINNED
    // at the top (rect.top ~ 0) while the site nav has scrolled up out of view
    // (rect.top < 0) — the "header scroll-snaps away, tool takes over the screen" UX.
    page.evaluate("window.scrollTo(0, document.body.scrollHeight)").await.ok();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stage_top: f64 = page
        .evaluate("document.getElementById('stage').getBoundingClientRect().top")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or(9999.0);
    let nav_top: f64 = page
        .evaluate("document.querySelector('nav[aria-label=\"Primary\"]').getBoundingClientRect().top")
        .await
        .ok()
        .and_then(|r| r.into_value().ok())
        .unwrap_or(9999.0);
    let tool_pinned = stage_top.abs() < 2.0;
    let nav_scrolled_away = nav_top < 0.0;

    let logs = collected.lock().unwrap().clone();
    let mut subresource_failures = Vec::new();
    let mut fatal_wasm = Vec::new();
    for (src, lvl, text, url) in &logs {
        let is_editor_sub = url.as_deref().map(|u| u.contains("/3d/editor/")).unwrap_or(false);
        if matches!(lvl, LogEntryLevel::Error) && matches!(src, LogEntrySource::Network) && is_editor_sub {
            subresource_failures.push(format!("{text} — {}", url.as_deref().unwrap_or("")));
        }
        let t = text.to_lowercase();
        if matches!(lvl, LogEntryLevel::Error)
            && (t.contains("runtimeerror")
                || t.contains("could not grow")
                || t.contains("unreachable executed")
                || t.contains("panicked"))
        {
            fatal_wasm.push(text.clone());
        }
    }
    // On any failure, dump the whole log so the CI output shows what happened.
    if !booted || !subresource_failures.is_empty() || !fatal_wasm.is_empty() || !canvas_present || !cross_origin_isolated || !nav_present || !starts_at_top || !tool_pinned || !nav_scrolled_away {
        eprintln!("--- editor boot diagnostics: {} log entries (initial_scroll_y={initial_scroll_y}, stage_top={stage_top}, nav_top={nav_top}) ---", logs.len());
        for (src, lvl, text, url) in &logs {
            eprintln!("[{src:?}/{lvl:?}] {text} {}", url.as_deref().unwrap_or(""));
        }
    }

    browser.close().await.ok();
    handle.abort();
    log_task.abort();
    let _ = std::fs::remove_dir_all(profile);

    assert!(
        subresource_failures.is_empty(),
        "editor bundle subresource(s) failed to load: {subresource_failures:?}"
    );
    assert!(fatal_wasm.is_empty(), "fatal wasm error(s) in console: {fatal_wasm:?}");
    assert!(canvas_present, "the #fab-gui bind canvas is missing from the served document");
    assert!(
        cross_origin_isolated,
        "the editor context is not cross-origin isolated (COOP/COEP did not take in the browser)"
    );
    assert!(nav_present, "the real site nav (CW.10) did not render on the editor page");
    assert!(
        starts_at_top,
        "on load the page did not stay at the top — the tool auto-scrolled past the site header (initial_scroll_y={initial_scroll_y}, want ~0)"
    );
    assert!(
        tool_pinned,
        "after scrolling, the tool region is not pinned to the top (stage_top={stage_top}, want ~0)"
    );
    assert!(
        nav_scrolled_away,
        "after scrolling, the site nav did not scroll up out of view (nav_top={nav_top}, want <0)"
    );
    assert!(
        booted,
        "the boot splash never lifted — fab-gui:ready did not fire within 30s (the app failed to boot)"
    );
}

// ── Admin media UI e2e (Phase DR.3) ──────────────────────────────────────────

/// The admin media library, driven end-to-end in a REAL browser as Admin, exercises
/// EVERY mutation over the canonical `/media` REST surface (DR): drop-upload
/// (`POST /media`), add-encode (`POST …/variants`), rename + visibility
/// (`PUT /media/<ref>`), per-variant delete (`DELETE …/variants/<key>`), and item
/// delete (`DELETE /media/<ref>`). Generic `.bin` payloads ingest as
/// `MediaKind::File` → exactly one variant each (no derived srcset), so the variant
/// counts are deterministic. Every assertion is a SERVER-RENDERED truth observed
/// AFTER the mutation's reload — never the pre-reload DOM the JS itself just
/// touched — so a broken write can't false-pass. `window.prompt`/`confirm` are
/// stubbed (headless auto-dismisses them, which would no-op the JS).
#[tokio::test]
async fn admin_media_library_full_crud_in_browser() {
    if !ffprobe_available() {
        eprintln!("skipping admin media library e2e: ffprobe not found");
        return;
    }
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    add_virtual_authenticator(&page).await;
    register_first_admin(&page, &server, "e2e-media-admin").await;

    let file_a = write_temp_fixture("e2e-media-a", b"e2e media library upload A");
    let file_b = write_temp_fixture(
        "e2e-media-b",
        &(0..2048u32).map(|i| i.wrapping_mul(7) as u8).collect::<Vec<u8>>(),
    );

    page.goto(server.url("/admin/media")).await.expect("goto /admin/media");

    // ── Upload → a card appears carrying its server-minted media_ref.
    set_file_input(&page, "#media-file-input", &[&file_a]).await;
    let media_ref = wait_nonempty(
        &page,
        "(function(){var c=document.querySelector('.media-card');return c?c.getAttribute('data-media-ref'):'';})()",
        "upload → a media card with a data-media-ref appears",
    )
    .await;
    let media_ref = media_ref.trim().to_string();
    // Selectors scoped to THIS card (a UUIDv7 ref is a safe attribute-selector value).
    let variants = format!(
        "document.querySelectorAll('.delete-variant[data-media-ref=\"{media_ref}\"]').length"
    );
    let card = format!("!!document.querySelector('.media-card[data-media-ref=\"{media_ref}\"]')");
    wait_true(&page, &format!("{variants} === 1"), "the uploaded item has one variant").await;

    // ── Add-encode a second file → the card gains a second variant row.
    set_file_input(
        &page,
        &format!(".add-encode-input[data-media-ref=\"{media_ref}\"]"),
        &[&file_b],
    )
    .await;
    wait_true(&page, &format!("{variants} === 2"), "add-encode → a second variant appears").await;

    // ── Rename → the new title renders on the card (prompt() stubbed to return it;
    // the string only reaches the DOM once the server persists + the card reloads).
    let _: bool =
        js(&page, "(function(){window.prompt=function(){return 'Renamed By E2E';};return true;})()").await;
    click_selector(&page, &format!(".rename-media[data-media-ref=\"{media_ref}\"]")).await;
    wait_true(
        &page,
        "document.body.textContent.indexOf('Renamed By E2E') >= 0",
        "rename → the new title renders after reload",
    )
    .await;

    // ── Change visibility → the SERVER re-renders the Family option as `selected`
    // (an attribute the JS-set .value can't fake; only the PUT + reload adds it).
    let _: bool = js(
        &page,
        &format!(
            "(function(){{var s=document.querySelector('.media-visibility[data-media-ref=\"{media_ref}\"]');s.value='Family';s.dispatchEvent(new Event('change',{{bubbles:true}}));return true;}})()"
        ),
    )
    .await;
    wait_true(
        &page,
        &format!(
            "(function(){{var o=document.querySelector('.media-visibility[data-media-ref=\"{media_ref}\"] option[value=\"Family\"]');return !!(o&&o.hasAttribute('selected'));}})()"
        ),
        "visibility change persisted (Family server-rendered as selected)",
    )
    .await;

    // ── Delete one variant → the card drops to a single variant (confirm() stubbed).
    let _: bool = js(&page, "(function(){window.confirm=function(){return true;};return true;})()").await;
    click_selector(&page, &format!(".delete-variant[data-media-ref=\"{media_ref}\"]")).await;
    wait_true(&page, &format!("{variants} === 1"), "delete-variant → one variant remains").await;

    // ── Delete the item → the card disappears.
    let _: bool = js(&page, "(function(){window.confirm=function(){return true;};return true;})()").await;
    click_selector(&page, &format!(".delete-media[data-media-ref=\"{media_ref}\"]")).await;
    wait_true(&page, &format!("{card} === false"), "delete-item → the card is gone").await;

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    let _ = std::fs::remove_file(&file_a);
    let _ = std::fs::remove_file(&file_b);
    drop(server);
}

/// The inline editor upload (the 🎞 button / drop on the markdown box) drives
/// `POST /media`, reads the manifest `ref`, and inserts `![](/media/<ref>)` at the
/// cursor with NO page reload — the DR contract for editor-support.js re-reading
/// `ref` (not the retired `{media_id, media_ref, markdown}`) AND the no-refresh
/// promise (a page reload would eat unsaved markdown). We seed a marker into the
/// textarea and assert BOTH the embed and the marker coexist afterward.
#[tokio::test]
async fn inline_editor_upload_inserts_media_ref_in_browser() {
    if !ffprobe_available() {
        eprintln!("skipping inline editor upload e2e: ffprobe not found");
        return;
    }
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    add_virtual_authenticator(&page).await;
    register_first_admin(&page, &server, "e2e-inline-admin").await;

    // Create a page via the admin hub → the create handler redirects to
    // `<page>?edit=1`, landing us in the editor (markdown box + media input).
    page.goto(server.url("/admin/pages")).await.expect("goto /admin/pages");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let title = page
        .find_element("form[hx-post='/pages'] input[name='page_title']")
        .await
        .expect("new-page title input");
    title.click().await.expect("focus title");
    title.type_str("E2E Inline Media").await.expect("type title");
    click_selector(&page, "form[hx-post='/pages'] button[type=submit]").await;
    wait_true(
        &page,
        "!!document.getElementById('page_markdown') && !!document.getElementById('media-upload-input')",
        "create → the page editor renders",
    )
    .await;

    // Seed a marker so we can prove the embed is APPENDED and no reload wiped it.
    let _: bool = js(
        &page,
        "(function(){var t=document.getElementById('page_markdown');t.value='before-edit-marker';return true;})()",
    )
    .await;

    // Inline upload → POST /media, read the manifest ref, insert ![](/media/<ref>).
    let f = write_temp_fixture("e2e-inline", b"inline editor media bytes");
    set_file_input(&page, "#media-upload-input", &[&f]).await;
    wait_true(
        &page,
        "document.getElementById('page_markdown').value.indexOf('![](/media/') >= 0",
        "inline upload → an ![](/media/<ref>) embed is inserted",
    )
    .await;
    // The pre-existing text survived → no reload ate the edit (the DR no-refresh promise).
    let survived: bool = js(
        &page,
        "document.getElementById('page_markdown').value.indexOf('before-edit-marker') >= 0",
    )
    .await;
    assert!(survived, "the inline upload must NOT reload the page (unsaved markdown survives)");

    // EB.3: the textarea was never FOCUSED (the marker was set via JS), so the
    // embed must APPEND at the end — before the fix it landed at caret 0 (the
    // top of the markdown), which is where every mobile upload ended up.
    let appended: bool = js(
        &page,
        "(function(){var v=document.getElementById('page_markdown').value;return v.indexOf('before-edit-marker') === 0 && v.indexOf('\\n![](/media/') > 0;})()",
    )
    .await;
    assert!(appended, "the embed must append at the END of a never-focused textarea, not the top");

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    let _ = std::fs::remove_file(&f);
    drop(server);
}

/// Phase EA: the STL viewer's attract spin honors `prefers-reduced-motion`
/// (no auto-rotate at all) and hands the camera off PERMANENTLY on first grab.
/// Asserted via the `data-autorotate` seam `htmx-stl-view.js` stamps on the
/// `.stl-view` element (`controls` is module-scoped, invisible to the page).
/// The model URL 404s on purpose — the flags under test are set during viewer
/// SETUP, before (and independent of) the async STLLoader fetch. The spin RATE
/// (delta-corrected update) stays eyeball-verified; asserting rotation speed
/// from pixels is not worth the flake.
#[tokio::test]
async fn stl_viewer_spin_respects_reduced_motion_and_stops_on_grab() {
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    server
        .seed_blog_post("stl-spin", "![model](/no-such-model.stl)")
        .await
        .unwrap();

    let (mut browser, handle, profile) = launch().await;

    // 1) Reduced-motion viewer — emulate BEFORE load so matchMedia sees it at setup.
    let page = browser.new_page("about:blank").await.expect("new page");
    page.execute(
        SetEmulatedMediaParams::builder()
            .feature(MediaFeature::new("prefers-reduced-motion", "reduce"))
            .build(),
    )
    .await
    .expect("Emulation.setEmulatedMedia");
    page.goto(server.url("/blog/stl-spin")).await.expect("goto post");
    wait_true(
        &page,
        "document.querySelector('.stl-view')?.dataset.autorotate === 'off'",
        "reduced-motion viewer should come up with auto-rotate off",
    )
    .await;

    // 2) Default viewer — spins on load; the first grab kills it for good.
    let page = browser.new_page("about:blank").await.expect("new page");
    page.goto(server.url("/blog/stl-spin")).await.expect("goto post");
    wait_true(
        &page,
        "document.querySelector('.stl-view')?.dataset.autorotate === 'on'",
        "default viewer should come up auto-rotating",
    )
    .await;

    // Drag the canvas with real CDP input (mouse → synthesized pointerdown →
    // OrbitControls dispatches 'start').
    let canvas = page
        .find_element(".stl-view canvas")
        .await
        .expect(".stl-view canvas");
    let pt = canvas.clickable_point().await.expect("canvas clickable point");
    for (kind, x, buttons) in [
        (DispatchMouseEventType::MousePressed, pt.x, 1),
        (DispatchMouseEventType::MouseMoved, pt.x + 30.0, 1),
        (DispatchMouseEventType::MouseReleased, pt.x + 30.0, 0),
    ] {
        page.execute(
            DispatchMouseEventParams::builder()
                .r#type(kind)
                .x(x)
                .y(pt.y)
                .button(MouseButton::Left)
                .buttons(buttons)
                .click_count(1)
                .build()
                .expect("DispatchMouseEventParams"),
        )
        .await
        .expect("Input.dispatchMouseEvent");
    }
    wait_true(
        &page,
        "document.querySelector('.stl-view')?.dataset.autorotate === 'off'",
        "grabbing the model should stop the attract spin",
    )
    .await;

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

/// Phase EB: the quick-capture flow end-to-end in a real browser — capture.js
/// uploads via POST /media, POSTs the ref to /admin/capture, and on a "new
/// draft" result AUTO-SWITCHES to append mode so a multi-shot session accretes
/// into ONE draft. Two files through the library input: the first mints a
/// scheduled draft, the second must land in the SAME post.
#[tokio::test]
async fn quick_capture_creates_draft_then_accretes_in_browser() {
    if !ffprobe_available() {
        eprintln!("skipping: ffprobe not installed");
        return;
    }
    let _e2e = e2e_lock().await;
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    add_virtual_authenticator(&page).await;
    register_first_admin(&page, &server, "e2e-capture-admin").await;

    page.goto(server.url("/admin/capture")).await.expect("goto /admin/capture");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // First shot → a new scheduled draft (mode defaults to "draft").
    let f1 = write_temp_fixture("e2e-capture-1", b"first capture bytes");
    set_file_input(&page, "#capture-library", &[&f1]).await;
    wait_true(
        &page,
        "!document.getElementById('capture-result').classList.contains('hidden')",
        "first capture → the result banner fills",
    )
    .await;
    // The auto-switch: append mode is now selected with the fresh draft as target.
    wait_true(
        &page,
        "document.querySelector('input[name=\"capture-mode\"][value=\"append\"]').checked",
        "after a draft capture the mode auto-switches to append",
    )
    .await;
    let target: String = js(&page, "document.getElementById('capture-target').value").await;
    assert!(target.starts_with("capture-"), "append target is the new draft: {target}");

    // Second shot → must accrete onto that same draft, not mint another.
    let f2 = write_temp_fixture("e2e-capture-2", b"second capture bytes");
    set_file_input(&page, "#capture-library", &[&f2]).await;
    wait_true(
        &page,
        "document.getElementById('capture-status').textContent === 'Done'",
        "second capture completes",
    )
    .await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM content_pages WHERE page_name LIKE 'capture-%'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    let markdown: String = sqlx::query_scalar(
        "SELECT page_markdown FROM content_pages WHERE page_name LIKE 'capture-%'",
    )
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "two shots, ONE draft (session accretion)");
    assert_eq!(
        markdown.matches("![](/media/").count(),
        2,
        "both embeds in the one draft: {markdown}"
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    let _ = std::fs::remove_file(&f1);
    let _ = std::fs::remove_file(&f2);
    drop(server);
}
