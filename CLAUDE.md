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
- `./build/macos/build.sh [--profile beta|prod]` — builds an ad-hoc-signed app bundle (no Apple Developer ID, no notarization, no `.pkg`). `--profile` (default `prod`) picks the bundle name + identifier so beta and prod coexist in `/Applications` and LaunchServices: `prod` → `Hotchkiss-IO.app` / `io.hotchkiss.web`, `beta` → `Hotchkiss-IO-Beta.app` / `io.hotchkiss.web.beta`. Honors `CARGO_TARGET_DIR`; writes `$CARGO_TARGET_DIR/<name>.app` and prints `BUILT_APP=<absolute-path>` + `PROFILE=<profile>` on stdout. Resolves `VERSION` from `$VERSION` → `$GITHUB_REF_NAME` → `git describe`, falling back to `0.0.0-dev` — on the mini the post-receive extracts a `.git`-less tree, so CFBundleVersion lands on `0.0.0-dev` (cosmetic; the runtime/log/tray version comes from `CARGO_PKG_VERSION`, so bump `Cargo.toml` to match each tag — single-source fix is backlogged). The post-receive hook calls this per profile.

Deployment is push-to-deploy with an **inverted flow** (Phase 12): one bare repo at `~/repos/hotchkiss-io.git` on the Mac mini, whose `post-receive` hook (`build/macos/post-receive`) **dispatches by pushed ref** — `git push origin main` → **beta** (bleeding edge), and `git push origin vX.Y.Z` (a `v*` tag) → **prod** (deliberate promotion). For each deploying ref the hook archives the tree into a per-profile work dir, runs `build/macos/build.sh --profile <profile>`, atomically swaps the new `.app` into `/Applications`, and kickstarts the matching LaunchAgent (self-bootstrapping if it isn't loaded). A failed build leaves the running app untouched, and each ref deploys in an isolated subshell so one ref's failure can't skip a sibling. **Releasing to prod:** bump `Cargo.toml`'s `version`, commit, `git push origin main` (lands on beta), then `git tag vX.Y.Z && git push origin vX.Y.Z`. Two LaunchAgents run side-by-side — prod `io.hotchkiss.web` on `:80`/`:443`, beta `io.hotchkiss.web.beta` on `:8080`/`:8443`. `origin` is the mini and is now the **only** remote — the public `github` mirror was removed (2026-06) to keep an in-progress rework private; re-add it (and scrub the docs first) before publishing again. Apple notarization / Developer ID / `.pkg` distribution was retired (2026-05): ad-hoc signing is sufficient. One-time mini setup is in `build/macos/SETUP.md`.

**Beta instance** (`beta.hotchkiss.io:8443`) is public (grey-cloud A record that beta's own `DnsProviderService` tracks to the public IP, reached via a router `:8443` forward) and serves its **own** LE-prod cert — beta deploys as a release build, so the cert is natively trusted (the iPhone installs the PWA with no profile). Every `main` push **snapshots prod's `database.sqlite` into beta** (`snapshot_prod_db_into_beta` in the hook, via `sqlite3 .backup` of the live prod DB): it scrubs `crypto_keys` (beta regenerates its session key), `tower_sessions`, `request_log` (visitor privacy), and `instant_acme_domains` (prod's LE account key); **preserves** beta's own cert + ACME account (captured/restored, so beta never re-orders against the 5/week LE duplicate-cert limit); and **carries prod's `users`/passkeys** so chris's prod passkey authenticates on beta — beta's `webauthn_rp_id` is the registrable parent `hotchkiss.io` (set in beta's config), and the WebAuthn `rp_origin` includes the non-default `:8443` port. Beta data is therefore ephemeral by design: any beta-only edit is blown away on the next `main` push.

## Build-time machinery (build.rs)

`build.rs` does three things that surprise people:

1. **Creates an ephemeral SQLite schema DB at `$OUT_DIR/schema.db`** and runs `src/db/migrations/` against it, then writes `DATABASE_URL` into `.env` and exports it to rustc. This is what lets `sqlx::query!` / `query_as!` macros type-check at compile time. **If you add a migration or change a query, you may need `cargo clean -p hotchkiss-io` (or delete the OUT_DIR `schema.db`) for sqlx macros to re-validate.**
2. **Downloads a pinned Tailwind CLI** (`TAILWIND_VERSION` in `build.rs`, currently `v4.3.0`, arm64 macOS standalone binary) into `OUT_DIR` (cached under a version-keyed filename, so bumping the pin re-downloads) and compiles `styles/tailwind.css` → `assets/styles/main.css`. The compiled CSS is gitignored. (DaisyUI used to be downloaded here too but was never wired into `tailwind.css` — removed 2026-05; the site is styled with hand-rolled Tailwind utilities.)
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

- **Routing** is composed in `web/router.rs`: `/` redirects to the first content page, `/login`, `/attachments`, `/pages`, `/projects`, `/blog`, `/admin` are nested routers, and `static_content()` is merged in. `LiveReloadLayer` is added **only** in debug builds. The outer `ServiceBuilder` stack adds (outermost first) a request-logging middleware (`web/middleware/request_log.rs` → `request_log` table, fire-and-forget insert), the `TraceLayer`, the session layer, and compression.
- **Templating**: askama templates in `templates/`, served via the `HtmlTemplate<T>` wrapper. Templates reference `BUILD_TIME_CACHE_BUST` (a compile-time UTC epoch) for asset cache-busting query strings.
- **HTMX**: the frontend uses HTMX (`assets/vendor/htmx/`). Mutating handlers return `htmx_refresh()` or `htmx_redirect(...)` from `htmx_responses.rs` rather than rendering a response body.
- **Sessions**: `tower-sessions` backed by SQLite, signed with a key from the `crypto_keys` table (auto-generated on first boot via `CryptoKey::get_or_create`). Expiry: 1 day inactivity, secure-only.
- **Auth**: WebAuthn / passkeys via `webauthn-rs` (discoverable credentials, conditional UI). State machine in `AuthenticationState`: `Anonymous → AuthOptions → Authenticated`, or `Anonymous → RegistrationStarted → Authenticated`. Registration first creates the `UserDao` with `Role::Anonymous`; the SQL `INSERT` then conditionally upgrades to `Admin` (first user) or `Registered`.
- **Authorization checks live in handlers**, not middleware — *except* the `/admin` nest (`web/features/admin/`), which is gated as a group by the `require_admin` layer (`web/middleware/require_admin.rs`); handlers under `/admin` don't repeat the check. Everywhere else, the pattern is `if !session_data.auth_state.is_admin() { return FORBIDDEN }` at the top of each mutating handler. Don't move those behind a layer without auditing every route (tracked in PLAN.md "Tech debt").
- **Errors**: handlers return `Result<Response, AppError>`. `AppError` wraps `anyhow::Error`, logs with a UUID trace id, and returns a 500 with that id to the client.
- **`AppError` swallows status info** — if a handler needs to return e.g. a 404 or 403, return `Ok((StatusCode::X, "msg").into_response())` instead of bubbling via `?`. This is a deliberate pattern, see existing handlers for examples.

## Content model

Pages are a self-referential tree in `content_pages` (parent_page_id → page_id). Lookup by URL path goes through `ContentPageDao::find_by_path(&[&str])` which walks segments. **Special pages** (`special_page = true`, seeded by migrations `0007` and `0010`) are routing redirects — their `page_markdown` is treated as a redirect target URL, not Markdown. `login`, `projects`, and `blog` are the current special pages; they cannot be deleted. The `/projects` and `/blog` handlers each query their special page by name and list its children — a misnamed/missing special page surfaces as a 500 with "Server misconfiguration". `/blog` (`web/features/blog.rs`) lists posts as cards newest-first by `page_creation_date` and serves `/blog/feed.xml` as Atom 1.0; `/blog/<slug>` renders a post via the same `GetPageTemplate` as `/pages/...` (so admin sees the same editor chrome from either URL — the editor form posts back to `/pages/<page_path>` absolutely).

Markdown is rendered through `web/markdown/transformer.rs`, which rewrites `.stl` image links to `<object class="stl-view" ...>` tags so the frontend STL viewer can pick them up, and turns fenced **```d2** code blocks into inline diagrams. The diagram pipeline (`web/markdown/diagram.rs`) is **source-in-HTML + HTMX swap** (Phase A): at page-render the fence becomes a one-line placeholder that shows the **d2 source** in a `<pre>` (so a crawler / LLM / no-JS reader sees the real source — diffable, LLM-parsable) and carries `hx-get="/diagram/<hash>" hx-trigger="load" hx-swap="outerHTML"`; the source is registered keyed by a SHA-256 content hash (content-addressed — distinct diagrams can't collide, identical ones dedupe). On load HTMX GETs the public `/diagram/{hash}` route (`web/features/diagram.rs`), which shells out to the **`d2` binary** (`brew install d2`; resolved via `$D2_BIN` → `/opt/homebrew/bin` → `/usr/local/bin` → PATH, so it works under the mini's minimal LaunchAgent PATH), embeds the SVG as a base64 `data:` URI `<img>` (isolated → no id/font collisions across diagrams), caches it, and returns it for the swap. The endpoint renders **only sources the server already emitted** (looked up by hash) — not an open d2 compiler. A missing d2 or a bad source returns a visible error block at HTTP 200 (so HTMX still swaps), never a 500. **`d2` must be installed** on dev + the mini + CI; without it, diagrams degrade to the error block. In-flow diagrams are capped at a max-height (the natural SVG size is injected so the `<img>` scales proportionally), and clicking one opens a zero-dependency pan/zoom lightbox (`assets/scripts/diagram-zoom.js`, loaded in `get_page.html`, bound via event delegation so it works on HTMX-swapped-in diagrams — pattern borrowed from recon-gen's `qs-lightbox`; CSS lives in `styles/tailwind.css`). Adding new AST rewrites means extending the `match` in the BFS walk in `transformer.rs`.

Attachments are stored as BLOBs in SQLite. `load_attachment[_by_id]` supports a `?width=N` query that re-encodes images to AVIF on the fly (passthrough for `.stl`).

## Configuration

`Settings` is loaded from JSON, in this order:

1. First CLI arg, if present (this is what `bacon` and dev use: `data/config.json`).
2. Otherwise `~/Library/Application Support/io.hotchkiss.web/config.json` on macOS, `$HOME/...` elsewhere.

Required fields: `cloudflare_token`, `domain`. Optional path fields — `database_path`, `log_path`, `cache_path` — default to `~/Library/Application Support/io.hotchkiss.web/data/database.sqlite`, `~/Library/Logs/io.hotchkiss.web`, and `~/Library/Caches/io.hotchkiss.web` respectively when omitted (see `Settings::resolve` in `src/settings.rs`). The on-disk JSON is deserialized into a private `RawSettings`; unknown keys are silently ignored. The `data/` directory in the repo is gitignored and contains the dev config + SQLite database.

The macOS app is **not sandboxed** (the `com.apple.security.app-sandbox` entitlement was dropped, 2026-05) — `NSHomeDirectory()` returns the real home, so the paths above are literal. On the production Mac the app runs as a LaunchAgent (`build/macos/io.hotchkiss.web.plist`) in the user GUI session, which keeps the tray icon alive; macOS Mojave+ lets the non-root process bind ports 80/443 because axum binds `INADDR_ANY`.

## Things to watch out for

- **Vendored OpenSSL**: `openssl` is pinned with `features = ["vendored"]` to avoid system-OpenSSL packaging issues.
- **Ports default to 80/443 but are configurable** (`http_port` / `https_port` in `Settings`, since Phase 11; beta runs 8080/8443) — running locally as non-root on 80/443 will fail to bind, so set high ports in your dev config or accept the bind failure. (On the production Mac 80/443 work unprivileged because macOS Mojave+ allows non-root binds to ports <1024 when binding `INADDR_ANY`.)
- **`assets/styles/main.css` is generated** — never edit it; edit `styles/tailwind.css` and let `build.rs` recompile.
- **Pushing deploys (inverted flow, Phase 12)** — `git push origin main` rebuilds + restarts **beta**; only a `v*` **tag** push rebuilds + restarts **prod** (~10–15s downtime during `kickstart`). Prod no longer auto-updates from `main`. A failed build aborts before the swap (the running app keeps serving), but a *successful build of broken runtime behavior* goes live on whichever target the ref maps to. `origin` is the mini and the only remote (the `github` mirror was dropped 2026-06).
- CI layout: `test_and_coverage.yml` runs on `macos-latest` per push (lint + test + grcov coverage), `check_ip.yml` is a small scheduled IP check — both are informational and gate nothing. The `release.yml` / `install.yml` GitHub Actions workflows were deleted (2026-05); deployment is the `git push origin main` → `post-receive` hook flow described under "Common commands". **Both remaining Actions are dormant since 2026-06**: with the `github` remote removed, nothing pushes to GitHub, so they no longer run.
