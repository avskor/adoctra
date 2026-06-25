//! DOCX backend (scaffold). Wire up `docx-rs` where `convert` returns
//! `Unsupported`. See README for the intended mapping.

use adoctra_core::{Backend, Document, Error, Result};

#[derive(Debug, Default, Clone, Copy)]
pub struct DocxBackend;

impl DocxBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Backend for DocxBackend {
    fn name(&self) -> &'static str {
        "docx"
    }

    fn extension(&self) -> &'static str {
        "docx"
    }

    fn convert(&self, doc: &Document) -> Result<Vec<u8>> {
        let _blocks = doc.blocks.len(); // traversal seam for the real impl
        Err(Error::Unsupported(
            "docx backend not implemented yet (add docx-rs and build the OOXML tree here)".into(),
        ))
    }
}
