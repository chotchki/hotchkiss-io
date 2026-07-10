//! DL.2 — pull every link/image/reference URL out of a page's markdown.

use markdown::mdast::Node;
use markdown::to_mdast;

/// Extract every DISTINCT link / image / reference-definition URL from `markdown`,
/// in document order. Mirrors `web::markdown::links::collect` — the immutable
/// `node.children()` recursion over the three URL-bearing node variants (Link,
/// Image, Definition; reference definitions included, which a Link-only walk would
/// miss) — not the heavier `transformer.rs` `children_mut()` BFS.
///
/// A parse failure OR a markdown-rs panic degrades to an EMPTY list: the alpha
/// parser can panic on pathological content (a smart-quote 2012 post took down the
/// feed in CG), and a background scan must never let one gnarly page abort the pass.
pub fn extract_links(markdown: &str) -> Vec<String> {
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        to_mdast(markdown, &Default::default()).ok()
    }));
    let ast = match parsed {
        Ok(Some(ast)) => ast,
        Ok(None) => return Vec::new(), // parse error — nothing to check, not a crash
        Err(_) => {
            tracing::warn!(
                "dead-link extraction panicked on {}-byte page; skipping its links",
                markdown.len()
            );
            return Vec::new();
        }
    };
    let mut urls = Vec::new();
    collect(&ast, &mut urls);
    urls
}

/// Read the URL off Link / Image / Definition nodes, recurse into children.
/// Distinct + document-ordered (a page linking the same URL twice is one ref).
fn collect(node: &Node, urls: &mut Vec<String>) {
    let url = match node {
        Node::Link(l) => Some(&l.url),
        Node::Image(i) => Some(&i.url),
        Node::Definition(d) => Some(&d.url),
        _ => None,
    };
    if let Some(u) = url {
        let u = u.trim().to_string();
        if !u.is_empty() && !urls.contains(&u) {
            urls.push(u);
        }
    }
    if let Some(children) = node.children() {
        for child in children {
            collect(child, urls);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_link_and_image() {
        let md = "see [the post](https://example.com/a) and ![pic](/media/xyz)";
        assert_eq!(
            extract_links(md),
            vec![
                "https://example.com/a".to_string(),
                "/media/xyz".to_string()
            ]
        );
    }

    #[test]
    fn nested_in_list_heading_table_blockquote() {
        // The walk must descend into ALL containers, not just top-level paragraphs
        // (the BW regression class). Links live in a list item, a heading, a table
        // cell, and a blockquote here.
        let md = "\
# heading [h](https://h.example/x)

- item [l](https://l.example/y)

> quoted [q](https://q.example/z)

| col |
|-----|
| [t](https://t.example/w) |
";
        let got = extract_links(md);
        for u in [
            "https://h.example/x",
            "https://l.example/y",
            "https://q.example/z",
            "https://t.example/w",
        ] {
            assert!(got.contains(&u.to_string()), "missing {u}: {got:?}");
        }
    }

    #[test]
    fn reference_definition_is_collected() {
        // A reference-style link's URL lives on the Definition node, not the Link.
        let md = "see [the ref][r]\n\n[r]: https://ref.example/page";
        assert!(
            extract_links(md).contains(&"https://ref.example/page".to_string()),
            "reference definition URL must be collected"
        );
    }

    #[test]
    fn distinct_and_ordered() {
        let md = "[a](https://x.example/1) [b](https://x.example/1) [c](https://x.example/2)";
        assert_eq!(
            extract_links(md),
            vec![
                "https://x.example/1".to_string(),
                "https://x.example/2".to_string()
            ]
        );
    }

    #[test]
    fn skips_empty_and_whitespace_urls() {
        // A malformed autolink can yield an empty url; it must not become a "".
        let md = "text with no links at all";
        assert!(extract_links(md).is_empty());
    }

    #[test]
    fn mixed_internal_external_mailto_anchor() {
        // Extraction is classification-BLIND — it pulls every URL; classify() sorts
        // internal/external/skip. So mailto + anchor come through here.
        let md = "[x](/blog/foo) [y](https://ext.example/z) [m](mailto:a@b.c) [f](#top)";
        assert_eq!(
            extract_links(md),
            vec![
                "/blog/foo".to_string(),
                "https://ext.example/z".to_string(),
                "mailto:a@b.c".to_string(),
                "#top".to_string(),
            ]
        );
    }
}
