use markdown::mdast::Node;
use markdown::to_mdast;

const MAX_CHARS: usize = 200;

/// Extract the first paragraph of `markdown` as plain text, truncated to
/// ~200 chars with "…" appended on overflow. Returns an empty string when
/// no paragraph with visible text exists (e.g. all-image lead, code/heading
/// only, empty input).
pub fn excerpt(markdown: &str) -> String {
    let Ok(ast) = to_mdast(markdown, &Default::default()) else {
        return String::new();
    };

    let Node::Root(root) = ast else {
        return String::new();
    };

    for child in &root.children {
        if let Node::Paragraph(p) = child {
            let text = collect_text(&p.children);
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return truncate(trimmed, MAX_CHARS);
            }
        }
    }
    String::new()
}

fn collect_text(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text(t) => out.push_str(&t.value),
            Node::InlineCode(c) => out.push_str(&c.value),
            Node::Emphasis(e) => out.push_str(&collect_text(&e.children)),
            Node::Strong(s) => out.push_str(&collect_text(&s.children)),
            Node::Link(l) => out.push_str(&collect_text(&l.children)),
            Node::Delete(d) => out.push_str(&collect_text(&d.children)),
            Node::Break(_) => out.push(' '),
            _ => {}
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let head: String = chars[..max].iter().collect();
        format!("{}…", head.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(excerpt(""), "");
    }

    #[test]
    fn first_paragraph_plain_text() {
        let md = "Hello world.\n\nSecond paragraph.";
        assert_eq!(excerpt(md), "Hello world.");
    }

    #[test]
    fn leading_image_only_is_skipped() {
        let md = "![cover](/cover.jpg)\n\nReal content here.";
        assert_eq!(excerpt(md), "Real content here.");
    }

    #[test]
    fn leading_heading_is_skipped() {
        let md = "# My Title\n\nThe body of the post.";
        assert_eq!(excerpt(md), "The body of the post.");
    }

    #[test]
    fn leading_code_block_is_skipped() {
        let md = "```rust\nfn main() {}\n```\n\nProse after code.";
        assert_eq!(excerpt(md), "Prose after code.");
    }

    #[test]
    fn inline_formatting_is_stripped() {
        let md = "Text with **bold** and *italic* and `code` and [link](http://x).";
        assert_eq!(
            excerpt(md),
            "Text with bold and italic and code and link."
        );
    }

    #[test]
    fn very_long_paragraph_is_truncated_with_ellipsis() {
        let long = "word ".repeat(60);
        let result = excerpt(&long);
        assert!(result.chars().count() <= MAX_CHARS + 1);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn at_threshold_no_ellipsis() {
        let body = "x".repeat(MAX_CHARS);
        let result = excerpt(&body);
        assert_eq!(result.chars().count(), MAX_CHARS);
        assert!(!result.ends_with('…'));
    }

    #[test]
    fn over_threshold_by_one_truncates() {
        let body = "x".repeat(MAX_CHARS + 1);
        let result = excerpt(&body);
        assert!(result.ends_with('…'));
        assert_eq!(result.chars().count(), MAX_CHARS + 1);
    }

    #[test]
    fn all_images_no_text_returns_empty() {
        let md = "![a](/a.jpg)\n\n![b](/b.jpg)";
        assert_eq!(excerpt(md), "");
    }

    #[test]
    fn whitespace_only_paragraph_is_skipped() {
        let md = "   \n\nActual content.";
        assert_eq!(excerpt(md), "Actual content.");
    }
}
