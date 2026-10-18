//! Tables: Ruby's `parse_table`, `Table::ParserContext`, `Table::Cell` and
//! `Table::Column` (psv/csv/dsv data, cell specs, implicit header, widths),
//! table captions and highlighting.
use adoctra_core::ast::{CellContent, CellStyle, HAlign, Table, TableCell, VAlign};
use adoctra_core::{
    parse, parse_resource, tokens, tokens_resource, Block, BlockKind, Inline, MemoryResources,
    ParseOptions, Severity, SourceId, SourceMap, Span, TokenKind,
};

fn blocks(source: &str) -> Vec<Block> {
    parse(source).document.into_flat_blocks()
}

fn table(block: &Block) -> &Table {
    match &block.kind {
        BlockKind::Table(table) => table,
        other => panic!("not a table: {other:?}"),
    }
}

fn text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => value.clone(),
            Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::CrossReference { children, .. } => text(children),
            Inline::Anchor { .. } => String::new(),
            other => panic!("unexpected inline {other:?}"),
        })
        .collect()
}

/// A cell's paragraphs as plain text.
fn paragraphs(cell: &TableCell) -> Vec<String> {
    match &cell.content {
        CellContent::Paragraphs(paragraphs) => paragraphs.iter().map(|p| text(p)).collect(),
        CellContent::Literal(text) => vec![format!("literal:{text}")],
        CellContent::Blocks { blocks, .. } => vec![format!("blocks:{}", blocks.len())],
    }
}

fn rows(rows: &[Vec<TableCell>]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|cell| paragraphs(cell).join("|")).collect())
        .collect()
}

#[test]
fn psv_table_has_an_implicit_header_and_body_rows_with_spans() {
    let source = "|===\n|Name |Value\n\n|a |*b*\n|===\n";
    let blocks = blocks(source);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].span, Span::new(0, source.len() - 1));
    let t = table(&blocks[0]);
    assert_eq!(rows(&t.head), [["Name", "Value"]]);
    assert_eq!(rows(&t.body), [["a", "b"]]);
    assert!(t.foot.is_empty());
    let widths: Vec<_> = t.columns.iter().map(|c| c.width.as_str()).collect();
    assert_eq!(widths, ["50", "50"]);
    assert_eq!(
        (t.width, t.frame.as_str(), t.grid.as_str()),
        (100, "all", "all")
    );
    // A cell spans its separator through its stripped text.
    let name = &t.head[0][0];
    assert_eq!(&source[name.span.start..name.span.end], "|Name");
    let value = &t.body[0][1];
    assert_eq!(&source[value.span.start..value.span.end], "|*b*");
    let CellContent::Paragraphs(paragraphs) = &value.content else {
        panic!("not paragraphs");
    };
    let strong = &paragraphs[0][0];
    assert!(matches!(strong, Inline::Strong { .. }));
    assert_eq!(&source[strong.span().start..strong.span().end], "*b*");
}

#[test]
fn implicit_header_needs_the_whole_first_row_followed_by_a_blank_line() {
    let t = blocks("[cols=2]\n|===\n|A\n|B\n\n|c|d\n|===\n");
    assert!(table(&t[0]).head.is_empty());
    assert_eq!(rows(&table(&t[0]).body), [["A", "B"], ["c", "d"]]);
    let t = blocks("|===\n\n|A|B\n\n|c|d\n|===\n");
    assert!(table(&t[0]).head.is_empty(), "a leading blank line");
    let t = blocks("[%noheader]\n|===\n|A|B\n\n|c|d\n|===\n");
    assert!(table(&t[0]).head.is_empty());
    let t = blocks("[%header%footer]\n|===\n|H|I\n|a|b\n|F|G\n|===\n");
    let t = table(&t[0]);
    assert_eq!(rows(&t.head), [["H", "I"]]);
    assert_eq!(rows(&t.body), [["a", "b"]]);
    assert_eq!(rows(&t.foot), [["F", "G"]]);
}

#[test]
fn column_specs_set_widths_alignments_and_styles() {
    let t = blocks("[cols=\"1,^.>2e,~\"]\n|===\n|a|b|c\n|===\n");
    let t = table(&t[0]);
    let columns: Vec<_> = t
        .columns
        .iter()
        .map(|c| (c.width.as_str(), c.halign, c.valign, c.style, c.autowidth))
        .collect();
    assert_eq!(
        columns,
        [
            ("1", HAlign::Left, VAlign::Top, None, false),
            (
                "2",
                HAlign::Center,
                VAlign::Bottom,
                Some(CellStyle::Emphasis),
                false
            ),
            ("97", HAlign::Left, VAlign::Top, None, true),
        ]
    );
    // The implicit header is unstyled; body cells take the column's.
    let cell = &t.body[0][1];
    assert_eq!(cell.style, Some(CellStyle::Emphasis));
    assert_eq!((cell.halign, cell.valign), (HAlign::Center, VAlign::Bottom));
    let t = blocks("[cols=\"1,2,3\"]\n|===\n|a|b|c\n|===\n");
    let widths: Vec<_> = table(&t[0])
        .columns
        .iter()
        .map(|c| c.width.clone())
        .collect();
    assert_eq!(widths, ["16.6666", "33.3333", "50.0001"]);
}

#[test]
fn cell_specs_span_repeat_align_and_style() {
    let source = "[cols=3]\n|===\n2+|wide |x\n.2+^.^s|tall |b |c\n|e |f\n3*|rep\nl|  lit <&>\n  more |m h|th\n|===\n";
    let t = blocks(source);
    let t = table(&t[0]);
    let body = &t.body;
    assert_eq!(body[0][0].colspan, Some(2));
    assert_eq!(body[0][0].rowspan, None);
    let tall = &body[1][0];
    assert_eq!((tall.rowspan, tall.colspan), (Some(2), None));
    assert_eq!((tall.halign, tall.valign), (HAlign::Center, VAlign::Middle));
    assert_eq!(tall.style, Some(CellStyle::Strong));
    assert_eq!(&source[tall.span.start..tall.span.end], ".2+^.^s|tall");
    // The rowspan leaves room for two more cells in the next row.
    assert_eq!(rows(&body[2..3]), [["e", "f"]]);
    // A repeated cell is three cells with the same source.
    assert_eq!(rows(&body[3..4]), [["rep", "rep", "rep"]]);
    assert_eq!(body[3][0].span, body[3][2].span);
    let literal = &body[4][0];
    assert_eq!(
        literal.content,
        CellContent::Literal("  lit <&>\n  more".into())
    );
    assert_eq!(literal.style, Some(CellStyle::Literal));
    assert_eq!(body[4][2].style, Some(CellStyle::Header));
}

#[test]
fn body_cells_split_into_paragraphs_and_escaped_separators_keep_spans() {
    let source = "|===\n|first\nline\n\nsecond |a \\| b\n|===\n";
    let t = blocks(source);
    let t = table(&t[0]);
    assert_eq!(paragraphs(&t.body[0][0]), ["first line", "second"]);
    let cell = &t.body[0][1];
    assert_eq!(paragraphs(cell), ["a | b"]);
    let CellContent::Paragraphs(p) = &cell.content else {
        panic!()
    };
    // The text maps around the removed backslash.
    let first = p[0].first().unwrap().span();
    let last = p[0].last().unwrap().span();
    assert_eq!(&source[first.start..last.end], "a \\| b");
}

#[test]
fn csv_and_dsv_data_unquote_and_unescape() {
    let source = ",===\nA,\"q, \"\"x\"\"\"\n\"multi\nline\",b\n,===\n";
    let t = blocks(source);
    let t = table(&t[0]);
    assert!(t.head.is_empty());
    assert_eq!(rows(&t.body), [["A", "q, \"x\""], ["multi line", "b"]]);
    let cell = &t.body[0][1];
    let CellContent::Paragraphs(p) = &cell.content else {
        panic!()
    };
    let span = p[0][0].span();
    assert_eq!(&source[span.start..span.end], "q, \"\"x\"");
    let t = blocks(":===\na:b\\:c\n:===\n");
    assert_eq!(rows(&table(&t[0]).body), [["a", "b:c"]]);
    let t = blocks("[format=tsv]\n|===\na\tb\n|===\n");
    assert_eq!(rows(&table(&t[0]).body), [["a", "b"]]);
    let t = blocks("[separator=;]\n|===\n;a;b\n|===\n");
    assert_eq!(rows(&table(&t[0]).body), [["a", "b"]]);
}

#[test]
fn malformed_tables_report_ruby_diagnostics_but_still_parse() {
    let source = "[cols=2,format=xsv]\n|===\nno separator\n|a|b|c\n|d\n|===\n\n[cols=2]\n|===\n|a 2+|b\n|c|d\n|===\n\n|===\n|x\n";
    let result = parse(source);
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.message.as_str()))
        .collect();
    assert_eq!(
        messages,
        [
            (Severity::Error, "illegal table format: xsv"),
            (
                Severity::Error,
                "table missing leading separator; recovering automatically"
            ),
            (
                Severity::Error,
                "dropping cells from incomplete row detected end of table"
            ),
            (
                Severity::Error,
                "dropping cell because it exceeds specified number of columns"
            ),
            (Severity::Warning, "unterminated table block"),
        ]
    );
    // Like Ruby, the first cell takes the next cell's spec, so the last one
    // lacks a separator; the error points at the table's first line.
    let missing = result.diagnostics[1].span;
    assert_eq!(&source[missing.start..missing.end], "no separator");
    let dropped = result.diagnostics[3].span;
    assert_eq!(&source[dropped.start..dropped.end], "2+|b");
    let blocks = result.document.into_flat_blocks();
    assert_eq!(
        rows(&table(&blocks[0]).body),
        [["no separator", "a"], ["b", "c"]]
    );
    assert_eq!(rows(&table(&blocks[1]).body), [["c", "d"]]);
    assert_eq!(rows(&table(&blocks[2]).body), [["x"]]);
    let result = parse("[cols=\"60,60,~\"]\n|===\n|a|b|c\n|===\n\n,===\n\"\n,===\n");
    let messages: Vec<_> = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "total column width must not exceed 100% when using autowidth columns; got 120%",
            "unclosed quote in CSV data; setting cell to empty",
        ]
    );
}

#[test]
fn titled_tables_are_numbered_and_referable() {
    let source = ":xrefstyle: full\n\n[[t]]\n.Results\n|===\n|a\n|===\n\n:table-caption: Таблица\n\n.Next\n|===\n|b\n|===\n\nSee <<t>>.\n";
    let blocks = blocks(source);
    assert_eq!(blocks[0].caption.as_deref(), Some("Table 1. "));
    assert_eq!(blocks[1].caption.as_deref(), Some("Таблица 2. "));
    let BlockKind::Paragraph { inlines } = &blocks[2].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "See Таблица 1, “Results”.");
}

#[test]
fn presentation_attributes_resolve_with_document_fallbacks() {
    let source = ":table-frame: topbot\n:table-stripes: odd\n\n[width=40%,grid=rows]\n|===\n|a\n|===\n\n[frame=none,stripes=hover]\n|===\n|b\n|===\n";
    let blocks = blocks(source);
    let first = table(&blocks[0]);
    assert_eq!(
        (first.width, first.frame.as_str(), first.grid.as_str()),
        (40, "ends", "rows")
    );
    assert_eq!(first.stripes.as_deref(), Some("odd"));
    let second = table(&blocks[1]);
    assert_eq!(
        (second.frame.as_str(), second.stripes.as_deref()),
        ("none", Some("hover"))
    );
}

#[test]
fn cells_convert_in_order_and_read_cellbgcolor_after_their_content() {
    let source =
        "[cols=3]\n|===\n3*|{counter:n}\n|{set:cellbgcolor:red}x |y |{set:cellbgcolor!}z\n|===\n";
    let blocks = blocks(source);
    let t = table(&blocks[0]);
    // Like Ruby, the unset expression leaves the cell text empty.
    assert_eq!(rows(&t.body), [["1", "2", "3"], ["x", "y", ""]]);
    let colors: Vec<_> = t.body[1].iter().map(|c| c.background.as_deref()).collect();
    assert_eq!(colors, [Some("red"), Some("red"), None]);
}

#[test]
fn leading_cell_anchors_are_cataloged_for_references() {
    let source = "|===\n|[[h1,Head]]H |x\n\n|[[b1]]Body |<<h1>> <<b1>>\n|===\n";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = result.document.into_flat_blocks();
    let t = table(&blocks[0]);
    assert_eq!(rows(&t.body)[0][1], "Head [b1]");
}

#[test]
fn a_table_line_ends_a_paragraph_and_line_comments_are_skipped() {
    let source = "text\n|===\n|a\n// note\n|b\n|===\n";
    let result = parse(source);
    let blocks = result.document.into_flat_blocks();
    assert!(matches!(blocks[0].kind, BlockKind::Paragraph { .. }));
    assert_eq!(rows(&table(&blocks[1]).body), [["a"], ["b"]]);
    assert_eq!(
        &source[result.comments[0].start..result.comments[0].end],
        "// note"
    );
}

#[test]
fn tokens_mark_fences_separators_and_cell_markup() {
    let source = "|===\n|*a* 2+|b\n// c\n|x\n|===\n";
    let tokens = tokens(source);
    let kinds: Vec<_> = tokens
        .iter()
        .map(|t| (t.kind, &source[t.span.start..t.span.end]))
        .collect();
    assert_eq!(
        kinds,
        [
            (TokenKind::TableDelimiter, "|==="),
            (TokenKind::CellSeparator, "|"),
            (TokenKind::Strong, "*a*"),
            (TokenKind::CellSeparator, "2+|"),
            (TokenKind::Comment, "// c"),
            (TokenKind::CellSeparator, "|"),
            (TokenKind::TableDelimiter, "|==="),
        ]
    );
    assert_eq!(TokenKind::CellSeparator.as_str(), "cell-separator");
    assert_eq!(TokenKind::TableDelimiter.as_str(), "table-delimiter");
}

#[test]
fn included_rows_keep_physical_cell_spans() {
    let mut resources = MemoryResources::new();
    resources
        .insert("/docs/main.adoc", "|===\ninclude::rows.adoc[]\n|===\n")
        .unwrap();
    resources.insert("/docs/rows.adoc", "|a |*b*\n").unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let blocks = result.document.into_flat_blocks();
    let cell = &table(&blocks[0]).body[0][1];
    assert_eq!(cell.span.source_id, SourceId(1));
    assert_eq!(sources.slice(cell.span), Some("|*b*"));
    let tokens = tokens_resource(&mut sources, &options, &resources).unwrap();
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::Strong && sources.slice(t.span) == Some("*b*")));
    assert!(tokens
        .iter()
        .any(|t| t.kind == TokenKind::CellSeparator && t.span.source_id == SourceId(1)));
}

/// The blocks of an AsciiDoc cell.
fn cell_blocks(cell: &TableCell) -> &[Block] {
    match &cell.content {
        CellContent::Blocks { blocks, .. } => blocks,
        other => panic!("not an AsciiDoc cell: {other:?}"),
    }
}

fn paragraph_text(block: &Block) -> String {
    match &block.kind {
        BlockKind::Paragraph { inlines } => text(inlines),
        other => panic!("not a paragraph: {other:?}"),
    }
}

#[test]
fn asciidoc_cells_are_nested_documents_with_source_spans() {
    let source = "[cols=\"1,1a\"]\n|===\n|x a|* one\n* *two*\n\n----\ncode\n----\n|===\n";
    let blocks = blocks(source);
    let t = table(&blocks[0]);
    let cell = &t.body[0][1];
    assert_eq!(cell.style, Some(CellStyle::Asciidoc));
    let nested = cell_blocks(cell);
    assert_eq!(nested.len(), 2);
    let list = &nested[0];
    assert!(matches!(list.kind, BlockKind::List { .. }));
    assert_eq!(&source[list.span.start..list.span.end], "* one\n* *two*");
    let BlockKind::List { items, .. } = &list.kind else {
        panic!()
    };
    let strong = items[1].inlines[0].span();
    assert_eq!(&source[strong.start..strong.end], "*two*");
    assert_eq!(
        nested[1].kind,
        BlockKind::Listing {
            text: "code".into(),
            callouts: Vec::new(),
            inlines: None,
        }
    );
}

#[test]
fn nested_documents_share_references_counters_and_captions() {
    let source = "See <<blk>>.\n\n.A\n====\na\n====\n\n|===\na|\n[[blk]]\n.B\n====\n{counter:c} {c}\n====\n|===\n\n.C\n====\n{c} {counter:c}\n====\n";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = result.document.into_flat_blocks();
    let BlockKind::Paragraph { inlines } = &blocks[0].kind else {
        panic!()
    };
    assert_eq!(text(inlines), "See B.");
    let nested = cell_blocks(&table(&blocks[2]).body[0][0]);
    assert_eq!(nested[0].caption.as_deref(), Some("Example 2. "));
    // The counter belongs to the root document; the nested document does
    // not see its attribute.
    let BlockKind::Example { blocks: inner } = &nested[0].kind else {
        panic!()
    };
    assert_eq!(paragraph_text(&inner[0]), "1 {c}");
    assert_eq!(blocks[3].caption.as_deref(), Some("Example 3. "));
    let BlockKind::Example { blocks: last } = &blocks[3].kind else {
        panic!()
    };
    assert_eq!(paragraph_text(&last[0]), "1 2");
}

#[test]
fn nested_documents_lock_inherited_attributes_and_keep_their_entries() {
    let source = ":foo: outer\n\n|===\na|:foo: inner\n:new: N\n\n{foo} {new}\n\n:new: changed\n\n{new}\n|===\n\n{foo} {new}\n";
    let blocks = blocks(source);
    let nested = cell_blocks(&table(&blocks[0]).body[0][0]);
    let texts: Vec<_> = nested.iter().map(paragraph_text).collect();
    assert_eq!(texts, ["outer N", "changed"]);
    assert_eq!(paragraph_text(&blocks[1]), "outer {new}");
}

#[test]
fn nested_tables_use_the_exclamation_separator() {
    let source = "|===\na|\n!===\n!in 2+!wide\n!===\n|===\n";
    let blocks = blocks(source);
    let nested = cell_blocks(&table(&blocks[0]).body[0][0]);
    let inner = table(&nested[0]);
    assert_eq!(rows(&inner.body), [["in", "wide"]]);
    assert_eq!(inner.body[0][1].colspan, Some(2));
    let span = inner.body[0][1].span;
    assert_eq!(&source[span.start..span.end], "2+!wide");
    let tokens = tokens(source);
    let kinds: Vec<_> = tokens
        .iter()
        .map(|t| (t.kind, &source[t.span.start..t.span.end]))
        .collect();
    assert_eq!(
        kinds,
        [
            (TokenKind::TableDelimiter, "|==="),
            (TokenKind::CellSeparator, "a|"),
            (TokenKind::TableDelimiter, "!==="),
            (TokenKind::CellSeparator, "!"),
            (TokenKind::CellSeparator, "2+!"),
            (TokenKind::TableDelimiter, "!==="),
            (TokenKind::TableDelimiter, "|==="),
        ]
    );
}

#[test]
fn asciidoc_cells_in_an_implicit_header_follow_ruby() {
    // A confirmed header row keeps its text; a rejected one reads the nested
    // document after all.
    let blocks = blocks("[cols=\"1a,1\"]\n|===\n|*H* |I\n\n|* x |y\n|===\n\n[cols=\"1a,1\"]\n|===\n|* one\n|b\n|===\n");
    let first = table(&blocks[0]);
    assert_eq!(rows(&first.head), [["H", "I"]]);
    assert!(matches!(
        cell_blocks(&first.body[0][0])[0].kind,
        BlockKind::List { .. }
    ));
    let second = table(&blocks[1]);
    assert!(second.head.is_empty());
    assert!(matches!(
        cell_blocks(&second.body[0][0])[0].kind,
        BlockKind::List { .. }
    ));
}

#[test]
fn the_first_line_of_an_asciidoc_cell_is_preprocessed_again() {
    let mut resources = MemoryResources::new();
    resources
        .insert(
            "/docs/main.adoc",
            "[cols=\"1a,1\"]\n|===\na|include::part.adoc[tag=a]\n\nmore *x* |y\n|===\n",
        )
        .unwrap();
    resources
        .insert(
            "/docs/part.adoc",
            "// tag::a[]\n* tagged *b*\n// end::a[]\nout\n",
        )
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let blocks = result.document.into_flat_blocks();
    let nested = cell_blocks(&table(&blocks[0]).body[0][0]);
    let BlockKind::List { items, .. } = &nested[0].kind else {
        panic!("{nested:?}")
    };
    let strong = items[0].inlines[1].span();
    assert_eq!(strong.source_id, SourceId(1));
    assert_eq!(sources.slice(strong), Some("*b*"));
    // The rest of the cell keeps its physical source.
    let BlockKind::Paragraph { inlines } = &nested[1].kind else {
        panic!()
    };
    let x = inlines[1].span();
    assert_eq!((x.source_id, sources.slice(x)), (source_id, Some("*x*")));
    // Without a resource provider the include stays text; a single-line
    // conditional is evaluated with the parent's attributes.
    let plain = self::blocks(":on:\n\n|===\na|ifdef::on[shown] a|include::x.adoc[]\n|===\n");
    let row = &table(&plain[0]).body[0];
    assert_eq!(paragraph_text(&cell_blocks(&row[0])[0]), "shown");
    assert_eq!(
        paragraph_text(&cell_blocks(&row[1])[0]),
        "include::x.adoc[]"
    );
}
