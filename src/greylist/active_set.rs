//! The in-memory active greylist the request path checks (CX.5).
//!
//! The sweep maintains the `greylist` TABLE; after each pass it refreshes this SNAPSHOT, so the
//! enforcement middleware answers "is this IP greylisted?" from memory with no per-request DB
//! hit. It's a per-instance `Arc` (NOT a process global) so each test server is isolated —
//! seeding one test's set can't leak into another.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use crate::db::dao::greylist::GreylistEntry;

#[derive(Clone, Default, Debug)]
pub struct GreylistSet {
    inner: Arc<RwLock<HashSet<String>>>,
}

impl GreylistSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the snapshot with the currently-active entries (called by the sweep after a pass).
    pub fn refresh(&self, entries: &[GreylistEntry]) {
        let set: HashSet<String> = entries.iter().map(|e| e.ip.clone()).collect();
        *self.inner.write().unwrap() = set;
    }

    /// Whether `ip` is greylisted in the latest snapshot. A short read lock; no DB.
    pub fn is_greylisted(&self, ip: &str) -> bool {
        self.inner.read().unwrap().contains(ip)
    }

    /// Add a single IP directly — used by tests and the admin manual-pin path to reflect a change
    /// immediately instead of waiting for the next sweep refresh.
    pub fn insert(&self, ip: &str) {
        self.inner.write().unwrap().insert(ip.to_string());
    }

    /// Remove an IP immediately (on admin release) so the un-toll takes effect without waiting
    /// for the next sweep refresh.
    pub fn remove(&self, ip: &str) {
        self.inner.write().unwrap().remove(ip);
    }
}
