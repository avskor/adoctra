//! Cross-crate integration test.

use adoctra::Backend;

#[test]
fn parse_then_render_html() {
    let doc = adoctra::parse("= Title\n\nHello *world*").document;
    let html = String::from_utf8(adoctra::html::HtmlBackend::new().convert(&doc).unwrap()).unwrap();
    assert!(html.contains("<h1>Title</h1>"));
    assert!(html.contains("<strong>world</strong>"));
}

#[test]
fn diagnostics_are_reported_not_fatal() {
    let result = adoctra::parse("----\nunterminated");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.document.blocks.len(), 1);
}

#[test]
fn cross_reference_resolves_to_section() {
    // A section gets an auto id; `<<id>>` resolves to its title, end to end.
    let result = adoctra::parse("== Hello\n\nSee <<_hello>>.");
    assert!(result.diagnostics.is_empty());
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(html.contains("<h2 id=\"_hello\">Hello</h2>"), "got: {html}");
    assert!(
        html.contains("<a href=\"#_hello\">Hello</a>"),
        "got: {html}"
    );
}

#[test]
fn unresolved_cross_reference_is_a_diagnostic() {
    let result = adoctra::parse("See <<missing>>.");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].severity, adoctra::Severity::Warning);
}

#[test]
fn tokens_drive_highlighting() {
    let toks = adoctra::tokens("= Title\n\ntext *bold*");
    assert!(toks.iter().any(|t| t.kind == adoctra::TokenKind::Heading));
    assert!(toks.iter().any(|t| t.kind == adoctra::TokenKind::Strong));
}
