use anyhow::Context;
use anyhow::Result;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqliteLockingMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteSynchronous;
use std::io;
use std::io::Cursor;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::str::FromStr;
use std::{env, process::Command};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// Pinned Tailwind CLI release (arm64 macOS standalone binary). Bumping this
/// changes the cache filename, forcing a re-download.
const TAILWIND_VERSION: &str = "v4.3.0";

#[tokio::main]
async fn main() -> Result<()> {
    // TailwindCSS and Sqlx Migrations Change Tracking
    println!("cargo::rerun-if-changed=assets/scripts");
    println!("cargo::rerun-if-changed=templates");
    // Tailwind also scans these for utility classes emitted from Rust string
    // builders (the media embeds + the markdown transformer), so a class change
    // there must recompile the CSS.
    println!("cargo::rerun-if-changed=src/web/markdown");
    println!("cargo::rerun-if-changed=src/web/features");
    println!("cargo::rerun-if-changed=src/db/migrations");

    // Inline-SVG icon set: codegen the askama macro partial from the vendored
    // FA-Free solid SVGs (build/svg-icons/). WRITE-IF-CHANGED inside, so an
    // unchanged regen doesn't bump mtimes / cascade a recompile.
    generate_icons().context("generating the inline-SVG icon partial")?;

    let out_dir = env::var("OUT_DIR").context("No OUT_DIR, cargo must be broken")?;

    let schema_key = "DATABASE_URL";
    let schema_url = "sqlite://".to_string() + &out_dir + "/schema.db";

    // SAFETY: build.rs is a single threaded program and should not suffer from the set_var issues
    unsafe {
        env::set_var(schema_key, schema_url.clone());
    }
    println!("cargo::rustc-env={schema_key}={schema_url}");
    File::create(format!("{}/.env", env::var("CARGO_MANIFEST_DIR")?))
        .await?
        .write_all(format!("{schema_key}={schema_url}").as_bytes())
        .await?;

    //Run a migration for sqlx so it can compile queries
    let con_opts = SqliteConnectOptions::from_str(&schema_url)
        .with_context(|| format!("Unable to parse schema_url {schema_url}"))?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .locking_mode(SqliteLockingMode::Normal)
        .shared_cache(true)
        .synchronous(SqliteSynchronous::Normal);

    let pool_opts = SqlitePoolOptions::new().min_connections(2);

    let pool = pool_opts
        .connect_with(con_opts)
        .await
        .context("Unable create the pool")?;

    sqlx::migrate!("./src/db/migrations")
        .run(&pool)
        .await
        .context("The build time migrations failed")?;

    // Download + cache the pinned Tailwind CLI (version-keyed filename, so
    // bumping TAILWIND_VERSION re-downloads instead of reusing a stale binary).
    let cli_path = Path::new(&out_dir).join(format!("tailwindcli-{TAILWIND_VERSION}"));
    if !cli_path.is_file() {
        let url = format!(
            "https://github.com/tailwindlabs/tailwindcss/releases/download/{TAILWIND_VERSION}/tailwindcss-macos-arm64"
        );
        let bytes = reqwest::get(&url)
            .await?
            .error_for_status()
            .with_context(|| format!("downloading the Tailwind CLI from {url}"))?
            .bytes()
            .await?;
        let mut cli_file = File::create(&cli_path).await?;
        tokio::io::copy(&mut Cursor::new(bytes), &mut cli_file).await?;
        let mut perms = cli_file.metadata().await?.permissions();
        perms.set_mode(0o755);
        cli_file.set_permissions(perms).await?;
    }

    // Compile styles/tailwind.css -> assets/styles/main.css (gitignored).
    let output = Command::new(&cli_path)
        .args(["-i", "styles/tailwind.css", "-o", "assets/styles/main.css"])
        .output()?;
    io::stdout().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;
    assert!(output.status.success(), "tailwind css build failed");

    Ok(())
}

/// Inline-SVG icon set. Reads the vendored Font Awesome Free solid SVGs in
/// `build/svg-icons/solid/` and generates `templates/partials/icons.html` — one
/// parameterless askama macro per icon, each emitting an inline
/// `<svg class="icon" fill="currentColor" …>` so it inherits text color + 1em
/// sizing exactly like the old `<i class="fa-…">`. A typo'd `icons::foo()` is
/// then an askama compile error, so a template can't reference an un-vendored
/// icon — the safety we'd otherwise need a scan for. WRITE-IF-CHANGED keeps an
/// unchanged regen from bumping the mtime and cascading a recompile, which
/// matters because the output lands under the `rerun-if-changed=templates` tree.
fn generate_icons() -> Result<()> {
    let src_dir = Path::new("build/svg-icons/solid");
    println!("cargo::rerun-if-changed=build/svg-icons");

    let mut paths: Vec<_> = std::fs::read_dir(src_dir)
        .with_context(|| format!("reading icon source dir {}", src_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("svg"))
        .collect();
    paths.sort(); // deterministic output regardless of readdir order

    let mut out = String::new();
    out.push_str("{# GENERATED by build.rs from build/svg-icons/solid/*.svg — DO NOT EDIT. #}\n");
    out.push_str("{# Icons: Font Awesome Free 6.7.2 (CC BY 4.0) — https://fontawesome.com #}\n");
    for path in &paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .with_context(|| format!("bad icon filename {}", path.display()))?;
        let macro_name = stem.replace('-', "_");
        let svg = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let view_box =
            svg_attr(&svg, "viewBox").with_context(|| format!("no viewBox in {}", path.display()))?;
        let inner =
            svg_inner(&svg).with_context(|| format!("no <svg> body in {}", path.display()))?;
        out.push_str(&format!(
            "{{% macro {macro_name}() %}}<svg class=\"icon\" viewBox=\"{view_box}\" fill=\"currentColor\" aria-hidden=\"true\" focusable=\"false\">{inner}</svg>{{% endmacro %}}\n"
        ));
    }

    write_if_changed(Path::new("templates/partials/icons.html"), &out)
}

/// First `attr="value"` in `svg`.
fn svg_attr(svg: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = svg.find(&needle)? + needle.len();
    let len = svg[start..].find('"')?;
    Some(svg[start..start + len].to_string())
}

/// Body between the opening `<svg …>` and closing `</svg>`, with the Font
/// Awesome attribution comment (`<!--! … -->`) stripped.
fn svg_inner(svg: &str) -> Option<String> {
    let open_end = svg.find('>')? + 1;
    let close = svg.rfind("</svg>")?;
    let mut inner = svg.get(open_end..close)?.to_string();
    if let (Some(cs), Some(ce)) = (inner.find("<!--"), inner.find("-->")) {
        inner.replace_range(cs..ce + 3, "");
    }
    Some(inner.trim().to_string())
}

/// Write only when the content differs (or the file is absent), so an unchanged
/// regen doesn't bump the mtime and force a downstream recompile.
fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if std::fs::read_to_string(path)
        .map(|c| c == content)
        .unwrap_or(false)
    {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
