use anyhow::anyhow;
use anyhow::Result;
use markdown::mdast::Html;
use markdown::mdast::Node;
use markdown::to_html_with_options;
use markdown::to_mdast;
use markdown::CompileOptions;
use markdown::Options;
use mdast_util_to_markdown::to_markdown;
use std::collections::VecDeque;

use crate::web::markdown::diagram;

///Function to take a markdown string, parse to nodes and then
/// ensure the output HTML flags stl files for use in the viewer
///
/// Technique from https://github.com/wooorm/markdown-rs/discussions/161
/// This is doing double the work until this is fixed: https://github.com/wooorm/markdown-rs/issues/27
pub fn transform(markdown: &str) -> Result<String> {
    let mut ast = to_mdast(markdown, &Default::default())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_link() -> Result<()> {
        let input = "![test](https://beta.hotchkiss.io/image.jpg)";

        let rendered = transform(input)?;

        assert_eq!(
            rendered,
            "<p><img src=\"https://beta.hotchkiss.io/image.jpg\" alt=\"test\" /></p>\n"
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
}
