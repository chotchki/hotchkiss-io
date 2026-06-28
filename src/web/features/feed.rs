//! The site's Atom feed — ONE feed carrying both blog posts and project pages,
//! newest-first. Lives at `/feed.xml` (canonical) and `/blog/feed.xml` (kept for
//! back-compat with existing subscribers + the `<link rel="alternate">` history).
//!
//! Projects aren't chronological the way posts are, but they DO carry a
//! creation/modified date, so a unified newest-first ordering is well-defined and
//! a recruiter watching the feed sees new project pages land alongside posts.

use crate::{
    db::dao::content_pages::ContentPageDao,
    web::{
        app_error::AppError,
        app_state::AppState,
        markdown::{excerpt::excerpt, title::strip_leading_h1, transformer::transform},
    },
};
use crate::web::util::host::{request_host, request_scheme};
use axum::{
    extract::State,
    http::{HeaderMap, Uri, header},
    response::{IntoResponse, Response},
};
use sqlx::types::chrono::{DateTime, Utc};

/// How many entries per section feed the feed. Generous — the site is small.
const PER_SECTION_LIMIT: i64 = 50;

/// One feed entry plus the URL section it links into (`blog` → `/blog/<slug>`,
/// `projects` → `/pages/projects/<slug>` — project detail pages live UNDER the
/// `/pages` tree, not at `/projects/<slug>`).
struct FeedEntry {
    section: &'static str,
    page: ContentPageDao,
}

/// The site path a section's entry links to. Blog posts have a dedicated
/// `/blog/<slug>` route; project detail pages are content-tree pages served at
/// `/pages/projects/<slug>` (the `/projects` route is the index only).
fn entry_path(section: &str, slug: &str) -> String {
    match section {
        "projects" => format!("pages/projects/{slug}"),
        _ => format!("{section}/{slug}"),
    }
}

/// `GET /feed.xml` (and `/blog/feed.xml`) — the unified Atom feed.
pub async fn show_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, AppError> {
    let mut entries = collect_entries(&state).await?;

    // Newest first by creation date; tiebreak page_id DESC — same total order the
    // blog index uses, so a same-second pair sorts deterministically.
    entries.sort_by(|a, b| {
        b.page
            .page_creation_date
            .cmp(&a.page.page_creation_date)
            .then(b.page.page_id.cmp(&a.page.page_id))
    });

    let base = format!(
        "{}://{}",
        request_scheme(),
        request_host(&headers, &uri)
    );

    let updated = entries
        .iter()
        .map(|e| e.page.page_modified_date)
        .max()
        .unwrap_or_else(Utc::now);

    let xml = render_atom(&base, &entries, updated)?;
    Ok((
        [(header::CONTENT_TYPE, "application/atom+xml; charset=utf-8")],
        xml,
    )
        .into_response())
}

/// Pull the children of the `blog` and `projects` special pages, tagged with
/// their URL section. A missing special page is skipped (not fatal) — the feed
/// degrades to whatever sections exist.
async fn collect_entries(state: &AppState) -> Result<Vec<FeedEntry>, AppError> {
    let mut entries: Vec<FeedEntry> = Vec::new();
    for section in ["blog", "projects"] {
        if let Some(parent) = ContentPageDao::find_by_name(&state.pool, None, section).await? {
            let children = ContentPageDao::find_by_parent_newest_first(
                &state.pool,
                Some(parent.page_id),
                Some(PER_SECTION_LIMIT),
            )
            .await?;
            for page in children {
                entries.push(FeedEntry { section, page });
            }
        }
    }
    Ok(entries)
}

fn render_atom(
    base: &str,
    entries: &[FeedEntry],
    updated: DateTime<Utc>,
) -> anyhow::Result<String> {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<feed xmlns=\"http://www.w3.org/2005/Atom\">\n");
    out.push_str("  <title>Christopher Hotchkiss</title>\n");
    out.push_str("  <subtitle>Blog posts and projects from hotchkiss.io</subtitle>\n");
    out.push_str(&format!("  <link href=\"{base}/\"/>\n"));
    out.push_str(&format!("  <link rel=\"self\" href=\"{base}/feed.xml\"/>\n"));
    out.push_str(&format!("  <id>{base}/</id>\n"));
    out.push_str(&format!("  <updated>{}</updated>\n", updated.to_rfc3339()));
    out.push_str("  <author><name>Christopher Hotchkiss</name></author>\n");
    for e in entries {
        let p = &e.page;
        let url = format!("{base}/{}", entry_path(e.section, &p.page_name));
        out.push_str("  <entry>\n");
        out.push_str(&format!(
            "    <title>{}</title>\n",
            escape_xml(&p.display_title())
        ));
        out.push_str(&format!("    <link href=\"{url}\"/>\n"));
        out.push_str(&format!("    <id>{url}</id>\n"));
        // Mark which section an entry is — lets a reader/filter tell a project
        // page from a blog post.
        out.push_str(&format!(
            "    <category term=\"{}\"/>\n",
            escape_xml(e.section)
        ));
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
            out.push_str(&format!("    <summary>{}</summary>\n", escape_xml(&summary)));
        }
        let html = transform(&strip_leading_h1(&p.page_markdown)).unwrap_or_default();
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
