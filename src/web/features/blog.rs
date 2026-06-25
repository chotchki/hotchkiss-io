use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError, app_state::AppState, authentication_state::AuthenticationState,
        features::{
            pages::{EditQuery, GetPageTemplate},
            top_bar::TopBar,
        },
        html_template::HtmlTemplate, markdown::{excerpt::excerpt, transformer::transform},
        session::SessionData,
    },
};
use anyhow::anyhow;
use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use sqlx::types::chrono::{DateTime, Utc};

pub fn blog_router() -> Router<AppState> {
    Router::new()
        .route("/", get(show_index))
        .route("/feed.xml", get(show_feed))
        .route("/{slug}", get(show_post))
}

pub struct BlogPostCard {
    pub page_name: String,
    pub title: String,
    pub page_creation_date: String,
    pub page_cover_attachment_id: Option<i64>,
    pub excerpt: String,
}

#[derive(Template)]
#[template(path = "blog/index.html")]
pub struct BlogIndexTemplate {
    pub top_bar: TopBar,
    pub auth_state: AuthenticationState,
    pub posts: Vec<BlogPostCard>,
}

pub async fn show_index(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let blog_page = ContentPageDao::find_by_name(&state.pool, None, "blog").await?;
    let Some(blog_page) = blog_page else {
        return Err(
            anyhow!("Server misconfiguration, could not find the `blog` special page").into(),
        );
    };

    let raw_posts =
        ContentPageDao::find_by_parent_newest_first(&state.pool, Some(blog_page.page_id), None)
            .await?;

    let posts: Vec<BlogPostCard> = raw_posts
        .into_iter()
        .map(|p| BlogPostCard {
            title: p.display_title(),
            page_name: p.page_name,
            page_creation_date: p.page_creation_date.format("%B %-d, %Y").to_string(),
            page_cover_attachment_id: p.page_cover_attachment_id,
            excerpt: excerpt(&p.page_markdown),
        })
        .collect();

    let template = BlogIndexTemplate {
        top_bar: TopBar::create(&state.pool, "blog").await?,
        auth_state: session_data.auth_state,
        posts,
    };
    Ok(HtmlTemplate(template).into_response())
}

pub async fn show_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let blog_page = ContentPageDao::find_by_name(&state.pool, None, "blog").await?;
    let Some(blog_page) = blog_page else {
        return Err(
            anyhow!("Server misconfiguration, could not find the `blog` special page").into(),
        );
    };

    let posts = ContentPageDao::find_by_parent_newest_first(
        &state.pool,
        Some(blog_page.page_id),
        Some(50),
    )
    .await?;

    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost");
    let scheme = if cfg!(debug_assertions) { "http" } else { "https" };
    let base = format!("{scheme}://{host}");

    let updated = posts
        .iter()
        .map(|p| p.page_modified_date)
        .max()
        .unwrap_or_else(Utc::now);

    let xml = render_atom(&base, &posts, updated)?;
    Ok((
        [(header::CONTENT_TYPE, "application/atom+xml; charset=utf-8")],
        xml,
    )
        .into_response())
}

fn render_atom(
    base: &str,
    posts: &[ContentPageDao],
    updated: DateTime<Utc>,
) -> anyhow::Result<String> {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str("  <title>Hotchkiss-io Blog</title>\n");
    out.push_str(&format!("  <link href=\"{base}/blog\"/>\n"));
    out.push_str(&format!(
        "  <link rel=\"self\" href=\"{base}/blog/feed.xml\"/>\n"
    ));
    out.push_str(&format!("  <id>{base}/blog</id>\n"));
    out.push_str(&format!("  <updated>{}</updated>\n", updated.to_rfc3339()));
    out.push_str("  <author><name>Christopher Hotchkiss</name></author>\n");
    for p in posts {
        out.push_str("  <entry>\n");
        out.push_str(&format!(
            "    <title>{}</title>\n",
            escape_xml(&p.display_title())
        ));
        out.push_str(&format!("    <link href=\"{base}/blog/{}\"/>\n", p.page_name));
        out.push_str(&format!("    <id>{base}/blog/{}</id>\n", p.page_name));
        out.push_str(&format!(
            "    <published>{}</published>\n",
            p.page_creation_date.to_rfc3339()
        ));
        out.push_str(&format!(
            "    <updated>{}</updated>\n",
            p.page_modified_date.to_rfc3339()
        ));
        let summary = excerpt(&p.page_markdown);
        if !summary.is_empty() {
            out.push_str(&format!(
                "    <summary>{}</summary>\n",
                escape_xml(&summary)
            ));
        }
        let html = transform(&p.page_markdown).unwrap_or_default();
        out.push_str(&format!(
            "    <content type=\"html\">{}</content>\n",
            escape_xml(&html)
        ));
        out.push_str("  </entry>\n");
    }
    out.push_str("</feed>\n");
    Ok(out)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub async fn show_post(
    State(state): State<AppState>,
    session_data: SessionData,
    Path(slug): Path<String>,
    Query(edit_q): Query<EditQuery>,
) -> Result<Response, AppError> {
    let pages_path = ContentPageDao::find_by_path(&state.pool, &["blog", &slug]).await?;

    let Some(lp) = pages_path.last() else {
        return Ok((StatusCode::NOT_FOUND, "No such post").into_response());
    };

    let gpt = GetPageTemplate {
        top_bar: TopBar::create(&state.pool, "blog").await?,
        auth_state: session_data.auth_state,
        page_path: format!("blog/{slug}"),
        page: lp.clone(),
        pages_path: pages_path.clone(),
        children_pages: ContentPageDao::find_by_parent(&state.pool, Some(lp.page_id)).await?,
        rendered_markdown: transform(&lp.page_markdown)?,
        edit: edit_q.edit.is_some(),
    };
    Ok(HtmlTemplate(gpt).into_response())
}
