use std::str::FromStr;

use sqlx::{
    sqlite::{
        SqliteConnectOptions, SqliteLockingMode::Exclusive, SqlitePoolOptions,
        SqliteSynchronous::Normal,
    },
    Error, SqlitePool,
};

pub struct DatabaseHandle;

impl DatabaseHandle {
    pub async fn create(path: &str) -> Result<SqlitePool, Error> {
        let pool_opts = SqlitePoolOptions::new().min_connections(2);

        let con_opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path))?
            .create_if_missing(true)
            .foreign_keys(true)
            .locking_mode(Exclusive)
            .shared_cache(true)
            .synchronous(Normal);

        let pool = pool_opts.connect_with(con_opts).await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(pool)
    }
}
