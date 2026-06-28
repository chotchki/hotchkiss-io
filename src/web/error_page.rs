use askama::Template;
use axum::response::{Html, IntoResponse, Response};
use http::{HeaderMap, StatusCode};

/// A self-contained, on-brand error page. It does NOT extend `base.html`: the auth
/// middleware and `AppError` have no DB handle to build `base.html`'s `TopBar`
/// nav, so this reuses the site CSS + colors instead. Shared by the styled 403 +
/// 500 (the 404 keeps its own richer cat page, which does have the pool).
#[derive(Template)]
#[template(path = "error_page.html")]
pub struct ErrorPageTemplate {
    pub icon: String,
    pub heading: String,
    pub subtext: String,
    pub link_href: String,
    pub link_label: String,
    /// Shown only on the 500 so chris can correlate a report to the log line.
    pub trace_id: Option<String>,
}

impl ErrorPageTemplate {
    /// Render at `status`; a template failure degrades to the bare heading.
    pub fn into_response_with(self, status: StatusCode) -> Response {
        match self.render() {
            Ok(html) => (status, Html(html)).into_response(),
            Err(_) => (status, self.heading).into_response(),
        }
    }
}

/// The styled 500 ("Oops — I tripped over the cable"), KEEPING the trace id
/// visible for support. Shared by `AppError` (a bubbled error) and the router's
/// `CatchPanicLayer` (a handler panic) so both look identical.
pub fn server_error_response(trace_id: Option<String>) -> Response {
    ErrorPageTemplate {
        icon: "fa-solid fa-plug-circle-xmark".to_string(),
        heading: "Oops — I tripped over the cable".to_string(),
        subtext: "Something broke on my end. If it keeps happening, send me this trace id."
            .to_string(),
        link_href: "/".to_string(),
        link_label: "Back home".to_string(),
        trace_id,
    }
    .into_response_with(StatusCode::INTERNAL_SERVER_ERROR)
}

/// The styled 403 ("How about NO!"). An HTMX request — a mutation fired after the
/// session died (e.g. a beta redeploy) — instead gets an `HX-Redirect` to /login:
/// returning a full HTML document would get swapped into a fragment target. A
/// real full-page navigation gets the page.
pub fn forbidden_response(headers: &HeaderMap) -> Response {
    if headers.contains_key("HX-Request") {
        return (StatusCode::FORBIDDEN, [("HX-Redirect", "/login")]).into_response();
    }
    ErrorPageTemplate {
        icon: "fa-solid fa-hand".to_string(),
        heading: "How about NO!".to_string(),
        subtext: "You aren't authorized to do that. Log in if it's you.".to_string(),
        link_href: "/login".to_string(),
        link_label: "Log in".to_string(),
        trace_id: None,
    }
    .into_response_with(StatusCode::FORBIDDEN)
}
