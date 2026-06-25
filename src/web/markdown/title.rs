/// Remove a leading level-1 ATX heading (`# Title`) from markdown, if the very
/// first non-empty content is one. The page title is rendered separately (from
/// `ContentPageDao::display_title`), so a body that opens with `# Title` would
/// otherwise render the title twice. Only the FIRST line is considered, and only
/// when it is an H1 (`# `, not `## `), so in-body headings are untouched.
pub fn strip_leading_h1(markdown: &str) -> String {
    let trimmed = markdown.trim_start();
    match trimmed.strip_prefix("# ") {
        // Drop the heading line; keep the rest, minus the blank line after it.
        Some(rest) => match rest.split_once('\n') {
            Some((_, after)) => after.trim_start().to_string(),
            None => String::new(),
        },
        None => markdown.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_leading_h1_and_following_blank() {
        assert_eq!(
            strip_leading_h1("# My Title\n\nThe body."),
            "The body."
        );
        assert_eq!(strip_leading_h1("# My Title\nimmediately"), "immediately");
    }

    #[test]
    fn leaves_non_h1_alone() {
        // h2 first
        assert_eq!(
            strip_leading_h1("## Section\n\nbody"),
            "## Section\n\nbody"
        );
        // no heading
        assert_eq!(strip_leading_h1("Just prose."), "Just prose.");
        // missing space ("#Title") is not an ATX h1
        assert_eq!(strip_leading_h1("#NotAHeading"), "#NotAHeading");
    }

    #[test]
    fn in_body_h1_is_untouched() {
        let md = "Intro paragraph.\n\n# Later Heading\n\nmore";
        assert_eq!(strip_leading_h1(md), md);
    }

    #[test]
    fn title_only_yields_empty_body() {
        assert_eq!(strip_leading_h1("# Only a title"), "");
    }
}
