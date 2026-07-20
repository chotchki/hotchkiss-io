//! SEO discovery endpoints — a dynamic `sitemap.xml` and `robots.txt`.
//!
//! Both are HOST-aware (built from the request's `Host`), which does two things:
//! the sitemap's `<loc>`s are same-origin as the sitemap itself (a hard Google
//! requirement), and `robots.txt` can DE-INDEX the non-canonical beta host so
//! `beta.hotchkiss.io` doesn't compete with prod for the same content
//! (duplicate-content is exactly the kind of thing that suppresses crawling).

use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError,
        app_state::AppState,
        markdown::render_cache::cached_excerpt,
        util::host::{request_host, request_scheme},
    },
};
use axum::{
    extract::State,
    http::{HeaderMap, Uri, header},
    response::{IntoResponse, Response},
};
use sqlx::types::chrono::{DateTime, Utc};

/// Fallback page description when a page has no excerpt-able body (the site
/// tagline). Mirrors the jumbotron line in `base.html`.
pub const DEFAULT_DESCRIPTION: &str =
    "Christopher Hotchkiss — crafting solutions, shaping products: from concept to code.";

/// Per-page SEO/social metadata rendered into `<head>` by `partials/seo_meta.html`
/// (the `{% block meta %}` override on content templates). All URLs are absolute
/// and CANONICAL — built from the registrable `site_host` (`hotchkiss.io`), NOT
/// the served host, so a beta page's canonical/OG URLs point at prod (dedupe +
/// the shared link preview).
pub struct Meta {
    pub title: String,
    pub description: String,
    pub canonical_url: String,
    pub og_image: String,
    pub og_type: &'static str,
}

impl Meta {
    /// Meta for a single content page: description from the markdown excerpt
    /// (falling back to the site tagline), image from the page cover when present
    /// (else the site photo). `canonical_path` is the page's path WITHOUT a
    /// leading slash (e.g. `blog/my-post`, `pages/about`, `resume`).
    pub fn page(
        site_host: &str,
        title: String,
        markdown: &str,
        canonical_path: &str,
        cover_url: Option<&str>,
        og_type: &'static str,
    ) -> Self {
        let mut description = cached_excerpt(markdown);
        if description.trim().is_empty() {
            description = DEFAULT_DESCRIPTION.to_string();
        }
        Self::raw(site_host, title, description, canonical_path, cover_url, og_type)
    }

    /// Meta for an index/section page: an explicit description, the site photo as
    /// the social image, `og:type=website`.
    pub fn section(
        site_host: &str,
        title: String,
        description: String,
        canonical_path: &str,
    ) -> Self {
        Self::raw(site_host, title, description, canonical_path, None, "website")
    }

    fn raw(
        site_host: &str,
        title: String,
        description: String,
        canonical_path: &str,
        cover_url: Option<&str>,
        og_type: &'static str,
    ) -> Self {
        let canonical_url = format!("https://{site_host}/{}", canonical_path.trim_start_matches('/'));
        let og_image = match cover_url {
            // cover_url is root-relative ("/media/file/<key>") — make it absolute.
            Some(p) => format!("https://{site_host}{p}"),
            None => format!("https://{site_host}/images/Photo.avif"),
        };
        Meta {
            title,
            description,
            canonical_url,
            og_image,
            og_type,
        }
    }
}

/// `GET /sitemap.xml` — every crawlable URL (home, top-level pages, the blog +
/// project indexes and their children, the résumé). `<lastmod>` comes from each
/// page's `page_modified_date`, so a crawler can tell what actually changed.
pub async fn sitemap_xml(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, AppError> {
    let base = format!(
        "{}://{}",
        request_scheme(),
        request_host(&headers, &uri)
    );

    // (absolute loc, optional lastmod)
    let mut urls: Vec<(String, Option<DateTime<Utc>>)> = vec![(format!("{base}/"), None)];

    let top = ContentPageDao::find_by_parent(&state.pool, None).await?;
    let mut blog_id = None;
    let mut projects_id = None;
    for p in &top {
        if p.special_page {
            // A special row carrying a min_role is a gated SECTION — skip it
            // like any other hidden page (DA; also drops its children below,
            // since blog_id/projects_id stay None), else the sessionless
            // sitemap would leak the gated section's URL to every crawler.
            if !p.is_visible_to(crate::db::dao::roles::Role::Anonymous) {
                continue;
            }
            // Special pages are routing redirects, not `/pages/<slug>` content —
            // map the known ones to their real routes; skip `login` + unknowns.
            match p.page_name.as_str() {
                "blog" => {
                    blog_id = Some(p.page_id);
                    urls.push((format!("{base}/blog"), Some(p.page_modified_date)));
                }
                "projects" => {
                    projects_id = Some(p.page_id);
                    urls.push((format!("{base}/projects"), Some(p.page_modified_date)));
                }
                "resume" => urls.push((format!("{base}/resume"), Some(p.page_modified_date))),
                _ => {}
            }
        } else if p.is_visible_to(crate::db::dao::roles::Role::Anonymous) {
            // Skip future-dated (scheduled) pages — never leak an unpublished URL
            // to crawlers (the sitemap is unconditional, no session). Phase CU.
            urls.push((
                format!("{base}/pages/{}", p.page_name),
                Some(p.page_modified_date),
            ));
        }
    }
    if let Some(id) = blog_id {
        for c in ContentPageDao::find_by_parent_newest_first(&state.pool, Some(id), None).await? {
            if c.is_visible_to(crate::db::dao::roles::Role::Anonymous) {
                urls.push((
                    format!("{base}/blog/{}", c.page_name),
                    Some(c.page_modified_date),
                ));
            }
        }
    }
    if let Some(id) = projects_id {
        // Project DETAIL pages are content-tree pages at `/pages/projects/<slug>`
        // (the `/projects` route is the index only) — NOT `/projects/<slug>`.
        for c in ContentPageDao::find_by_parent(&state.pool, Some(id)).await? {
            if c.is_visible_to(crate::db::dao::roles::Role::Anonymous) {
                urls.push((
                    format!("{base}/pages/projects/{}", c.page_name),
                    Some(c.page_modified_date),
                ));
            }
        }
    }

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for (loc, lastmod) in &urls {
        out.push_str("  <url>\n");
        out.push_str(&format!("    <loc>{}</loc>\n", loc.replace('&', "&amp;")));
        if let Some(m) = lastmod {
            out.push_str(&format!("    <lastmod>{}</lastmod>\n", m.format("%Y-%m-%d")));
        }
        out.push_str("  </url>\n");
    }
    out.push_str("</urlset>\n");

    Ok((
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        out,
    )
        .into_response())
}

/// `GET /robots.txt` — host-aware. On the canonical host (prod) it allows
/// crawling, hides the admin/login surfaces, and points at the sitemap. On any
/// OTHER host (beta) it disallows everything, so the ephemeral beta copy is never
/// indexed against prod.
pub async fn robots_txt(State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let scheme = request_scheme();
    let host = request_host(&headers, &uri);

    // Canonical = the registrable site host (`hotchkiss.io`), its www variant, or
    // localhost (dev/tests). Anything else (beta.hotchkiss.io) is non-canonical.
    // Shared with the icon routes (EB.8) so the two hosts' identities can't drift.
    let canonical = crate::web::util::host::is_canonical_host(&host, &state.site_host);

    let body = if canonical {
        format!(
            "User-agent: *\n\
             Allow: /\n\
             Disallow: /admin/\n\
             Disallow: /login/\n\
             \n\
             Sitemap: {scheme}://{host}/sitemap.xml\n"
        )
    } else {
        "User-agent: *\nDisallow: /\n".to_string()
    };

    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}
