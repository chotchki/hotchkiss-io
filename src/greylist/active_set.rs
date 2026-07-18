//! The in-memory active greylist the request path checks (CX.5).
//!
//! The sweep maintains the `greylist` TABLE; after each pass it refreshes this SNAPSHOT, so the
//! enforcement middleware answers "is this IP greylisted?" from memory with no per-request DB
//! hit. It's a per-instance `Arc` (NOT a process global) so each test server is isolated —
//! seeding one test's set can't leak into another.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};

use crate::db::dao::greylist::GreylistEntry;

#[derive(Clone, Default, Debug)]
pub struct GreylistSet {
    inner: Arc<RwLock<HashSet<String>>>,
    /// The operator allowlist — the server's OWN public IP(s), fed from the
    /// `IpProviderService` broadcast (the same set that drives the Cloudflare DNS
    /// updates). The mini lives on the operator's home network, so its public IP IS
    /// the operator's browsing IP; it must NEVER be tolled or scored. Auto-maintained
    /// (follows a residential IP rotation), zero config — from any OTHER network the
    /// operator just authenticates (an authenticated session is never tolled). Phase DU.
    allow: Arc<RwLock<HashSet<String>>>,
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

    /// Set the operator allowlist to the server's current public IP(s) (Phase DU) — called
    /// by the coordinator's IP-broadcast subscriber whenever the tracked public IP changes.
    pub fn set_public_ips(&self, ips: &HashSet<IpAddr>) {
        *self.allow.write().unwrap() = ips.iter().map(|ip| ip.to_string()).collect();
    }

    /// Whether `ip` is on the operator allowlist (the server's own public IP). The `refresh`
    /// snapshot + `insert` NEVER cover this — it's derived from the IP broadcast, not the table.
    pub fn is_allowlisted(&self, ip: &str) -> bool {
        self.allow.read().unwrap().contains(ip)
    }

    /// The allowlisted operator IP(s), sorted — for the admin view (why an IP is never tolled).
    pub fn allowlisted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.allow.read().unwrap().iter().cloned().collect();
        v.sort();
        v
    }

    /// Whether `ip` is greylisted in the latest snapshot. A short read lock; no DB. The
    /// operator allowlist WINS over any entry (a snapshot row OR a manual pin) — defense in
    /// depth so the operator's own network can never be tolled even by a stale/pinned entry.
    pub fn is_greylisted(&self, ip: &str) -> bool {
        !self.is_allowlisted(ip) && self.inner.read().unwrap().contains(ip)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlisted_operator_ip_is_never_greylisted() {
        let set = GreylistSet::new();
        let op = "203.0.113.9"; // stand-in for the mini's own public IP
        let scanner = "198.51.100.4";

        // Both look greylisted in the snapshot (op as if a stale/pinned entry carried it).
        set.insert(op);
        set.insert(scanner);
        assert!(set.is_greylisted(scanner), "a normal greylisted IP is tolled");
        assert!(set.is_greylisted(op), "before allowlisting, even the operator IP would toll");

        // Allowlisting the operator IP WINS over the snapshot entry (defense in depth).
        set.set_public_ips(&HashSet::from(["203.0.113.9".parse::<IpAddr>().unwrap()]));
        assert!(set.is_allowlisted(op));
        assert!(!set.is_greylisted(op), "the allowlist wins over a snapshot/pinned entry");
        assert!(set.is_greylisted(scanner), "other IPs are unaffected");
        assert_eq!(set.allowlisted(), vec!["203.0.113.9".to_string()]);

        // A later IP rotation replaces the allowlist (old op IP is tolled again if still listed).
        set.set_public_ips(&HashSet::from(["203.0.113.55".parse::<IpAddr>().unwrap()]));
        assert!(!set.is_allowlisted(op));
        assert!(set.is_greylisted(op), "the previous public IP is no longer exempt after rotation");
    }
}
