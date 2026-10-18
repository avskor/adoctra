use adoctra_core::ast::ListItem;
use adoctra_core::{
    parse, parse_with_options, tokens_with_options, AttributePart as P, Block, BlockKind, Inline,
    ParseOptions, SourceId, Span, TokenKind,
};

fn items(block: &Block) -> &[ListItem] {
    match &block.kind {
        BlockKind::List { items, .. } => items,
        _ => panic!("{block:?}"),
    }
}

#[test]
fn bibliography_has_neutral_anchor_parts_and_physical_unicode_spans() {
    let source = "[bibliography]\r\n* [[[книга,Ссылка]]] Текст\r\n\r\n<<книга>>";
    let options = ParseOptions {
        source_id: SourceId(7),
        ..Default::default()
    };
    let result = parse_with_options(source, &options);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let block = &result.document.blocks[0];
    assert_eq!(block.attrs.style.as_deref(), Some("bibliography"));
    let item = &items(block)[0];
    let start = source.find("[[[").unwrap();
    let end = source.find("]]]").unwrap() + 3;
    let span = Span::in_source(SourceId(7), start, end);
    assert_eq!(item.inlines[0].span(), span);
    match &item.inlines[0] {
        Inline::Substituted { parts, value, .. } => {
            assert_eq!(value, "[Ссылка]");
            assert_eq!(
                parts,
                &[P::Anchor("книга".into()), P::Text("[Ссылка]".into())]
            );
        }
        node => panic!("{node:?}"),
    }
    let tokens = tokens_with_options(source, &options);
    assert!(
        tokens
            .iter()
            .any(|t| t.kind == TokenKind::Anchor && t.span == span),
        "{tokens:?}"
    );
    assert!(tokens
        .iter()
        .all(|t| source.get(t.span.start..t.span.end).is_some()));
}

#[test]
fn bibliography_style_is_inherited_only_by_direct_unordered_section_children() {
    let source = "[bibliography]\n== References\n\n* [[[a]]] Book\n** [[[nested]]] Child\n\n--\n* [[[inside]]] Container\n--\n\n[square]\n* [[[override]]] Override\n\n== Next\n\n* [[[next]]] Normal";
    let result = parse(source);
    let BlockKind::Section { blocks, .. } = &result.document.blocks[0].kind else {
        panic!()
    };
    assert_eq!(blocks[1].attrs.style.as_deref(), Some("bibliography"));
    assert_eq!(items(&blocks[1])[0].blocks[0].attrs.style, None);
    let BlockKind::Open { blocks: inner } = &blocks[2].kind else {
        panic!()
    };
    assert_eq!(inner[0].attrs.style, None);
    assert_eq!(blocks[3].attrs.style.as_deref(), Some("square"));
    let BlockKind::Section { blocks, .. } = &result.document.blocks[1].kind else {
        panic!()
    };
    assert_eq!(blocks[1].attrs.style, None);
}

#[test]
fn bibliography_suppresses_checkboxes_and_regular_anchor_registration() {
    let result = parse("[bibliography]\n* [x] Task\n* [[ordinary,Label]] Book\n\n<<ordinary>>");
    let block = &result.document.blocks[0];
    assert!(!block.attrs.options.iter().any(|o| o == "checklist"));
    assert!(items(block).iter().all(|i| i.checkbox.is_none()));
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("ordinary")));
}

#[test]
fn duplicate_bibliography_id_warns_at_the_second_declaration() {
    let source = "[bibliography]\n* [[[book,First]]] One\n* [[[book,Second]]] Two\n\n<<book>>";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    let warning = &result.diagnostics[0];
    assert_eq!(
        warning.message,
        "id assigned to bibliography anchor already in use: book"
    );
    assert_eq!(
        &source[warning.span.start..warning.span.end],
        "[[[book,Second]]]"
    );
}

#[test]
fn incomplete_bibliography_input_keeps_valid_spans() {
    let complete = "[bibliography]\r\n* [[[книга,*Ссылка*]]] Текст\r\n\r\n<<книга>>";
    for end in (0..=complete.len()).filter(|&end| complete.is_char_boundary(end)) {
        let source = &complete[..end];
        let result = parse(source);
        for d in result.diagnostics {
            assert!(source.get(d.span.start..d.span.end).is_some(), "{d:?}");
        }
        for t in adoctra_core::tokens(source) {
            assert!(source.get(t.span.start..t.span.end).is_some(), "{t:?}");
        }
    }
}

#[test]
fn included_bibliography_keeps_its_source_identity_and_editor_tokens() {
    use adoctra_core::{parse_resource, tokens_resource, MemoryResources, SourceMap};
    let mut resources = MemoryResources::new();
    resources
        .insert(
            "/main.adoc",
            "[bibliography]\ninclude::refs.adoc[]\n\n<<книга>>",
        )
        .unwrap();
    resources
        .insert("/refs.adoc", "* [[[книга,Ссылка]]] Текст")
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let node = &items(&result.document.blocks[0])[0].inlines[0];
    assert_eq!(node.span().source_id, SourceId(1));
    assert_eq!(sources.slice(node.span()), Some("[[[книга,Ссылка]]]"));
    let ts = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(ts
        .iter()
        .any(|t| t.kind == TokenKind::Anchor && t.span == node.span()));
}
