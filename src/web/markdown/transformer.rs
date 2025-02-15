use std::collections::VecDeque;
use std::default;

use anyhow::anyhow;
use anyhow::Result;
use markdown::mdast::Html;
use markdown::mdast::Node;
use markdown::to_html_with_options;
use markdown::to_mdast;
use markdown::CompileOptions;
use markdown::Options;
use mdast_util_to_markdown::to_markdown;
use tracing::debug;

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
}
