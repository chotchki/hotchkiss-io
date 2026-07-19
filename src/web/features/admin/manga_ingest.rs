//! Bulk manga ingest (Phase DW) — the 271-volume ergonomics.
//!
//! A folder of `.epub`/`.cbz` files on the mini, or a browser multi-file drop, becomes
//! an ordered stack of volume pages under a series (`/library/manga/<series>`). Each
//! file → one content-addressed Epub/Cbz media item + one volume child page embedding
//! `![](/media/<ref>)`, ordered by the number parsed from its filename (DW.1). Both
//! formats are read by the same foliate reader (DW.8); most manga are CBZ, novels EPUB.
//!
//! The two front doors (filesystem DW.3, browser DW.4) only differ in how they get
//! the bytes onto disk; both funnel into the ONE ingest core (DW.2) that streams to
//! the store, mints the item, and creates the ordered page — so the item/page policy
//! (gate inheritance, dedup, the embed) can't drift between them.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use askama::Template;
use axum::extract::{Multipart, State};
use axum::response::{IntoResponse, Response};
use axum::Form;
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::io::AsyncReadExt;

use crate::db::dao::content_pages::ContentPageDao;
use crate::media::MediaStore;
use crate::web::app_error::AppError;
use crate::web::app_state::AppState;
use crate::web::authentication_state::AuthenticationState;
use crate::web::features::admin::media::ingest_stored_file;
use crate::web::features::pages::write::{create_page, update_page, PageUpdate, PageWriteError};
use crate::web::features::top_bar::TopBar;
use crate::web::html_template::HtmlTemplate;
use crate::web::session::SessionData;
use crate::web::util::slug::Slug;

/// The manga section title under `library` (DW.5) — a Family-gated section alongside
/// audiobooks, auto-created on the first ingest so a fresh install needs zero setup.
const MANGA_SECTION_TITLE: &str = "Manga";

/// A volume marker kind — drives the page title ("Volume 12" vs "Chapter 1"). Manga
/// EPUBs come both ways (a bound volume, or a single serialized chapter), and chris's
/// own library carries both, so the parse preserves which one the filename named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeLabel {
    Volume,
    Chapter,
}

impl VolumeLabel {
    fn as_str(self) -> &'static str {
        match self {
            VolumeLabel::Volume => "Volume",
            VolumeLabel::Chapter => "Chapter",
        }
    }
}

/// A volume number + display title parsed from an `.epub`'s filename (DW.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVolume {
    /// The number found in the filename, or `None` — then the caller falls back to
    /// the upload/lexical position so a NUMBER-less file still lands in a stable slot.
    pub number: Option<i64>,
    /// The volume page title: "Volume {n}" / "Chapter {n}" when a number was found,
    /// else the cleaned filename stem (an un-numbered one-off keeps its name).
    pub title: String,
}

/// Classify a leading letter-run as a volume/chapter marker (or neither). Exact
/// matches only — so a series name that merely STARTS with `c`/`v` ("Cover",
/// "Villain") is NOT a marker; only the tokens `c001` / `v12` / `Vol 5` are.
fn classify_marker(letters: &str) -> Option<VolumeLabel> {
    match letters {
        "volume" | "vol" | "v" => Some(VolumeLabel::Volume),
        "chapter" | "chap" | "ch" | "c" => Some(VolumeLabel::Chapter),
        _ => None,
    }
}

/// Extract the volume number + its label from a filename stem. Two-tier so a marker
/// beats a stray number (a year): a marker-attached / immediately-after-marker number
/// (`v12`, `Vol 12`, `c003`) WINS over a bare number; among bare numbers the LAST
/// wins (a trailing volume index beats a leading series index). No marker + a lone
/// bare number → treated as a Volume.
fn extract_number(stem: &str) -> Option<(i64, VolumeLabel)> {
    // Preferred: a number tied to a v/vol/chapter marker (attached or right after).
    let mut marked: Option<(i64, VolumeLabel)> = None;
    // Fallback: the last bare number (no marker) — a year / raw index.
    let mut bare: Option<i64> = None;
    // A bare marker word ("Vol", "Chapter") sets this; the NEXT pure-digit token binds.
    let mut pending: Option<VolumeLabel> = None;

    for tok in stem.split(|c: char| !c.is_ascii_alphanumeric()) {
        if tok.is_empty() {
            continue;
        }
        let lower = tok.to_ascii_lowercase();
        let letters: String = lower.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let digits: String = lower[letters.len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        // A clean token is exactly letters-then-digits; anything mixed after
        // (`s01e02`, `v12a`) is junk that also clears a pending marker.
        if letters.len() + digits.len() != lower.len() {
            pending = None;
            continue;
        }

        if digits.is_empty() {
            // Pure word: a marker word arms `pending`; a series-name word clears it.
            pending = classify_marker(&letters);
        } else if letters.is_empty() {
            // Pure digits: bind to a pending marker, else it's a bare number.
            if let Ok(n) = digits.parse::<i64>() {
                match pending.take() {
                    Some(label) => marked = Some((n, label)),
                    None => bare = Some(n),
                }
            }
        } else {
            // Attached letters+digits (`v12`, `c003`) — only a real marker counts.
            if let (Some(label), Ok(n)) = (classify_marker(&letters), digits.parse::<i64>()) {
                marked = Some((n, label));
            }
            pending = None;
        }
    }

    marked.or_else(|| bare.map(|n| (n, VolumeLabel::Volume)))
}

/// Drop a trailing `.epub`/`.cbz` (case-insensitive) for the title stem; leave any
/// other name untouched (keeps the fn total).
fn strip_book_ext(filename: &str) -> &str {
    for ext in [".epub", ".cbz"] {
        if filename.len() >= ext.len()
            && filename[filename.len() - ext.len()..].eq_ignore_ascii_case(ext)
        {
            return &filename[..filename.len() - ext.len()];
        }
    }
    filename
}

/// Parse a volume number + display title from a manga `.epub` filename (DW.1).
///
/// Handles the real shapes — `Series v012.epub`, `Series Vol.12`, `Series Volume 12`,
/// `#12`, `c003` (chapter), a bare trailing `12` — with a marker winning over a stray
/// year and the after-marker number winning over a later bare one. No number found →
/// `number: None` (the caller orders by position) and the cleaned stem as the title.
pub fn parse_volume(filename: &str) -> ParsedVolume {
    let stem = strip_book_ext(filename).trim();
    match extract_number(stem) {
        Some((n, label)) => ParsedVolume {
            number: Some(n),
            title: format!("{} {n}", label.as_str()),
        },
        None => {
            let title = if stem.is_empty() {
                "Untitled".to_string()
            } else {
                stem.to_string()
            };
            ParsedVolume { number: None, title }
        }
    }
}

/// One staged `.epub` ready for ingest — its bytes are ALREADY committed to the
/// content store (both front doors stream the file to disk first), so the core works
/// from the sha and NEVER holds a whole volume in memory. `filename` drives the
/// title/order parse (DW.1); `root` is the store-root hint for the probe.
pub struct StagedVolume {
    pub sha: String,
    pub len: i64,
    pub root: String,
    pub filename: String,
}

/// What happened to one file in a bulk run — every file lands in exactly one arm, so
/// the operator report is complete + honest (no silent drops).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeOutcome {
    /// A new volume item + page was created.
    Created { title: String },
    /// The exact bytes are already a volume under this series — idempotent re-run skip.
    SkippedDuplicate { filename: String },
    /// A volume page with this slug already exists (a same-numbered but DIFFERENT
    /// file); left untouched rather than colliding. Delete the page to re-ingest.
    SkippedExisting { filename: String, slug: String },
    /// The file couldn't be ingested (a corrupt EPUB, a DB error); the reason is
    /// surfaced and any half-created page is rolled back.
    Failed { filename: String, error: String },
}

/// The tally of a bulk run — every outcome, plus cheap counts for the summary.
#[derive(Debug, Default)]
pub struct IngestReport {
    pub outcomes: Vec<VolumeOutcome>,
}

impl IngestReport {
    pub fn created(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, VolumeOutcome::Created { .. }))
            .count()
    }
    pub fn skipped(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| {
                matches!(
                    o,
                    VolumeOutcome::SkippedDuplicate { .. } | VolumeOutcome::SkippedExisting { .. }
                )
            })
            .count()
    }
    pub fn failed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, VolumeOutcome::Failed { .. }))
            .count()
    }
}

/// Has this series already got a volume backed by these exact bytes? True when a
/// child page of the series embeds a media item that has a variant with `sha` — the
/// CONTENT-hash idempotency check (DW.2) that makes a re-run over the same folder
/// skip every already-ingested volume. The `sha` predicate hits the indexed
/// `media_variant.sha256` first, so the child scan stays tiny.
async fn series_has_volume_with_sha(
    pool: &SqlitePool,
    series_id: i64,
    sha: &str,
) -> Result<bool> {
    let hit = sqlx::query_scalar!(
        r#"SELECT EXISTS(
            SELECT 1
            FROM media_variant mv
            JOIN media m ON m.media_id = mv.media_id
            JOIN content_pages cp ON instr(cp.page_markdown, '/media/' || m.media_ref) > 0
            WHERE mv.sha256 = ?1 AND cp.parent_page_id = ?2
        ) AS "hit!: i64""#,
        sha,
        series_id
    )
    .fetch_one(pool)
    .await?;
    Ok(hit != 0)
}

/// Ingest a batch of staged `.epub`s into ordered volume pages under `series` (DW.2)
/// — the ONE core both front doors funnel into. Per file: dedup by content-hash (skip
/// a re-run), reserve the page (inheriting the series' gate; a slug collision is a
/// soft skip), ingest the EPUB media item (cover auto-extracted), then fill the page
/// with the `![](/media/<ref>)` embed + the parsed order. Best-effort per file — one
/// bad EPUB is a `Failed` entry, never an aborted batch.
pub async fn ingest_volumes(
    state: &AppState,
    series: &ContentPageDao,
    series_path: &[&str],
    mut staged: Vec<StagedVolume>,
) -> IngestReport {
    // Deterministic order: sort by filename so an un-numbered fallback (position) is
    // stable across runs and matches the operator's on-disk view.
    staged.sort_by(|a, b| a.filename.cmp(&b.filename));
    let mut report = IngestReport::default();
    for (idx, vol) in staged.iter().enumerate() {
        let outcome = match ingest_one(state, series, series_path, vol, idx as i64).await {
            Ok(o) => o,
            Err(e) => VolumeOutcome::Failed {
                filename: vol.filename.clone(),
                error: format!("{e:#}"),
            },
        };
        report.outcomes.push(outcome);
    }
    report
}

async fn ingest_one(
    state: &AppState,
    series: &ContentPageDao,
    series_path: &[&str],
    vol: &StagedVolume,
    position: i64,
) -> Result<VolumeOutcome> {
    // 1. Idempotent re-run: exact bytes already a volume under this series → skip.
    if series_has_volume_with_sha(&state.pool, series.page_id, &vol.sha).await? {
        return Ok(VolumeOutcome::SkippedDuplicate {
            filename: vol.filename.clone(),
        });
    }
    let parsed = parse_volume(&vol.filename);
    let order = parsed.number.unwrap_or(position);

    // 2. Reserve the volume page (inherits the series gate). A slug collision means a
    //    same-numbered DIFFERENT file already owns this page — a soft skip, no media.
    let written = match create_page(&state.pool, series_path, &parsed.title).await {
        Ok(w) => w,
        Err(PageWriteError::DuplicateSlug { slug, .. }) => {
            return Ok(VolumeOutcome::SkippedExisting {
                filename: vol.filename.clone(),
                slug,
            });
        }
        Err(PageWriteError::EmptyTitle) => {
            return Ok(VolumeOutcome::Failed {
                filename: vol.filename.clone(),
                error: "the filename produced an empty title".into(),
            });
        }
        Err(PageWriteError::NotFound) => return Err(anyhow!("series page not found")),
        Err(PageWriteError::Internal(e)) => return Err(e),
    };

    // 3. Ingest the EPUB media item (probe + variant + OPF cover). On failure, roll
    //    back the empty page so a corrupt file leaves no orphan volume behind.
    let media = match ingest_stored_file(
        state,
        vol.sha.clone(),
        vol.len,
        vol.root.clone(),
        &vol.filename,
        Some(parsed.title.clone()),
        series.min_role.clone(),
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            if let Ok(Some(page)) = ContentPageDao::find_by_id(&state.pool, written.page_id).await {
                let _ = page.delete(&state.pool).await;
            }
            return Err(anyhow!("ingest epub media: {e:#}"));
        }
    };

    // 4. Fill the page: the media embed + the parsed order. min_role/date default to
    //    KEEP (the inherited gate + the create instant); cover clears (the card
    //    auto-covers from the embed via DV.11).
    let vpath: Vec<&str> = written.path_segments.iter().map(String::as_str).collect();
    update_page(
        &state.pool,
        &state.site_host,
        &vpath,
        PageUpdate {
            title: Some(parsed.title.clone()),
            markdown: format!("![](/media/{})", media.media_ref),
            order,
            ..Default::default()
        },
    )
    .await
    .map_err(|e| anyhow!("fill volume page: {e:?}"))?;

    Ok(VolumeOutcome::Created {
        title: parsed.title,
    })
}

/// Find-or-create the `library → manga → <series>` chain, returning the SERIES page
/// plus its `/pages` path segments (DW.5). Every level inherits its parent's gate
/// (`library` seeds Family), so a freshly-created series is Family-gated for free,
/// and the ingest self-bootstraps the whole section on the very first drop. A
/// newly-created section/series page is stamped with a ` ```children ` fence so it
/// lists its children when viewed directly at `/pages/library/manga[/…]`.
pub async fn resolve_or_create_series(
    state: &AppState,
    series_name: &str,
) -> Result<(ContentPageDao, Vec<String>)> {
    let series_title = series_name.trim();
    if series_title.is_empty() {
        return Err(anyhow!("the series name is empty"));
    }
    let library = ContentPageDao::find_by_name(&state.pool, None, "library")
        .await?
        .ok_or_else(|| anyhow!("the `library` special page is missing"))?;

    // The manga section (child of library) — lists its series newest-first (matches
    // the `/library/manga` section route's ordering).
    let manga = ensure_child(
        state,
        &["library"],
        Some(library.page_id),
        MANGA_SECTION_TITLE,
        "newest",
    )
    .await?;

    // The series (child of manga) — lists its volumes by manual page_order (the
    // parsed volume number), so 1..N read in order.
    let series = ensure_child(
        state,
        &["library", "manga"],
        Some(manga.page_id),
        series_title,
        "manual",
    )
    .await?;

    let path = vec![
        "library".to_string(),
        "manga".to_string(),
        series.page_name.clone(),
    ];
    Ok((series, path))
}

/// Find a child by the slug `title` would produce, or create it (inheriting the
/// parent's gate) and stamp it with a ` ```children order=<order> ` fence so a direct
/// visit lists its children. Idempotent — a re-run finds the existing page; a race (a
/// concurrent create) re-finds rather than erroring.
async fn ensure_child(
    state: &AppState,
    parent_path: &[&str],
    parent_id: Option<i64>,
    title: &str,
    child_order: &str,
) -> Result<ContentPageDao> {
    let slug = Slug::new(title).ok_or_else(|| anyhow!("`{title}` slugified to empty"))?;
    if let Some(existing) =
        ContentPageDao::find_by_name(&state.pool, parent_id, slug.as_str()).await?
    {
        return Ok(existing);
    }
    let written = match create_page(&state.pool, parent_path, title).await {
        Ok(w) => w,
        Err(PageWriteError::DuplicateSlug { .. }) => {
            return ContentPageDao::find_by_name(&state.pool, parent_id, slug.as_str())
                .await?
                .ok_or_else(|| anyhow!("`{title}` slug collided but the page is missing"));
        }
        Err(e) => return Err(anyhow!("create `{title}`: {e:?}")),
    };
    // Stamp the children-index fence so a direct visit lists the section's children.
    let path: Vec<&str> = written.path_segments.iter().map(String::as_str).collect();
    update_page(
        &state.pool,
        &state.site_host,
        &path,
        PageUpdate {
            title: Some(title.to_string()),
            markdown: format!("```children order={child_order}\n```\n"),
            order: 0,
            ..Default::default()
        },
    )
    .await
    .map_err(|e| anyhow!("stamp `{title}` markdown: {e:?}"))?;
    ContentPageDao::find_by_id(&state.pool, written.page_id)
        .await?
        .ok_or_else(|| anyhow!("just-created `{title}` vanished"))
}

// ─────────────────────────── front doors (DW.3 / DW.4) ───────────────────────────

/// The bulk-ingest console (`GET /admin/media/import`) — a filesystem-folder form
/// (the 271-volume path) + a browser multi-file drop (small batches) + guidance. The
/// POST handlers re-render it with a `flash` banner (and, for the synchronous browser
/// path, the per-file `report`).
#[derive(Template)]
#[template(path = "admin/manga_ingest.html")]
struct MangaIngestTemplate {
    top_bar: TopBar,
    auth_state: AuthenticationState,
    /// A banner after a POST (started / rejected). `None` on the plain GET.
    flash: Option<String>,
    /// The synchronous browser-upload report. `None` on GET / the spawned filesystem path.
    report: Option<ReportView>,
}

/// A rendered ingest report for the console — the counts + a human line per file.
struct ReportView {
    created: usize,
    skipped: usize,
    failed: usize,
    lines: Vec<String>,
}

fn report_view(report: &IngestReport) -> ReportView {
    let lines = report
        .outcomes
        .iter()
        .map(|o| match o {
            VolumeOutcome::Created { title } => format!("✓ {title}"),
            VolumeOutcome::SkippedDuplicate { filename } => {
                format!("· skipped (already ingested): {filename}")
            }
            VolumeOutcome::SkippedExisting { filename, slug } => {
                format!("· skipped (a page “{slug}” exists): {filename}")
            }
            VolumeOutcome::Failed { filename, error } => format!("✗ {filename}: {error}"),
        })
        .collect();
    ReportView {
        created: report.created(),
        skipped: report.skipped(),
        failed: report.failed(),
        lines,
    }
}

async fn render_console(
    state: &AppState,
    session_data: &SessionData,
    flash: Option<String>,
    report: Option<ReportView>,
) -> Result<Response, AppError> {
    let template = MangaIngestTemplate {
        top_bar: TopBar::create(&state.pool, "admin", session_data.auth_state.role()).await?,
        auth_state: session_data.auth_state.clone(),
        flash,
        report,
    };
    Ok(HtmlTemplate(template).into_response())
}

pub async fn show_ingest_console(
    State(state): State<AppState>,
    session_data: SessionData,
) -> Result<Response, AppError> {
    render_console(&state, &session_data, None, None).await
}

/// Filesystem front door (DW.3): `POST /admin/media/import/filesystem` — a server-side
/// folder path + series name. Validates the folder, resolves/creates the series, then
/// SPAWNS the long ingest (staging can copy tens of GB) and returns immediately; the
/// report goes to `/admin/logs` and volumes appear on the series page as they process.
#[derive(Deserialize)]
pub struct FilesystemIngestForm {
    series: String,
    folder: String,
}

pub async fn ingest_filesystem(
    State(state): State<AppState>,
    session_data: SessionData,
    Form(form): Form<FilesystemIngestForm>,
) -> Result<Response, AppError> {
    let series_name = form.series.trim().to_string();
    if series_name.is_empty() {
        return render_console(&state, &session_data, Some("A series name is required.".into()), None).await;
    }
    let folder = match validate_folder(&form.folder) {
        Ok(p) => p,
        Err(e) => {
            return render_console(&state, &session_data, Some(format!("Folder rejected: {e}")), None).await;
        }
    };
    let books = list_books(&folder).await?;
    if books.is_empty() {
        let msg = format!("No .epub or .cbz files found in {}.", folder.display());
        return render_console(&state, &session_data, Some(msg), None).await;
    }
    let count = books.len();
    let (series, series_path) = match resolve_or_create_series(&state, &series_name).await {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Could not resolve the series: {e:#}");
            return render_console(&state, &session_data, Some(msg), None).await;
        }
    };

    // Spawn the ingest — staging tens of GB + creating pages can take minutes; a
    // request can't hold that. Detached like the coordinator backfills: it logs every
    // step + the final tally, so `/admin/logs` is the progress view (alongside the
    // series page filling in). A failure inside can't take the app down.
    let st = state.clone();
    let series_name_log = series_name.clone();
    tokio::spawn(async move {
        tracing::info!(
            "manga ingest: staging {count} volume(s) into series `{series_name_log}` from {}",
            folder.display()
        );
        let path_refs: Vec<&str> = series_path.iter().map(String::as_str).collect();
        let report = ingest_folder(&st, &series, &path_refs, books).await;
        tracing::info!(
            "manga ingest into `{series_name_log}` done: {} created, {} skipped, {} failed",
            report.created(),
            report.skipped(),
            report.failed()
        );
        for o in &report.outcomes {
            if let VolumeOutcome::Failed { filename, error } = o {
                tracing::warn!("manga ingest: {filename} failed: {error}");
            }
        }
    });

    let flash = format!(
        "Ingest of {count} file(s) into “{series_name}” started — watch /admin/logs; volumes appear on the series page as they process."
    );
    render_console(&state, &session_data, Some(flash), None).await
}

/// Browser front door (DW.4): `POST /admin/media/import/upload` — a multi-file drop
/// with a series name, streamed to the store. SYNCHRONOUS (for SMALL batches — a full
/// 27 GB series must go through the filesystem path), returning the per-file report.
pub async fn ingest_upload(
    State(state): State<AppState>,
    session_data: SessionData,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut series_name = String::new();
    let mut staged: Vec<StagedVolume> = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow!("reading multipart: {e}"))?
    {
        let Some(fname) = field.file_name().map(|s| s.to_string()) else {
            let name = field.name().unwrap_or("").to_string();
            let value = field.text().await.unwrap_or_default();
            if name == "series" {
                series_name = value.trim().to_string();
            }
            continue;
        };
        if !is_book_filename(&fname) {
            // Drain the non-book part so the multipart stream stays aligned.
            while field.chunk().await.map_err(|e| anyhow!("draining upload: {e}"))?.is_some() {}
            continue;
        }
        let mut blob = state.media_store.stage().await?;
        while let Some(chunk) = field.chunk().await.map_err(|e| anyhow!("reading upload: {e}"))? {
            blob.write_chunk(&chunk).await?;
        }
        if blob.is_empty() {
            continue;
        }
        let (sha, len, root) = blob.commit(&state.media_store).await?;
        staged.push(StagedVolume {
            sha,
            len: len as i64,
            root: root.to_string_lossy().into_owned(),
            filename: fname,
        });
    }

    if series_name.is_empty() {
        return render_console(&state, &session_data, Some("A series name is required.".into()), None).await;
    }
    if staged.is_empty() {
        return render_console(&state, &session_data, Some("No .epub or .cbz files in the upload.".into()), None).await;
    }
    let (series, series_path) = resolve_or_create_series(&state, &series_name).await?;
    let path_refs: Vec<&str> = series_path.iter().map(String::as_str).collect();
    let report = ingest_volumes(&state, &series, &path_refs, staged).await;
    let view = report_view(&report);
    let flash = format!(
        "Ingested into “{series_name}”: {} created, {} skipped, {} failed.",
        view.created, view.skipped, view.failed
    );
    render_console(&state, &session_data, Some(flash), Some(view)).await
}

/// Stage every book file (`.epub`/`.cbz`) in `folder` into ordered volume pages under
/// `series`, STREAMING each file to the store then ingesting it immediately (so pages appear
/// incrementally + one 27 GB series never sits in memory). Sorted by path so the
/// position fallback (an un-numbered file) is deterministic.
async fn ingest_folder(
    state: &AppState,
    series: &ContentPageDao,
    series_path: &[&str],
    mut epubs: Vec<PathBuf>,
) -> IngestReport {
    epubs.sort();
    let mut report = IngestReport::default();
    for (idx, path) in epubs.iter().enumerate() {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let outcome = match stage_file(&state.media_store, path).await {
            Ok((sha, len, root)) => {
                let vol = StagedVolume { sha, len, root, filename: filename.clone() };
                match ingest_one(state, series, series_path, &vol, idx as i64).await {
                    Ok(o) => o,
                    Err(e) => VolumeOutcome::Failed { filename, error: format!("{e:#}") },
                }
            }
            Err(e) => VolumeOutcome::Failed {
                filename,
                error: format!("staging failed: {e:#}"),
            },
        };
        report.outcomes.push(outcome);
    }
    report
}

/// Stream a file on disk into the content store (O(chunk) memory — never buffered
/// whole), returning its `(sha, len, root)`. Shared shape with the multipart stage
/// path; the source here is a local file the admin pointed the ingest at.
async fn stage_file(store: &MediaStore, path: &Path) -> Result<(String, i64, String)> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| anyhow!("open {}: {e}", path.display()))?;
    let mut staged = store.stage().await?;
    let mut buf = vec![0u8; 1 << 20]; // 1 MiB chunks
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        staged.write_chunk(&buf[..n]).await?;
    }
    if staged.is_empty() {
        return Err(anyhow!("empty file"));
    }
    let (sha, len, root) = staged.commit(store).await?;
    Ok((sha, len as i64, root.to_string_lossy().into_owned()))
}

/// A supported book file — `.epub` or `.cbz` (both read by the same foliate reader;
/// most manga are CBZ, novels EPUB). The one place the accepted extensions live.
fn is_book_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".epub") || lower.ends_with(".cbz")
}

/// List the book files (`.epub`/`.cbz`) directly in `folder` (non-recursive). Used by
/// the filesystem front door; a subfolder-per-series layout is the operator's to flatten.
async fn list_books(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut rd = tokio::fs::read_dir(folder)
        .await
        .map_err(|e| anyhow!("read dir {}: {e}", folder.display()))?;
    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_book_filename)
        {
            out.push(path);
        }
    }
    Ok(out)
}

/// Validate the admin-supplied folder path (DW.3). CANONICALIZE resolves `..` +
/// symlinks to a real path (so a traversal string can't escape to somewhere the
/// admin didn't mean), then it must be an existing directory. Admin-only by the
/// `/admin` gate; restricting to a configured drop-dir / the media roots is a
/// deferred tightening (the operator is trusted here).
fn validate_folder(input: &str) -> Result<PathBuf> {
    let input = input.trim();
    if input.is_empty() {
        return Err(anyhow!("empty path"));
    }
    let path = std::fs::canonicalize(input).map_err(|e| anyhow!("cannot resolve `{input}`: {e}"))?;
    if !path.is_dir() {
        return Err(anyhow!("`{}` is not a directory", path.display()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(name: &str) -> (Option<i64>, String) {
        let p = parse_volume(name);
        (p.number, p.title)
    }

    #[test]
    fn v_prefixed_volume_numbers() {
        assert_eq!(parse("Series v012.epub"), (Some(12), "Volume 12".into()));
        assert_eq!(parse("Naruto v5.epub"), (Some(5), "Volume 5".into()));
        // The series name merely starting with `v` must not swallow the number.
        assert_eq!(parse("Villain v03.epub"), (Some(3), "Volume 3".into()));
    }

    #[test]
    fn separated_vol_and_volume_words() {
        assert_eq!(parse("Naruto Vol.5.epub"), (Some(5), "Volume 5".into()));
        assert_eq!(parse("Naruto Vol 12.epub"), (Some(12), "Volume 12".into()));
        assert_eq!(
            parse("One Piece Volume 100.epub"),
            (Some(100), "Volume 100".into())
        );
    }

    #[test]
    fn chapter_markers_keep_the_chapter_label() {
        // chris's Jujutsu Kaisen library reads "Chapter 1", not "Volume 1".
        assert_eq!(parse("Jujutsu Kaisen c001.epub"), (Some(1), "Chapter 1".into()));
        assert_eq!(parse("JJK Chapter 5.epub"), (Some(5), "Chapter 5".into()));
        assert_eq!(parse("ch12.epub"), (Some(12), "Chapter 12".into()));
    }

    #[test]
    fn hash_and_bare_trailing_numbers() {
        assert_eq!(parse("Bleach #12.epub"), (Some(12), "Volume 12".into()));
        assert_eq!(parse("Series 05.epub"), (Some(5), "Volume 5".into()));
        assert_eq!(parse("Bleach 001.epub"), (Some(1), "Volume 1".into()));
    }

    #[test]
    fn a_marker_beats_a_stray_year() {
        assert_eq!(parse("Series 2020 v12.epub"), (Some(12), "Volume 12".into()));
        // The number right after the marker wins even with a trailing year.
        assert_eq!(
            parse("Series Vol 12 (2020).epub"),
            (Some(12), "Volume 12".into())
        );
    }

    #[test]
    fn no_number_falls_back_to_the_stem() {
        assert_eq!(parse("cover.epub"), (None, "cover".into()));
        assert_eq!(parse("readme.epub"), (None, "readme".into()));
        // A word starting with a marker letter but not a marker → not a number.
        assert_eq!(parse("Villain.epub"), (None, "Villain".into()));
        assert_eq!(parse(".epub"), (None, "Untitled".into()));
    }

    #[test]
    fn extension_is_case_insensitive_and_optional() {
        assert_eq!(parse("Series v3.EPUB"), (Some(3), "Volume 3".into()));
        // No extension still parses (a browser drop could hand a bare name).
        assert_eq!(parse("Series v7"), (Some(7), "Volume 7".into()));
        // CBZ is stripped the same as EPUB.
        assert_eq!(parse("Series v9.cbz"), (Some(9), "Volume 9".into()));
        assert_eq!(parse("One-Shot.cbz"), (None, "One-Shot".into()));
    }
}
