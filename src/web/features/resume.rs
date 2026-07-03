//! `/resume` — the résumé page + its generated PDF.
//!
//! `/resume` renders the newest child of the `resume` special page (chris authors
//! the résumé into that child at `/resume?edit`). `/resume.pdf` renders that SAME
//! markdown to HTML, wraps it in a print stylesheet and pipes it through
//! `weasyprint` — so the PDF DERIVES from the one source, no drift. The binary is
//! resolved like d2 (`$WEASYPRINT_BIN` → brew → PATH). A missing/broken binary is
//! logged + surfaced as a 500 via `AppError` (a PDF download can't show an inline
//! error block the way the diagram swap can).

use anyhow::anyhow;
use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use openssl::sha::sha256;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};

use crate::db::dao::content_pages::ContentPageDao;
use crate::web::app_error::AppError;
use crate::web::app_state::AppState;
use crate::web::features::pages::{EditQuery, GetPageTemplate};
use crate::web::features::top_bar::TopBar;
use crate::web::html_template::HtmlTemplate;
use crate::web::markdown::render_cache::cached_transform;
use crate::web::markdown::title::strip_leading_h1;
use crate::web::session::SessionData;

/// Résumé-specific print stylesheet (NOT Tailwind — weasyprint styles the
/// semantic HTML directly). Single source: the same markdown renders the web view
/// AND, through this CSS, the PDF. `include_str!` makes it a compile-time dep, so
/// edits trigger a rebuild.
const RESUME_PRINT_CSS: &str = include_str!("resume-print.css");

pub fn resume_routes() -> Router<AppState> {
    Router::new()
        .route("/resume", get(show_resume))
        .route("/resume.pdf", get(show_resume_pdf))
}

/// The résumé content lives in the newest child of the `resume` special page.
/// The newest child of the `resume` special page the viewer may see. Non-admins
/// get the newest PUBLISHED child; a scheduled/draft résumé sitting in front of a
/// published one is skipped — dropping the old `LIMIT 1`, since a naive gate on the
/// single newest row would 404 all of `/resume` whenever a future draft exists.
async fn newest_resume_child(pool: &SqlitePool, is_admin: bool) -> Result<Option<ContentPageDao>> {
    let resume = ContentPageDao::find_by_name(pool, None, "resume")
        .await?
        .ok_or_else(|| anyhow!("Server misconfiguration: `resume` special page missing"))?;
    let children =
        ContentPageDao::find_by_parent_newest_first(pool, Some(resume.page_id), None).await?;
    Ok(children.into_iter().find(|c| c.is_visible_to(is_admin)))
}

pub async fn show_resume(
    State(state): State<AppState>,
    session_data: SessionData,
    Query(edit_q): Query<EditQuery>,
) -> Result<Response, AppError> {
    let is_admin = session_data.auth_state.is_admin();
    let Some(child) = newest_resume_child(&state.pool, is_admin).await? else {
        return Ok((StatusCode::NOT_FOUND, "Résumé not published yet").into_response());
    };

    let meta = crate::web::features::seo::Meta::page(
        &state.site_host,
        child.display_title(),
        &child.page_markdown,
        "resume",
        None,
        "website",
    );

    let gpt = GetPageTemplate {
        top_bar: TopBar::create(&state.pool, "resume").await?,
        auth_state: session_data.auth_state,
        page_path: format!("resume/{}", child.page_name),
        page: child.clone(),
        pages_path: vec![child.clone()],
        children_pages: ContentPageDao::find_by_parent(&state.pool, Some(child.page_id)).await?,
        rendered_markdown: cached_transform(&strip_leading_h1(&child.page_markdown))?,
        edit: edit_q.edit.is_some(),
        prev_post: None,
        next_post: None,
        pdf_url: Some("/resume.pdf".to_string()),
        cover_media_ref: crate::web::features::media::cover_ref_for(&state.pool, child.page_id)
            .await,
        meta,
        posted_date: None,
        // The résumé has no cover hero (Phase CV).
        hero: None,
    };
    Ok(HtmlTemplate(gpt).into_response())
}

pub async fn show_resume_pdf(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    let Some(child) = newest_resume_child(&state.pool, session_data.auth_state.is_admin()).await?
    else {
        return Ok((StatusCode::NOT_FOUND, "Résumé not published yet").into_response());
    };
    let pdf = render_resume_pdf(&child, &state.site_host)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"Christopher-Hotchkiss-Resume.pdf\"",
            ),
        ],
        pdf,
    )
        .into_response())
}

/// hash(markdown) -> rendered PDF bytes. The PDF derives from the markdown, so the
/// content hash is a safe cache key (regenerates when the résumé changes).
static PDF_CACHE: LazyLock<Mutex<HashMap<String, Vec<u8>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn render_resume_pdf(child: &ContentPageDao, site_host: &str) -> Result<Vec<u8>> {
    // Key the cache on the ACTUAL PDF inputs (title + body), NOT markdown alone:
    // editing the title (the `page_title` column) leaves `page_markdown` unchanged,
    // so a markdown-only key kept serving a stale PDF after a title edit.
    let title = child.display_title();
    let hash = pdf_cache_key(&title, &child.page_markdown);
    if let Some(hit) = PDF_CACHE
        .lock()
        .expect("resume pdf cache poisoned")
        .get(&hash)
    {
        return Ok(hit.clone());
    }
    let body = cached_transform(&strip_leading_h1(&child.page_markdown))?;
    let title = html_escape(&title);
    let html = resume_html(&title, &body, site_host);
    let pdf = weasyprint(&html)?;
    PDF_CACHE
        .lock()
        .expect("resume pdf cache poisoned")
        .insert(hash, pdf.clone());
    Ok(pdf)
}

/// Assemble the print HTML. The `<base href>` is the canonical PUBLIC site —
/// `site_host` is the WebAuthn rp-id (`hotchkiss.io` on BOTH prod and beta, NOT
/// the served domain) — so the root-relative links `rewrite_site_links` stores
/// resolve to ABSOLUTE, clickable URLs in the downloaded PDF. Without it
/// weasyprint emits dead `file:///path` link annotations (verified). Using the
/// rp-id host (not the served domain) means a résumé downloaded from beta still
/// points recruiters at the real `hotchkiss.io`, never ephemeral beta. The same
/// base lets a root-relative image src resolve against the public site too.
fn resume_html(title_escaped: &str, body: &str, site_host: &str) -> String {
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<base href=\"https://{site_host}/\"><title>{title_escaped}</title>\
<style>{RESUME_PRINT_CSS}</style></head>\
<body><h1 class=\"resume-name\">{title_escaped}</h1>{body}</body></html>"
    )
}

/// Resolved once: `$WEASYPRINT_BIN`, then brew locations, then PATH (the mini's
/// LaunchAgent PATH excludes /opt/homebrew/bin, so a bare name can't be relied on).
static WEASYPRINT_BIN: LazyLock<Option<String>> = LazyLock::new(resolve_weasyprint_bin);

fn resolve_weasyprint_bin() -> Option<String> {
    if let Ok(p) = std::env::var("WEASYPRINT_BIN")
        && !p.is_empty()
    {
        return Some(p);
    }
    for cand in ["/opt/homebrew/bin/weasyprint", "/usr/local/bin/weasyprint"] {
        if Path::new(cand).is_file() {
            return Some(cand.to_string());
        }
    }
    let on_path = Command::new("weasyprint")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    on_path.then(|| "weasyprint".to_string())
}

/// HTML (string) -> PDF (bytes): `weasyprint - -` reads HTML from stdin, writes
/// the PDF to stdout.
fn weasyprint(html: &str) -> Result<Vec<u8>> {
    let bin = WEASYPRINT_BIN.as_deref().ok_or_else(|| {
        anyhow!("weasyprint not found — `brew install weasyprint` (looked at $WEASYPRINT_BIN, /opt/homebrew/bin, /usr/local/bin, PATH)")
    })?;
    let mut child = Command::new(bin)
        .arg("-")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("failed to spawn weasyprint ({bin}): {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("weasyprint stdin unavailable"))?;
        stdin.write_all(html.as_bytes())?;
    } // drop stdin -> EOF so weasyprint starts
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "weasyprint failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if out.stdout.is_empty() {
        return Err(anyhow!("weasyprint produced no output"));
    }
    Ok(out.stdout)
}

/// Content hash of the markdown: SHA-256 truncated to 128 bits, hex.
fn content_hash(source: &str) -> String {
    let digest = sha256(source.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Cache key for the rendered PDF — over the ACTUAL inputs (title + body) so a
/// title-only edit busts it (markdown alone wouldn't change). NUL-joined so a
/// shift of the title/body boundary can't collide.
fn pdf_cache_key(title: &str, markdown: &str) -> String {
    content_hash(&format!("{title}\u{0}{markdown}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_cache_key_busts_on_title_or_body_change() {
        let md = "# Name\n\nthe body";
        // The bug: a title-only edit (markdown unchanged) must change the key.
        assert_ne!(
            pdf_cache_key("Old Title", md),
            pdf_cache_key("New Title", md),
            "a title-only change must invalidate the PDF cache"
        );
        // Body-only change too.
        assert_ne!(pdf_cache_key("T", "a"), pdf_cache_key("T", "b"));
        // Identical inputs -> same key (so the cache actually hits).
        assert_eq!(pdf_cache_key("T", md), pdf_cache_key("T", md));
    }

    #[test]
    fn resume_html_sets_canonical_base() {
        // The <base> is what makes weasyprint resolve the root-relative links
        // `rewrite_site_links` stores into absolute, clickable PDF URLs (verified:
        // without it they become dead `file:///path`). It must be the rp-id host
        // (canonical, public) — not the served domain — so a beta-generated PDF
        // still points at hotchkiss.io.
        let html = resume_html("Chris", "<p><a href=\"/projects/x\">x</a></p>", "hotchkiss.io");
        assert!(
            html.contains("<base href=\"https://hotchkiss.io/\">"),
            "PDF HTML must carry the canonical base: {html}"
        );
    }
}
