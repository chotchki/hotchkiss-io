use sqlx::SqlitePool;
use tower_sessions_sqlx_store::SqliteStore;
use webauthn_rs::Webauthn;

#[derive(Clone, Debug)]
pub struct AppState {
    pub pool: SqlitePool,
    pub session_store: SqliteStore,
    pub webauthn: Webauthn,
    /// The registrable site host used to rewrite absolute same-site links to
    /// root-relative on save. Sourced from `webauthn_rp_id` (the registrable
    /// parent), NOT the served `domain`: on beta the served host is
    /// `beta.hotchkiss.io` while content links the canonical `hotchkiss.io`, so
    /// matching the parent relativizes those links on beta as well as prod.
    pub site_host: String,
}
