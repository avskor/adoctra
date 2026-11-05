//! How the HTML backend renders blocks created by extension processors
//! (compared with Asciidoctor 2.0.23 and the equivalent Ruby extensions).

use adoctra_core::{
    parse_with_options, AttributeFormat, Backend, BlockContext, BlockMacroProcessor,
    BlockMacroRequest, BlockProcessor, BlockRequest, ExtensionAttributes, InlineMacroFormat,
    InlineMacroProcessor, InlineMacroRequest, MacroContent, NewBlock, NewInline, ParseOptions,
    ProcessContext, Registry,
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

/// Ruby: `create_inline parent, :image, nil, target: 'p.png', attributes:
/// {…}` for `img`, `nil` for `drop`, else `create_inline parent, :quoted,
/// "<b>#{target}</b>", type: :monospaced, attributes: {'role' => …}`.
struct Tag;

impl InlineMacroProcessor for Tag {
    fn name(&self) -> &str {
        "tag"
    }
    fn positional_attributes(&self) -> &[&str] {
        &["first", "second"]
    }
    fn default_attributes(&self) -> &[(&str, &str)] {
        &[("d", "v")]
    }
    fn process(
        &self,
        request: InlineMacroRequest,
        _: &mut ProcessContext<'_>,
    ) -> Option<NewInline> {
        let mut attributes = ExtensionAttributes::new();
        match request.target.as_str() {
            "drop" => None,
            "img" => {
                for (name, value) in [
                    ("alt", "A &amp; B"),
                    ("width", "10"),
                    ("role", "r"),
                    ("title", "T"),
                ] {
                    attributes.set(name, value);
                }
                Some(NewInline::image("p.png", attributes))
            }
            target => {
                if let Some(role) = request.attributes.get("role") {
                    attributes.set("role", role);
                }
                Some(NewInline::quoted(
                    AttributeFormat::Monospace,
                    format!("<b>{target}</b>"),
                    attributes,
                ))
            }
        }
    }
}

/// Ruby: `format :short`, `content_model :text`, a `:strong` quoted inline.
struct Say;

impl InlineMacroProcessor for Say {
    fn name(&self) -> &str {
        "say"
    }
    fn format(&self) -> InlineMacroFormat {
        InlineMacroFormat::Short
    }
    fn content(&self) -> MacroContent {
        MacroContent::Text
    }
    fn process(
        &self,
        request: InlineMacroRequest,
        _: &mut ProcessContext<'_>,
    ) -> Option<NewInline> {
        let text = request.attributes.get("text").unwrap_or_default();
        Some(NewInline::quoted(
            AttributeFormat::Strong,
            format!("{}|{text}", request.target),
            ExtensionAttributes::new(),
        ))
    }
}

fn render(source: &str) -> String {
    let mut registry = Registry::new();
    registry
        .register_block(Shout)
        .register_block_macro(Gen)
        .register_inline_macro(Tag)
        .register_inline_macro(Say);
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

#[test]
fn processor_inlines_render_like_rubys_inline_nodes() {
    let html = render(
        "A tag:x&y[one, k=v,two,  three , opts=\"a, b\",role=rr] B xtag:img[] C \\tag:esc[] D tag:drop[] E.\n\n\
         tag:a[b]c[] and tag:t[x\\]y] and tag:sp ace[] and tag:{nope}[] *tag:in[]* say:[ hi \\] there ] say:[].\n\n\
         Multi tag:m[a\nb] end. tag:v[,,z]\n",
    );
    // Ruby writes the quoted text as is and the image like `image:`; a
    // macro list does not cross lines (the line feed folds in paragraphs,
    // as everywhere in adoctra's HTML).
    assert_eq!(
        html,
        "<p>A <code class=\"rr\"><b>x&amp;y</b></code> B x<span class=\"image r\"><img src=\"p.png\" alt=\"A &amp; B\" width=\"10\" title=\"T\"></span> C tag:esc[] D  E.</p>\n\
         <p><code><b>a</b></code>c[] and <code><b>t</b></code> and tag:sp ace[] and <code><b>{nope}</b></code> <strong><code><b>in</b></code></strong> <strong>hi ] there|hi ] there</strong> <strong>|</strong>.</p>\n\
         <p>Multi tag:m[a b] end. <code><b>v</b></code></p>\n"
    );
}
