//! `ParseOptions.output`: Ruby's `Asciidoctor.convert` names the output file
//! after reading and `Document#convert` assigns `outfile`/`outdir` for the
//! conversion only, below server mode.

use adoctra_core::{
    parse_with_options, Attributes, BlockKind, Document, OutputPaths, OutputTarget, ParseOptions,
    SafeMode,
};

fn convert(source: &str, output: OutputTarget, safe: SafeMode) -> Document {
    let mut attributes = Attributes::new();
    attributes.set("docname", "guide");
    let options = ParseOptions {
        attributes,
        safe,
        output: Some(output),
        ..Default::default()
    };
    parse_with_options(source, &options).document
}

fn texts(doc: &Document) -> Vec<String> {
    doc.blocks
        .iter()
        .filter_map(|block| match &block.kind {
            BlockKind::Paragraph { inlines } => Some(
                inlines
                    .iter()
                    .map(|inline| match inline {
                        adoctra_core::Inline::Text { value, .. } => value.clone(),
                        adoctra_core::Inline::AttributeRef { value, name, .. } => {
                            value.clone().unwrap_or_else(|| format!("{{{name}}}"))
                        }
                        other => format!("{other:?}"),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn a_directory_names_the_file_with_the_header_suffix() {
    let source = ":outfilesuffix: .htm\n\nifdef::outfile[read]\n\n{outfile} {outdir}\n\n\
                  :outfilesuffix: .late\n\n{outfile}";
    let doc = convert(
        source,
        OutputTarget::Directory("/out/".into()),
        SafeMode::Safe,
    );
    // Ruby pins outfilesuffix at the end of the header; the body entry
    // changes the attribute, not the name.
    let output = OutputPaths {
        file: "/out/guide.htm".into(),
        dir: "/out/".into(),
    };
    assert_eq!(doc.output, Some(output));
    // Conversion sees the values; reading (the preprocessor) did not, and
    // the header snapshot keeps neither.
    assert_eq!(texts(&doc), ["/out/guide.htm /out/", "/out/guide.htm"]);
    assert_eq!(doc.attributes.get("outfile"), None);
    assert_eq!(doc.final_attributes.get("outdir"), Some("/out/"));
    assert_eq!(doc.final_attributes.get("outfilesuffix"), Some(".late"));
}

#[test]
fn a_file_gives_its_directory_and_server_mode_hides_both() {
    let source = ":outfilesuffix: .htm\n\n{outfile} {outdir}";
    let target = OutputTarget::File("/site/a/b.html".into());
    let doc = convert(source, target.clone(), SafeMode::Safe);
    assert_eq!(texts(&doc), ["/site/a/b.html /site/a"]);
    let doc = convert(source, target, SafeMode::Server);
    assert_eq!(texts(&doc), ["{outfile} {outdir}"]);
    assert_eq!(
        doc.output.map(|output| output.file),
        Some("/site/a/b.html".to_owned())
    );
    // Without docname and suffix, Ruby's File.join gives a bare name.
    assert_eq!(
        OutputTarget::Directory("/d".into()).resolve(None, None),
        OutputPaths {
            file: "/d/".into(),
            dir: "/d".into()
        }
    );
    assert_eq!(
        OutputTarget::File("x.html".into()).resolve(None, None).dir,
        "."
    );
}
