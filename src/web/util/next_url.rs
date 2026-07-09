//! Post-login `?next` destination validation (Phase DE).
//!
//! The login flow stashes a validated `next` in the session and redirects there
//! after the WebAuthn ceremony. The validation is deliberately strict — this is
//! an open-redirect boundary: the value must be a SAME-SITE absolute path, and
//! browsers' WHATWG URL parsing treats `\` as `/`, so `/\evil.com` (which a
//! naive leading-`/`-plus-not-`//` check passes) navigates OFF-SITE. Rules:
//! starts with `/`, second char neither `/` nor `\`, and NO backslash anywhere.

/// Return the path if it's a safe same-site redirect target, else `None`.
pub fn safe_next(raw: &str) -> Option<&str> {
    let mut chars = raw.chars();
    if chars.next() != Some('/') {
        return None;
    }
    if matches!(chars.next(), Some('/') | Some('\\')) {
        return None;
    }
    if raw.contains('\\') {
        return None;
    }
    // Control bytes (the query decoder hands us %0d%0a as REAL CR/LF): a value
    // with CR/LF makes `Redirect::to` panic on the invalid header (a caught,
    // self-inflicted 500 — HeaderValue rejects the injection itself). Reject
    // the whole class here instead.
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return None;
    }
    Some(raw)
}

#[cfg(test)]
mod tests {
    use super::safe_next;

    #[test]
    fn accepts_site_paths() {
        assert_eq!(safe_next("/"), Some("/"));
        assert_eq!(safe_next("/library"), Some("/library"));
        assert_eq!(
            safe_next("/library/audiobooks?page=2"),
            Some("/library/audiobooks?page=2")
        );
    }

    #[test]
    fn rejects_offsite_and_parser_bypasses() {
        // Protocol-relative → off-site.
        assert_eq!(safe_next("//evil.com"), None);
        // The WHATWG-parser bypass: browsers treat \ as / — `/\evil.com`
        // navigates to evil.com despite the leading slash.
        assert_eq!(safe_next("/\\evil.com"), None);
        // Backslash ANYWHERE is rejected, not just position two.
        assert_eq!(safe_next("/lib\\rary"), None);
        assert_eq!(safe_next("https://evil.com"), None);
        assert_eq!(safe_next("evil.com"), None);
        assert_eq!(safe_next(""), None);
        // Control bytes → Redirect::to would panic on the header value.
        assert_eq!(safe_next("/a\r\nX-Inject: 1"), None);
        assert_eq!(safe_next("/a\nb"), None);
        assert_eq!(safe_next("/a\tb"), None);
    }
}
