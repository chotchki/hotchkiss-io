//! The media token types + the ONE parse path that turns whatever an admin pastes
//! into the editor's "Cover (media ref)" field into a token that actually resolves.
//!
//! Two distinct string kinds get conflated as bare `&str` everywhere else: a
//! [`MediaRef`] (the opaque `/media/<ref>` author token) and a [`UrlKey`] (the
//! `/media/file/<url_key>` HMAC-SHA256 byte-URL key). DJ.4 gives each a borrowed
//! newtype whose CONSTRUCTOR is the format gate — holding a `UrlKey` means the
//! 64-hex check already passed — and folds the cover-paste shape-splitter
//! (`parse_cover_reference`) into a single canonical parse that yields them.
//!
//! The field says "media ref", but the media library gives you no bare-ref copy
//! button — its two per-item buttons hand you `![](/media/<ref>)` ("Copy ![]()")
//! and `/media/file/<url_key>` ("Copy link"). Feeding either straight into an
//! exact-match `find_by_ref` misses, so the cover silently never set (the bug
//! behind "setting a cover image for a project doesn't work"). This extracts the
//! token from any of those shapes so the natural copy-paste just works.

use std::fmt;

/// An author media token — the `/media/<ref>` / `![](/media/<ref>)` form. Opaque:
/// a `Uuid::now_v7().simple()` for new uploads, a legacy slug for pre-BZ content,
/// so the ONLY invariant is the token charset `[A-Za-z0-9_-]` (a UUID-shaped parse
/// would 404 every legacy embed). Resolves via `MediaDao::find_by_ref`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaRef<'a>(&'a str);

impl<'a> MediaRef<'a> {
    /// The leading `[A-Za-z0-9_-]` run of `s`; `None` if that run is empty. This is
    /// the only token invariant — charset, never shape — so slug refs still resolve.
    pub fn parse(s: &'a str) -> Option<Self> {
        trim_token(s).map(MediaRef)
    }

    pub fn as_str(&self) -> &str {
        self.0
    }
}

impl fmt::Display for MediaRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A media byte-URL token — the `/media/file/<url_key>` form, EXACTLY 64 lowercase
/// hex (HMAC-SHA256). The type GUARANTEES `is_sha256_hex`: holding a `UrlKey` means
/// the format gate already passed (no `../` traversal, no short slice, no
/// uppercase). Resolves via `MediaVariantDao::find_by_url_key{,_with_required_rank}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrlKey<'a>(&'a str);

impl<'a> UrlKey<'a> {
    /// STRICT — the WHOLE string must be 64 lowercase hex. This is the single
    /// format gate the serve route depends on (blocks `../`, short slices).
    pub fn parse(s: &'a str) -> Option<Self> {
        crate::media::is_sha256_hex(s).then_some(UrlKey(s))
    }

    /// From a pasted-URL tail: strip trailing token cruft FIRST (a `)`, `?query`),
    /// then hex-validate. A non-hex tail is `None` (not a bogus key).
    fn parse_token(s: &'a str) -> Option<Self> {
        trim_token(s).and_then(Self::parse)
    }

    pub fn as_str(&self) -> &str {
        self.0
    }
}

impl fmt::Display for UrlKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A media token pulled from a pasted cover value, tagged by which lookup resolves
/// it. Borrows from the input — no allocation. This is THE canonical media-token
/// parse (DJ.4): the variants carry the format-validated newtypes.
#[derive(Debug, PartialEq, Eq)]
pub enum MediaReference<'a> {
    /// The author ref — resolves via `MediaDao::find_by_ref`.
    Ref(MediaRef<'a>),
    /// The byte-URL HMAC key — resolves via `MediaVariantDao::find_by_url_key`.
    UrlKey(UrlKey<'a>),
}

/// Extract the media token from a pasted cover value. Accepts, in order of
/// specificity: `.../media/file/<url_key>` (any prefix — bare path or full URL),
/// `.../media/<ref>`, or a bare token. Trailing markdown/query/fragment/quote cruft
/// (`)`, `?`, `#`, whitespace) is stripped. Returns `None` when nothing token-like
/// remains — OR, for the `file` shape, when the tail isn't a valid 64-hex `UrlKey`
/// (a malformed file URL is left alone, NOT fudged into a bogus `Ref` — DJ.4).
pub fn parse_cover_reference(raw: &str) -> Option<MediaReference<'_>> {
    let raw = raw.trim();

    if let Some((_, rest)) = raw.rsplit_once("/media/file/") {
        // A /media/file/<tail> that isn't 64-hex is a malformed byte URL → None
        // (leave the cover alone), never a fall-through to a bogus Ref.
        return UrlKey::parse_token(rest).map(MediaReference::UrlKey);
    }
    if let Some((_, rest)) = raw.rsplit_once("/media/") {
        return MediaRef::parse(rest).map(MediaReference::Ref);
    }
    // No `/media/` marker — treat the whole (delimiter-trimmed) value as a bare ref.
    MediaRef::parse(raw).map(MediaReference::Ref)
}

/// Take the leading run of valid token characters, rejecting an empty result.
/// A `media_ref` (UUIDv7 or a legacy slug) and a `url_key` (64 lowercase hex) are
/// both drawn from `[A-Za-z0-9_-]`, so anything else — the closing `)` of an
/// `![](...)` embed, a `?query`, a stray `!` — ends the token. An allowlist beats a
/// delimiter blocklist: no punctuation can leak in. Reached ONLY through the
/// newtype constructors.
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
    /// A real 64-lowercase-hex url_key (what `media_url_key` actually emits).
    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn media_ref(s: &str) -> MediaReference<'_> {
        MediaReference::Ref(MediaRef::parse(s).unwrap())
    }
    fn url_key(s: &str) -> MediaReference<'_> {
        MediaReference::UrlKey(UrlKey::parse(s).unwrap())
    }

    #[test]
    fn extracts_from_the_copy_button_forms() {
        // "Copy ![]()" button output.
        assert_eq!(
            parse_cover_reference(&format!("![](/media/{REF})")),
            Some(media_ref(REF))
        );
        // "Copy ![]()" with alt text.
        assert_eq!(
            parse_cover_reference(&format!("![a cover](/media/{REF})")),
            Some(media_ref(REF))
        );
        // bare "/media/<ref>".
        assert_eq!(parse_cover_reference(&format!("/media/{REF}")), Some(media_ref(REF)));
        // full URL.
        assert_eq!(
            parse_cover_reference(&format!("https://hotchkiss.io/media/{REF}")),
            Some(media_ref(REF))
        );
        // "Copy link" button output — the byte URL, a url_key not a ref.
        assert_eq!(
            parse_cover_reference(&format!("https://hotchkiss.io/media/file/{KEY}")),
            Some(url_key(KEY))
        );
        assert_eq!(parse_cover_reference(&format!("/media/file/{KEY}")), Some(url_key(KEY)));
    }

    #[test]
    fn a_bare_ref_passes_through() {
        assert_eq!(parse_cover_reference(REF), Some(media_ref(REF)));
        assert_eq!(parse_cover_reference("  slug-style-ref  "), Some(media_ref("slug-style-ref")));
    }

    #[test]
    fn strips_query_and_fragment_cruft() {
        assert_eq!(
            parse_cover_reference(&format!("/media/{REF}?cb=123#frag")),
            Some(media_ref(REF))
        );
        // A url_key with trailing cruft still validates after the token trim.
        assert_eq!(
            parse_cover_reference(&format!("/media/file/{KEY}?cb=9")),
            Some(url_key(KEY))
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

    #[test]
    fn file_url_with_non_hex_tail_is_none_not_a_bogus_ref() {
        // DJ.4 tightening: a /media/file/<non-hex> can't be a real url_key (the
        // route would 404 it), so it resolves to None (leave the cover alone),
        // NOT a fudged Ref("abc123key") that would also miss but muddy intent.
        assert_eq!(parse_cover_reference("/media/file/abc123key"), None);
        assert_eq!(parse_cover_reference("https://hotchkiss.io/media/file/nothex"), None);
    }

    #[test]
    fn url_key_parse_is_the_64_hex_gate() {
        assert!(UrlKey::parse(KEY).is_some());
        assert!(UrlKey::parse("tooshort").is_none());
        assert!(UrlKey::parse(&"A".repeat(64)).is_none(), "uppercase is rejected");
        assert!(UrlKey::parse("../../etc/passwd").is_none(), "traversal is rejected");
    }
}
