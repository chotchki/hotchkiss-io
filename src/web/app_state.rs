use sqlx::SqlitePool;
use tower_sessions_sqlx_store::SqliteStore;
use webauthn_rs::Webauthn;

#[derive(Clone, Debug)]
pub struct AppState {
    pub pool: SqlitePool,
    pub session_store: SqliteStore,
    pub webauthn: Webauthn,
}
