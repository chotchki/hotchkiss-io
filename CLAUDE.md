# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Christopher Hotchkiss's personal site / CRM. A single self-contained Rust binary that ships as a macOS `.app` with a tray icon, serves the site over HTTPS, and self-manages its own DNS (Cloudflare) and TLS certificates (ACME via Let's Encrypt). User roles: `Anonymous` (read-only) / `Registered` (read-only, logged in) / `Admin` (edit). The very first user to register is automatically promoted to `Admin` (enforced inline in `UserDao::create`).

## Common commands

- `bacon` — default dev loop. Runs `cargo run -- data/config.json`, kill-restarts on changes in `assets/`, `styles/`, `templates/`. Press `c` for clippy.
- `cargo build` / `cargo run -- data/config.json` — direct equivalents.
- `cargo test` — runs the suite. SQLx unit tests use `#[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]` (isolated SQLite per test). `tests/` integration tests (`server.rs`, `web.rs`) spin up the web layer in-process via `hotchkiss_io::test_support::spawn_test_server()` (no IP/DNS/ACME coordinator; plain HTTP on `127.0.0.1:0`) and hit it with `reqwest`. A debug-only `POST /test/login?role=Admin|Registered` seam (`web/features/test_login.rs`, `#[cfg(debug_assertions)]` — absent from release) lets them reach role-gated routes without the WebAuthn dance.
- `cargo test --test e2e_browser -- --ignored` — browser e2e (`#[ignore]`d so plain `cargo test` skips them; needs Google Chrome installed). Pure Rust via `chromiumoxide` (CDP): drives headless Chrome against `spawn_test_server`, using a CDP **virtual authenticator** to complete the real passkey registration ceremony, then checks the `/admin/analytics` auth gate.
- `cargo test <test_name>` — single test (e.g. `cargo test roundtrip`).
- `npm run format` — Prettier + Tailwind plugin formatting (templates / styles / config).
- `./build/macos/build.sh` — builds an ad-hoc-signed `Hotchkiss-IO.app` (no Apple Developer ID, no notarization, no `.pkg`). Honors `CARGO_TARGET_DIR`; writes the bundle to `$CARGO_TARGET_DIR/Hotchkiss-IO.app` (or `target/Hotchkiss-IO.app` if unset) and prints `BUILT_APP=<absolute-path>` on stdout. Resolves `VERSION` from `$VERSION` → `$GITHUB_REF_NAME` → `git describe`, falling back to `0.0.0-dev`. The post-receive hook on the mini calls this.

Deployment is push-to-deploy: `git push origin main` pushes to a bare repo at `~/repos/hotchkiss-io.git` on the Mac mini, whose `post-receive` hook (`build/macos/post-receive`) archives the pushed tree, runs `build/macos/build.sh` with a persistent `CARGO_TARGET_DIR`, atomically swaps the new `.app` into `/Applications`, and `launchctl kickstart -k`s the `io.hotchkiss.web` LaunchAgent. A failed build leaves the running app untouched. `origin` is the mini; `github` is a mirror — push there too to keep it current. Apple notarization / Developer ID / `.pkg` distribution was retired (2026-05): the binary only ever deploys to one self-hosted Mac, so ad-hoc signing is sufficient. One-time mini setup is documented in `build/macos/SETUP.md`.

## Build-time machinery (build.rs)

`build.rs` does three things that surprise people:

1. **Creates an ephemeral SQLite schema DB at `$OUT_DIR/schema.db`** and runs `src/db/migrations/` against it, then writes `DATABASE_URL` into `.env` and exports it to rustc. This is what lets `sqlx::query!` / `query_as!` macros type-check at compile time. **If you add a migration or change a query, you may need `cargo clean -p hotchkiss-io` (or delete the OUT_DIR `schema.db`) for sqlx macros to re-validate.**
2. **Downloads the Tailwind CLI and DaisyUI** into `OUT_DIR` (cached; arm64 macOS binary only) and compiles `styles/tailwind.css` → `assets/styles/main.css`. The compiled CSS is gitignored.
3. Re-runs only when `assets/scripts`, `templates`, or `migrations` change.

`assets/` and `templates/` are bundled into the binary at compile time via `rust-embed` (`web/static_content.rs`) and `askama` respectively — there are no loose static files at runtime besides the SQLite database.

## Runtime architecture

`src/main.rs` → `lib::real_main` → `tray-wrapper` (macOS menu-bar anchor) → `create_server` closure → `ServiceCoordinator`.

The coordinator (`src/coordinator/service_coordinator.rs`) wires four long-lived tokio tasks connected by `broadcast` channels:

```
IpProviderService ──IPs──▶ DnsProviderService (Cloudflare API)
                                 │
AcmeProviderService ──RustlsConfig──▶ EndpointsProviderService (HTTP/HTTPS axum servers)
```

- **IpProviderService** polls `https://1.1.1.1/cdn-cgi/trace` hourly (parsing the `ip=` line) and broadcasts the public IP. Connecting to the IPv4 literal forces a v4 path. In `debug_assertions` builds it forces `127.0.0.1` so dev never touches the public DNS.
- **DnsProviderService** updates Cloudflare A records when the IP differs from DNS.
- **AcmeProviderService** loads or orders a Let's Encrypt cert (DNS-01 via Cloudflare), persists it in SQLite (`certificates` table), broadcasts a `RustlsConfig`, and refreshes every 6h.
- **EndpointsProviderService** waits for a `RustlsConfig` then starts both an HTTP→HTTPS redirect server on `:80` and the real axum app on `:443` (served with `into_make_service_with_connect_info::<SocketAddr>()` so handlers/middleware can see the client IP). It also owns the `tower-sessions-sqlx-store` session GC task and a daily `request_log` prune task (90-day retention).

If any task returns, `tokio::try_join!` fails the whole coordinator — there is no auto-restart.

## Web layer (`src/web/`)

- **Routing** is composed in `web/router.rs`: `/` redirects to the first content page, `/login`, `/attachments`, `/pages`, `/projects`, `/admin` are nested routers, and `static_content()` is merged in. `LiveReloadLayer` is added **only** in debug builds. The outer `ServiceBuilder` stack adds (outermost first) a request-logging middleware (`web/middleware/request_log.rs` → `request_log` table, fire-and-forget insert), the `TraceLayer`, the session layer, and compression.
- **Templating**: askama templates in `templates/`, served via the `HtmlTemplate<T>` wrapper. Templates reference `BUILD_TIME_CACHE_BUST` (a compile-time UTC epoch) for asset cache-busting query strings.
- **HTMX**: the frontend uses HTMX (`assets/vendor/htmx/`). Mutating handlers return `htmx_refresh()` or `htmx_redirect(...)` from `htmx_responses.rs` rather than rendering a response body.
- **Sessions**: `tower-sessions` backed by SQLite, signed with a key from the `crypto_keys` table (auto-generated on first boot via `CryptoKey::get_or_create`). Expiry: 1 day inactivity, secure-only.
- **Auth**: WebAuthn / passkeys via `webauthn-rs` (discoverable credentials, conditional UI). State machine in `AuthenticationState`: `Anonymous → AuthOptions → Authenticated`, or `Anonymous → RegistrationStarted → Authenticated`. Registration first creates the `UserDao` with `Role::Anonymous`; the SQL `INSERT` then conditionally upgrades to `Admin` (first user) or `Registered`.
- **Authorization checks live in handlers**, not middleware — *except* the `/admin` nest (`web/features/admin/`), which is gated as a group by the `require_admin` layer (`web/middleware/require_admin.rs`); handlers under `/admin` don't repeat the check. Everywhere else, the pattern is `if !session_data.auth_state.is_admin() { return FORBIDDEN }` at the top of each mutating handler. Don't move those behind a layer without auditing every route (tracked in PLAN.md "Tech debt").
- **Errors**: handlers return `Result<Response, AppError>`. `AppError` wraps `anyhow::Error`, logs with a UUID trace id, and returns a 500 with that id to the client.
- **`AppError` swallows status info** — if a handler needs to return e.g. a 404 or 403, return `Ok((StatusCode::X, "msg").into_response())` instead of bubbling via `?`. This is a deliberate pattern, see existing handlers for examples.

## Content model

Pages are a self-referential tree in `content_pages` (parent_page_id → page_id). Lookup by URL path goes through `ContentPageDao::find_by_path(&[&str])` which walks segments. **Special pages** (`special_page = true`, seeded by migration `0007`) are routing redirects — their `page_markdown` is treated as a redirect target URL, not Markdown. `login` and `projects` are the current special pages; they cannot be deleted. The `/projects` handler queries the `projects` special page by name and lists its children — a misnamed/missing special page surfaces as a 500 with "Server misconfiguration".

Markdown is rendered through `web/markdown/transformer.rs`, which rewrites `.stl` image links to `<object class="stl-view" ...>` tags so the frontend STL viewer can pick them up. Adding new AST rewrites means extending the `match` in the BFS walk there.

Attachments are stored as BLOBs in SQLite. `load_attachment[_by_id]` supports a `?width=N` query that re-encodes images to AVIF on the fly (passthrough for `.stl`).

## Configuration

`Settings` is loaded from JSON, in this order:

1. First CLI arg, if present (this is what `bacon` and dev use: `data/config.json`).
2. Otherwise `~/Library/Application Support/io.hotchkiss.web/config.json` on macOS, `$HOME/...` elsewhere.

Required fields: `cloudflare_token`, `domain`. Optional path fields — `database_path`, `log_path`, `cache_path` — default to `~/Library/Application Support/io.hotchkiss.web/data/database.sqlite`, `~/Library/Logs/io.hotchkiss.web`, and `~/Library/Caches/io.hotchkiss.web` respectively when omitted (see `Settings::resolve` in `src/settings.rs`). The on-disk JSON is deserialized into a private `RawSettings`; unknown keys are silently ignored. The `data/` directory in the repo is gitignored and contains the dev config + SQLite database.

The macOS app is **not sandboxed** (the `com.apple.security.app-sandbox` entitlement was dropped, 2026-05) — `NSHomeDirectory()` returns the real home, so the paths above are literal. On the production Mac the app runs as a LaunchAgent (`build/macos/io.hotchkiss.web.plist`) in the user GUI session, which keeps the tray icon alive; macOS Mojave+ lets the non-root process bind ports 80/443 because axum binds `INADDR_ANY`.

## Things to watch out for

- **Vendored OpenSSL**: `openssl` is pinned with `features = ["vendored"]` to avoid system-OpenSSL packaging issues.
- **Ports 80/443 are hardcoded** in `endpoints_provider_service.rs` — running locally as non-root will fail to bind. In practice you run via `bacon`/`cargo run` only after pointing a debug DNS at localhost, or you accept that the HTTP/HTTPS bind fails. (On the production Mac it works unprivileged because macOS Mojave+ allows non-root binds to ports <1024 when binding `INADDR_ANY`.)
- **`assets/styles/main.css` is generated** — never edit it; edit `styles/tailwind.css` and let `build.rs` recompile.
- **`git push origin main` deploys to production** — no staging step. The post-receive hook builds and swaps the live app, with ~10–15s of downtime during `launchctl kickstart -k`. A failed build aborts before the swap (running app keeps serving), but a *successful build of broken runtime behavior* goes live immediately. `origin` is the mini; `github` is the mirror.
- CI layout: `test_and_coverage.yml` runs on `macos-latest` per push (lint + test + grcov coverage), `check_ip.yml` is a small scheduled IP check — both are informational and gate nothing. The `release.yml` / `install.yml` GitHub Actions workflows were deleted (2026-05); deployment is the `git push origin main` → `post-receive` hook flow described under "Common commands".
