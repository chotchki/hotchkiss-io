use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString};

/// User trust tiers, TEXT-persisted by variant name (strum) in `users.app_role`
/// and the `min_role` visibility columns.
///
/// NEVER `derive(Ord)`/`derive(PartialOrd)` here: the variants are declared
/// ALPHABETICALLY (a derived order would rank `Admin` below `Anonymous`). The
/// trust ladder is the explicit `rank()` below — compare roles only through it.
#[derive(
    Clone,
    Copy,
    Debug,
    Display,
    Deserialize,
    Eq,
    EnumIter,
    EnumString,
    PartialEq,
    Serialize,
    sqlx::Type,
)]
pub enum Role {
    Admin,
    Anonymous,
    Family,
    Registered,
}

impl Role {
    /// The trust ladder: Anonymous 0 < Registered 1 < Family 2 < Admin 3.
    /// `Family` is the promotion-only household tier between "has logged in"
    /// and "runs the site" (docs/library-design.md). A higher rank passes every
    /// gate a lower rank passes (`viewer.rank() >= required.rank()`).
    pub fn rank(self) -> u8 {
        match self {
            Role::Anonymous => 0,
            Role::Registered => 1,
            Role::Family => 2,
            Role::Admin => 3,
        }
    }
}

/// A content/media visibility gate — the typed form of the `min_role` TEXT column
/// (`content_pages.min_role`, `media.min_role`). `None` (public) is the ONLY public
/// spelling; a recognized gate role gates to its rank; EVERYTHING else — a manual DB
/// edit, a future role after a rollback, the literal `"Admin"`, the unsanctioned
/// `"Anonymous"` — decodes FAIL-CLOSED to Admin-only (hiding content on a value we
/// don't understand is recoverable; leaking it is not).
///
/// This is the ONE decode (DJ.1): the twin `min_role_rank` / `visibility_label`
/// methods on `ContentPageDao` + `MediaDao` and the write tri-state all delegate
/// here. The SQL CASE in the paged / byte-serve queries stays LITERAL (sqlx
/// `query!` needs literal SQL) and the parity tests keep it pinned to `rank()`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MinRole(Option<Role>);

impl MinRole {
    /// Fail-closed decode of the stored `min_role` string.
    pub fn from_stored(raw: Option<&str>) -> MinRole {
        match raw {
            None => MinRole(None),
            Some("Registered") => MinRole(Some(Role::Registered)),
            Some("Family") => MinRole(Some(Role::Family)),
            // "Admin" AND every unrecognized value — fail-closed to the top.
            Some(_) => MinRole(Some(Role::Admin)),
        }
    }

    /// The stored string form: `None` for public, else the gate role's name.
    pub fn to_stored(self) -> Option<String> {
        self.0.map(|r| r.to_string())
    }

    /// The required rank: public 0, Registered 1, Family 2, Admin 3.
    pub fn rank(self) -> u8 {
        self.0.map(Role::rank).unwrap_or(0)
    }

    /// May `viewer` pass this gate? (`viewer.rank() >= required`.)
    pub fn is_visible_to(self, viewer: Role) -> bool {
        viewer.rank() >= self.rank()
    }

    /// The badge / selector label from the decode — `None` = public (no badge),
    /// else `"Registered"` / `"Family"` / `"Admin-only"` (a fail-closed value reads
    /// as "Admin-only", never its own raw text).
    pub fn label(self) -> Option<&'static str> {
        match self.0 {
            None => None,
            Some(Role::Registered) => Some("Registered"),
            Some(Role::Family) => Some("Family"),
            _ => Some("Admin-only"),
        }
    }

    /// Apply an authoring value to the CURRENT gate: `"Public"` clears it, a known
    /// gate role sets it, and anything else — an ABSENT field (`None`) or an
    /// unrecognized value — KEEPS the current gate. Never silently loosens.
    pub fn apply_write(self, wire: Option<&str>) -> MinRole {
        match wire {
            Some("Public") => MinRole(None),
            Some("Registered") => MinRole(Some(Role::Registered)),
            Some("Family") => MinRole(Some(Role::Family)),
            Some("Admin") => MinRole(Some(Role::Admin)),
            _ => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    /// Pins the FULL ladder via `Role::iter()`: a new variant fails the count
    /// assertion until it's deliberately placed here, and a rank edit fails the
    /// per-variant pin — the ladder can't drift silently.
    #[test]
    fn rank_ladder_is_pinned() {
        let expected = [
            (Role::Anonymous, 0),
            (Role::Registered, 1),
            (Role::Family, 2),
            (Role::Admin, 3),
        ];
        assert_eq!(
            Role::iter().count(),
            expected.len(),
            "new Role variant — place it on the rank() ladder AND in this test"
        );
        for (role, rank) in expected {
            assert_eq!(role.rank(), rank, "{role} rank drifted");
        }
    }

    /// Every variant's rank is unique — two roles must never tie (a tie would
    /// make `>=` gates treat them as interchangeable).
    #[test]
    fn ranks_are_unique() {
        let mut ranks: Vec<u8> = Role::iter().map(Role::rank).collect();
        ranks.sort_unstable();
        ranks.dedup();
        assert_eq!(ranks.len(), Role::iter().count());
    }

    /// Admin is THE TOP of the ladder — the fail-closed catch-alls (the
    /// `min_role` decode's `Role::Admin.rank()` arm and the SQL CASE's
    /// `else 3`) mean "maximum restriction". If a variant ever outranked
    /// Admin, "unknown value" would silently decode to a MIDDLE tier — a
    /// leak. Renumbering the ladder means revisiting every catch-all first.
    #[test]
    fn admin_is_the_top_rank() {
        assert_eq!(
            Role::Admin.rank(),
            Role::iter().map(Role::rank).max().unwrap(),
            "Admin must outrank every variant — see the fail-closed catch-alls"
        );
    }

    #[test]
    fn min_role_decode_is_fail_closed() {
        assert_eq!(MinRole::from_stored(None).rank(), 0);
        assert_eq!(MinRole::from_stored(Some("Registered")).rank(), 1);
        assert_eq!(MinRole::from_stored(Some("Family")).rank(), 2);
        assert_eq!(MinRole::from_stored(Some("Admin")).rank(), 3);
        // Unknown / Anonymous / garbage all rank as Admin (top) — never public.
        assert_eq!(
            MinRole::from_stored(Some("Anonymous")).rank(),
            Role::Admin.rank()
        );
        assert_eq!(MinRole::from_stored(Some("wat")).rank(), Role::Admin.rank());
    }

    #[test]
    fn min_role_label_tracks_the_decode() {
        assert_eq!(MinRole::from_stored(None).label(), None);
        assert_eq!(
            MinRole::from_stored(Some("Registered")).label(),
            Some("Registered")
        );
        assert_eq!(MinRole::from_stored(Some("Family")).label(), Some("Family"));
        assert_eq!(MinRole::from_stored(Some("Admin")).label(), Some("Admin-only"));
        // A garbage value reads as Admin-only, never its own text.
        assert_eq!(
            MinRole::from_stored(Some("garbage")).label(),
            Some("Admin-only")
        );
    }

    #[test]
    fn min_role_apply_write_is_public_keep_set() {
        let family = MinRole::from_stored(Some("Family"));
        assert_eq!(family.apply_write(Some("Public")).to_stored(), None); // clear
        assert_eq!(
            family.apply_write(None).to_stored().as_deref(),
            Some("Family") // keep (absent)
        );
        assert_eq!(
            family.apply_write(Some("bogus")).to_stored().as_deref(),
            Some("Family") // keep (unrecognized — never silently loosens)
        );
        assert_eq!(
            family.apply_write(Some("Registered")).to_stored().as_deref(),
            Some("Registered") // set
        );
    }

    #[test]
    fn min_role_gates_by_rank() {
        let family = MinRole::from_stored(Some("Family"));
        assert!(!family.is_visible_to(Role::Registered));
        assert!(family.is_visible_to(Role::Family));
        assert!(family.is_visible_to(Role::Admin));
        assert!(MinRole::from_stored(None).is_visible_to(Role::Anonymous));
    }
}
