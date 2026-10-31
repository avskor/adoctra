//! One diagnostic stream in Ruby's reporting order (checked against
//! Asciidoctor 2.0.23): the reader's, the parser's and reading-time
//! conversions as reading reaches them, then conversion. Spans stay the
//! physical byte ranges of each report.

use adoctra_core::{
    parse, parse_resource, MemoryResources, ParseOptions, ParseResult, SourceId, SourceMap, Span,
};

/// Ruby's message (the conversion report) and the editor span, in order.
fn reports(result: &ParseResult) -> Vec<(String, Span)> {
    result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.report.message.as_deref().unwrap_or(&d.message).to_owned(),
                d.span,
            )
        })
        .collect()
}

/// The byte range of the `nth` (0-based) occurrence of `needle`.
fn at(source: &str, needle: &str, nth: usize) -> Span {
    let start = source
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("{needle:?} #{nth} in {source:?}"))
        .0;
    Span::new(start, start + needle.len())
}

fn owned(expected: &[(&str, Span)]) -> Vec<(String, Span)> {
    expected
        .iter()
        .map(|(message, span)| ((*message).to_owned(), *span))
        .collect()
}

#[test]
fn preprocessor_errors_precede_the_unterminated_container_read_ahead() {
    // Ruby reads a container's body up to its fence before parsing it: the
    // directive on the way errs first, the missing fence warns at the end
    // of input, and the children's own reports follow.
    let source = "****\nendif::[]\n----\nx\n";
    let result = parse(source);
    assert_eq!(
        reports(&result),
        owned(&[
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 0)
            ),
            ("unterminated sidebar block", at(source, "****", 0)),
            ("unterminated listing block", at(source, "----", 0)),
        ])
    );
}

#[test]
fn a_section_title_converted_while_reading_warns_before_later_reading() {
    // With `sectids`, Ruby converts the title for its id while reading.
    let source = ":attribute-missing: warn\n== {g}\n```\n";
    let result = parse(source);
    assert_eq!(
        reports(&result),
        owned(&[
            (
                "skipping reference to missing attribute: g",
                at(source, "{g}", 0)
            ),
            ("unterminated listing block", at(source, "```", 0)),
        ])
    );
    // Ruby also converts a title with an explicit id while reading (its
    // registration caches it); without `sectids` it converts with the body.
    let source = ":attribute-missing: warn\n\n[#x]\n== {t2}\n\nendif::[]\n\n:sectids!:\n\n== {t3}\n\nendif::[]\n";
    let messages: Vec<_> = reports(&parse(source)).into_iter().map(|r| r.0).collect();
    assert_eq!(
        messages,
        [
            "skipping reference to missing attribute: t2",
            "unmatched preprocessor directive: endif::[]",
            "unmatched preprocessor directive: endif::[]",
            "skipping reference to missing attribute: t3",
        ]
    );
}

#[test]
fn section_level_warnings_follow_earlier_preprocessor_errors() {
    let source = ":attribute-missing: warn\n\n=== {t1}\n\nendif::[]\n\n== Ok\n\n{p}\n\n==== Bad\n";
    let result = parse(source);
    assert_eq!(
        reports(&result),
        owned(&[
            (
                "skipping reference to missing attribute: t1",
                at(source, "{t1}", 0)
            ),
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 0)
            ),
            (
                "section title out of sequence: expected level 2, got level 3",
                at(source, "==== Bad", 0)
            ),
            (
                "skipping reference to missing attribute: p",
                at(source, "{p}", 0)
            ),
        ])
    );
}

#[test]
fn a_nested_document_reports_while_its_cell_is_read() {
    // An AsciiDoc cell is parsed while the table is read: its section title
    // converts between the reader's reports around the table.
    let source = ":attribute-missing: warn\n\n.{bt}\n----\nendif::[]\n----\n\n|===\na|\n== {cellsec}\n\nendif::[]\n|===\n\nendif::[]\n";
    let result = parse(source);
    assert_eq!(
        reports(&result),
        owned(&[
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 0)
            ),
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 1)
            ),
            (
                "skipping reference to missing attribute: cellsec",
                at(source, "{cellsec}", 0)
            ),
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 2)
            ),
            // A block title converts with the body.
            (
                "skipping reference to missing attribute: bt",
                at(source, "{bt}", 0)
            ),
        ])
    );
}

#[test]
fn duplicate_ids_are_reported_where_reading_registers_them() {
    // Ruby registers a block's id when reading completes it, an inline
    // anchor's when it catalogs the paragraph and a section's at its title.
    let source = ":attribute-missing: warn\n\n[[a]]\npara {m1}\n\n[[a]]\n----\nx\n----\n\nendif::[]\n\nText [[b]] and [[b]] {m2}\n\nendif::[]\n\n== S [[c]]\n\n== T [[c]]\n\nendif::[]\n\n****\n";
    let result = parse(source);
    let block = at(source, "----\nx\n----", 0);
    assert_eq!(
        reports(&result),
        owned(&[
            ("id assigned to block already in use: a", block),
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 0)
            ),
            (
                "id assigned to anchor already in use: b",
                at(source, "[[b]]", 1)
            ),
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 1)
            ),
            (
                "id assigned to section already in use: c",
                at(source, "== T [[c]]", 0)
            ),
            (
                "unmatched preprocessor directive: endif::[]",
                at(source, "endif::[]", 2)
            ),
            ("unterminated sidebar block", at(source, "****", 0)),
            (
                "skipping reference to missing attribute: m1",
                at(source, "{m1}", 0)
            ),
            (
                "skipping reference to missing attribute: m2",
                at(source, "{m2}", 0)
            ),
        ])
    );
}

#[test]
fn include_selection_reports_keep_their_place_and_physical_ranges() {
    // Selection warnings already carry the include file's ranges; they stay
    // where the reader emitted them, before the container's read-ahead ends.
    let root = "= Doc\n:attribute-missing: warn\n\n****\ninclude::part.adoc[tag=x]\n\nendif::[]\n\n== {late}\n\ninclude::part.adoc[tag=missing]\n";
    let part = "before\n// tag::x[]\nin\n// end::y[]\nafter\n";
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", root).unwrap();
    resources.insert("/docs/part.adoc", part).unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    let in_part = |needle| {
        let span = at(part, needle, 0);
        Span::in_source(SourceId(1), span.start, span.end)
    };
    let in_root = |needle, nth| {
        let span = at(root, needle, nth);
        Span::in_source(source_id, span.start, span.end)
    };
    let actual = reports(&result);
    assert_eq!(
        actual,
        owned(&[
            (
                "detected unclosed tag 'x' starting at line 2 of include file: /docs/part.adoc",
                in_part("// tag::x[]")
            ),
            (
                "unmatched preprocessor directive: endif::[]",
                in_root("endif::[]", 0)
            ),
            (
                "tag 'missing' not found in include file: /docs/part.adoc",
                in_root("include::part.adoc[tag=missing]", 0)
            ),
            ("unterminated sidebar block", in_root("****", 0)),
            (
                "skipping reference to missing attribute: late",
                in_root("{late}", 0)
            ),
        ]),
        "{:#?}",
        result.diagnostics
    );
}
