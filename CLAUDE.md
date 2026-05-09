# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Christopher Hotchkiss's personal site / CRM. A single self-contained Rust binary that ships as a macOS `.app` with a tray icon, serves the site over HTTPS, and self-manages its own DNS (Cloudflare) and TLS certificates (ACME via Let's Encrypt). User roles: `Anonymous` (read-only) / `Registered` (read-only, logged in) / `Admin` (edit). The very first user to register is automatically promoted to `Admin` (enforced inline in `UserDao::create`).

## Common commands

- `bacon` — default dev loop. Runs `cargo run -- data/config.json`, kill-restarts on changes in `assets/`, `styles/`, `templates/`. Press `c` for clippy.
- `cargo build` / `cargo run -- data/config.json` — direct equivalents.
- `cargo test` — runs the suite. SQLx tests use `#[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]` and spin up isolated SQLite DBs per test.
- `cargo test <test_name>` — single test (e.g. `cargo test roundtrip`).
- `npm run format` — Prettier + Tailwind plugin formatting (templates / styles / config).
- `./build/macos/build.sh` — produces a signed + notarized `.pkg`. Resolves `VERSION` from `$GITHUB_REF_NAME` (or `$VERSION` / `git describe`) and reads `SIGN_IDENTITY`, `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID` from env. Fails fast with a clear error if any are unset.

Releases are tag-triggered: bump `version` in `Cargo.toml`, commit, then `git tag v$VERSION && git push --tags`. `release.yml` runs on a GitHub-hosted `macos-14` runner, builds + signs + notarizes, and creates a draft release. Un-drafting the release fires `install.yml` on the self-hosted server runner, which downloads the `.pkg` and runs `installer -target /`.

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

- **IpProviderService** polls `ifconfig.me` hourly and broadcasts the public IP. In `debug_assertions` builds it forces `127.0.0.1` so dev never touches the public DNS.
- **DnsProviderService** updates Cloudflare A records when the IP differs from DNS.
- **AcmeProviderService** loads or orders a Let's Encrypt cert (DNS-01 via Cloudflare), persists it in SQLite (`certificates` table), broadcasts a `RustlsConfig`, and refreshes every 6h.
- **EndpointsProviderService** waits for a `RustlsConfig` then starts both an HTTP→HTTPS redirect server on `:80` and the real axum app on `:443`. It also owns the `tower-sessions-sqlx-store` session GC task.

If any task returns, `tokio::try_join!` fails the whole coordinator — there is no auto-restart.

## Web layer (`src/web/`)

- **Routing** is composed in `web/router.rs`: `/` redirects to the first content page, `/login`, `/attachments`, `/pages`, `/projects` are nested routers, and `static_content()` is merged in. `LiveReloadLayer` is added **only** in debug builds.
- **Templating**: askama templates in `templates/`, served via the `HtmlTemplate<T>` wrapper. Templates reference `BUILD_TIME_CACHE_BUST` (a compile-time UTC epoch) for asset cache-busting query strings.
- **HTMX**: the frontend uses HTMX (`assets/vendor/htmx/`). Mutating handlers return `htmx_refresh()` or `htmx_redirect(...)` from `htmx_responses.rs` rather than rendering a response body.
- **Sessions**: `tower-sessions` backed by SQLite, signed with a key from the `crypto_keys` table (auto-generated on first boot via `CryptoKey::get_or_create`). Expiry: 1 day inactivity, secure-only.
- **Auth**: WebAuthn / passkeys via `webauthn-rs` (discoverable credentials, conditional UI). State machine in `AuthenticationState`: `Anonymous → AuthOptions → Authenticated`, or `Anonymous → RegistrationStarted → Authenticated`. Registration first creates the `UserDao` with `Role::Anonymous`; the SQL `INSERT` then conditionally upgrades to `Admin` (first user) or `Registered`.
- **Authorization checks live in handlers**, not middleware. Pattern: `if !session_data.auth_state.is_admin() { return FORBIDDEN }` at the top of each mutating handler. Don't move these to a layer without auditing every route.
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

Required fields: `cloudflare_token`, `database_path`, `domain`, `log_path`, `cache_path`. The `data/` directory in the repo is gitignored and contains the dev config + SQLite database.

## Things to watch out for

- **Patched `cookie` crate**: `Cargo.toml` patches `cookie` to a fork (`chotchki/cookie-rs` `serde_support` branch) to get serde on `cookie::Cookie`. Don't remove the `[patch.crates-io]` block when bumping deps.
- **Vendored OpenSSL**: `openssl` is pinned with `features = ["vendored"]` to avoid system-OpenSSL packaging issues for the signed `.pkg`.
- **Ports 80/443 are hardcoded** in `endpoints_provider_service.rs` — running locally as non-root will fail to bind. In practice you run via `bacon`/`cargo run` only after pointing a debug DNS at localhost, or you accept that the HTTP/HTTPS bind fails.
- **`assets/styles/main.css` is generated** — never edit it; edit `styles/tailwind.css` and let `build.rs` recompile.
- CI layout: `test_and_coverage.yml` runs on `macos-latest` per push (lint + test + grcov coverage). `release.yml` runs on `macos-14` (GitHub-hosted) on tag pushes — imports the Developer ID cert from a base64 secret into a temp keychain, signs and notarizes inline. `install.yml` runs on `self-hosted` and is the only workflow that needs your server: it fires on `release: types: [published]` and installs the latest `.pkg`.
