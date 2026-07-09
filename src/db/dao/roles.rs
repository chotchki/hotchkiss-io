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
}
