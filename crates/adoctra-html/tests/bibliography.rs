use adoctra_core::{parse, Backend};
use adoctra_html::HtmlBackend;

fn render(source: &str) -> String {
    String::from_utf8(HtmlBackend::new().convert(&parse(source).document).unwrap()).unwrap()
}

#[test]
fn bibliography_labels_follow_rubys_distinct_item_and_reference_substitutions() {
    let html = render(":label: Book\n\n<<book>> <<attr>>\n\n[bibliography]\n* [[[book,*Bold*]]] Text\n* [[[attr,{label}]]] Text");
    assert!(html.contains("<ul class=\"bibliography\">"));
    assert!(
        html.contains("<a id=\"book\"></a>[&lt;strong&gt;Bold&lt;/strong&gt;] Text"),
        "{html}"
    );
    assert!(html.contains("<a href=\"#book\">[<strong>Bold</strong>]</a>"));
    assert!(html.contains("<a id=\"attr\"></a>[Book] Text"));
    assert!(html.contains("<a href=\"#attr\">[{label}]</a>"));
}

#[test]
fn first_bibliographic_declaration_wins_and_expressions_execute_once() {
    let html = render("[bibliography]\n* [[[book,{counter:n}]]] First\n* [[[book,Second]]] Second\n\n<<book>> {n}");
    assert!(html.contains("<a id=\"book\"></a>[1] First"));
    assert!(html.contains("<a href=\"#book\">[{counter:n}]</a> 1"));
}
