//! Cross-crate integration test.

use adoctra::Backend;

fn diagnostic_output(source: &str, mode: Option<&str>) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut command = Command::new(env!("CARGO_BIN_EXE_adoctra"));
    command.args(["-", "--stdout", "--embedded"]);
    if let Some(mode) = mode {
        command.args(["--diagnostics", mode]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn cli_default_diagnostics_match_conversion_messages_and_warn_threshold() {
    for (source, expected) in [
        (
            "----\ntext\n",
            "adoctra: -:1: warning: unterminated listing block\n",
        ),
        (
            "\nendif::x[]\n",
            "adoctra: -:2: error: unmatched preprocessor directive: endif::x[]\n",
        ),
        (
            ":attribute-missing: warn\n\nЖ {missing}",
            "adoctra: warning: skipping reference to missing attribute: missing\n",
        ),
        ("See <<missing>>.", ""),
        ("ifdef::x[]\ntext\n", ""),
    ] {
        assert_eq!(diagnostic_output(source, None), expected, "{source}");
        assert_eq!(
            diagnostic_output(source, Some("compat")),
            expected,
            "{source}"
        );
        assert!(
            !diagnostic_output(source, Some("editor")).is_empty(),
            "{source}"
        );
    }
}

#[test]
fn cli_editor_keeps_physical_unicode_coordinates_and_original_severity() {
    let source = ":attribute-missing: warn\r\n\r\nЖ🙂 {missing}\r\n";
    let start = source.find("{missing}").unwrap();
    assert_eq!(diagnostic_output(source, Some("editor")), format!(
        "adoctra: -:3:4: warning at {start}..{}: skipping reference to missing attribute: missing\n", start + 9));
    assert_eq!(
        diagnostic_output("----\ntext\n", Some("editor")),
        "adoctra: -:1:1: error at 0..4: unterminated listing block (missing closing `----`)\n"
    );
}

#[test]
fn cli_document_paths_resolve_nested_includes_and_render_in_html_and_json() {
    use std::process::Command;
    let root = std::env::temp_dir().join(format!("adoctra-cli-paths-{}", std::process::id()));
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(
        root.join("guide.adoc"),
        "= {docname}\n\ninclude::{docdir}/nested/part.adoc[]",
    )
    .unwrap();
    std::fs::write(
        root.join("nested/part.adoc"),
        "include::{docdir}/{docname}.txt[]",
    )
    .unwrap();
    std::fs::write(
        root.join("guide.txt"),
        "{docname} {docfilesuffix} {docfile}",
    )
    .unwrap();
    let canonical = root.join("guide.adoc").canonicalize().unwrap();
    let display_path = canonical
        .to_str()
        .unwrap()
        .replace(std::path::MAIN_SEPARATOR, "/");
    for format in ["html", "json"] {
        let output = Command::new(env!("CARGO_BIN_EXE_adoctra"))
            .arg(root.join("guide.adoc"))
            .args(["--to", format, "--stdout"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        assert!(output.stderr.is_empty(), "{:?}", output.stderr);
        let rendered = String::from_utf8(output.stdout).unwrap();
        if format == "html" {
            assert!(rendered.contains("<h1>guide</h1>"), "{rendered}");
            assert!(
                rendered.contains(&format!("<p>guide .adoc {display_path}</p>")),
                "{rendered}"
            );
        } else {
            for value in ["guide", ".adoc", display_path.as_str()] {
                assert!(
                    rendered.contains(&format!(r#""value":"{value}""#)),
                    "{rendered}"
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn parse_then_render_html() {
    let doc = adoctra::parse("= Title\n\nHello *world*").document;
    let html = String::from_utf8(adoctra::html::HtmlBackend::new().convert(&doc).unwrap()).unwrap();
    assert!(html.contains("<h1>Title</h1>"));
    assert!(html.contains("<strong>world</strong>"));
}

#[test]
fn diagnostics_are_reported_not_fatal() {
    let result = adoctra::parse("----\nunterminated");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.document.blocks.len(), 1);
}

#[test]
fn cross_reference_resolves_to_section() {
    // A section gets an auto id; `<<id>>` resolves to its title, end to end.
    let result = adoctra::parse("== Hello\n\nSee <<_hello>>.");
    assert!(result.diagnostics.is_empty());
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(html.contains("<h2 id=\"_hello\">Hello</h2>"), "got: {html}");
    assert!(
        html.contains("<a href=\"#_hello\">Hello</a>"),
        "got: {html}"
    );
}

#[test]
fn unresolved_cross_reference_is_a_diagnostic() {
    let result = adoctra::parse("See <<missing>>.");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].severity, adoctra::Severity::Warning);
}

#[test]
fn tokens_drive_highlighting() {
    let toks = adoctra::tokens("= Title\n\ntext *bold*");
    assert!(toks.iter().any(|t| t.kind == adoctra::TokenKind::Heading));
    assert!(toks.iter().any(|t| t.kind == adoctra::TokenKind::Strong));
}

#[test]
fn reference_in_block_title_renders_in_html_and_json() {
    let source = ".See <<_target>>\n----\ncode\n----\n\n== Target";
    let result = adoctra::parse(source);
    assert!(result.diagnostics.is_empty());
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(
        html.contains("<div class=\"title\">See <a href=\"#_target\">Target</a></div>"),
        "{html}"
    );
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(json.contains(r##""title":[{"name":"text","value":"See "},{"name":"xref","target":"#_target","refid":"_target","inlines":[{"name":"text","value":"Target"}]}]"##), "{json}");
}

#[test]
fn header_and_body_attribute_values_reach_html_and_json() {
    let result = adoctra::parse("= Doc\n:who: first\n\n{who}\n\n:who: second\n\n{who}");
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(
        html.contains("<p>first</p>") && html.contains("<p>second</p>"),
        "{html}"
    );
    assert!(!html.contains(":who:"));
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(
        json.contains(r#""value":"first""#) && json.contains(r#""value":"second""#),
        "{json}"
    );
}

#[test]
fn cli_diagnostic_has_source_line_and_unicode_column() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_adoctra"))
        .args(["--diagnostics", "editor"])
        .args(["-", "--stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("Привет <<missing>>".as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("-:1:8: warning"), "{stderr}");
}

#[test]
fn header_declarations_and_resolved_values_reach_backends() {
    let result = adoctra::parse(
        "= Doc\nJane Doe <jane@example.org>\nv2, today: Draft\n\nBy {author}, version {revnumber}.",
    );
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::embedded()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(html.contains("<p>By Jane Doe, version 2.</p>"), "{html}");
    assert!(!html.contains("jane@example.org"));
    // A standalone document shows the declarations in its header.
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(
        html.contains(
            "<span id=\"email\" class=\"email\"><a href=\"mailto:jane@example.org\">jane@example.org</a></span><br>\n\
             <span id=\"revnumber\">version 2,</span>\n<span id=\"revdate\">today</span>\n\
             <br><span id=\"revremark\">Draft</span>"
        ),
        "{html}"
    );
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(json.contains(r#""header":{"authors":[{"name":"Jane Doe","firstname":"Jane","middlename":null,"lastname":"Doe","initials":"JD","email":"jane@example.org"}],"revision":{"number":"2","date":"today","remark":"Draft"},"title":"Doc"}"#), "{json}");
    assert!(json.contains(r#""value":"Jane Doe""#), "{json}");
}

#[test]
fn comments_are_omitted_in_html_and_json_but_verbatim_is_preserved() {
    let source = "= Doc\n// hidden author note\nJane Doe\n\none\n// secret\n*two*\n\n////\n<<missing>>\n////\n\n----\n// visible\n----";
    let result = adoctra::parse(source);
    assert!(result.diagnostics.is_empty());
    let html = String::from_utf8(
        adoctra::html::HtmlBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(html.contains("<p>one <strong>two</strong></p>"), "{html}");
    assert!(html.contains("// visible"));
    let json = String::from_utf8(
        adoctra::json::JsonBackend::new()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap();
    assert!(json.contains(r#""text":"// visible""#), "{json}");
    for output in [html, json] {
        assert!(
            !output.contains("secret")
                && !output.contains("hidden author")
                && !output.contains("missing"),
            "{output}"
        );
    }
}

#[test]
fn cli_file_resource_reports_canonical_source_name_and_renders_content() {
    use std::process::Command;
    let path =
        std::env::temp_dir().join(format!("adoctra-cli-resource-{}.adoc", std::process::id()));
    std::fs::write(&path, "Привет <<missing>>").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_adoctra"))
        .args(["--diagnostics", "editor"])
        .arg(&path)
        .arg("--stdout")
        .output()
        .unwrap();
    let canonical = path.canonicalize().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&format!("{}:1:8: warning", canonical.display())),
        "{stderr}"
    );
    assert!(String::from_utf8(output.stdout).unwrap().contains("Привет"));
}

#[test]
fn cli_expands_nested_includes_and_reports_child_locations_and_denied_paths() {
    use std::process::Command;
    let root = std::env::temp_dir().join(format!("adoctra-cli-includes-{}", std::process::id()));
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(
        root.join("main.adoc"),
        "include::sub/child.adoc[]\n\ninclude::../outside.adoc[]\n\nafter",
    )
    .unwrap();
    std::fs::write(root.join("sub/child.adoc"), "Я *текст* <<missing>>").unwrap();
    // An existing resource outside the root must still be denied.
    let outside = root.with_file_name(format!("adoctra-outside-{}.adoc", std::process::id()));
    std::fs::write(&outside, "must not leak").unwrap();
    let target = format!("../{}", outside.file_name().unwrap().to_str().unwrap());
    std::fs::write(
        root.join("main.adoc"),
        format!("include::sub/child.adoc[]\n\ninclude::{target}[]\n\nafter"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_adoctra"))
        .args(["--diagnostics", "editor"])
        .arg(root.join("main.adoc"))
        .arg("--stdout")
        .output()
        .unwrap();
    let child = root.join("sub/child.adoc").canonicalize().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    std::fs::remove_file(outside).unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&format!("{}:1:11: warning", child.display())),
        "{stderr}"
    );
    assert!(stderr.contains("AccessDenied"), "{stderr}");
    let html = String::from_utf8(output.stdout).unwrap();
    assert!(
        html.contains("<strong>текст</strong>") && html.contains("<p>after</p>"),
        "{html}"
    );
    assert!(!html.contains("must not leak"));
}

#[test]
fn included_document_has_the_same_html_and_json_as_a_single_logical_stream() {
    use adoctra::{parse_resource, MemoryResources, ParseOptions, SourceMap};
    let mut resources = MemoryResources::new();
    resources
        .insert("main.adoc", "= Doc\n\ninclude::part.adoc[]\n\n== Target")
        .unwrap();
    resources
        .insert(
            "part.adoc",
            ".See <<_target>>\n====\n* first\n* second\n====",
        )
        .unwrap();
    let mut sources = SourceMap::new();
    let source_id = sources.load("main.adoc", None, &resources).unwrap();
    let result = parse_resource(
        &mut sources,
        &ParseOptions {
            source_id,
            ..Default::default()
        },
        &resources,
    )
    .unwrap();
    let expected =
        adoctra::parse("= Doc\n\n.See <<_target>>\n====\n* first\n* second\n====\n\n== Target");
    assert!(result.diagnostics.is_empty());
    for backend in [
        Box::new(adoctra::html::HtmlBackend::new()) as Box<dyn adoctra::Backend>,
        Box::new(adoctra::json::JsonBackend::new()),
    ] {
        assert_eq!(
            backend.convert(&result.document).unwrap(),
            backend.convert(&expected.document).unwrap()
        );
    }
}

#[test]
fn cli_writes_a_standalone_document_or_embedded_content() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let run = |args: &[&str]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_adoctra"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"= Doc\nAnn Lee\n\nText.\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    let standalone = run(&["-", "--stdout"]);
    assert!(standalone.starts_with("<!DOCTYPE html>\n<html lang=\"en\">"));
    assert!(standalone.contains("<span id=\"author\" class=\"author\">Ann Lee</span>"));
    assert!(standalone.contains("<footer id=\"footer\">"));
    // Embedded content hides the title unless `showtitle` is set.
    for flag in ["-s", "--embedded", "--no-header-footer"] {
        assert_eq!(run(&["-", flag, "--stdout"]), "<p>Text.</p>\n");
    }
    assert_eq!(
        run(&["-", "-s", "-a", "showtitle", "--stdout"]),
        "<h1>Doc</h1>\n<p>Text.</p>\n"
    );
    assert_eq!(
        run(&[
            "-",
            "-s",
            "--attribute",
            "author=X",
            "-a",
            "showtitle=",
            "--stdout"
        ]),
        "<h1>Doc</h1>\n<p>Text.</p>\n"
    );
}

#[test]
fn cli_supplies_dates_docinfo_and_copies_linked_stylesheets() {
    use std::process::Command;
    let root = std::env::temp_dir().join(format!("adoctra-cli-document-{}", std::process::id()));
    std::fs::create_dir_all(root.join("css")).unwrap();
    std::fs::write(
        root.join("guide.adoc"),
        "= Guide\n:docinfo: shared,private\n\n{docdate} {docyear} {doctime} | {localdatetime}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("docinfo.html"),
        "<meta name=\"t\" content=\"{doctitle}\">\n",
    )
    .unwrap();
    std::fs::write(
        root.join("guide-docinfo-footer.html"),
        "<p>{docname} end</p>",
    )
    .unwrap();
    std::fs::write(root.join("css/my.css"), "body { color: red; }").unwrap();
    let run = |args: &[&str], epoch: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_adoctra"));
        command.arg(root.join("guide.adoc")).args(args);
        match epoch {
            Some(epoch) => command.env("SOURCE_DATE_EPOCH", epoch),
            None => command.env_remove("SOURCE_DATE_EPOCH"),
        };
        let output = command.output().unwrap();
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    // SOURCE_DATE_EPOCH fixes both clocks in UTC, and the footer shows it.
    let (ok, html, _) = run(&["--stdout"], Some("86400"));
    assert!(ok);
    assert!(
        html.contains("<p>1970-01-02 1970 00:00:00 UTC | 1970-01-02 00:00:00 UTC</p>"),
        "{html}"
    );
    assert!(
        html.contains("<div id=\"footer-text\">\nLast updated 1970-01-02 00:00:00 UTC\n</div>"),
        "{html}"
    );
    assert!(
        html.contains("<meta name=\"t\" content=\"Guide\">\n</head>"),
        "{html}"
    );
    assert!(html.contains("<p>guide end</p>\n</body>"), "{html}");
    let (ok, _, stderr) = run(&["--stdout"], Some("soon"));
    assert!(!ok);
    assert!(
        stderr.contains("invalid SOURCE_DATE_EPOCH: soon"),
        "{stderr}"
    );
    // Otherwise the document date is the file's modification time.
    let modified = std::fs::metadata(root.join("guide.adoc"))
        .unwrap()
        .modified()
        .unwrap();
    let year = 1970
        + modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 31_556_952;
    let (ok, html, _) = run(&["--stdout"], None);
    assert!(ok && html.contains(&format!(" {year} ")), "{html}");
    // `-a` overrides; `linkcss` with `copycss` writes the stylesheet next to
    // the output.
    let out = root.join("out/guide.html");
    let (ok, _, _) = run(
        &[
            "-a",
            "linkcss",
            "-a",
            "stylesdir=styles",
            "-o",
            out.to_str().unwrap(),
        ],
        Some("0"),
    );
    assert!(ok);
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(
        html.contains("<link rel=\"stylesheet\" href=\"styles/adoctra.css\">"),
        "{html}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("out/styles/adoctra.css")).unwrap(),
        adoctra::html::STYLESHEET
    );
    // A custom stylesheet is copied from the input's directory, or inlined
    // without `linkcss`.
    let (ok, _, _) = run(
        &[
            "-a",
            "linkcss",
            "-a",
            "stylesheet=css/my.css",
            "-o",
            out.to_str().unwrap(),
        ],
        Some("0"),
    );
    assert!(ok);
    assert_eq!(
        std::fs::read_to_string(root.join("out/css/my.css")).unwrap(),
        "body { color: red; }"
    );
    let (ok, html, _) = run(
        &["-a", "stylesheet=my.css", "-a", "stylesdir=css", "--stdout"],
        Some("0"),
    );
    assert!(
        ok && html.contains("<style>\nbody { color: red; }\n</style>"),
        "{html}"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn cli_safe_mode_controls_includes_user_home_and_copied_stylesheets() {
    use std::process::Command;
    let root = std::env::temp_dir().join(format!("adoctra-cli-safe-{}", std::process::id()));
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("outside.adoc"), "Outside text").unwrap();
    std::fs::write(
        root.join("docs/main.adoc"),
        "{safe-mode-name} [{user-home}]\n\ninclude::../outside.adoc[]\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_adoctra"))
            .arg(root.join("docs/main.adoc"))
            .args(args)
            .env("HOME", "/home/someone")
            .output()
            .unwrap();
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
        )
    };
    // The default safe mode keeps paths in the input's directory.
    let (ok, html) = run(&["-s", "--stdout"]);
    assert!(ok);
    assert!(html.contains("<p>safe [/home/someone]</p>"), "{html}");
    assert!(html.contains("include::../outside.adoc[]"), "{html}");
    let (_, html) = run(&["-s", "--safe", "--stdout"]);
    assert!(html.contains("<p>safe "), "{html}");
    // Unsafe mode reads anywhere.
    let (_, html) = run(&["-s", "-S", "unsafe", "--stdout"]);
    assert!(html.contains("<p>Outside text</p>"), "{html}");
    // Server mode hides the home; secure mode links includes.
    let (_, html) = run(&["-s", "-S", "server", "--stdout"]);
    assert!(html.contains("<p>server [.]</p>"), "{html}");
    let (_, html) = run(&["-s", "-S", "SECURE", "--stdout"]);
    assert!(
        html.contains("<a href=\"../outside.adoc\" class=\"bare include\">../outside.adoc</a>"),
        "{html}"
    );
    // An unknown mode is a usage error.
    let (_, html) = run(&["-S", "paranoid", "--stdout"]);
    assert!(html.is_empty());
    // Secure mode links the stylesheet but never copies it.
    let out = root.join("out/main.html");
    let (ok, _) = run(&["-S", "secure", "-a", "copycss", "-o", out.to_str().unwrap()]);
    assert!(ok);
    assert!(std::fs::read_to_string(&out)
        .unwrap()
        .contains("<link rel=\"stylesheet\" href=\"./adoctra.css\">"));
    assert!(!root.join("out/adoctra.css").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn cli_soft_and_unset_overrides_keep_last_occurrence_across_spellings() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (flags, expected) in [
        (vec!["cash=heroes@"], "money"),
        (vec!["cash@=heroes"], "money"),
        (vec!["cash!"], "{cash}"),
        (vec!["!cash"], "{cash}"),
        (vec!["cash!", "cash=heroes@"], "money"),
        (vec!["cash=heroes@", "!cash"], "{cash}"),
        (vec!["cash!@", "cash=heroes"], "heroes"),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_adoctra"));
        command.args(["-", "--embedded", "--stdout"]);
        for flag in &flags {
            command.args(["-a", flag]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b":cash: money\n\n{cash}")
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success());
        assert!(result.stderr.is_empty());
        assert!(
            String::from_utf8(result.stdout)
                .unwrap()
                .contains(&format!("<p>{expected}</p>")),
            "{flags:?}"
        );
    }
}

#[test]
fn cli_json_attributes_are_opt_in_and_rejected_for_html() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (backend, enabled) in [("json", false), ("json", true), ("html", true)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_adoctra"));
        command.args(["-", "--to", backend, "--stdout"]);
        if enabled {
            command.arg("--json-attributes");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b":v: first\n\n{v}\n\n:v: second\n\n{v}")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.success(), backend == "json");
        let stdout = String::from_utf8(output.stdout).unwrap();
        if backend == "json" {
            assert!(output.stderr.is_empty());
            assert_eq!(stdout.contains("\"attribute_snapshots\":"), enabled);
            if enabled {
                assert!(stdout.contains("\"v\":\"first\""));
                assert!(stdout.contains("\"v\":\"second\""));
            }
        } else {
            assert!(stdout.is_empty());
            assert!(String::from_utf8(output.stderr)
                .unwrap()
                .contains("--json-attributes requires --to json"));
        }
    }
}
