//! How the HTML backend renders blocks created by extension processors
//! (compared with Asciidoctor 2.0.23 and the equivalent Ruby extensions).

use adoctra_core::{
    parse_with_options, Backend, BlockContext, BlockMacroProcessor, BlockMacroRequest,
    BlockProcessor, BlockRequest, NewBlock, ParseOptions, ProcessContext, Registry,
};
use adoctra_html::HtmlBackend;
use std::sync::Arc;

struct Shout;

impl BlockProcessor for Shout {
    fn name(&self) -> &str {
        "shout"
    }
    fn contexts(&self) -> &[BlockContext] {
        &[
            BlockContext::Listing,
            BlockContext::Open,
            BlockContext::Paragraph,
        ]
    }
    fn positional_attributes(&self) -> &[&str] {
        &["target"]
    }
    fn process(&self, request: BlockRequest, _: &mut ProcessContext<'_>) -> Option<NewBlock> {
        let mut attributes = request.attributes.clone();
        if attributes.get("target") == Some("drop") {
            return None;
        }
        let target = format!("{}.png", attributes.get("target").unwrap_or("x"));
        attributes.set("target", target.clone());
        attributes.set("alt", request.lines.join("|"));
        Some(NewBlock::image(target, attributes))
    }
}

struct Gen;

impl BlockMacroProcessor for Gen {
    fn name(&self) -> &str {
        "gen"
    }
    fn process(&self, request: BlockMacroRequest, _: &mut ProcessContext<'_>) -> Option<NewBlock> {
        Some(NewBlock::listing(
            format!("T={} <b>", request.target),
            request.attributes,
        ))
    }
}

fn render(source: &str) -> String {
    let mut registry = Registry::new();
    registry.register_block(Shout).register_block_macro(Gen);
    let options = ParseOptions {
        extensions: Some(Arc::new(registry)),
        embedded: true,
        ..ParseOptions::default()
    };
    let result = parse_with_options(source, &options);
    String::from_utf8(HtmlBackend::embedded().convert(&result.document).unwrap()).unwrap()
}

#[test]
fn processor_blocks_render_like_builtin_blocks() {
    let html = render(
        ".Cap\n[shout#sid, pic]\n----\na\nb\n----\n\nSee <<sid>>.\n\n[shout, drop]\n--\nzz\n--\n\n\
         [[mid]]\n.Mac\ngen::tgt{nope}[k=v, 9]\n\n[shout]\nline one\n// c\nline two\n",
    );
    // Ruby: the image block with `Figure 1. Cap`, the xref text `Cap`, the
    // dropped open block, the escaped listing with title and id, the
    // paragraph's lines without the comment line.
    assert_eq!(
        html,
        "<figure id=\"sid\" class=\"imageblock\">\n\
         <img src=\"pic.png\" alt=\"a|b\">\n\
         <figcaption class=\"title\">Figure 1. Cap</figcaption>\n\
         </figure>\n\
         <p>See <a href=\"#sid\">Cap</a>.</p>\n\
         <div class=\"title\">Mac</div>\n\
         <pre class=\"listing\" id=\"mid\">T=tgt{nope} &lt;b&gt;</pre>\n\
         <figure class=\"imageblock\">\n\
         <img src=\"x.png\" alt=\"line one|line two\">\n\
         </figure>\n"
    );
}
