//! Ruby's locked processor overrides `asciidoctor` and `asciidoctor-version`.
use adoctra_core::{
    parse, parse_with_options, Attributes, BlockKind, Inline, ParseOptions, COMPATIBILITY_VERSION,
};

fn paragraph_text(blocks: &[adoctra_core::Block]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(
                inlines
                    .iter()
                    .map(|inline| match inline {
                        Inline::Text { value, .. } => value.clone(),
                        Inline::AttributeRef { value, name, .. } => {
                            value.clone().unwrap_or_else(|| format!("{{{name}}}"))
                        }
                        _ => String::new(),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn version_matches_the_pinned_reference() {
    assert_eq!(
        COMPATIBILITY_VERSION,
        include_str!("../../../compatibility/asciidoctor-version").trim()
    );
}

#[test]
fn attributes_are_set_and_locked_against_documents_and_hosts() {
    let mut host = Attributes::new();
    host.set("asciidoctor-version", "9");
    host.set("asciidoctor", "x");
    let options = ParseOptions {
        attributes: host,
        ..Default::default()
    };
    let source = ":asciidoctor-version: 1\n:asciidoctor!:\n\n{asciidoctor-version}\n\n\
                  :asciidoctor-version: 3\n:asciidoctor!:\n\n{asciidoctor}{asciidoctor-version}";
    let result = parse_with_options(source, &options);
    let attributes = &result.document.attributes;
    assert_eq!(attributes.get("asciidoctor"), Some(""));
    assert_eq!(
        attributes.get("asciidoctor-version"),
        Some(COMPATIBILITY_VERSION)
    );
    assert_eq!(
        paragraph_text(&result.document.blocks),
        [COMPATIBILITY_VERSION, COMPATIBILITY_VERSION]
    );
}

#[test]
fn conditions_and_nested_documents_see_them() {
    let source = "ifdef::asciidoctor[]\nyes\nendif::[]\n\n\
                  ifeval::[\"{asciidoctor-version}\" >= \"2.0.0\"]\nnew\nendif::[]\n\n\
                  |===\na|{asciidoctor-version}\n|===";
    let document = parse(source).document;
    assert_eq!(paragraph_text(&document.blocks), ["yes", "new"]);
    let BlockKind::Table(table) = &document.blocks[2].kind else {
        panic!("{:?}", document.blocks[2].kind)
    };
    let cell = format!("{:?}", table.body);
    assert!(
        cell.contains(&format!("\"{COMPATIBILITY_VERSION}\"")),
        "{cell}"
    );
}
