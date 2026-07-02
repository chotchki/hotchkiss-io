//! `content_pages.page_category` as a lightweight comma-separated TAG list, with
//! `featured` reserved as the landing-page PIN (Phase 13.8).
//!
//! The field was fully plumbed (editor input, saved, in every query) but read by
//! nothing — so we lean on it instead of a migration. It stays a real taxonomy
//! field (`3d`, `web`, …); `featured` is just one reserved tag the pin button
//! toggles, so a page can be BOTH categorized and pinned (`"3d, featured"`). The
//! landing's Featured band = pages carrying the `featured` tag.

/// The reserved tag that pins a page to the landing's Featured band.
pub const FEATURED_TAG: &str = "featured";

/// Split a `page_category` value into trimmed, non-empty tags (original case
/// preserved). `None`/blank → no tags.
pub fn tags(category: Option<&str>) -> Vec<&str> {
    category
        .into_iter()
        .flat_map(|c| c.split(','))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

/// True iff the category carries the `featured` tag (case-insensitive, whole-tag
/// — `"featured-post"` does NOT count).
pub fn is_featured(category: Option<&str>) -> bool {
    tags(category).iter().any(|t| t.eq_ignore_ascii_case(FEATURED_TAG))
}

/// Toggle the `featured` tag, preserving every other tag. Returns the new category
/// string, or `None` when the result is empty (so the column clears to NULL rather
/// than holding `""`). Rejoins with `", "` — normalizes spacing, leaves tag text
/// intact.
pub fn toggle_featured(category: Option<&str>) -> Option<String> {
    let mut kept: Vec<&str> = tags(category)
        .into_iter()
        .filter(|t| !t.eq_ignore_ascii_case(FEATURED_TAG))
        .collect();
    // Was it featured? If not, pin it now (append so it reads last).
    if !is_featured(category) {
        kept.push(FEATURED_TAG);
    }
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_featured_matches_whole_tag_case_insensitively() {
        assert!(is_featured(Some("featured")));
        assert!(is_featured(Some("Featured")));
        assert!(is_featured(Some("3d, featured")));
        assert!(is_featured(Some("  3D ,  FEATURED  ")));
        assert!(!is_featured(Some("featured-post"))); // boundary: not a substring match
        assert!(!is_featured(Some("3d")));
        assert!(!is_featured(Some("")));
        assert!(!is_featured(None));
    }

    #[test]
    fn toggle_pins_and_unpins_preserving_other_tags() {
        // Pin from nothing.
        assert_eq!(toggle_featured(None).as_deref(), Some("featured"));
        // Pin alongside an existing tag (featured reads last).
        assert_eq!(toggle_featured(Some("3d")).as_deref(), Some("3d, featured"));
        // Unpin, keeping the taxonomy tag.
        assert_eq!(toggle_featured(Some("3d, featured")).as_deref(), Some("3d"));
        // Unpin the only tag → clears to None (NULL), not "".
        assert_eq!(toggle_featured(Some("featured")), None);
        // Case + whitespace tolerant, and dedups a doubled featured on unpin.
        assert_eq!(toggle_featured(Some(" Featured , featured ")), None);
        assert_eq!(toggle_featured(Some("web, Featured")).as_deref(), Some("web"));
    }
}
