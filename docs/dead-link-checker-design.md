# Dead-link checker (Phase DL)

A daily background scan that flags rotted links in the site's own content, surfaced on an admin page (`/admin/dead-links`) alongside analytics + greylist, with a re-check trigger. The point is to catch link rot BEFORE a visitor does — a résumé pointing recruiters at a 404, a blog post citing a source that moved — without me remembering to click every link I've ever written.

Read this before touching `src/deadlinks/*`, `web/features/admin/dead_links.rs`, or migration `0029`.

## What makes a link checker USEFUL vs ignored

Naive link checkers get turned off within a week for two reasons, and the whole design is a reaction to both:

1. **They cry wolf on transient failures.** A momentary 5xx, a rate-limit, a DNS blip — the checker screams "DEAD", you go look, the link is fine, you learn to ignore it. Fix: **confirm-before-alarm** — an external link is only "confirmed dead" after N *consecutive daily* failures. One bad day is noise, three is a signal.
2. **They HTTP-fetch internal links and false-positive on auth.** This site role-gates (`min_role`) and schedules (`page_creation_date`) content — a gated-but-perfectly-alive page returns the cat-404 to an anonymous fetch. A checker that GETs its own host would flag every Family-gated library page as broken. Fix: **internal links are resolved STRUCTURALLY** against the DB (does the row exist), never fetched. HTTP is reserved for genuinely external hosts.

Those two bets — confirm-before-alarm + internal-structural — are the phase. Everything else is plumbing.

## Scope

**IN:** every link + image URL in the markdown of every `content_pages` row (blog posts, project pages, the résumé, library book pages, plain pages). That's where the authored, rot-prone links live.

**OUT:** template-hardcoded links (nav, footer — those are compile-checked askama and don't rot silently), the generated résumé PDF (derives from the same markdown, so checking the markdown covers it), and asset references. Non-content is not in the corpus.

**Link shapes handled** (mirroring `links.rs::collect`, which already walks `Node::Link` / `Node::Image` / `Node::Definition` — reference-style definitions included, which a Link-only walk would miss):
- **External** — `http(s)://otherhost/...` → HTTP-checked.
- **Internal** — root-relative `/...` (the form `rewrite_site_links` stores; same-site absolutes are relativized on save) → DB-resolved.
- **Skipped** — `mailto:` / `tel:` / `#anchor` / `data:` / `javascript:` / protocol-relative `//host` (rare, ambiguous) → recorded as `skipped`, never checked. A bare relative link (no leading `/`) is malformed for this site's stored form; treat as `skipped` + note.

The internal-vs-external fork IS `links.rs::relativize(url, site_host)`: `Some(path)` = internal (resolve in-DB), `None` = external-or-skippable (classify the scheme, HTTP-check only `http`/`https`).

## Architecture

One module `src/deadlinks/` with the DAO-free logic + a coordinator loop, plus the admin feature. Data flows:

```
content_pages ──enumerate──▶ extract_links (per page, over mdast)
                                   │
                          classify: internal | external | skipped
                                   │
                    ┌──────────────┴───────────────┐
              internal                          external
        resolve in-DB (no HTTP)          HTTP HEAD→GET (reqwest)
        find_by_path / find_by_ref       classify: ok/dead/transient/blocked
                    └──────────────┬───────────────┘
                                   ▼
                       link_check (per distinct URL) + link_ref (page↔url)
                                   ▼
                     /admin/dead-links  (grouped by page, with re-check)
```

### DL.2 — link extraction

`extract_links(markdown: &str) -> Vec<String>` (or a typed `RawLink`), a NEW read-only walk mirroring `links.rs::collect` (immutable `node.children()` recursion, the three-variant URL match) — NOT the `transformer.rs` BFS (that one is `children_mut()` for in-place rewriting, heavier than we need). Parse via `to_mdast(markdown, &Default::default())`.

The alpha `markdown-rs` can PANIC on pathological content (a real 2012-post smart-quote case took down the feed in CG). So extraction wraps the parse in `std::panic::catch_unwind` (or treats an `Err` as "skip this page's links") — a background scan must NEVER let one gnarly page abort the whole pass. Unit-tested against nested (list/table/heading/blockquote/emphasis) links, image links, reference definitions, and the skip cases.

### DL.3 — internal resolver (STRUCTURAL, no HTTP)

`resolve_internal(pool, path) -> InternalVerdict { Ok | Dead | Unknown }`. The route map (from the real router):

| URL shape | resolution | dead when |
|---|---|---|
| `/` | home | never |
| `/pages/A/B/C` | `find_by_path(pool, &["A","B","C"])` non-empty | path doesn't resolve |
| `/blog/<slug>` | `find_by_path(pool, &["blog", slug])` | slug not a blog child |
| `/pages/projects/<slug>` | `find_by_path` under projects | not found |
| `/resume`, `/resume.pdf` | special routes | never (special page exists) |
| `/media/<ref>` | `MediaDao::find_by_ref` is `Some` | ref not found |
| `/media/file/<url_key>` | `MediaVariantDao::find_by_url_key` is `Some` (64-hex guarded) | key not found |
| `/feed.xml` `/sitemap.xml` `/robots.txt` `/library` `/library/audiobooks` `/login` | known static routes | never |
| **`/projects/<slug>`** (slug present) | — | **ALWAYS DEAD** |
| any other `/...` | — | **Unknown** (surfaced for review, not hard-dead) |

Two load-bearing rules:

- **`/projects/<slug>` is the known dead-shape class (Phase CD).** `/projects` is the INDEX; project detail pages live at `/pages/projects/<slug>`. A link to `/projects/<something>` is the exact bug CD fixed in the feed — the checker must flag it, so this is an explicit rule, not a lookup.
- **Resolution is GATE-BLIND.** `find_by_path` is date/role-blind (it's the shared mutation lookup), which is exactly right here: a scheduled or `min_role`-gated page still EXISTS, so it's not dead. We resolve existence, never visibility. (This is why we don't HTTP-fetch — a fetch would apply the gate and lie.)
- **Special-page rows are aliases:** if a resolved row is `special_page`, its `page_markdown` is a redirect target, not content — treat as "exists", don't chase or flag it.

An `Unknown` internal path (a `/...` matching no known prefix) is surfaced as "unrecognized internal route — review", NOT confirmed-dead. The route map is hand-maintained and can drift from the router; failing soft on the unknown case means a new route I forgot to add here reads as "review this", never a false "broken". (Honest limit, documented below.)

### DL.4 — external checker

One reused `reqwest::Client` (built like `cloudflare_trace.rs`: `connect_timeout(10s)`, `timeout(15s)`, rustls), **redirects followed** (a 301→200 is healthy, not dead), an **identifying User-Agent** (`hotchkiss.io-linkcheck/<version> (+https://hotchkiss.io)` — so a webmaster who sees it in their logs knows what it is). **HEAD first, GET fallback** (many servers 405 a HEAD or lie about it; a GET with `Range: bytes=0-0` avoids pulling the body). Classification into four buckets:

| bucket | triggers | confirmed-dead-eligible |
|---|---|---|
| `ok` | 2xx, or 3xx that resolved to 2xx | no (resets the streak) |
| `dead` | 404, 410, DNS NXDOMAIN, connection refused | **yes** |
| `transient` | timeout, 429, 5xx, connection reset, TLS error | counts toward the streak, but never the LABEL |
| `blocked` | 401, 403, anti-bot 999/challenge | **no** — the link works in a browser; surfaced as "review", not dead |

The `blocked` bucket is the honest one: a 403 to our checker's UA doesn't mean the link is dead — plenty of sites bot-block. We surface it separately ("N links block automated checks — verify by hand") instead of lying.

DNS classification reuses the `crawler.rs` NoRecords-vs-error-vs-timeout split (`is_no_records` → `dead`; timeout/other → `transient`) — `reqwest` already runs `hickory-dns`, so a resolution failure surfaces as a reqwest error we bucket.

**Politeness + privacy** (I flagged this): an external HEAD reveals the mini's public IP + a once-daily crawl cadence to every host I link. That's an accepted, DELIBERATE cost for a personal site checking its own outbound links — mitigated by the identifying UA, a low **concurrency cap** (`MAX_CONCURRENT_CHECKS = 4`, a `Semaphore`), and **per-host serialization** (never two in-flight to the same host, a small inter-request delay) so I never hammer one server. **URLs are deduped** before checking — many pages cite the same source, it gets checked once per scan. An operator who wants ZERO outbound gets a `dead_link_check_external` config toggle (default on) that scopes the scan to internal-only.

### DL.5 — persistence + confirm-before-alarm

Migration `0029_TableDeadLinks.sql`, two tables (house style: `CREATE TABLE IF NOT EXISTS`, text timestamps, separate indexes — the `0024_TableGreylist` template):

```sql
-- One row per DISTINCT url ever seen in content. The scan's memory across days:
-- consecutive_failures is what makes "confirmed dead" mean "dead for N days
-- running", not "flaked once".
CREATE TABLE IF NOT EXISTS link_check (
    url                   text    NOT NULL,
    kind                  text    NOT NULL,   -- 'internal' | 'external'
    last_class            text    NOT NULL,   -- 'ok' | 'dead' | 'transient' | 'blocked' | 'unknown'
    last_status           INTEGER,            -- HTTP status, or NULL (DNS/internal)
    detail                text,               -- short human note (error kind, resolved-to)
    consecutive_failures  INTEGER NOT NULL DEFAULT 0,
    first_failed_at       text,               -- start of the current non-ok streak
    last_ok_at            text,
    last_checked_at       text    NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (url)
);
CREATE INDEX IF NOT EXISTS idx_link_check_class ON link_check (last_class);

-- Which pages reference which url, refreshed each scan (a url's referrers change
-- as content is edited). Powers the "grouped by page" admin view.
CREATE TABLE IF NOT EXISTS link_ref (
    page_id  INTEGER NOT NULL REFERENCES content_pages(page_id) ON DELETE CASCADE,
    url      text    NOT NULL,
    PRIMARY KEY (page_id, url)
);
CREATE INDEX IF NOT EXISTS idx_link_ref_url ON link_ref (url);
```

**Confirm-before-alarm.** `consecutive_failures` increments on ANY non-ok (dead OR transient), resets to 0 on `ok`. A URL is **confirmed dead** = `consecutive_failures >= CONFIRM_THRESHOLD` (const, **3** — three daily passes) AND `last_class == 'dead'` (the latest verdict is a hard-dead, not a flap). So: a genuinely-dead link surfaces after 3 days; a site that 5xx-flaps for a week accrues the streak but stays labeled `transient` (surfaced as "failing, not yet confirmed") until it either recovers or hard-404s. `blocked` and `unknown` never enter the confirmed-dead set — they're their own review buckets.

The `link_ref` table is REPLACED each scan (delete the scanned pages' rows, re-insert current) so the page↔url mapping always reflects live content — a link removed from a post stops being that post's problem. `link_check` PERSISTS across scans (that's where the streak lives); a URL that vanishes from all content just goes stale (a retention prune drops `link_check` rows with no `link_ref` after `RETAIN_DAYS`).

### DL.6 — daily coordinator loop

Mirrors the greylist sweep + daily backup: a `run_scan(pool, client, resolver, site_host, scanner) -> Result<ScanSummary>` split out from a `spawn(...)` wrapper, a `DEAD_LINK_SCAN_INTERVAL = 24h` module const, `tokio::time::interval` (`tick()` fires immediately then daily), every fallible step matched+logged so a bad pass logs + retries next tick and NEVER bubbles into the coordinator `try_join!`. Spawned DETACHED in `service_coordinator.rs` next to the backfills/sweep — a scan failure can't take the app down. Dark-launch-safe: on the scrubbed-`request_log` beta it just scans beta's content (which mirrors prod's), independent of traffic.

**Shared handle `DeadLinkScanState`** (mirrors `GreylistSet`: `Arc<Mutex<Inner>>`, cloned coordinator→loop + coordinator→`AppState`) holds `{ running: bool, last_started, last_finished, last_summary }`. Its job: a **single-flight guard** (`try_begin() -> Option<Guard>`, `None` if a scan is already running — so the daily tick and a manual trigger can't overlap) and the "scan running…" / last-run status the admin page shows.

### DL.7 — admin surface `/admin/dead-links`

Added to `admin_router()` (gated as a group by `require_admin` — no per-handler check), template `admin/dead_links.html`, styled like analytics/greylist:

- **Confirmed dead** — grouped by page, each row: the URL, `last_status`/`detail`, `consecutive_failures` ("dead 4 days"), `last_checked_at`, and an **Edit page** link (`/pages/<path>?edit=1`) to go fix it.
- **Failing (not yet confirmed)** — the `transient`-streak links, so a slow rot is visible before day 3.
- **Needs review** — `blocked` (bot-walled) + `unknown` internal routes, explicitly labeled as "verify by hand", not counted as broken.
- **Header** — last-scan time + summary counts + a **Run scan now** button.

**Run-now is SPAWN-and-return, not synchronous** (the key difference from greylist's `run_sweep`, which awaits because it's DB-only + fast). A full scan does external HTTP to every distinct host — potentially minutes — so the handler `try_begin()`s the single-flight guard, **spawns** `run_scan`, and immediately returns `htmx_refresh()` (the page shows "Scan running…"; a later refresh shows results). If a scan is already running, it's a no-op toast. **Per-URL re-check** (`POST /admin/dead-links/recheck` with the url) IS synchronous — one URL, bounded by the timeout — so I can clear a false positive (or confirm a fix) without re-scanning the whole site; on `ok` it resets the streak immediately.

### DL.8 — docs

CLAUDE.md delta (a "Dead-link checker (Phase DL)" bullet under the coordinator/admin sections) + PLAN sweep to `PLAN_ARCHIVE.md`.

## Testing

- **Extraction** (unit, pure): nested/table/heading/blockquote links, image links, reference definitions, skip cases (mailto/anchor/data), a panic-inducing input degrades to "no links" not a crash.
- **Internal resolver** (`#[sqlx::test]`): seed pages + media, assert Ok/Dead/Unknown incl. the `/projects/<slug>` dead-shape, a gated/scheduled page resolves Ok (existence not visibility), a `/media/<ref>` hit + miss.
- **Classification** (unit, pure): status→bucket table, the `consecutive_failures` streak math (increment/reset, confirmed-dead threshold), the `blocked`/`transient`/`dead` split.
- **External** — NOT hitting the real network in tests: the checker takes an injectable "fetch one URL → outcome" so the classification + streak logic tests use a stub; a single opt-in/feature-gated test may hit a known-stable URL, but the default suite is offline (no flaky network in CI).
- **Admin** (integration, `spawn_test_server`): `/admin/dead-links` is admin-gated (anon → 401, Registered → 403, Admin → 200), renders seeded dead links grouped by page, Run-scan-now + per-URL recheck are admin-gated mutations.

## Honest limits / deferred

- **The internal route map is hand-maintained** and can drift from the real router — a new top-level route not added here reads as `unknown` (soft "review"), never a false "dead". Acceptable; the alternative (introspecting axum's route table) isn't worth it at this scale.
- **`blocked` is a judgment call** — a 403 might be genuinely dead or just bot-walled. We surface, we don't decide. If a host I care about always 403s the checker, that's a manual "verify by hand".
- **No anchor-fragment validation** (`/pages/foo#section` checks `/pages/foo` exists, not the `#section`). Fragment-checking needs rendering the target; deferred.
- **No JS-rendered / SPA external targets** — a HEAD/GET sees the initial response only. A link to a client-rendered 404 that returns 200 + JS reads as `ok`. Inherent to non-browser checking; accepted.
- **Deferred levers:** operator-tunable interval/threshold in `Settings` (module consts for now, the house pattern), an email/notification on newly-confirmed-dead (the admin page is pull-only today), `link_check` history beyond the current streak (trend graph), and checking the résumé PDF's rendered links independently.

## DL.9 — dogfood fixes (Accept header + manual ignore)

Two things surfaced dogfooding the checker on beta:

**1. crates.io (and content-negotiating hosts) served a false 404.** The cause wasn't an IP or UA block — it's `Accept`-header content negotiation. crates.io returns `404` to a request that doesn't advertise `Accept: text/html` (it routes bot/API requests differently), and `200` to a browser. Our checker sent HEAD + a bare `Range` GET with no `Accept`, so a live link read as dead. **Fix:** the checker now sends a browser-like `Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8` on every request. Verified: server-rendered sites (GitHub) stay accurate (200 real / 404 missing); a client-rendered SPA (crates.io) degrades to always-`200` — which is the documented "assume ok" honest-limit (a false-ok on a genuinely-dead crate link is far less noisy than a false-dead on every live one).

**2. The "can we client-side recheck to work around a block?" question → no, and here's why.** A browser `fetch()` to a cross-origin URL returns an **opaque** response (CORS) — JS literally can't read the status, `.ok` is always false and `.status` is 0. So a client-initiated *automated* recheck is impossible for exactly the third-party URLs we'd want it for. The only client-side check is a human opening the link and eyeballing it. **So the escape hatch is a manual `Ignore`:** each problem link gets an **Open** (new tab, to check by hand) + an **Ignore** (dismiss) action; a dismissed link — a browser-only SPA, an IP/login-walled host that works in a browser but not for the checker — drops out of the problem buckets into a collapsed **Ignored** list (un-ignore to restore). The daily scan keeps recording it (the flag is `link_check.ignored`, migration `0030`, preserved across scans by `next_state`); the admin view just suppresses it. `Ignore`/`Un-ignore` are `POST /admin/dead-links/{ignore,unignore}`.
