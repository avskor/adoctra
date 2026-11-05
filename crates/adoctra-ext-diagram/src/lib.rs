//! Diagrams for adoctra: block, block macro and inline macro processors for
//! PlantUML (`plantuml`, `salt`), Graphviz (`graphviz`) and Mermaid
//! (`mermaid`), with Kroki or a PlantUML server through `diagram-server-url`.
//! A port of
//! asciidoctor-diagram 3.2.1 (pinned in
//! `compatibility/asciidoctor-diagram-version`) on adoctra's extension API.
//!
//! ```
//! use std::sync::Arc;
//! let mut registry = adoctra_core::Registry::new();
//! adoctra_ext_diagram::register(&mut registry, Arc::new(adoctra_ext_diagram::SystemHost::new()));
//! let options = adoctra_core::ParseOptions {
//!     extensions: Some(Arc::new(registry)),
//!     ..Default::default()
//! };
//! # let _ = options;
//! ```
//!
//! What the processors run, read and write goes through the
//! [`DiagramHost`] the host supplies.

mod cache;
mod converter;
mod host;
mod image;
mod md5;
mod processor;
mod source;

pub use host::{Command, DiagramHost, HttpClient, HttpRequest, Output, SystemHost};

use converter::{graphviz::Graphviz, mermaid::Mermaid, plantuml::PlantUml, Converter};
use processor::{Diagram, DiagramBlock, DiagramBlockMacro, DiagramInlineMacro};
use std::sync::Arc;

/// The diagram names this crate registers.
pub const DIAGRAM_TYPES: [&str; 4] = ["plantuml", "salt", "graphviz", "mermaid"];

/// Register the block, block macro and inline macro processors of every
/// diagram type. Inline macros apply in Ruby's registration order (the
/// order `asciidoctor-diagram.rb` requires the types: graphviz, mermaid,
/// plantuml, salt).
pub fn register(registry: &mut adoctra_core::Registry, host: Arc<dyn DiagramHost>) {
    let types: [(&'static str, &'static str, Box<dyn Converter>); 4] = [
        ("plantuml", "PlantUml", Box::new(PlantUml { tag: "uml" })),
        ("salt", "Salt", Box::new(PlantUml { tag: "salt" })),
        ("graphviz", "Graphviz", Box::new(Graphviz)),
        ("mermaid", "Mermaid", Box::new(Mermaid)),
    ];
    let mut diagrams = Vec::new();
    for (name, class, converter) in types {
        let diagram = Arc::new(Diagram {
            name,
            class,
            converter,
            host: Arc::clone(&host),
        });
        registry
            .register_block(DiagramBlock(Arc::clone(&diagram)))
            .register_block_macro(DiagramBlockMacro(Arc::clone(&diagram)));
        diagrams.push(diagram);
    }
    for name in ["graphviz", "mermaid", "plantuml", "salt"] {
        if let Some(diagram) = diagrams.iter().find(|d| d.name == name) {
            registry.register_inline_macro(DiagramInlineMacro(Arc::clone(diagram)));
        }
    }
}
