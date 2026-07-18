# MCP publishing server — agent-driven authoring over the Admin key

Design + rationale for the MCP server that lets an agent publish to the blog / projects / pages (Phase DI). This is the single source for WHY the pieces are shaped the way they are — the code comments carry the how, PLAN.md carries the task breakdown, this carries the decisions and the honest limits.

**Scope:** this is AUTHORING AUTOMATION for one operator, reused over the API key that already exists — an agent that can do what the web editor does, driven from `claude mcp add`. It is NOT a public API, NOT a new auth model, NOT multi-tenant, and NOT a general HTTP surface. What's explicitly NOT the job: OAuth, per-tool key scopes, binary media transport over the wire, server-push/streaming, or exposing anything a leaked Admin key couldn't already reach through the existing REST routes. The whole point is to add a VERB SURFACE (a set of MCP tools) on top of machinery that's already built and already authorizes — the new code is tools + one refactor, not a new subsystem.

## The problem, concretely

Publishing today means the web editor: create a title-only page (`POST /pages/blog`), then a second `PUT` fills markdown / date / cover / visibility, and covers get pasted as `![](/media/<ref>)` refs. It's fine at a desk. It is NOT something an agent can drive — there's no verb an LLM can call to say "write this post and put it under /blog," and the moment you want an agent in the loop (draft from notes, cross-post, backfill a Wayback-recovered post, bulk-tag) you're hand-piloting a browser.

The auth for this already exists. Phase CA shipped API keys: an `Authorization: Bearer hio_…` key authenticates as its user with full role delegation, and Phase E's fail-closed authz layer already lets an Admin key hit every mutation route. So the missing piece is NOT authentication and NOT authorization — it's a standard way for an agent to discover and call the operations. That standard is MCP, and the transport that fits a hosted HTTPS server reached over the internet is Streamable HTTP. Everything below is "mount an MCP endpoint that reuses the key, and give the tools a domain layer clean enough to call."

## Threat model — the blast radius is the Admin key, and that's a decision

Being honest about the adversary, because it sets how much machinery is worth building.

- **The key IS Admin (full delegation, decided).** The MCP tool set is the capability surface for the AGENT — an agent can only call the tools we define — but the SAME `hio_…` key still unlocks the entire Admin REST API if it leaks out of the agent's config. An agent holding this key can delete pages, flip visibility, and change any content, exactly as chris can at the editor. That is the accepted blast radius: the key lives in `claude mcp add --header`, on chris's own machine, same trust boundary as being logged in.
- **NOT in scope: a scoped "publish-only" key.** Tempting (a leak of a publish key can't nuke users or release greylist entries), but it's a real addition to the `api_keys` model and the auth middleware, and for a single-operator tool the Admin key is the honest default. It's the first deferred lever, with the trigger named below.
- **NOT in scope: defending the MCP endpoint against the public.** The endpoint is admin-gated end to end; an unauthenticated caller gets a flat 403 and can't enumerate a single tool. There is no anonymous read surface to protect.

The takeaway that drives the mount: because the key already carries Admin and the authz layer already passes it, the MCP server needs ZERO new auth code. It rides the existing middleware. The only auth decision left is failure SHAPE (§ One auth path).

## The request path

```d2
direction: down
client: "Claude Code\n(Bearer hio_… key)" {shape: oval}

stack: Global ServiceBuilder stack {
  log: "request_log\n(/mcp LOGGED + greylisted)" {shape: rectangle}
  apikey: "api_key_auth\ninjects Authenticated(Admin)" {shape: rectangle}
  authz: "require_admin_for_mutations\n(POST → Admin; GET public → 405)" {shape: rectangle}
  log -> apikey -> authz
}

svc: "/mcp: rmcp StreamableHttpService\nstateless + json_response" {shape: rectangle}
pagewrite: "PageWrite service\n(slug · link-rewrite · date · cover ·\nmin_role decode · two-write)" {shape: rectangle}
dao: "ContentPageDao / MediaDao" {shape: rectangle}

client -> stack.log
stack.authz -> svc: "Admin-only\nPOST tool calls"
svc -> pagewrite: "write tools"
svc -> dao: "read tools\n(is_visible_to)"
pagewrite -> dao
```

## Transport + mount — rmcp, stateless, under the existing stack

- **Crate: `rmcp` 2.2** (the official `modelcontextprotocol/rust-sdk`), features `server` + `macros` + `transport-streamable-http-server`, plus `schemars = "1.0"` for tool input schemas. It targets **axum ^0.8** — we're already on axum 0.8.1, so it drops in with no version bridge. Tools are declared with the `#[tool_router]` / `#[tool(description=…)]` / `#[tool_handler]` macros; each tool's input schema is auto-derived from a `#[derive(Deserialize, JsonSchema)]` request struct wrapped in `Parameters<T>`.
- **`StreamableHttpService` is a tower `Service`**, so it mounts as `Router::new().nest_service("/mcp", service)` merged into `create_router`'s composition — same shape as every other nest. Because the whole composed router is wrapped by the global `ServiceBuilder` AFTER composition, `/mcp` inherits the full stack (request-log, catch-panic, trace, session, api_key_auth, refresh_session_role, greylist, authz, compression) with no per-route wiring.
- **Run it STATELESS with `json_response: true` — both set EXPLICITLY.** The DI.1 spike found the defaults are the OPPOSITE of what the early (blog-derived) research claimed: `StreamableHttpServerConfig::default()` is `stateful_mode: true` / `json_response: false`, so we call `.with_stateful_mode(false).with_json_response(true)` (the config is `#[non_exhaustive]` — builder methods, not a struct literal). Stateless means no `Mcp-Session-Id` bookkeeping and every POST self-contained — rmcp synthesizes the init context per request (`peer_info_for_stateless_request`), so a bare `tools/call` needs NO prior `initialize` handshake — exactly right for one operator. `json_response: true` makes tool calls come back as a single JSON body instead of an SSE frame — load-bearing: we NEVER open a `text/event-stream`, sidestepping the CompressionLayer-buffers-SSE footgun entirely. We give up server→client push (progress, `tools/list_changed`, elicitation) — none of which a "publish my post" tool needs. `LocalSessionManager::default()` is still passed as the manager arg; it's inert in stateless mode.
- **Host validation: DISABLED — the Bearer-Admin gate IS the boundary (DI.4 decision).** rmcp's `allowed_hosts` (default loopback) exists to block DNS-REBINDING: a malicious web page making a BROWSER hit a local server with the victim's AMBIENT credentials (cookies). `/mcp` has NO ambient auth — it's reached by a non-browser client (Claude Code) carrying an explicit `Authorization: Bearer hio_…` header an attacker's page can neither forge nor replay from a victim's browser. So DNS rebinding doesn't apply, and host validation would defend nothing the Admin-key gate doesn't already. We `disable_allowed_hosts()` and let the auth gate BE the boundary — which also sidesteps the h2-`:authority`-survives-`nest` question entirely (moot when the check is off). (rmcp IS h2-correct if we ever want the check back — the DI.1 spike confirmed `parse_host_header` falls back to `uri.authority()` when the `Host` header is absent, the exact `axum::nest`-drops-the-synthesized-`Host` case `request_host` was built for.)
- **No `/mcp` auth guard — the global authz layer is the ONE path (chris's rule).** `require_admin_for_mutations` already gates every POST (and all tool calls are POSTs) to Admin regardless of client — api_key_auth runs OUTER, injects `Authenticated(Admin)`, the authz layer's `is_admin()` fallback passes with no allowlist entry; a non-Admin/no-key POST gets its flat 403 (no `WWW-Authenticate` → Claude Code clean-fails). A bespoke nest guard would just duplicate that. GET `/mcp` stays public at the global layer but harmless (stateless rmcp → 405, no SSE channel). See § Tool authorization.
- **`/mcp` is LOGGED + greylisted (a public attack surface).** It is NOT excluded from `request_log` and NOT in the greylist exempt-prefixes: chris wants `/mcp` abuse VISIBLE in the log + feeding the detection sweep (an IP probing `/mcp` can earn a greylist), and a greylisted unauthenticated IP hitting `/mcp` gets the 429 toll (a valid Admin key bypasses — the enforcement waves through any authenticated identity). Legit agent traffic is authenticated + low-volume (discrete tool calls, not media streaming), so it doesn't swamp the Humans/Bots signal.

## One auth path — reuse `api_key_auth`, don't invent a second token

The tempting alternative (rmcp examples show it) is a dedicated bearer-check middleware in front of `/mcp` comparing against a standalone token. We do NOT do that, because it's a SECOND auth mechanism for the same job, and this codebase has exactly one: the `hio_…` API key. Reusing it means:

- The client is configured with `claude mcp add --transport http hotchkiss https://hotchkiss.io/mcp --header "Authorization: Bearer ${HIO_TOKEN}"` (env-var expansion in the header is supported; the token is a normal Admin API key minted at `/admin/api-keys`).
- `api_key_auth` (global layer, OUTER) resolves the key and injects `Authenticated(Admin)`. The `/mcp` nest guard and the global authz layer both see Admin and pass. rmcp never sees auth at all — auth is the axum layer's job, cleanly separated from the protocol.
- Promote/demote/delete of the key's user takes effect immediately (the identity is re-read per request), and revoking the key at `/admin/api-keys` kills the agent instantly. No new lifecycle.

**Failure shape matters for the client.** Claude Code only chases an OAuth discovery flow when the server answers 401/403 WITH a `WWW-Authenticate` header. We implement no OAuth (the spec makes authorization OPTIONAL — a server MAY validate its own token), so a bad/absent/revoked key must return a **plain 403 with NO `WWW-Authenticate`** → Claude Code reports a clean "auth failed" instead of trying to run OAuth against us. That's why the nest guard returns a flat forbidden, not the `/admin` redirect and not an RFC-9728 challenge.

**Client target is Claude Code.** Claude.ai and Claude Desktop custom connectors lean OAuth-only today (no static-header UI as of this writing), so the static-Bearer approach targets Claude Code specifically. Widening to the connector UIs is a deferred lever (trigger: they ship static-header support, or we decide the OAuth resource-server dance is worth it).

## The `PageWrite` service — kill the handler-trapped logic (the real work)

This is the load-bearing refactor, and MCP is just the forcing function.

Recon found the publishing ORCHESTRATION lives inline in the axum handlers (`web/features/pages/mod.rs`), not in any reusable service. The `ContentPageDao` is clean, but `put_page_path` / `post_page_path` wrap it with policy that a non-HTTP caller can't reach:

- the cover 3-way resolve (absent → clear, resolves → set, unresolvable → PRESERVE so a typo can't wipe the cover),
- `rewrite_site_links(markdown, site_host)` on SAVE (absolute→root-relative; skipped entirely if you write markdown straight through the DAO),
- `parse_local_datetime` (PRIVATE — the datetime-local backdate semantics aren't reachable),
- the `min_role` string decode (`"Public"→NULL` / known role → set / else → KEEP; fail-closed, never silently loosen — lives in the handler, the DAO writes whatever string it's handed),
- inherit-on-create (`min_role = parent.min_role`),
- the `update()`-then-`set_cover()` TWO-WRITE (both stamp `page_modified_date`),
- the `2999` unpublish sentinel (a literal in the handler).

If the MCP tools re-implement any of this, it DRIFTS from the editor — the exact single-source failure CLAUDE.md warns about across the whole content model. So DI extracts a **`PageWrite`** service (a `web/features/pages/write.rs` module) that owns the create-and-fill orchestration end to end and returns a TYPED result:

```
WrittenPage { page_id, slug, path, url, title, min_role, scheduled, featured }
```

Both the existing HTTP handlers AND the MCP tools call `PageWrite`. The handlers become thin (extract form → build spec → call service → render), the MCP tools become thin (deserialize args → build spec → call service → wrap result), and there is ONE place the slug/link-rewrite/date/cover/min_role policy lives. It's a behavior-preserving refactor pinned by the existing integration tests plus new unit tests on the service — the kind of extraction that's overdue on its own merits and that MCP finally pays for.

## The response fork — headers as the state oracle, body as content (why SPA backends feel nice)

This is the generalization worth naming, because it's the thing that makes React/Angular backends feel clean, it falls out of the `PageWrite` extraction almost for free, and it's fodder for a post.

The naive read is "HTMX returns HTML-to-swap, a JSON API returns data — two different interaction models, you can't share them." That's WRONG, and seeing why is the whole idea: model the interaction as a VALUE, put it in the response HEADERS as a state oracle, and let the BODY carry content. HTMX already works this way — `htmx_refresh()` / `htmx_redirect(url)` are pure control-plane (empty body + one `HX-*` header), and HTMX's header vocabulary (`HX-Location`, `HX-Push-Url`, `HX-Redirect`, `HX-Refresh`, `HX-Retarget`, `HX-Reswap`, `HX-Reselect`, `HX-Trigger`) is a full state-transition DSL that lives ENTIRELY in headers. The split is already latent in the codebase (today's handlers throw the result away — they drop the DAO after `update()`, so even the new slug gets recomputed downstream); DI makes it explicit.

Three planes, and only the third is ever client-specific:

1. **Content** — the domain result (`WrittenPage`, or a page's fields). ONE value, always.
2. **State directive** — what the client does next: `Navigate(url) | Refresh | Swap{target, reselect} | Event{name, payload} | None`. ONE value, RENDERED per client — to `HX-*` headers for HTMX, to a native `303 Location` for a no-JS browser, to a small JSON envelope (or headers) for an SPA, DROPPED for MCP.
3. **Presentation** — how content becomes pixels: a server-rendered askama PARTIAL (HTMX), client-rendered from JSON (SPA), or not rendered at all (MCP).

The handler produces (1)+(2) — pure domain logic. A responder renders both off an extracted `ClientKind`. React/Angular is just "always pick the JSON render, the client owns the swap"; HTMX is "pick the HTML partial, the headers own the swap." SAME backend. **MCP is the PROOF the planes separate**: a tool call takes the content and DROPS the directive (it doesn't navigate), so a single handler that serves both HTMX and MCP has demonstrated content ⊥ control. And the oracle SHRINKS the read-side work — pulling target/swap/push-url out of the templates and into directive headers leaves the view as pure content, which is far more JSON-able than an askama context wired with swap semantics.

Two constraints keep it honest (and keep the post from being naive):

- **The oracle is SMALL; content stays in the body.** Proxy/server header limits (~8–16 KB) mean the directive carries control + tiny `HX-Trigger` payloads ONLY — never content. That reinforces the split rather than fighting it.
- **A dumb browser is NOT an oracle reader.** This site's no-JS commitment (forms, nav, the 404 all work JS-free) means the directive must ALSO render to native HTTP: `Navigate` → a real `303 Location` (browsers honor it natively), NOT an `HX-Redirect` a plain `<form>` POST ignores. So directives partition into NAVIGATE (body moot — the client leaves) vs RENDER (body IS the content), and the render step is three-way (HTMX headers+partial / native 303+HTML / JSON envelope+data), not two.

The irreducible bit — the honest limit — is plane (3): server-renders-a-partial vs client-renders-from-JSON is a real difference, but it's a RENDER-TARGET flag over one result, not two apps. And the GOAL is NOT to build an SPA — it's frontend PLURALISM at minimal cost: once the handler emits `(content, directive)`, a new client costs a RENDER impl, not a handler. That's the whole win — HTMX, a JSON API, and MCP off ONE write path, and the fourth frontend is a `ClientKind` arm. The thesis: **the interaction model is DATA, not code — put it in the headers, keep the body as content, and adding a frontend is a render not a rewrite.** DI builds this WRITE-side (planes 1–3 for the mutating handlers, per the phasing — writes are directive-heavy and body-light, so it's the clean place to prove it); the READ-side generalization (a serializable view-model per askama template) stays the deferred lever below, because the reads don't have a second consumer yet.

## The tool surface — full editor parity

Read tools so the agent can see before it writes, write tools that mirror the editor PUT, action tools that mirror the editor buttons, media the reference-only way (§ Media). All args are the editor's fields; the write tools funnel through `PageWrite`.

| Tool | Kind | Args | Notes |
|---|---|---|---|
| `list_pages` | read | `parent_path?`, `query?` | blog / projects / a subtree / top-level; returns `[{path, title, slug, visibility, scheduled, featured, created, order}]`. Admin sees scheduled + gated. |
| `get_page` | read | `path` | full `{title, markdown, category, min_role, creation_date, cover_ref, order, featured, url}`. |
| `list_media` | read | `query?` | title search; returns `[{ref, title, kind, url_key, dims}]` so the agent can pick a cover / embed. |
| `create_page` | write | `parent_path`, `title`, `markdown?`, `min_role?`, `creation_date?`, `cover_ref?`, `category?`, `featured?` | `parent_path` = `"blog"` / `"projects"` / `""` (top-level) / any node. Inherits parent `min_role`. Returns `WrittenPage`. |
| `update_page` | write | `path` + any of `{title, markdown, category, page_order, creation_date, min_role, cover_ref, featured}` | partial; mirrors `PutPageForm` exactly. The canonical PUT. |
| `delete_page` | write | `path`, `confirm: true` | refuses special pages (blog/projects/resume/library); `confirm` gate against an over-eager agent. |
| `publish_page` | action | `path` | `set_creation_date(now)` — mirrors the Publish-now button. |
| `unpublish_page` | action | `path` | the `2999` draft sentinel — mirrors Unpublish. |
| `feature_page` | action | `path`, `featured: bool` | idempotent SET (not toggle — an agent wants a target state), read-modify-write on the `featured` tag. |
| `media_upload_recipe` | read | — | returns the ready-to-run `curl` for THIS host + how to parse the ref (§ Media). |

The visibility / schedule / cover / featured controls are BOTH fields on `update_page` (the canonical PUT) and discrete action tools — deliberately, because the editor works the same way (the PUT form AND the buttons both exist). A single general `create_page`/`update_page` pair keeps the surface small; if an agent fumbles `parent_path` in practice, dedicated `create_blog_post` / `create_project` convenience wrappers are a trivial add (they'd just pin the parent).

## Tool authorization — the tools go THROUGH the site's gates, not around them

Ratified requirement (chris): the MCP tools must OBEY the site's auth model, never bypass it. Three rules, and they mirror the web app one-for-one.

- **ONE authz path gates it — the global `require_admin_for_mutations`.** No bespoke `/mcp` guard (chris's rule: auth is client-agnostic, one path). Every MCP tool call — initialize / tools/list / tools/call, read AND write — is a POST, and the global layer requires an Admin for every non-safe method, so a non-Admin key or no key gets that layer's flat 403 (no `WWW-Authenticate` → Claude Code clean-fails, no OAuth chase). `GET /mcp` is public at that layer but HARMLESS: stateless rmcp answers a GET with 405 (it offers no SSE channel), exposing nothing — not a tool list, not data (tools/list is a POST). The single global layer covers every meaningful operation; the DI.3 responder's `ClientKind` branches are RESPONSE rendering, never auth.
- **Page reads honor the visibility gate; media enumeration is ADMIN-ONLY.** `list_pages`/`get_page` derive the caller's role from the authenticated identity (the `api_key_auth`-injected `SessionData`, read from rmcp's request `Parts`) and apply the SAME `is_visible_to(viewer)` gate the web read paths use — never a raw `SELECT *`. Pages ARE browsable (the nav + `/blog` + `/projects` list them), so viewer-gating is the right model: relaxing `/mcp` to a lower tier later just opens the pages that viewer may see, and a fail-closed default (missing identity → `Anonymous`) HIDES not leaks. **`list_media` is DIFFERENT — it checks `viewer == Admin`, not `is_visible_to`** (chris's security review): ENUMERATING the media library is an admin capability, not viewer-gated content — the opaque `media_ref` exists precisely so a non-admin can't enumerate media (they reach only the refs handed to them, e.g. embedded in a page). Both are Admin-gated by the transport TODAY; the explicit `list_media` admin check keeps the "relax later" design safe — media enumeration stays behind Admin while the page reads correctly open up.
- **Mutations require the permission.** Every write tool sits behind that Admin gate — a non-Admin can't reach one, matching the web editor (Admin-only). No per-handler authz is skipped; the write tools call the same `PageWrite` service the editor does, so the min_role decode / cover / link-rewrite policy is identical.
- **The agent can DEFINE gates.** `create_page`/`update_page` take a `min_role` arg (`Public`/`Registered`/`Family`/`Admin`) that flows through `PageWrite`'s fail-closed decode (unknown → KEEP, never loosen), so the agent can publish gated content — an Admin-only draft, a Family-only library page. Media gating (`media.min_role`, Phase DC) is set at UPLOAD; the reference-only media tools SURFACE an item's gate but don't change it (that stays the web `/admin/media/{id}/visibility` control until a media-management tool is added).

## Media — two lanes, neither is binary-over-MCP

Media stays reference-first because binary transport over MCP is a bad fit (structured JSON args → base64 inflates ~33% and buffers the whole file in the JSON-RPC message, defeating the streaming upload path that exists precisely so multi-GB media is disk-bound not RAM-bound).

- **In-band (reference existing):** `list_media` surfaces refs by title; `create_page`/`update_page` take a `cover_ref`, and the agent embeds media in markdown as `![](/media/<ref>)` (the transformer already dispatches those to the media embed). The agent references what's already uploaded.
- **Out-of-band (new bytes via curl):** the agent uploads new files by firing `curl -F file=@… https://hotchkiss.io/media -H "Authorization: Bearer hio_…"` with the SAME key (the canonical `POST /media` — DR retired the old `/admin/media/upload`). It returns `201` + the item manifest; the agent parses `.ref` and feeds it back through the MCP tools. The `media_upload_recipe` tool hands the agent the exact command for the current host so it's self-sufficient without chris pasting instructions (the agent already holds the key). This reuses the proven streaming/dedup upload verbatim.

## What this deliberately does NOT do

- **No OAuth.** Authorization is OPTIONAL per the MCP spec; we validate our own Admin key. A bad token returns a flat 403 (no `WWW-Authenticate`) precisely so clients don't chase an OAuth flow.
- **No per-tool / scoped key.** The Admin key is the capability boundary; the tool set bounds the AGENT, not the key. Tightening is a deferred lever.
- **No binary media over the wire.** Reference existing + out-of-band curl. Base64-in-JSON is explicitly rejected.
- **No SSE / server-push.** Stateless + `json_response` — no progress, no `list_changed`, no elicitation. A publish tool doesn't need them.
- **No public / multi-tenant API.** Single operator, admin-gated end to end.
- **Not Claude.ai / Claude Desktop connectors (yet).** Their custom-connector UIs lean OAuth-only; the static-Bearer path targets Claude Code.
- **No new content SEMANTICS.** Scheduling, visibility (`min_role`), featured, covers — all reuse the EXACT existing page model and its fail-closed decodes. MCP adds no new state, no migration to `content_pages`.
- **rmcp is bleeding-edge (2.2.0, published two days before this doc) — but the DI.1 spike PASSED.** It compiled first-try against axum 0.8 (~30 lines of wiring) and round-trips `initialize` / `tools/list` / `tools/call` over a real Admin `hio_…` Bearer key through the FULL middleware stack (`tests/mcp.rs`, 2 green tests incl. the unauth-403 gate), with the h2 host-validation question resolved in its favor. Pinned exactly; the hand-roll (~150–250 LOC stateless JSON-RPC) stays the named fallback but is no longer expected. Cost accepted: ~19 transitive crates (schemars 1.2, sse-stream, darling, rand 0.10, …).

## Beta caveat

The prod→beta snapshot scrubs `crypto_keys` EXCEPT id 2 — and id 3 is the API-key HMAC pepper. So a PROD `hio_…` key does NOT authenticate on beta (its hash won't verify against beta's regenerated pepper), and the carried `api_keys` rows are inert there. To drive beta, mint a beta-specific Admin key against `https://beta.hotchkiss.io:8443` and point a SEPARATE `claude mcp add … https://beta.hotchkiss.io:8443/mcp` at it. `allowed_hosts` must include `beta.hotchkiss.io`. Because beta is a release build with a natively-trusted cert, the MCP endpoint is reachable there without a profile — which makes beta the right place to dogfood DI before a prod tag.

## Deferred levers (with their triggers)

- **Publish-scoped API key.** Trigger: the agent key leaks, or chris wants an agent that provably can't touch users/greylist/keys. Adds a scope column to `api_keys` + a scope check in the authz path.
- **The three-plane responder on the WEB handlers (header oracle + body content).** Trigger: chris wants the site's own routes to serve JSON (an SPA experiment, a second client, or the blog post). A `StateDirective` value rendered per `ClientKind` (HX-* headers / native 303 / JSON envelope) + a `View` render (askama partial / JSON) over one domain result. Cheap for writes (mostly directive-only), a per-resource lift for reads (the view-models) — build it when there's a consumer; DI leaves `WrittenPage` behind as the seed.
- **Binary media upload via server-fetches-URL.** Trigger: the agent needs to upload without a shell (no curl lane). An MCP tool takes a `url`, the handler streams the fetch straight into `MediaStore::stage().write_chunk()` — reuses the streaming path, still no base64.
- **MCP resources / prompts.** Trigger: read-heavy agent workflows want `@`-mentionable context (e.g. a `blog://drafts` resource) or slash-command templates. Tools cover authoring; resources/prompts are additive.
- **Stateful / SSE mode.** Trigger: a tool needs progress or server-push (none today).
- **Hand-rolled JSON-RPC.** Trigger: the DI.1 spike finds rmcp's h2 host-validation uncleanable, the dep tree too heavy, or the macros too constraining for `&self` tools that need `AppState`. The stateless JSON-only hand-roll is ~200 LOC and owns spec-drift; rmcp is preferred for durability but this is the escape hatch.

## Interactions decided elsewhere

- **`/mcp` is LOGGED, not excluded** (chris's call) — a public attack surface, so log it to SEE abuse + feed the greylist detection sweep. Unlike the `/admin/analytics` / `/media/file` / `/challenge` self-exclusions, `/mcp` isn't self-feeding and its legit traffic is authenticated + low-volume.
- **Greylist COVERS `/mcp`** — deliberately NOT in the exempt-prefixes, so a greylisted IP hitting `/mcp` without a valid key gets the 429 toll (correct — that's abuse), while a valid Admin key bypasses it (the enforcement waves through any authenticated identity, and api_key_auth injects it OUTER). With the logging above, an IP that probes `/mcp` both shows up as abuse AND can be greylisted by the sweep. **The toll is a JS proof-of-work page, and an MCP client has no JS engine** — so a greylisted NON-BROWSER client (`Accept: application/json`) gets a machine-readable 429 NOTICE instead of the unsolvable HTML interstitial: it names the greylist and tells the human behind the client to authenticate (a key bypasses the toll) or open a browser to solve it (`greylist_challenge::greylist_json_notice`, reusing DI.3's `ClientKind` classifier). A browser still gets the real interstitial.
- **The `PageWrite` extraction touches the existing page handlers** — behavior-preserving, pinned by the current integration suite plus new service-level unit tests. Any behavior change there is a bug, not a feature.
- **`site_host`** (the WebAuthn rp-id, `hotchkiss.io` on prod AND beta) is the source for `rewrite_site_links` (and would source rmcp's `allowed_hosts` if host validation were ever re-enabled) — one canonical host value, already on `AppState`.

## Phasing (DI, on ratification)

Slice B: MCP ships AND the write handlers get the multi-frontend responder (planes 1–3, write-side) — one write path serving HTMX + JSON + MCP.

- **DI.0** — Phase exit: an Admin `hio_…` key added via `claude mcp add --transport http … /mcp` can, from Claude Code, list/create/update/publish/schedule/gate/feature blog + project + generic pages and reference existing media (out-of-band curl recipe for new bytes); AND the web write handlers serve HTMX + JSON off ONE `StateDirective`/`ClientKind` responder (a new frontend is a render, not a handler); shipped to beta and dogfooded from a real Claude Code session.
- **DI.1** — Spike + build-vs-buy gate: pin `rmcp` 2.2 + `schemars` 1.0, stand up a bare `StreamableHttpService` (stateless, `json_response`) nested at `/mcp`, and DETERMINE rmcp's Host/Origin validation behavior over HTTP/2 (`:authority` vs `Host`). Decide rmcp vs hand-roll on the result. Bare `initialize` / `tools/list` reachable with an Admin key; flat 403 without.
- **DI.2** — Extract the `PageWrite` service (create-and-fill orchestration: slug, `rewrite_site_links`, datetime parse, cover 3-way resolve, `min_role` decode, inherit-on-create, two-write, publish/unpublish/feature setters) returning a typed `WrittenPage`; refactor the existing handlers onto it (behavior-preserving, pinned by tests + new unit tests). [content plane]
- **DI.3** — The multi-frontend write responder: `StateDirective` (Navigate / Refresh / Swap{target,reselect} / Event{name,payload} / None) + `ClientKind` (Htmx / NativeBrowser / Json) rendered off one `(directive, WrittenPage)` — HX-* headers + partial | native `303 Location` + HTML | JSON envelope + data. Refactor the write handlers (put/post/delete/publish/unpublish/feature) onto it; HTMX byte-identical, `Accept: application/json` returns `WrittenPage` + a directive envelope, a no-JS `<form>` POST gets a native 303. [directive + presentation planes]
- **DI.4** — Mount + auth: `/mcp` rides the ONE global authz path (`require_admin_for_mutations` gates the POST tool calls; NO bespoke nest guard — chris's rule). Host validation disabled (Bearer-Admin is the boundary). `/mcp` LOGGED + greylistable (public attack surface, not exempt). CompressionLayer is fine as-is (json_response, no SSE).
- **DI.5** — Read tools: `list_pages`, `get_page`, `list_media` (reuse the DI.3 JSON render of the domain result).
- **DI.6** — Write tools: `create_page`, `update_page`, `delete_page` (via `PageWrite`; tool results reuse `WrittenPage`'s JSON render; special-page + confirm guards).
- **DI.7** — Action tools + media lane: `publish_page`, `unpublish_page`, `feature_page`, `media_upload_recipe`.
- **DI.8** — Tests: unit (`PageWrite`, the directive renders), integration (`spawn_test_server` + a mint-Admin-key + raw JSON-RPC-over-HTTP client hitting `/mcp` for `initialize` / `tools/list` / `tools/call`, asserting a real page lands; Accept-negotiation on the web write routes returns HTMX vs JSON vs a 303; auth negatives 403).
- **DI.9** — Docs (CLAUDE.md delta: the `/mcp` surface, the `claude mcp add` recipe, the beta-key caveat, the multi-frontend responder) + deploy to beta + dogfood from a live Claude Code session; capture any tool-ergonomics pain as the follow-on seed.
