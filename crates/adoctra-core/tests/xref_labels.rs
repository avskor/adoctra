//! Cross-reference labels are produced during conversion, like Ruby's
//! `xreftext`: reftext is formatted again at each reference, otherwise a
//! block's title (converted once, possibly by a forward reference) is used.
use adoctra_core::{
    parse, parse_with_options, Block, BlockKind, Inline, ParseOptions, Severity, SourceId,
};

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Link { children, .. }
            | Inline::CrossReference { children, .. } => text(children),
            Inline::Anchor { .. } => String::new(),
            _ => panic!("{node:?}"),
        })
        .collect()
}

fn inlines(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => inlines,
        _ => panic!("{block:?}"),
    }
}

fn xrefs(nodes: &[Inline]) -> Vec<&[Inline]> {
    let mut found = Vec::new();
    for node in nodes {
        match node {
            Inline::CrossReference { children, .. } => found.push(children.as_slice()),
            Inline::Strong { children, .. } | Inline::Link { children, .. } => {
                found.extend(xrefs(children))
            }
            _ => {}
        }
    }
    found
}

fn labels(block: &Block) -> Vec<String> {
    xrefs(inlines(block)).into_iter().map(text).collect()
}

#[test]
fn titled_blocks_lend_their_title_backward_and_forward() {
    let source = "See <<ex>>.\n\n[[code]]\n.Sample *code*\n----\nx\n----\n\n[[ex]]\n.Example title\n====\nbody\n====\n\n<<code>> <<ex>>";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.flat_blocks();
    assert_eq!(labels(blocks[0]), ["Example title"]);
    assert_eq!(labels(blocks[3]), ["Sample code", "Example title"]);
    // A borrowed title keeps its formatting and the title's source spans.
    let label = xrefs(inlines(blocks[3]))[0];
    let strong = label
        .iter()
        .find(|n| matches!(n, Inline::Strong { .. }))
        .unwrap();
    assert_eq!(&source[strong.span().start..strong.span().end], "*code*");
    assert_eq!(text(blocks[2].title.as_ref().unwrap()), "Example title");
    assert_eq!(blocks[2].caption.as_deref(), Some("Example 1. "));
}

#[test]
fn reftext_wins_and_an_empty_reftext_falls_back_to_the_title() {
    let doc = parse(
        ":empty:\n\n[[a,Ref]]\n.Title\n----\nx\n----\n\n[[b,{empty}]]\n.Fallback\n----\ny\n----\n\n[[c]]\n----\nz\n----\n\n<<a>> <<b>> <<c>>",
    )
    .document;
    assert_eq!(labels(doc.flat_blocks()[3]), ["Ref", "Fallback", "[c]"]);
}

#[test]
fn reftext_quote_metadata_uses_the_state_at_each_reference() {
    let source = "p\n\n[[b,[.r\\{counter:c}]#X#]]\npara\n\n<<b>> <<b>>\n\nc={c}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let doc = result.document;
    let roles: Vec<String> = xrefs(inlines(doc.flat_blocks()[2]))
        .into_iter()
        .map(|label| match &label[0] {
            Inline::Substituted { parts, .. } => format!("{parts:?}"),
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(roles[0].contains("\"r1\""), "{roles:?}");
    assert!(roles[1].contains("\"r2\""), "{roles:?}");
    // The counter advanced by the references reaches later body text.
    assert_eq!(text(inlines(doc.flat_blocks()[3])), "c=2");
    // The model's reftext is a read-only snapshot.
    let model = format!("{:?}", doc.flat_blocks()[1].reftext);
    assert!(model.contains("r{counter:c}"), "{model}");
}

#[test]
fn reftext_warnings_are_reported_per_reference_with_physical_spans() {
    let source = ":attribute-missing: warn\r\n\r\n[[b,[.Ж\\{nope}]#X#]]\r\npara\r\n\r\n<<b>> <<b>>";
    let options = ParseOptions {
        source_id: SourceId(3),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
    for warning in &result.diagnostics {
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.span.source_id, SourceId(3));
        assert_eq!(&source[warning.span.start..warning.span.end], "\\{nope}");
    }
    // Without a reference, Ruby never formats the reftext.
    let unreferenced = parse(":attribute-missing: warn\n\n[[b,[.r\\{nope}]#X#]]\npara");
    assert!(unreferenced.diagnostics.is_empty());
}

#[test]
fn inline_and_section_anchor_reftext_is_formatted_per_reference() {
    let doc = parse(
        ":x: 0\n\nx [[a,[.r\\{x}]#A#]] y\n\n[[s,[.s\\{x}]#S#]]\n== Sec\n\n:x: 1\n\n<<a>> <<s>>\n\n:x: 2\n\n<<a>> <<s>>",
    )
    .document;
    let roles = |block: &Block| format!("{:?}", inlines(block));
    assert!(roles(doc.flat_blocks()[2]).contains("\"r1\""));
    assert!(roles(doc.flat_blocks()[2]).contains("\"s1\""));
    assert!(roles(doc.flat_blocks()[3]).contains("\"r2\""));
    assert!(roles(doc.flat_blocks()[3]).contains("\"s2\""));
}

#[test]
fn an_anchored_stateful_title_is_converted_once_while_reading() {
    // An anchored title with a literal `{` is cached while reading; its
    // effects do not reach conversion, so the body counter restarts.
    let doc = parse(
        "See <<t>> and <<t>>.\n\n[[t]]\n.Item {counter:n}\n----\nx\n----\n\nAfter {counter:n}.",
    )
    .document;
    assert_eq!(labels(doc.flat_blocks()[0]), ["Item 1", "Item 1"]);
    assert_eq!(text(doc.flat_blocks()[1].title.as_ref().unwrap()), "Item 1");
    assert_eq!(text(inlines(doc.flat_blocks()[2])), "After 1.");
}

#[test]
fn references_inside_a_label_display_their_id() {
    let doc =
        parse("See <<t>>.\n\n[[t]]\n.About <<u>>\n----\nx\n----\n\n[[u]]\n.U title\n----\ny\n----")
            .document;
    assert_eq!(labels(doc.flat_blocks()[0]), ["About [u]"]);
    // The title converted for the reference is the block's title too.
    let title = doc.flat_blocks()[1].title.as_ref().unwrap();
    assert_eq!(text(title), "About [u]");
    assert_eq!(xrefs(title).len(), 1);

    let doc = parse("[[t]]\n.Self <<t>>\n----\nx\n----\n\nSee <<t>>.").document;
    assert_eq!(
        text(doc.flat_blocks()[0].title.as_ref().unwrap()),
        "Self Self [t]"
    );
    assert_eq!(labels(doc.flat_blocks()[1]), ["Self Self [t]"]);
}

#[test]
fn borrowed_labels_drop_links_and_keep_their_text() {
    let doc = parse(
        "[[s]]\n== See https://example.org[site] and <<t>>\n\n[[t]]\n.Target\n----\nx\n----\n\n<<s>>",
    )
    .document;
    let label = xrefs(inlines(doc.flat_blocks()[2]))[0];
    assert_eq!(text(label), "See site and Target");
    assert!(label
        .iter()
        .all(|n| !matches!(n, Inline::Link { .. } | Inline::CrossReference { .. })));
    // The heading itself keeps its link and reference.
    assert_eq!(xrefs(inlines(doc.flat_blocks()[0])).len(), 1);
}

#[test]
fn automatic_section_ids_use_references_resolved_while_reading() {
    let doc = parse("== See <<later>>\n\n[[later]]\n.Later\n----\nx\n----").document;
    assert_eq!(doc.flat_blocks()[0].id.as_deref(), Some("_see_later"));
    assert_eq!(labels(doc.flat_blocks()[0]), ["[later]"]);

    let doc = parse("[[early]]\n.Early\n----\nx\n----\n\n== See <<early>>\n\nbody").document;
    assert_eq!(doc.flat_blocks()[1].id.as_deref(), Some("_see_early"));
    assert_eq!(labels(doc.flat_blocks()[1]), ["Early"]);

    let doc = parse("[[a,Alpha]]\n----\nx\n----\n\n== See <<a>>\n\n<<_see_alpha>>").document;
    assert_eq!(doc.flat_blocks()[1].id.as_deref(), Some("_see_alpha"));
    assert_eq!(labels(doc.flat_blocks()[2]), ["See Alpha"]);

    let doc = parse("== See <<_see_see>>\n\nbody").document;
    assert_eq!(doc.flat_blocks()[0].id.as_deref(), Some("_see_see_see"));

    // An explicit id defers the title's references to conversion.
    let doc = parse("[[s]]\n== See <<later>>\n\n[[later]]\n.Later\n----\nx\n----").document;
    assert_eq!(labels(doc.flat_blocks()[0]), ["Later"]);
}

#[test]
fn a_title_converted_while_reading_keeps_the_reading_state() {
    let doc =
        parse("[[early]]\n.Early {counter:n}\n----\nx\n----\n\n== See <<early>>\n\n{counter:n}")
            .document;
    assert_eq!(
        text(doc.flat_blocks()[0].title.as_ref().unwrap()),
        "Early 1"
    );
    assert_eq!(doc.flat_blocks()[1].id.as_deref(), Some("_see_early_1"));
    assert_eq!(text(inlines(doc.flat_blocks()[2])), "1");
}

#[test]
fn unresolved_references_warn_once_at_their_physical_span() {
    let source = "== See <<nope>>\r\n\r\n.T <<gone>>\r\n----\r\nx\r\n----\r\n\r\nЖ <<nope>>";
    let options = ParseOptions {
        source_id: SourceId(9),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let spans: Vec<&str> = result
        .diagnostics
        .iter()
        .inspect(|d| {
            assert_eq!(d.severity, Severity::Warning);
            assert_eq!(d.span.source_id, SourceId(9));
        })
        .map(|d| &source[d.span.start..d.span.end])
        .collect();
    assert_eq!(spans, ["<<nope>>", "<<gone>>", "<<nope>>"]);
    let doc = result.document;
    assert_eq!(labels(doc.flat_blocks()[0]), ["[nope]"]);
    assert_eq!(xrefs(doc.flat_blocks()[1].title.as_ref().unwrap()).len(), 1);
    assert_eq!(labels(doc.flat_blocks()[2]), ["[nope]"]);
}

#[test]
fn incomplete_references_and_titles_do_not_panic() {
    for source in [
        "[[t]]\n.<<t",
        "[[t]]\n.<<t>>",
        "See <<t>>\n\n[[t]]\n.",
        "== <<_\n\n<<_>>",
        "[[b,[.r\\{counter:c}]#X",
        "<<b>>\n\n[[b,[.r\\{counter:c}]#X#]]",
    ] {
        let result = parse(source);
        assert!(result
            .diagnostics
            .iter()
            .all(|d| source.get(d.span.start..d.span.end).is_some()));
    }
}
