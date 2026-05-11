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
    println!("cargo::rerun-if-changed=migrations");

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
