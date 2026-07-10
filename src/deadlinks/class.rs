//! The shared verdict vocabulary — one `CheckClass` for internal + external
//! results, so the streak math + admin buckets are uniform.

use super::internal::InternalVerdict;

/// Which checking PATH a link takes — the persisted `link_check.kind`. Distinct
/// from `CheckClass` (the verdict): kind is fixed by the URL shape, class is the
/// outcome of checking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// A site path, resolved in-DB.
    Internal,
    /// An external host, HTTP-checked.
    External,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Internal => "internal",
            LinkKind::External => "external",
        }
    }

    /// Decode the stored form. Fails SAFE to `Internal` (the checked-in-DB, no
    /// outbound-HTTP path) on an unrecognized value.
    pub fn from_stored(s: &str) -> LinkKind {
        match s {
            "external" => LinkKind::External,
            _ => LinkKind::Internal,
        }
    }
}

/// What a single check concluded about a link.
///
/// - `Ok` — reachable (2xx, or a redirect that resolved to 2xx; an internal row
///   that exists). Resets the failure streak.
/// - `Dead` — definitively gone (404/410, DNS no-such-host, connection refused; an
///   internal target that doesn't resolve, or the `/projects/<slug>` dead-shape).
///   The ONLY class that "confirmed dead" is built from.
/// - `Transient` — probably-temporary (timeout, 429, 5xx, network flake). Advances
///   the streak but never becomes the confirmed LABEL.
/// - `Blocked` — the site rejects automated checks (401/403/405/451/999…). The link
///   likely works in a browser, so it's surfaced for MANUAL review, not called dead.
/// - `Unknown` — an internal route the resolver's hand-maintained map doesn't
///   recognize. Review, not dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckClass {
    Ok,
    Dead,
    Transient,
    Blocked,
    Unknown,
}

impl CheckClass {
    /// Stored form (the `link_check.last_class` text column).
    pub fn as_str(self) -> &'static str {
        match self {
            CheckClass::Ok => "ok",
            CheckClass::Dead => "dead",
            CheckClass::Transient => "transient",
            CheckClass::Blocked => "blocked",
            CheckClass::Unknown => "unknown",
        }
    }

    /// Decode the stored form. An unrecognized value fails SAFE to `Unknown`
    /// (a "review", never a false "dead").
    pub fn from_stored(s: &str) -> CheckClass {
        match s {
            "ok" => CheckClass::Ok,
            "dead" => CheckClass::Dead,
            "transient" => CheckClass::Transient,
            "blocked" => CheckClass::Blocked,
            _ => CheckClass::Unknown,
        }
    }

    /// Does this class advance the consecutive-failure streak? `Dead` and
    /// `Transient` do; `Ok` resets it; `Blocked`/`Unknown` are orthogonal review
    /// states that leave the streak untouched (we couldn't determine liveness).
    pub fn counts_as_failure(self) -> bool {
        matches!(self, CheckClass::Dead | CheckClass::Transient)
    }
}

impl From<InternalVerdict> for CheckClass {
    fn from(v: InternalVerdict) -> Self {
        match v {
            InternalVerdict::Ok => CheckClass::Ok,
            InternalVerdict::Dead => CheckClass::Dead,
            InternalVerdict::Unknown => CheckClass::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_roundtrips_and_fails_safe() {
        for c in [
            CheckClass::Ok,
            CheckClass::Dead,
            CheckClass::Transient,
            CheckClass::Blocked,
            CheckClass::Unknown,
        ] {
            assert_eq!(CheckClass::from_stored(c.as_str()), c);
        }
        // Garbage decodes to Unknown (review), never Dead.
        assert_eq!(CheckClass::from_stored("garbage"), CheckClass::Unknown);
    }

    #[test]
    fn link_kind_stored_roundtrips_and_fails_safe() {
        assert_eq!(LinkKind::from_stored(LinkKind::Internal.as_str()), LinkKind::Internal);
        assert_eq!(LinkKind::from_stored(LinkKind::External.as_str()), LinkKind::External);
        assert_eq!(LinkKind::from_stored("garbage"), LinkKind::Internal);
    }

    #[test]
    fn only_dead_and_transient_count_as_failure() {
        assert!(CheckClass::Dead.counts_as_failure());
        assert!(CheckClass::Transient.counts_as_failure());
        assert!(!CheckClass::Ok.counts_as_failure());
        assert!(!CheckClass::Blocked.counts_as_failure());
        assert!(!CheckClass::Unknown.counts_as_failure());
    }
}
