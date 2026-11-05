//! The extension API: block, block macro and tree processors invoked where
//! Ruby's `next_block` invokes extensions. Expected attribute orders and
//! output come from Asciidoctor 2.0.23 with equivalent Ruby extensions.

use adoctra_core::{
    parse_with_options, AttrKey, Block, BlockContext, BlockKind, BlockMacroProcessor,
    BlockMacroRequest, BlockProcessor, BlockRequest, ContentModel, Document, ExtensionAttributes,
    Inline, MacroContent, NewBlock, ParseOptions, ParseResult, ProcessContext, Registry, Severity,
    Span, TokenKind, TreeProcessor,
};
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

fn describe(attributes: &ExtensionAttributes) -> String {
    attributes
        .iter()
        .map(|(key, value)| match key {
            AttrKey::Positional(index) => format!("{index}={value}"),
            AttrKey::Named(name) => format!("{name}={value}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Ruby: `create_image_block parent, attrs.merge('target' => …, 'alt' => …)`;
/// a `drop` target answers `nil`.
struct Shout {
    log: Log,
    contexts: Vec<BlockContext>,
    model: ContentModel,
}

impl BlockProcessor for Shout {
    fn name(&self) -> &str {
        "shout"
    }
    fn contexts(&self) -> &[BlockContext] {
        &self.contexts
    }
    fn positional_attributes(&self) -> &[&str] {
        &["target"]
    }
    fn content_model(&self) -> ContentModel {
        self.model
    }
    fn process(&self, request: BlockRequest, context: &mut ProcessContext<'_>) -> Option<NewBlock> {
        self.log.lock().unwrap().push(format!(
            "{} {:?} [{}] {:?}",
            request.name,
            request.context,
            describe(&request.attributes),
            request.lines
        ));
        if request.attributes.get("target") == Some("drop") {
            context.warning("dropped");
            return None;
        }
        let mut attributes = request.attributes.clone();
        let target = format!("{}.png", attributes.get("target").unwrap_or("x"));
        attributes.set("target", target.clone());
        attributes.set("alt", request.lines.join("|"));
        Some(NewBlock::image(target, attributes))
    }
}

/// Ruby: `create_listing_block parent, "T=#{target}", attrs`.
struct Gen {
    log: Log,
    content: MacroContent,
}

impl BlockMacroProcessor for Gen {
    fn name(&self) -> &str {
        "gen"
    }
    fn content(&self) -> MacroContent {
        self.content
    }
    fn process(
        &self,
        request: BlockMacroRequest,
        _context: &mut ProcessContext<'_>,
    ) -> Option<NewBlock> {
        self.log.lock().unwrap().push(format!(
            "{} {} [{}]",
            request.name,
            request.target,
            describe(&request.attributes)
        ));
        Some(NewBlock::listing(
            format!("T={}", request.target),
            request.attributes,
        ))
    }
}

/// Counts the top-level blocks and reports them.
struct Count;

impl TreeProcessor for Count {
    fn name(&self) -> &str {
        "count"
    }
    fn process(&self, document: &mut Document, context: &mut ProcessContext<'_>) {
        context.warning(format!("{} blocks", document.blocks.len()));
        document.attributes.set("counted", "yes");
    }
}

fn registry(log: &Log) -> Registry {
    let mut registry = Registry::new();
    registry
        .register_block(Shout {
            log: Arc::clone(log),
            contexts: vec![
                BlockContext::Listing,
                BlockContext::Open,
                BlockContext::Paragraph,
            ],
            model: ContentModel::Simple,
        })
        .register_block_macro(Gen {
            log: Arc::clone(log),
            content: MacroContent::Attributes,
        });
    registry
}

fn parse_with(source: &str, registry: Registry) -> ParseResult {
    let options = ParseOptions {
        extensions: Some(Arc::new(registry)),
        ..ParseOptions::default()
    };
    parse_with_options(source, &options)
}

fn title(block: &Block) -> String {
    block
        .title
        .iter()
        .flatten()
        .map(|inline| match inline {
            Inline::Text { value, .. } => value.as_str(),
            other => panic!("unexpected title inline {other:?}"),
        })
        .collect()
}

const SOURCE: &str = "\
.Cap
[shout#sid, pic]
----
a
b
----

See <<sid>>.

[shout, drop]
--
zz
--

[[mid]]
.Mac
gen::tgt{nope}[k=v, 9]

[shout]
line one
// c
line two
";

#[test]
fn block_and_macro_processors_receive_rubys_attributes() {
    let log = Log::default();
    let result = parse_with(SOURCE, registry(&log));
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "shout Listing [title=Cap, 1=shout, 2=pic, id=sid, target=pic, \
             cloaked-context=listing] [\"a\", \"b\"]",
            "shout Open [1=shout, 2=drop, target=drop, cloaked-context=open] [\"zz\"]",
            "gen tgt{nope} [id=mid, title=Mac, k=v, 2=9]",
            "shout Paragraph [1=shout, cloaked-context=paragraph] [\"line one\", \"line two\"]",
        ]
    );
    let blocks = &result.document.blocks;
    assert_eq!(blocks.len(), 4, "{blocks:#?}");

    // The image replaces the listing, with its metadata and a figure caption.
    let image = &blocks[0];
    match &image.kind {
        BlockKind::Image { target, src, .. } => {
            assert_eq!(target, "pic.png");
            assert_eq!(src, "pic.png");
        }
        other => panic!("expected an image, got {other:?}"),
    }
    assert_eq!(image.id.as_deref(), Some("sid"));
    assert_eq!(title(image), "Cap");
    assert_eq!(image.caption.as_deref(), Some("Figure 1. "));
    assert_eq!(
        image.attrs.named.get("alt").map(String::as_str),
        Some("a|b")
    );
    assert_eq!(
        image.attrs.named.get("cloaked-context").map(String::as_str),
        Some("listing")
    );
    let fenced = "----\na\nb\n----";
    let at = SOURCE.find(fenced).unwrap();
    assert_eq!(image.span, Span::new(at, at + fenced.len()));

    // The cross reference resolves to the processor's block.
    assert!(matches!(blocks[1].kind, BlockKind::Paragraph { .. }));

    // `None` dropped the open block; the macro became a titled listing.
    let listing = &blocks[2];
    match &listing.kind {
        BlockKind::Listing { text, .. } => assert_eq!(text, "T=tgt{nope}"),
        other => panic!("expected a listing, got {other:?}"),
    }
    assert_eq!(listing.id.as_deref(), Some("mid"));
    assert_eq!(title(listing), "Mac");
    assert_eq!(listing.caption, None);
    let line = "gen::tgt{nope}[k=v, 9]";
    let at = SOURCE.find(line).unwrap();
    assert_eq!(listing.span, Span::new(at, at + line.len()));

    // The paragraph's line comment is not one of its lines.
    match &blocks[3].kind {
        BlockKind::Image { target, .. } => assert_eq!(target, "x.png"),
        other => panic!("expected an image, got {other:?}"),
    }

    // The processor's warning is reported at the dropped block.
    let dropped = result
        .diagnostics
        .iter()
        .find(|d| d.message == "dropped")
        .expect("processor diagnostic");
    assert_eq!(dropped.severity, Severity::Warning);
    let open = SOURCE.find("--\nzz").unwrap();
    assert_eq!(dropped.span.start, open);
}

#[test]
fn tokens_and_spans_come_from_the_read_block() {
    let log = Log::default();
    let result = parse_with(SOURCE, registry(&log));
    let kinds: Vec<(TokenKind, &str)> = result
        .tokens
        .iter()
        .map(|t| (t.kind, &SOURCE[t.span.start..t.span.end]))
        .collect();
    assert!(kinds.contains(&(TokenKind::ListingDelimiter, "----")));
    assert!(kinds.contains(&(TokenKind::ListingText, "a")));
    assert!(kinds.contains(&(TokenKind::OpenDelimiter, "--")));
    assert!(kinds.contains(&(TokenKind::BlockMacro, "gen::tgt{nope}[k=v, 9]")));
}

#[test]
fn unterminated_block_is_still_processed() {
    let log = Log::default();
    let source = "[shout]\n----\nunterminated\n";
    let result = parse_with(source, registry(&log));
    assert_eq!(
        *log.lock().unwrap(),
        vec!["shout Listing [1=shout, cloaked-context=listing] [\"unterminated\"]"]
    );
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(
        diagnostic.message,
        "unterminated shout block (missing closing `----`)"
    );
    assert!(matches!(
        result.document.blocks[0].kind,
        BlockKind::Image { .. }
    ));
}

#[test]
fn builtin_styles_and_unregistered_contexts_win() {
    let log = Log::default();
    let mut registry = Registry::new();
    // A processor named after a masquerade is never asked for that block,
    // nor for a context it did not register.
    for name in ["source", "shout"] {
        struct Named(&'static str, Log);
        impl BlockProcessor for Named {
            fn name(&self) -> &str {
                self.0
            }
            fn contexts(&self) -> &[BlockContext] {
                &[BlockContext::Listing]
            }
            fn process(
                &self,
                request: BlockRequest,
                _: &mut ProcessContext<'_>,
            ) -> Option<NewBlock> {
                self.1.lock().unwrap().push(request.name);
                None
            }
        }
        registry.register_block(Named(name, Arc::clone(&log)));
    }
    let source = "[source]\n----\nx\n----\n\n[shout]\n....\ny\n....\n\n[shout]\nz\n";
    let result = parse_with(source, registry);
    assert!(log.lock().unwrap().is_empty());
    let kinds: Vec<_> = result
        .document
        .blocks
        .iter()
        .map(|b| match b.kind {
            BlockKind::Listing { .. } => "listing",
            BlockKind::Literal { .. } => "literal",
            BlockKind::Paragraph { .. } => "paragraph",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["listing", "literal", "paragraph"]);
}

#[test]
fn without_a_registry_styles_and_macros_stay_plain() {
    let result = parse_with_options(SOURCE, &ParseOptions::default());
    let blocks = &result.document.blocks;
    assert!(matches!(blocks[0].kind, BlockKind::Listing { .. }));
    assert!(blocks
        .iter()
        .any(|b| matches!(b.kind, BlockKind::Open { .. })));
}

#[test]
fn macro_text_content_and_verbatim_lines() {
    let log = Log::default();
    let mut registry = Registry::new();
    registry
        .register_block(Shout {
            log: Arc::clone(&log),
            contexts: vec![BlockContext::Literal],
            model: ContentModel::Verbatim,
        })
        .register_block_macro(Gen {
            log: Arc::clone(&log),
            content: MacroContent::Text,
        });
    let source = "gen::t[a, b]\n\n[shout,indent=0]\n....\n    x\n      y\n....\n";
    parse_with(source, registry);
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "gen t [text=a, b]",
            "shout Literal [1=shout, indent=0, cloaked-context=literal] [\"x\", \"  y\"]",
        ]
    );
}

#[test]
fn tree_processors_see_the_finished_document() {
    let mut registry = Registry::new();
    registry.register_tree_processor(Count);
    let result = parse_with("a\n\nb\n", registry);
    assert_eq!(result.document.attributes.get("counted"), Some("yes"));
    let last = result.diagnostics.last().expect("a report");
    assert_eq!(last.message, "2 blocks");
}

#[test]
fn processors_never_make_parsing_fail() {
    // A macro processor that drops everything and a malformed block still
    // leave a document with diagnostics.
    let log = Log::default();
    let result = parse_with("gen::[]\n\n[shout]\n--\n", registry(&log));
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.starts_with("unterminated shout block")));
}

#[test]
fn processor_text_loses_trailing_whitespace_like_ruby_block_source() {
    struct Spaces;
    impl BlockMacroProcessor for Spaces {
        fn name(&self) -> &str {
            "spaces"
        }
        fn process(
            &self,
            request: BlockMacroRequest,
            _: &mut ProcessContext<'_>,
        ) -> Option<NewBlock> {
            Some(NewBlock::literal(" a  \n\tb\t\r\n\nc ", request.attributes))
        }
    }
    let mut registry = Registry::new();
    registry.register_block_macro(Spaces);
    let result = parse_with("spaces::x[]\n", registry);
    match &result.document.blocks[0].kind {
        BlockKind::Literal { text, .. } => assert_eq!(text, " a\n\tb\n\nc"),
        other => panic!("expected a literal, got {other:?}"),
    }
}

#[test]
fn unlocated_warnings_keep_the_block_span_but_no_cursor() {
    struct Unlocated;
    impl BlockMacroProcessor for Unlocated {
        fn name(&self) -> &str {
            "unlocated"
        }
        fn process(
            &self,
            _: BlockMacroRequest,
            context: &mut ProcessContext<'_>,
        ) -> Option<NewBlock> {
            context.warning_unlocated("no place");
            None
        }
    }
    let mut registry = Registry::new();
    registry.register_block_macro(Unlocated);
    let result = parse_with("\nunlocated::x[]\n", registry);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.message, "no place");
    assert_eq!(diagnostic.span, Span::new(1, 15));
    assert_eq!(
        diagnostic.report.location,
        adoctra_core::DiagnosticLocation::Unlocated
    );
}

/// Logs like a Ruby inline macro `warn`ing its target and attributes;
/// `err` reports an error, `drop` answers `nil`.
struct Tag {
    log: Log,
}

impl adoctra_core::InlineMacroProcessor for Tag {
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
        request: adoctra_core::InlineMacroRequest,
        context: &mut ProcessContext<'_>,
    ) -> Option<adoctra_core::NewInline> {
        self.log.lock().unwrap().push(format!(
            "{} {} [{}]",
            request.name,
            request.target,
            describe(&request.attributes)
        ));
        match request.target.as_str() {
            "drop" => None,
            "err" => {
                context.error("bad tag");
                None
            }
            target => Some(adoctra_core::NewInline::quoted(
                adoctra_core::AttributeFormat::Monospace,
                format!("<{target}>"),
                ExtensionAttributes::new(),
            )),
        }
    }
}

struct Say {
    log: Log,
}

impl adoctra_core::InlineMacroProcessor for Say {
    fn name(&self) -> &str {
        "say"
    }
    fn format(&self) -> adoctra_core::InlineMacroFormat {
        adoctra_core::InlineMacroFormat::Short
    }
    fn content(&self) -> MacroContent {
        MacroContent::Text
    }
    fn process(
        &self,
        request: adoctra_core::InlineMacroRequest,
        _: &mut ProcessContext<'_>,
    ) -> Option<adoctra_core::NewInline> {
        self.log.lock().unwrap().push(format!(
            "say {:?} [{}]",
            request.target,
            describe(&request.attributes)
        ));
        None
    }
}

fn inline_registry(log: &Log) -> Registry {
    let mut registry = Registry::new();
    registry
        .register_inline_macro(Tag {
            log: Arc::clone(log),
        })
        .register_inline_macro(Say {
            log: Arc::clone(log),
        });
    registry
}

#[test]
fn inline_macro_processors_receive_rubys_attributes() {
    let log = Log::default();
    let source = "A tag:x&y[one, k=v,two,  three , opts=\"a, b\",role=rr] B xtag:img[] C \\tag:esc[] D tag:drop[] E.\n\n\
         tag:a[b]c[] and tag:t[x\\]y] and tag:sp ace[] and tag:{nope}[] *tag:in[]* say:[ hi \\] there ] say:[].\n\n\
         Multi tag:m[a\nb] end. tag:v[,,z]\n";
    let result = parse_with(source, inline_registry(&log));
    // Ruby's log, except that an empty positional (Ruby's `nil`) is left out.
    assert_eq!(
        *log.lock().unwrap(),
        [
            "tag x&amp;y [d=v, first=one, 1=one, k=v, 3=two, 4=three, a-option=, b-option=, role=rr]",
            "tag img [d=v]",
            "tag drop [d=v]",
            "tag a [d=v, first=b, 1=b]",
            "tag t [d=v, first=x]y, 1=x]y]",
            "tag {nope} [d=v]",
            "tag in [d=v]",
            "say \"hi ] there\" [text=hi ] there]",
            "say \"\" [text=]",
            "tag v [d=v, 3=z]",
        ]
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    // The macro is a monospace part holding the raw text, spanned over the
    // macro and highlighted as an inline macro.
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("expected a paragraph");
    };
    let macro_span = Span::new(2, 53);
    assert_eq!(
        &source[macro_span.start..macro_span.end],
        "tag:x&y[one, k=v,two,  three , opts=\"a, b\",role=rr]"
    );
    let found = inlines.iter().any(|inline| match inline {
        Inline::Substituted { parts, span, .. } => {
            *span == macro_span
                && parts.contains(&adoctra_core::AttributePart::Raw("<x&amp;y>".into()))
        }
        _ => false,
    });
    assert!(found, "{inlines:#?}");
    assert!(result
        .tokens
        .iter()
        .any(|token| token.span == macro_span && token.kind == TokenKind::InlineMacro));
}

#[test]
fn inline_macro_reports_sit_at_rubys_document_reader_mark() {
    let log = Log::default();
    // Ruby converts the paragraph after reading: the reader's mark is the
    // last block's first line (after its title), whatever the macro's line.
    let result = parse_with(
        "Para tag:err[] text.\n\n.Title\nlast\n",
        inline_registry(&log),
    );
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.message, "bad tag");
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.span, Span::new(5, 14));
    assert_eq!(
        diagnostic.report.location,
        adoctra_core::DiagnosticLocation::Cursor(adoctra_core::DiagnosticCursor {
            source_id: adoctra_core::SourceId::default(),
            line: 4,
        })
    );
    assert!(diagnostic.report.conversion);
}

#[test]
fn inline_macros_need_a_registered_processor() {
    let log = Log::default();
    let result = parse_with("tag:a[] say:[x]\n", registry(&log));
    assert!(log.lock().unwrap().is_empty());
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("expected a paragraph");
    };
    assert_eq!(
        inlines,
        &[Inline::Text {
            value: "tag:a[] say:[x]".into(),
            span: Span::new(0, 15),
        }]
    );
}
