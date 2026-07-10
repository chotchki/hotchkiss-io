//! DI.1 spike — a bare MCP server over rmcp's Streamable HTTP transport, mounted
//! under the existing axum stack at `/mcp`. One trivial `ping` tool proves the
//! SDK integrates with axum 0.8 + `AppState` and round-trips a JSON-RPC
//! `tools/call`. Auth is the EXISTING stack: `api_key_auth` (an Admin `hio_…`
//! Bearer key) or an Admin session, gated by the global `require_admin_for_mutations`
//! (POST is admin-only by default) — this module adds no auth of its own yet.
//!
//! Config is stateless + `json_response` on purpose: no `Mcp-Session-Id`
//! bookkeeping and no `text/event-stream`, so the global `CompressionLayer` can't
//! corrupt an SSE frame. Host validation is DISABLED here (`disable_allowed_hosts`)
//! because our own `request_host`-based guard (h2 `:authority`-correct on this
//! site) is the intended owner — see docs/mcp-publishing-design.md + DI.4.

use std::sync::Arc;

use rmcp::{
    ErrorData, Json, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};

use crate::web::app_state::AppState;

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
