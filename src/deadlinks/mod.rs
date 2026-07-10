//! Dead-link checker (Phase DL). A daily background scan that flags rotted links
//! in the site's OWN content — surfaced on `/admin/dead-links` with a re-check
//! trigger. Design + rationale + the honest limits live in
//! `docs/dead-link-checker-design.md` (read it before touching this).
//!
//! Two design bets drive the whole thing: (1) **confirm-before-alarm** — an
//! external link is "confirmed dead" only after N consecutive daily failures, so a
//! transient 5xx/timeout doesn't cry wolf; (2) **internal links resolve
//! STRUCTURALLY** against the DB (does the row exist), never by HTTP-fetching our
//! own host — a role-gated / scheduled page correctly 404s an anonymous fetch, so
//! a self-fetch would false-positive a live-but-gated page as dead.

mod class;
mod classify;
mod dao;
mod external;
mod extract;
mod internal;
mod scan;

// Only the cross-module surface is re-exported here. Everything else (CheckClass,
// LinkKind, LinkTarget, the checker/extractor/resolver) is an implementation detail
// reached through these — callers use the values via method returns without naming
// the types.
pub use dao::{LinkCheckDao, LinkCheckRow, LinkRefDao};
pub use external::ReqwestChecker;
pub use scan::{recheck_one, spawn, trigger_now, DeadLinkScanState};
