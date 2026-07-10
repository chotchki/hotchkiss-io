//! DI.3 — the multi-frontend write responder. A handler produces a `WriteOutcome`
//! (a state DIRECTIVE + optional content); this renders it per the request's
//! `ClientKind` — HX-* headers for htmx, a native `303` for a no-JS `<form>`, a
//! JSON envelope for an API / SPA / the MCP JSON path. ONE handler, N frontends: a
//! new client is a `ClientKind` arm, not a new handler. The interaction model is
//! DATA (the directive rendered to headers), not control flow — the header oracle.
//! See docs/mcp-publishing-design.md "The response fork".
//!
//! Scope today: the page WRITE handlers (put / post / delete). The READ side (a
//! serializable view-model per template) stays deferred — it has no second
//! consumer yet.

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Redirect, Response};
use http::request::Parts;
use http::{HeaderMap, StatusCode, header};
use serde::Serialize;

use crate::web::features::pages::write::WrittenPage;
use crate::web::htmx_responses::{htmx_redirect, htmx_refresh};

/// Which frontend is asking, inferred from request headers — the ONLY
/// client-specific axis (content + directive above it are shared). htmx wins over
/// `Accept` (an htmx request also sends `Accept`, but it wants the header oracle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    /// `HX-Request: true` — directives ride HX-* response headers, body empty.
    Htmx,
    /// `Accept: application/json` — content + directive ride a JSON body.
    Json,
    /// A plain browser / no-JS `<form>` — directives become native HTTP (a 303).
    NativeBrowser,
}

impl ClientKind {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        if headers.get("hx-request").is_some_and(|v| v == "true") {
            ClientKind::Htmx
        } else if headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("application/json"))
        {
            ClientKind::Json
        } else {
            ClientKind::NativeBrowser
        }
    }
}

impl<S> FromRequestParts<S> for ClientKind
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientKind::from_headers(&parts.headers))
    }
}

/// What the client should do next — the interaction model as a VALUE. Only the two
/// directives the page write handlers actually emit today; the fuller htmx
/// vocabulary (`Swap`, `Event`) gets added when a handler needs it.
#[derive(Debug, Clone)]
pub enum StateDirective {
    /// Send the client to `url`.
    Navigate(String),
    /// Re-render the current view.
    Refresh,
}

/// A write's outcome: the DIRECTIVE (what to do) + optional CONTENT (the entity).
/// `content` is `None` where there's nothing to represent (a delete).
pub struct WriteOutcome {
    pub directive: StateDirective,
    pub content: Option<WrittenPage>,
}

impl WriteOutcome {
    pub fn navigate(url: impl Into<String>, content: Option<WrittenPage>) -> Self {
        Self {
            directive: StateDirective::Navigate(url.into()),
            content,
        }
    }

    pub fn refresh(content: Option<WrittenPage>) -> Self {
        Self {
            directive: StateDirective::Refresh,
            content,
        }
    }

    /// Render per client. htmx → the existing `htmx_redirect`/`htmx_refresh`
    /// (byte-identical, so the htmx tests are the guardrail); native → a `303`;
    /// json → a `{ directive, page }` envelope.
    pub fn into_response(self, client: ClientKind) -> Response {
        match client {
            ClientKind::Htmx => match self.directive {
                StateDirective::Navigate(url) => htmx_redirect(&url)
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                StateDirective::Refresh => htmx_refresh(),
            },
            ClientKind::NativeBrowser => {
                // A no-JS client can't read the oracle, so the directive becomes a
                // real 303: Navigate → its url; Refresh → reload the entity just
                // written (fallback root if there's no content, e.g. a delete).
                let url = match &self.directive {
                    StateDirective::Navigate(url) => url.clone(),
                    StateDirective::Refresh => self
                        .content
                        .as_ref()
                        .map(WrittenPage::pages_url)
                        .unwrap_or_else(|| "/".to_string()),
                };
                Redirect::to(&url).into_response()
            }
            ClientKind::Json => {
                let directive = match self.directive {
                    StateDirective::Navigate(url) => JsonDirective::Navigate { url },
                    StateDirective::Refresh => JsonDirective::Refresh,
                };
                axum::Json(JsonEnvelope {
                    directive,
                    page: self.content,
                })
                .into_response()
            }
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonDirective {
    Navigate { url: String },
    Refresh,
}

#[derive(Serialize)]
struct JsonEnvelope {
    directive: JsonDirective,
    page: Option<WrittenPage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_page() -> WrittenPage {
        WrittenPage {
            page_id: 7,
            slug: "my-post".into(),
            path_segments: vec!["blog".into(), "my-post".into()],
            title: "My Post".into(),
            min_role: None,
            scheduled: false,
            featured: false,
        }
    }

    #[test]
    fn htmx_refresh_sets_hx_refresh_header() {
        let r = WriteOutcome::refresh(Some(sample_page())).into_response(ClientKind::Htmx);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers().get("hx-refresh").unwrap(), "true");
    }

    #[test]
    fn htmx_navigate_sets_hx_redirect_header() {
        let r = WriteOutcome::navigate("/pages/blog/my-post?edit=1", Some(sample_page()))
            .into_response(ClientKind::Htmx);
        assert_eq!(
            r.headers().get("hx-redirect").unwrap(),
            "/pages/blog/my-post?edit=1"
        );
    }

    #[test]
    fn native_navigate_is_a_303_to_the_url() {
        let r = WriteOutcome::navigate("/pages", None).into_response(ClientKind::NativeBrowser);
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/pages");
    }

    #[test]
    fn native_refresh_redirects_to_the_written_page() {
        let r = WriteOutcome::refresh(Some(sample_page())).into_response(ClientKind::NativeBrowser);
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            r.headers().get(header::LOCATION).unwrap(),
            "/pages/blog/my-post"
        );
    }

    #[tokio::test]
    async fn json_carries_the_directive_and_the_page() {
        let r = WriteOutcome::navigate("/pages/blog/my-post?edit=1", Some(sample_page()))
            .into_response(ClientKind::Json);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["directive"]["type"], "navigate");
        assert_eq!(v["directive"]["url"], "/pages/blog/my-post?edit=1");
        assert_eq!(v["page"]["slug"], "my-post");
        assert_eq!(v["page"]["featured"], false);
    }

    #[test]
    fn client_kind_precedence_htmx_over_json_over_native() {
        let mut h = HeaderMap::new();
        assert_eq!(ClientKind::from_headers(&h), ClientKind::NativeBrowser);
        h.insert(header::ACCEPT, "application/json".parse().unwrap());
        assert_eq!(ClientKind::from_headers(&h), ClientKind::Json);
        h.insert("hx-request", "true".parse().unwrap());
        assert_eq!(ClientKind::from_headers(&h), ClientKind::Htmx);
    }
}
