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
        markdown::{
            render_cache::{cached_excerpt, cached_transform},
            title::strip_leading_h1,
        },
    },
};
use crate::web::util::host::{request_host, request_scheme};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use openssl::sha::sha256;
use sqlx::types::chrono::{DateTime, NaiveDateTime, Utc};

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
///
/// Conditional-request aware: the validator (below) is derived from the CHEAP
/// entry fetch, so a repeat crawler that echoes `If-None-Match`/`If-Modified-Since`
/// gets a `304` WITHOUT the expensive per-entry markdown transform ever running.
/// The transforms that DO run on a `200` are content-cached (`render_cache`), so a
/// warm feed is near-free either way — this just also saves the body+bandwidth.
pub async fn show_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, AppError> {
    let mut entries = collect_entries(&state).await?;

    let host = request_host(&headers, &uri);
    // Validator inputs: `updated` (max modified_date) moves on ANY edit — every
    // save stamps page_modified_date via update()/set_cover; `count` catches an
    // add/delete of a NON-newest entry (which wouldn't move max); `host` is folded
    // in because the body's absolute URLs differ per host. (Feed order is by
    // creation date, so a reorder — which doesn't touch modified_date — correctly
    // does NOT invalidate.)
    let updated = entries.iter().map(|e| e.page.page_modified_date).max();
    let etag = feed_etag(&host, updated, entries.len());
    let last_modified = updated.map(httpdate);

    if not_modified(&headers, &etag, updated) {
        return Ok(conditional_304(&etag, last_modified.as_deref()));
    }

    // Newest first by creation date; tiebreak page_id DESC — same total order the
    // blog index uses, so a same-second pair sorts deterministically.
    entries.sort_by(|a, b| {
        b.page
            .page_creation_date
            .cmp(&a.page.page_creation_date)
            .then(b.page.page_id.cmp(&a.page.page_id))
    });

    let base = format!("{}://{}", request_scheme(), host);
    let xml = render_atom(&base, &entries, updated.unwrap_or_else(Utc::now))?;

    let mut resp = (
        [(header::CONTENT_TYPE, "application/atom+xml; charset=utf-8")],
        xml,
    )
        .into_response();
    set_validators(resp.headers_mut(), &etag, last_modified.as_deref());
    Ok(resp)
}

/// Weak ETag over the feed's validator inputs (host + latest edit + entry count).
/// Weak (`W/`) because the body is semantically — not necessarily byte — stable
/// for a given tuple (e.g. the `<updated>` fallback on an empty feed uses `now()`);
/// any real content change moves one of the three inputs. Same 128-bit-hex content
/// hash the rest of the codebase uses.
fn feed_etag(host: &str, updated: Option<DateTime<Utc>>, count: usize) -> String {
    let stamp = updated.map(|d| d.timestamp_millis()).unwrap_or(0);
    let digest = sha256(format!("{host}\u{0}{stamp}\u{0}{count}").as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("W/\"{hex}\"")
}

/// HTTP-date (IMF-fixdate) for `Last-Modified`, e.g. `Sun, 01 Jul 2026 06:31:30
/// GMT`. This is the format modern clients echo back in `If-Modified-Since`.
fn httpdate(d: DateTime<Utc>) -> String {
    d.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

fn parse_httpdate(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s.trim(), "%a, %d %b %Y %H:%M:%S GMT")
        .ok()
        .map(|n| n.and_utc())
}

/// Is the client's cached copy still current? `If-None-Match` takes precedence
/// over `If-Modified-Since` (RFC 7232 §3.3). An unparseable date → not a match
/// (fall through to a full `200`), never an error.
fn not_modified(headers: &HeaderMap, etag: &str, updated: Option<DateTime<Utc>>) -> bool {
    if let Some(inm) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        return inm.split(',').map(str::trim).any(|t| t == "*" || t == etag);
    }
    if let (Some(ims), Some(updated)) = (
        headers
            .get(header::IF_MODIFIED_SINCE)
            .and_then(|v| v.to_str().ok()),
        updated,
    ) && let Some(since) = parse_httpdate(ims)
    {
        // Our timestamp truncated to whole seconds — the echoed HTTP-date carries
        // no sub-second precision.
        return updated.timestamp() <= since.timestamp();
    }
    false
}

/// Attach the ETag + Last-Modified validators to a response.
fn set_validators(h: &mut HeaderMap, etag: &str, last_modified: Option<&str>) {
    h.insert(header::ETAG, etag.parse().expect("etag is a valid header value"));
    if let Some(lm) = last_modified {
        h.insert(
            header::LAST_MODIFIED,
            lm.parse().expect("httpdate is a valid header value"),
        );
    }
}

/// A `304 Not Modified` carrying the current validators (per RFC 7232 a 304 SHOULD
/// echo the ETag/Last-Modified), no body — the expensive render is skipped.
fn conditional_304(etag: &str, last_modified: Option<&str>) -> Response {
    let mut resp = StatusCode::NOT_MODIFIED.into_response();
    set_validators(resp.headers_mut(), etag, last_modified);
    resp
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
        let summary = cached_excerpt(&p.page_markdown);
        if !summary.is_empty() {
            out.push_str(&format!("    <summary>{}</summary>\n", escape_xml(&summary)));
        }
        let html = cached_transform(&strip_leading_h1(&p.page_markdown)).unwrap_or_default();
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
