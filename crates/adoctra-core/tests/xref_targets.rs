//! Cross-reference targets like Ruby's `InlineXrefMacroRx`: interdocument
//! paths, the current document, included documents and natural (title)
//! references resolved through `Document#resolve_id`.
use adoctra_core::{
    parse, parse_resource, tokens, AttributePart as P, Block, BlockKind, Inline, MemoryResources,
    ParseOptions, Severity, SourceMap, TokenKind, Xref,
};

fn text(nodes: &[Inline]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Inline::Text { value, .. }
            | Inline::Raw { value, .. }
            | Inline::Substituted { value, .. } => value.clone(),
            Inline::AttributeRef { value, .. } => value.clone().unwrap_or_default(),
            Inline::Strong { children, .. } | Inline::CrossReference { children, .. } => {
                text(children)
            }
            _ => String::new(),
        })
        .collect()
}

fn inlines(block: &Block) -> &[Inline] {
    match &block.kind {
        BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => inlines,
        _ => panic!("{block:?}"),
    }
}

/// Every cross-reference of the document's paragraphs and headings.
fn xrefs(blocks: &[&Block]) -> Vec<(Xref, String, adoctra_core::Span)> {
    blocks
        .iter()
        .filter(|b| {
            matches!(
                b.kind,
                BlockKind::Paragraph { .. } | BlockKind::Heading { .. }
            )
        })
        .flat_map(|b| inlines(b).iter())
        .filter_map(|node| match node {
            Inline::CrossReference {
                xref,
                children,
                span,
            } => Some((xref.clone(), text(children), *span)),
            _ => None,
        })
        .collect()
}

fn targets(source: &str) -> Vec<(String, String)> {
    xrefs(&parse(source).document.flat_blocks())
        .into_iter()
        .map(|(xref, label, _)| (xref.target, label))
        .collect()
}

#[test]
fn interdocument_paths_keep_refid_fragment_and_display_the_path() {
    let source = "See <<other.adoc#frag>> and xref:dir/x.y/z#a[].";
    let result = parse(source);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let found = xrefs(&result.document.flat_blocks());
    let (xref, label, span) = &found[0];
    assert_eq!(xref.target, "other.html#frag");
    assert_eq!(xref.refid.as_deref(), Some("other#frag"));
    assert_eq!(xref.path.as_deref(), Some("other.html"));
    assert_eq!(xref.fragment.as_deref(), Some("frag"));
    assert_eq!(label, "other.html");
    assert_eq!(&source[span.start..span.end], "<<other.adoc#frag>>");
    // The borrowed path label sits on the whole construct.
    let Inline::CrossReference { children, .. } = &inlines(&result.document.blocks[0])[1] else {
        panic!()
    };
    assert_eq!(children[0].span(), *span);
    assert_eq!(found[1].0.target, "dir/x.y/z.html#a");
    // The editor still highlights both constructs.
    let xref_tokens: Vec<_> = tokens(source)
        .into_iter()
        .filter(|t| t.kind == TokenKind::CrossReference)
        .map(|t| &source[t.span.start..t.span.end])
        .collect();
    assert_eq!(xref_tokens, ["<<other.adoc#frag>>", "xref:dir/x.y/z#a[]"]);
}

#[test]
fn macro_and_angle_forms_classify_paths_like_ruby() {
    assert_eq!(
        targets("<<other.md#x>> xref:other.md#x[] xref:other.html[] xref:other[] <<other.adoc>>"),
        [
            ("other.md.html#x".into(), "other.md.html".into()),
            ("other.md#x".into(), "other.md".into()),
            ("other.html".into(), "other.html".into()),
            ("#other".into(), "[other]".into()),
            ("#other.adoc".into(), "[other.adoc]".into()),
        ]
    );
}

#[test]
fn link_constraints_apply_to_interdocument_references_only() {
    let found = xrefs(
        &parse("xref:o.adoc#f[T,window=_blank,opts=nofollow,role=r] xref:#x[T,window=_blank]\n\n[[x]]\n== X")
            .document
            .flat_blocks(),
    );
    let (interdoc, _, _) = &found[0];
    assert_eq!(interdoc.window.as_deref(), Some("_blank"));
    assert_eq!(interdoc.rel.as_deref(), Some("nofollow noopener"));
    assert_eq!(interdoc.role.as_deref(), Some("r"));
    assert_eq!(
        (found[1].0.window.clone(), found[1].0.rel.clone()),
        (None, None)
    );
}

#[test]
fn file_prefix_and_pinned_suffix_shape_paths() {
    let source = "= Doc\n:outfilesuffix: .htm\n:relfileprefix: ../\n\nA <<a.adoc#x>>\n\n:outfilesuffix: .xhtml\n\nB xref:b.adoc[]\n\n:relfilesuffix: /\n\nC <<c#y>>";
    assert_eq!(
        targets(source),
        [
            ("../a.htm#x".into(), "../a.htm".into()),
            ("../b.htm".into(), "../b.htm".into()),
            ("../c/#y".into(), "../c/".into()),
        ]
    );
    // Like Ruby's converter default, the suffix is an ordinary attribute.
    assert_eq!(
        parse("x").document.attributes.get("outfilesuffix"),
        Some(".html")
    );
    assert_eq!(
        targets("= Doc\n:outfilesuffix!:\n\n<<a.adoc#x>>"),
        [("a#x".into(), "a".into())]
    );
}

#[test]
fn natural_references_resolve_titles_and_reftext_in_registration_order() {
    let source = "== Section One\n\n[[blk,Block Ref]]\npara\n\n== Same\n\n== Same\n\nA <<Section One>> B <<Block Ref>> C <<Same>> D <<section one>> E <<#Section One>>";
    let result = parse(source);
    let found: Vec<_> = xrefs(&result.document.flat_blocks())
        .into_iter()
        .map(|(x, label, _)| (x.target, x.refid.unwrap(), label))
        .collect();
    assert_eq!(
        found,
        [
            (
                "#_section_one".into(),
                "_section_one".into(),
                "Section One".into()
            ),
            ("#blk".into(), "blk".into(), "Block Ref".into()),
            ("#_same".into(), "_same".into(), "Same".into()),
            (
                "#section one".into(),
                "section one".into(),
                "[section one]".into()
            ),
            (
                "#Section One".into(),
                "Section One".into(),
                "[Section One]".into()
            ),
        ]
    );
    // Only the lowercase and explicit forms stay unresolved.
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| &source[d.span.start..d.span.end])
        .collect();
    assert_eq!(warnings, ["<<section one>>", "<<#Section One>>"]);
}

#[test]
fn natural_references_compare_the_converted_spelling() {
    assert_eq!(
        targets("== A & B\n\n== *Bold* Title\n\nX <<A & B>> Y <<Bold Title>>"),
        [
            ("#_a_b".into(), "A & B".into()),
            ("#Bold Title".into(), "[Bold Title]".into()),
        ]
    );
    // Compat mode never looks titles up.
    assert_eq!(
        targets(":compat-mode:\n\n== Section One\n\n<<Section One>>"),
        [("#Section One".into(), "[Section One]".into())]
    );
}

#[test]
fn natural_references_in_values_retarget_their_parts() {
    let source = ":r: <<Section One>>\n\n== Section One\n\nA {r}";
    let doc = parse(source).document;
    let parts = doc
        .flat_blocks()
        .iter()
        .find_map(|b| match &b.kind {
            BlockKind::Paragraph { inlines } => inlines.iter().find_map(|n| match n {
                Inline::AttributeRef { parts, .. } => Some(parts.clone()),
                _ => None,
            }),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        parts,
        [
            P::OpenXref(Xref::to_id("_section_one")),
            P::Text("Section One".into()),
            P::CloseXref
        ]
    );
}

#[test]
fn headings_look_up_natural_references_among_earlier_ids_while_reading() {
    let source =
        "== Earlier Title\n\n== See <<Earlier Title>>\n\n== Ref <<Later Title>>\n\n== Later Title";
    let result = parse(source);
    let blocks = result.document.flat_blocks();
    let ids: Vec<_> = blocks.iter().filter_map(|b| b.id.as_deref()).collect();
    assert_eq!(
        ids,
        [
            "_earlier_title",
            "_see_earlier_title",
            "_ref_later_title",
            "_later_title"
        ]
    );
    let found = xrefs(&blocks);
    assert_eq!(found[0].0.target, "#_earlier_title");
    // A forward title is not registered yet; conversion does not retry it.
    assert_eq!(found[1].0.target, "#Later Title");
    assert_eq!(found[1].1, "[Later Title]");
    assert_eq!(result.diagnostics.len(), 1);
    let span = result.diagnostics[0].span;
    assert_eq!(&source[span.start..span.end], "<<Later Title>>");
}

#[test]
fn the_current_document_displays_its_reftext_title_or_first_section() {
    assert_eq!(
        targets("= Doc *T*\n\nA <<#>> B xref:#[label]")[..],
        [("#".into(), "Doc T".into()), ("#".into(), "label".into())]
    );
    assert_eq!(targets("A <<#>>"), [("#".into(), "[^top]".into())]);
    assert_eq!(
        targets("pre\n\n== First\n\nA <<#>>"),
        [("#".into(), "First".into())]
    );
    assert_eq!(
        targets("[[top,Top Ref]]\n= Doc\n\nA <<#>>")[..],
        [("#".into(), "Top Ref".into())]
    );
    assert_eq!(
        targets(":reftext: R *x*\n\nA <<#>>"),
        [("#".into(), "R x".into())]
    );
}

fn resource(root: &str, files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", root).unwrap();
    for (name, text) in files {
        resources.insert(name, *text).unwrap();
    }
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    let options = ParseOptions {
        source_id,
        ..Default::default()
    };
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    xrefs(&result.document.flat_blocks())
        .into_iter()
        .map(|(xref, label, _)| (xref.target, label))
        .collect()
}

#[test]
fn this_document_and_full_includes_stay_internal() {
    let chapter = ("/docs/parts/chapter.adoc", "[[c]]\n== Chapter");
    assert_eq!(
        resource(
            "= Main\n\ninclude::parts/chapter.adoc[]\n\nA <<parts/chapter.adoc#c>> B xref:main.adoc[] C <<main#c>>",
            &[chapter]
        ),
        [
            ("#c".into(), "Chapter".into()),
            ("#".into(), "Main".into()),
            ("#c".into(), "Chapter".into()),
        ]
    );
    // Selected lines are a partial include; a later full include counts for
    // paragraphs, which convert after reading.
    assert_eq!(
        resource(
            "include::parts/chapter.adoc[lines=1..2]\n\nA <<parts/chapter.adoc#c>>",
            &[chapter]
        )[0]
        .0,
        "parts/chapter.html#c"
    );
    assert_eq!(
        resource(
            "include::parts/chapter.adoc[lines=1]\n\nA <<parts/chapter.adoc#c>>\n\ninclude::parts/chapter.adoc[]\n\nB <<parts/chapter.adoc#c>>",
            &[chapter]
        )
        .into_iter()
        .map(|(target, _)| target)
        .collect::<Vec<_>>(),
        ["#c", "#c"]
    );
    // tags=** keeps the whole file; an empty selection never registers.
    assert_eq!(
        resource(
            "include::parts/chapter.adoc[tags=**]\n\nA <<parts/chapter.adoc#c>>",
            &[chapter]
        )[0]
        .0,
        "#c"
    );
    assert_eq!(
        resource(
            "include::parts/chapter.adoc[lines=9]\n\nA <<parts/chapter.adoc#c>>",
            &[chapter]
        )[0]
        .0,
        "parts/chapter.html#c"
    );
}

#[test]
fn headings_only_see_includes_read_before_them() {
    let found = resource(
        "== Before <<c.adoc#c>>\n\ninclude::c.adoc[]\n\n== After <<c.adoc#c>>",
        &[("/docs/c.adoc", "[[c]]\n== Chapter")],
    );
    assert_eq!(
        found.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        ["c.html#c", "#c"]
    );
}
