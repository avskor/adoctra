//! Block `subs` attributes, like Ruby's `commit_subs`, `resolve_block_subs`,
//! `catalog_callouts` and `sub_callouts`.
use adoctra_core::{
    parse, parse_with_options, tokens, tokens_with_options, AttributeFormat, AttributePart as P,
    Block, BlockKind, Inline, InlineCallout, ParseOptions, Severity, SourceId, Span, TokenKind,
};

fn substituted(nodes: &[Inline]) -> (&str, &[P], Span) {
    match nodes {
        [Inline::Substituted { value, parts, span }] => (value, parts, *span),
        _ => panic!("{nodes:?}"),
    }
}

fn paragraph(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } => inlines,
        _ => panic!("{block:?}"),
    }
}

/// The converted content of a listing, literal or passthrough block.
fn content(block: &Block) -> Option<&[Inline]> {
    match &block.kind {
        BlockKind::Listing { inlines, .. }
        | BlockKind::Literal { inlines, .. }
        | BlockKind::Passthrough { inlines, .. } => inlines.as_deref(),
        _ => panic!("{block:?}"),
    }
}

fn callouts(parts: &[P]) -> Vec<&InlineCallout> {
    parts
        .iter()
        .filter_map(|p| match p {
            P::Callout(callout) => Some(&**callout),
            _ => None,
        })
        .collect()
}

#[test]
fn paragraphs_select_stages_relative_to_normal_defaults() {
    let source = ":v: 1.0 <b>\n\n[subs=\"quotes\"]\n*Ж* {v} <i>\n\n[subs=\"-quotes\"]\n*b* {v}\n\n[subs=none]\n*n*";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    let (value, parts, span) = substituted(paragraph(&blocks[0]));
    assert_eq!(value, "Ж {v} <i>");
    assert_eq!(
        parts,
        [
            P::Open(AttributeFormat::Strong),
            P::Raw("Ж".into()),
            P::Close(AttributeFormat::Strong),
            P::Raw(" {v} <i>".into()),
        ]
    );
    assert_eq!(&source[span.start..span.end], "*Ж* {v} <i>");
    // Removing quotes keeps the other normal stages.
    let (value, parts, _) = substituted(paragraph(&blocks[1]));
    assert_eq!(value, "*b* 1.0 <b>");
    assert!(!parts.iter().any(|p| matches!(p, P::Open(_))));
    assert_eq!(substituted(paragraph(&blocks[2])).1, [P::Raw("*n*".into())]);
}

#[test]
fn defaults_and_empty_selections() {
    // A selection equal to the defaults keeps the ordinary conversion.
    let result = parse("[subs=normal]\n*a*\n\n[subs=verbatim]\n----\nx <1>\n----\n<1> c");
    assert!(matches!(
        paragraph(&result.document.blocks[0]),
        [Inline::Strong { .. }]
    ));
    let BlockKind::Listing {
        inlines, callouts, ..
    } = &result.document.blocks[1].kind
    else {
        panic!()
    };
    assert!(inlines.is_none());
    assert_eq!(callouts.len(), 1);
    // An empty value applies no substitutions at all.
    let result = parse("[subs=\"\"]\n*a* {b}\n\n[subs=\",\"]\n----\n<c>\n----");
    assert_eq!(
        substituted(paragraph(&result.document.blocks[0])).1,
        [P::Raw("*a* {b}".into())]
    );
    assert_eq!(
        substituted(content(&result.document.blocks[1]).unwrap()).1,
        [P::Raw("<c>".into())]
    );
}

#[test]
fn modifiers_groups_and_invalid_names_follow_ruby() {
    let result = parse(":v: x\n\n[subs=\"specialchars+, quotes+,bogus\"]\n----\n*s* {v}\n----\n\n[subs=\"bogus,bogus,quotes\"]\nz");
    assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
    assert_eq!(result.diagnostics[0].severity, Severity::Warning);
    assert_eq!(
        result.diagnostics[0].message,
        "invalid substitution type for listing: bogus"
    );
    assert_eq!(result.diagnostics[0].span, result.document.blocks[0].span);
    assert_eq!(
        result.diagnostics[1].message,
        "invalid substitution types for paragraph: bogus, bogus"
    );
    // Quotes prepended before special characters: the generated tags become
    // text, and attributes stay unselected.
    let (value, parts, _) = substituted(content(&result.document.blocks[0]).unwrap());
    assert_eq!(value, "<strong>s</strong> {v}");
    assert!(!parts.iter().any(|p| matches!(p, P::Open(_))));
}

#[test]
fn verbatim_blocks_convert_with_callouts_and_trim_edges_after_expansion() {
    let source = ":v: 1.0 <b>\n:empty:\n\n[source,java,subs=\"attributes+\"]\n----\n{empty}\nx = \"{v}\"; // <1>\n*y* <.>\n{empty}\n----\n<1> one\n<2> two";
    let result = parse(source);
    // Like Ruby, the second item explains no registered mark.
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].message, "no callout found for <2>");
    let block = &result.document.blocks[0];
    let BlockKind::Listing {
        text,
        callouts: catalog,
        inlines: Some(nodes),
        ..
    } = &block.kind
    else {
        panic!("{block:?}")
    };
    // The source text stays; the catalog registered the source marks.
    assert!(text.contains("{v}"));
    assert_eq!(catalog.len(), 0);
    let (value, parts, span) = substituted(nodes);
    assert_eq!(value, "x = \"1.0 &lt;b&gt;\"; \n*y* ");
    assert_eq!(
        &source[span.start..span.end],
        "{empty}\nx = \"{v}\"; // <1>\n*y* <.>\n{empty}"
    );
    let marks = callouts(parts);
    assert_eq!(marks.len(), 2);
    assert_eq!(marks[0].number, "1");
    assert_eq!(marks[0].guard.as_deref(), Some("// "));
    assert_eq!(marks[0].id.as_deref(), Some("CO1-1"));
    assert_eq!(marks[1].number, "1");
    assert_eq!(marks[1].id.as_deref(), Some("CO1-2"));
    assert!(marks.iter().all(|m| !m.xml && m.icons.is_none()));
    // Blank edge lines made by `{empty}` are trimmed after substitution.
    assert!(!value.starts_with('\n') && !value.ends_with('\n'));
    let BlockKind::CalloutList { items } = &result.document.blocks[1].kind else {
        panic!()
    };
    assert_eq!(items[0].coids, ["CO1-1", "CO1-2"]);
    assert!(items[1].coids.is_empty());
}

#[test]
fn callouts_need_protected_source_marks() {
    // Without source marks the stage is dropped: a generated mark stays text.
    let result = parse(":co: pass:[<2>]\n\n[subs=\"attributes+\"]\n----\ny {co}\n----");
    let (_, parts, _) = substituted(content(&result.document.blocks[0]).unwrap());
    assert!(callouts(parts).is_empty());
    // With one, a generated mark protected by special characters converts.
    let result = parse(":co: pass:[<2>]\n\n[subs=\"attributes+\"]\n----\nx <1>\ny {co}\n----");
    let (_, parts, _) = substituted(content(&result.document.blocks[0]).unwrap());
    let marks = callouts(parts);
    assert_eq!(marks.len(), 2);
    assert_eq!(marks[1].number, "2");
    assert_eq!(marks[1].id, None);
    // Callouts before special characters see raw marks and match nothing.
    let result = parse("[subs=\"callouts\"]\n----\nx <1>\n----");
    let (_, parts, _) = substituted(content(&result.document.blocks[0]).unwrap());
    assert_eq!(parts, [P::Raw("x <1>".into())]);
    // Escaped marks lose their backslash; xml marks keep their form.
    let result = parse("[subs=\"+quotes\"]\n----\nb \\<2>\nc <!--3-->\n----");
    let (value, parts, _) = substituted(content(&result.document.blocks[0]).unwrap());
    assert_eq!(value, "b <2>\nc ");
    assert!(callouts(parts)[0].xml);
}

#[test]
fn converted_callouts_read_the_ids_every_mark_registered() {
    // Ruby's `catalog_callouts` registers `<!--2-->` too; the marks the
    // empty `line-comment` converts read the ids in order (`read_next_id`),
    // so the next block's mark reads the id `<!--2-->` registered.
    let result = parse(
        "[subs=\"+quotes\",line-comment=]\n----\n*e* <1> \\<2>\nf <!--2-->\n----\n\n[subs=\"+quotes\"]\n----\ng <3>\n----\n<1> One\n<2> Two\n<3> Three",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    let (value, parts, _) = substituted(content(&blocks[0]).unwrap());
    assert_eq!(value, "e  <2>\nf <!--2-->");
    let marks = callouts(parts);
    assert_eq!(marks.len(), 1);
    assert_eq!(
        (marks[0].id.as_deref(), marks[0].guard.as_deref()),
        (Some("CO1-1"), Some(""))
    );
    let (_, parts, _) = substituted(content(&blocks[1]).unwrap());
    let marks = callouts(parts);
    assert_eq!(
        (marks[0].id.as_deref(), marks[0].guard.as_deref()),
        (Some("CO1-2"), None)
    );
}

#[test]
fn callouts_capture_line_comments_and_icons() {
    let result =
        parse(":icons: font\n\n[subs=\"+quotes\",line-comment=%]\n----\nx % <1>\ny <!--2-->\n----");
    let (_, parts, _) = substituted(content(&result.document.blocks[0]).unwrap());
    let marks = callouts(parts);
    assert_eq!(marks.len(), 1, "a custom line comment only allows <N>");
    assert_eq!(marks[0].guard.as_deref(), Some("% "));
    assert_eq!(marks[0].icons.as_deref(), Some("font"));
    let result =
        parse(":icons:\n:iconsdir: img/i\n:icontype: svg\n\n[subs=\"+quotes\"]\n----\nx <1>\n----");
    let (_, parts, _) = substituted(content(&result.document.blocks[0]).unwrap());
    assert_eq!(callouts(parts)[0].src, "img/i/callouts/1.svg");
}

#[test]
fn literal_and_passthrough_forms_use_their_own_sources() {
    let source = ":v: x\n\n  indented *a* {v}\n    deeper\n\n[literal,subs=attributes]\nlit {v}\n\n[pass,subs=attributes]\n<u>{v}</u>\n// hidden\n\n[pass,subs=\"attributes,quotes\"]\n--\n<u>{v}</u> *q*\n--\n\n[subs=+quotes]\n  indented *a* {v}\n    deeper";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert!(content(&blocks[0]).is_none());
    let (value, ..) = substituted(content(&blocks[1]).unwrap());
    assert_eq!(value, "lit x");
    // A passthrough paragraph skips line comments, like Ruby.
    let (value, parts, _) = substituted(content(&blocks[2]).unwrap());
    assert_eq!(value, "<u>x</u>");
    assert_eq!(parts[0], P::Raw("<u>".into()));
    let (value, parts, span) = substituted(content(&blocks[3]).unwrap());
    assert_eq!(value, "<u>x</u> q");
    assert!(parts.contains(&P::Open(AttributeFormat::Strong)));
    assert_eq!(&source[span.start..span.end], "<u>{v}</u> *q*");
    // An indented literal paragraph converts without its indentation.
    let (value, _, span) = substituted(content(&blocks[4]).unwrap());
    assert_eq!(value, "indented a {v}\n  deeper");
    assert_eq!(
        &source[span.start..span.end],
        "indented *a* {v}\n    deeper"
    );
}

#[test]
fn styled_paragraphs_apply_their_subs_but_delimited_containers_do_not() {
    let result = parse("[NOTE,subs=none]\n*a*\n\n[subs=none]\nTIP: *t*\n\n[example,subs=none]\n*e*\n\n[quote,Me,subs=none]\n*q*\n\n[sidebar,subs=none]\n*s*\n\n[subs=none]\n====\n*inner*\n====\n\n[verse,subs=none]\n____\n*v*\n____");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    for block in &result.document.blocks[..5] {
        let (BlockKind::Admonition { blocks, .. }
        | BlockKind::Example { blocks }
        | BlockKind::Quote { blocks, .. }
        | BlockKind::Sidebar { blocks }) = &block.kind
        else {
            panic!("{block:?}")
        };
        assert!(substituted(paragraph(&blocks[0])).0.starts_with('*'));
    }
    let BlockKind::Example { blocks } = &result.document.blocks[5].kind else {
        panic!()
    };
    assert!(matches!(paragraph(&blocks[0]), [Inline::Strong { .. }]));
    let BlockKind::Verse { inlines, .. } = &result.document.blocks[6].kind else {
        panic!()
    };
    assert_eq!(substituted(inlines).1, [P::Raw("*v*".into())]);
}

#[test]
fn conversion_state_macros_and_footnotes_apply() {
    let result = parse("[subs=\"attributes+\"]\n----\nbody {counter:n} {n}\n----\n\n{n}\n\n[subs=+macros]\n----\nsee <<sec>> footnote:[fn] pass:[<b>]\n----\n\n[[sec]]\n== Sec");
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = &result.document.blocks;
    assert_eq!(substituted(content(&blocks[0]).unwrap()).0, "body 1 1");
    assert_eq!(result.document.final_attributes.get("n"), Some("1"));
    let (_, parts, _) = substituted(content(&blocks[2]).unwrap());
    assert!(parts.iter().any(|p| matches!(p, P::OpenXref(_))));
    assert!(parts.iter().any(|p| matches!(p, P::Footnote(_))));
    assert!(parts.contains(&P::Raw("<b>".into())));
    assert!(parts.iter().any(|p| matches!(p, P::Text(t) if t == "Sec")));
    assert_eq!(result.document.footnotes.len(), 1);
}

#[test]
fn nested_contexts_crlf_and_unicode_keep_physical_spans() {
    let source = ":v: Ж\r\n\r\n* item\r\n+\r\n[subs=+quotes]\r\n....\r\n*ли* <1>\r\n....\r\n\r\n|===\r\na|\r\n[subs=none]\r\n*яч* {v}\r\n|===\r\n";
    let options = ParseOptions {
        source_id: SourceId(3),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let BlockKind::List { items, .. } = &result.document.blocks[0].kind else {
        panic!()
    };
    let (value, parts, span) = substituted(content(&items[0].blocks[0]).unwrap());
    assert_eq!(value, "ли ");
    assert_eq!(callouts(parts).len(), 1);
    let at = source.find("*ли*").unwrap();
    assert_eq!(
        span,
        Span::in_source(SourceId(3), at, at + "*ли* <1>".len())
    );
    let BlockKind::Table(table) = &result.document.blocks[1].kind else {
        panic!()
    };
    let adoctra_core::ast::CellContent::Blocks { blocks, .. } = &table.body[0][0].content else {
        panic!()
    };
    let (value, _, span) = substituted(paragraph(&blocks[0]));
    assert_eq!(value, "*яч* {v}");
    assert_eq!(&source[span.start..span.end], "*яч* {v}");
}

#[test]
fn highlighting_keeps_tokens_of_executed_stages() {
    let source = ":v: x\n\n[subs=none]\n*a* {v} pass:[p]\n\n[subs=quotes]\n*q* {v}\n\n[literal]\n*l* {v}\n\n[subs=+attributes]\n----\n{v} <1>\n----\n\n[subs=\"callouts\"]\n----\nraw <1>\n----";
    let ts = tokens(source);
    let at = |text: &str| source.find(text).unwrap();
    let kinds_in = |start: usize, end: usize| -> Vec<TokenKind> {
        ts.iter()
            .filter(|t| start <= t.span.start && t.span.end <= end)
            .map(|t| t.kind)
            .collect()
    };
    let first = at("*a*");
    assert!(kinds_in(first, first + 16).is_empty());
    let second = at("*q*");
    assert_eq!(kinds_in(second, second + 7), [TokenKind::Strong]);
    let literal = at("*l*");
    assert_eq!(kinds_in(literal, literal + 7), [TokenKind::LiteralText]);
    let mark = at("<1>");
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::Callout && t.span == Span::new(mark, mark + 3)));
    // Callouts before special characters convert nothing.
    let raw = at("raw <1>") + 4;
    assert!(!ts
        .iter()
        .any(|t| t.kind == TokenKind::Callout && t.span.start == raw));
    let ts = tokens_with_options(
        source,
        &ParseOptions {
            source_id: SourceId(2),
            ..Default::default()
        },
    );
    assert!(ts.iter().all(|t| t.span.source_id == SourceId(2)));
}

#[test]
fn included_listing_bodies_keep_provenance() {
    use adoctra_core::{parse_resource, tokens_resource, MemoryResources, SourceMap};
    let mut resources = MemoryResources::new();
    resources
        .insert(
            "/main.adoc",
            ":x: 1\n\n[subs=\"attributes+\"]\n----\ninclude::code.txt[]\n----\n<1> one",
        )
        .unwrap();
    resources.insert("/code.txt", "{x} *y* <1>\r\nЖ").unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let (value, parts, span) = substituted(content(&result.document.blocks[0]).unwrap());
    assert_eq!(value, "1 *y* \nЖ");
    assert_eq!(callouts(parts)[0].id.as_deref(), Some("CO1-1"));
    assert_eq!(span.source_id, SourceId(1));
    assert_eq!(sources.slice(span), Some("{x} *y* <1>\r\nЖ"));
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    let mark = ts.iter().find(|t| t.kind == TokenKind::Callout).unwrap();
    assert_eq!(mark.span.source_id, SourceId(1));
    assert_eq!(sources.slice(mark.span), Some("<1>"));
}

#[test]
fn incomplete_editor_input_never_fails() {
    for source in [
        "[subs=",
        "[subs=+quotes]\n----\n*x* <1>",
        "[subs=\"+quotes\"]\n....",
        "[subs=attributes]\n++++\n{a",
        "[subs=+callouts]\n<1>",
        "[subs=\"-,+,\"]\n----\nx\n----",
    ] {
        let result = parse(source);
        let ts = tokens(source);
        assert!(ts
            .iter()
            .all(|t| source.get(t.span.start..t.span.end).is_some()));
        let _ = result.document;
    }
}

#[test]
fn callout_icons_follow_the_mode_where_blocks_convert() {
    // Ruby reads `icons` while converting each block, after body entries.
    let result = parse("----\na <1>\n----\n<1> one\n\n:icons: font\n\n----\nb <1>\n----\n<1> two\n\n[subs=+quotes]\n----\nc <1>\n----\n\n:icons:\n:iconsdir: i\n\n----\nd <1>\n----\n<1> three\n\n:icons!:\n\n[subs=+quotes]\n----\ne <1>\n----");
    let blocks = &result.document.blocks;
    let mark = |block: &Block| match &block.kind {
        BlockKind::Listing { callouts, .. } => (callouts[0].icons.clone(), callouts[0].src.clone()),
        _ => panic!("{block:?}"),
    };
    let item = |block: &Block| match &block.kind {
        BlockKind::CalloutList { items } => (items[0].icons.clone(), items[0].src.clone()),
        _ => panic!("{block:?}"),
    };
    assert_eq!(mark(&blocks[0]), (None, String::new()));
    assert_eq!(item(&blocks[1]), (None, String::new()));
    assert_eq!(mark(&blocks[2]), (Some("font".into()), String::new()));
    assert_eq!(item(&blocks[3]), (Some("font".into()), String::new()));
    let (_, parts, _) = substituted(content(&blocks[4]).unwrap());
    assert_eq!(callouts(parts)[0].icons.as_deref(), Some("font"));
    let image = (Some(String::new()), "i/callouts/1.png".to_owned());
    assert_eq!(mark(&blocks[5]), image);
    assert_eq!(item(&blocks[6]), image);
    let (_, parts, _) = substituted(content(&blocks[7]).unwrap());
    assert_eq!(callouts(parts)[0].icons, None);
    // The header snapshot is unchanged.
    assert_eq!(result.document.attributes.get("icons"), None);
}
