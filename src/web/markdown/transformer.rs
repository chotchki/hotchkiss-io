use anyhow::anyhow;
use anyhow::Result;
use markdown::mdast::Html;
use markdown::mdast::Node;
use markdown::to_html_with_options;
use markdown::to_mdast;
use markdown::CompileOptions;
use markdown::Constructs;
use markdown::Options;
use markdown::ParseOptions;
use mdast_util_to_markdown::to_markdown;
use std::collections::VecDeque;

use crate::web::markdown::diagram;

///Function to take a markdown string, parse to nodes and then
/// ensure the output HTML flags stl files for use in the viewer
///
/// Technique from https://github.com/wooorm/markdown-rs/discussions/161
/// This is doing double the work until this is fixed: https://github.com/wooorm/markdown-rs/issues/27
pub fn transform(markdown: &str) -> Result<String> {
    // Enable math, but ONLY with `$$…$$` delimiters (single-dollar OFF) so prose
    // prices like "$200 … $250/month" don't get parsed as inline math. The math
    // nodes become source-carrying `.math` spans below; KaTeX (katex-render.js)
    // typesets them client-side.
    let parse_options = ParseOptions {
        constructs: Constructs {
            math_text: true,
            math_flow: true,
            ..Constructs::default()
        },
        math_text_single_dollar: false,
        ..ParseOptions::default()
    };
    let mut ast = to_mdast(markdown, &parse_options)
        .map_err(|m: markdown::message::Message| anyhow!("Failed to parse markdown {}", m))?;

    let mut queue = VecDeque::from([&mut ast]);
    while let Some(node) = queue.pop_front() {
        match node {
            Node::Image(image) => {
                if image.url.ends_with(".stl") {
                    *node = Node::Html(Html {
                        value: format!(
                            "<object class=\"stl-view size-40 m-2 rounded-md border-8 border-navy\" data-filename=\"{}\"></object>",
                            image.url
                        ),
                        position: None,
                    })
                } else {
                    // Cap a content image's in-flow height and make it click-to-zoom,
                    // reusing the diagram lightbox (diagram-zoom.js binds any
                    // `img[data-zoomable]`). The full src loads in-flow, CSS-capped, so
                    // the zoom clone shows it at full resolution.
                    *node = Node::Html(Html {
                        value: format!(
                            "<img class=\"content-image mx-auto my-4 block cursor-zoom-in\" \
style=\"max-width:100%;max-height:{MAX_IMAGE_HEIGHT_PX}px\" data-zoomable=\"true\" tabindex=\"0\" \
role=\"button\" aria-label=\"Zoom image\" src=\"{}\" alt=\"{}\" />",
                            attr_escape(&image.url),
                            attr_escape(&image.alt),
                        ),
                        position: None,
                    })
                }
            }
            Node::Code(code) => {
                if let Some(lang) = code.lang.as_deref()
                    && diagram::is_diagram_lang(lang)
                {
                    // Don't compile here. Emit a placeholder that carries the d2
                    // source (LLM / no-JS friendly) + an hx-get that swaps in the
                    // rendered SVG on load. `register` returns the content hash;
                    // the actual d2 compile happens lazily at /diagram/<hash>.
                    let hash = diagram::register(&code.value);
                    *node = Node::Html(Html {
                        value: diagram::placeholder(&hash, &code.value),
                        position: None,
                    });
                }
            }
            Node::InlineMath(m) => {
                // Carry the TeX verbatim (no-JS / crawler / LLM reads the source);
                // KaTeX (katex-render.js) typesets `.math` elements client-side.
                *node = Node::Html(Html {
                    value: format!(
                        "<span class=\"math math-inline\">{}</span>",
                        attr_escape(&m.value)
                    ),
                    position: None,
                });
            }
            Node::Math(m) => {
                *node = Node::Html(Html {
                    value: format!(
                        "<div class=\"math math-display\">{}</div>",
                        attr_escape(&m.value)
                    ),
                    position: None,
                });
            }
            Node::Root(root) => queue.extend(root.children.iter_mut()),
            Node::Paragraph(p) => queue.extend(p.children.iter_mut()),
            _ => {}
        }
    }

    let options = Options {
        compile: CompileOptions {
            allow_dangerous_html: true,
            ..CompileOptions::default()
        },
        ..Options::default()
    };

    let transformed_markdown =
        to_markdown(&ast).map_err(|m| anyhow!("AST to Markdown failed {}", m))?;

    to_html_with_options(&transformed_markdown, &options)
        .map_err(|m: markdown::message::Message| anyhow!("Failed to stringify markdown {}", m))
}

/// In-flow cap for content images — matches the diagram cap so the two read
/// consistently; click-to-zoom (diagram-zoom.js) reveals the full image.
const MAX_IMAGE_HEIGHT_PX: u32 = 480;

/// Minimal HTML-attribute escaping for values interpolated into an emitted tag.
fn attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_link() -> Result<()> {
        let input = "![test](https://beta.hotchkiss.io/image.jpg)";

        let rendered = transform(input)?;

        // A content image renders capped + click-to-zoom (reusing the diagram
        // lightbox), not a bare passthrough <img>.
        assert!(
            rendered.contains("src=\"https://beta.hotchkiss.io/image.jpg\""),
            "src kept: {rendered}"
        );
        assert!(rendered.contains("alt=\"test\""), "alt kept: {rendered}");
        assert!(
            rendered.contains("data-zoomable=\"true\""),
            "click-to-zoom hook: {rendered}"
        );
        assert!(
            rendered.contains("max-height:480px"),
            "in-flow height cap: {rendered}"
        );
        assert!(
            rendered.contains("cursor-zoom-in"),
            "zoom affordance: {rendered}"
        );

        Ok(())
    }

    #[test]
    fn stl_link() -> Result<()> {
        let input = "![test](https://beta.hotchkiss.io/image.stl)";

        let rendered = transform(input)?;

        assert_eq!(
            rendered,
            "<p><object class=\"stl-view size-40 m-2 rounded-md border-8 border-navy\" data-filename=\"https://beta.hotchkiss.io/image.stl\"></object></p>\n"
        );

        Ok(())
    }

    #[test]
    fn d2_fence_becomes_a_swap_placeholder() -> Result<()> {
        let input = "```d2\nx -> y -> z\n```";

        let rendered = transform(input)?;

        // Not a plain markdown code block any more — a swap placeholder that
        // carries the source. (It still uses <pre><code> to *display* the
        // source, so we check for the swap target, not the absence of <code>.)
        assert!(
            rendered.contains("hx-get=\"/diagram/"),
            "expected the HTMX swap target: {rendered}"
        );
        assert!(
            rendered.contains("class=\"d2-source"),
            "source should be shown in the d2-source block: {rendered}"
        );
        assert!(
            rendered.contains("x -&gt; y"),
            "the d2 source must be in the served HTML (escaped): {rendered}"
        );
        Ok(())
    }

    #[test]
    fn non_diagram_code_is_left_alone() -> Result<()> {
        let input = "```rust\nlet x = 1;\n```";

        let rendered = transform(input)?;

        assert!(rendered.contains("<code"), "normal code stays a code block");
        assert!(!rendered.contains("hx-get=\"/diagram/"), "must not become a diagram");
        Ok(())
    }

    #[test]
    fn dollar_dollar_math_becomes_katex_spans() -> Result<()> {
        let input = "drift is $$d = b - c$$ inline.\n\n$$\n\\sum x\n$$";

        let rendered = transform(input)?;

        assert!(
            rendered.contains("class=\"math math-inline\""),
            "inline math should be a .math span: {rendered}"
        );
        assert!(
            rendered.contains("d = b - c"),
            "the inline TeX source must survive into the HTML: {rendered}"
        );
        assert!(
            rendered.contains("class=\"math math-display\""),
            "display math should be a .math-display div: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn single_dollar_prices_are_not_math() -> Result<()> {
        // single `$` must stay literal so prose prices don't parse as math
        let input = "it cost $200 and then $250 a month";

        let rendered = transform(input)?;

        assert!(
            !rendered.contains("class=\"math"),
            "prose prices must NOT become math: {rendered}"
        );
        assert!(rendered.contains("$200"), "the price text must survive: {rendered}");
        Ok(())
    }
}
