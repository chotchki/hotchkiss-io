//! DI.1 spike — a bare MCP server over rmcp's Streamable HTTP transport, mounted
//! under the existing axum stack at `/mcp`. One trivial `ping` tool proves the
//! SDK integrates with axum 0.8 + `AppState` and round-trips a JSON-RPC
//! `tools/call`. Auth is the EXISTING stack: `api_key_auth` (an Admin `hio_…`
//! Bearer key) or an Admin session, gated by the global `require_admin_for_mutations`
//! — the ONE client-agnostic authz path (chris's rule): every MCP tool call is a
//! POST, which that layer requires an Admin for, so there is NO per-nest auth
//! guard. GET /mcp is public there but harmless (stateless rmcp answers a GET with
//! 405 — no SSE channel — leaking nothing). Reads additionally honor the per-viewer
//! visibility gates via the shared `is_visible_to` (the read tools) — content
//! filtering, not authz.
//!
//! `/mcp` is a PUBLIC attack surface, so it's deliberately LOGGED (`request_log`)
//! and SUBJECT TO GREYLISTING (not in the greylist exempt-prefixes): abuse is
//! visible + defended. A greylisted unauthenticated IP hitting `/mcp` gets the 429
//! toll; a valid Admin key bypasses it (authenticated), and the detection sweep can
//! greylist an IP that probes `/mcp`.
//!
//! Config is stateless + `json_response` on purpose: no `Mcp-Session-Id`
//! bookkeeping and no `text/event-stream`, so the global `CompressionLayer` can't
//! corrupt an SSE frame. Host validation is DISABLED here (`disable_allowed_hosts`)
//! because our own `request_host`-based guard (h2 `:authority`-correct on this
//! site) is the intended owner — see docs/mcp-publishing-design.md + DI.4.

use std::sync::Arc;

use http::request::Parts;
use rmcp::{
    ErrorData, Json, ServerHandler,
    handler::server::{common::Extension, router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};

use crate::db::dao::content_pages::ContentPageDao;
use crate::db::dao::media::MediaDao;
use crate::db::dao::roles::Role;
use crate::web::app_state::AppState;
use crate::web::session::SessionData;

/// The MCP handler. Holds `AppState` so tools can reach the pool / site_host /
/// media store; a fresh one is built per session by the service factory.
#[derive(Clone)]
pub struct McpServer {
    state: AppState,
    tool_router: ToolRouter<McpServer>,
}

/// `ping` input. The schema is derived (schemars) and surfaced in `tools/list`.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PingParams {
    /// Optional message to echo back; defaults to "pong".
    #[serde(default)]
    pub message: Option<String>,
}

/// `ping` output — returned as structured JSON content (the content plane DI reuses).
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct PingResult {
    pub pong: String,
    pub site_host: String,
}

/// The caller's role, from the `api_key_auth`-injected `SessionData` in the request
/// `Parts` (rmcp injects the whole `Parts` into the tool context). Fail-closed to
/// Anonymous. `/mcp` is Admin-gated (the global authz layer), so this is Admin in
/// practice — but deriving it means the reads apply the visibility gate BY
/// CONSTRUCTION (via the shared `is_visible_to`), so relaxing `/mcp` to a lower tier
/// later just works, and a wiring slip HIDES content rather than leaking it.
fn viewer_role(parts: &Parts) -> Role {
    parts
        .extensions
        .get::<SessionData>()
        .map(|s| s.auth_state.role())
        .unwrap_or(Role::Anonymous)
}

/// Map a DB / transform error to a JSON-RPC internal error.
fn internal(e: anyhow::Error) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

/// A page's tree path from its parent path + slug (empty parent = top-level).
fn child_path(parent_path: Option<&str>, slug: &str) -> String {
    match parent_path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!("{p}/{slug}"),
        None => slug.to_string(),
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListPagesParams {
    /// Parent page path, e.g. "blog" / "projects" / "projects/sub". Empty or absent = top-level pages.
    #[serde(default)]
    pub parent_path: Option<String>,
    /// Optional case-insensitive filter on title or slug.
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct PageSummary {
    /// Full tree path — pass this to `get_page` / `update_page`.
    pub path: String,
    pub slug: String,
    pub title: String,
    /// Visibility gate: null = public; otherwise the minimum role (Registered / Family / Admin).
    pub min_role: Option<String>,
    /// True if the post is future-dated (a scheduled draft).
    pub scheduled: bool,
    pub featured: bool,
    /// Creation date (RFC3339).
    pub created: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetPageParams {
    /// The page's tree path, e.g. "blog/my-post" or "projects/skylander".
    pub path: String,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct PageDetail {
    pub path: String,
    pub slug: String,
    pub title: String,
    pub markdown: String,
    pub category: Option<String>,
    /// Visibility gate: null = public; otherwise the minimum role.
    pub min_role: Option<String>,
    /// Post date (RFC3339). A future date = a scheduled draft.
    pub creation_date: String,
    pub scheduled: bool,
    pub featured: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListMediaParams {
    /// Optional case-insensitive filter on the media title.
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct MediaSummary {
    /// The opaque media ref — embed as `![](/media/<media_ref>)` or set as a page `cover_ref`.
    pub media_ref: String,
    /// image / video / stl / audio / file.
    pub kind: String,
    pub title: Option<String>,
    /// Visibility gate: null = public; otherwise the minimum role.
    pub min_role: Option<String>,
}

/// `list_pages` output — object-wrapped because the MCP spec requires a tool's
/// outputSchema ROOT to be an object, not a bare array (a `Json<Vec<_>>` panics at
/// router construction).
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ListPagesResult {
    pub pages: Vec<PageSummary>,
}

/// `list_media` output — object-wrapped for the same reason as `ListPagesResult`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ListMediaResult {
    pub media: Vec<MediaSummary>,
}

#[tool_router]
impl McpServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Health check: echoes `message` (or \"pong\") and returns the site host. Proves the MCP transport + AppState wiring."
    )]
    async fn ping(
        &self,
        Parameters(PingParams { message }): Parameters<PingParams>,
    ) -> Result<Json<PingResult>, ErrorData> {
        Ok(Json(PingResult {
            pong: message.unwrap_or_else(|| "pong".to_string()),
            site_host: self.state.site_host.clone(),
        }))
    }

    #[tool(
        description = "List pages under a parent (empty parent_path = top-level). Honors the caller's visibility gate. Use it to find blog posts, project pages, or any content page before editing."
    )]
    async fn list_pages(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(ListPagesParams { parent_path, query }): Parameters<ListPagesParams>,
    ) -> Result<Json<ListPagesResult>, ErrorData> {
        let viewer = viewer_role(&parts);
        let pool = &self.state.pool;

        let parent = parent_path.as_deref().map(str::trim).filter(|p| !p.is_empty());
        let parent_id = match parent {
            None => None,
            Some(p) => {
                let segs: Vec<&str> = p.split('/').collect();
                let chain = ContentPageDao::find_by_path(pool, &segs)
                    .await
                    .map_err(internal)?;
                Some(
                    chain
                        .last()
                        .ok_or_else(|| ErrorData::invalid_params("parent_path not found", None))?
                        .page_id,
                )
            }
        };

        let q = query.as_deref().map(str::to_lowercase);
        let children = ContentPageDao::find_by_parent(pool, parent_id)
            .await
            .map_err(internal)?;
        let out = children
            .into_iter()
            .filter(|c| c.is_visible_to(viewer))
            .filter(|c| {
                q.as_deref().is_none_or(|q| {
                    c.display_title().to_lowercase().contains(q)
                        || c.page_name.to_lowercase().contains(q)
                })
            })
            .map(|c| PageSummary {
                path: child_path(parent, &c.page_name),
                slug: c.page_name.clone(),
                title: c.display_title(),
                min_role: c.min_role.clone(),
                scheduled: c.is_scheduled(),
                featured: c.is_featured(),
                created: c.page_creation_date.to_rfc3339(),
            })
            .collect();
        Ok(Json(ListPagesResult { pages: out }))
    }

    #[tool(
        description = "Get a single page's full markdown + metadata by path (e.g. 'blog/my-post'). Honors the caller's visibility gate: a page the caller can't see returns not-found (no existence leak)."
    )]
    async fn get_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(GetPageParams { path }): Parameters<GetPageParams>,
    ) -> Result<Json<PageDetail>, ErrorData> {
        let viewer = viewer_role(&parts);
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let chain = ContentPageDao::find_by_path(&self.state.pool, &segs)
            .await
            .map_err(internal)?;
        // Whole-path scan (like get_page_path): a gated ancestor hides the leaf, and a
        // gated OR missing page returns the SAME not-found (no existence oracle).
        if chain.is_empty() || !chain.iter().all(|n| n.is_visible_to(viewer)) {
            return Err(ErrorData::resource_not_found("page not found", None));
        }
        let lp = chain.last().unwrap();
        Ok(Json(PageDetail {
            path: segs.join("/"),
            slug: lp.page_name.clone(),
            title: lp.display_title(),
            markdown: lp.page_markdown.clone(),
            category: lp.page_category.clone(),
            min_role: lp.min_role.clone(),
            creation_date: lp.page_creation_date.to_rfc3339(),
            scheduled: lp.is_scheduled(),
            featured: lp.is_featured(),
        }))
    }

    #[tool(
        description = "List uploaded media (images, video, audio, STL, files). Honors the caller's media visibility gate. Returns each item's media_ref — embed it as `![](/media/<ref>)` or pass it as a page cover_ref."
    )]
    async fn list_media(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(ListMediaParams { query }): Parameters<ListMediaParams>,
    ) -> Result<Json<ListMediaResult>, ErrorData> {
        let viewer = viewer_role(&parts);
        let q = query.as_deref().map(str::to_lowercase);
        let all = MediaDao::find_all(&self.state.pool)
            .await
            .map_err(internal)?;
        let out = all
            .into_iter()
            .filter(|m| m.is_visible_to(viewer))
            .filter(|m| {
                q.as_deref()
                    .is_none_or(|q| m.title.as_deref().unwrap_or("").to_lowercase().contains(q))
            })
            .map(|m| MediaSummary {
                media_ref: m.media_ref,
                kind: m.kind,
                title: m.title,
                min_role: m.min_role,
            })
            .collect();
        Ok(Json(ListMediaResult { media: out }))
    }
}

// `router = self.tool_router` makes the handler read the stored router instead of
// rebuilding it via `Self::tool_router()` on every call (the macro's default).
#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo (= InitializeResult) is #[non_exhaustive], so mutate a default
        // rather than struct-literal it. The default advertises no capabilities;
        // enable tools so a client sees the tool surface on initialize.
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions =
            Some("hotchkiss.io publishing (DI.1 spike). Currently only `ping`.".to_string());
        info
    }
}

/// Build the Streamable-HTTP tower `Service` to nest at `/mcp`. Stateless +
/// JSON-response; host validation disabled (owned by our guard — DI.4).
pub fn mcp_service(state: AppState) -> StreamableHttpService<McpServer, LocalSessionManager> {
    StreamableHttpService::new(
        move || Ok(McpServer::new(state.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts(),
    )
}
