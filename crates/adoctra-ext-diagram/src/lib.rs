//! Diagrams for adoctra: block, block macro and inline macro processors for
//! the diagram types of asciidoctor-diagram ([`DIAGRAM_TYPES`]: PlantUML,
//! Graphviz, Mermaid, D2, Vega, …), each with
//! its local tool or Kroki or a PlantUML server through `diagram-server-url`.
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

use converter::{
    graphviz::Graphviz, java_tools, mermaid::Mermaid, plantuml::PlantUml, tools::*, Converter,
};
use processor::{
    Diagram, DiagramBlock, DiagramBlockMacro, DiagramInlineMacro, Structurizr, StructurizrBlock,
    StructurizrBlockMacro,
};
use std::sync::Arc;

/// The diagram types (processor names) this crate registers, in Ruby's
/// registration order (the order `asciidoctor-diagram.rb` requires them),
/// which is also the order inline macros apply.
pub const DIAGRAM_TYPES: [&str; 42] = [
    "a2s",
    "blockdiag",
    "seqdiag",
    "actdiag",
    "nwdiag",
    "rackdiag",
    "packetdiag",
    "bpmn",
    "bytefield",
    "d2",
    "dbml",
    "diagrams",
    "ditaa",
    "dpic",
    "erd",
    "goat",
    "gnuplot",
    "graphviz",
    "graphviz_py",
    "lilypond",
    "meme",
    "mermaid",
    "msc",
    "nomnoml",
    "oxdraw",
    "penrose",
    "pikchr",
    "pintora",
    "plantuml",
    "salt",
    "shaape",
    "smcat",
    "structurizr",
    "svgbob",
    "symbolator",
    "syntrax",
    "tikz",
    "umlet",
    "vega",
    "vegalite",
    "tape",
    "wavedrom",
];

/// The converter and Ruby's processor class prefix of a type.
fn diagram_type(name: &str) -> (&'static str, Box<dyn Converter>) {
    let blockdiag = |tool: &'static str, alt_tool: &'static str| -> Box<dyn Converter> {
        Box::new(BlockDiag { tool, alt_tool })
    };
    match name {
        "a2s" => ("AsciiToSvg", Box::new(AsciiToSvg)),
        "blockdiag" => ("BlockDiag", blockdiag("blockdiag", "blockdiag3")),
        "seqdiag" => ("SeqDiag", blockdiag("seqdiag", "seqdiag3")),
        "actdiag" => ("ActDiag", blockdiag("actdiag", "actdiag3")),
        "nwdiag" => ("NwDiag", blockdiag("nwdiag", "nwdiag3")),
        "rackdiag" => ("RackDiag", blockdiag("rackdiag", "rackdiag3")),
        "packetdiag" => ("PacketDiag", blockdiag("packetdiag", "packetdiag3")),
        "bpmn" => ("Bpmn", Box::new(Bpmn)),
        "bytefield" => ("Bytefield", Box::new(Bytefield)),
        "d2" => ("D2", Box::new(D2)),
        "dbml" => ("Dbml", Box::new(Dbml)),
        "diagrams" => ("Diagrams", Box::new(Diagrams)),
        "ditaa" => ("Ditaa", Box::new(java_tools::Ditaa)),
        "dpic" => ("Dpic", Box::new(Dpic)),
        "erd" => ("Erd", Box::new(Erd)),
        "goat" => ("GoAT", Box::new(GoAT)),
        "gnuplot" => ("Gnuplot", Box::new(Gnuplot)),
        "graphviz" => ("Graphviz", Box::new(Graphviz)),
        "graphviz_py" => ("GraphvizPy", Box::new(GraphvizPy)),
        "lilypond" => ("Lilypond", Box::new(Lilypond)),
        "meme" => ("Meme", Box::new(converter::meme::Meme)),
        "mermaid" => ("Mermaid", Box::new(Mermaid)),
        "msc" => ("Msc", Box::new(Msc)),
        "nomnoml" => ("Nomnoml", Box::new(Nomnoml)),
        "oxdraw" => ("Oxdraw", Box::new(Oxdraw)),
        "penrose" => ("Penrose", Box::new(Penrose)),
        "pikchr" => ("Pikchr", Box::new(Pikchr)),
        "pintora" => ("Pintora", Box::new(Pintora)),
        "plantuml" => ("PlantUml", Box::new(PlantUml { tag: "uml" })),
        "salt" => ("Salt", Box::new(PlantUml { tag: "salt" })),
        "shaape" => ("Shaape", Box::new(Shaape)),
        "smcat" => ("Smcat", Box::new(Smcat)),
        "svgbob" => ("SvgBob", Box::new(Svgbob)),
        "symbolator" => ("Symbolator", Box::new(Symbolator)),
        "syntrax" => ("Syntrax", Box::new(Syntrax)),
        "tikz" => ("TikZ", Box::new(TikZ)),
        "umlet" => ("Umlet", Box::new(Umlet)),
        "vega" | "vegalite" => ("Vega", Box::new(Vega)),
        "tape" => ("Vhs", Box::new(Vhs)),
        "wavedrom" => ("Wavedrom", Box::new(Wavedrom)),
        other => unreachable!("unknown diagram type {other}"),
    }
}

/// Register the block, block macro and inline macro processors of every
/// diagram type, in Ruby's order. Each processor caches the tool paths it
/// finds, as Ruby's processor `config` does, for the documents parsed with
/// this registry.
pub fn register(registry: &mut adoctra_core::Registry, host: Arc<dyn DiagramHost>) {
    for name in DIAGRAM_TYPES {
        if name == "structurizr" {
            // Block and block macro only, each with its own processors.
            let structurizr = || {
                let diagram = |class, converter: Box<dyn Converter>| {
                    Arc::new(Diagram {
                        name: "structurizr",
                        class,
                        converter,
                        host: Arc::clone(&host),
                        macro_positional: &["format"],
                        meme: false,
                    })
                };
                Arc::new(Structurizr {
                    convert: diagram("StructurizrConvert", Box::new(java_tools::Structurizr)),
                    renderers: [
                        diagram("D2", Box::new(D2)),
                        diagram("Graphviz", Box::new(Graphviz)),
                        diagram("Mermaid", Box::new(Mermaid)),
                        diagram("PlantUml", Box::new(PlantUml { tag: "uml" })),
                    ],
                })
            };
            registry
                .register_block(StructurizrBlock(
                    structurizr(),
                    Default::default(),
                    Default::default(),
                ))
                .register_block_macro(StructurizrBlockMacro(
                    structurizr(),
                    Default::default(),
                    Default::default(),
                ));
            continue;
        }
        let (class, converter) = diagram_type(name);
        // meme: a block macro and an inline macro (background, top, bottom).
        let meme = name == "meme";
        let diagram = Arc::new(Diagram {
            name,
            class,
            converter,
            host: Arc::clone(&host),
            macro_positional: if meme {
                &["top", "bottom", "target", "format"]
            } else {
                &["format"]
            },
            meme,
        });
        if !meme {
            registry.register_block(DiagramBlock(Arc::clone(&diagram), Default::default()));
        }
        registry
            .register_block_macro(DiagramBlockMacro(Arc::clone(&diagram), Default::default()))
            .register_inline_macro(DiagramInlineMacro(diagram, Default::default()));
    }
}
