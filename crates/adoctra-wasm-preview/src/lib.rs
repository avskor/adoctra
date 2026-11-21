//! `adoctra-wasm-preview` — the editor bindings with a live HTML preview.
//!
//! `adoctra-wasm` gives an editor the parser and never a converter. A host
//! that also shows the converted document loads this module **instead**: it
//! answers each edit with the [editor view](adoctra_wasm::editor) *and* the
//! embedded HTML of the same parse, so the document is read once.
//!
//! The answer is `{"html": "...", "editor": {...}}`. Each block's first
//! element carries `data-source-start`/`data-source-end` — UTF-16 offsets
//! into the edited source, the unit of the editor view — for scrolling the
//! preview with the editor; blocks of an included file are not marked.
//! [`STYLESHEET`] is the stylesheet the markup is written for.
//!
//! Build: `wasm-pack build crates/adoctra-wasm-preview --target web`.
//! The contract is `docs/editor.adoc`.

use adoctra_core::diagnostic::ParseResult;
use adoctra_core::json::escape_into as write_str;
use adoctra_core::{ResourceResult, SourceId};
use adoctra_html::HtmlBackend;
use adoctra_wasm::editor::Utf16Offsets;
use adoctra_wasm::ResourceWorkspace;

/// The reference stylesheet of the preview's markup (`adoctra.css`).
pub use adoctra_html::STYLESHEET;

/// Parse `src` and return its editor view and embedded HTML.
pub fn preview_json(src: &str) -> String {
    let mut result = adoctra_core::parse(src);
    write_preview(&mut result, src, SourceId(0))
}

/// The preview of a source loaded into `workspace`, with `diagrams` like
/// its `parse_json` when they are enabled.
pub fn workspace_preview_json(
    workspace: &mut ResourceWorkspace,
    source_id: usize,
) -> ResourceResult<String> {
    workspace.parse_with(source_id, |result, text| {
        write_preview(result, text, SourceId(source_id))
    })
}

fn write_preview(result: &mut ParseResult, source: &str, root: SourceId) -> String {
    let offsets = Utf16Offsets::new(source);
    let html = HtmlBackend::embedded()
        .with_source_positions(move |span| {
            (span.source_id == root).then(|| (offsets.utf16(span.start), offsets.utf16(span.end)))
        })
        .render(&result.document)
        .html;
    let mut json = String::from("{\"html\":");
    write_str(&html, &mut json);
    json.push_str(",\"editor\":");
    json.push_str(&adoctra_wasm::serialize_editor_view(result, source, root));
    json.push('}');
    json
}

// ---- wasm glue (only compiled for wasm32) ---------------------------------
//
// The module also exports everything `adoctra-wasm` does (its bindings are
// linked in), so only the preview is added here.

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    /// Parse AsciiDoc → JSON `{ html, editor }`.
    #[wasm_bindgen]
    pub fn preview_json(src: &str) -> String {
        super::preview_json(src)
    }

    /// The preview of a source loaded into a `ResourceWorkspace`.
    #[wasm_bindgen]
    pub fn workspace_preview_json(
        workspace: &mut adoctra_wasm::JsResourceWorkspace,
        source_id: usize,
    ) -> Result<String, JsValue> {
        super::workspace_preview_json(workspace.inner_mut(), source_id)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// The stylesheet of the preview's markup.
    #[wasm_bindgen]
    pub fn stylesheet() -> String {
        super::STYLESHEET.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_with_marked_html_and_the_editor_view() {
        // «Я» is two bytes and one UTF-16 unit: the second paragraph starts
        // at byte 4, unit 3.
        let json = preview_json("Я\n\n*b*\n");
        assert!(
            json.starts_with(r#"{"html":"<p data-source-start=\"0\" data-source-end=\"1\">Я</p>\n<p data-source-start=\"3\" data-source-end=\"6\"><strong>b</strong></p>\n","editor":{"version":1,"source_id":0,"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"start":3,"end":6,"depth":0,"kind":"paragraph"}"#),
            "{json}"
        );
        assert!(json.ends_with("}}"), "{json}");
    }

    #[test]
    fn a_workspace_preview_marks_only_the_edited_source() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("main.adoc", "top\n\ninclude::part.adoc[]\n")
            .unwrap();
        workspace.set("part.adoc", "inner\n").unwrap();
        let id = workspace.load("main.adoc", None).unwrap();
        let json = workspace_preview_json(&mut workspace, id).unwrap();
        assert!(
            json.contains(
                r#"<p data-source-start=\"0\" data-source-end=\"3\">top</p>\n<p>inner</p>"#
            ),
            "{json}"
        );
        assert!(workspace_preview_json(&mut workspace, 99).is_err());
    }

    #[test]
    fn ships_the_reference_stylesheet() {
        assert!(STYLESHEET.contains("font-family"));
    }
}
