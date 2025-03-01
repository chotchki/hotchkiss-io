use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderName, HeaderValue};

pub fn htmx_refresh() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("hx-refresh"),
        HeaderValue::from_static("true"),
    );
    headers.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct() {
        let r = htmx_refresh();

        assert!(r.headers().contains_key("hx-refresh"));
    }
}
