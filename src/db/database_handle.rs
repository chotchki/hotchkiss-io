use anyhow::Result;
use sqlx::{
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode::Wal, SqliteLockingMode::Normal, SqlitePoolOptions,
        SqliteSynchronous,
    },
    SqlitePool,
};
use std::str::FromStr;
use tracing::debug;

pub struct DatabaseHandle;

#[cfg(not(test))]
impl DatabaseHandle {
    pub async fn create(path: &str) -> Result<SqlitePool> {
        debug!("Creating database on disk");
        let pool_opts = SqlitePoolOptions::new().min_connections(2);

        let con_opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path))?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(Wal)
            .locking_mode(Normal)
            .shared_cache(true)
            .synchronous(SqliteSynchronous::Normal);

        let pool = pool_opts.connect_with(con_opts).await?;

        sqlx::migrate!("./src/db/migrations").run(&pool).await?;

        Ok(pool)
    }
}

#[cfg(test)]
impl DatabaseHandle {
    pub async fn create(_: &str) -> Result<SqlitePool> {
        debug!("Creating database in memory");
        let pool_opts = SqlitePoolOptions::new().min_connections(2);

        let con_opts = SqliteConnectOptions::new();

        let pool = pool_opts.connect_with(con_opts).await?;

        sqlx::migrate!("./src/db/migrations").run(&pool).await?;
        Ok(pool)
    }
}

#[cfg(test)]
mod test {
    use sqlx::query;

    use super::*;

    #[sqlx::test]
    async fn test_handle(pool: SqlitePool) -> Result<()> {
        let result = query!("select * from users").fetch_optional(&pool).await?;

        //We shouldn't have users yet
        assert!(result.is_none());

        Ok(())
    }
}
