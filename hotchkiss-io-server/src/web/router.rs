use askama::Template;
use axum::{
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use reqwest::StatusCode;

pub fn create_router() -> Router {
    let mut router = Router::new().route("/", get(hello));

    //app = app.merge(authentication::router(
    //    self.pool.clone(),
    //    session_layer.clone(),
    //    webauthn,
    //));
    //app = app.merge(clients::router(self.pool.clone()));
    //app = app.merge(client_groups::router(self.pool.clone()));
    //app = app.merge(domains::router(self.pool.clone(), session_layer));
    //app = app.merge(domain_groups::router(self.pool.clone()));
    //app = app.merge(groups_applied::router(self.pool.clone()));
    //app = app.merge(health::router());
    //app = app.merge(setup::router(self.pool.clone()));

    //Only enable embedded static content if we're in release mode
    #[cfg(debug_assertions)]
    {
        router = router.merge(crate::web::dev_frontend::router());
    }
    #[cfg(not(debug_assertions))]
    {
        router = router.merge(crate::web::frontend::router());
    }

    router
}

async fn hello() -> impl IntoResponse {
    let template = HelloTemplate {};
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "hello.html")]
struct HelloTemplate;

/// A wrapper type that we'll use to encapsulate HTML parsed by askama into valid HTML for axum to serve.
struct HtmlTemplate<T>(T);

/// Allows us to convert Askama HTML templates into valid HTML for axum to serve in the response.
impl<T> IntoResponse for HtmlTemplate<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        // Attempt to render the template with askama
        match self.0.render() {
            // If we're able to successfully parse and aggregate the template, serve it
            Ok(html) => Html(html).into_response(),
            // If we're not, return an error or some bit of fallback HTML
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template. Error: {}", err),
            )
                .into_response(),
        }
    }
}
