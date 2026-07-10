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
use sqlx::types::chrono::{DateTime, Utc};

use crate::db::dao::content_pages::ContentPageDao;
use crate::db::dao::media::MediaDao;
use crate::db::dao::roles::Role;
use crate::web::app_state::AppState;
use crate::web::features::pages::write::{self, PageUpdate, PageWriteError, WrittenPage};
use crate::web::session::SessionData;
use crate::web::util::category;

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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CreatePageParams {
    /// Where to create it: "blog" for a post, "projects" for a project, "" / absent for a top-level page, or any node path.
    #[serde(default)]
    pub parent_path: Option<String>,
    /// The human title; the URL slug is derived from it.
    pub title: String,
    /// Optional page body (markdown).
    #[serde(default)]
    pub markdown: Option<String>,
    /// Optional comma-separated category tags (add "featured" to pin on the home page).
    #[serde(default)]
    pub category: Option<String>,
    /// Visibility gate: "Public" / "Registered" / "Family" / "Admin". Omit to inherit the parent's gate.
    #[serde(default)]
    pub min_role: Option<String>,
    /// Optional post date (UTC "YYYY-MM-DDTHH:MM[:SS]"). A future date = a scheduled draft.
    #[serde(default)]
    pub creation_date: Option<String>,
    /// Optional cover: a media ref (from list_media) or a copyable `/media/...` form.
    #[serde(default)]
    pub cover_ref: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdatePageParams {
    /// The page's tree path, e.g. "blog/my-post".
    pub path: String,
    /// New title (omit to keep the current one).
    #[serde(default)]
    pub title: Option<String>,
    /// New markdown body (omit to keep).
    #[serde(default)]
    pub markdown: Option<String>,
    /// New comma-separated category tags (omit to keep; add/remove "featured" to pin/unpin).
    #[serde(default)]
    pub category: Option<String>,
    /// Visibility gate: "Public" clears it, a role sets it, omit keeps it (never silently loosens).
    #[serde(default)]
    pub min_role: Option<String>,
    /// New post date ("YYYY-MM-DDTHH:MM[:SS]"; omit to keep). A future date schedules it.
    #[serde(default)]
    pub creation_date: Option<String>,
    /// Cover: omit to KEEP the current cover, "" to CLEAR it, a media ref to SET it.
    #[serde(default)]
    pub cover_ref: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeletePageParams {
    /// The page's tree path.
    pub path: String,
    /// Must be true — delete is destructive.
    pub confirm: bool,
}

/// A create/update tool's result — the page's identity after the write.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct PageWriteResult {
    pub path: String,
    pub slug: String,
    /// The `/pages/<path>` URL.
    pub url: String,
    pub title: String,
    pub min_role: Option<String>,
    pub scheduled: bool,
    pub featured: bool,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct DeleteResult {
    pub deleted: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PagePathParam {
    /// The page's tree path, e.g. "blog/my-post".
    pub path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FeaturePageParams {
    /// The page's tree path.
    pub path: String,
    /// true to pin on the home Featured band, false to unpin (idempotent).
    pub featured: bool,
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct MediaUploadRecipe {
    /// A ready-to-run curl — fill in the file path; set $HIO_TOKEN to your API key.
    pub curl: String,
    /// What the response looks like + how to reference the uploaded media.
    pub notes: String,
}

/// Fields a create/update tool can set, merged over the CURRENT page (a partial
/// update) before the full-replace `write::update_page`.
struct WriteFields {
    title: Option<String>,
    markdown: Option<String>,
    category: Option<String>,
    min_role: Option<String>,
    creation_date: Option<String>,
    cover_ref: Option<String>,
}

fn write_result(w: WrittenPage) -> PageWriteResult {
    PageWriteResult {
        path: w.path_segments.join("/"),
        url: w.pages_url(),
        slug: w.slug,
        title: w.title,
        min_role: w.min_role,
        scheduled: w.scheduled,
        featured: w.featured,
    }
}

fn map_write_err(e: PageWriteError) -> ErrorData {
    match e {
        PageWriteError::EmptyTitle => {
            ErrorData::invalid_params("title must contain letters or numbers", None)
        }
        PageWriteError::NotFound => ErrorData::resource_not_found("page or parent not found", None),
        PageWriteError::Internal(e) => ErrorData::internal_error(e.to_string(), None),
    }
}

/// Merge `f` over the current page at `path` and write it — partial-update semantics
/// (absent fields keep their current value) over `write::update_page`'s full replace.
/// cover_ref: absent = keep the current cover, "" = clear, a ref = set.
async fn apply_page_update(
    state: &AppState,
    path: &[&str],
    f: WriteFields,
) -> Result<PageWriteResult, ErrorData> {
    let chain = ContentPageDao::find_by_path(&state.pool, path)
        .await
        .map_err(internal)?;
    let lp = chain
        .last()
        .ok_or_else(|| ErrorData::resource_not_found("page not found", None))?;
    let cover_ref = match f.cover_ref {
        None => crate::web::features::media::cover_ref_for(&state.pool, lp.page_id).await,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s),
    };
    let input = PageUpdate {
        title: f.title.or_else(|| lp.page_title.clone()),
        category: f.category.or_else(|| lp.page_category.clone()),
        markdown: f.markdown.unwrap_or_else(|| lp.page_markdown.clone()),
        order: lp.page_order,
        creation_date: f.creation_date,
        min_role: f.min_role,
        cover_ref,
    };
    let w = write::update_page(&state.pool, &state.site_host, path, input)
        .await
        .map_err(map_write_err)?;
    Ok(write_result(w))
}

/// Fetch the leaf page at `path` or a JSON-RPC not-found.
async fn find_leaf(state: &AppState, path: &[&str]) -> Result<ContentPageDao, ErrorData> {
    let chain = ContentPageDao::find_by_path(&state.pool, path)
        .await
        .map_err(internal)?;
    chain
        .last()
        .cloned()
        .ok_or_else(|| ErrorData::resource_not_found("page not found", None))
}

/// The page's current summary — re-fetched after an action mutation so the result
/// reflects the new scheduled / featured state.
async fn page_summary_at(state: &AppState, path: &[&str]) -> Result<PageSummary, ErrorData> {
    let lp = find_leaf(state, path).await?;
    Ok(PageSummary {
        path: path.join("/"),
        slug: lp.page_name.clone(),
        title: lp.display_title(),
        min_role: lp.min_role.clone(),
        scheduled: lp.is_scheduled(),
        featured: lp.is_featured(),
        created: lp.page_creation_date.to_rfc3339(),
    })
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
        // Enumerating the media library is an ADMIN capability, NOT viewer-gated like
        // the page reads: the opaque media_ref exists precisely so a non-admin CAN'T
        // enumerate media (unlike pages, which are browsable). So this tool is
        // admin-ONLY — even if /mcp is relaxed to a lower tier later, media
        // enumeration stays behind the admin role (/mcp is Admin-gated today, so this
        // is the belt-and-suspenders that keeps the "relax later" design safe).
        if viewer_role(&parts) != Role::Admin {
            return Err(ErrorData::invalid_request(
                "listing media requires the Admin role",
                None,
            ));
        }
        let q = query.as_deref().map(str::to_lowercase);
        let all = MediaDao::find_all(&self.state.pool)
            .await
            .map_err(internal)?;
        let out = all
            .into_iter()
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

    #[tool(
        description = "Create a page under parent_path (empty = top-level) from a title (the slug is derived). Optionally set markdown / min_role / creation_date / category / cover_ref in the same call. Inherits the parent's visibility gate unless min_role is given. Returns the new page's path + url."
    )]
    async fn create_page(
        &self,
        Parameters(p): Parameters<CreatePageParams>,
    ) -> Result<Json<PageWriteResult>, ErrorData> {
        // The write is authorized by the transport (/mcp is Admin-gated); the tool
        // reuses the same PageWrite service the editor does, so slug / link-rewrite /
        // min_role / inherit-on-create policy is identical.
        let parent: Vec<&str> = p
            .parent_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.split('/').collect())
            .unwrap_or_default();
        let created = write::create_page(&self.state.pool, &parent, &p.title)
            .await
            .map_err(map_write_err)?;

        // Title-only create → done. Any content field → fill it in via a merge-update.
        let has_content = p.markdown.is_some()
            || p.min_role.is_some()
            || p.creation_date.is_some()
            || p.category.is_some()
            || p.cover_ref.is_some();
        if !has_content {
            return Ok(Json(write_result(created)));
        }
        let segs: Vec<&str> = created.path_segments.iter().map(String::as_str).collect();
        let fields = WriteFields {
            title: Some(created.title.clone()),
            markdown: p.markdown,
            category: p.category,
            min_role: p.min_role,
            creation_date: p.creation_date,
            cover_ref: p.cover_ref,
        };
        Ok(Json(apply_page_update(&self.state, &segs, fields).await?))
    }

    #[tool(
        description = "Update a page by path — a PARTIAL update: only the fields you pass change, the rest keep their current value. cover_ref: omit to keep, \"\" to clear, a media ref to set. min_role: \"Public\" clears the gate, a role sets it, omit keeps it."
    )]
    async fn update_page(
        &self,
        Parameters(p): Parameters<UpdatePageParams>,
    ) -> Result<Json<PageWriteResult>, ErrorData> {
        let segs: Vec<&str> = p.path.split('/').filter(|s| !s.is_empty()).collect();
        let fields = WriteFields {
            title: p.title,
            markdown: p.markdown,
            category: p.category,
            min_role: p.min_role,
            creation_date: p.creation_date,
            cover_ref: p.cover_ref,
        };
        Ok(Json(apply_page_update(&self.state, &segs, fields).await?))
    }

    #[tool(
        description = "Delete a page by path. Requires confirm=true (destructive). Special pages (blog / projects / resume / library) cannot be deleted."
    )]
    async fn delete_page(
        &self,
        Parameters(DeletePageParams { path, confirm }): Parameters<DeletePageParams>,
    ) -> Result<Json<DeleteResult>, ErrorData> {
        if !confirm {
            return Err(ErrorData::invalid_params(
                "set confirm=true to delete a page",
                None,
            ));
        }
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let chain = ContentPageDao::find_by_path(&self.state.pool, &segs)
            .await
            .map_err(internal)?;
        let lp = chain
            .last()
            .ok_or_else(|| ErrorData::resource_not_found("page not found", None))?;
        if lp.special_page {
            return Err(ErrorData::invalid_params(
                "special pages cannot be deleted",
                None,
            ));
        }
        lp.delete(&self.state.pool).await.map_err(internal)?;
        Ok(Json(DeleteResult { deleted: segs.join("/") }))
    }

    #[tool(
        description = "Publish a page NOW (set its post date to the current instant), taking it out of scheduled/draft state. Returns the page's updated status."
    )]
    async fn publish_page(
        &self,
        Parameters(PagePathParam { path }): Parameters<PagePathParam>,
    ) -> Result<Json<PageSummary>, ErrorData> {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let lp = find_leaf(&self.state, &segs).await?;
        ContentPageDao::set_creation_date(&self.state.pool, lp.page_id, Utc::now())
            .await
            .map_err(internal)?;
        Ok(Json(page_summary_at(&self.state, &segs).await?))
    }

    #[tool(
        description = "Unpublish a page: move its post date far into the future so it becomes a hidden draft (Admin-only until re-published). Returns the page's updated status."
    )]
    async fn unpublish_page(
        &self,
        Parameters(PagePathParam { path }): Parameters<PagePathParam>,
    ) -> Result<Json<PageSummary>, ErrorData> {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let lp = find_leaf(&self.state, &segs).await?;
        let sentinel: DateTime<Utc> = "2999-01-01T00:00:00Z"
            .parse()
            .map_err(|e| ErrorData::internal_error(format!("bad draft sentinel: {e}"), None))?;
        ContentPageDao::set_creation_date(&self.state.pool, lp.page_id, sentinel)
            .await
            .map_err(internal)?;
        Ok(Json(page_summary_at(&self.state, &segs).await?))
    }

    #[tool(
        description = "Pin or unpin a page on the home page's Featured band (idempotent: featured=true pins, false unpins). Returns the page's updated status."
    )]
    async fn feature_page(
        &self,
        Parameters(FeaturePageParams { path, featured }): Parameters<FeaturePageParams>,
    ) -> Result<Json<PageSummary>, ErrorData> {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let lp = find_leaf(&self.state, &segs).await?;
        if category::is_featured(lp.page_category.as_deref()) != featured {
            let new_cat = category::toggle_featured(lp.page_category.as_deref());
            ContentPageDao::set_category(&self.state.pool, lp.page_id, new_cat)
                .await
                .map_err(internal)?;
        }
        Ok(Json(page_summary_at(&self.state, &segs).await?))
    }

    #[tool(
        description = "How to upload NEW media (images/video/files) out-of-band: returns a ready-to-run curl for POST /admin/media/upload with your API key. The MCP tools reference EXISTING media (list_media + cover_ref); this is the lane for adding new bytes. The response gives a media_ref to reference."
    )]
    async fn media_upload_recipe(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<Json<MediaUploadRecipe>, ErrorData> {
        let host = crate::web::util::host::request_host(&parts.headers, &parts.uri);
        let curl = format!(
            "curl -X POST https://{host}/admin/media/upload \\\n  \
             -H \"Authorization: Bearer $HIO_TOKEN\" \\\n  \
             -F \"file=@/path/to/your-file\" \\\n  \
             -F \"title=Optional title\" \\\n  \
             -F \"min_role=Family\"   # optional gate; omit for public"
        );
        let notes = "Response JSON: {\"media_id\":.., \"media_ref\":\"<ref>\", \"markdown\":\"![](/media/<ref>)\"}. \
             Reference <media_ref> via create_page/update_page cover_ref, or embed ![](/media/<ref>) in markdown."
            .to_string();
        Ok(Json(MediaUploadRecipe { curl, notes }))
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
        info.instructions = Some(
            "hotchkiss.io publishing server. Read: list_pages, get_page, list_media. Write: \
             create_page, update_page, delete_page. All Admin-gated; reads honor the visibility \
             gate, and create/update take a min_role to gate content."
                .to_string(),
        );
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
