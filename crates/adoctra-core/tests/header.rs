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

#[test]
fn setext_header_uses_character_lengths_and_retains_two_line_span() {
    for (title, underline) in [
        ("Doc", "=="),
        ("Doc", "==="),
        ("Doc", "===="),
        ("A", "="),
        ("١", "="),
        ("Ⅳ", "="),
        ("𝟡", "="),
        ("Я🙂", "=="),
        (" Doc", "===="),
        ("*Doc*", "====="),
    ] {
        let source = format!("{title}\n{underline}\nJane Doe\nv2\n\n{{author}}");
        let result = parse(&source);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        let header = result.document.header.as_ref().unwrap();
        assert_eq!(
            header.title_span,
            adoctra_core::Span::new(0, title.len() + 1 + underline.len())
        );
        assert_eq!(result.document.blocks[0].span, header.title_span);
        assert!(matches!(
            result.document.blocks[0].kind,
            BlockKind::Heading { level: 0, .. }
        ));
        assert_eq!(header.authors[0].name, "Jane Doe");
        assert_eq!(
            header.revision.as_ref().unwrap().number.as_deref(),
            Some("2")
        );
        assert_eq!(result.document.attributes.get("compat-mode"), Some(""));
        let highlighted = adoctra_core::tokens(&source);
        assert_eq!(
            highlighted
                .iter()
                .filter(|t| t.kind == TokenKind::Heading)
                .count(),
            2
        );
        assert_eq!(highlighted[0].span, adoctra_core::Span::new(0, title.len()));
        assert_eq!(
            highlighted[1].span,
            adoctra_core::Span::new(title.len() + 1, title.len() + 1 + underline.len())
        );
    }
}

#[test]
fn setext_rejects_non_titles_and_does_not_cross_comments_or_blank_lines() {
    for source in [
        "Doc",
        "Doc\n=",
        "Doc\n=====",
        ".Doc\n====",
        "🙂\n=",
        "²\n=",
        "½\n=",
        "***\n===",
        "Doc\n= =",
        "Doc\n ===",
        "Doc\n\n===",
        "Doc\n// comment\n===",
        "Doc\n---",
        "Doc\n~~~",
        "Doc\n^^^",
        "Doc\n+++",
        "Before\n\nDoc\n===",
    ] {
        assert!(parse(source).document.header.is_none(), "{source:?}");
    }
    // ATX takes precedence; the underline is the author declaration here.
    let result = parse("= Doc\n=====\n\nBody");
    let header = result.document.header.unwrap();
    assert_eq!(header.title_span, adoctra_core::Span::new(0, 5));
    assert_eq!(header.authors[0].name, "=====");
    assert!(result.document.attributes.get("compat-mode").is_none());
}

#[test]
fn setext_leveloffset_and_compat_mode_use_header_context() {
    for (marker, level) in [('=', 0), ('-', 1), ('~', 2), ('^', 3), ('+', 4)] {
        let source = format!(
            ":leveloffset: -{level}\nDoc\n{marker}{marker}{marker}\n:leveloffset: 0\n\nBody"
        );
        let result = parse(&source);
        assert!(result.document.header.is_some(), "{source}");
        assert!(matches!(
            result.document.blocks[0].kind,
            BlockKind::Heading { level: 0, .. }
        ));
        assert_eq!(
            adoctra_core::tokens(&source)
                .iter()
                .filter(|t| t.kind == TokenKind::Heading)
                .count(),
            2
        );
    }
    assert!(parse(":leveloffset: -1\nDoc\n===")
        .document
        .header
        .is_none());
    assert_eq!(
        parse(":compat-mode: custom\nDoc\n===")
            .document
            .attributes
            .get("compat-mode"),
        Some("")
    );
    assert!(parse("Doc\n===\n:compat-mode!:\n\nBody")
        .document
        .attributes
        .get("compat-mode")
        .is_none());
    let mut attributes = Attributes::new();
    attributes.set("compat-mode", "locked");
    assert_eq!(
        parse_with_options(
            "Doc\n===",
            &ParseOptions {
                attributes,
                ..Default::default()
            }
        )
        .document
        .attributes
        .get("compat-mode"),
        Some("locked")
    );
}

#[test]
fn setext_preprocessing_preserves_unicode_normalization_and_inline_spans() {
    let source = "\u{feff}*Я🙂* \t\r\nifdef::missing[]\r\nignored\r\nendif::[]\r\n==== \t\r\n:who: World\r\n\r\n{who}";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let header = result.document.header.as_ref().unwrap();
    let fragments = &result.provenance[header.title_span.provenance.unwrap()];
    assert_eq!(fragments.first().unwrap().start, 3);
    assert_eq!(
        fragments.last().unwrap().end,
        source.find("====").unwrap() + 4
    );
    let BlockKind::Heading { inlines, .. } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(
        &source[inlines[0].span().start..inlines[0].span().end],
        "*Я🙂*"
    );
    let highlighted = adoctra_core::tokens(source);
    let headings: Vec<_> = highlighted
        .iter()
        .filter(|t| t.kind == TokenKind::Heading)
        .map(|t| &source[t.span.start..t.span.end])
        .collect();
    assert_eq!(headings, ["*Я🙂*", "===="]);
    assert_eq!(result.document.attributes.get("who"), Some("World"));
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let partial = &source[..end];
        let parsed = parse(partial);
        for span in adoctra_core::tokens(partial)
            .into_iter()
            .map(|t| t.span)
            .chain(parsed.diagnostics.iter().map(|d| d.span))
            .chain(parsed.provenance.iter().flatten().copied())
        {
            assert!(
                partial.get(span.start..span.end).is_some(),
                "{partial:?}: {span:?}"
            );
        }
    }
}

fn heading_inlines(doc: &adoctra_core::Document) -> &[adoctra_core::Inline] {
    let BlockKind::Heading { level: 0, inlines } = &doc.blocks[0].kind else {
        panic!("expected document title: {:?}", doc.blocks[0]);
    };
    inlines
}

#[test]
fn doctitle_overrides_restore_initial_value_and_preserve_declarations() {
    for (prefix, entries, expected) in [
        ("", "", "Source"),
        ("", ":doctitle: After\n", "After"),
        ("", ":doctitle:\n", "Source"),
        ("", ":doctitle!:\n", "Source"),
        ("", ":!doctitle:\n", "Source"),
        (":doctitle: Before\n", "", "Before"),
        (":doctitle: Before\n", ":doctitle: After\n", "After"),
        (
            ":doctitle: Before\n",
            ":doctitle: After\n:doctitle!:\n",
            "Before",
        ),
        (":doctitle:\n", "", "Source"),
    ] {
        for title in ["= Source", "Source\n======"] {
            let source = format!("{prefix}{title}\n{entries}\n{{doctitle}}");
            let result = parse(&source);
            assert!(result.diagnostics.is_empty(), "{source}");
            let doc = result.document;
            assert_eq!(doc.attributes.get("doctitle"), Some(expected), "{source}");
            assert_eq!(doc.header.as_ref().unwrap().title, "Source");
            let span = doc.header.as_ref().unwrap().title_span;
            assert_eq!(&source[span.start..span.end], title);
            assert_eq!(doc.blocks[0].span, span);
            assert!(
                matches!(&heading_inlines(&doc)[0], adoctra_core::Inline::Text { value, .. } if value == expected)
            );
        }
    }
}

#[test]
fn doctitle_hard_overrides_include_empty_values_and_no_source_title() {
    for host in ["Locked", ""] {
        let mut attributes = Attributes::new();
        attributes.set("doctitle", host);
        let options = ParseOptions {
            source_id: adoctra_core::SourceId(9),
            attributes,
            ..Default::default()
        };
        for source in [
            "= Source\n:doctitle: Changed\n\n{doctitle}",
            "= Source\n:doctitle!:\n\n{doctitle}",
        ] {
            let doc = parse_with_options(source, &options).document;
            let expected = if host.is_empty() { "Source" } else { host };
            assert_eq!(doc.attributes.get("doctitle"), Some(expected));
            assert!(
                matches!(&heading_inlines(&doc)[0], adoctra_core::Inline::Text { value, .. } if value == expected)
            );
            let span = heading_inlines(&doc)[0].span();
            assert_eq!(span.source_id, options.source_id);
            if !host.is_empty() {
                assert!(span.is_empty());
            }
        }
        let doc = parse_with_options("Body", &options).document;
        assert!(doc.header.is_none());
        assert_eq!(doc.blocks.len(), if host.is_empty() { 1 } else { 2 });
        if !host.is_empty() {
            assert!(doc.blocks[0].span.is_empty());
            assert!(heading_inlines(&doc)[0].span().is_empty());
        }
    }
    let source = ":doctitle: *Only*\n\nBody";
    let doc = parse(source).document;
    assert!(doc.header.is_none());
    assert_eq!(
        &source[doc.blocks[0].span.start..doc.blocks[0].span.end],
        ":doctitle: *Only*"
    );
    assert!(matches!(
        heading_inlines(&doc)[0],
        adoctra_core::Inline::Strong { .. }
    ));
    assert!(!adoctra_core::tokens(source)
        .iter()
        .any(|t| t.kind == TokenKind::Heading));
}

#[test]
fn doctitle_early_snapshot_and_body_changes_do_not_rewrite_the_heading() {
    // A title consisting of a marker must point at its text, not the ATX prefix.
    let doc = parse("= =").document;
    assert_eq!(
        heading_inlines(&doc)[0].span(),
        adoctra_core::Span::new(2, 3)
    );
    let source = ":word: Early\n= {word}\n:copy: {doctitle}\n:word: Late\n\nBefore {doctitle}\n\n:doctitle: Body\n\nAfter {doctitle}";
    let doc = parse(source).document;
    assert_eq!(doc.attributes.get("doctitle"), Some("Early"));
    assert_eq!(doc.attributes.get("copy"), Some("Early"));
    assert_eq!(doc.header.as_ref().unwrap().title, "{word}");
    assert!(
        matches!(&heading_inlines(&doc)[0], adoctra_core::Inline::AttributeRef { value: Some(value), .. } if value == "Late")
    );
    assert_eq!(
        doc.blocks[1].attribute_context.get("doctitle"),
        Some("Early")
    );
    assert_eq!(
        doc.blocks[2].attribute_context.get("doctitle"),
        Some("Body")
    );
    let doc = parse("= {later}\n:doctitle: {doctitle}\n:later: Known\n\n{doctitle}").document;
    assert_eq!(doc.attributes.get("doctitle"), Some("{later}"));
    assert!(
        matches!(&heading_inlines(&doc)[0], adoctra_core::Inline::AttributeRef { value: Some(value), .. } if value == "Known")
    );
}

#[test]
fn doctitle_replacement_ranges_cover_real_entries_and_nested_inlines() {
    use adoctra_core::visit::{walk_document_mut, VisitorMut};
    let source = "\u{feff}= Исходный \t\r\n:doctitle: *Новое🙂* \\\r\n  <<missing>>\r\n\r\nBody";
    let mut sources = SourceMap::new();
    sources.add("unused", "");
    let id = sources.add("doc.adoc", source);
    let options = ParseOptions {
        source_id: id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    let origin = heading_inlines(&result.document)[0].span();
    assert_eq!(
        sources.slice(origin),
        Some(":doctitle: *Новое🙂* \\\r\n  <<missing>>")
    );
    assert_eq!(result.document.header.as_ref().unwrap().title, "Исходный");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].span, origin);
    let adoctra_core::Inline::Strong { children, .. } = &heading_inlines(&result.document)[0]
    else {
        panic!()
    };
    assert_eq!(children[0].span(), origin);
    let tokens = tokens_with_options(source, &options);
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Heading)
            .count(),
        1
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::AttributeEntry)
            .count(),
        2
    );
    struct ValidRanges<'a>(&'a str);
    impl VisitorMut for ValidRanges<'_> {
        fn visit_span(&mut self, span: &mut adoctra_core::Span) {
            assert!(self.0.get(span.start..span.end).is_some(), "{span:?}");
        }
    }
    for end in (0..=source.len()).filter(|&i| source.is_char_boundary(i)) {
        let partial = &source[..end];
        let mut result = parse(partial);
        walk_document_mut(&mut ValidRanges(partial), &mut result.document);
        for token in adoctra_core::tokens(partial) {
            assert!(partial.get(token.span.start..token.span.end).is_some());
        }
    }
}

#[test]
fn indexed_authors_reconcile_names_but_retain_source_declarations() {
    let source = "\u{feff}= Документ\r\nИван Петров <first>; Mary_Jane Q Public <second>\r\n:early: {authors}\r\n:author_2: Анна Смирнова\r\n\r\n{authors}\r\n\r\n:author_2: Later\r\n\r\n{author_2}";
    let mut sources = SourceMap::new();
    sources.add("unused", "");
    let id = sources.add("authors.adoc", source);
    let options = ParseOptions {
        source_id: id,
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty());
    let doc = &result.document;
    for (key, value) in [
        ("authors", "Иван Петров, Анна Смирнова"),
        ("early", "Иван Петров, Mary Jane Q Public"),
        ("authorcount", "2"),
        ("firstname_2", "Анна"),
        ("lastname_2", "Смирнова"),
        ("authorinitials_2", "АС"),
        ("middlename_2", "Q"),
        ("email_2", "second"),
    ] {
        assert_eq!(doc.attributes.get(key), Some(value), "{key}");
    }
    let header = doc.header.as_ref().unwrap();
    assert_eq!(header.authors[1].name, "Mary Jane Q Public");
    assert_eq!(
        sources.slice(header.authors[1].span),
        Some("Mary_Jane Q Public <second>")
    );
    assert_eq!(
        doc.blocks[1].attribute_context.get("author_2"),
        Some("Анна Смирнова")
    );
    assert_eq!(
        doc.blocks[2].attribute_context.get("author_2"),
        Some("Later")
    );
    assert_eq!(
        doc.blocks[2].attribute_context.get("firstname_2"),
        Some("Анна")
    );
    let BlockKind::Paragraph { inlines } = &doc.blocks[1].kind else {
        panic!()
    };
    assert_eq!(sources.slice(inlines[0].span()), Some("{authors}"));
    assert!(
        matches!(&inlines[0], adoctra_core::Inline::AttributeRef { value: Some(value), .. } if value == "Иван Петров, Анна Смирнова")
    );
    let tokens = tokens_with_options(source, &options);
    let author = tokens.iter().find(|t| t.kind == TokenKind::Author).unwrap();
    assert_eq!(
        sources.slice(author.span),
        Some("Иван Петров <first>; Mary_Jane Q Public <second>")
    );
}

#[test]
fn indexed_authors_preserve_compound_components_and_require_contiguous_keys() {
    let doc = parse("= Doc\nMary_Jane Q Public; John Doe\n:author_2: Alice Brown\n\nBody").document;
    assert_eq!(doc.attributes.get("firstname"), Some("Mary Jane"));
    assert_eq!(doc.attributes.get("middlename"), Some("Q"));
    assert_eq!(doc.attributes.get("lastname"), Some("Public"));
    assert_eq!(doc.attributes.get("authorinitials_1"), Some("MQP"));
    for source in [
        ":author_1: Alice Brown\n\nBody",
        "= Doc\n:author_2: Alice Brown\n\nBody",
    ] {
        let doc = parse(source).document;
        assert_eq!(doc.attributes.get("authorcount"), Some("0"));
        assert!(doc.attributes.get("firstname").is_none());
    }
    let doc =
        parse("= Doc\nJohn Doe; Mary Roe\n:author_1!:\n:author_2: Alice Brown\n\nBody").document;
    assert_eq!(doc.attributes.get("authors"), Some("John Doe, Mary Roe"));
    assert_eq!(doc.attributes.get("firstname_2"), Some("Mary"));
    let doc = parse("= Doc\n:author_1: Alice Brown\n:author_3: Bob White\n\nBody").document;
    assert_eq!(doc.attributes.get("authors"), Some("Alice Brown"));
    assert_eq!(doc.attributes.get("authorcount"), Some("1"));
    assert!(doc.attributes.get("firstname_3").is_none());
}

#[test]
fn indexed_empty_overrides_compact_names_without_deleting_old_fields() {
    for (entries, expected, count) in [
        (":author_1:", "Mary Roe", "1"),
        (":author_2:", "John Doe", "1"),
        (":author_1:\n:author_2:", "John Doe, Mary Roe", "2"),
    ] {
        let doc = parse(&format!("= Doc\nJohn Doe; Mary Roe\n{entries}\n\nBody")).document;
        assert_eq!(doc.attributes.get("authors"), Some(expected));
        assert_eq!(doc.attributes.get("authorcount"), Some(count));
        assert_eq!(doc.attributes.get("lastname_2"), Some("Roe"));
    }
    let doc = parse("= Doc\n:author_1: Alice Brown\n:email_1: contact\n\nBody").document;
    assert_eq!(doc.attributes.get("email"), Some("contact"));
    let doc = parse(":author: Alice Brown\n:email_1: contact\n\nBody").document;
    assert!(doc.attributes.get("email").is_none());
}

#[test]
fn derived_author_metadata_replaces_host_fields_without_unlocking_body_entries() {
    let mut attributes = Attributes::new();
    for (key, value) in [
        ("author_2", "New_Name"),
        ("firstname_2", "LOCKED"),
        ("authorcount", "99"),
    ] {
        attributes.set(key, value);
    }
    let doc = parse_with_options(
        "= Doc\nJohn Doe; Mary Roe\n:author_2: Ignored\n\nFirst\n\n:firstname_2: Ignored\n\nSecond",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.get("author_2"), Some("New Name"));
    assert_eq!(doc.attributes.get("firstname_2"), Some("New Name"));
    assert_eq!(doc.attributes.get("authorcount"), Some("2"));
    assert_eq!(
        doc.blocks
            .last()
            .unwrap()
            .attribute_context
            .get("firstname_2"),
        Some("New Name")
    );
}

#[test]
fn indexed_overrides_follow_explicit_name_priority_and_preserve_partial_input() {
    for (entry, names) in [
        (":author: Single Name", "Single Name"),
        (":authors: One Name; Two Names", "One Name, Two Names"),
    ] {
        let doc = parse(&format!(
            "= Doc\nJohn Doe; Mary Roe\n{entry}\n:author_2: Other Name\n\nBody"
        ))
        .document;
        assert_eq!(doc.attributes.get("authors"), Some(names));
    }
    let source =
        "= Я\r\nИван Петров; Mary_Jane Q Public\r\n:author_2: Анна Смирнова\r\n\r\n{authors}";
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let partial = &source[..end];
        let parsed = parse(partial);
        for span in adoctra_core::tokens(partial)
            .into_iter()
            .map(|t| t.span)
            .chain(
                parsed
                    .document
                    .header
                    .iter()
                    .flat_map(|h| h.authors.iter().map(|a| a.span)),
            )
            .chain(parsed.diagnostics.iter().map(|d| d.span))
        {
            assert!(
                partial.get(span.start..span.end).is_some(),
                "{partial:?}: {span:?}"
            );
        }
    }
}
