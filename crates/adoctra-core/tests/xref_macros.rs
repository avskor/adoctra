//! Cross-references recognized by the macros stage (Ruby's `InlineXrefMacroRx`)
//! after quotes, attribute references, replacements, links and anchors.
use adoctra_core::{
    parse, tokens, AttributeFormat as F, AttributePart as P, Block, BlockKind, Inline, Severity,
    TokenKind, Xref,
};

const SECTION: &str = "[[sec]]\n== Sec Title\n\n";

#[test]
fn attribute_snapshots_keep_definition_time_xref_labels_and_use_spans() {
    for newline in ["\n", "\r\n"] {
        let source = ":early: pass:m[xref:s[]]\n\n[[s]]\n== Target *Ж*\n\n:late: pass:m[xref:s[]]\n\n{early} {late}".replace('\n', newline);
        let result = parse(&source);
        let doc = &result.document;
        assert_eq!(doc.attributes.get("early"), Some("[s]"));
        assert_eq!(doc.final_attributes.get("early"), Some("[s]"));
        assert_eq!(doc.final_attributes.get("late"), Some("Target Ж"));
        assert_eq!(doc.attributes.get("late"), None);
        let parts = doc.final_attributes.raw_parts("late").unwrap();
        assert!(matches!(&parts[0], P::OpenXref(xref) if xref.target == "#s"));
        assert!(parts.contains(&P::Open(F::Strong)));
        assert!(matches!(parts.last(), Some(P::CloseXref)));
        let uses = paragraph(doc.blocks.last().unwrap());
        assert_eq!(text(uses), "[s] Target Ж");
        let spans: Vec<_> = uses
            .iter()
            .filter(|node| matches!(node, Inline::AttributeRef { .. }))
            .map(|node| slice(&source, node))
            .collect();
        assert_eq!(spans, ["{early}", "{late}"]);
    }
}

#[test]
fn resolving_snapshot_xrefs_does_not_repeat_title_counters() {
    let source = ":n: 0\n\n[[s]]\n== Title {counter:n}\n\n:r: pass:m[xref:s[]]\n\n{r} {r}";
    let doc = parse(source).document;
    assert_eq!(doc.final_attributes.get("r"), Some("Title 1"));
    // The title was cached while reading; conversion restores the header
    // counter value, as Ruby does. Snapshot completion must not run it again.
    assert_eq!(doc.final_attributes.get("n"), Some("0"));
    assert_eq!(
        text(paragraph(doc.blocks.last().unwrap())),
        "Title 1 Title 1"
    );
}

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { name, value, .. } => {
                value.clone().unwrap_or_else(|| format!("{{{name}}}"))
            }
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::CrossReference { children, .. } => text(children),
            Inline::Anchor { .. } => String::new(),
            _ => panic!("{node:?}"),
        })
        .collect()
}

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

fn xrefs(nodes: &[Inline]) -> Vec<(&str, Option<&str>, String)> {
    nodes
        .iter()
        .filter_map(|n| match n {
            Inline::CrossReference { xref, children, .. } => Some((
                xref.refid.as_deref().unwrap_or_default(),
                xref.role.as_deref(),
                text(children),
            )),
            _ => None,
        })
        .collect()
}

fn slice(source: &str, node: &Inline) -> String {
    let span = node.span();
    source[span.start..span.end].to_owned()
}

#[test]
fn expanded_reference_names_the_target_and_stays_an_empty_ref() {
    let source = format!(":id: sec\n\n{SECTION}C <<{{id}}>> D xref:{{id}}[] E");
    let nodes = last(&source);
    assert_eq!(
        xrefs(&nodes),
        [
            ("sec", None, "Sec Title".into()),
            ("sec", None, "Sec Title".into())
        ]
    );
    let spans: Vec<_> = nodes
        .iter()
        .filter(|n| matches!(n, Inline::CrossReference { .. }))
        .map(|n| slice(&source, n))
        .collect();
    assert_eq!(spans, ["<<{id}>>", "xref:{id}[]"]);
    // The consumed reference keeps its physical token for the editor.
    let refs: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef { value, .. } => Some((value.clone(), slice(&source, n))),
            _ => None,
        })
        .collect();
    let consumed = (Some(String::new()), "{id}".to_owned());
    assert_eq!(refs, [consumed.clone(), consumed]);
}

#[test]
fn value_generated_references_stay_parts_and_take_labels_at_conversion() {
    let source = format!(":r: <<sec>>\n:m: xref:sec[Text]\n\n{SECTION}A {{r}} B {{m}} C");
    let nodes = last(&source);
    let parts: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef { parts, .. } => Some((parts.clone(), slice(&source, n))),
            _ => None,
        })
        .collect();
    let open = P::OpenXref(Box::new(Xref::to_id("sec")));
    assert_eq!(
        parts,
        [
            (
                vec![open.clone(), P::Text("Sec Title".into()), P::CloseXref],
                "{r}".into()
            ),
            (
                vec![open, P::Text("Text".into()), P::CloseXref],
                "{m}".into()
            ),
        ]
    );
}

#[test]
fn value_generated_forward_references_use_reftext_and_titles() {
    let nodes = last(":r: <<later>>\n:t: <<blk>>\n\nBefore {r} {t}.\n\n[[later,Later Ref]]\n== Later\n\n[#blk]\n.Block *Title*\n====\nx\n====\n\nAfter {r} {t}.");
    let labels: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef { parts, .. } => Some(parts.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(labels[0][1], P::Text("Later Ref".into()));
    assert_eq!(
        labels[1][1..5],
        [
            P::Text("Block ".into()),
            P::Open(F::Strong),
            P::Text("Title".into()),
            P::Close(F::Strong)
        ]
    );
}

#[test]
fn missing_value_target_warns_at_the_reference() {
    let source = ":r: <<missing>>\n\nA {r} B";
    let result = parse(source);
    let nodes = paragraph(&result.document.blocks[0]);
    assert_eq!(text(nodes), "A [missing] B");
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| (d.message.as_str(), &source[d.span.start..d.span.end]))
        .collect();
    assert_eq!(warnings, [("unresolved cross reference: 'missing'", "{r}")]);
}

#[test]
fn escaped_references_lose_only_their_backslash() {
    let source = format!("{SECTION}G \\<<sec>> and \\xref:sec[] H \\<<sec,x>>");
    let nodes = last(&source);
    assert!(xrefs(&nodes).is_empty(), "{nodes:?}");
    assert_eq!(text(&nodes), "G <<sec>> and xref:sec[] H <<sec,x>>");
    let kinds: Vec<_> = tokens(&source).into_iter().map(|t| t.kind).collect();
    assert!(!kinds.contains(&TokenKind::CrossReference));
}

#[test]
fn angle_labels_are_left_stripped_and_blank_labels_are_textless() {
    let source = format!("{SECTION}I <<sec, text >> J <<sec,  >> K <<sec,>> L");
    assert_eq!(
        xrefs(&last(&source)),
        [
            ("sec", None, "text ".into()),
            ("sec", None, "Sec Title".into()),
            ("sec", None, "Sec Title".into())
        ]
    );
    // The refid must start with a word character or one of `#/.:{`.
    let source = format!("{SECTION}T << sec>> U <<>> V <<,x>> W");
    assert!(xrefs(&last(&source)).is_empty());
}

#[test]
fn macro_attribute_lists_supply_label_and_role() {
    let source = format!("{SECTION}K xref:sec[text,role=r] L xref:sec[role=r] M xref:sec[a=b]");
    assert_eq!(
        xrefs(&last(&source)),
        [
            ("sec", Some("r"), "text".into()),
            ("sec", Some("r"), "Sec Title".into()),
            // Only named attributes: the reference stays textless.
            ("sec", None, "Sec Title".into())
        ]
    );
}

#[test]
fn leading_hash_names_a_fragment_except_in_compat_mode() {
    let source = format!("{SECTION}N <<#sec>> O xref:#sec[] P");
    assert_eq!(
        xrefs(&last(&source)),
        [
            ("sec", None, "Sec Title".into()),
            ("sec", None, "Sec Title".into())
        ]
    );
    let source = format!(":compat-mode:\n\n{SECTION}N <<#sec>> O");
    assert_eq!(xrefs(&last(&source)), [("#sec", None, "[#sec]".into())]);
}

#[test]
fn consumed_label_keeps_semantic_parts_and_its_physical_range() {
    // A value is not quoted again; plain label text keeps a Text child.
    let source = format!(":lab: *Label*\n\n{SECTION}E <<sec,{{lab}} more>> F");
    let nodes = last(&source);
    let Some(Inline::CrossReference { children, .. }) = nodes.get(1) else {
        panic!("{nodes:?}")
    };
    let [label @ Inline::Text { .. }] = children.as_slice() else {
        panic!("{children:?}")
    };
    assert_eq!(slice(&source, label), "{lab} more");
    assert_eq!(text(children), "*Label* more");
    let source = format!("{SECTION}E <<sec, _a_ {{lab}}>> F");
    let nodes = last(&source);
    let Some(Inline::CrossReference { children, .. }) = nodes.get(1) else {
        panic!("{nodes:?}")
    };
    let [label @ Inline::Substituted { parts, .. }] = children.as_slice() else {
        panic!("{children:?}")
    };
    assert_eq!(slice(&source, label), "_a_ {lab}");
    assert_eq!(parts[0], P::Open(F::Emphasis));
}

#[test]
fn a_quote_crossing_the_closer_keeps_ruby_nesting() {
    // Quotes run first: the label consumes the opening boundary only.
    let source = format!("{SECTION}P <<sec,*a>> b* Q");
    let nodes = last(&source);
    let Some(Inline::CrossReference { children, .. }) = nodes.get(1) else {
        panic!("{nodes:?}")
    };
    let [Inline::Substituted { parts, .. }] = children.as_slice() else {
        panic!("{children:?}")
    };
    assert_eq!(parts, &[P::Open(F::Strong), P::Text("a".into())]);
    assert!(nodes.iter().any(|n| matches!(n,
        Inline::Substituted { parts, .. } if parts == &[P::Close(F::Strong)])));
}

#[test]
fn macro_labels_unescape_brackets_and_accept_multibyte_text() {
    let source = format!("{SECTION}R xref:sec[a\\]b] S xref:sec[Я] T xref:sec[+a]b+]");
    assert_eq!(
        xrefs(&last(&source)),
        [
            ("sec", None, "a]b".into()),
            ("sec", None, "Я".into()),
            ("sec", None, "a]b".into())
        ]
    );
}

#[test]
fn closers_and_openers_from_values_complete_references() {
    let source = format!(":c: >>\n:o: <<\n\n{SECTION}A <<sec{{c}} B {{o}}sec>> C");
    let nodes = last(&source);
    // The first starts in source text; the second inside the value.
    assert_eq!(xrefs(&nodes), [("sec", None, "Sec Title".into())]);
    assert!(nodes.iter().any(|n| matches!(n,
        Inline::AttributeRef { name, parts, .. } if name == "o"
            && parts.first() == Some(&P::OpenXref(Box::new(Xref::to_id("sec")))))));
}

#[test]
fn headings_resolve_value_generated_references_while_reading() {
    let source = format!(":r: <<sec>>\n\n{SECTION}== Other {{r}}\n\n<<_other_sec_title>>");
    let doc = parse(&source).document;
    let ids: Vec<_> = doc
        .flat_blocks()
        .iter()
        .filter_map(|b| b.id.clone())
        .collect();
    assert_eq!(ids, ["sec", "_other_sec_title"]);
    assert_eq!(
        xrefs(&last(&source)),
        [("_other_sec_title", None, "Other Sec Title".into())]
    );
}

#[test]
fn reference_tokens_cover_the_source_construct() {
    let source = ":id: sec\n\nC <<{id}>> D xref:sec[*b*] E";
    let toks = tokens(source);
    let xrefs: Vec<_> = toks
        .iter()
        .filter(|t| t.kind == TokenKind::CrossReference)
        .map(|t| &source[t.span.start..t.span.end])
        .collect();
    assert_eq!(xrefs, ["<<{id}>>", "xref:sec[*b*]"]);
    // Consumed inner syntax keeps its own tokens.
    assert!(toks
        .iter()
        .any(|t| t.kind == TokenKind::AttributeRef && &source[t.span.start..t.span.end] == "{id}"));
    assert!(toks
        .iter()
        .any(|t| t.kind == TokenKind::Strong && &source[t.span.start..t.span.end] == "*b*"));
}

#[test]
fn incomplete_references_stay_text_without_panicking() {
    let inputs = [
        "<<",
        "<<Я",
        "<<a,Я",
        "<<a,",
        "xref:",
        "xref:Я[",
        "xref:a[Я",
        "xref:a[\\",
        "xref:a[\\]",
        "<<{",
        "\\",
        "\\<<",
        "Я\\xref:a[",
        "xref:[a]",
        "<<a>",
        "xref:a[]]",
        "<<a\n,b>",
    ];
    for input in inputs {
        let source = format!("{SECTION}{input}");
        let result = parse(&source);
        let nodes = paragraph(result.document.blocks.last().unwrap());
        let expected = match input {
            "xref:a[]]" => vec![("a", None, "[a]".into())],
            _ => Vec::new(),
        };
        assert_eq!(xrefs(nodes), expected, "{input:?}");
        tokens(&source);
    }
}

#[test]
fn passthrough_macros_stage_creates_references_in_substituted_parts() {
    let source = format!("{SECTION}A pass:c,m[<<sec>>] B pass:c,q,m[<<sec,*b*>>] C pass:c,m[<<nope>>] D pass:m[xref:sec[x\\]]");
    let result = parse(&source);
    let nodes = paragraph(result.document.blocks.last().unwrap()).to_vec();
    let substituted: Vec<_> = nodes
        .iter()
        .filter_map(|n| match n {
            Inline::Substituted { parts, .. } => Some((parts.clone(), slice(&source, n))),
            _ => None,
        })
        .collect();
    let open = P::OpenXref(Box::new(Xref::to_id("sec")));
    assert_eq!(
        substituted,
        [
            (
                vec![open.clone(), P::Text("Sec Title".into()), P::CloseXref],
                "pass:c,m[<<sec>>]".into()
            ),
            (
                vec![
                    open.clone(),
                    P::Open(F::Strong),
                    P::Text("b".into()),
                    P::Close(F::Strong),
                    P::CloseXref
                ],
                "pass:c,q,m[<<sec,*b*>>]".into()
            ),
            (
                vec![
                    P::OpenXref(Box::new(Xref::to_id("nope"))),
                    P::Text("[nope]".into()),
                    P::CloseXref
                ],
                "pass:c,m[<<nope>>]".into()
            ),
            (
                // Without c the label stays raw passthrough text.
                vec![open, P::Raw("x".into()), P::CloseXref],
                "pass:m[xref:sec[x\\]]".into()
            ),
        ]
    );
    // The unresolved warning points at the enclosing passthrough.
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| &source[d.span.start..d.span.end])
        .collect();
    assert_eq!(warnings, ["pass:c,m[<<nope>>]"]);
    // Generated syntax adds no xref token; the passthrough keeps its own.
    assert!(!tokens(&source)
        .iter()
        .any(|t| t.kind == TokenKind::CrossReference));
}

#[test]
fn passthrough_references_resolve_natural_titles_and_heading_labels() {
    let source = format!(
        "{SECTION}== See pass:c,m[<<Sec Title>>]\n\n<<_see_sec_title>> pass:c,m[<<other.adoc#f>>]"
    );
    let doc = parse(&source).document;
    let ids: Vec<_> = doc
        .flat_blocks()
        .iter()
        .filter_map(|b| b.id.clone())
        .collect();
    assert_eq!(ids, ["sec", "_see_sec_title"]);
    let nodes = last(&source);
    assert_eq!(
        xrefs(&nodes),
        [("_see_sec_title", None, "See Sec Title".into())]
    );
    let interdoc = nodes
        .iter()
        .find_map(|n| match n {
            Inline::Substituted { parts, .. } => match parts.first() {
                Some(P::OpenXref(xref)) => Some(xref.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert_eq!(interdoc.target, "other.html#f");
    assert_eq!(interdoc.path.as_deref(), Some("other.html"));
}

/// The attribute references of a paragraph: name, generated parts and the
/// physical source of each reference.
fn attribute_refs(source: &str, nodes: &[Inline]) -> Vec<(String, Vec<P>, String)> {
    nodes
        .iter()
        .filter_map(|n| match n {
            Inline::AttributeRef { name, parts, .. } => {
                Some((name.clone(), parts.clone(), slice(source, n)))
            }
            _ => None,
        })
        .collect()
}

/// Ruby converts a `pass:m` reference in an attribute definition while
/// defining it: natural lookup, label and xrefstyle use the ids registered
/// before the definition and the attributes at that point.
#[test]
fn definition_references_fix_target_and_label_at_the_definition() {
    let source = ":r: pass:c,m[<<sec>>]\n:n: pass:c,m[<<Sec Title>>]\n:o: pass:c,m[<<other.adoc#x>>]\n\n\
        [[sec]]\n== Sec Title\n\n:s: pass:c,m[<<sec>>]\n:m: pass:c,m[<<Sec Title>>]\n:t: {r}\n\n\
        [[ex]]\n.My Ex\n====\ny\n====\n\n:xrefstyle: short\n:e: pass:c,m[<<ex>>]\n:xrefstyle: full\n\n\
        A {r} B {s} C {o} D {n} E {m} F {t} G {e}";
    let result = parse(source);
    let nodes = last(source);
    let link = |id: &str, label: &str| {
        vec![
            P::OpenXref(Box::new(Xref::to_id(id))),
            P::Text(label.into()),
            P::CloseXref,
        ]
    };
    let mut refs = attribute_refs(source, &nodes);
    // The interdocument path (the label) misses the suffix pinned at header end.
    let (_, interdoc, _) = refs.remove(2);
    let P::OpenXref(xref) = &interdoc[0] else {
        panic!("{interdoc:?}")
    };
    assert_eq!(
        (
            xref.target.as_str(),
            xref.refid.as_deref(),
            xref.path.as_deref()
        ),
        ("other#x", Some("other#x"), Some("other"))
    );
    assert_eq!(interdoc[1..], [P::Text("other".into()), P::CloseXref]);
    assert_eq!(
        refs,
        [
            ("r".into(), link("sec", "[sec]"), "{r}".into()),
            ("s".into(), link("sec", "Sec Title"), "{s}".into()),
            ("n".into(), link("Sec Title", "[Sec Title]"), "{n}".into()),
            ("m".into(), link("sec", "Sec Title"), "{m}".into()),
            ("t".into(), link("sec", "[sec]"), "{t}".into()),
            ("e".into(), link("ex", "Example 1"), "{e}".into()),
        ]
    );
    // Only the natural reference that failed at its definition is unknown.
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| (d.message.as_str(), &source[d.span.start..d.span.end]))
        .collect();
    assert_eq!(
        warnings,
        [("unresolved cross reference: 'Sec Title'", "{n}")]
    );
    tokens(source);
}

/// A heading converted while reading and a later use both show the label
/// fixed at the definition, not the one the use would compute; an inline
/// anchor joins the partial registry once its paragraph is read.
#[test]
fn definition_references_in_headings_containers_and_after_anchors() {
    let source =
        "= Doc\n\n:r: pass:c,m[<<later>>]\n:a: pass:c,m[<<a1>>]\n\nA [[a1,Anchor One]]x {a}\n\n\
        :b: pass:c,m[<<a1>>]\n\n****\n:inner: pass:c,m[<<a1>>]\n{inner} {b}\n****\n\n\
        [[later]]\n== Later\n\n== See {r} {b}\n\nL {r}";
    let doc = parse(source).document;
    let labels = |nodes: &[Inline]| -> Vec<String> {
        nodes
            .iter()
            .filter_map(|n| match n {
                Inline::AttributeRef { value, .. } => value.clone(),
                _ => None,
            })
            .collect()
    };
    let blocks = doc.flat_blocks();
    let mut paragraphs = Vec::new();
    for block in &blocks {
        match &block.kind {
            BlockKind::Paragraph { inlines } => paragraphs.push(labels(inlines)),
            BlockKind::Sidebar { blocks } => paragraphs.push(labels(paragraph(&blocks[0]))),
            _ => {}
        }
    }
    assert_eq!(
        paragraphs,
        [
            vec!["[a1]".to_owned()],
            vec!["Anchor One".into(), "Anchor One".into()],
            vec!["[later]".into()],
        ]
    );
    let heading = blocks
        .iter()
        .find_map(|b| match &b.kind {
            BlockKind::Heading { inlines, .. }
                if b.id.as_deref() == Some("_see_later_anchor_one") =>
            {
                Some(labels(inlines))
            }
            _ => None,
        })
        .expect("auto id derived from the fixed labels");
    assert_eq!(heading, ["[later]", "Anchor One"]);
}
