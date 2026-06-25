//! Extension API: the adoctra analogue of Asciidoctor's extension points.
//!
//! Extensions transform the document at well-defined phases. Concrete
//! extensions live in their own `adoctra-ext-*` crates and are registered on a
//! [`Registry`] before processing.

use crate::ast::Document;
use crate::error::Result;

/// Runs after parsing, before backend conversion. The broadest, simplest hook
/// to start with; split into pre/block/inline/tree/post phases as needs grow.
pub trait TreeProcessor {
    fn name(&self) -> &'static str;
    fn process(&self, doc: &mut Document) -> Result<()>;
}

#[derive(Default)]
pub struct Registry {
    tree_processors: Vec<Box<dyn TreeProcessor>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_tree_processor(&mut self, ext: Box<dyn TreeProcessor>) -> &mut Self {
        self.tree_processors.push(ext);
        self
    }

    /// Apply every registered extension to the document, in registration order.
    pub fn run(&self, doc: &mut Document) -> Result<()> {
        for ext in &self.tree_processors {
            ext.process(doc)?;
        }
        Ok(())
    }
}
