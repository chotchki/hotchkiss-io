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

/// A validated URL slug — non-empty BY CONSTRUCTION (built from a title via
/// `slugify`). A `Slug` in a signature means the empty-slug check already happened:
/// there's no `""` slug value to guard against downstream, because the empty-title
/// guard IS `Slug::new` returning `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slug(String);

impl Slug {
    /// Slugify a title; `None` if it collapses to empty (no alphanumerics).
    pub fn new(title: &str) -> Option<Slug> {
        let s = slugify(title);
        (!s.is_empty()).then_some(Slug(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for Slug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
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

    #[test]
    fn slug_new_validates_non_empty() {
        assert_eq!(Slug::new("Hello World").unwrap().as_str(), "hello-world");
        assert!(Slug::new("").is_none());
        assert!(
            Slug::new("!!!").is_none(),
            "a title with no alphanumerics has no slug"
        );
    }
}
