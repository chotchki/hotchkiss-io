//! Behavioral greylisting — the cat-toll bot challenge (Phase CX).
//!
//! Design + rationale: `docs/greylist-challenge-design.md`. Persistence lives in
//! `crate::db::dao::greylist`; this module is the logic (detection, and — later — the
//! challenge kernel + image derivation).

pub mod active_set;
pub mod challenge;
pub mod crawler;
pub mod detection;
pub mod image;
pub mod sweep;

use std::sync::Arc;

use crate::db::dao::crypto_key::CryptoKey;
use crate::greylist::image::TollImage;

/// `crypto_keys` id for the challenge server key — 1 = session, 2 = media-URL HMAC, 3 = API-key
/// pepper, 4 = this. Auto-generated on first boot (64 random bytes, fine as an HMAC key). The
/// beta snapshot preserves only id 2, so beta regenerates its own id 4 — beta clearances don't
/// verify on prod and vice-versa, which is correct (separate hosts).
pub const CHALLENGE_KEY_ID: i64 = 4;

/// Everything the challenge endpoints need, shared read-only on `AppState`: the decoded toll
/// image (with its precomputed digest + version) and the server HMAC key. Cheap to clone (both
/// `Arc`). Its `Debug` REDACTS the key so it can't leak via an `AppState` debug-print.
#[derive(Clone)]
pub struct ChallengeState {
    pub toll: Arc<TollImage>,
    pub key: Arc<Vec<u8>>,
}

impl ChallengeState {
    pub async fn load(pool: &sqlx::SqlitePool) -> anyhow::Result<Self> {
        let toll = TollImage::shared();
        let key = CryptoKey::get_or_create(pool, CHALLENGE_KEY_ID)
            .await?
            .key_value;
        Ok(Self {
            toll,
            key: Arc::new(key),
        })
    }
}

impl std::fmt::Debug for ChallengeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChallengeState")
            .field("toll", &self.toll)
            .field("key", &"<redacted>")
            .finish()
    }
}
