use adoctra_core::{parse, Backend};
use adoctra_json::JsonBackend;

#[test]
fn setext_serializes_the_same_semantic_header_as_atx() {
    let render = |source| {
        let result = parse(source);
        assert!(result.diagnostics.is_empty());
        String::from_utf8(JsonBackend.convert(&result.document).unwrap()).unwrap()
    };
    let actual = render("*Doc*\n=====\nJane Doe\nv2\n\n{author}");
    assert_eq!(actual, render("= *Doc*\nJane Doe\nv2\n\n{author}"));
    assert!(actual.contains(r#""name":"strong""#), "{actual}");
    assert!(actual.contains(r#""number":"2""#), "{actual}");
}

#[test]
fn setext_body_section_serializes_like_atx_heading() {
    let render = |source| {
        let result = parse(source);
        assert!(result.diagnostics.is_empty());
        String::from_utf8(JsonBackend.convert(&result.document).unwrap()).unwrap()
    };
    let actual = render("Intro\n\n*Sec*\n-----\n\n<<_sec>>");
    assert!(
        actual.contains(r#""name":"heading","level":1,"id":"_sec""#),
        "{actual}"
    );
    assert!(actual.contains(r#""name":"strong""#), "{actual}");
    assert!(!actual.contains("-----"), "{actual}");
}
