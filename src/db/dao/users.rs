use super::roles::Role;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{query, query_as, FromRow, SqlitePool};
use std::fmt::Display;
use uuid::Uuid;
use webauthn_rs::prelude::Passkey;

#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Serialize)]
pub struct UserDao {
    pub display_name: String,
    pub id: Uuid,
    pub keys: sqlx::types::Json<Vec<Passkey>>,
    pub role: Role,
}

/// One row of the admin user list — no passkey blobs, just role + counts.
#[derive(Clone, Debug, PartialEq)]
pub struct UserSummary {
    pub display_name: String,
    pub id: Uuid,
    pub role: Role,
    pub passkey_count: i64,
    pub api_key_count: i64,
}

impl UserDao {
    pub async fn create(&mut self, pool: &SqlitePool) -> Result<()> {
        let id = self.id.to_string();
        let keys = serde_json::to_string(&self.keys)?;

        //Handling the fist user
        let rec = query!(
            r#"
        insert into users (
            display_name,
            id,
            keys,
            app_role
        ) VALUES (
            ?1,
            ?2,
            ?3,
            CASE WHEN (SELECT COUNT(*) from users) == 0
            THEN 'Admin'
            ELSE 'Registered'
            END
        )
        RETURNING app_role as "role: Role"
        "#,
            self.display_name,
            id,
            keys
        )
        .fetch_one(pool)
        .await?;

        self.role = rec.role;

        Ok(())
    }

    pub async fn find_by_passkey(pool: &SqlitePool, passkey: &Passkey) -> Result<Option<UserDao>> {
        let passkey = sqlx::types::Json(passkey);
        Ok(query_as!(
            UserDao,
            r#"
        SELECT 
            display_name,
            id as "id: uuid::fmt::Hyphenated",
            keys as "keys: sqlx::types::Json<Vec<Passkey>>",
            app_role as "role: Role"
        FROM users
        WHERE
            EXISTS (SELECT 1 FROM json_each(keys) WHERE value = ?1)
    "#,
            passkey
        )
        .fetch_optional(pool)
        .await?)
    }

    pub async fn find_by_uuid(pool: &SqlitePool, uuid: &Uuid) -> Result<Option<UserDao>> {
        let temp_uuid = uuid.to_string();
        Ok(query_as!(
            UserDao,
            r#"
        SELECT 
            display_name,
            id as "id: uuid::fmt::Hyphenated",
            keys as "keys: sqlx::types::Json<Vec<Passkey>>",
            app_role as "role: Role"
        FROM users
        WHERE
            id = ?1
    "#,
            temp_uuid
        )
        .fetch_optional(pool)
        .await?)
    }

    pub async fn update(&self, pool: &SqlitePool) -> Result<()> {
        let id = self.id.to_string();
        let keys = serde_json::to_string(&self.keys)?;
        let role = self.role.to_string();

        query!(
            r#"
        update users
        set 
            display_name = ?1,
            keys = ?2,
            app_role = ?3
        where
            id = ?4
        "#,
            self.display_name,
            keys,
            role,
            id
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Every user as a lightweight summary for the admin list — role plus passkey
    /// count (the `keys` JSON array length) and live (non-revoked) API-key count.
    /// No passkey blobs.
    pub async fn list_summaries(pool: &SqlitePool) -> Result<Vec<UserSummary>> {
        Ok(query_as!(
            UserSummary,
            r#"
        SELECT
            u.display_name,
            u.id as "id: uuid::fmt::Hyphenated",
            u.app_role as "role: Role",
            json_array_length(u.keys) as "passkey_count!: i64",
            (SELECT COUNT(*) FROM api_keys k
             WHERE k.user_id = u.id AND k.revoked_at IS NULL) as "api_key_count!: i64"
        FROM users u
        ORDER BY u.display_name
        "#
        )
        .fetch_all(pool)
        .await?)
    }

    /// How many Admins exist — backs the lockout guard ("never remove the last
    /// admin").
    pub async fn count_admins(pool: &SqlitePool) -> Result<i64> {
        Ok(
            query!(r#"SELECT COUNT(*) as "count!: i64" FROM users WHERE app_role = 'Admin'"#)
                .fetch_one(pool)
                .await?
                .count,
        )
    }

    /// Set a user's role (Registered ↔ Admin). The last-admin guard lives in the
    /// handler, which has the target user + admin count in hand.
    pub async fn set_role(pool: &SqlitePool, id: &Uuid, role: Role) -> Result<()> {
        let id = id.to_string();
        let role = role.to_string();
        query!("UPDATE users SET app_role = ?1 WHERE id = ?2", role, id)
            .execute(pool)
            .await?;
        Ok(())
    }

    /// Delete a user. Their API keys reference `users(id)` (FK on), so wipe those
    /// first — both in one transaction; the passkeys live in the `keys` column and
    /// go with the row. (Cookie sessions are opaque, so the `refresh_session_role`
    /// middleware downgrades a deleted user to Anonymous on their next request.)
    pub async fn delete(pool: &SqlitePool, id: &Uuid) -> Result<()> {
        let id = id.to_string();
        let mut tx = pool.begin().await?;
        query!("DELETE FROM api_keys WHERE user_id = ?1", id)
            .execute(&mut *tx)
            .await?;
        query!("DELETE FROM users WHERE id = ?1", id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

impl Display for UserDao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "User name:{}, id:{}, role: {}",
            self.display_name, self.id, self.role
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    //Demo Passkey found here: https://github.com/kanidm/webauthn-rs/blob/master/webauthn-rp-proxy/tests/data/authenticate-start.json
    const SAMPLE_PASSKEY: &str = r#"{
      "cred": {
        "cred_id": "Abr4cz81v7rNJR7OnKUJeB297HaWkpwUeEPAWAGTkAWA62e0fw20tf6LDL6CWmsZ3yVse9Yw1tpXpNLK5q7e2Po",
        "cred": {
          "type_": "ES256",
          "key": {
            "EC_EC2": {
              "curve": "SECP256R1",
              "x": "HvbGIj6R0H5dnvpqNZwKacuF3KN18CdZKEPBLSrndao",
              "y": "bpEXmUfWwhW0XwIEREPdUr-RxBH-QIprEWFxSgki6Ms"
            }
          }
        },
        "counter": 0,
        "transports": null,
        "user_verified": true,
        "backup_eligible": false,
        "backup_state": false,
        "registration_policy": "required",
        "extensions": {
          "cred_protect": "Ignored",
          "hmac_create_secret": "NotRequested",
          "appid": "NotRequested",
          "cred_props": "Ignored"
        },
        "attestation": {
          "data": "None",
          "metadata": "None"
        },
        "attestation_format": "None"
      }
    }"#;

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn roundtrip(pool: SqlitePool) -> Result<()> {
        let passkey: Passkey = serde_json::from_str(SAMPLE_PASSKEY)?;

        let mut u = UserDao {
            display_name: "somebody".to_string(),
            id: Uuid::new_v4(),
            keys: sqlx::types::Json(vec![passkey]),
            role: Role::Registered,
        };

        u.create(&pool).await?;

        assert_eq!(u.role, Role::Admin); //First user gets admin

        let found_u = UserDao::find_by_uuid(&pool, &u.id)
            .await?
            .expect("We just inserted this value");

        assert_eq!(u, found_u);

        let mut u2 = UserDao {
            display_name: "somebody2".to_string(),
            id: Uuid::new_v4(),
            keys: sqlx::types::Json(vec![]),
            role: Role::Registered,
        };

        u2.create(&pool).await?;

        assert_eq!(u2.role, Role::Registered); //Second user just gets registered

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn passkey_lookup(pool: SqlitePool) -> Result<()> {
        let passkey: Passkey = serde_json::from_str(SAMPLE_PASSKEY)?;

        let mut u = UserDao {
            display_name: "somebody".to_string(),
            id: Uuid::new_v4(),
            keys: sqlx::types::Json(vec![passkey.clone()]),
            role: Role::Registered,
        };

        u.create(&pool).await?;

        let found_u = UserDao::find_by_passkey(&pool, &passkey).await?;

        assert_eq!(u, found_u.unwrap());

        Ok(())
    }

    /// Insert a user with an explicit role (bypasses the first-user→Admin rule so
    /// tests can set up multiple admins/registereds deterministically).
    async fn seed_user(pool: &SqlitePool, name: &str, role: Role) -> Result<UserDao> {
        let id = Uuid::now_v7();
        let id_str = id.to_string();
        let role_str = role.to_string();
        sqlx::query!(
            r#"INSERT INTO users (display_name, id, keys, app_role) VALUES (?1, ?2, '[]', ?3)"#,
            name,
            id_str,
            role_str,
        )
        .execute(pool)
        .await?;
        Ok(UserDao {
            display_name: name.to_string(),
            id,
            keys: sqlx::types::Json(vec![]),
            role,
        })
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn list_summaries_counts_passkeys_and_api_keys(pool: SqlitePool) -> Result<()> {
        use crate::db::dao::api_keys::ApiKeyDao;
        let passkey: Passkey = serde_json::from_str(SAMPLE_PASSKEY)?;

        // First user → Admin, with one passkey + two API keys (one revoked).
        let mut admin = UserDao {
            display_name: "aaa-admin".to_string(),
            id: Uuid::now_v7(),
            keys: sqlx::types::Json(vec![passkey]),
            role: Role::Registered,
        };
        admin.create(&pool).await?;
        assert_eq!(admin.role, Role::Admin);
        let (_k, live) = ApiKeyDao::create(&pool, &admin.id, "live").await?;
        let (_k2, revoked) = ApiKeyDao::create(&pool, &admin.id, "revoked").await?;
        ApiKeyDao::revoke(&pool, revoked.id, &admin.id).await?;
        let _ = live;

        // Second user → Registered, no passkeys, no keys.
        let _reg = seed_user(&pool, "bbb-reg", Role::Registered).await?;

        let summaries = UserDao::list_summaries(&pool).await?;
        assert_eq!(summaries.len(), 2);
        // Ordered by display_name: admin first.
        assert_eq!(summaries[0].display_name, "aaa-admin");
        assert_eq!(summaries[0].role, Role::Admin);
        assert_eq!(summaries[0].passkey_count, 1);
        assert_eq!(summaries[0].api_key_count, 1, "only the live key counts");
        assert_eq!(summaries[1].display_name, "bbb-reg");
        assert_eq!(summaries[1].passkey_count, 0);
        assert_eq!(summaries[1].api_key_count, 0);

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn count_admins_and_set_role(pool: SqlitePool) -> Result<()> {
        let admin = seed_user(&pool, "admin", Role::Admin).await?;
        let reg = seed_user(&pool, "reg", Role::Registered).await?;
        assert_eq!(UserDao::count_admins(&pool).await?, 1);

        UserDao::set_role(&pool, &reg.id, Role::Admin).await?;
        assert_eq!(UserDao::count_admins(&pool).await?, 2);

        UserDao::set_role(&pool, &admin.id, Role::Registered).await?;
        assert_eq!(UserDao::count_admins(&pool).await?, 1);
        assert_eq!(
            UserDao::find_by_uuid(&pool, &admin.id).await?.unwrap().role,
            Role::Registered
        );

        Ok(())
    }

    #[sqlx::test(migrator = "crate::db::database_handle::MIGRATOR")]
    async fn delete_cascades_api_keys(pool: SqlitePool) -> Result<()> {
        use crate::db::dao::api_keys::ApiKeyDao;
        let admin = seed_user(&pool, "admin", Role::Admin).await?;
        let victim = seed_user(&pool, "victim", Role::Registered).await?;
        ApiKeyDao::create(&pool, &victim.id, "k").await?;
        assert_eq!(ApiKeyDao::list_for_user(&pool, &victim.id).await?.len(), 1);

        UserDao::delete(&pool, &victim.id).await?;

        assert!(UserDao::find_by_uuid(&pool, &victim.id).await?.is_none());
        // The FK'd API key is gone too (no orphan / no FK violation).
        assert!(ApiKeyDao::list_for_user(&pool, &victim.id).await?.is_empty());
        // The other user is untouched.
        assert!(UserDao::find_by_uuid(&pool, &admin.id).await?.is_some());

        Ok(())
    }
}
