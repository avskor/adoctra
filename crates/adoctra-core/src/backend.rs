//! The [`Backend`] trait: a render target for a parsed [`Document`].
//!
//! Each output format (HTML, DOCX, JSON, ...) lives in its own crate and
//! implements this trait. Third parties can add formats the same way without
//! touching the core — that's the whole point of keeping this trait here and
//! depending only on the document model.

use crate::ast::Document;
use crate::error::Result;

pub trait Backend {
    /// Stable identifier used on the CLI (`--to <name>`).
    fn name(&self) -> &'static str;

    /// Conventional file extension for this backend's output.
    fn extension(&self) -> &'static str;

    /// Render a document to bytes (text formats return UTF-8 bytes).
    fn convert(&self, doc: &Document) -> Result<Vec<u8>>;
}
