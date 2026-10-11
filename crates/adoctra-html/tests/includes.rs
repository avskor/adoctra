use adoctra_core::{parse, parse_resource, Backend, MemoryResources, ParseOptions, SourceMap};
use adoctra_html::HtmlBackend;

#[test]
fn include_selection_renders_only_retained_content() {
    for (source, child, expected) in [
        (
            "include::part.adoc[lines=2;4]",
            "omitted\n*first\nomitted\nlast*",
            "*first\nlast*",
        ),
        (
            "----\ninclude::part.adoc[tag=a]\n----",
            "omitted\n// tag::a[]\n  Я <&>\n\tline\n// end::a[]",
            "----\n  Я <&>\n\tline\n----",
        ),
        (
            "include::part.adoc[tag=a]",
            "// tag::a[]\n= Title\nJane Doe\n\n* item\n// end::a[]\nomitted",
            "= Title\nJane Doe\n\n* item",
        ),
    ] {
        let mut resources = MemoryResources::new();
        resources.insert("main.adoc", source).unwrap();
        resources.insert("part.adoc", child).unwrap();
        let mut sources = SourceMap::new();
        let source_id = sources.load("main.adoc", None, &resources).unwrap();
        let options = ParseOptions {
            source_id,
            ..Default::default()
        };
        let actual = parse_resource(&mut sources, &options, &resources).unwrap();
        assert!(actual.diagnostics.is_empty(), "{:?}", actual.diagnostics);
        assert_eq!(
            HtmlBackend.convert(&actual.document).unwrap(),
            HtmlBackend.convert(&parse(expected).document).unwrap(),
            "{source}",
        );
    }
}
