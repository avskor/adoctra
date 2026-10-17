//! The experimental keyboard, button and menu macros (Ruby's
//! `InlineKbdBtnMacroRx`, `InlineMenuMacroRx` and `InlineMenuRx`), the first
//! macros of the macros stage, recognized only with `experimental`.
use adoctra_core::{
    attributes::parts_text, parse, tokens, AttributeFormat as F, AttributePart as P, Block,
    BlockKind, Inline, Span, TokenKind, UiMacro,
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

/// Source-owned macros with their spans.
fn macros(nodes: &[Inline]) -> Vec<(UiMacro, Span)> {
    let mut found = Vec::new();
    for node in nodes {
        match node {
            Inline::Ui { ui, span } => found.push((ui.clone(), *span)),
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                found.extend(macros(children))
            }
            _ => {}
        }
    }
    found
}

fn texts(labels: &[Vec<P>]) -> Vec<String> {
    labels.iter().map(|label| parts_text(label)).collect()
}

fn keys(ui: &UiMacro) -> Vec<String> {
    match ui {
        UiMacro::Keyboard { keys } => texts(keys),
        _ => panic!("{ui:?}"),
    }
}

/// `(menu, submenus, item)` as flat text.
fn menu(ui: &UiMacro) -> (String, Vec<String>, Option<String>) {
    match ui {
        UiMacro::Menu {
            menu,
            submenus,
            item,
            ..
        } => (
            parts_text(menu),
            texts(submenus),
            item.as_deref().map(parts_text),
        ),
        _ => panic!("{ui:?}"),
    }
}

fn flat(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => value.clone(),
            Inline::Ui { .. } => "#".into(),
            node => panic!("{node:?}"),
        })
        .collect()
}

#[test]
fn keyboard_keys_split_at_the_first_comma_or_plus_with_spans() {
    // Another `+` after `kbd:[Ctrl++]` would close a `++` passthrough.
    let source = ":experimental:\n\nA kbd:[Ctrl+T] kbd:[ Ctrl , Shift + T ] kbd:[Ctrl,,] \
                  kbd:[a,b+c] kbd:[\\]] kbd:[*Ctrl*+<] kbd:[,] b";
    let nodes = last(source);
    let found = macros(&nodes);
    let start = source.find("kbd:").unwrap();
    assert_eq!(found[0].1, Span::new(start, start + "kbd:[Ctrl+T]".len()));
    let all: Vec<_> = found.iter().map(|(ui, _)| keys(ui)).collect();
    assert_eq!(
        all,
        [
            vec!["Ctrl", "T"],
            vec!["Ctrl", "Shift + T"],
            // A trailing delimiter is itself the last key.
            vec!["Ctrl", ","],
            vec!["a", "b+c"],
            vec!["]"],
            vec!["Ctrl", "<"],
            vec![","],
        ]
    );
    // Keys keep the earlier stages' formatting boundaries.
    let UiMacro::Keyboard { keys: strong } = &found[5].0 else {
        unreachable!()
    };
    assert_eq!(
        strong[0],
        [
            P::Open(F::Strong),
            P::Text("Ctrl".into()),
            P::Close(F::Strong)
        ]
    );
    assert!(matches!(&nodes[0], Inline::Text { value, .. } if value == "A "));
    assert!(matches!(nodes.last(), Some(Inline::Text { value, .. }) if value == " b"));
    let found = macros(&last(":experimental:\n\nkbd:[Ctrl++]"));
    assert_eq!(keys(&found[0].0), ["Ctrl", "+"]);
}

#[test]
fn buttons_normalize_whitespace_and_escapes_keep_text() {
    let nodes = last(
        ":experimental:\n\nbtn:[Save] btn:[  Multi\nline ] btn:[a\\]b] \\kbd:[x] \\btn:[y] kbd:[]",
    );
    let labels: Vec<_> = macros(&nodes)
        .into_iter()
        .map(|(ui, _)| match ui {
            UiMacro::Button { label } => parts_text(&label),
            ui => panic!("{ui:?}"),
        })
        .collect();
    assert_eq!(labels, ["Save", "Multi line", "a]b"]);
    assert_eq!(flat(&nodes), "# # # kbd:[x] btn:[y] kbd:[]");
}

#[test]
fn menu_macros_split_items_at_angle_brackets_or_commas() {
    let nodes = last(
        ":experimental:\n\nmenu:File[Save] menu:View[Zoom > Reset] menu:A[B,C,D] menu:Tools[] \
         menu:X[ ] menu:F[Edit,] menu:G[,] menu:a b[c] menu:&amp;[a] menu:&[y] \\menu:File[Save] \
         menu:x [y] menu:1 [y] menu:\u{2192}[y]",
    );
    let found: Vec<_> = macros(&nodes).iter().map(|(ui, _)| menu(ui)).collect();
    let item = |s: &str| Some(s.to_owned());
    assert_eq!(
        found,
        [
            ("File".into(), vec![], item("Save")),
            ("View".into(), vec!["Zoom".into()], item("Reset")),
            ("A".into(), vec!["B".into(), "C".into()], item("D")),
            ("Tools".into(), vec![], None),
            ("X".into(), vec![], None),
            // Ruby's split drops trailing empty items.
            ("F".into(), vec![], item("Edit")),
            ("G".into(), vec![], None),
            ("a b".into(), vec![], item("c")),
            // A source entity stays raw; a lone `&` is already `&amp;` here.
            ("&amp;".into(), vec![], item("a")),
            ("&".into(), vec![], item("y")),
        ]
    );
    let tail = flat(&nodes);
    assert!(
        tail.ends_with("# menu:File[Save] menu:x [y] menu:1 [y] menu:\u{2192}[y]"),
        "{tail}"
    );
}

#[test]
fn menu_shorthand_needs_a_spaced_angle_bracket() {
    let nodes = last(
        ":experimental:\n\n\"File > Save\" \"View >\nZoom > Reset\" \\\"A > B\" \"nope>here\" \" a > b\"",
    );
    let found: Vec<_> = macros(&nodes).iter().map(|(ui, _)| menu(ui)).collect();
    assert_eq!(
        found,
        [
            ("File".into(), vec![], Some("Save".into())),
            ("View".into(), vec!["Zoom".into()], Some("Reset".into())),
        ]
    );
    assert_eq!(flat(&nodes), "# # \"A > B\" \"nope>here\" \" a > b\"");
}

#[test]
fn macros_need_experimental_at_the_point_of_use() {
    let doc = parse(
        "kbd:[a] menu:A[B]\n\n:experimental:\n\nkbd:[a] menu:A[B]\n\n:experimental!:\n\nkbd:[a] \"A > B\"",
    )
    .document;
    let found: Vec<_> = doc
        .flat_blocks()
        .into_iter()
        .map(|block| macros(paragraph(block)).len())
        .collect();
    assert_eq!(found, [0, 2, 0]);
}

#[test]
fn the_caret_follows_font_icons_at_the_macro() {
    let doc = parse(":experimental:\n\nmenu:A[B]\n\n:icons: font\n\n\"A > B\"").document;
    let fonts: Vec<_> = doc
        .flat_blocks()
        .into_iter()
        .map(|block| match &macros(paragraph(block))[0].0 {
            UiMacro::Menu { font, .. } => *font,
            ui => panic!("{ui:?}"),
        })
        .collect();
    assert_eq!(fonts, [false, true]);
}

#[test]
fn attribute_values_references_and_passthroughs() {
    let doc = parse(
        ":experimental:\n:k: kbd:[Alt+F4]\n:key: Ctrl\n\n{k} kbd:[{key}+X] menu:{key}[Go]\n\nkbd:[+a+b+]",
    )
    .document;
    let nodes = paragraph(&doc.blocks[0]);
    // A macro generated wholly by an attribute value stays a part of it.
    let parts = nodes
        .iter()
        .find_map(|n| match n {
            Inline::AttributeRef { name, parts, .. } if name == "k" => Some(parts.clone()),
            _ => None,
        })
        .unwrap();
    let [P::Ui(ui)] = parts.as_slice() else {
        panic!("{parts:?}");
    };
    assert_eq!(keys(ui), ["Alt", "F4"]);
    let found = macros(nodes);
    assert_eq!(keys(&found[0].0), ["Ctrl", "X"]);
    assert_eq!(menu(&found[1].0).0, "Ctrl");
    // The protected passthrough is one key, restored after the macro.
    let found = macros(paragraph(&doc.blocks[1]));
    assert_eq!(keys(&found[0].0), ["a+b"]);
}

#[test]
fn macros_in_footnotes_section_ids_and_xref_labels() {
    let doc = parse(
        ":experimental:\n\n== Press kbd:[Ctrl+S] and menu:File[Save]\n\n<<_press_ctrls_and_file_save>> footnote:[Use btn:[OK]]",
    )
    .document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    // Ruby's id drops the generated tags and the caret's entities.
    assert_eq!(blocks[0].id.as_deref(), Some("_press_ctrls_and_file_save"));
    let label = paragraph(&blocks[1]);
    let found = macros(&label[..1]);
    assert_eq!(keys(&found[0].0), ["Ctrl", "S"]);
    assert_eq!(menu(&found[1].0).0, "File");
    let content = &doc.footnotes[0].content;
    assert!(
        matches!(&content[1], P::Ui(ui) if matches!(ui.as_ref(), UiMacro::Button { label } if parts_text(label) == "OK")),
        "{content:?}"
    );
}

#[test]
fn macros_are_inline_macro_tokens() {
    let source = ":experimental:\n\nA kbd:[*b*] btn:[x] \\kbd:[y] \"F > S\"";
    let base = source.find("A ").unwrap();
    let found: Vec<_> = tokens(source)
        .into_iter()
        .filter(|t| t.kind == TokenKind::InlineMacro)
        .map(|t| t.span)
        .collect();
    assert_eq!(
        found,
        [
            Span::new(base + 2, base + 11),
            Span::new(base + 12, base + 19),
            Span::new(base + 29, base + 36),
        ]
    );
}

/// The label of a button macro.
fn label(ui: &UiMacro) -> &[P] {
    match ui {
        UiMacro::Button { label } => label,
        _ => panic!("{ui:?}"),
    }
}

/// `[OpenLink(target), text…, CloseLink]` as `(target, text)`.
fn link(parts: &[P]) -> (String, String) {
    let [P::OpenLink(open), inner @ .., P::CloseLink] = parts else {
        panic!("{parts:?}");
    };
    (open.target.clone(), parts_text(inner))
}

#[test]
fn macros_nested_in_labels_stay_inside_them() {
    let nodes = last(
        ":experimental:\n\nbtn:[link:x[y]] btn:[link:x[*b*] z] kbd:[Ctrl+link:k[a+b]] \
         menu:A[link:m[x,y] > Z] kbd:[https://example.com]",
    );
    let found = macros(&nodes);
    // The content runs to the bracket balancing the nested macro.
    assert_eq!(link(label(&found[0].0)), ("x".into(), "y".into()));
    let formatted = label(&found[1].0);
    assert!(
        matches!(formatted, [P::OpenLink(_), P::Open(F::Strong), P::Text(b), P::Close(F::Strong), P::CloseLink, P::Text(z)] if b == "b" && z == " z"),
        "{formatted:?}"
    );
    // Delimiters inside a nested macro do not split keys or menu items.
    let UiMacro::Keyboard { keys } = &found[2].0 else {
        panic!("{:?}", found[2].0);
    };
    assert_eq!(parts_text(&keys[0]), "Ctrl");
    assert_eq!(link(&keys[1]), ("k".into(), "a+b".into()));
    let UiMacro::Menu { submenus, item, .. } = &found[3].0 else {
        panic!("{:?}", found[3].0);
    };
    assert_eq!(link(&submenus[0]), ("m".into(), "x,y".into()));
    assert_eq!(item.as_deref().map(parts_text).as_deref(), Some("Z"));
    let UiMacro::Keyboard { keys } = &found[4].0 else {
        panic!("{:?}", found[4].0);
    };
    assert_eq!(
        link(&keys[0]),
        ("https://example.com".into(), "https://example.com".into())
    );
    // Each macro keeps one node over its whole source.
    let source = ":experimental:\n\nbtn:[link:x[y]] z";
    let found = macros(&last(source));
    let start = source.find("btn:").unwrap();
    assert_eq!(
        found[0].1,
        Span::new(start, start + "btn:[link:x[y]]".len())
    );
}

#[test]
fn cross_references_and_footnotes_inside_labels() {
    let doc = parse(
        ":experimental:\n\n[[t]]Target.\n\nkbd:[<<t>>] menu:Help[<<t,About>>] btn:[OK footnote:[Press it.]]",
    )
    .document;
    let found = macros(paragraph(&doc.blocks[1]));
    // A textless reference gets its label inside the key.
    let UiMacro::Keyboard { keys } = &found[0].0 else {
        panic!("{:?}", found[0].0);
    };
    assert!(
        matches!(keys[0].as_slice(), [P::OpenXref(x), P::Text(label), P::CloseXref] if x.target == "#t" && label == "[t]"),
        "{keys:?}"
    );
    // A `<<id,label>>` keeps its comma inside one menu item.
    let (menu, submenus, item) = menu(&found[1].0);
    assert_eq!((menu.as_str(), submenus.len()), ("Help", 0));
    assert_eq!(item.as_deref(), Some("About"));
    // A footnote inside a button is listed after the document.
    assert!(matches!(label(&found[2].0), [P::Text(ok), P::Footnote(_)] if ok == "OK "));
    assert_eq!(doc.footnotes.len(), 1);
    assert_eq!(parts_text(&doc.footnotes[0].content), "Press it.");
}

#[test]
fn formatting_crossing_the_markup_is_split_into_labels() {
    let nodes = last(":experimental:\n\nkbd:[*a+b*] *x kbd:[a*] end");
    let found = macros(&nodes);
    let strong = |text: &str| {
        vec![
            P::Open(F::Strong),
            P::Text(text.into()),
            P::Close(F::Strong),
        ]
    };
    let UiMacro::Keyboard { keys } = &found[0].0 else {
        panic!("{:?}", found[0].0);
    };
    assert_eq!(keys, &[strong("a"), strong("b")]);
    // Formatting opened before the macro closes before it.
    let UiMacro::Keyboard { keys } = &found[1].0 else {
        panic!("{:?}", found[1].0);
    };
    assert_eq!(keys, &[strong("a")]);
    assert!(
        nodes.iter().any(|node| matches!(node, Inline::Strong { children, .. } if matches!(children.as_slice(), [Inline::Text { value, .. }] if value == "x "))),
        "{nodes:?}"
    );
}

#[test]
fn an_unsplittable_crossing_keeps_ruby_markup() {
    // The link's label ends inside the key: Ruby's misnested markup stays.
    let nodes = last(":experimental:\n\nlink:u[a kbd:[b\\]c] z");
    assert!(macros(&nodes).is_empty());
    let raw: Vec<_> = nodes
        .iter()
        .flat_map(|node| match node {
            Inline::Substituted { parts, .. } => parts.clone(),
            _ => Vec::new(),
        })
        .filter_map(|part| match part {
            P::Raw(raw) => Some(raw),
            _ => None,
        })
        .collect();
    assert!(raw.iter().any(|raw| raw == "<kbd>"), "{nodes:?}");
}

#[test]
fn borrowed_xref_labels_drop_links_inside_labels() {
    let doc = parse(":experimental:\n\n== See kbd:[link:k[K]]\n\n<<_see_k>>").document;
    let BlockKind::Section { blocks, .. } = &doc.blocks[0].kind else {
        panic!("{:?}", doc.blocks[0].kind);
    };
    assert_eq!(blocks[0].id.as_deref(), Some("_see_k"));
    let title = macros(match &blocks[0].kind {
        BlockKind::Heading { inlines, .. } => inlines,
        kind => panic!("{kind:?}"),
    });
    let UiMacro::Keyboard { keys } = &title[0].0 else {
        panic!("{:?}", title[0].0);
    };
    assert_eq!(link(&keys[0]), ("k".into(), "K".into()));
    let found = macros(&paragraph(&blocks[1])[..1]);
    let UiMacro::Keyboard { keys } = &found[0].0 else {
        panic!("{:?}", found[0].0);
    };
    assert_eq!(keys[0], [P::Text("K".into())]);
}
