//! How the JSON backend exports blocks created by extension processors
//! (compared with Asciidoctor 2.0.23 and the equivalent Ruby extensions).

use adoctra_core::{
    parse_with_options, AttributeFormat, Backend, BlockContext, BlockMacroProcessor,
    BlockMacroRequest, BlockProcessor, BlockRequest, ExtensionAttributes, InlineMacroProcessor,
    InlineMacroRequest, NewBlock, NewInline, ParseOptions, ProcessContext, Registry,
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

/// An image for `img`, else the target as raw monospaced text with a role.
struct Tag;

impl InlineMacroProcessor for Tag {
    fn name(&self) -> &str {
        "tag"
    }
    fn process(
        &self,
        request: InlineMacroRequest,
        _: &mut ProcessContext<'_>,
    ) -> Option<NewInline> {
        let mut attributes = ExtensionAttributes::new();
        if request.target == "img" {
            attributes.set("alt", "A");
            attributes.set("width", "10");
            return Some(NewInline::image("p.png", attributes));
        }
        attributes.set("role", "r");
        Some(NewInline::quoted(
            AttributeFormat::Monospace,
            format!("<{}>", request.target),
            attributes,
        ))
    }
}

fn render(source: &str) -> String {
    let mut registry = Registry::new();
    registry
        .register_block(Shout)
        .register_block_macro(Gen)
        .register_inline_macro(Tag);
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

#[test]
fn processor_inlines_export_as_parts() {
    let json = render("A tag:x[] B tag:img[] C\n");
    // Ruby's quoted inline (raw text, role) and image node.
    assert_eq!(
        json,
        r#"{"name":"document","blocks":[{"name":"paragraph","id":null,"roles":[],"style":null,"attributes":{},"options":[],"title":null,"caption":null,"inlines":[{"name":"text","value":"A "},{"name":"substituted","value":"<x>","parts":[{"kind":"open","format":"monospace","role":"r"},{"kind":"raw","value":"<x>"},{"kind":"close","format":"monospace"}]},{"name":"text","value":" B "},{"name":"image","icon":false,"target":"p.png","src":"p.png","alt":"A","icons":null,"attributes":{"width":"10"},"options":[],"parts":[{"kind":"text","value":"A"}]},{"name":"text","value":" C"}]}]}"#
    );
}
