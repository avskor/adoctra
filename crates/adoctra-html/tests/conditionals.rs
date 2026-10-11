use adoctra_core::{parse, Backend};
use adoctra_html::HtmlBackend;

#[test]
fn selected_content_renders_in_all_content_models() {
    for (source, expected) in [
        (
            "ifndef::missing[*shown*]\nifdef::missing[hidden]",
            "*shown*",
        ),
        (
            "----\nifndef::missing[shown]\nifdef::missing[hidden]\n----",
            "----\nshown\n----",
        ),
        (
            "[literal]\nifndef::missing[shown]\nifdef::missing[hidden]",
            "[literal]\nshown",
        ),
        (
            "[verse]\n____\nifndef::missing[*shown*]\nifdef::missing[hidden]\n____",
            "[verse]\n____\n*shown*\n____",
        ),
        (
            "++++\nifndef::missing[<b>shown</b>]\nifdef::missing[hidden]\n++++",
            "++++\n<b>shown</b>\n++++",
        ),
        (
            "ifndef::missing[* first]\nifdef::missing[* hidden]\n* last",
            "* first\n* last",
        ),
    ] {
        let actual = parse(source);
        assert!(actual.diagnostics.is_empty());
        assert_eq!(
            HtmlBackend.convert(&actual.document).unwrap(),
            HtmlBackend.convert(&parse(expected).document).unwrap(),
            "{source}"
        );
    }
}
