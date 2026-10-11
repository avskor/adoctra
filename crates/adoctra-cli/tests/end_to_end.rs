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

#[test]
fn reference_in_block_title_renders_in_html_and_json() {
    let source = ".See <<_target>>\n----\ncode\n----\n\n== Target";
    let result = adoctra::parse(source);
    assert!(result.diagnostics.is_empty());
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(
        html.contains("<div class=\"title\">See <a href=\"#_target\">Target</a></div>"),
        "{html}"
    );
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(json.contains(r#""title":[{"name":"text","value":"See "},{"name":"xref","target":"_target","inlines":[{"name":"text","value":"Target"}]}]"#), "{json}");
}

#[test]
fn header_and_body_attribute_values_reach_html_and_json() {
    let result = adoctra::parse("= Doc\n:who: first\n\n{who}\n\n:who: second\n\n{who}");
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(
        html.contains("<p>first</p>") && html.contains("<p>second</p>"),
        "{html}"
    );
    assert!(!html.contains(":who:"));
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(
        json.contains(r#""value":"first""#) && json.contains(r#""value":"second""#),
        "{json}"
    );
}

#[test]
fn cli_diagnostic_has_source_line_and_unicode_column() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_adoctra"))
        .args(["-", "--stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("Привет <<missing>>".as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("-:1:8: warning"), "{stderr}");
}

#[test]
fn header_declarations_and_resolved_values_reach_backends() {
    let result = adoctra::parse(
        "= Doc\nJane Doe <jane@example.org>\nv2, today: Draft\n\nBy {author}, version {revnumber}.",
    );
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(html.contains("<p>By Jane Doe, version 2.</p>"), "{html}");
    assert!(!html.contains("jane@example.org"));
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(json.contains(r#""header":{"authors":[{"name":"Jane Doe","firstname":"Jane","middlename":null,"lastname":"Doe","initials":"JD","email":"jane@example.org"}],"revision":{"number":"2","date":"today","remark":"Draft"}}"#), "{json}");
    assert!(json.contains(r#""value":"Jane Doe""#), "{json}");
}

#[test]
fn comments_are_omitted_in_html_and_json_but_verbatim_is_preserved() {
    let source = "= Doc\n// hidden author note\nJane Doe\n\none\n// secret\n*two*\n\n////\n<<missing>>\n////\n\n----\n// visible\n----";
    let result = adoctra::parse(source);
    assert!(result.diagnostics.is_empty());
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(html.contains("<p>one <strong>two</strong></p>"), "{html}");
    assert!(html.contains("// visible"));
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(json.contains(r#""text":"// visible""#), "{json}");
    for output in [html, json] {
        assert!(
            !output.contains("secret")
                && !output.contains("hidden author")
                && !output.contains("missing"),
            "{output}"
        );
    }
}
