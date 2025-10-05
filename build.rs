use anyhow::Context;
use anyhow::Result;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqliteLockingMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteSynchronous;
use std::collections::HashMap;
use std::io;
use std::io::Cursor;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::str::FromStr;
use std::{env, process::Command};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

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

    //Download and cache the tailwind cli for build
    let components = HashMap::from([
        (
            "tailwindcli",
            "https://github.com/tailwindlabs/tailwindcss/releases/latest/download/tailwindcss-macos-arm64",
        ),
        (
            "daisyui.js",
            "https://github.com/saadeghi/daisyui/releases/latest/download/daisyui.js",
        ),
        (
            "daisyui-theme.js",
            "https://github.com/saadeghi/daisyui/releases/latest/download/daisyui-theme.js",
        ),
    ]);

    for (file, comp) in components {
        let cache_path = Path::new(&out_dir).join(file);
        if !cache_path.is_file() {
            let response = reqwest::get(comp).await?;
            let mut content = Cursor::new(response.bytes().await?);
            let mut cli_file = File::create(cache_path.clone()).await?;
            tokio::io::copy(&mut content, &mut cli_file).await?;
            let meta = cli_file.metadata().await?;
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            cli_file.set_permissions(perms).await?;
        }
    }

    //Get the tailwindcss cli
    let output = Command::new(Path::new(&out_dir).join("tailwindcli"))
        .args(["-i", "styles/tailwind.css", "-o", "assets/styles/main.css"])
        .output()?;

    io::stdout().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;

    assert!(output.status.success());

    Ok(())
}
