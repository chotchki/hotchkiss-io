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
    /// The greylist bot-challenge state (Phase CX): the decoded toll image + the
    /// challenge server HMAC key. Shared read-only; its `Debug` redacts the key.
    pub challenge: crate::greylist::ChallengeState,
    /// The in-memory active greylist snapshot (Phase CX) the enforcement middleware
    /// checks each request against. Refreshed by the detection sweep; no per-request DB.
    pub greylist: crate::greylist::active_set::GreylistSet,
    /// The DNS resolver (shared with the ACME path). Only used by the admin "Run sweep now"
    /// action (Phase CX) to run a detection pass on demand, with FCrDNS crawler verification.
    pub resolver: hickory_resolver::TokioAsyncResolver,
    /// The dead-link scanner's shared runtime handle (Phase DL): the single-flight
    /// guard + last-run status the `/admin/dead-links` page shows and the "Run scan
    /// now" button triggers. Shared with the detached daily scan loop.
    pub dead_links: crate::deadlinks::DeadLinkScanState,
}
