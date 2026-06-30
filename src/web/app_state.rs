use crate::media::MediaStore;
use sqlx::SqlitePool;
use std::path::PathBuf;
use tower_sessions_sqlx_store::SqliteStore;
use webauthn_rs::Webauthn;

#[derive(Clone, Debug)]
pub struct AppState {
    pub pool: SqlitePool,
    pub session_store: SqliteStore,
    pub webauthn: Webauthn,
    /// Content-addressed disk store for large media (Phase BZ). Handlers store
    /// uploads here and serve bytes from it via the range route.
    pub media_store: MediaStore,
    /// The registrable site host used to rewrite absolute same-site links to
    /// root-relative on save. Sourced from `webauthn_rp_id` (the registrable
    /// parent), NOT the served `domain`: on beta the served host is
    /// `beta.hotchkiss.io` while content links the canonical `hotchkiss.io`, so
    /// matching the parent relativizes those links on beta as well as prod.
    pub site_host: String,
    /// Directory holding the rolling app logs (`hotchkiss.io.log*`), so the
    /// `/admin/logs` viewer (Phase CO) can tail the newest one. From
    /// `Settings.log_path`.
    pub log_path: PathBuf,
}
