use adoctra_core::{ast::BlockKind, parse, Inline, Severity};

fn reference(inlines: &[Inline]) -> (&str, &[Inline], adoctra_core::Span) {
    inlines
        .iter()
        .find_map(|node| match node {
            Inline::CrossReference {
                target,
                children,
                span,
            } => Some((target.as_str(), children.as_slice(), *span)),
            _ => None,
        })
        .expect("cross-reference")
}

#[test]
fn forward_reference_in_block_title_resolves_and_preserves_source_span() {
    let source = ".See <<_target>>\n----\ncode\n----\n\n== Target\n\nBody.";
    let result = parse(source);
    assert!(result.diagnostics.is_empty());
    let (target, children, span) = reference(result.document.blocks[0].title.as_ref().unwrap());
    assert_eq!(target, "_target");
    assert!(matches!(&children[0], Inline::Text { value, .. } if value == "Target"));
    assert_eq!(&source[span.start..span.end], "<<_target>>");
}

#[test]
fn unresolved_reference_in_nested_title_reports_original_span() {
    let source = "====\n.See <<missing>>\n----\nx\n----\n====";
    let result = parse(source);
    assert_eq!(result.diagnostics.len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.severity, Severity::Warning);
    assert_eq!(
        &source[diagnostic.span.start..diagnostic.span.end],
        "<<missing>>"
    );
    let BlockKind::Example { blocks } = &result.document.blocks[0].kind else {
        panic!("example")
    };
    let (_, children, _) = reference(blocks[0].title.as_ref().unwrap());
    assert!(matches!(&children[0], Inline::Text { value, .. } if value == "[missing]"));
}
