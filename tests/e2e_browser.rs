//! Browser e2e (Phase 8.4) — pure Rust via `chromiumoxide` (CDP). Drives a real
//! headless Chrome against the in-process harness (`spawn_test_server`), using a
//! CDP **virtual authenticator** so the WebAuthn/passkey ceremony completes with
//! no hardware or human.
//!
//! `#[ignore]`d (needs Chrome installed) — run with:
//!   `cargo test --test e2e_browser -- --ignored --nocapture`

use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::web_authn::{
    AddVirtualAuthenticatorParams, AuthenticatorProtocol, AuthenticatorTransport, EnableParams,
    VirtualAuthenticatorOptions,
};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use hotchkiss_io::test_support::{spawn_test_server, TestServer};

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
#[ignore = "browser e2e — needs Chrome; run via `cargo test --test e2e_browser -- --ignored`"]
async fn passkey_registration_then_admin_dashboard() {
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

/// Evaluate a JS expression on the page and return it as a JSON value.
async fn js<T: serde::de::DeserializeOwned>(page: &Page, expr: &str) -> T {
    page.evaluate(expr)
        .await
        .unwrap_or_else(|e| panic!("evaluate `{expr}`: {e}"))
        .into_value()
        .unwrap_or_else(|e| panic!("into_value `{expr}`: {e}"))
}

#[tokio::test]
#[ignore = "browser e2e — needs Chrome; run via `cargo test --test e2e_browser -- --ignored`"]
async fn blog_no_horizontal_scroll_on_mobile() {
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
#[ignore = "browser e2e — needs Chrome; run via `cargo test --test e2e_browser -- --ignored`"]
async fn top_nav_no_overflow_on_mobile() {
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");
    use_mobile_viewport(&page).await;

    page.goto(server.url("/blog")).await.expect("goto /blog");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let nav_scroll: i64 = js(
        &page,
        "document.querySelector('nav ul, ul.flex.flex-row')?.scrollWidth ?? 0",
    )
    .await;
    let nav_client: i64 = js(
        &page,
        "document.querySelector('nav ul, ul.flex.flex-row')?.clientWidth ?? 0",
    )
    .await;

    assert!(nav_client > 0, "nav <ul> not found on page");
    assert!(
        nav_scroll <= nav_client,
        "top nav <ul> overflows on a 390px viewport: scrollWidth={nav_scroll}, clientWidth={nav_client}",
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
#[ignore = "browser e2e — needs Chrome; run via `cargo test --test e2e_browser -- --ignored`"]
async fn admin_new_post_form_and_slugify_on_blog() {
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

    // Anonymous would not see the form; admin should.
    page.goto(server.url("/blog")).await.expect("goto /blog");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let has_form: bool = js(
        &page,
        "!!document.querySelector('form[hx-post=\"/pages/blog\"] input[name=\"page_name\"]')",
    )
    .await;
    assert!(has_form, "admin should see the + New post form on /blog");

    // Typing "Hello world" with a space should result in "hello-world" — the
    // oninput slugifier must run AND not eat the space.
    let slug_input = page
        .find_element("form[hx-post='/pages/blog'] input[name='page_name']")
        .await
        .expect("slug input");
    slug_input.click().await.expect("focus slug input");
    slug_input.type_str("Hello world").await.expect("type slug");

    let value: String = js(
        &page,
        "document.querySelector('form[hx-post=\"/pages/blog\"] input[name=\"page_name\"]').value",
    )
    .await;
    assert_eq!(
        value, "hello-world",
        "slugify should convert spaces and lowercase as you type"
    );

    // Submit and confirm the round-trip succeeds (no silent 400). After htmx
    // refresh the new post card should be on /blog.
    page.find_element("form[hx-post='/pages/blog'] button[type=submit]")
        .await
        .expect("submit button")
        .click()
        .await
        .expect("click submit");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let html = page.content().await.expect("page content");
    assert!(
        html.contains("hello-world"),
        "new post card should appear after submission; first 500 chars: {}",
        &html[..html.len().min(500)]
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
#[ignore = "browser e2e — needs Chrome; run via `cargo test --test e2e_browser -- --ignored`"]
async fn anonymous_forbidden_from_admin_dashboard() {
    let server: TestServer = spawn_test_server().await.expect("spawn harness");
    let (mut browser, handle, profile) = launch().await;
    let page = browser.new_page("about:blank").await.expect("new page");

    page.goto(server.url("/admin/analytics"))
        .await
        .expect("goto /admin/analytics");
    let html = page.content().await.expect("page content");
    assert!(
        html.contains("Admin only"),
        "anonymous request should hit the 403 body; first 300 chars: {}",
        &html[..html.len().min(300)]
    );

    browser.close().await.ok();
    handle.await.ok();
    let _ = std::fs::remove_dir_all(&profile);
    drop(server);
}

#[tokio::test]
#[ignore = "browser e2e — needs Chrome; run via `cargo test --test e2e_browser -- --ignored`"]
async fn analytics_usable_on_mobile() {
    // Regression guard for the prod report: the analytics dashboard "didn't even
    // look like a table, none of the widgets show" on a phone — a wide unwrapped
    // table forced the document past 390px and Safari mangled the whole layout.
    // The fix wraps every table in overflow-x-auto so nothing exceeds the
    // viewport; this asserts no page-wide horizontal scroll AND that the widgets
    // (chart, stat numbers, top-pages row) actually render.
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
    tokio::time::sleep(Duration::from_millis(250)).await;

    // The core fix: no element forces the document wider than the phone viewport.
    // (The wide recent-requests table still exists, but now scrolls inside its
    // own overflow-x-auto box rather than growing the page.)
    let scroll_width: i64 = js(&page, "document.documentElement.scrollWidth").await;
    let inner_width: i64 = js(&page, "window.innerWidth").await;
    assert!(
        scroll_width <= inner_width,
        "/admin/analytics has horizontal scroll on a 390px viewport: scrollWidth={scroll_width}, innerWidth={inner_width}",
    );

    // The widgets are actually present (not collapsed/hidden).
    let html = page.content().await.expect("page content");
    assert!(html.contains("<svg"), "the views-per-day chart should render");
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
