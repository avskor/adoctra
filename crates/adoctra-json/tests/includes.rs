use adoctra_core::{parse, parse_resource, Backend, MemoryResources, ParseOptions, SourceMap};
use adoctra_json::JsonBackend;

#[test]
fn include_options_render_the_resolved_content() {
    for (source, child, expected) in [
        (
            "----\ninclude::part.adoc[indent=2]\n----",
            "    alpha\n      beta",
            "----\n  alpha\n    beta\n----",
        ),
        (
            ":tabsize: 4\n\n----\ninclude::part.adoc[indent=0]\n----",
            "\tЯ🙂\tX",
            ":tabsize: 4\n\n----\nЯ🙂  X\n----",
        ),
        ("include::part.adoc[indent=0,depth=0]", "  *Я*", "*Я*"),
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
        (
            "= Main\n\ninclude::part.adoc[leveloffset=+1]\n\n== After",
            "= Chapter\n\n== Section\n\ntext",
            "= Main\n\n== Chapter\n\n=== Section\n\ntext\n\n== After",
        ),
        (
            "before\ninclude::missing.adoc[opts=optional]\nafter",
            "unused",
            "before\nafter",
        ),
        (
            "----\ninclude::part.adoc[leveloffset=+1]\n----",
            "== Literal",
            "----\n:leveloffset: +1\n\n== Literal\n\n:leveloffset!:\n----",
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
            JsonBackend.convert(&actual.document).unwrap(),
            JsonBackend.convert(&parse(expected).document).unwrap(),
            "{source}",
        );
    }
}
