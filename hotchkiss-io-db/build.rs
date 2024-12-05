use sqlx::sqlite::SqliteLockingMode::Exclusive;
use sqlx::sqlite::SqliteSynchronous::Normal;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{env, str::FromStr};

fn main() {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(real_main())
}

async fn real_main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let src_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    let schema_key = "DATABASE_URL";
    let schema_url = "sqlite://".to_string() + &out_dir + "/schema.db";

    //Run a migration for sqlx so it can compile queries
    let con_opts = SqliteConnectOptions::from_str(&schema_url)
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true)
        .locking_mode(Exclusive)
        .shared_cache(true)
        .synchronous(Normal);

    let pool_opts = SqlitePoolOptions::new().min_connections(2);

    let pool = pool_opts.connect_with(con_opts).await.unwrap();

    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    env::set_var(schema_key, schema_url.clone());

    println!("cargo::rustc-env={}={}", schema_key, schema_url);
    println!("cargo::rerun-if-changed={}", src_dir + "/migrations");
    println!("cargo::metadata=database_url={}", schema_url); //Path to the migration database for dependents
}
