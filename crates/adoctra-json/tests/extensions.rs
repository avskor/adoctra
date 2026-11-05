//! How the JSON backend exports blocks created by extension processors
//! (compared with Asciidoctor 2.0.23 and the equivalent Ruby extensions).

use adoctra_core::{
    parse_with_options, Backend, BlockContext, BlockMacroProcessor, BlockMacroRequest,
    BlockProcessor, BlockRequest, NewBlock, ParseOptions, ProcessContext, Registry,
};
use adoctra_json::JsonBackend;
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
        ..ParseOptions::default()
    };
    let result = parse_with_options(source, &options);
    String::from_utf8(JsonBackend::new().convert(&result.document).unwrap()).unwrap()
}

#[test]
fn processor_blocks_export_like_builtin_blocks() {
    let json = render("[[mid]]\ngen::t[k=v]\n\n.Cap\n[shout, pic]\n----\na\n----\n");
    // The processor's blocks are ordinary nodes; the image keeps Ruby's
    // block attributes (`alt`, `target`, `cloaked-context`).
    assert_eq!(
        json,
        r#"{"name":"document","blocks":[{"name":"listing","id":"mid","roles":[],"style":null,"attributes":{"k":"v"},"options":[],"title":null,"caption":null,"text":"T=t <b>"},{"name":"image","target":"pic.png","src":"pic.png","alt":"a","parts":[{"kind":"text","value":"a"}],"id":null,"roles":[],"style":null,"attributes":{"alt":"a","cloaked-context":"listing","target":"pic.png"},"options":[],"title":[{"name":"text","value":"Cap"}],"caption":"Figure 1. "}]}"#
    );
}
