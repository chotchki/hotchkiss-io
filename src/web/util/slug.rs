/// Turn a human title into a URL-safe slug: lowercase, non-alphanumeric runs
/// collapse to a single `-`, leading/trailing dashes trimmed. Mirrors the
/// client-side preview in the create forms so the server is authoritative.
///
/// `"How I Make AI Write Software I Trust"` -> `"how-i-make-ai-write-software-i-trust"`.
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in input.trim().chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            if pending_dash {
                out.push('-');
                pending_dash = false;
            }
            out.push(c);
        } else if !out.is_empty() {
            pending_dash = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basics() {
        assert_eq!(
            slugify("How I Make AI Write Software I Trust"),
            "how-i-make-ai-write-software-i-trust"
        );
        // collapses runs, trims edges, drops punctuation
        assert_eq!(slugify("  Hello,   World!!!  "), "hello-world");
        assert_eq!(slugify("already-a-slug"), "already-a-slug");
        // leading/trailing non-alnum don't leave stray dashes
        assert_eq!(slugify("--Edge--"), "edge");
        assert_eq!(slugify(""), "");
        assert_eq!(slugify("!!!"), "");
    }
}
