use adoctra_core::{parse, AttributePart, AuthorDetails};

fn text(parts: &[AttributePart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            AttributePart::Text(text) | AttributePart::Raw(text) => text.as_str(),
            _ => "",
        })
        .collect()
}

#[test]
fn authors_follow_ruby_document_authors_with_replacements_and_macros() {
    let doc = parse(
        "= Doc\nJohn O'Doe <john@example.com>; Jane -- Roe <https://example.org>; Max\n\nx\n",
    )
    .document;
    let authors = doc.authors();
    assert_eq!(authors.len(), 3);
    // Names get replacements (generated entities stay raw spelling).
    assert_eq!(text(&authors[0].name), "John O&#8217;Doe");
    assert_eq!(
        authors[2],
        AuthorDetails {
            name: vec![AttributePart::Text("Max".into())],
            email: None
        }
    );
    // Emails get macros: an address becomes a mailto link.
    let email = authors[0].email.as_ref().unwrap();
    assert!(
        email.iter().any(|part| matches!(part, AttributePart::OpenLink(link) if link.target == "mailto:john@example.com")),
        "{email:?}"
    );
    assert_eq!(
        text(&doc.authors_line().unwrap()),
        "John O&#8217;Doe, Jane&#8201;&#8212;&#8201;Roe <https://example.org>, Max"
    );
    // Like Ruby, a bracketed URL is part of the name, not an email.
    assert!(authors[1].email.is_none());
}

#[test]
fn authors_come_from_header_attributes() {
    let doc = parse("= Doc\n:author: Ann Lee\n:email: ann@example.com\n\nx\n").document;
    let authors = doc.authors();
    assert_eq!(authors.len(), 1);
    assert_eq!(text(&authors[0].name), "Ann Lee");
    assert!(authors[0].email.is_some());
    // No author attribute, no authors (Ruby's `key? 'author'`).
    let doc = parse("= Doc\n\nx\n").document;
    assert!(doc.authors().is_empty());
    assert!(doc.authors_line().is_none());
    // Body assignments do not reach the header snapshot.
    let doc = parse("= Doc\n\nx\n\n:author: Late\n\ny\n").document;
    assert!(doc.authors().is_empty());
}
