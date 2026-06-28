use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

use super::crypto_key::CryptoKey;

/// CryptoKey row id for the API-key HMAC pepper (1 = session signing, 2 = media
/// URL key, 3 = this). A server secret, never stored alongside the hashes.
const API_KEY_PEPPER_KEY_ID: i64 = 3;

/// One API-key row. Never carries the plaintext key OR its hash — both stay
/// server-side (the plaintext is shown to the user exactly once, at creation).
pub struct ApiKeyDao {
    pub id: i64,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ApiKeyDao {
    /// Mint a key for `user_id`, returning the PLAINTEXT (`hio_<43-char base64url>`
    /// — shown once, never recoverable) plus the stored row. Only the HMAC hash is
    /// persisted.
    pub async fn create(
        pool: &SqlitePool,
        user_id: &Uuid,
        label: &str,
    ) -> Result<(String, ApiKeyDao)> {
        let key = generate_key()?;
        let key_hash = hash_key(pool, &key).await?;
        let user_id = user_id.to_string();
        let created_at = Utc::now();
        let row = sqlx::query!(
            r#"INSERT INTO api_keys (user_id, key_hash, label, created_at)
               VALUES (?1, ?2, ?3, ?4) RETURNING id as "id!""#,
            user_id,
            key_hash,
            label,
            created_at,
        )
        .fetch_one(pool)
        .await?;
        Ok((
            key,
            ApiKeyDao {
                id: row.id,
                label: label.to_string(),
                created_at,
                last_used_at: None,
                revoked_at: None,
            },
        ))
    }

    /// Resolve a presented key → `(user_id, key_id)` for a LIVE (non-revoked) key,
    /// or `None`. The lookup is by the full HMAC hash, so an attacker can't probe
    /// without the pepper.
    pub async fn authenticate(pool: &SqlitePool, presented_key: &str) -> Result<Option<(Uuid, i64)>> {
        let key_hash = hash_key(pool, presented_key).await?;
        let row = sqlx::query!(
            r#"SELECT id as "id!", user_id FROM api_keys
               WHERE key_hash = ?1 AND revoked_at IS NULL"#,
            key_hash,
        )
        .fetch_optional(pool)
        .await?;
        match row {
            Some(r) => {
                let uid = Uuid::parse_str(&r.user_id).context("api_keys.user_id is not a uuid")?;
                Ok(Some((uid, r.id)))
            }
            None => Ok(None),
        }
    }

    /// Stamp `last_used_at` after a successful auth (best-effort — the caller logs
    /// but does not fail the request on error).
    pub async fn touch_last_used(pool: &SqlitePool, key_id: i64) -> Result<()> {
        let now = Utc::now();
        sqlx::query!(
            r#"UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2"#,
            now,
            key_id,
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// A user's keys, newest first — for the management UI (no hash, ever).
    pub async fn list_for_user(pool: &SqlitePool, user_id: &Uuid) -> Result<Vec<ApiKeyDao>> {
        let uid = user_id.to_string();
        let rows = sqlx::query_as!(
            ApiKeyDao,
            r#"SELECT id as "id!", label,
                      created_at as "created_at!: DateTime<Utc>",
                      last_used_at as "last_used_at?: DateTime<Utc>",
                      revoked_at as "revoked_at?: DateTime<Utc>"
               FROM api_keys WHERE user_id = ?1 ORDER BY created_at DESC"#,
            uid,
        )
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// Revoke a key, scoped to `user_id` so a user can only revoke their own.
    /// Returns whether a live key was actually revoked.
    pub async fn revoke(pool: &SqlitePool, key_id: i64, user_id: &Uuid) -> Result<bool> {
        let uid = user_id.to_string();
        let now = Utc::now();
        let res = sqlx::query!(
            r#"UPDATE api_keys SET revoked_at = ?1
               WHERE id = ?2 AND user_id = ?3 AND revoked_at IS NULL"#,
            now,
            key_id,
            uid,
        )
        .execute(pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

/// `hio_<43-char base64url>` from 32 cryptographically-random bytes (openssl).
fn generate_key() -> Result<String> {
    use base64::Engine;
    let mut raw = [0u8; 32];
    openssl::rand::rand_bytes(&mut raw).context("generating api-key bytes")?;
    Ok(format!(
        "hio_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    ))
}

/// HMAC-SHA256(server pepper, key) → lowercase hex. The pepper is `crypto_keys`
/// id 3 (a server secret), so a leak of `key_hash` alone can't be brute-forced or
/// verified offline. Mirrors `media::media_url_key`'s openssl HMAC pattern.
async fn hash_key(pool: &SqlitePool, key: &str) -> Result<String> {
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::sign::Signer;

    let pepper = CryptoKey::get_or_create(pool, API_KEY_PEPPER_KEY_ID)
        .await?
        .key_value;
    let pkey = PKey::hmac(&pepper).context("building the api-key HMAC key")?;
    let mut signer = Signer::new(MessageDigest::sha256(), &pkey).context("api-key HMAC signer")?;
    signer.update(key.as_bytes()).context("api-key HMAC update")?;
    let mac = signer.sign_to_vec().context("api-key HMAC sign")?;
    Ok(mac.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::dao::roles::Role;
    use crate::db::dao::users::UserDao;

    async fn seed_admin(pool: &SqlitePool) -> Result<UserDao> {
        let mut user = UserDao {
            display_name: "chris".to_string(),
            id: Uuid::now_v7(),
            keys: sqlx::types::Json(vec![]),
            role: Role::Registered,
        };
        user.create(pool).await?; // first user is promoted to Admin inline
        Ok(user)
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn create_authenticate_list_revoke(pool: SqlitePool) -> Result<()> {
        let user = seed_admin(&pool).await?;

        let (key, row) = ApiKeyDao::create(&pool, &user.id, "laptop").await?;
        assert!(key.starts_with("hio_"), "key has the hio_ prefix: {key}");

        // The presented key authenticates to (user, key_id).
        assert_eq!(
            ApiKeyDao::authenticate(&pool, &key).await?,
            Some((user.id, row.id))
        );
        // A bogus key does not.
        assert!(ApiKeyDao::authenticate(&pool, "hio_nope").await?.is_none());

        // It lists for the user (newest first), no hash exposed by the type.
        let list = ApiKeyDao::list_for_user(&pool, &user.id).await?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].label, "laptop");
        assert!(list[0].last_used_at.is_none());

        // last_used stamps.
        ApiKeyDao::touch_last_used(&pool, row.id).await?;
        assert!(ApiKeyDao::list_for_user(&pool, &user.id).await?[0]
            .last_used_at
            .is_some());

        // Revoke → it no longer authenticates, and a re-revoke is a no-op.
        assert!(ApiKeyDao::revoke(&pool, row.id, &user.id).await?);
        assert!(ApiKeyDao::authenticate(&pool, &key).await?.is_none());
        assert!(!ApiKeyDao::revoke(&pool, row.id, &user.id).await?);

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn distinct_keys_get_distinct_hashes(pool: SqlitePool) -> Result<()> {
        let user = seed_admin(&pool).await?;
        let (k1, _) = ApiKeyDao::create(&pool, &user.id, "a").await?;
        let (k2, _) = ApiKeyDao::create(&pool, &user.id, "b").await?;
        assert_ne!(k1, k2, "each minted key is unique");
        assert_eq!(ApiKeyDao::list_for_user(&pool, &user.id).await?.len(), 2);
        Ok(())
    }
}
