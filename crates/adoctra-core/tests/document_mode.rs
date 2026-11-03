//! Ruby's document initialization: date attributes from the host's clock,
//! the embedded/standalone attribute defaults and docinfo selection.
use adoctra_core::{
    docinfo, parse_with_options, AttributePart, Attributes, DateTime, DocinfoLocation,
    DocumentDates, MemoryResources, ParseOptions,
};

fn time(year: i32, hour: u8, utc_offset: i32) -> DateTime {
    DateTime {
        year,
        month: 3,
        day: 4,
        hour,
        minute: 6,
        second: 7,
        utc_offset,
    }
}

fn attributes(source: &str, options: ParseOptions) -> Attributes {
    parse_with_options(source, &options)
        .document
        .attributes
        .clone()
}

#[test]
fn dates_fill_ruby_attributes_from_the_host_clock() {
    let options = ParseOptions {
        dates: Some(DocumentDates {
            local: time(2026, 20, 3 * 3600),
            document: time(2020, 5, 0),
        }),
        ..Default::default()
    };
    let attrs = attributes("= T\n\nx", options.clone());
    for (key, value) in [
        ("localdate", "2026-03-04"),
        ("localyear", "2026"),
        ("localtime", "20:06:07 +0300"),
        ("localdatetime", "2026-03-04 20:06:07 +0300"),
        ("docdate", "2020-03-04"),
        ("docyear", "2020"),
        ("doctime", "05:06:07 UTC"),
        ("docdatetime", "2020-03-04 05:06:07 UTC"),
    ] {
        assert_eq!(attrs.get(key), Some(value), "{key}");
    }
    // A negative offset and a header override (soft, like Ruby's attrs).
    let options = ParseOptions {
        dates: Some(DocumentDates {
            local: time(2026, 1, -(5 * 3600 + 30 * 60)),
            document: time(2020, 5, 0),
        }),
        ..Default::default()
    };
    let attrs = attributes("= T\n:docdate: 1999-12-31\n\nx", options.clone());
    assert_eq!(attrs.get("localtime"), Some("01:06:07 -0530"));
    assert_eq!(attrs.get("docdate"), Some("1999-12-31"));
    // A host date derives its year and the datetime, like `||=`.
    let mut host = Attributes::new();
    host.set("docdate", "1999-12-31");
    host.set("doctime", "noon");
    let attrs = attributes(
        "x",
        ParseOptions {
            attributes: host,
            ..options
        },
    );
    assert_eq!(attrs.get("docyear"), Some("1999"));
    assert_eq!(attrs.get("docdatetime"), Some("1999-12-31 noon"));
    // Without a clock, core sets no dates.
    assert_eq!(
        attributes("x", ParseOptions::default()).get("docdate"),
        None
    );
}

#[test]
fn embedded_and_standalone_defaults_follow_ruby() {
    let standalone = attributes("x", ParseOptions::default());
    for key in ["copycss", "iconfont-remote", "stylesheet", "webfonts"] {
        assert_eq!(standalone.get(key), Some(""), "{key}");
    }
    assert_eq!(standalone.get("embedded"), None);
    assert_eq!(standalone.get("notitle"), None);
    let embedded = ParseOptions {
        embedded: true,
        ..Default::default()
    };
    let attrs = attributes("x", embedded.clone());
    assert_eq!(attrs.get("embedded"), Some(""));
    assert_eq!(attrs.get("notitle"), Some(""));
    assert_eq!(attrs.get("stylesheet"), None);
    // The document cannot change `embedded`; `showtitle` unsets `notitle`.
    let attrs = attributes(":embedded!:\n:showtitle:\n\nx", embedded.clone());
    assert_eq!(attrs.get("embedded"), Some(""));
    assert_eq!(attrs.get("notitle"), None);
    let attrs = attributes(":embedded:\n\nx", ParseOptions::default());
    assert_eq!(attrs.get("embedded"), None);
    // A host `showtitle` locks `notitle` unset; a host `notitle` locks
    // `showtitle` unset.
    let mut host = Attributes::new();
    host.set("showtitle", "");
    let attrs = attributes(
        ":notitle:\n:showtitle!:\n\nx",
        ParseOptions {
            attributes: host,
            ..embedded.clone()
        },
    );
    assert_eq!(attrs.get("notitle"), None);
    let mut host = Attributes::new();
    host.set("notitle", "");
    let attrs = attributes(
        ":showtitle:\n\nx",
        ParseOptions {
            attributes: host,
            ..embedded
        },
    );
    assert_eq!(
        (attrs.get("notitle"), attrs.get("showtitle")),
        (Some(""), None)
    );
    // A standalone `showtitle!` sets `notitle`.
    let attrs = attributes(":showtitle!:\n\nx", ParseOptions::default());
    assert_eq!(attrs.get("notitle"), Some(""));
}

fn text(parts: &[AttributePart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            AttributePart::Text(text) | AttributePart::Raw(text) => text.as_str(),
            _ => "",
        })
        .collect()
}

#[test]
fn docinfo_selects_files_and_substitutes_attributes_like_ruby() {
    let mut resources = MemoryResources::new();
    for (name, text) in [
        (
            "/d/docinfo.html",
            "\u{feff}<meta content=\"{doctitle}\">  \r\n<x>\n",
        ),
        ("/d/guide-docinfo.html", "<private {docname}>"),
        ("/d/docinfo-footer.html", "<footer {missing}>"),
        ("/d/info/docinfo-header.html", "<header *q*>"),
        ("/d/guide-docinfo-header.html", "<private-header>"),
    ] {
        resources.insert(name, text).unwrap();
    }
    let attrs = |entries: &[(&str, &str)]| {
        let mut attributes = Attributes::new();
        attributes.set("doctitle", "T");
        attributes.set("docname", "guide");
        for (key, value) in entries {
            attributes.set(*key, *value);
        }
        attributes
    };
    let load = |attributes: &Attributes, location| {
        text(&docinfo(
            attributes,
            location,
            ".html",
            &resources,
            Some("/d/guide.adoc"),
        ))
    };
    // `docinfo` alone is private; files are normalized and substituted.
    let private = attrs(&[("docinfo", "")]);
    assert_eq!(load(&private, DocinfoLocation::Head), "<private guide>");
    let both = attrs(&[("docinfo2", "")]);
    assert_eq!(
        load(&both, DocinfoLocation::Head),
        "<meta content=\"T\">\n<x>\n<private guide>"
    );
    assert_eq!(
        load(&attrs(&[("docinfo1", "")]), DocinfoLocation::Head),
        "<meta content=\"T\">\n<x>"
    );
    // Location-qualified names, `docinfodir` and missing references (skip).
    let footer = attrs(&[("docinfo", "shared-footer, private-header")]);
    assert_eq!(load(&footer, DocinfoLocation::Footer), "<footer {missing}>");
    assert_eq!(load(&footer, DocinfoLocation::Head), "");
    assert_eq!(load(&footer, DocinfoLocation::Header), "<private-header>");
    let dir = attrs(&[("docinfo", "shared"), ("docinfodir", "info")]);
    assert_eq!(load(&dir, DocinfoLocation::Header), "<header *q*>");
    // `docinfosubs` selects the substitutions.
    let quoted = attrs(&[
        ("docinfo", "shared"),
        ("docinfodir", "info"),
        ("docinfosubs", "quotes"),
    ]);
    assert_eq!(load(&quoted, DocinfoLocation::Header), "<header q>");
    assert!(docinfo(
        &quoted,
        DocinfoLocation::Header,
        ".html",
        &resources,
        Some("/d/guide.adoc")
    )
    .iter()
    .any(|part| matches!(part, AttributePart::Open(_))));
    // Nothing selected, nothing read.
    assert_eq!(load(&attrs(&[]), DocinfoLocation::Head), "");
}

#[test]
fn docinfo_decodes_utf16_boms_and_right_strips_lines_like_ruby() {
    let utf16 = |little: bool, text: &str| {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        bytes
    };
    let mut attributes = Attributes::new();
    attributes.set("docname", "guide");
    attributes.set("doctitle", "Т");
    attributes.set("docinfo", "shared,private");
    let load = |resources: &dyn adoctra_core::ResourceProvider| {
        text(&docinfo(
            &attributes,
            DocinfoLocation::Head,
            ".html",
            resources,
            Some("/d/guide.adoc"),
        ))
    };
    // `prepare_source_string`: a UTF-16 BOM decodes the file; `rstrip` keeps
    // U+00A0 and U+2028 but drops NUL, tab and CR; only LF splits lines.
    let mut resources = MemoryResources::new();
    resources
        .insert_binary(
            "/d/docinfo.html",
            utf16(
                true,
                "<meta content=\"{doctitle}\">\u{a0}\n<x>\0\t\r\n\n<y a\rb>\u{2028}",
            ),
        )
        .unwrap();
    resources
        .insert_binary("/d/guide-docinfo.html", utf16(false, "<private>\n"))
        .unwrap();
    assert_eq!(
        load(&resources),
        "<meta content=\"Т\">\u{a0}\n<x>\n\n<y a\rb>\u{2028}\n<private>"
    );
    // UTF-8 is unchanged: BOM removed, lines stripped the same way.
    resources.remove_binary("/d/docinfo.html").unwrap();
    resources
        .insert("/d/docinfo.html", "\u{feff}<a>\u{a0}\u{0}\n")
        .unwrap();
    assert_eq!(load(&resources), "<a>\u{a0}\n<private>");
    // Ruby fails the whole conversion on malformed docinfo; adoctra skips the
    // file like a missing one (an explicit boundary, not a compatible result).
    resources
        .insert_binary("/d/docinfo.html", b"<a>\xff\n".to_vec())
        .unwrap();
    let mut odd = utf16(true, "<a>");
    odd.push(0);
    resources
        .insert_binary("/d/guide-docinfo.html", odd)
        .unwrap();
    assert_eq!(load(&resources), "");

    // A text-only host keeps working through `load`.
    struct TextOnly;
    impl adoctra_core::ResourceProvider for TextOnly {
        fn load(
            &self,
            request: adoctra_core::ResourceRequest<'_>,
        ) -> adoctra_core::ResourceResult<adoctra_core::Resource> {
            Ok(adoctra_core::Resource {
                name: format!("/d/{}", request.target),
                text: format!("<{}>\u{a0}\0\n", request.target),
            })
        }
    }
    assert_eq!(
        load(&TextOnly),
        "<docinfo.html>\u{a0}\n<guide-docinfo.html>\u{a0}"
    );
}
