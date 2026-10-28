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
    fn bom_reader_frames_export_positions_without_fabricating_snapshot_newlines() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("/main.adoc", "include::part.adoc[lines=1;2]")
            .unwrap();
        let mut bytes = vec![0xfe, 0xff];
        for unit in "*first* 上second".encode_utf16() {
            bytes.extend(unit.to_be_bytes());
        }
        workspace.set_binary("/part.adoc", &bytes).unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "\u{feff}*first* 上second"
        );
        assert!(!workspace.source(1).unwrap().is_complete());
        assert_eq!(
            workspace.source(1).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 1 }
        );
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(tokens.contains(r#""source_id":1"#), "{tokens}");
        assert!(workspace
            .parse_json(main)
            .unwrap()
            .contains(r#""diagnostics":[]"#));
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary("/part.adoc", &bytes[..bytes.len() - 12])
            .unwrap();
        assert!(workspace
            .parse_json(main)
            .unwrap()
            .contains(r#""diagnostics":[]"#));
        assert!(workspace.source(2).is_ok());
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "\u{feff}*first* 上second"
        );
    }

    #[test]
    fn skipped_implicit_bom_exports_raw_nuls_omissions_and_immutable_spans() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set(
                "/main.adoc",
                "include::part.adoc[lines=2]\n\ninclude::part.adoc[encoding=UTF-16LE,lines=2]",
            )
            .unwrap();
        let mut bytes = vec![0xff, 0xfe];
        for unit in "*one*\n*two*\n*three*\n".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        workspace.set_binary("/part.adoc", &bytes).unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains(r"\u0000"), "{json}");
        assert_eq!(workspace.source(1).unwrap().text(), "\0*\0t\0w\0o\0*\0\n");
        assert_eq!(
            workspace.source(1).unwrap().omitted_lines()[0].bytes,
            bytes[..13]
        );
        assert_eq!(
            workspace.source(1).unwrap().position(4).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 5 }
        );
        assert_eq!(
            workspace.source(2).unwrap().text(),
            "\u{feff}*one*\n*two*\n*three*\n"
        );
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(tokens.contains(r#""source_id":1"#), "{tokens}");
        assert!(workspace
            .parse_json(main)
            .unwrap()
            .contains(r#""diagnostics":[]"#));
        assert!(workspace.source(3).is_err());
        workspace.set_binary("/part.adoc", &bytes[2..]).unwrap();
        assert!(workspace
            .parse_json(main)
            .unwrap()
            .contains(r#""diagnostics":[]"#));
        assert!(workspace.source(3).is_ok());
        assert_eq!(workspace.source(1).unwrap().text(), "\0*\0t\0w\0o\0*\0\n");
    }

    #[test]
    fn utf8_mac_exported_snapshots_compose_and_reuse_equivalent_input() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc","include::part.adoc[encoding=UTF8-MAC,lines=2]\n\ninclude::part.adoc[encoding=UTF-8-HFS,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", "ignored\r\n*e\u{301}*\r\n".as_bytes())
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        for input in ["ignored\r\n*e\u{301}*\r\n", "ignored\r\n*é*\r\n"] {
            workspace
                .set_binary("/part.adoc", input.as_bytes())
                .unwrap();
            let json = workspace.parse_json(main).unwrap();
            assert!(
                json.contains(r#""diagnostics":[]"#) && json.contains('é'),
                "{json}"
            );
            assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*é*\r\n");
            assert_eq!(
                workspace.source(1).unwrap().position(12).unwrap(),
                adoctra_core::SourcePosition { line: 2, column: 3 }
            );
            let tokens = workspace.tokens_json(main).unwrap();
            assert!(
                tokens.contains(r#""source_id":1,"start":9,"end":13"#),
                "{tokens}"
            );
            assert!(workspace.source(2).is_err());
        }
        workspace
            .set_binary(
                "/part.adoc",
                [
                    "ignored\r\n*\u{3b1}\u{313}\u{300}\u{345}*\r\n".as_bytes(),
                    b"A\xff",
                ]
                .concat()
                .as_slice(),
            )
            .unwrap();
        let prefix = workspace.parse_json(main).unwrap();
        assert!(
            prefix.contains(r#""diagnostics":[]"#) && prefix.contains('\u{1f82}'),
            "{prefix}"
        );
        assert_eq!(
            workspace.source(2).unwrap().text(),
            "ignored\r\n*\u{1f82}*\r\n"
        );
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*é*\r\n");
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn cesu8_exported_tokens_and_immutable_snapshots_address_decoded_utf8() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc","include::part.adoc[encoding=CESU-8,lines=2]\n\ninclude::part.adoc[encoding=cesu-8,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xed\xa0\xbd\xed\xb8\x80*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(
            json.contains(r#""diagnostics":[]"#) && json.contains("😀"),
            "{json}"
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*😀*\r\n");
        assert_eq!(
            workspace.source(1).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":15"#),
            "{tokens}"
        );
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary("/part.adoc", "ignored\r\n*😀*\r\n".as_bytes())
            .unwrap();
        let invalid = workspace.parse_json(main).unwrap();
        assert!(!invalid.contains(r#""diagnostics":[]"#), "{invalid}");
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary(
                "/part.adoc",
                b"ignored\r\n*\xed\xaf\xbf\xed\xbf\xbf*\r\n\xed\xa0\x80",
            )
            .unwrap();
        let prefix = workspace.parse_json(main).unwrap();
        assert!(
            prefix.contains(r#""diagnostics":[]"#) && prefix.contains('\u{10ffff}'),
            "{prefix}"
        );
        assert_eq!(
            workspace.source(2).unwrap().text(),
            "ignored\r\n*\u{10ffff}*\r\n"
        );
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*😀*\r\n");
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn utf8_kddi_duplicate_banks_share_exported_snapshots_and_unicode_positions() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=UTF8-KDDI,lines=2]\n\ninclude::part.adoc[encoding=utf8-kddi,lines=2]").unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        for flag in ["\u{e4cc}", "\u{efa5}", "🇯🇵"] {
            workspace
                .set_binary("/part.adoc", format!("ignored\r\n*{flag}*\r\n").as_bytes())
                .unwrap();
            let json = workspace.parse_json(main).unwrap();
            assert!(
                json.contains(r#""diagnostics":[]"#) && json.contains("🇯🇵"),
                "{json}"
            );
            assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
            assert_eq!(
                workspace.source(1).unwrap().position(18).unwrap(),
                adoctra_core::SourcePosition { line: 2, column: 4 }
            );
            assert!(workspace.source(2).is_err());
            let tokens = workspace.tokens_json(main).unwrap();
            assert!(
                tokens.contains(r#""source_id":1,"start":9,"end":19"#),
                "{tokens}"
            );
        }
        workspace
            .set_binary("/part.adoc", "ignored\r\n*\u{effb}*\r\n\u{e472}".as_bytes())
            .unwrap();
        let prefix = workspace.parse_json(main).unwrap();
        assert!(
            prefix.contains(r#""diagnostics":[]"#) && prefix.contains("1\u{20e3}"),
            "{prefix}"
        );
        assert_eq!(
            workspace.source(2).unwrap().text(),
            "ignored\r\n*1\u{20e3}*\r\n"
        );
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(
            workspace.source(2).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 4 }
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn utf8_docomo_exported_snapshots_share_equivalent_unicode_and_validate_prefixes() {
        utf8_mobile_exported_snapshots(
            "UTF8-DoCoMo",
            "utf8-docomo",
            "\u{e6e2}",
            "\u{e66c}",
            "\u{e6f1}",
        );
    }

    #[test]
    fn utf8_softbank_exported_snapshots_share_equivalent_unicode_and_validate_prefixes() {
        utf8_mobile_exported_snapshots(
            "UTF8-SoftBank",
            "utf8-softbank",
            "\u{e21c}",
            "\u{e14f}",
            "\u{e059}",
        );
    }

    fn utf8_mobile_exported_snapshots(
        name: &str,
        lower: &str,
        keycap: &str,
        supplementary: &str,
        undefined: &str,
    ) {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", &format!("include::part.adoc[encoding={name},lines=2]\n\ninclude::part.adoc[encoding={lower},lines=2]")).unwrap();
        workspace
            .set_binary(
                "/part.adoc",
                format!("ignored\r\n*{keycap}*\r\n").as_bytes(),
            )
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(
            json.contains(r#""diagnostics":[]"#) && json.contains("1\u{20e3}"),
            "{json}"
        );
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "ignored\r\n*1\u{20e3}*\r\n"
        );
        assert_eq!(
            workspace.source(1).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 4 }
        );
        assert!(workspace.source(2).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":15"#),
            "{tokens}"
        );
        workspace
            .set_binary("/part.adoc", "ignored\r\n*1\u{20e3}*\r\n".as_bytes())
            .unwrap();
        let equivalent = workspace.parse_json(main).unwrap();
        assert!(equivalent.contains(r#""diagnostics":[]"#), "{equivalent}");
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary(
                "/part.adoc",
                format!("ignored\r\n*{supplementary}*\r\n{undefined}").as_bytes(),
            )
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains(r#""diagnostics":[]"#) && changed.contains("🅿"),
            "{changed}"
        );
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*🅿*\r\n");
        assert_eq!(
            workspace.source(2).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "ignored\r\n*1\u{20e3}*\r\n"
        );
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn softbank_duplicate_encodings_share_exported_immutable_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=SJIS-SoftBank,lines=2]\n\ninclude::part.adoc[encoding=sjis-softbank,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xf6\xe4*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(
            json.contains(r#""diagnostics":[]"#) && json.contains("🇯🇵"),
            "{json}"
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
        assert_eq!(
            workspace.source(1).unwrap().position(18).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 4 }
        );
        assert!(workspace.source(2).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":19"#),
            "{tokens}"
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xfb\xab*\r\n")
            .unwrap();
        let duplicate = workspace.parse_json(main).unwrap();
        assert!(
            duplicate.contains(r#""diagnostics":[]"#) && duplicate.contains("🇯🇵"),
            "{duplicate}"
        );
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xf2\xe5*\r\n\xf9\x9a")
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains(r#""diagnostics":[]"#) && changed.contains("1\u{20e3}"),
            "{changed}"
        );
        assert_eq!(
            workspace.source(2).unwrap().text(),
            "ignored\r\n*1\u{20e3}*\r\n"
        );
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn docomo_keycaps_export_scalar_positions_and_immutable_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=SJIS-DoCoMo,lines=2]\n\ninclude::part.adoc[encoding=sjis-docomo,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xf9\x87*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(
            json.contains(r#""diagnostics":[]"#) && json.contains("1\u{20e3}"),
            "{json}"
        );
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "ignored\r\n*1\u{20e3}*\r\n"
        );
        assert_eq!(
            workspace.source(1).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 4 }
        );
        assert!(workspace.source(2).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":15"#),
            "{tokens}"
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xf8\xcd*\r\n\xf9\x96")
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains(r#""diagnostics":[]"#) && changed.contains("🅿"),
            "{changed}"
        );
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*🅿*\r\n");
        assert_eq!(
            workspace.source(2).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "ignored\r\n*1\u{20e3}*\r\n"
        );
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn sjis_kddi_compounds_export_scalar_positions_and_immutable_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=SJIS-KDDI,lines=2]\n\ninclude::part.adoc[encoding=sjis-kddi,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xf6\xa5*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(
            json.contains(r#""diagnostics":[]"#) && json.contains("🇯🇵"),
            "{json}"
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
        assert_eq!(
            workspace.source(1).unwrap().position(18).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 4 }
        );
        assert!(workspace.source(2).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":19"#),
            "{tokens}"
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xf6\x7e*\r\n\xf6\x4a")
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains(r#""diagnostics":[]"#) && changed.contains("🅿"),
            "{changed}"
        );
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*🅿*\r\n");
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn jis2004_aliases_export_supplementary_spans_and_immutable_prefix_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=EUC-JIS-2004,lines=2]\n\ninclude::part.adoc[encoding=EUC-JISX0213,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x8f\xa1\xa1*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(
            json.contains(r#""diagnostics":[]"#) && json.contains("𠂉"),
            "{json}"
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*𠂉*\r\n");
        assert!(workspace.source(2).is_err());
        assert_eq!(
            workspace.source(1).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":15"#),
            "{tokens}"
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x8f\xfe\xf6*\r\n\xa4\xf7")
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains(r#""diagnostics":[]"#) && changed.contains("𪚲"),
            "{changed}"
        );
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*𪚲*\r\n");
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*𠂉*\r\n");
        assert!(workspace.source(3).is_err());
    }

    #[test]
    fn kddi_compounds_export_scalar_positions_tokens_and_immutable_snapshots() {
        for (name, flag, supplementary, bad) in [
            (
                "ISO-2022-JP-KDDI",
                &b"\x1b$Bv'\x1b(B"[..],
                &b"\x1b$Bu_\x1b(B"[..],
                &b"\x1b$"[..],
            ),
            (
                "stateless-ISO-2022-JP-KDDI",
                &b"\x92\xf6\xa7"[..],
                &b"\x92\xf5\xdf"[..],
                &b"\x92\xf6"[..],
            ),
        ] {
            let mut workspace = ResourceWorkspace::new();
            workspace.set("/main.adoc", &format!("include::part.adoc[encoding={name},lines=2]\n\ninclude::part.adoc[encoding={},lines=2]", name.to_uppercase())).unwrap();
            workspace
                .set_binary(
                    "/part.adoc",
                    &[&b"ignored\r\n*"[..], flag, b"*\r\n"].concat(),
                )
                .unwrap();
            let main = workspace.load("/main.adoc", None).unwrap();
            let json = workspace.parse_json(main).unwrap();
            assert!(
                json.contains(r#""diagnostics":[]"#) && json.contains("🇯🇵"),
                "{json}"
            );
            assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
            assert!(workspace.source(2).is_err());
            assert_eq!(
                workspace.source(1).unwrap().position(18).unwrap(),
                adoctra_core::SourcePosition { line: 2, column: 4 }
            );
            let tokens = workspace.tokens_json(main).unwrap();
            assert!(
                tokens.contains(r#""source_id":1,"start":9,"end":19"#),
                "{tokens}"
            );
            workspace
                .set_binary(
                    "/part.adoc",
                    &[&b"ignored\r\n*"[..], supplementary, b"*\r\n", bad].concat(),
                )
                .unwrap();
            let changed = workspace.parse_json(main).unwrap();
            assert!(
                changed.contains(r#""diagnostics":[]"#) && changed.contains("🅿"),
                "{changed}"
            );
            assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*🅿*\r\n");
            assert!(!workspace.source(2).unwrap().is_complete());
            assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*🇯🇵*\r\n");
        }
    }

    #[test]
    fn stateless_jp_includes_export_utf8_spans_and_share_prefix_variants() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=stateless-ISO-2022-JP,lines=2]\n\ninclude::part.adoc[encoding=STATELESS-ISO-2022-JP,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x92\xa4\xa2*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(
            json.contains(r#""diagnostics":[]"#) && json.contains("あ"),
            "{json}"
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*あ*\r\n");
        assert!(workspace.source(2).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":14"#),
            "{tokens}"
        );
        assert_eq!(
            workspace.source(1).unwrap().position(13).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x90\xa4\xa2*\r\n")
            .unwrap();
        let same = workspace.parse_json(main).unwrap();
        assert!(same.contains(r#""diagnostics":[]"#), "{same}");
        assert!(workspace.source(2).is_err());
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x90\xa4\xa4*\r\n\x0f")
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains(r#""diagnostics":[]"#) && changed.contains("い"),
            "{changed}"
        );
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*い*\r\n");
        assert!(!workspace.source(2).unwrap().is_complete());
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*あ*\r\n");
    }

    #[test]
    fn cp5022_includes_share_variants_and_export_decoded_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=CP50220,lines=2]\n\ninclude::part.adoc[encoding=CP50221,lines=2]\n\ninclude::part.adoc[encoding=ISO-2022-JP,lines=2,opts=optional]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x1b$B!A\x1b(B*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("～") && json.contains("〜"), "{json}");
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*～*\r\n");
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*〜*\r\n");
        assert!(workspace.source(3).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":14"#),
            "{tokens}"
        );
        assert_eq!(
            workspace.source(1).unwrap().position(13).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x0e1\x0f*\r\n\x1b$")
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains("ｱ") && changed.contains("InvalidEncoding"),
            "{changed}"
        );
        assert_eq!(workspace.source(3).unwrap().text(), "ignored\r\n*ｱ*\r\n");
        assert!(!workspace.source(3).unwrap().is_complete());
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*～*\r\n");
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*〜*\r\n");
    }

    #[test]
    fn iso2022_includes_export_decoded_spans_and_immutable_alias_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=ISO-2022-JP,lines=2]\n\ninclude::part.adoc[encoding=ISO2022-JP,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x1b$B$\"\x1b(B*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("あ"), "{json}");
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*あ*\r\n");
        assert!(workspace.source(2).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":14"#),
            "{tokens}"
        );
        assert_eq!(
            workspace.source(1).unwrap().position(13).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x1b$BF|\x1b(B*\r\n\x1b$")
            .unwrap();
        let updated = workspace.parse_json(main).unwrap();
        assert!(
            updated.contains("日") && updated.contains(r#""diagnostics":[]"#),
            "{updated}"
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*あ*\r\n");
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*日*\r\n");
        assert!(!workspace.source(2).unwrap().is_complete());
    }

    #[test]
    fn korean_includes_export_utf8_and_preserve_alias_identity() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=EUC-KR,lines=2]\n\ninclude::part.adoc[encoding=eucKR,lines=2]\n\ninclude::part.adoc[encoding=CP949,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xb0\xa1*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("가"));
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*가*\r\n");
        assert!(workspace.source(2).is_err());
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":14"#),
            "{tokens}"
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x81\x41*\r\n")
            .unwrap();
        let changed = workspace.parse_json(main).unwrap();
        assert!(
            changed.contains("갂") && changed.contains("InvalidEncoding"),
            "{changed}"
        );
        assert_eq!(workspace.source(1).unwrap().text(), "ignored\r\n*가*\r\n");
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*갂*\r\n");
    }

    #[test]
    fn gb18030_includes_export_four_byte_characters_as_utf8_snapshots() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=GB18030,lines=2]\n\ninclude::part.adoc[encoding=gb18030,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x95\x30\x85\x32\xa8\xbc*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("🙂\u{e7c7}"), "{json}");
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "ignored\r\n*🙂\u{e7c7}*\r\n"
        );
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":18"#),
            "{tokens}"
        );
        assert!(workspace.source(2).is_err());
        assert_eq!(
            workspace.source(1).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xd6\xd0*\r\n\x81\x30")
            .unwrap();
        let updated = workspace.parse_json(main).unwrap();
        assert!(
            updated.contains("中") && updated.contains(r#""diagnostics":[]"#),
            "{updated}"
        );
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*中*\r\n");
        assert!(!workspace.source(2).unwrap().is_complete());
        assert!(workspace.source(2).unwrap().omitted_lines().is_empty());
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "ignored\r\n*🙂\u{e7c7}*\r\n"
        );
    }

    #[test]
    fn big5_includes_export_variant_snapshots_and_non_bmp_utf8_offsets() {
        let mut workspace = ResourceWorkspace::new();
        workspace.set("/main.adoc", "include::part.adoc[encoding=CP950,lines=2]\n\ninclude::part.adoc[encoding=Big5-HKSCS,lines=2]\n\ninclude::part.adoc[encoding=Big5-HKSCS:2008,lines=2]").unwrap();
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\x8e\x40\xa1\x45*\r\n")
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let json = workspace.parse_json(main).unwrap();
        assert!(json.contains(r#""diagnostics":[]"#), "{json}");
        assert!(json.contains("\u{e311}‧") && json.contains("𣻗•"), "{json}");
        assert_eq!(
            workspace.source(1).unwrap().text(),
            "ignored\r\n*\u{e311}‧*\r\n"
        );
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*𣻗•*\r\n");
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":9,"end":17"#),
            "{tokens}"
        );
        assert!(
            tokens.contains(r#""source_id":2,"start":9,"end":18"#),
            "{tokens}"
        );
        assert!(workspace.source(3).is_err());
        assert_eq!(
            workspace.source(2).unwrap().position(14).unwrap(),
            adoctra_core::SourcePosition { line: 2, column: 3 }
        );
        workspace
            .set_binary("/part.adoc", b"ignored\r\n*\xa4\xa4*\r\n\x81")
            .unwrap();
        let updated = workspace.parse_json(main).unwrap();
        assert!(
            updated.contains("中") && updated.contains(r#""diagnostics":[]"#),
            "{updated}"
        );
        assert_eq!(workspace.source(3).unwrap().text(), "ignored\r\n*中*\r\n");
        assert!(!workspace.source(3).unwrap().is_complete());
        assert_eq!(workspace.source(2).unwrap().text(), "ignored\r\n*𣻗•*\r\n");
    }

    #[test]
    fn binary_marker_diagnostics_export_raw_spans_and_immutable_omissions() {
        let mut workspace = ResourceWorkspace::new();
        workspace
            .set("/main.adoc", "include::part.adoc[encoding=BINARY,tag=body]")
            .unwrap();
        workspace
            .set_binary(
                "/part.adoc",
                b"\xff// end::body[]\n\xfe// tag::body[]\n*good*\n",
            )
            .unwrap();
        let main = workspace.load("/main.adoc", None).unwrap();
        let parsed = workspace.parse_json(main).unwrap();
        for line in [1, 2] {
            assert!(
                parsed.contains(&format!(
                    "\"omitted_span\":{{\"line\":{line},\"start\":4,\"end\":15}}"
                )),
                "{parsed}"
            );
        }
        assert!(
            parsed.contains(r#""span":{"source_id":1,"start":0,"end":0}"#),
            "{parsed}"
        );
        assert!(parsed.contains("good"));
        let file = workspace.source(1).unwrap();
        assert_eq!(file.text(), "*good*\n");
        assert_eq!(file.position(0).unwrap().line, 3);
        assert_eq!(
            workspace.source_omitted_lines_json(1).unwrap(),
            r#"[{"offset":0,"line":1,"bytes":"ff2f2f20656e643a3a626f64795b5d0a"},{"offset":0,"line":2,"bytes":"fe2f2f207461673a3a626f64795b5d0a"}]"#
        );
        let tokens = workspace.tokens_json(main).unwrap();
        assert!(
            tokens.contains(r#""source_id":1,"start":0,"end":6"#),
            "{tokens}"
        );
        assert!(workspace.source(2).is_err());
    }

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
