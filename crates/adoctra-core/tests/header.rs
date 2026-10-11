use adoctra_core::{
    parse, parse_with_options, tokens_with_options, Attributes, BlockKind, ParseOptions, SourceMap,
    TokenKind,
};

#[test]
fn authors_revision_attributes_and_original_ranges() {
    let source = "= Документ\r\nИван Петров <ivan@example.org>; Mary_Jane Q Public\r\nv1.2, 2026-10-11: Выпуск\r\n\r\n{authors} / {revnumber}";
    let mut sources = SourceMap::new();
    sources.add("unused", "");
    let id = sources.add("doc.adoc", source);
    let options = ParseOptions {
        source_id: id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty());
    let doc = result.document;
    assert_eq!(doc.blocks.len(), 2);
    let header = doc.header.unwrap();
    assert_eq!(sources.slice(header.title_span), Some("= Документ"));
    assert_eq!(
        sources.slice(header.authors[0].span),
        Some("Иван Петров <ivan@example.org>")
    );
    assert_eq!(header.authors[0].initials, "ИП");
    assert_eq!(header.authors[1].firstname, "Mary Jane");
    assert_eq!(header.authors[1].middlename.as_deref(), Some("Q"));
    let revision = header.revision.unwrap();
    assert_eq!(
        sources.slice(revision.span),
        Some("v1.2, 2026-10-11: Выпуск")
    );
    assert_eq!(revision.number.as_deref(), Some("1.2"));
    assert_eq!(revision.date.as_deref(), Some("2026-10-11"));
    assert_eq!(revision.remark.as_deref(), Some("Выпуск"));
    assert_eq!(
        sources.slice(header.span),
        Some(source.split("\r\n\r\n").next().unwrap())
    );
    for (key, value) in [
        ("authorcount", "2"),
        ("author", "Иван Петров"),
        ("author_1", "Иван Петров"),
        ("email_1", "ivan@example.org"),
        ("author_2", "Mary Jane Q Public"),
        ("authorinitials_2", "MQP"),
        ("authors", "Иван Петров, Mary Jane Q Public"),
    ] {
        assert_eq!(doc.attributes.get(key), Some(value), "{key}");
    }
    let tokens = tokens_with_options(source, &options);
    assert_eq!(tokens[1].kind, TokenKind::Author);
    assert_eq!(
        sources.slice(tokens[1].span),
        Some("Иван Петров <ivan@example.org>; Mary_Jane Q Public")
    );
    assert_eq!(tokens[2].kind, TokenKind::Revision);
    assert_eq!(tokens[2].span, revision.span);
    assert!(tokens.iter().all(|token| token.span.source_id == id));
}

#[test]
fn attributes_between_header_lines_and_hard_overrides() {
    let mut attributes = Attributes::new();
    attributes.set("revnumber", "locked");
    let source = "= Doc\n:domain: example.org\nJohn Doe <john@{domain}>\n:revdate: explicit date\nv2, today: Release\n:extra: {author} {revnumber}\n\n{extra}";
    let doc = parse_with_options(
        source,
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.get("email"), Some("john@example.org"));
    assert_eq!(doc.attributes.get("revdate"), Some("explicit date"));
    assert_eq!(doc.attributes.get("revnumber"), Some("locked"));
    assert_eq!(doc.attributes.get("extra"), Some("John Doe locked"));
    // Source declarations remain distinguishable from effective overrides.
    assert_eq!(
        doc.header.unwrap().revision.unwrap().number.as_deref(),
        Some("2")
    );
}

#[test]
fn blank_line_ends_header_and_title_is_required() {
    let doc = parse("= Doc\n\nJohn Doe\nv1.0").document;
    let header = doc.header.unwrap();
    assert!(header.authors.is_empty());
    assert!(header.revision.is_none());
    assert!(matches!(doc.blocks[1].kind, BlockKind::Paragraph { .. }));
    let doc = parse("John Doe\nv1.0\n\nBody").document;
    assert!(doc.header.is_none());
    assert!(doc.attributes.get("author").is_none());
    let doc = parse("= Doc\nJohn Doe\n\nv1.0").document;
    assert!(doc.header.unwrap().revision.is_none());
    assert_eq!(doc.blocks.len(), 2);
}

#[test]
fn revision_variants_and_body_boundary() {
    for (line, number, date, remark) in [
        ("v1.0", Some("1.0"), None, None),
        ("2026-10-11", None, Some("2026-10-11"), None),
        ("1.0, Jan 01, 2026", Some("1.0"), Some("Jan 01, 2026"), None),
        ("v1.0: Draft", Some("1.0"), None, Some("Draft")),
        (
            "v1.0, today: Draft",
            Some("1.0"),
            Some("today"),
            Some("Draft"),
        ),
        (
            "release candidate, today: note",
            Some(""),
            Some("today"),
            Some("note"),
        ),
    ] {
        let doc = parse(&format!("= Doc\nJohn Doe\n{line}\nBody")).document;
        let rev = doc.header.unwrap().revision.unwrap();
        assert_eq!(
            (
                rev.number.as_deref(),
                rev.date.as_deref(),
                rev.remark.as_deref()
            ),
            (number, date, remark),
            "{line}"
        );
        assert_eq!(doc.blocks.len(), 2);
    }
    let doc = parse("= Doc\nJohn Doe\n:invalid:value\n\nBody").document;
    assert!(doc.header.unwrap().revision.is_none());
    assert_eq!(doc.blocks.len(), 3);
}

#[test]
fn explicit_names_are_derived_and_implicit_declarations_are_retained() {
    let doc = parse("= Doc\nJohn Doe\n:author: Jane Smith\n\n{author}").document;
    assert_eq!(doc.attributes.get("firstname"), Some("Jane"));
    assert_eq!(doc.attributes.get("lastname"), Some("Smith"));
    assert_eq!(doc.attributes.get("authorinitials"), Some("JS"));
    assert_eq!(doc.header.unwrap().authors[0].name, "John Doe");
    let doc = parse(":authors: Jane Smith; Иван Петров\n\n{authors}").document;
    assert!(doc.header.is_none());
    assert_eq!(doc.attributes.get("authorcount"), Some("2"));
    assert_eq!(doc.attributes.get("authorinitials_2"), Some("ИП"));
}

#[test]
fn unfinished_and_unconventional_authors_do_not_panic() {
    for source in [
        "=",
        "= Doc",
        "= Doc\n; ",
        "= Doc\n<",
        "= Doc\nJohn <",
        "= Doc\n  ",
        "= Doc\nJohn Doe\nv",
        "= Doc\nJohn Doe\n:",
    ] {
        let _ = parse(source);
    }
    let doc = parse("= Doc\nResearch Team of Four <team@example.org>\n\nBody").document;
    assert_eq!(
        doc.attributes.get("firstname"),
        Some("Research Team of Four <team@example.org>")
    );
    assert!(doc.attributes.get("email").is_none());
    let doc = parse("= Doc\nJohn Doe;Jane Smith\n\nBody").document;
    assert_eq!(doc.attributes.get("authorcount"), Some("1"));
}

#[test]
fn explicit_initials_survive_author_name_derivation() {
    let doc = parse(":author: Jane Smith\n:authorinitials: custom\n\n{authorinitials}").document;
    assert_eq!(doc.attributes.get("authorinitials"), Some("custom"));
    let doc = parse("= Doc\nJohn Doe\n:author: Jane Smith\n:!authorinitials:\n\nBody").document;
    assert!(doc.attributes.get("authorinitials").is_none());
}
