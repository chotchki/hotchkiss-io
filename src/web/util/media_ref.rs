//! Parse whatever an admin pastes into the editor's "Cover (media ref)" field
//! down to the token that actually resolves.
//!
//! The field says "media ref", but the media library gives you no bare-ref copy
//! button — its two per-item buttons hand you `![](/media/<ref>)` ("Copy ![]()")
//! and `/media/file/<url_key>` ("Copy link"). Feeding either straight into an
//! exact-match `find_by_ref` misses, so the cover silently never set (the bug
//! behind "setting a cover image for a project doesn't work"). This extracts the
//! token from any of those shapes so the natural copy-paste just works; the caller
//! then resolves an [`author ref`](MediaReference::Ref) via `media_ref` and a
//! [`file url_key`](MediaReference::UrlKey) via the variant table.

/// A media token pulled from a pasted cover value, tagged by which lookup resolves
/// it. Borrows from the input — no allocation.
#[derive(Debug, PartialEq, Eq)]
pub enum MediaReference<'a> {
    /// The author ref (`/media/<ref>` token) — resolves via `MediaDao::find_by_ref`.
    Ref(&'a str),
    /// The byte-URL HMAC key (`/media/file/<url_key>`) — resolves via
    /// `MediaVariantDao::find_by_url_key`.
    UrlKey(&'a str),
}

/// Extract the media token from a pasted cover value. Accepts, in order of
/// specificity: `.../media/file/<url_key>` (any prefix — bare path or full URL),
/// `.../media/<ref>`, or a bare token. Trailing markdown/query/fragment/quote
/// cruft (`)`, `?`, `#`, whitespace, quotes) is stripped. Returns `None` when
/// nothing token-like remains (empty / whitespace / a stray delimiter).
pub fn parse_cover_reference(raw: &str) -> Option<MediaReference<'_>> {
    let raw = raw.trim();

    if let Some(rest) = raw.rsplit_once("/media/file/") {
        return trim_token(rest.1).map(MediaReference::UrlKey);
    }
    if let Some(rest) = raw.rsplit_once("/media/") {
        return trim_token(rest.1).map(MediaReference::Ref);
    }
    // No `/media/` marker — treat the whole (delimiter-trimmed) value as a bare ref.
    trim_token(raw).map(MediaReference::Ref)
}

/// Take the leading run of valid token characters, rejecting an empty result.
/// A `media_ref` (UUIDv7 or a legacy slug) and a `url_key` (64 lowercase hex from
/// `media_url_key`) are both drawn from `[A-Za-z0-9_-]`, so anything else — the
/// closing `)` of an `![](...)` embed, a `?query`, a stray `!` from `![]()` — ends
/// the token. An allowlist beats a delimiter blocklist: no punctuation can leak in.
fn trim_token(s: &str) -> Option<&str> {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(s.len());
    let token = &s[..end];
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REF: &str = "0190aaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    #[test]
    fn extracts_from_the_copy_button_forms() {
        // "Copy ![]()" button output.
        assert_eq!(
            parse_cover_reference(&format!("![](/media/{REF})")),
            Some(MediaReference::Ref(REF))
        );
        // "Copy ![]()" with alt text.
        assert_eq!(
            parse_cover_reference(&format!("![a cover](/media/{REF})")),
            Some(MediaReference::Ref(REF))
        );
        // bare "/media/<ref>".
        assert_eq!(
            parse_cover_reference(&format!("/media/{REF}")),
            Some(MediaReference::Ref(REF))
        );
        // full URL.
        assert_eq!(
            parse_cover_reference(&format!("https://hotchkiss.io/media/{REF}")),
            Some(MediaReference::Ref(REF))
        );
        // "Copy link" button output — the byte URL, a url_key not a ref.
        assert_eq!(
            parse_cover_reference("https://hotchkiss.io/media/file/abc123key"),
            Some(MediaReference::UrlKey("abc123key"))
        );
        assert_eq!(
            parse_cover_reference("/media/file/abc123key"),
            Some(MediaReference::UrlKey("abc123key"))
        );
    }

    #[test]
    fn a_bare_ref_passes_through() {
        assert_eq!(parse_cover_reference(REF), Some(MediaReference::Ref(REF)));
        assert_eq!(parse_cover_reference("  slug-style-ref  "), Some(MediaReference::Ref("slug-style-ref")));
    }

    #[test]
    fn strips_query_and_fragment_cruft() {
        assert_eq!(
            parse_cover_reference(&format!("/media/{REF}?cb=123#frag")),
            Some(MediaReference::Ref(REF))
        );
    }

    #[test]
    fn nothing_token_like_is_none() {
        assert_eq!(parse_cover_reference(""), None);
        assert_eq!(parse_cover_reference("   "), None);
        assert_eq!(parse_cover_reference("/media/"), None);
        assert_eq!(parse_cover_reference("/media/file/"), None);
        assert_eq!(parse_cover_reference("![]()"), None);
    }
}
