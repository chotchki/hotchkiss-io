use anyhow::Result;
use sqlx::ConnectOptions;
use sqlx::{
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode::Wal, SqliteLockingMode::Normal, SqlitePoolOptions,
        SqliteSynchronous,
    },
    SqlitePool,
};
use std::str::FromStr;
use tracing::debug;
use tracing::log::LevelFilter;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./src/db/migrations");

pub struct DatabaseHandle;

impl DatabaseHandle {
    pub async fn create(path: &str) -> Result<SqlitePool> {
        debug!("Creating database on disk");
        let pool_opts = SqlitePoolOptions::new().min_connections(2);

        let con_opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path))?
            .log_statements(LevelFilter::Debug)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(Wal)
            .locking_mode(Normal)
            .synchronous(SqliteSynchronous::Normal);

        let pool = pool_opts.connect_with(con_opts).await?;

        MIGRATOR.run(&pool).await?;

        Ok(pool)
    }
}

#[cfg(test)]
mod test {
    use sqlx::query;

    use super::*;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn test_handle(pool: SqlitePool) -> Result<()> {
        let result = query!("select * from users").fetch_optional(&pool).await?;

        //We shouldn't have users yet
        assert!(result.is_none());

        Ok(())
    }
}
