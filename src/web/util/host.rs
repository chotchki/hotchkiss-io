//! Resolve the request's host robustly across HTTP/1.1 and HTTP/2.

use axum::http::{HeaderMap, Uri, header};

/// The request's host (with port if non-default). HTTP/1.1 carries it in the
/// `Host` header; **HTTP/2 carries it as the `:authority` pseudo-header**, which
/// hyper surfaces on the request URI — there is NO `Host` header on an h2
/// request. Checking both is required since enabling h2 (v0.0.69): otherwise an
/// h2 request (every real browser/crawler now) falls through to `localhost`,
/// which is exactly what broke the sitemap/robots/feed. Falls back to
/// `localhost` only when neither is present (dev / tests).
pub fn request_host(headers: &HeaderMap, uri: &Uri) -> String {
    headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .or_else(|| uri.authority().map(|a| a.as_str().to_string()))
        .unwrap_or_else(|| "localhost".to_string())
}

/// Scheme for absolute URLs: plain HTTP only in debug (test harness / dev); prod
/// + beta are HTTPS.
pub fn request_scheme() -> &'static str {
    if cfg!(debug_assertions) { "http" } else { "https" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(host: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = host {
            h.insert(header::HOST, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn host_header_used_for_http11() {
        let uri: Uri = "/sitemap.xml".parse().unwrap();
        assert_eq!(
            request_host(&hdrs(Some("hotchkiss.io")), &uri),
            "hotchkiss.io"
        );
    }

    #[test]
    fn falls_back_to_authority_for_http2() {
        // No Host header (h2); the host is the URI's :authority (with port).
        let uri: Uri = "https://beta.hotchkiss.io:8443/sitemap.xml".parse().unwrap();
        assert_eq!(request_host(&hdrs(None), &uri), "beta.hotchkiss.io:8443");
    }

    #[test]
    fn neither_present_is_localhost() {
        let uri: Uri = "/sitemap.xml".parse().unwrap();
        assert_eq!(request_host(&hdrs(None), &uri), "localhost");
    }
}
