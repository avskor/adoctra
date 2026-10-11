use adoctra_core::{parse, tokens, BlockKind, Inline, Span, TokenKind};

#[test]
fn ascii_suffixes_are_removed_but_internal_and_unicode_whitespace_survive() {
    for fence in ["----", "....", "++++"] {
        let source = format!(
            "{fence} \t\r\n\tЯ\t🙂 \t\x0b\x0c\0\r\n \t\0\r\nX\u{a0}\u{2003} \t\r\n{fence} \t"
        );
        let result = parse(&source);
        assert!(result.diagnostics.is_empty(), "{result:?}");
        let block = &result.document.blocks[0];
        let text = match &block.kind {
            BlockKind::Listing { text }
            | BlockKind::Literal { text }
            | BlockKind::Passthrough { text } => text,
            other => panic!("{other:?}"),
        };
        // The capture-based passthrough path preserves physical CRLF endings.
        assert_eq!(text.replace("\r\n", "\n"), "\tЯ\t🙂\n\nX\u{a0}\u{2003}");
        let end = source.rfind(fence).unwrap() + fence.len();
        let fragments = &result.provenance[block.span.provenance.unwrap()];
        assert_eq!(fragments.last().unwrap().end, end);
    }
}

#[test]
fn normalization_preserves_inline_diagnostic_and_token_positions() {
    let source = "\u{feff}*Я* \t\r\n_🙂_ \t\0\r\n<<missing>> \t\r\n \t\r\n";
    let result = parse(source);
    assert_eq!(result.document.blocks.len(), 1);
    let BlockKind::Paragraph { inlines } = &result.document.blocks[0].kind else {
        panic!("{result:?}")
    };
    assert!(matches!(inlines[0], Inline::Strong { .. }));
    let highlighted = tokens(source);
    for (text, kind) in [
        ("*Я*", TokenKind::Strong),
        ("_🙂_", TokenKind::Emphasis),
        ("<<missing>>", TokenKind::CrossReference),
    ] {
        let start = source.find(text).unwrap();
        let span = Span::new(start, start + text.len());
        assert!(inlines.iter().any(|inline| inline.span() == span));
        assert!(highlighted.iter().any(|t| t.span == span && t.kind == kind));
        if text == "<<missing>>" {
            assert_eq!(result.diagnostics[0].span, span);
        }
    }
    for token in highlighted {
        let text = &source[token.span.start..token.span.end];
        assert!(!text.contains('\t') && !text.contains('\0'), "{token:?}");
    }
}

#[test]
fn primary_reader_condenses_trailing_empty_lines_even_for_incomplete_blocks() {
    for source in ["----\nX\n\n \t\n", "----\r\nX \t\r\n\0\r\n"] {
        let result = parse(source);
        assert_eq!(result.diagnostics.len(), 1);
        let BlockKind::Listing { text } = &result.document.blocks[0].kind else {
            panic!("{result:?}")
        };
        assert_eq!(text, "X");
        let end = source.find('X').unwrap() + 1;
        let block = &result.document.blocks[0];
        let last = block
            .span
            .provenance
            .map_or(block.span, |id| *result.provenance[id].last().unwrap());
        assert_eq!(last.end, end);
    }
    assert!(parse("\u{feff} \t\0\r\n\n").document.blocks.is_empty());
}

#[test]
fn every_partial_unicode_input_retains_valid_spans() {
    let source = "\u{feff}= Я \t\r\n\r\n*🙂* \t\r\n---- \0\r\nX \t\r\n \t\r\n----";
    for end in (0..=source.len()).filter(|&end| source.is_char_boundary(end)) {
        let partial = &source[..end];
        let result = parse(partial);
        for span in tokens(partial)
            .into_iter()
            .map(|t| t.span)
            .chain(result.diagnostics.iter().map(|d| d.span))
            .chain(result.provenance.iter().flatten().copied())
        {
            assert!(
                partial.get(span.start..span.end).is_some(),
                "{partial:?}: {span:?}"
            );
        }
    }
}
