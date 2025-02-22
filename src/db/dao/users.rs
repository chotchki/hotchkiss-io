use super::roles::Role;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{query, query_as, sqlite::SqliteRow, Error::ColumnDecode, FromRow, Row, SqlitePool};
use std::{fmt::Display, str::FromStr};
use uuid::Uuid;
use webauthn_rs::prelude::Passkey;

#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Serialize)]
pub struct UserDao {
    pub display_name: String,
    pub id: Uuid,
    pub keys: sqlx::types::Json<Vec<Passkey>>,
    pub role: Role,
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
}
