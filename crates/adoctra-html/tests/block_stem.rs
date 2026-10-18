use adoctra_core::{parse, Backend};
use adoctra_html::HtmlBackend;

fn render(source: &str) -> String {
    String::from_utf8(HtmlBackend::new().convert(&parse(source).document).unwrap()).unwrap()
}

#[test]
fn block_math_matches_pinned_golden_markup() {
    let html = render(include_str!("golden/math/blocks.adoc"));
    let body = html
        .split_once("<body>\n")
        .unwrap()
        .1
        .split_once("</body>")
        .unwrap()
        .0;
    assert_eq!(body, include_str!("golden/math/blocks.expected.html"));
}

#[test]
fn mathjax_configuration_matches_pinned_converter() {
    let html = render(":stem:\n\nx");
    let scripts = html
        .split_once("<script type=\"text/x-mathjax-config\">")
        .unwrap()
        .1
        .split_once("</head>")
        .unwrap()
        .0;
    assert_eq!(
        format!("<script type=\"text/x-mathjax-config\">{scripts}"),
        include_str!("golden/math/mathjax.expected.html")
    );
    assert!(!render("stem:[x]").contains("MathJax.Hub.Config"));
    assert!(!render("[stem]\n++++\nx\n++++").contains("MathJax.Hub.Config"));
}

#[test]
fn mathjax_uses_conversion_state_and_header_asset_scheme() {
    for (source, enabled, number) in [
        (":stem:\n:eqnums:\n\nx", true, "AMS"),
        (":stem:\n:eqnums: all\n\nx", true, "all"),
        ("x\n\n:stem:\n:eqnums: AMS\n\ny", true, "AMS"),
        (":stem:\n\nx\n\n:stem!:\n\ny", false, "none"),
        (":stem:\n\nx\n\n:stem!:", true, "none"),
        ("{set:stem:}{set:eqnums:AMS}", true, "AMS"),
    ] {
        let html = render(source);
        assert_eq!(html.contains("MathJax.Hub.Config"), enabled, "{source}");
        if enabled {
            assert!(
                html.contains(&format!("autoNumber: \"{number}\"")),
                "{source}"
            );
        }
    }
    for (scheme, expected) in [("http", "http:"), ("", "")] {
        let html = render(&format!(
            ":stem:\n:asset-uri-scheme: {scheme}\n\nx\n\n:asset-uri-scheme: https\n\ny"
        ));
        assert!(html.contains(&format!(
            "src=\"{expected}//cdnjs.cloudflare.com/ajax/libs/mathjax/2.7.9/MathJax.js"
        )));
    }
}

#[test]
fn equation_numbering_is_serialized_as_script_data() {
    let html = render(":stem:\n:eqnums: x\"</script><script>alert(1)</script>\n\nx");
    assert!(!html.contains("<script>alert"));
    assert!(html.contains(r#"autoNumber: "x\"\u003c/script>\u003cscript>alert(1)\u003c/script>""#));
}

#[test]
fn long_ascii_space_runs_and_unicode_preserve_content() {
    let spaces = " ".repeat(100_000);
    let html = render(&format!("[stem]\n++++\nЖ{spaces}Ю\n++++"));
    assert!(html.contains(&format!("\\$Ж{spaces}Ю\\$")));
}
