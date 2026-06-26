//! Save-time link normalization: rewrite absolute links/images that point at
//! THIS site to root-relative, so authored content stays portable — it works on
//! prod, on beta, and on any future host without hard-coding the canonical
//! domain. (Dogfooded out of the first image blog post, which carried absolute
//! `https://hotchkiss.io/...` links.)
//!
//! Runs on SAVE, not render: the stored markdown becomes the canonical portable
//! form (so the Atom feed and any consumer also get the relative links). We
//! string-replace the exact URL strings the markdown AST identifies as
//! link / image / reference-definition targets — longest-first, so a bare-domain
//! match can't corrupt a longer same-origin path URL. This preserves the
//! author's formatting byte-for-byte everywhere else; a full AST round-trip would
//! reflow the whole document (bullet markers, emphasis chars, spacing).

use anyhow::anyhow;
use anyhow::Result;
use markdown::mdast::Node;
use markdown::to_mdast;
use url::Url;

/// Rewrite every absolute http(s) URL pointing at `site_host` (or its `www.`
/// variant, any port) into a root-relative path, preserving query + fragment.
/// External hosts, `mailto:`, and already-relative URLs are left untouched.
pub fn rewrite_site_links(markdown: &str, site_host: &str) -> Result<String> {
    let ast = to_mdast(markdown, &Default::default())
        .map_err(|m: markdown::message::Message| anyhow!("Failed to parse markdown {}", m))?;

    let mut pairs: Vec<(String, String)> = Vec::new();
    collect(&ast, site_host, &mut pairs);

    // Longest absolute URL first: a bare-domain replacement must not run before a
    // longer same-origin path URL, or "https://h/blog" would become "//blog".
    pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));

    let mut out = markdown.to_string();
    for (abs, rel) in pairs {
        out = out.replace(&abs, &rel);
    }
    Ok(out)
}

/// Collect distinct (absolute, relative) URL pairs for every site-matching
/// link / image / reference-definition target in the tree.
fn collect(node: &Node, site_host: &str, pairs: &mut Vec<(String, String)>) {
    let url = match node {
        Node::Link(l) => Some(&l.url),
        Node::Image(i) => Some(&i.url),
        Node::Definition(d) => Some(&d.url),
        _ => None,
    };
    if let Some(u) = url
        && let Some(rel) = relativize(u, site_host)
    {
        let pair = (u.clone(), rel);
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    }
    if let Some(children) = node.children() {
        for child in children {
            collect(child, site_host, pairs);
        }
    }
}

/// `https://[www.]<host>[:port]/path?q#f` -> `/path?q#f`. Returns `None` for any
/// URL that isn't an http(s) link to this site (external host, `mailto:`,
/// already-relative, unparseable).
fn relativize(url_str: &str, site_host: &str) -> Option<String> {
    let parsed = Url::parse(url_str).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let site = site_host.to_ascii_lowercase();
    if host != site && host != format!("www.{site}") {
        return None;
    }
    let mut rel = parsed.path().to_string();
    if let Some(q) = parsed.query() {
        rel.push('?');
        rel.push_str(q);
    }
    if let Some(f) = parsed.fragment() {
        rel.push('#');
        rel.push_str(f);
    }
    Some(rel)
}

#[cfg(test)]
mod tests {
    use super::*;
    const HOST: &str = "hotchkiss.io";

    #[test]
    fn absolute_site_link_becomes_relative() {
        let md = "see [the post](https://hotchkiss.io/blog/foo)";
        assert_eq!(
            rewrite_site_links(md, HOST).unwrap(),
            "see [the post](/blog/foo)"
        );
    }

    #[test]
    fn www_variant_relativized() {
        let md = "[x](https://www.hotchkiss.io/about)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), "[x](/about)");
    }

    #[test]
    fn port_is_dropped() {
        let md = "[x](https://hotchkiss.io:8443/blog)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), "[x](/blog)");
    }

    #[test]
    fn query_and_fragment_preserved() {
        let md = "[x](https://hotchkiss.io/blog?page=2#top)";
        assert_eq!(
            rewrite_site_links(md, HOST).unwrap(),
            "[x](/blog?page=2#top)"
        );
    }

    #[test]
    fn bare_domain_becomes_root() {
        let md = "[home](https://hotchkiss.io)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), "[home](/)");
    }

    #[test]
    fn external_links_untouched() {
        let md = "[gh](https://github.com/chotchki/recon-gen)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), md);
    }

    #[test]
    fn image_src_relativized() {
        let md = "![cost](https://hotchkiss.io/attachments/5)";
        assert_eq!(
            rewrite_site_links(md, HOST).unwrap(),
            "![cost](/attachments/5)"
        );
    }

    #[test]
    fn http_scheme_also_matches() {
        let md = "[x](http://hotchkiss.io/y)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), "[x](/y)");
    }

    #[test]
    fn non_http_scheme_untouched() {
        let md = "[mail](mailto:chris@hotchkiss.io)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), md);
    }

    #[test]
    fn already_relative_untouched() {
        let md = "[x](/blog/foo)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), md);
    }

    #[test]
    fn bare_domain_and_path_no_double_slash() {
        // longest-first ordering must keep the bare-domain replace from
        // corrupting the path URL.
        let md = "[home](https://hotchkiss.io) and [post](https://hotchkiss.io/blog/x)";
        assert_eq!(
            rewrite_site_links(md, HOST).unwrap(),
            "[home](/) and [post](/blog/x)"
        );
    }

    #[test]
    fn reference_definition_relativized() {
        let md = "[x]\n\n[x]: https://hotchkiss.io/ref";
        let out = rewrite_site_links(md, HOST).unwrap();
        assert!(out.contains("[x]: /ref"), "ref def should relativize: {out}");
    }

    #[test]
    fn external_image_untouched() {
        let md = "![y](https://example.com/pic.png)";
        assert_eq!(rewrite_site_links(md, HOST).unwrap(), md);
    }
}
