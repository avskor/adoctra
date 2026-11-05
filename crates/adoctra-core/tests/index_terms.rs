//! Index terms (Ruby's `InlineIndextermMacroRx`): flow terms `((…))` and
//! `indexterm2:[…]` show their text, concealed terms `(((…)))` and
//! `indexterm:[…]` show nothing. Recognized in the macros stage after images.
use adoctra_core::{
    attributes::parts_text, parse, tokens, AttributeFormat as F, AttributePart as P, Block,
    BlockKind, IndexTerm, Inline, Span, TokenKind,
};

fn paragraph(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } => inlines,
        BlockKind::Section { blocks, .. } => paragraph(blocks.last().unwrap()),
        _ => panic!("{block:?}"),
    }
}

/// The inlines of the last paragraph of a document.
fn last(source: &str) -> Vec<Inline> {
    let doc = parse(source).document;
    paragraph(doc.blocks.last().unwrap()).to_vec()
}

/// Source-owned index terms with their spans.
fn terms(nodes: &[Inline]) -> Vec<(IndexTerm, Span)> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Inline::IndexTerm { term, span } => Some(((**term).clone(), *span)),
            _ => None,
        })
        .collect()
}

fn span(source: &str, construct: &str) -> Span {
    let start = source.find(construct).unwrap();
    Span::new(start, start + construct.len())
}

/// Visible text of a paragraph as rendered: flow terms contribute their
/// text, concealed terms nothing.
fn shown(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. } => value.clone(),
            Inline::IndexTerm { term, .. } => {
                term.text.as_deref().map(parts_text).unwrap_or_default()
            }
            Inline::Substituted { value, .. } => value.clone(),
            Inline::Strong { children, .. } => shown(children),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn flow_and_concealed_terms_with_spans() {
    let source = "The ((Tigers)) and (((Big cats,Lions))) indexterm:[Tigers,Big cats] indexterm2:[Lions] roam.";
    let nodes = last(source);
    let found = terms(&nodes);
    assert_eq!(found.len(), 4, "{nodes:?}");
    assert_eq!(found[0].1, span(source, "((Tigers))"));
    assert_eq!(
        found[0].0.text.as_deref().map(parts_text).as_deref(),
        Some("Tigers")
    );
    assert!(found[0].0.terms.is_empty());
    assert_eq!(found[1].1, span(source, "(((Big cats,Lions)))"));
    assert_eq!(found[1].0.text, None);
    assert_eq!(found[1].0.terms, ["Big cats", "Lions"]);
    assert_eq!(found[2].1, span(source, "indexterm:[Tigers,Big cats]"));
    assert_eq!(found[2].0.terms, ["Tigers", "Big cats"]);
    assert_eq!(found[3].1, span(source, "indexterm2:[Lions]"));
    assert_eq!(found[3].0.text, Some(vec![P::Text("Lions".into())]));
    assert_eq!(shown(&nodes), "The Tigers and   Lions roam.");
}

#[test]
fn see_and_see_also_targets() {
    let nodes = last("((Tigers >> Big cats)) (((a, b >> c))) ((Tigers &> Cats &> Lions)) indexterm:[x,y,z,w,see-also=\"p, q\"] indexterm2:[t,see-also=u] indexterm2:[term=x]");
    let found = terms(&nodes);
    let flow = |i: usize| found[i].0.text.as_deref().map(parts_text);
    assert_eq!(flow(0).as_deref(), Some("Tigers"));
    assert_eq!(found[0].0.see.as_deref(), Some("Big cats"));
    assert_eq!(found[1].0.terms, ["a", "b"]);
    assert_eq!(found[1].0.see.as_deref(), Some("c"));
    assert_eq!(flow(2).as_deref(), Some("Tigers"));
    assert_eq!(found[2].0.see_also, ["Cats", "Lions"]);
    // An attribute list keeps three positional terms.
    assert_eq!(found[3].0.terms, ["x", "y", "z"]);
    assert_eq!(found[3].0.see_also, ["p", "q"]);
    assert_eq!(flow(4).as_deref(), Some("t"));
    assert_eq!(found[4].0.see_also, ["u"]);
    // Without a positional term the whole text is the term.
    assert_eq!(flow(5).as_deref(), Some("term=x"));
    // A named field before the term moves it out of the first position.
    let found = terms(&last("indexterm:[see=x,Tigers] indexterm:[A,\"B, C\"]"));
    assert_eq!(found[0].0.terms, ["see=x,Tigers"]);
    assert_eq!(found[1].0.terms, ["A", "B, C"]);
}

#[test]
fn parentheses_escapes_and_whitespace() {
    let source = "a (((x)) b ((y))) c \\((t)) d \\(((u))) e \\indexterm:[v] f ((\nspan\nlines )) g ((a\\]b)) h indexterm2:[a\\]b]";
    let nodes = last(source);
    assert_eq!(
        shown(&nodes),
        "a (x b y) c ((t)) d (u) e indexterm:[v] f span lines g a\\]b h a]b"
    );
    let found = terms(&nodes);
    // The unmatched parenthesis stays outside the term's span.
    assert_eq!(found[0].1, span(source, "(((x))"));
    assert_eq!(found[1].1, span(source, "((y)))"));
    // An escaped concealed term is a flow term between literal parentheses;
    // its span includes the backslash.
    assert_eq!(found[2].1, span(source, "\\(((u)))"));
    assert_eq!(
        found[2].0.text.as_deref().map(parts_text).as_deref(),
        Some("u")
    );
}

#[test]
fn later_macros_see_ruby_text_and_nest_inside_flow_terms() {
    // A concealed term has an empty spelling: the URL continues across it.
    let nodes =
        last("Link https://example.org(((hidden)))/x and before(((x)))https://example.org.");
    let Inline::Substituted { parts, .. } = &nodes[1] else {
        panic!("{nodes:?}")
    };
    assert!(
        matches!(parts.as_slice(), [P::OpenLink(link), P::Text(a), P::IndexTerm(t), P::Text(b), P::CloseLink]
            if link.target == "https://example.org/x" && a == "https://example.org" && b == "/x" && t.terms == ["hidden"]),
        "{parts:?}"
    );
    assert!(!nodes.iter().any(|n| matches!(n, Inline::Substituted { parts, .. } if parts.iter().any(|p| matches!(p, P::OpenLink(l) if l.target == "https://example.org")))), "{nodes:?}");
    // A macro ending at the end of a flow term stays inside it.
    let found = terms(&last("A ((*x* https://example.org)) B"));
    let text = found[0].0.text.clone().unwrap();
    assert!(
        matches!(text.as_slice(), [P::Open(F::Strong), P::Text(x), P::Close(F::Strong), P::Text(_), P::OpenLink(_), P::Text(_), P::CloseLink] if x == "x"),
        "{text:?}"
    );
}

#[test]
fn attribute_values_generate_terms_and_hard_breaks_see_ruby_text() {
    let doc = parse(":t: ((Attr term))\n:c: (((hidden)))\n\nA {t} B {c} C").document;
    let nodes = paragraph(&doc.blocks[0]);
    let parts: Vec<_> = nodes
        .iter()
        .filter_map(|node| match node {
            Inline::AttributeRef { parts, .. } => Some(parts.clone()),
            _ => None,
        })
        .collect();
    assert!(
        matches!(parts[0].as_slice(), [P::IndexTerm(t)] if t.text == Some(vec![P::Text("Attr term".into())])),
        "{parts:?}"
    );
    assert!(
        matches!(parts[1].as_slice(), [P::IndexTerm(t)] if t.terms == ["hidden"]),
        "{parts:?}"
    );
    // The marker before a concealed term still ends the line.
    let nodes = last("Break a +(((x)))\nnext");
    assert!(
        nodes.iter().any(
            |n| matches!(n, Inline::Substituted { parts, .. } if parts.contains(&P::LineBreak))
        ),
        "{nodes:?}"
    );
}

#[test]
fn section_ids_and_xref_labels_use_the_shown_text() {
    let doc = parse("== Section ((Visible)) (((x,y)))\n\nSee <<_section_visible>>.").document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[0].kind else {
        panic!()
    };
    assert_eq!(blocks[0].id.as_deref(), Some("_section_visible"));
    let nodes = paragraph(blocks.last().unwrap());
    let Inline::CrossReference { children, .. } = &nodes[1] else {
        panic!("{nodes:?}")
    };
    assert_eq!(terms(children).len(), 2, "{children:?}");
}

#[test]
fn passthroughs_protect_terms_except_pass_m() {
    let nodes = last("pass:m[((Passed))] pass:q[((NotM))] +((plain))+");
    assert_eq!(nodes.len(), 5, "{nodes:?}");
    assert!(
        matches!(&nodes[0], Inline::Substituted { parts, .. } if matches!(parts.as_slice(), [P::IndexTerm(t)] if t.text == Some(vec![P::Raw("Passed".into())]))),
        "{nodes:?}"
    );
    assert!(terms(&nodes).is_empty());
}

#[test]
fn terms_are_highlighted_as_inline_macros() {
    let source = "A ((B)) C (((D))) E indexterm:[F]";
    let macros: Vec<Span> = tokens(source)
        .into_iter()
        .filter(|t| t.kind == TokenKind::InlineMacro)
        .map(|t| t.span)
        .collect();
    assert_eq!(
        macros,
        [
            span(source, "((B))"),
            span(source, "(((D)))"),
            span(source, "indexterm:[F]")
        ]
    );
}

#[test]
fn adjacent_and_empty_terms_keep_their_nodes_and_metadata() {
    let found = terms(&last("((A))(((B)))((D))((()))(( ))indexterm2:[ ]"));
    assert_eq!(found.len(), 6);
    assert_eq!(found[1].0.terms, ["B"]);
    assert_eq!(found[3].0.text, None);
    assert!(found[3].0.terms.is_empty());
    assert_eq!(found[4].0.text, Some(vec![]));
    assert_eq!(found[5].0.text, Some(vec![]));
    let found = terms(&last("indexterm:[A,B,,] indexterm:[A,\"B\",,] indexterm:[A,see=B,see-also=\"C, D\"] indexterm2:[X,see=Y]"));
    // Ruby's plain split drops empty tails; its quoted CSV reader keeps them.
    assert_eq!(found[0].0.terms, ["A", "B"]);
    assert_eq!(found[1].0.terms, ["A", "B", "", ""]);
    assert_eq!(found[2].0.see.as_deref(), Some("B"));
    assert_eq!(found[2].0.see_also, ["C", "D"]);
    assert_eq!(found[3].0.see.as_deref(), Some("Y"));
}

#[test]
fn unicode_crlf_and_incomplete_input_keep_physical_spans() {
    let source = "Ж ((Лев\r\nи тигр)) (((Кошки)))";
    let found = terms(&last(source));
    assert_eq!(found[0].1, span(source, "((Лев\r\nи тигр))"));
    assert_eq!(
        found[0].0.text.as_deref().map(parts_text).as_deref(),
        Some("Лев и тигр")
    );
    assert_eq!(found[1].1, span(source, "(((Кошки)))"));
    // Every partial prefix is valid editor input, including an escape at EOF.
    for (end, _) in source
        .char_indices()
        .chain(std::iter::once((source.len(), ' ')))
    {
        parse(&source[..end]);
        tokens(&source[..end]);
    }
    for source in [
        "((",
        "(((",
        "((Ж)",
        "indexterm:[",
        "indexterm2:[Ж\\]",
        "\\",
        "((Ж))\\",
    ] {
        parse(source);
        tokens(source);
    }
}

#[test]
fn nested_references_footnotes_and_passthroughs_are_restored() {
    let doc = parse("[[t]]Target.\n\n((<<t>>)) ((footnote:[Note])) ((+<raw>+))").document;
    let found = terms(paragraph(&doc.blocks[1]));
    assert!(
        matches!(found[0].0.text.as_deref(), Some([P::OpenXref(x), P::Text(label), P::CloseXref]) if x.target == "#t" && label == "[t]")
    );
    assert!(matches!(found[1].0.text.as_deref(), Some([P::Footnote(_)])));
    assert_eq!(doc.footnotes.len(), 1);
    assert_eq!(parts_text(&doc.footnotes[0].content), "Note");
    assert_eq!(found[2].0.text, Some(vec![P::Text("<raw>".into())]));
}

#[test]
fn single_quoted_fields_repeat_normal_substitutions() {
    let found = terms(&last("indexterm2:['*A*',see='<B>',see-also='C, D'] indexterm:['<E>',F,see='G'] indexterm2:['((Inner))',see=X]"));
    assert_eq!(
        found[0].0.text,
        Some(vec![P::Text("<strong>A</strong>".into())])
    );
    assert_eq!(found[0].0.see.as_deref(), Some("&lt;B&gt;"));
    assert_eq!(found[0].0.see_also, ["C", "D"]);
    assert_eq!(found[1].0.terms, ["&lt;E&gt;", "F"]);
    assert_eq!(found[1].0.see.as_deref(), Some("G"));
    assert!(
        matches!(found[2].0.text.as_deref(), Some([P::IndexTerm(inner)]) if inner.text == Some(vec![P::Text("Inner".into())]))
    );
}
