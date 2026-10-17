use adoctra_core::{parse, Backend};
use adoctra_json::JsonBackend;

fn render(source: &str) -> String {
    String::from_utf8(JsonBackend.convert(&parse(source).document).unwrap()).unwrap()
}

#[test]
fn sections_and_preamble_serialize_as_nested_containers() {
    let json = render("= Doc\n\nPre\n\n== A\n\na\n\n=== B");
    assert!(
        json.contains(r#"{"name":"preamble","blocks":[{"name":"paragraph""#),
        "{json}"
    );
    assert!(
        json.contains(
            r#"{"name":"section","level":1,"blocks":[{"name":"heading","level":1,"id":"_a""#
        ),
        "{json}"
    );
    assert!(
        json.contains(
            r#"{"name":"section","level":2,"blocks":[{"name":"heading","level":2,"id":"_b""#
        ),
        "{json}"
    );
    // A standalone preamble is unwrapped and a discrete heading stays a heading.
    let json = render("= Doc\n\nPre\n\n[discrete]\n== D");
    assert!(
        !json.contains("preamble") && !json.contains("section"),
        "{json}"
    );
}
