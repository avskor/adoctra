//! Single divergences found by the upstream test suite audit
//! (scripts/upstream_audit.py), each pinned to its Asciidoctor test.
use adoctra_core::{parse, parse_with_options, Block, BlockKind, Inline, ParseOptions, Severity};

fn with(attributes: &[(&str, &str)]) -> ParseOptions {
    let mut options = ParseOptions::default();
    for (name, value) in attributes {
        options.attributes.set(*name, *value);
    }
    options
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } => value.clone(),
            Inline::Strong { children, .. }
            | Inline::Link { children, .. }
            | Inline::Monospace { children, .. } => text(children),
            Inline::Substituted { value, .. } | Inline::Raw { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            other => format!("{other:?}"),
        })
        .collect()
}

fn paragraph(block: &Block) -> String {
    match &block.kind {
        BlockKind::Paragraph { inlines } => text(inlines),
        other => panic!("not a paragraph: {other:?}"),
    }
}

fn messages(source: &str, options: &ParseOptions) -> Vec<(Severity, String)> {
    parse_with_options(source, options)
        .diagnostics
        .into_iter()
        .map(|d| (d.severity, d.report.message.unwrap_or(d.message)))
        .collect()
}

#[test]
fn revision_line_with_an_empty_date() {
    // document_test.rb: "should parse revision line if date is empty".
    let header = parse("= Document Title\nAuthor Name\nv1.0.0,:remark\n\ncontent")
        .document
        .header
        .clone()
        .unwrap();
    let revision = header.revision.unwrap();
    assert_eq!(revision.number.as_deref(), Some("1.0.0"));
    assert_eq!(revision.date, None);
    assert_eq!(revision.remark.as_deref(), Some("remark"));
}

#[test]
fn quote_attribution_from_named_attributes_and_a_leading_comma() {
    // attributes_test.rb: "attribute list may begin with comma" and "normal
    // substitutions are performed on single-quoted named attribute".
    let attribution = |source: &str| match parse(source).document.blocks.remove(0).kind {
        BlockKind::Quote { attribution, .. } => attribution.map(|a| {
            (
                a.author.as_deref().map(text),
                a.citetitle.as_deref().map(text),
            )
        }),
        other => panic!("{other:?}"),
    };
    let some = |a: &str, c: &str| Some((Some(a.to_owned()), Some(c.to_owned())));
    assert_eq!(
        attribution("[, author, source]\n____\nq\n____"),
        some("author", "source")
    );
    assert_eq!(
        attribution("[quote, author, citetitle='http://wikipedia.org[source]']\n____\nq\n____"),
        some("author", "source")
    );
    assert_eq!(
        attribution("[quote, attribution=A, citetitle=W]\n____\nq\n____"),
        some("A", "W")
    );
    // A positional value wins over a named one (Ruby's rekey).
    assert_eq!(
        attribution("[quote, P, attribution=A, citetitle=W]\n____\nq\n____"),
        some("P", "W")
    );
}

#[test]
fn counter_on_the_locked_built_in_max_include_depth() {
    // attributes_test.rb: counters may not modify a locked built-in attribute.
    let document = parse(
        "{counter:max-include-depth:128} is one more than {max-include-depth}\n\n\
         {counter2:max-include-depth:128}{max-include-depth}",
    )
    .document;
    assert_eq!(paragraph(&document.blocks[0]), "65 is one more than 64");
    assert_eq!(paragraph(&document.blocks[1]), "64");
    assert_eq!(document.attributes.get("max-include-depth"), Some("64"));
    // A host value is kept (Ruby's `||= 64`).
    let document =
        parse_with_options("{max-include-depth}", &with(&[("max-include-depth", "8")])).document;
    assert_eq!(paragraph(&document.blocks[0]), "8");
}

#[test]
fn block_title_above_the_document_title_cancels_the_header() {
    // blocks_test.rb: the title line becomes a level-0 section.
    let source = ".Block title\n= Section Title\n\nsection paragraph";
    assert_eq!(
        messages(source, &ParseOptions::default()),
        [(
            Severity::Error,
            "level 0 sections can only be used when doctype is book".to_owned()
        )]
    );
    assert!(!parse(source).document.has_title);
    // A body that simply starts with a section still skips the check.
    assert!(messages("== A\n\n= B", &ParseOptions::default()).is_empty());
}

#[test]
fn fragment_accepts_any_top_level_section_level() {
    // sections_test.rb: `fragment` only relaxes the top level.
    let fragment = with(&[("fragment", "")]);
    assert!(messages(
        "= Document Title\n\n=== First Section\n\ncontent",
        &fragment
    )
    .is_empty());
    assert_eq!(
        messages(
            "= Document Title\n\n=== First Section\n\n===== Nested Section",
            &fragment
        ),
        [(
            Severity::Warning,
            "section title out of sequence: expected level 3, got level 4".to_owned()
        )]
    );
}

#[test]
fn sectnums_set_by_the_api_can_be_toggled_by_the_document() {
    // sections_test.rb: Ruby's FLEXIBLE_ATTRIBUTES; an API unset stays.
    let source = "= T\n\n:numbered!:\n\n== A\n\n:numbered:\n\n== B\n";
    let document = parse_with_options(source, &with(&[("numbered", "")])).document;
    let attributes: Vec<_> = document
        .flat_blocks()
        .iter()
        .filter(|b| matches!(b.kind, BlockKind::Heading { level: 1, .. }))
        .map(|b| b.conversion_attributes().get("sectnums").is_some())
        .collect();
    assert_eq!(attributes, [false, true]);
    let document =
        parse_with_options("= T\n\n:sectnums:\n\n== A", &with(&[("sectnums!", "")])).document;
    assert!(document.flat_blocks()[1]
        .conversion_attributes()
        .get("sectnums")
        .is_none());
}

#[test]
fn a_text_icon_contributes_its_alt_to_a_section_id() {
    // sections_test.rb: "synthetic id removes XML tags" (`[gear&#93;`).
    let id = |options: &ParseOptions| {
        parse_with_options("== Use the `run` command to make it icon:gear[]", options)
            .document
            .flat_blocks()
            .into_iter()
            .find_map(|b| b.id.clone())
            .unwrap()
    };
    assert_eq!(
        id(&ParseOptions::default()),
        "_use_the_run_command_to_make_it_gear"
    );
    assert_eq!(
        id(&with(&[("icons", "font")])),
        "_use_the_run_command_to_make_it"
    );
}

#[test]
fn an_escaped_backtick_after_legacy_attributes_is_not_a_passthrough() {
    // substitutions_test.rb: "[x-]\`leave it alone`" stays text.
    let document = parse("[x-]\\`leave it alone`\n\n[x-]`code`").document;
    assert_eq!(paragraph(&document.blocks[0]), "[x-]`leave it alone`");
    assert_ne!(paragraph(&document.blocks[1]), "[x-]`code`");
}

#[test]
fn a_folded_indented_description_loses_its_common_indentation() {
    // lists_test.rb: adjust_indentation! on the folded paragraph's lines.
    let source = "term1::\n  def1\n  -- and a note\n\nterm2::\n  def2";
    let document = parse(source).document;
    let BlockKind::DescriptionList { items } = &document.blocks[0].kind else {
        panic!("{:?}", document.blocks[0].kind);
    };
    let description = items[0].description.as_ref().unwrap();
    assert_eq!(
        text(&description.inlines),
        "def1\u{2009}\u{2014}\u{2009}and a note"
    );
    // Spans stay physical: the note starts after its indentation.
    let note = description
        .inlines
        .iter()
        .rev()
        .find_map(|inline| match inline {
            Inline::Text { value, span } if value.contains("and a note") => Some(*span),
            _ => None,
        })
        .unwrap();
    assert!(source[note.start..note.end].ends_with("and a note"));
}

#[test]
fn a_continuation_placeholder_attaches_to_the_nested_term() {
    // lists_test.rb: "nested dlist with attached block offset by empty line".
    let document = parse("category::\n\nterm 1:::\n+\n--\ndef 1\n--").document;
    let BlockKind::DescriptionList { items } = &document.blocks[0].kind else {
        panic!();
    };
    let outer = items[0].description.as_ref().unwrap();
    let [nested] = outer.blocks.as_slice() else {
        panic!("{:?}", outer.blocks);
    };
    let BlockKind::DescriptionList { items } = &nested.kind else {
        panic!("{:?}", nested.kind);
    };
    let inner = items[0].description.as_ref().unwrap();
    assert!(matches!(inner.blocks[0].kind, BlockKind::Open { .. }));
}

#[test]
fn an_unterminated_container_warns_before_its_children() {
    // sections_test.rb: Ruby reads the quote's lines before parsing them.
    assert_eq!(
        messages("____\n----", &ParseOptions::default()),
        [
            (Severity::Error, "unterminated quote block".to_owned()),
            (Severity::Error, "unterminated listing block".to_owned()),
        ]
    );
}

/// syntax_highlighter_test.rb:235 "should rename document attribute named
/// language to source-language when compat-mode is enabled": Ruby's
/// save_attributes copies `language` to `source-language` in compat mode,
/// which then defaults listings to source.
#[test]
fn compat_mode_copies_language_to_source_language() {
    let source = ":language: ruby\n\n{source-language}\n\n----\nputs 1\n----\n";
    let doc = parse_with_options(source, &with(&[("compat-mode", "")])).document;
    assert_eq!(doc.attributes.get("source-language"), Some("ruby"));
    assert_eq!(paragraph(&doc.blocks[0]), "ruby");
    assert_eq!(doc.blocks[1].attrs.style.as_deref(), Some("source"));
    assert_eq!(
        doc.blocks[1]
            .attrs
            .named
            .get("language")
            .map(String::as_str),
        Some("ruby")
    );
    // Without compat mode the attribute stays unset.
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("source-language"), None);
    assert_eq!(doc.blocks[1].attrs.style, None);
}
