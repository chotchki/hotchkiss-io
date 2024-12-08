use acme_lib::persist::{Persist, PersistKey};
use acme_lib::Error;
use sqlx::{query, query_scalar, SqlitePool};
use std::sync::Arc;
use tokio::runtime::Handle;

#[derive(Clone, Debug)]
pub struct AcmePersistKey {
    pool: SqlitePool,
    handle: Arc<Handle>,
}

impl Persist for AcmePersistKey {
    fn put(&self, key: &PersistKey<'_>, value: &[u8]) -> Result<(), Error> {
        let key_str = key.to_string();

        self.handle
            .block_on(self.put_key_value(&key_str, value))
            .map_err(|_| Error::Call("Upsert failed for key ".to_string() + &key_str))?;

        Ok(())
    }

    fn get(&self, key: &PersistKey<'_>) -> Result<Option<Vec<u8>>, Error> {
        let key_str = key.to_string();

        let res = self
            .handle
            .block_on(self.get_key_value(&key_str))
            .map_err(|_| Error::Call("Error querying sqlx for ".to_string() + &key_str))?;

        Ok(res)
    }
}

impl AcmePersistKey {
    pub fn create(pool: SqlitePool, handle: Arc<Handle>) -> Self {
        Self { pool, handle }
    }

    async fn put_key_value(&self, key: &str, value: &[u8]) -> Result<(), sqlx::Error> {
        query(
            r#"
                INSERT OR REPLACE INTO acme_persist (
                    acme_key, 
                    acme_value
                ) VALUES (
                    ?1,
                    ?2
                ) ON CONFLICT (acme_key) 
                DO UPDATE 
                SET 
                    acme_value = ?2
                "#,
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_key_value(&self, key: &str) -> Result<Option<Vec<u8>>, sqlx::Error> {
        let rec = query_scalar(
            r#"
                SELECT acme_value
                FROM acme_persist
                WHERE
                    acme_key = ?1
                "#,
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = rec {
            Ok(Some(row))
        } else {
            Ok(None)
        }
    }
}
