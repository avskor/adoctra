use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributeFormat as F, AttributeLink,
    AttributePart as P, Attributes, BlockKind, Inline, ParseOptions, SourceMap, TokenKind,
};

fn parts(source: &str) -> Vec<P> {
    parse(source)
        .document
        .attributes
        .raw_parts("v")
        .unwrap_or_default()
        .to_vec()
}
fn link(target: &str) -> P {
    P::OpenLink(AttributeLink {
        target: target.into(),
        ..Default::default()
    })
}

#[test]
fn link_macros_have_neutral_boundaries_and_flat_labels_with_physical_reference_spans() {
    for (body, target, value) in [
        ("link:page.adoc[Жёлтый]", "page.adoc", "Жёлтый"),
        (
            "https://example.org[Жёлтый]",
            "https://example.org",
            "Жёлтый",
        ),
        ("mailto:a@example.org[Mail]", "mailto:a@example.org", "Mail"),
        (
            "mailto:a@example.org[]",
            "mailto:a@example.org",
            "a@example.org",
        ),
        ("a@example.org", "mailto:a@example.org", "a@example.org"),
        ("link:[Empty]", "", "Empty"),
    ] {
        let source = format!(":v: pass:m[{body}]\n\n{{v}}");
        let result = parse(&source);
        assert!(result.diagnostics.is_empty());
        let expected = vec![link(target), P::Raw(value.into()), P::CloseLink];
        assert_eq!(result.document.attributes.get("v"), Some(value));
        assert_eq!(result.document.attributes.raw_parts("v").unwrap(), expected);
        let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
            panic!()
        };
        assert!(
            matches!(&inlines[0], Inline::AttributeRef { parts, value: Some(v), span, .. }
            if parts == &expected && v == value && &source[span.start..span.end] == "{v}")
        );
    }
}

#[test]
fn bare_url_definitions_keep_targets_schemes_and_punctuation_rules() {
    for target in [
        "https://example.org",
        "http://example.org",
        "ftp://h/a",
        "file:///a",
        "irc://h",
    ] {
        let source = format!(":v: pass:m[{target}]");
        let P::OpenLink(value) = &parts(&source)[0] else {
            panic!()
        };
        assert_eq!(value.target, target);
        assert_eq!(value.role.as_deref(), Some("bare"));
    }
    assert_eq!(
        parse(":hide-uri-scheme:\n:v: pass:m[https://example.org]")
            .document
            .attributes
            .get("v"),
        Some("example.org")
    );
    assert_eq!(
        parts(":v: pass:m[&lt;https://example.org]")[0],
        P::Raw("&lt;https://example.org".into())
    );
    assert_eq!(
        parts(":v: pass:m[&lt;https://&gt;]")[0],
        P::Raw("&lt;https://&gt;".into())
    );
    let p = parts(":v: pass:m[https://h/a);]");
    assert!(matches!(&p[0], P::OpenLink(l) if l.target == "https://h/a"));
    assert_eq!(p.last(), Some(&P::Raw(");".into())));
}

#[test]
fn escaping_removes_one_layer_at_the_selected_macro_stage() {
    for suffix in ["-", ".toolong"] {
        assert_eq!(
            parts(&format!(":v: pass:m[a@b.com{suffix}]")),
            [
                link("mailto:a@b.com"),
                P::Raw("a@b.com".into()),
                P::CloseLink,
                P::Raw(suffix.into())
            ]
        );
    }
    for body in [
        r"\link:page[Text]",
        r"\mailto:a@example.org[Mail]",
        r"\https://example.org",
        r"\a@example.org",
        r"\ftp://;",
    ] {
        assert_eq!(
            parts(&format!(":v: pass:m,macros[{body}]")),
            [P::Raw(body[1..].into())]
        );
    }
    assert_eq!(
        parts(r":v: pass:m[link:page[A\]B]]"),
        [link("page"), P::Raw("A]B".into()), P::CloseLink]
    );
    for body in [
        "xhttps://example.org",
        "link:https://example.org",
        "\"https://example.org\"",
        "a@example.museum",
    ] {
        assert_eq!(parts(&format!(":v: pass:m[{body}]")), [P::Raw(body.into())]);
    }
}

#[test]
fn named_properties_mail_subjects_and_windows_preserve_reference_semantics() {
    let p = parts(
        ":v: pass:m[link:page[Text,id=anchor,role=red,title=Title,window=_blank,opts=nofollow]]",
    );
    assert_eq!(
        p[0],
        P::OpenLink(AttributeLink {
            target: "page".into(),
            id: Some("anchor".into()),
            role: Some("red".into()),
            title: Some("Title".into()),
            window: Some("_blank".into()),
            rel: Some("nofollow noopener".into())
        })
    );
    let p = parts(":v: pass:m[mailto:a@example.org[Mail,Hello world,Ж&]]");
    assert_eq!(
        p[0],
        link("mailto:a@example.org?subject=Hello%20world&amp;body=%D0%96%26")
    );
    let p = parts(":v: pass:m[link:page[role=red,Text]]");
    assert_eq!(p[1], P::Raw("page".into())); // named fields still occupy a positional slot
    assert!(matches!(&p[0],P::OpenLink(l) if l.role.as_deref()==Some("bare red")));
    assert!(
        matches!(&parts(":v: pass:m[link:page[Text^]]")[0],P::OpenLink(l)
        if l.window.as_deref()==Some("_blank") && l.rel.as_deref()==Some("noopener"))
    );
    assert!(
        matches!(&parts(":v: pass:m[link:page[Text,window=frame,opts=nofollow]]")[0],P::OpenLink(l)
        if l.window.as_deref()==Some("frame") && l.rel.is_none())
    );
}

#[test]
fn ordered_quotes_and_protection_preserve_link_and_label_structure() {
    let expected = [
        link("page"),
        P::Open(F::Strong),
        P::Raw("Bold".into()),
        P::Close(F::Strong),
        P::CloseLink,
    ];
    for stages in ["q,m", "m,q", "q,m,quotes,macros"] {
        assert_eq!(
            parts(&format!(":v: pass:{stages}[link:page[*Bold*]]")),
            expected
        );
    }
    let doc = parse(":v: pass:m,c[link:page[Text]]").document;
    assert_eq!(doc.attributes.get("v"), Some("<a href=\"page\">Text</a>"));
    assert!(doc.attributes.raw_parts("v").is_none());
    let p = parts(":v: pass:c,m[link:page?a=1&b=2[A&B]]");
    assert_eq!(
        p,
        [
            link("page?a=1&amp;b=2"),
            P::Text("A&B".into()),
            P::CloseLink
        ]
    );
    let p = parts(":v: pass:q,m[link:page['*Bold*',title='T']]");
    assert!(matches!(&p[1],P::Text(t) if t=="<strong>Bold</strong>"));
}

#[test]
fn attribute_expansion_crosses_parts_and_updates_semantic_link_properties() {
    let source=":head: pass:[link:page[]\n:tail: pass:[]]\n:label: pass:q[*Bold*]\n:v: pass:a,m[{head}{label}{tail}]";
    assert_eq!(
        parts(source),
        [
            link("page"),
            P::Open(F::Strong),
            P::Raw("Bold".into()),
            P::Close(F::Strong),
            P::CloseLink
        ]
    );
    let p = parts(":target: page\n:label: Text\n:v: pass:m,a[link:{target}[{label}]]");
    assert_eq!(p, [link("page"), P::Text("Text".into()), P::CloseLink]);
    let p = parts(":v: pass:m,r[link:page(C)[Text,title=Title(C),role=role(C)]]");
    assert!(
        matches!(&p[0],P::OpenLink(l) if l.target=="page&#169;" && l.title.as_deref()==Some("Title&#169;") && l.role.as_deref()==Some("role&#169;"))
    );
    let p = parts(r":v: pass:m,r[link:page\(C)[Text,title=Title\(C)]]");
    assert!(
        matches!(&p[0],P::OpenLink(l) if l.target=="page(C)" && l.title.as_deref()==Some("Title(C)"))
    );
    // Macro conversion does not reinterpret arbitrary raw HTML as semantic links.
    assert_eq!(
        parts(":v: pass:m[<a href=\"page\">Text</a>]"),
        [P::Raw("<a href=\"page\">Text</a>".into())]
    );
}

#[test]
fn links_survive_authors_snapshots_section_ids_and_resolved_reference_labels() {
    let doc=parse("= Doc\nJohn Doe; Mary Roe\n:author_2: pass:m[link:author_id[Ada_Lovelace,role=author_role]]\n\n{authors}").document;
    assert_eq!(doc.attributes.get("firstname_2"), Some("Ada Lovelace"));
    assert!(
        matches!(&doc.attributes.raw_parts("author_2").unwrap()[0],P::OpenLink(l) if l.target=="author id" && l.role.as_deref()==Some("author role"))
    );
    assert_eq!(doc.header.unwrap().authors[1].name, "Mary Roe");
    let result=parse(":v: pass:m[link:page[Alpha]]\n\n<<_alpha>>\n\n== {v}\n\n:v: pass:m[link:other[Beta]]\n\n{v}");
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.document.blocks[1].id.as_deref(), Some("_alpha"));
    assert_eq!(result.document.attributes.get("v"), Some("Alpha"));
    assert_eq!(
        result.document.blocks[2]
            .attribute_context
            .raw_parts("v")
            .unwrap()[0],
        link("other")
    );
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!()
    };
    let Inline::CrossReference { children, .. } = &inlines[0] else {
        panic!()
    };
    assert!(
        matches!(&children[0],Inline::AttributeRef {parts,..} if parts==&[P::Raw("Alpha".into())])
    );
}

#[test]
fn host_link_parts_and_cached_compat_mode_are_respected() {
    let host = vec![link("host"), P::Text("Host".into()), P::CloseLink];
    let mut attributes = Attributes::new();
    attributes.set_parts("v", host.clone());
    let doc = parse_with_options(
        ":v: ignored\n\n{v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    assert_eq!(doc.attributes.raw_parts("v").unwrap(), host);
    let doc=parse(":compat-mode:\n:v: pass:m[link:page[Text,role=red]]\n\n{v}\n\n:v: pass:m[link:page[Text,role=red]]\n\n{v}").document;
    assert!(
        matches!(&doc.attributes.raw_parts("v").unwrap()[0],P::OpenLink(l) if l.role.as_deref()==Some("red"))
    );
    assert_eq!(
        doc.blocks[1].attribute_context.get("v"),
        Some("Text,role=red")
    );
    assert!(
        matches!(&doc.blocks[1].attribute_context.raw_parts("v").unwrap()[0],P::OpenLink(l) if l.role.is_none())
    );
}

#[test]
fn links_keep_unicode_crlf_reference_positions_and_partial_input_is_safe() {
    let source = "\u{feff}:v: pass:c,q,m[link:страница[*Ж&*,role=роль]]\r\n\r\nДо {v}";
    let mut sources = SourceMap::new();
    let source_id = sources.add("macros.adoc", source);
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let doc = parse_with_options(source, &options).document;
    let BlockKind::Paragraph { inlines } = &doc.blocks[0].kind else {
        panic!()
    };
    assert_eq!(sources.slice(inlines[1].span()), Some("{v}"));
    assert!(tokens_with_options(source, &options)
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && t.span == inlines[1].span()));
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let partial = &source[..end];
        let _ = parse_with_options(partial, &options);
        for token in tokens_with_options(partial, &options) {
            assert_eq!(token.span.source_id, source_id);
            assert!(token.span.end <= end);
            assert!(partial.is_char_boundary(token.span.start));
            assert!(partial.is_char_boundary(token.span.end));
        }
    }
}

#[test]
fn incomplete_macro_runs_and_long_email_candidates_do_not_rescan_each_suffix() {
    for input in [
        "link:x[ ".repeat(10_000),
        "a&amp;".repeat(10_000),
        "mailto:a[ ".repeat(10_000),
    ] {
        let doc = parse(&format!(":v: pass:m[{input}]\n\n{{v}}")).document;
        assert_eq!(doc.attributes.get("v"), Some(input.as_str()));
    }
}

#[test]
fn recursive_single_quoted_attribute_values_have_a_bounded_macro_depth() {
    let mut attributes = Attributes::new();
    attributes.set("cycle", "link:page['{cycle}',title=T]");
    let doc = parse_with_options(
        ":v: pass:m[link:page['{cycle}',title=T]]\n\n{v}",
        &ParseOptions {
            attributes,
            ..Default::default()
        },
    )
    .document;
    let parts = doc.attributes.raw_parts("v").unwrap();
    let links = parts.iter().filter(|p| matches!(p, P::OpenLink(_))).count();
    assert_eq!(links, 33);
    assert!(doc.attributes.get("v").unwrap().contains("{cycle}"));
}
