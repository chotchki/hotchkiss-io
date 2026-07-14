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

use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
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

/// Evaluate a JS expression on the page and return it as a JSON value.
async fn js<T: serde::de::DeserializeOwned>(page: &Page, expr: &str) -> T {
    page.evaluate(expr)
        .await
        .unwrap_or_else(|e| panic!("evaluate `{expr}`: {e}"))
        .into_value()
        .unwrap_or_else(|e| panic!("into_value `{expr}`: {e}"))
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

    // CW.10 sticky behavior: scroll to the bottom, then the tool region must be PINNED
    // at the top (rect.top ~ 0) while the site nav has scrolled up out of view
    // (rect.top < 0) — the "header slides away, tool takes over the screen" UX.
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
    if !booted || !subresource_failures.is_empty() || !fatal_wasm.is_empty() || !canvas_present || !cross_origin_isolated || !nav_present || !tool_pinned || !nav_scrolled_away {
        eprintln!("--- editor boot diagnostics: {} log entries (stage_top={stage_top}, nav_top={nav_top}) ---", logs.len());
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
