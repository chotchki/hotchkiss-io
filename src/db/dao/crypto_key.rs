use anyhow::Result;
use sqlx::{prelude::FromRow, query, query_as, SqlitePool};
use tower_sessions::cookie::Key;
use tracing::debug;

#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct CryptoKey {
    pub id: i64,
    pub key_value: sqlx::types::Json<Key>,
}

impl CryptoKey {
    pub async fn create(&self, pool: &SqlitePool) -> Result<()> {
        debug!("Creating key");

        query!(
            r#"
        INSERT INTO crypto_keys (
            id,
            key_value
        ) VALUES (
            ?1,
            ?2
        )
        "#,
            self.id,
            self.key_value
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn find_by_id(pool: &SqlitePool, id: i64) -> Result<Option<CryptoKey>> {
        debug!("Finding key id {id}");
        let key: Option<CryptoKey> = query_as!(
            CryptoKey,
            r#"
            select 
                id,
                key_value as "key_value: sqlx::types::Json<Key>"
            from 
                crypto_keys
            where id = ?1
        "#,
            id
        )
        .fetch_optional(pool)
        .await?;

        debug!("Sql returned");

        Ok(key)
    }

    pub async fn get_or_create(pool: &SqlitePool, id: i64) -> Result<CryptoKey> {
        match Self::find_by_id(pool, id).await? {
            Some(s) => {
                debug!("Found key");
                Ok(s)
            }
            None => {
                debug!("Didn't find, creating");

                let key = CryptoKey {
                    id,
                    key_value: sqlx::types::Json(Key::generate()),
                };
                key.create(pool).await?;
                Ok(key)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let ck = CryptoKey {
            id: 2,
            key_value: sqlx::types::Json(Key::generate()),
        };

        ck.create(&pool).await?;

        let found_ck = CryptoKey::find_by_id(&pool, 2)
            .await?
            .expect("We just inserted this value");

        assert_eq!(ck, found_ck);

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn combo(pool: SqlitePool) -> Result<()> {
        let ck = CryptoKey::get_or_create(&pool, 1).await?;

        assert!(!ck.key_value.signing().is_empty());

        Ok(())
    }
}
