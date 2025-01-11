use anyhow::Context;
use anyhow::Result;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqliteLockingMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteSynchronous;
use std::str::FromStr;
use std::{env, process::Command};

#[tokio::main]
async fn main() -> Result<()> {
    // TailwindCSS and Sqlx Migrations Change Tracking
    println!("cargo::rerun-if-changed=templates");
    println!("cargo::rerun-if-changed=migrations");

    let out_dir = env::var("OUT_DIR").context("No OUT_DIR, cargo must be broken")?;

    let schema_key = "DATABASE_URL";
    let schema_url = "sqlite://".to_string() + &out_dir + "/schema.db";

    env::set_var(schema_key, schema_url.clone());
    println!("cargo::rustc-env={schema_key}={schema_url}");

    //Run a migration for sqlx so it can compile queries
    let con_opts = SqliteConnectOptions::from_str(&schema_url)
        .with_context(|| format!("Unable to parse schema_url {schema_url}"))?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .locking_mode(SqliteLockingMode::Exclusive)
        .shared_cache(true)
        .synchronous(SqliteSynchronous::Normal);

    let pool_opts = SqlitePoolOptions::new().min_connections(2);

    let pool = pool_opts
        .connect_with(con_opts)
        .await
        .context("Unable create the pool")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("The build time migrations failed")?;

    Command::new("npx")
        .args([
            "tailwindcss",
            "-i",
            "styles/tailwind.css",
            "-o",
            "assets/styles/main.css",
        ])
        .output()?;

    Ok(())
}
