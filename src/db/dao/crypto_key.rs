use anyhow::{Context, Result};
use sqlx::{prelude::FromRow, query, query_as, SqlitePool};
use tower_sessions::cookie::Key;
use tracing::debug;

/// A persisted signing key. `key_value` is the raw 64-byte `cookie::Key`
/// master key (`Key::master()`) — stored as bytes rather than via serde so
/// we don't need a forked `cookie` crate just to derive `Serialize`/`Deserialize`.
#[derive(Clone, Debug, FromRow, PartialEq)]
pub struct CryptoKey {
    pub id: i64,
    pub key_value: Vec<u8>,
}

impl CryptoKey {
    /// Reconstruct the `cookie::Key` from the stored master bytes.
    pub fn key(&self) -> Result<Key> {
        Key::try_from(self.key_value.as_slice())
            .context("stored crypto key is not a valid cookie::Key (needs >= 64 bytes)")
    }

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
                key_value
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
                    key_value: Key::generate().master().to_vec(),
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
            key_value: Key::generate().master().to_vec(),
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

        assert!(!ck.key()?.signing().is_empty());

        Ok(())
    }
}
