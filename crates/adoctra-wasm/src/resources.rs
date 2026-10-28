//! An editor-owned virtual filesystem and immutable source snapshots.
use adoctra_core::{
    MemoryResources, ResourceError, ResourceErrorKind, ResourceResult, SourceFile, SourceId,
    SourceMap,
};

/// Buffers are registered by the host. Loading captures a snapshot with a stable
/// source id; replacing/removing a buffer never invalidates previously loaded
/// snapshots. Includes load the current host buffers; every parse reports the
/// ids of the immutable snapshots it actually read.
#[derive(Default)]
pub struct ResourceWorkspace {
    resources: MemoryResources,
    sources: SourceMap,
}

impl ResourceWorkspace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: &str, text: &str) -> ResourceResult<()> {
        self.resources.insert(name, text)?;
        Ok(())
    }

    /// Original bytes of an image or an encoded include. For includes, source
    /// ids and spans address the decoded UTF-8 snapshot returned by `source`.
    pub fn set_binary(&mut self, name: &str, bytes: &[u8]) -> ResourceResult<()> {
        self.resources.insert_binary(name, bytes)?;
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> ResourceResult<bool> {
        let text = self.resources.remove(name)?.is_some();
        Ok(self.resources.remove_binary(name)?.is_some() || text)
    }

    pub fn load(&mut self, target: &str, relative_to: Option<usize>) -> ResourceResult<usize> {
        self.sources
            .load(target, relative_to.map(SourceId), &self.resources)
            .map(|id| id.0)
    }

    pub fn source(&self, source_id: usize) -> ResourceResult<&SourceFile> {
        self.sources.get(SourceId(source_id)).ok_or_else(|| {
            ResourceError::new(
                ResourceErrorKind::NotFound,
                source_id.to_string(),
                "source id is not registered",
            )
        })
    }

    /// Physical gaps in an incomplete snapshot. Bytes are hexadecimal; offsets
    /// address retained UTF-8 text, while line numbers refer to the original.
    pub fn source_omitted_lines_json(&self, source_id: usize) -> ResourceResult<String> {
        use std::fmt::Write;
        let mut json = String::from("[");
        for (index, line) in self.source(source_id)?.omitted_lines().iter().enumerate() {
            if index != 0 {
                json.push(',');
            }
            write!(
                json,
                "{{\"offset\":{},\"line\":{},\"bytes\":\"",
                line.offset, line.line
            )
            .unwrap();
            for byte in &line.bytes {
                write!(json, "{byte:02x}").unwrap();
            }
            json.push_str("\"}");
        }
        json.push(']');
        Ok(json)
    }

    pub fn parse_json(&mut self, source_id: usize) -> ResourceResult<String> {
        self.source(source_id)?;
        let options = adoctra_core::ParseOptions {
            source_id: SourceId(source_id),
            ..Default::default()
        };
        let result = adoctra_core::parse_resource(&mut self.sources, &options, &self.resources)?;
        Ok(crate::serialize_parse_result(&result))
    }

    pub fn tokens_json(&mut self, source_id: usize) -> ResourceResult<String> {
        self.source(source_id)?;
        let options = adoctra_core::ParseOptions {
            source_id: SourceId(source_id),
            ..Default::default()
        };
        let tokens = adoctra_core::tokens_resource(&mut self.sources, &options, &self.resources)?;
        Ok(crate::serialize_tokens(&tokens))
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(js_name = ResourceWorkspace)]
    pub struct Workspace(super::ResourceWorkspace);

    #[wasm_bindgen(js_class = ResourceWorkspace)]
    impl Workspace {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self(super::ResourceWorkspace::new())
        }

        pub fn set(&mut self, name: &str, text: &str) -> Result<(), JsValue> {
            self.0.set(name, text).map_err(js_error)
        }

        pub fn set_binary(&mut self, name: &str, bytes: &[u8]) -> Result<(), JsValue> {
            self.0.set_binary(name, bytes).map_err(js_error)
        }

        pub fn remove(&mut self, name: &str) -> Result<bool, JsValue> {
            self.0.remove(name).map_err(js_error)
        }

        pub fn load(&mut self, target: &str, relative_to: Option<usize>) -> Result<usize, JsValue> {
            self.0.load(target, relative_to).map_err(js_error)
        }

        pub fn source_name(&self, source_id: usize) -> Result<String, JsValue> {
            self.0
                .source(source_id)
                .map(|source| source.name().to_owned())
                .map_err(js_error)
        }

        pub fn source_text(&self, source_id: usize) -> Result<String, JsValue> {
            self.0
                .source(source_id)
                .map(|source| source.text().to_owned())
                .map_err(js_error)
        }

        pub fn source_is_complete(&self, source_id: usize) -> Result<bool, JsValue> {
            self.0
                .source(source_id)
                .map(|source| source.is_complete())
                .map_err(js_error)
        }

        pub fn source_omitted_lines_json(&self, source_id: usize) -> Result<String, JsValue> {
            self.0
                .source_omitted_lines_json(source_id)
                .map_err(js_error)
        }

        pub fn parse_json(&mut self, source_id: usize) -> Result<String, JsValue> {
            self.0.parse_json(source_id).map_err(js_error)
        }

        pub fn tokens_json(&mut self, source_id: usize) -> Result<String, JsValue> {
            self.0.tokens_json(source_id).map_err(js_error)
        }
    }

    fn js_error(error: adoctra_core::ResourceError) -> JsValue {
        JsValue::from_str(&error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_include_exports_raw_gaps_and_maps_later_tokens_to_original_lines() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("/main.adoc", "include::part.adoc[lines=1;3]")
            .unwrap();
        workspace
            .set_binary("/part.adoc", b"first\r\n\xff\r\n*last*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("last"));
        assert_eq!(workspace.source(1).unwrap().text(), "first\r\n*last*\r\n");
        assert!(!workspace.source(1).unwrap().is_complete());
        assert_eq!(
            workspace.source_omitted_lines_json(1).unwrap(),
            r#"[{"offset":7,"line":2,"bytes":"ff0d0a"}]"#
        );
        let position = workspace.source(1).unwrap().position(8).unwrap();
        assert_eq!((position.line, position.column), (3, 2));
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(tokens.contains(r#""start":7,"end":13"#), "{tokens}");
        assert!(tokens.contains(r#""source_id":1"#), "{tokens}");
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary("/part.adoc", b"first\r\n\xfe\r\n*last*\r\n")
            .unwrap();
        workspace.parse_json(main).unwrap();
        assert_eq!(
            workspace.source_omitted_lines_json(2).unwrap(),
            r#"[{"offset":7,"line":2,"bytes":"fe0d0a"}]"#
        );
        assert_eq!(
            workspace.source_omitted_lines_json(1).unwrap(),
            r#"[{"offset":7,"line":2,"bytes":"ff0d0a"}]"#
        );
        assert_eq!(workspace.source_omitted_lines_json(main).unwrap(), "[]");
        assert!(workspace.source_omitted_lines_json(99).is_err());
    }

    #[test]
    fn finite_includes_export_prefix_completeness_without_replacing_bad_bytes() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set(
                "/main.adoc",
                "include::part.adoc[encoding=Shift_JIS,lines=2]",
            )
            .unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x95\x5c*\r\n\x81")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("表"), "{json}");
        let source = workspace.source(1).unwrap();
        assert!(!source.is_complete());
        assert_eq!(source.text(), "ignored\r\n*表*\r\n");
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(tokens.contains(r#""source_id":1"#), "{tokens}");
        assert!(tokens.contains(r#""start":9,"end":14"#), "{tokens}");
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x95\x5c*\r\n")
            .unwrap();
        let updated = workspace.parse_json(main).unwrap();
        assert!(updated.contains(r#""diagnostics":[]"#), "{updated}");
        assert!(workspace.source(2).unwrap().is_complete());
        assert_eq!(
            workspace.source(2).unwrap().text(),
            workspace.source(1).unwrap().text()
        );
        assert!(!workspace.source(1).unwrap().is_complete());
    }

    #[test]
    fn encoded_binary_includes_export_decoded_snapshots_ast_and_tokens() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set(
                "/main.adoc",
                "include::part.adoc[encoding=UTF-16LE,lines=2]",
            )
            .unwrap();
        let text = "ignored\r\n*Ж🙂*\r\n";
        let bytes: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        workspace.set_binary("/part.adoc", &bytes).unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("Ж🙂"), "{json}");
        assert_eq!(workspace.source(1).unwrap().text(), text);
        assert_eq!(workspace.source(1).unwrap().name(), "/part.adoc");
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(tokens.contains(r#""source_id":1"#), "{tokens}");
        assert!(tokens.contains(r#""start":9,"end":17"#), "{tokens}");
        assert_eq!(&text[9..17], "*Ж🙂*");
        assert_eq!(workspace.source(1).unwrap().text(), text);
        let replacement = "ignored\r\n*новый*\r\n";
        let bytes: Vec<_> = replacement
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        workspace.set_binary("/part.adoc", &bytes).unwrap();
        let updated = workspace.parse_json(main).unwrap();
        assert!(updated.contains("новый"), "{updated}");
        assert_eq!(workspace.source(1).unwrap().text(), text);
        assert_eq!(workspace.source(2).unwrap().text(), replacement);
    }

    #[test]
    fn japanese_includes_export_utf8_offsets_and_preserve_variant_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=Shift_JIS,lines=2]\n\ninclude::part.adoc[encoding=SJIS,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x95\x5c\x81\x60*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("表〜"), "{json}");
        assert!(json.contains("表～"), "{json}");
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*表〜*\r\n");
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*表～*\r\n");
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(tokens.contains(r#""start":9,"end":17"#), "{tokens}");
        assert!(tokens.contains(r#""source_id":1"#), "{tokens}");
        assert!(tokens.contains(r#""source_id":2"#), "{tokens}");

        workspace
            .set("/main.adoc", "include::part.adoc[encoding=EUC-JP,lines=2]")
            .unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xc9\xbd\x8f\xa2\xaf*\r\n")
            .unwrap();
        let updated_main = workspace.load("/main.adoc", None).unwrap();
        let updated = workspace.parse_json(updated_main).unwrap();
        assert!(updated.contains("表˘"), "{updated}");
        assert!(updated.contains(r#""diagnostics":[]"#), "{updated}");
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*表〜*\r\n");
    }

    #[test]
    fn binary_buffers_embed_images_with_data_uri() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("docs/main.adoc", ":data-uri:\n\nimage::i/a.png[]")
            .unwrap();
        workspace.set_binary("docs/i/a.png", b"foo").unwrap();
        let main = workspace.load("docs/main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        assert!(
            parsed.contains(r#""src":"data:image/png;base64,Zm9v""#),
            "{parsed}"
        );
        assert!(workspace.remove("docs/i/a.png").unwrap());
        let parsed = workspace.parse_json(main).unwrap();
        assert!(
            parsed.contains(r#""src":"data:image/png;base64,""#),
            "{parsed}"
        );
        assert!(
            parsed.contains("image to embed not found or not readable: /docs/i/a.png"),
            "{parsed}"
        );
    }

    #[test]
    fn virtual_document_paths_reach_includes_json_and_original_reference_spans() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("docs/главная.adoc", "include::{docdir}/part.adoc[]")
            .unwrap();
        let text = "\u{feff}Я {docname} {docfilesuffix}\r\n";
        workspace.set("docs/part.adoc", text).unwrap();
        let main = workspace.load("docs/главная.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        assert!(
            parsed.contains(r#""attribute":"docname","value":"главная""#),
            "{parsed}"
        );
        assert!(
            parsed.contains(r#""attribute":"docfilesuffix","value":".adoc""#),
            "{parsed}"
        );
        assert!(parsed.contains(r#""diagnostics":[]"#), "{parsed}");
        let child = workspace.load("part.adoc", Some(main)).unwrap();
        let tokens = workspace.tokens_json(main).unwrap();
        for reference in ["{docname}", "{docfilesuffix}"] {
            let start = text.find(reference).unwrap();
            let end = start + reference.len();
            let span = format!(r#""source_id":{child},"start":{start},"end":{end}"#);
            assert!(parsed.contains(&span), "{parsed}");
            assert!(tokens.contains(&span), "{tokens}");
        }
        // Parsing the child as the primary document derives its own paths.
        assert!(workspace
            .parse_json(child)
            .unwrap()
            .contains(r#""attribute":"docname","value":"part""#));
        workspace.remove("docs/главная.adoc").unwrap();
        assert_eq!(workspace.parse_json(main).unwrap(), parsed);
        assert_eq!(workspace.source(child).unwrap().text(), text);
    }

    #[test]
    fn editor_buffers_resolve_relative_paths_and_export_source_aware_json() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("docs/main.adoc", "= Main\n\nbody").unwrap();
        workspace
            .set("docs/часть.adoc", "Я *текст* <<missing>>\r\n")
            .unwrap();
        let main = workspace.load("/docs/main.adoc", None).unwrap();
        let child = workspace.load("часть.adoc", Some(main)).unwrap();
        assert_eq!(workspace.source(child).unwrap().name(), "/docs/часть.adoc");
        assert_eq!(
            workspace.source(child).unwrap().text(),
            "Я *текст* <<missing>>\r\n"
        );
        let parsed = workspace.parse_json(child).unwrap();
        let tokens = workspace.tokens_json(child).unwrap();
        let expected_span = format!(r#""span":{{"source_id":{child},"start":3,"end":15}}"#);
        assert!(parsed.contains(&expected_span), "{parsed}");
        assert!(tokens.contains(&expected_span), "{tokens}");
        assert!(parsed.contains("missing"));
        assert_eq!(
            workspace
                .source(child)
                .unwrap()
                .position(16)
                .unwrap()
                .column,
            11
        );
        assert_eq!(workspace.load("./часть.adoc", Some(main)).unwrap(), child);
    }

    #[test]
    fn buffer_edits_and_deletion_preserve_previously_loaded_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("doc.adoc", "old").unwrap();
        let old = workspace.load("doc.adoc", None).unwrap();
        workspace.set("doc.adoc", "new").unwrap();
        let new = workspace.load("doc.adoc", None).unwrap();
        assert_ne!(old, new);
        assert!(workspace.parse_json(old).unwrap().contains("old"));
        assert!(workspace.parse_json(new).unwrap().contains("new"));
        assert!(workspace.remove("/doc.adoc").unwrap());
        assert!(!workspace.remove("/doc.adoc").unwrap());
        assert_eq!(
            workspace.load("doc.adoc", None).unwrap_err().kind,
            ResourceErrorKind::NotFound
        );
        assert_eq!(workspace.source(new).unwrap().text(), "new");
        assert_eq!(
            workspace.parse_json(999).unwrap_err().kind,
            ResourceErrorKind::NotFound
        );
        assert_eq!(
            workspace.tokens_json(999).unwrap_err().kind,
            ResourceErrorKind::NotFound
        );
    }
}

#[cfg(test)]
mod include_tests {
    use super::ResourceWorkspace;

    #[test]
    fn normalized_content_exports_physical_positions_and_keeps_raw_spaces() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set(
                "main.adoc",
                "include::part.adoc[]\n\n----\ninclude::raw.rs[]\n----",
            )
            .unwrap();
        let text = "\u{feff}*Я* \t\r\n_🙂_ \t\0\r\n";
        workspace.set("part.adoc", text).unwrap();
        workspace.set("raw.rs", "X \t\r\n").unwrap();
        let main = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        let tokens = workspace.tokens_json(main).unwrap();
        let child = workspace.load("part.adoc", None).unwrap();
        for value in ["*Я*", "_🙂_"] {
            let start = text.find(value).unwrap();
            let end = start + value.len();
            let span = format!(r#""source_id":{child},"start":{start},"end":{end}"#);
            assert!(parsed.contains(&span), "{parsed}");
            assert!(tokens.contains(&span), "{tokens}");
        }
        assert!(parsed.contains(r#""text":"X \t""#), "{parsed}");
        assert!(parsed.contains("\"provenance\":[["), "{parsed}");
        assert!(!parsed.contains("\\u0000"), "{parsed}");
        assert_eq!(workspace.source(child).unwrap().text(), text);
    }

    #[test]
    fn included_spans_provenance_tokens_and_buffer_versions_are_exported() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set(
                "main.adoc",
                "*before\ninclude::part.adoc[]\nafter*\n\ninclude::diagnostic.adoc[]",
            )
            .unwrap();
        workspace.set("part.adoc", "_Я_").unwrap();
        workspace.set("diagnostic.adoc", "<<missing>>").unwrap();
        let main = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        assert!(parsed.contains("\"include_directives\":"), "{parsed}");
        assert!(parsed.contains("\"provenance\":[["), "{parsed}");
        assert!(parsed.contains("\"provenance\":0"), "{parsed}");
        let child = workspace.load("part.adoc", Some(main)).unwrap();
        let diagnostic = workspace.load("diagnostic.adoc", Some(main)).unwrap();
        assert!(
            parsed.contains(&format!(r#""source_id":{diagnostic},"start":0,"end":11"#)),
            "{parsed}"
        );
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(tokens.contains("include-directive"), "{tokens}");
        assert!(
            tokens.contains(&format!(r#""source_id":{child}"#)),
            "{tokens}"
        );
        assert!(!tokens.contains("provenance"), "{tokens}");
        workspace.set("part.adoc", "edited").unwrap();
        let edited = workspace.parse_json(main).unwrap();
        let new = workspace.load("part.adoc", Some(main)).unwrap();
        assert_ne!(child, new);
        assert!(edited.contains("edited"));
        assert_eq!(workspace.source(child).unwrap().text(), "_Я_");
        assert_eq!(workspace.source(new).unwrap().text(), "edited");
    }
}

#[cfg(test)]
mod selection_tests {
    use super::ResourceWorkspace;

    #[test]
    fn selected_fragments_warnings_and_edited_snapshots_retain_physical_positions() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("main.adoc", "include::part.adoc[tag=a]")
            .unwrap();
        let text = "\u{feff}omitted\r\n// tag::a[]\r\n*Я*\r\n// end::a[]\r\nomitted\r\n// tag::a[]\r\n_ты_";
        workspace.set("part.adoc", text).unwrap();
        let main = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        let child = workspace.load("part.adoc", Some(main)).unwrap();
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(parsed.contains("\"provenance\":[["), "{parsed}");
        assert!(parsed.contains("unclosed tag 'a'"), "{parsed}");
        assert!(!parsed.contains("omitted"), "{parsed}");
        assert!(tokens.contains("include-directive"), "{tokens}");
        for marker in ["*Я*", "_ты_"] {
            let start = text.find(marker).unwrap();
            let end = start + marker.len();
            let span = format!(r#""source_id":{child},"start":{start},"end":{end}"#);
            assert!(parsed.contains(&span), "{parsed}");
            assert!(tokens.contains(&span), "{tokens}");
        }
        let start = text.rfind("// tag::a[]").unwrap();
        let end = start + "// tag::a[]".len();
        assert!(
            parsed.contains(&format!(
                r#""source_id":{child},"start":{start},"end":{end}"#
            )),
            "{parsed}"
        );
        workspace
            .set("part.adoc", "// tag::a[]\nedited\n// end::a[]")
            .unwrap();
        assert!(workspace.parse_json(main).unwrap().contains("edited"));
        let edited = workspace.load("part.adoc", Some(main)).unwrap();
        assert_ne!(child, edited);
        assert_eq!(workspace.source(child).unwrap().text(), text);
    }
}

#[cfg(test)]
mod offset_tests {
    use super::ResourceWorkspace;

    #[test]
    fn optional_and_offset_export_levels_physical_positions_and_immutable_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("main.adoc", "= Main\n\ninclude::missing.adoc[opts=optional]\ninclude::part.adoc[leveloffset=+1]\n\n== After").unwrap();
        let text = "\u{feff}= Глава\r\n\r\n== Раздел";
        workspace.set("part.adoc", text).unwrap();
        let main = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        let child = workspace.load("part.adoc", Some(main)).unwrap();
        let tokens = workspace.tokens_json(main).unwrap();
        let span = format!(r#""source_id":{child},"start":3,"end":15"#);
        assert!(parsed.contains(&span), "{parsed}");
        assert!(tokens.contains(&span), "{tokens}");
        assert!(
            parsed.contains(r#""name":"heading","level":1,"id":"_глава""#),
            "{parsed}"
        );
        assert!(
            parsed.contains(r#""name":"heading","level":2,"id":"_раздел""#),
            "{parsed}"
        );
        assert!(parsed.contains(r#""diagnostics":[]"#), "{parsed}");
        assert_eq!(tokens.matches("include-directive").count(), 2);
        assert!(!tokens.contains("attribute-entry"), "{tokens}");
        workspace.set("part.adoc", "== Edited").unwrap();
        let edited = workspace.parse_json(main).unwrap();
        assert!(
            edited.contains(r#""name":"heading","level":2,"id":"_edited""#),
            "{edited}"
        );
        assert_eq!(workspace.source(child).unwrap().text(), text);
    }
}

#[cfg(test)]
mod indent_depth_tests {
    use super::ResourceWorkspace;

    #[test]
    fn transformed_text_exports_original_spans_and_scoped_depth_diagnostics() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("main.adoc", "include::part.adoc[indent=0,depth=0]")
            .unwrap();
        let text = "\u{feff}  *Я*\r\n\r\n  include::leaf.adoc[]";
        workspace.set("part.adoc", text).unwrap();
        workspace.set("leaf.adoc", "unreachable").unwrap();
        let main = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        let child = workspace.load("part.adoc", Some(main)).unwrap();
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            parsed.contains("maximum include depth of 0 exceeded"),
            "{parsed}"
        );
        assert!(!parsed.contains("unreachable"), "{parsed}");
        for marker in ["*Я*", "include::leaf.adoc[]"] {
            let start = text.find(marker).unwrap();
            let end = start + marker.len();
            let span = format!(r#""source_id":{child},"start":{start},"end":{end}"#);
            assert!(parsed.contains(&span), "{parsed}");
            assert!(tokens.contains(&span), "{tokens}");
        }
        workspace.set("part.adoc", "  _Edited_").unwrap();
        let edited = workspace.parse_json(main).unwrap();
        assert!(edited.contains("Edited"), "{edited}");
        assert!(edited.contains(r#""diagnostics":[]"#), "{edited}");
        assert_eq!(workspace.source(child).unwrap().text(), text);
    }
}

#[cfg(test)]
mod multiline_attribute_tests {
    use super::ResourceWorkspace;

    #[test]
    fn expanded_values_and_attribute_tokens_keep_physical_sources() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("main.adoc", ":x: Я \\\ninclude::part.adoc[]\n\n{x}")
            .unwrap();
        let child = "\u{feff}  🙂 \\ \t\r\n  конец";
        workspace.set("part.adoc", child).unwrap();
        let main_id = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main_id).unwrap();
        let child_id = workspace.load("part.adoc", Some(main_id)).unwrap();
        assert!(
            parsed.contains(r#""attribute":"x","value":"Я 🙂 конец""#),
            "{parsed}"
        );
        assert!(parsed.contains(r#""diagnostics":[]"#), "{parsed}");
        let tokens = workspace.tokens_json(main_id).unwrap();
        for text in ["  🙂 \\", "  конец"] {
            let start = child.find(text).unwrap();
            let end = start + text.len();
            let token = format!(
                r#""source_id":{child_id},"start":{start},"end":{end}}},"kind":"attribute-entry""#
            );
            assert!(tokens.contains(&token), "{tokens}");
        }
        workspace.set("part.adoc", "  edited").unwrap();
        let edited = workspace.parse_json(main_id).unwrap();
        assert!(edited.contains(r#""value":"Я edited""#), "{edited}");
        assert_eq!(workspace.source(child_id).unwrap().text(), child);
    }
}

#[cfg(test)]
mod include_attribute_list_tests {
    use super::ResourceWorkspace;

    #[test]
    fn grammar_and_substitution_export_selected_nodes_and_physical_warnings() {
        let mut workspace = ResourceWorkspace::new();
        let main = "\u{feff}:selection: lines=\"2,4\"\r\n:attribute-missing: warn\r\n\r\ninclude::part.adoc[ignored,{selection},indent=0,other={missing}]";
        let child = "\u{feff}omitted\r\n  *Я*\r\nomitted\r\n  _🙂_\r\n";
        workspace.set("main.adoc", main).unwrap();
        workspace.set("part.adoc", child).unwrap();
        let main_id = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main_id).unwrap();
        let child_id = workspace.load("part.adoc", Some(main_id)).unwrap();
        let tokens = workspace.tokens_json(main_id).unwrap();
        assert!(
            parsed.contains("skipping reference to missing attribute: missing"),
            "{parsed}"
        );
        assert!(!parsed.contains("omitted"), "{parsed}");
        assert!(parsed.contains("provenance"), "{parsed}");
        for marker in ["*Я*", "_🙂_"] {
            let start = child.find(marker).unwrap();
            let end = start + marker.len();
            let span = format!(r#""source_id":{child_id},"start":{start},"end":{end}"#);
            assert!(parsed.contains(&span), "{parsed}");
            assert!(tokens.contains(&span), "{tokens}");
        }
        let start = main.find("include::").unwrap();
        let end = main.len();
        let directive_span = format!(r#""source_id":{main_id},"start":{start},"end":{end}"#);
        assert!(parsed.contains(&directive_span), "{parsed}");
        assert!(tokens.contains(&directive_span), "{tokens}");
        workspace.set("part.adoc", "omit\n  edited").unwrap();
        assert!(workspace.parse_json(main_id).unwrap().contains("edited"));
        assert_eq!(workspace.source(child_id).unwrap().text(), child);
    }
}

#[cfg(test)]
mod include_target_tests {
    use super::ResourceWorkspace;

    #[test]
    fn target_policies_export_physical_warnings_tokens_and_selected_content() {
        let mut workspace = ResourceWorkspace::new();
        let main = ":attribute-missing: warn\n\ninclude::part.adoc[lines=2,indent=0]";
        let child = "\u{feff}omitted\r\n  include::{missing}leaf.adoc[]\r\n";
        workspace.set("main.adoc", main).unwrap();
        workspace.set("part.adoc", child).unwrap();
        workspace.set("leaf.adoc", "*Я*").unwrap();
        let main_id = workspace.load("main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main_id).unwrap();
        let child_id = workspace.load("part.adoc", Some(main_id)).unwrap();
        assert!(
            parsed.contains("include dropped due to missing attribute"),
            "{parsed}"
        );
        assert!(!parsed.contains("Я"), "{parsed}");
        let start = child.find("include::").unwrap();
        let end = child.len() - 2;
        let span = format!(r#""source_id":{child_id},"start":{start},"end":{end}"#);
        assert!(parsed.contains(&span), "{parsed}");
        let tokens = workspace.tokens_json(main_id).unwrap();
        assert!(tokens.contains(&span), "{tokens}");
        assert!(tokens.contains("include-directive"), "{tokens}");
        workspace
            .set("main.adoc", &main.replace("warn", "drop"))
            .unwrap();
        let edited_id = workspace.load("main.adoc", None).unwrap();
        let edited = workspace.parse_json(edited_id).unwrap();
        assert!(edited.contains("Я"), "{edited}");
        assert!(!edited.contains("include dropped"), "{edited}");
        assert_eq!(workspace.source(main_id).unwrap().text(), main);
        assert_eq!(workspace.source(child_id).unwrap().text(), child);
        assert!(workspace
            .parse_json(main_id)
            .unwrap()
            .contains("include dropped"));
    }
}
