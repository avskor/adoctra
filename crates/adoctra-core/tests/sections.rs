use adoctra_core::{
    parse, parse_resource, tokens, tokens_resource, Block, BlockKind, MemoryResources,
    ParseOptions, Severity, SourceId, SourceMap, Span, TokenKind,
};

/// The tree as `(kind, level, text)` lines, indented by depth.
fn outline(blocks: &[Block], source: &str, depth: usize, out: &mut Vec<String>) {
    for block in blocks {
        let pad = "  ".repeat(depth);
        match &block.kind {
            BlockKind::Section { level, blocks } => {
                out.push(format!("{pad}section {level}"));
                outline(blocks, source, depth + 1, out);
            }
            BlockKind::Preamble { blocks } => {
                out.push(format!("{pad}preamble"));
                outline(blocks, source, depth + 1, out);
            }
            BlockKind::Heading { level, .. } => out.push(format!(
                "{pad}h{level} {}",
                &source[block.span.start..block.span.end]
            )),
            BlockKind::Paragraph { .. } => out.push(format!(
                "{pad}p {}",
                &source[block.span.start..block.span.end]
            )),
            BlockKind::Example { blocks } => {
                out.push(format!("{pad}example"));
                outline(blocks, source, depth + 1, out);
            }
            other => out.push(format!("{pad}{other:?}")),
        }
    }
}

fn tree(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    outline(&parse(source).document.blocks, source, 0, &mut out);
    out
}

fn messages(source: &str) -> Vec<(Severity, String, &str)> {
    parse(source)
        .diagnostics
        .into_iter()
        .map(|d| (d.severity, d.message, &source[d.span.start..d.span.end]))
        .collect()
}

#[test]
fn sections_nest_by_level_with_a_preamble_after_the_header() {
    let source = "= Doc\n\nPre\n\n== A\n\na\n\n=== B\n\nb\n\n== C";
    assert_eq!(
        tree(source),
        [
            "h0 = Doc",
            "preamble",
            "  p Pre",
            "section 1",
            "  h1 == A",
            "  p a",
            "  section 2",
            "    h2 === B",
            "    p b",
            "section 1",
            "  h1 == C",
        ]
    );
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(blocks[1].span, Span::new(7, 10));
    // A section spans its title through its last nested block.
    assert_eq!(
        &source[blocks[2].span.start..blocks[2].span.end],
        "== A\n\na\n\n=== B\n\nb"
    );
    assert_eq!(&source[blocks[3].span.start..blocks[3].span.end], "== C");
    // Ids, roles and attributes stay on the title heading.
    let BlockKind::Section { blocks: a, .. } = &blocks[2].kind else {
        panic!()
    };
    assert_eq!(a[0].id.as_deref(), Some("_a"));
    assert!(blocks[2].id.is_none());
    // The flat views keep the previous document order.
    let flat: Vec<_> = result
        .document
        .flat_blocks()
        .iter()
        .map(|b| &source[b.span.start..b.span.end])
        .collect();
    assert_eq!(flat, ["= Doc", "Pre", "== A", "a", "=== B", "b", "== C"]);
    assert_eq!(result.document.clone().into_flat_blocks().len(), 7);
}

#[test]
fn preamble_requires_a_title_and_a_section_except_in_a_book() {
    assert_eq!(tree("Pre\n\n== A"), ["p Pre", "section 1", "  h1 == A"]);
    assert_eq!(
        tree("= Doc\n\nPre\n\nMore"),
        ["h0 = Doc", "p Pre", "p More"]
    );
    assert_eq!(
        tree("= Doc\n\n== A"),
        ["h0 = Doc", "section 1", "  h1 == A"]
    );
    assert_eq!(
        tree(":doctitle: X\n\nPre\n\n== A"),
        [
            "h0 :doctitle: X",
            "preamble",
            "  p Pre",
            "section 1",
            "  h1 == A"
        ]
    );
    assert_eq!(
        tree(":doctype: book\n= Doc\n\nPre"),
        ["h0 = Doc", "preamble", "  p Pre"]
    );
    // Discrete headings are content, never sections.
    assert_eq!(
        tree("= Doc\n\nPre\n\n[discrete]\n== D\n\nx"),
        ["h0 = Doc", "p Pre", "h1 == D", "p x"]
    );
}

#[test]
fn skipped_levels_warn_and_article_parts_are_errors() {
    let source = "= Doc\n\n=== Deep\n\n== A\n\n==== D\n\n= P\n\n=== Q";
    assert_eq!(
        messages(source),
        [
            (
                Severity::Warning,
                "section title out of sequence: expected level 1, got level 2".into(),
                "=== Deep"
            ),
            (
                Severity::Warning,
                "section title out of sequence: expected level 2, got level 3".into(),
                "==== D"
            ),
            (
                Severity::Error,
                "level 0 sections can only be used when doctype is book".into(),
                "= P"
            ),
            (
                Severity::Warning,
                "section title out of sequence: expected level 1, got level 2".into(),
                "=== Q"
            ),
        ]
    );
    assert_eq!(
        tree(source),
        [
            "h0 = Doc",
            "section 2",
            "  h2 === Deep",
            "section 1",
            "  h1 == A",
            "  section 3",
            "    h3 ==== D",
            "section 0",
            "  h0 = P",
            "  section 2",
            "    h2 === Q",
        ]
    );
    // A book accepts parts and names both expected levels.
    assert_eq!(
        messages(":doctype: book\n= Doc\n\n= P\n\n== A\n\n=== Deep"),
        []
    );
    assert_eq!(
        messages(":doctype: book\n= Doc\n\n=== Deep"),
        [(
            Severity::Warning,
            "section title out of sequence: expected levels 0 or 1, got level 2".into(),
            "=== Deep"
        )]
    );
}

#[test]
fn leading_sections_without_a_header_are_not_checked() {
    let source = "=== Deep\n\n== A\n\n= P\n\n=== Q";
    assert_eq!(
        messages(source),
        [(
            Severity::Warning,
            "section title out of sequence: expected level 1, got level 2".into(),
            "=== Q"
        )]
    );
    // Content before the first section enables the checks again.
    let checked = messages("Pre\n\n=== Deep\n\n= P");
    assert_eq!(checked.len(), 2);
    assert_eq!(checked[1].0, Severity::Error);
    // So does a leading discrete heading.
    assert_eq!(messages("[discrete]\n== D\n\n=== Deep").len(), 1);
}

#[test]
fn section_titles_are_text_inside_containers_paragraphs_and_lists() {
    for source in [
        "Para\n== Title\nmore",
        "====\n== In\n====",
        "--\n== In\n--",
        "____\n== In\n____",
        "****\n== In\n****",
    ] {
        let result = parse(source);
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let debug = format!("{:?}", result.document.blocks);
        assert!(!debug.contains("Heading"), "{source:?}: {debug}");
        assert!(debug.contains("Paragraph"), "{source:?}: {debug}");
        assert!(!tokens(source).iter().any(|t| t.kind == TokenKind::Heading));
    }
    // A paragraph continues across the title line.
    assert_eq!(tree("Para\n== Title\nmore"), ["p Para\n== Title\nmore"]);
    for (source, attached) in [("* item\n== T", false), ("* item\n+\n== U", true)] {
        let result = parse(source);
        let BlockKind::List { items, .. } = &result.document.blocks[0].kind else {
            panic!("{:?}", result.document.blocks)
        };
        if attached {
            assert!(matches!(
                items[0].blocks[0].kind,
                BlockKind::Paragraph { .. }
            ));
        } else {
            assert!(format!("{:?}", items[0].inlines).contains("== T"));
        }
    }
    // A discrete heading is still recognized anywhere.
    let source = "====\n[discrete]\n== In\n====";
    let BlockKind::Example { blocks } = &parse(source).document.blocks[0].kind else {
        panic!()
    };
    assert!(matches!(
        blocks[0].kind,
        BlockKind::Heading { level: 1, .. }
    ));
    assert!(tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::Heading && &source[t.span.start..t.span.end] == "== In"));
}

#[test]
fn unicode_crlf_and_partial_input_keep_section_spans() {
    let source = "= Документ\r\n\r\nВступление\r\n\r\n== Глава 🙂\r\n\r\nТекст\r\n\r\n=== ";
    let result = parse(source);
    let blocks = &result.document.blocks;
    assert!(matches!(blocks[1].kind, BlockKind::Preamble { .. }));
    assert_eq!(
        &source[blocks[1].span.start..blocks[1].span.end],
        "Вступление"
    );
    assert!(matches!(
        blocks[2].kind,
        BlockKind::Section { level: 1, .. }
    ));
    // The trailing incomplete `=== ` is paragraph text inside the section.
    assert_eq!(
        &source[blocks[2].span.start..blocks[2].span.end],
        "== Глава 🙂\r\n\r\nТекст\r\n\r\n==="
    );
    for partial in [
        "== ",
        "==",
        "= Doc\n\n==",
        "= Doc\n\n== A\n\n=== B\n\n=",
        "\n\n== A\n",
    ] {
        let _ = parse(partial);
        let _ = tokens(partial);
    }
}

#[test]
fn sections_span_include_sources_with_provenance() {
    let mut resources = MemoryResources::new();
    resources
        .insert(
            "/docs/main.adoc",
            "= Doc\n\nPre\n\ninclude::part.adoc[]\n\n== C\n\nz",
        )
        .unwrap();
    resources
        .insert("/docs/part.adoc", "== A\r\n\r\nx\r\n\r\n=== B\r\n\r\ny\r\n")
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert!(matches!(blocks[1].kind, BlockKind::Preamble { .. }));
    // Section A lies entirely in the included file.
    assert_eq!(blocks[2].span.source_id, SourceId(1));
    assert_eq!(
        sources.slice(blocks[2].span),
        Some("== A\r\n\r\nx\r\n\r\n=== B\r\n\r\ny")
    );
    assert_eq!(sources.slice(blocks[3].span), Some("== C\n\nz"));
    // Tokens are ordered by source, then position.
    let headings: Vec<_> = tokens_resource(&mut sources, &options, &resources)
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == TokenKind::Heading)
        .map(|t| (t.span.source_id, sources.slice(t.span).unwrap().to_owned()))
        .collect();
    assert_eq!(
        headings,
        [
            (source_id, "= Doc".into()),
            (source_id, "== C".into()),
            (SourceId(1), "== A".into()),
            (SourceId(1), "=== B".into()),
        ]
    );
}

#[test]
fn ids_references_captions_and_snapshots_cross_sections() {
    let source =
        "= Doc\n\n<<_b>>\n\n== A\n:v: one\n\n.One\n====\nx\n====\n\n=== B\n\n.Two\n====\n{v}\n====";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let flat = result.document.flat_blocks();
    let captions: Vec<_> = flat.iter().filter_map(|b| b.caption.as_deref()).collect();
    assert_eq!(captions, ["Example 1. ", "Example 2. "]);
    assert!(format!("{:?}", flat[1].kind).contains("\"B\""));
    let section = &result.document.blocks[2];
    // A section's snapshot is the state when its last block completed.
    assert_eq!(section.attribute_context.get("v"), Some("one"));
}
