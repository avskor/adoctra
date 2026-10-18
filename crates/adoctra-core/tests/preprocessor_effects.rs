//! Set/counter expressions in `ifeval` operands, include targets and include
//! parameter lists execute like Ruby's `Document#sub_attributes` from the
//! preprocessor: they change the reading attributes only, exactly where
//! Ruby's branches substitute, and highlighting never executes them again.
use adoctra_core::{
    parse, parse_resource, parse_with_options, tokens_resource, tokens_with_options, BlockKind,
    Inline, MemoryResources, ParseOptions, ParseResult, ResourceProvider, ResourceRequest,
    SafeMode, Severity, SourceMap, TokenKind,
};
use std::cell::RefCell;

fn paragraphs(result: &ParseResult) -> Vec<String> {
    result
        .document
        .flat_blocks()
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(
                inlines
                    .iter()
                    .map(|inline| match inline {
                        Inline::Text { value, .. }
                        | Inline::AttributeRef {
                            value: Some(value), ..
                        } => value.clone(),
                        Inline::AttributeRef {
                            name, value: None, ..
                        } => format!("{{{name}}}"),
                        Inline::Substituted { value, .. } => value.clone(),
                        _ => panic!("unexpected inline {inline:?}"),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

fn setup(root: &str, files: &[(&str, &str)]) -> (SourceMap, MemoryResources, ParseOptions) {
    let mut resources = MemoryResources::new();
    resources.insert("/docs/main.adoc", root).unwrap();
    for (name, text) in files {
        resources.insert(name, *text).unwrap();
    }
    let mut sources = SourceMap::new();
    let source_id = sources.load("/docs/main.adoc", None, &resources).unwrap();
    (
        sources,
        resources,
        ParseOptions {
            source_id,
            ..Default::default()
        },
    )
}

fn include(root: &str, files: &[(&str, &str)]) -> ParseResult {
    let (mut sources, resources, options) = setup(root, files);
    parse_resource(&mut sources, &options, &resources).unwrap()
}

#[test]
fn ifeval_expressions_change_reading_attributes_only() {
    let result = parse(
        "= T\n\nintro {n}\n\nifeval::[{counter:n} == 1]\nfirst {n}\nendif::[]\n\n\
         ifeval::[\"{set:foo:bar}{counter:n}\" == \"2\"]\nfoo={foo} {n}\nendif::[]\n\n\
         ifeval::[\"{foo}\" == \"bar\"]\nreading sees foo\nendif::[]",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    // Conditions see the effects; conversion restarts from the header, like
    // other reading effects (checked against Asciidoctor 2.0.23).
    assert_eq!(
        paragraphs(&result),
        [
            "intro {n}",
            "first {n}",
            "foo={foo} {n}",
            "reading sees foo"
        ]
    );
    assert_eq!(result.document.attributes.get("n"), None);
    assert_eq!(result.document.final_attributes.get("foo"), None);
}

#[test]
fn ifeval_operands_execute_left_to_right_and_skipped_branches_do_not_execute() {
    let result = parse(
        "ifeval::[{counter:a} < {counter:a}]\nordered\nendif::[]\n\n\
         ifdef::missing[]\nifeval::[{counter:z} == 1]\nendif::[]\nendif::[]\n\n\
         ifndef::z[]\nz untouched\nendif::[]\n\n\
         ifeval::[{counter:e}{set:g:h} == 1]\nset hides itself\nendif::[]\n\n\
         ifeval::[\"{g}\" == \"h\"]\ng set\nendif::[]\n\n\
         ifeval::[{set:u!}{counter:x} == 1]\nunset drops the operand\nendif::[]",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    // `{set:u!}` drops its line (attribute-undefined), so the left operand is
    // nil and the comparison is false.
    assert_eq!(
        paragraphs(&result),
        ["ordered", "z untouched", "set hides itself", "g set"]
    );
}

#[test]
fn ifeval_operands_use_the_stored_spelling_and_unicode_names() {
    let result = parse(
        ":x: a<b\n:café: 1\n\nifeval::[\"{x}\" == \"a&lt;b\"]\nstored\nendif::[]\n\n\
         ifeval::[{café} == 1]\nunicode\nendif::[]\n\nifeval::[\"{nope}\" == \"\"]\ndropped\nendif::[]",
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(paragraphs(&result), ["stored", "unicode", "dropped"]);
}

#[test]
fn ifeval_effects_respect_locks_and_reach_the_header_snapshot() {
    let mut options = ParseOptions::default();
    options.attributes.set("locked", "x");
    let result = parse_with_options(
        "= T\nifeval::[\"{set:locked:y}{set:early:1}\" == \"\"]\n:after: {early}\nendif::[]\n\n\
         ifeval::[\"{locked}\" == \"x\"]\nlocked kept\nendif::[]",
        &options,
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(paragraphs(&result), ["locked kept"]);
    // Inside the header the effect precedes header completion.
    assert_eq!(result.document.attributes.get("early"), Some("1"));
    assert_eq!(result.document.attributes.get("after"), Some("1"));
}

#[test]
fn highlighting_does_not_execute_preprocessor_expressions_again() {
    let source = "ifeval::[{counter:n} == 1]\nfirst\nendif::[]\n\n\
                  ifeval::[{counter:n} == 1]\nhidden\nendif::[]";
    let result = parse(source);
    assert_eq!(paragraphs(&result), ["first"]);
    let tokens = tokens_with_options(source, &ParseOptions::default());
    let inactive: Vec<_> = tokens
        .iter()
        .filter(|t| t.kind == TokenKind::Inactive)
        .map(|t| &source[t.span.start..t.span.end])
        .collect();
    assert_eq!(inactive, ["hidden"]);
    assert_eq!(result.inactive.len(), 1);
}

#[test]
fn include_targets_and_parameter_lists_execute_once_while_reading() {
    let result = include(
        "include::part-{counter:n}.adoc[]\n\ninclude::part-{counter:n}.adoc[]\n\n\
         ifeval::[{n} == 2]\nn is two\nendif::[]\n\n\
         include::p.adoc[lines={counter:l}]\n\ninclude::p.adoc[lines={counter:l}..3]\n\n\
         ifeval::[{l} == 2]\nl is two\nendif::[]\n\nafter {n}",
        &[
            ("/docs/part-1.adoc", "one"),
            ("/docs/part-2.adoc", "two"),
            ("/docs/p.adoc", "l1\nl2\nl3"),
        ],
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    // Without a header, the first include is read before the header
    // attributes are saved, so conversion sees its counter (as in Ruby).
    assert_eq!(
        paragraphs(&result),
        ["one", "two", "n is two", "l1", "l2 l3", "l is two", "after 1"]
    );
}

#[test]
fn blank_and_missing_targets_execute_like_ruby_branches() {
    let result = include(
        "include::{set:foo:bar}[]\n\nifeval::[\"{foo}\" == \"bar\"]\nfoo set\nendif::[]\n\n\
         :attribute-missing: drop-line\n\ninclude::{counter:c}{missing}.adoc[lines={counter:k}]\n\n\
         ifeval::[{c} == 2]\nc twice\nendif::[]\n\nifndef::k[]\nlist skipped\nendif::[]\n\n\
         :attribute-missing: warn\n\ninclude::{counter:w}{missing}[]\n\n\
         ifeval::[{w} == 2]\nw twice\nendif::[]\n\n\
         include::{counter:v}{missing}[opts=optional]\n\nifeval::[{v} == 1]\nv once\nendif::[]",
        &[],
    );
    let texts = paragraphs(&result);
    assert_eq!(texts[0], "Unresolved directive in main.adoc - include::[]");
    assert_eq!(&texts[1..3], ["foo set", "c twice"]);
    assert_eq!(texts[3], "list skipped");
    assert!(texts[4].starts_with("Unresolved directive in main.adoc - include::"));
    assert_eq!(&texts[5..], ["w twice", "v once"]);
    let warnings: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        &warnings[..2],
        [
            "include dropped because resolved target is blank: {set:foo:bar}",
            "include dropped due to missing attribute: {counter:w}{missing}",
        ]
    );
    // Directive substitutions report no informational drop-line messages.
    assert!(!result
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Info
            && d.message.starts_with("dropping line")
            && d.span.start < 100));
}

#[test]
fn secure_mode_and_exceeded_depth_do_not_evaluate_parameter_lists() {
    let options = ParseOptions {
        safe: SafeMode::Secure,
        ..Default::default()
    };
    let result = parse_with_options(
        "include::x-{counter:t}.adoc[lines={counter:s}]\n\n\
         ifndef::s[]\nno s\nendif::[]\n\nifeval::[{t} == 1]\nt one\nendif::[]",
        &options,
    );
    assert_eq!(&paragraphs(&result)[1..], ["no s", "t one"]);

    let (mut sources, resources, mut options) = setup(
        "include::a.adoc[]\n\nifndef::d[]\nno d\nendif::[]",
        &[("/docs/a.adoc", "include::b.adoc[lines={counter:d}]")],
    );
    options.includes.max_depth = 1;
    let result = parse_resource(&mut sources, &options, &resources).unwrap();
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message == "maximum include depth of 1 exceeded"));
    assert_eq!(paragraphs(&result)[1], "no d");
}

#[test]
fn resource_highlighting_does_not_load_includes_again() {
    struct Counting<'a>(&'a MemoryResources, RefCell<Vec<String>>);
    impl ResourceProvider for Counting<'_> {
        fn load(
            &self,
            request: ResourceRequest<'_>,
        ) -> adoctra_core::ResourceResult<adoctra_core::Resource> {
            self.1.borrow_mut().push(request.target.to_owned());
            self.0.load(request)
        }
        fn document_path(&self, name: &str) -> Option<String> {
            self.0.document_path(name)
        }
    }
    let (mut sources, resources, options) = setup(
        "include::part-{counter:n}.adoc[]\n\ninclude::part-{counter:n}.adoc[]",
        &[("/docs/part-1.adoc", "one"), ("/docs/part-2.adoc", "two")],
    );
    let counting = Counting(&resources, RefCell::new(Vec::new()));
    let tokens = tokens_resource(&mut sources, &options, &counting).unwrap();
    // One parse loads each expanded target once; the token scan reuses it.
    assert_eq!(*counting.1.borrow(), ["part-1.adoc", "part-2.adoc"]);
    assert_eq!(
        tokens
            .iter()
            .filter(|t| t.kind == TokenKind::IncludeDirective)
            .count(),
        2
    );
}
