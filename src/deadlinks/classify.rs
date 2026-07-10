//! DL.2 — classify a raw content URL into what the scanner does with it.

use url::Url;

use super::class::LinkKind;

/// Why a link isn't checkable — the closed set of skip cases (typed so a reason
/// can't be a typo'd magic string, and so a caller can branch on it later).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Empty,
    Anchor,
    ProtocolRelative,
    Mailto,
    Tel,
    Data,
    JavaScript,
    OtherScheme,
    Malformed,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::Empty => "empty",
            SkipReason::Anchor => "anchor",
            SkipReason::ProtocolRelative => "protocol-relative",
            SkipReason::Mailto => "mailto",
            SkipReason::Tel => "tel",
            SkipReason::Data => "data",
            SkipReason::JavaScript => "javascript",
            SkipReason::OtherScheme => "other-scheme",
            SkipReason::Malformed => "relative-or-malformed",
        }
    }
}

/// What the scanner does with a link URL pulled from content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A root-relative site path — resolved in-DB (existence, NOT visibility), no
    /// HTTP. Carries the path (query/fragment included; the resolver strips them).
    Internal(String),
    /// An external http(s) URL — HTTP-checked (HEAD→GET).
    External(String),
    /// Not checkable (mailto/tel/#anchor/data/javascript/protocol-relative/malformed).
    Skip(SkipReason),
}

impl LinkTarget {
    /// The persisted `LinkKind` for a checkable target; `None` for a skip (skips
    /// aren't recorded — they're just excluded from checking).
    pub fn kind(&self) -> Option<LinkKind> {
        match self {
            LinkTarget::Internal(_) => Some(LinkKind::Internal),
            LinkTarget::External(_) => Some(LinkKind::External),
            LinkTarget::Skip(_) => None,
        }
    }
}

/// Classify a raw link URL against the site host. Internal links in stored content
/// are already root-relative (`rewrite_site_links` runs on save), but a same-site
/// ABSOLUTE (imported or not-yet-resaved content) folds to `Internal` too — a link
/// is NEVER HTTP-fetched against our own host, because gated/scheduled content
/// correctly 404s an anonymous fetch and would read as a false "dead". Internal
/// always resolves in-DB.
pub fn classify(raw: &str, site_host: &str) -> LinkTarget {
    let url = raw.trim();
    if url.is_empty() {
        return LinkTarget::Skip(SkipReason::Empty);
    }
    // Same-page fragment — nothing to resolve.
    if url.starts_with('#') {
        return LinkTarget::Skip(SkipReason::Anchor);
    }
    if url.starts_with('/') {
        // Protocol-relative `//host/…` is an ABSOLUTE URL, not a root-relative path.
        if url.starts_with("//") {
            return LinkTarget::Skip(SkipReason::ProtocolRelative);
        }
        return LinkTarget::Internal(url.to_string());
    }
    // Absolute URL: classify by scheme, then (for http) by host.
    let Ok(parsed) = Url::parse(url) else {
        // Not root-relative and not a valid absolute URL. Stored content is one or
        // the other, so this is malformed for our corpus (a bare relative path, junk).
        return LinkTarget::Skip(SkipReason::Malformed);
    };
    match parsed.scheme() {
        "http" | "https" => {
            let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
            let site = site_host.to_ascii_lowercase();
            if host == site || host == format!("www.{site}") {
                // Same-site absolute → fold to the root-relative form + resolve in-DB.
                let mut path = parsed.path().to_string();
                if let Some(q) = parsed.query() {
                    path.push('?');
                    path.push_str(q);
                }
                if let Some(f) = parsed.fragment() {
                    path.push('#');
                    path.push_str(f);
                }
                LinkTarget::Internal(path)
            } else {
                // Keep the raw external URL (stable identity for link_check/ref +
                // display), not a re-serialized `parsed.to_string()`.
                LinkTarget::External(url.to_string())
            }
        }
        "mailto" => LinkTarget::Skip(SkipReason::Mailto),
        "tel" => LinkTarget::Skip(SkipReason::Tel),
        "data" => LinkTarget::Skip(SkipReason::Data),
        "javascript" => LinkTarget::Skip(SkipReason::JavaScript),
        _ => LinkTarget::Skip(SkipReason::OtherScheme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const HOST: &str = "hotchkiss.io";

    #[test]
    fn root_relative_is_internal() {
        assert_eq!(
            classify("/blog/foo", HOST),
            LinkTarget::Internal("/blog/foo".to_string())
        );
        assert_eq!(
            classify("/media/abc?x=1#frag", HOST),
            LinkTarget::Internal("/media/abc?x=1#frag".to_string())
        );
    }

    #[test]
    fn external_http_is_external() {
        assert_eq!(
            classify("https://github.com/chotchki/recon-gen", HOST),
            LinkTarget::External("https://github.com/chotchki/recon-gen".to_string())
        );
    }

    #[test]
    fn same_site_absolute_folds_to_internal() {
        // A self-fetch would apply the gate and lie, so a same-site absolute is
        // resolved in-DB like any root-relative link — path + query + fragment.
        assert_eq!(
            classify("https://hotchkiss.io/blog/x?p=2#top", HOST),
            LinkTarget::Internal("/blog/x?p=2#top".to_string())
        );
        assert_eq!(
            classify("https://www.hotchkiss.io/about", HOST),
            LinkTarget::Internal("/about".to_string())
        );
        assert_eq!(
            classify("https://hotchkiss.io:8443/resume", HOST),
            LinkTarget::Internal("/resume".to_string())
        );
    }

    #[test]
    fn non_checkable_schemes_and_fragments_skip() {
        assert_eq!(classify("mailto:chris@hotchkiss.io", HOST), LinkTarget::Skip(SkipReason::Mailto));
        assert_eq!(classify("tel:+15551234567", HOST), LinkTarget::Skip(SkipReason::Tel));
        assert_eq!(classify("#section", HOST), LinkTarget::Skip(SkipReason::Anchor));
        assert_eq!(classify("data:image/png;base64,AAAA", HOST), LinkTarget::Skip(SkipReason::Data));
        assert_eq!(classify("javascript:void(0)", HOST), LinkTarget::Skip(SkipReason::JavaScript));
        assert_eq!(classify("ftp://x.example/f", HOST), LinkTarget::Skip(SkipReason::OtherScheme));
    }

    #[test]
    fn protocol_relative_is_skipped_not_internal() {
        // "//host/…" starts with '/' but is an absolute URL, not a site path.
        assert_eq!(
            classify("//evil.example/x", HOST),
            LinkTarget::Skip(SkipReason::ProtocolRelative)
        );
    }

    #[test]
    fn empty_and_bare_relative_skip() {
        assert_eq!(classify("   ", HOST), LinkTarget::Skip(SkipReason::Empty));
        assert_eq!(
            classify("relative/path/no/leading/slash", HOST),
            LinkTarget::Skip(SkipReason::Malformed)
        );
    }

    #[test]
    fn target_kind_maps_to_link_kind() {
        assert_eq!(classify("/blog/x", HOST).kind(), Some(LinkKind::Internal));
        assert_eq!(classify("https://ext.example/", HOST).kind(), Some(LinkKind::External));
        assert_eq!(classify("mailto:a@b.c", HOST).kind(), None);
    }
}
