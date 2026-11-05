//! Extension API: adoctra's own processors at the points where Asciidoctor
//! invokes its extensions (user decision 2026-11-05: an own Rust API, not
//! Ruby API compatibility).
//!
//! * A [`BlockProcessor`] handles a block whose style names it: a delimited
//!   block of one of its [contexts](BlockProcessor::contexts) or a paragraph
//!   (Ruby's `registered_for_block?` in `next_block`).
//! * A [`BlockMacroProcessor`] handles a `name::target[attrlist]` line (Ruby's
//!   `CustomBlockMacroRx`).
//! * An [`InlineMacroProcessor`] handles a `name:target[attrlist]` macro in
//!   text (Ruby's custom inline macros, first in `sub_macros`).
//! * A [`TreeProcessor`] receives the finished document.
//!
//! Processors are registered on a [`Registry`] that a host puts into
//! [`ParseOptions::extensions`](crate::ParseOptions::extensions). Core still
//! performs no I/O: what a processor reads, runs or writes is its own policy.
//! A processor answers with a [`NewBlock`], which then goes through the same
//! completion as a built-in block (title, caption, id, roles, options), or
//! with `None`, which drops the block like Ruby's `nil`. An inline macro
//! processor answers with a [`NewInline`] (an image or quoted text) or with
//! `None`, which removes the macro.

use crate::ast::{Document, Span};
use crate::attributes::Attributes;
use crate::context::{OutputTarget, SafeMode};
use crate::diagnostic::{Diagnostic, DiagnosticCursor, DiagnosticLocation, Severity};
use std::sync::Arc;

/// The context of the block a [`BlockProcessor`] replaces: the delimited
/// block's own context or a paragraph (Ruby's `cloaked-context`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockContext {
    Paragraph,
    Listing,
    Literal,
    Open,
    Example,
    Sidebar,
    Quote,
    Pass,
}

impl BlockContext {
    /// Ruby's context name (`listing`, `open`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            BlockContext::Paragraph => "paragraph",
            BlockContext::Listing => "listing",
            BlockContext::Literal => "literal",
            BlockContext::Open => "open",
            BlockContext::Example => "example",
            BlockContext::Sidebar => "sidebar",
            BlockContext::Quote => "quote",
            BlockContext::Pass => "pass",
        }
    }

    /// The context of a delimited block's tip (`----`, `--`, …).
    pub(crate) fn of_tip(tip: &str) -> Option<Self> {
        Some(match tip {
            "----" => BlockContext::Listing,
            "...." => BlockContext::Literal,
            "--" => BlockContext::Open,
            "====" => BlockContext::Example,
            "****" => BlockContext::Sidebar,
            "____" => BlockContext::Quote,
            "++++" => BlockContext::Pass,
            _ => return None,
        })
    }

    /// Whether `style` is one Ruby handles itself on this delimited block (the
    /// context's own name or a masquerade), before any extension.
    pub(crate) fn builtin_style(self, style: &str) -> bool {
        const ADMONITIONS: [&str; 5] = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"];
        style == self.as_str()
            || match self {
                BlockContext::Listing => matches!(style, "literal" | "source"),
                BlockContext::Literal => matches!(style, "listing" | "source"),
                BlockContext::Open => {
                    matches!(
                        style,
                        "comment"
                            | "example"
                            | "literal"
                            | "listing"
                            | "pass"
                            | "quote"
                            | "sidebar"
                            | "source"
                            | "verse"
                            | "admonition"
                            | "abstract"
                            | "partintro"
                    ) || ADMONITIONS.contains(&style)
                }
                BlockContext::Example => style == "admonition" || ADMONITIONS.contains(&style),
                BlockContext::Quote => style == "verse",
                BlockContext::Pass => matches!(style, "stem" | "latexmath" | "asciimath"),
                BlockContext::Sidebar | BlockContext::Paragraph => false,
            }
    }
}

/// How a [`BlockProcessor`] receives the block's lines (Ruby's
/// `content_model`). The lines are always the preprocessed lines of the
/// block (includes and conditionals applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentModel {
    /// The lines as read (Ruby's `:simple`).
    #[default]
    Simple,
    /// The same lines; Ruby distinguishes it only for substitutions.
    Raw,
    /// The lines with tabs expanded and the indentation adjusted by the
    /// `indent` and `tabsize` attributes, like a listing block.
    Verbatim,
    /// The lines are read and discarded; the processor is not invoked.
    Skip,
}

/// How a [`BlockMacroProcessor`] receives the text in its brackets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MacroContent {
    /// Parsed as an attribute list (Ruby's `:attributes`, the default).
    #[default]
    Attributes,
    /// Kept whole in the `text` attribute.
    Text,
}

/// A key of [`ExtensionAttributes`]: Ruby's integer keys for positional
/// values and string keys for named ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrKey {
    Positional(usize),
    Named(String),
}

/// Block attributes in Ruby's insertion order (a Ruby `Hash`): metadata lines
/// in reading order, then the keys the extension point adds (positional names,
/// default attributes, `cloaked-context`). Setting an existing key keeps its
/// position. A key can hold Ruby's `nil`: an empty positional value
/// (`[,svg]`) is a key without a value, and Ruby code that walks the hash
/// (asciidoctor-diagram's checksum) sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionAttributes {
    entries: Vec<(AttrKey, Option<String>)>,
}

impl ExtensionAttributes {
    pub fn new() -> Self {
        Self::default()
    }

    /// The value of a named attribute (`None` when missing or `nil`, like
    /// Ruby's `attributes[name]`).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries.iter().find_map(|(key, value)| match key {
            AttrKey::Named(key) if key == name => value.as_deref(),
            _ => None,
        })
    }

    /// The value of a positional attribute (1-based, like Ruby; `None` when
    /// missing or `nil`).
    pub fn positional(&self, index: usize) -> Option<&str> {
        self.entries.iter().find_map(|(key, value)| match key {
            AttrKey::Positional(key) if *key == index => value.as_deref(),
            _ => None,
        })
    }

    /// Whether the named attribute has a value (a `nil` value counts as
    /// missing, like Ruby's `attributes[name]` truthiness).
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Set a named attribute; an existing key keeps its position.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.set_key(AttrKey::Named(name.into()), value.into());
    }

    /// Set any key; an existing key keeps its position.
    pub fn set_key(&mut self, key: AttrKey, value: String) {
        self.set_value(key, Some(value));
    }

    /// Set any key to a value or to Ruby's `nil`; an existing key keeps its
    /// position.
    pub fn set_value(&mut self, key: AttrKey, value: Option<String>) {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Remove a named attribute and return its value.
    pub fn remove(&mut self, name: &str) -> Option<String> {
        let index = self
            .entries
            .iter()
            .position(|(key, _)| matches!(key, AttrKey::Named(key) if key == name))?;
        self.entries.remove(index).1
    }

    /// Whether the block has the option (`name-option`).
    pub fn has_option(&self, name: &str) -> bool {
        self.contains(&format!("{name}-option"))
    }

    /// The keys with their values in insertion order (`None` is Ruby's
    /// `nil`).
    pub fn iter(&self) -> impl Iterator<Item = (&AttrKey, Option<&str>)> {
        self.entries
            .iter()
            .map(|(key, value)| (key, value.as_deref()))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A block handed to a [`BlockProcessor`].
#[derive(Debug, Clone)]
pub struct BlockRequest {
    /// The style that selected the processor.
    pub name: String,
    /// The delimited block's context, or [`BlockContext::Paragraph`].
    pub context: BlockContext,
    /// The block's lines (see [`ContentModel`]).
    pub lines: Vec<String>,
    /// The block's attributes, positional names applied, with
    /// `cloaked-context`; the style is removed, like Ruby's `build_block`.
    pub attributes: ExtensionAttributes,
    /// The source range of the whole block.
    pub span: Span,
}

/// A block macro handed to a [`BlockMacroProcessor`].
#[derive(Debug, Clone)]
pub struct BlockMacroRequest {
    pub name: String,
    /// The target after attribute references.
    pub target: String,
    /// The preceding metadata merged with the macro's own attribute list
    /// (or `text`, see [`MacroContent`]).
    pub attributes: ExtensionAttributes,
    /// The macro line.
    pub span: Span,
}

/// How an [`InlineMacroProcessor`] is written (Ruby's `format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InlineMacroFormat {
    /// `name:target[attrlist]` (Ruby's `:long`, the default).
    #[default]
    Long,
    /// `name:[attrlist]`; the bracket text is also the target (`:short`).
    Short,
}

/// An inline macro handed to an [`InlineMacroProcessor`].
#[derive(Debug, Clone)]
pub struct InlineMacroRequest {
    pub name: String,
    /// The target as the macros stage sees it (after the earlier
    /// substitutions: special characters, quotes, attribute references and
    /// replacements). A short macro's target is its normalized bracket text.
    pub target: String,
    /// The default attributes, then the bracket text parsed as an attribute
    /// list (or in `text`, see [`MacroContent`]) after Ruby's
    /// `normalize_text` (stripped, line feeds as spaces, `\]` unescaped).
    pub attributes: ExtensionAttributes,
    /// The whole macro.
    pub span: Span,
}

/// What a processor can see of the document and how it reports problems.
pub struct ProcessContext<'a> {
    attributes: &'a Attributes,
    safe: SafeMode,
    base_dir: Option<&'a str>,
    output: Option<&'a OutputTarget>,
    span: Span,
    /// Where reports are located when it differs from `span` (Ruby's
    /// document reader mark while converting).
    cursor: Option<DiagnosticCursor>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> ProcessContext<'a> {
    pub(crate) fn new(
        attributes: &'a Attributes,
        safe: SafeMode,
        base_dir: Option<&'a str>,
        output: Option<&'a OutputTarget>,
        span: Span,
    ) -> Self {
        Self {
            attributes,
            safe,
            base_dir,
            output,
            span,
            cursor: None,
            diagnostics: Vec::new(),
        }
    }

    /// Reports located at this reader cursor instead of the span.
    pub(crate) fn with_cursor(mut self, cursor: Option<DiagnosticCursor>) -> Self {
        self.cursor = cursor;
        self
    }

    /// A document attribute as the block sees it.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name)
    }

    /// All document attributes as the block sees them.
    pub fn attributes(&self) -> &Attributes {
        self.attributes
    }

    pub fn safe(&self) -> SafeMode {
        self.safe
    }

    /// Ruby's `base_dir` of the document, when the host supplied one.
    pub fn base_dir(&self) -> Option<&str> {
        self.base_dir
    }

    /// Where the host writes the converted document (Ruby's `:to_file` or
    /// `:to_dir`), if it writes a file.
    pub fn output(&self) -> Option<&OutputTarget> {
        self.output
    }

    /// The source range of the block or macro being processed.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Report an error at the block (Ruby's `logger.error` with the block's
    /// source location). An inline macro converted after reading reports at
    /// Ruby's document reader mark: the last block read from the document's
    /// own reader (its span stays the macro's for editors).
    pub fn error(&mut self, message: impl Into<String>) {
        self.report(Severity::Error, message.into());
    }

    /// Report a warning at the block.
    pub fn warning(&mut self, message: impl Into<String>) {
        self.report(Severity::Warning, message.into());
    }

    /// Report a warning without a source location (Ruby's messages logged
    /// without `source_location`, e.g. the path resolver's).
    pub fn warning_unlocated(&mut self, message: impl Into<String>) {
        self.report(Severity::Warning, message.into());
        if let Some(last) = self.diagnostics.last_mut() {
            last.report.location = crate::diagnostic::DiagnosticLocation::Unlocated;
        }
    }

    fn report(&mut self, severity: Severity, message: String) {
        self.diagnostics.push(Diagnostic {
            omitted_span: None,
            report: crate::DiagnosticReport {
                location: self
                    .cursor
                    .map_or(DiagnosticLocation::Source, DiagnosticLocation::Cursor),
                ..Default::default()
            },
            span: self.span,
            severity,
            message,
        });
    }

    pub(crate) fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

/// The kind of block a processor creates. Text sources are split into
/// lines with trailing whitespace removed, like Ruby's `Block.new` with a
/// string `:source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewBlockKind {
    /// An image block (`target` as an `image::` macro would give it).
    Image { target: String },
    /// A listing block with this text (verbatim, no further substitutions).
    Listing { source: String },
    /// A literal block with this text (verbatim, no further substitutions).
    Literal { source: String },
    /// A passthrough block with this raw text.
    Pass { source: String },
}

/// A block created by a processor, with its attributes. `title`, `caption`,
/// `id`, `role`, `style`, `reftext` and `*-option` keys become the block's
/// metadata; other named keys stay block attributes; positional keys are
/// dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBlock {
    pub kind: NewBlockKind,
    pub attributes: ExtensionAttributes,
    /// Caption contexts (`figure`, …) of titled blocks the processor
    /// created and then dropped after Ruby's `assign_caption` numbered
    /// them: each advances `<context>-number` before this block is
    /// numbered, under the same conditions (a title, no `caption`
    /// attribute, a `<context>-caption` prefix).
    pub discarded_captions: Vec<String>,
}

impl NewBlock {
    pub fn image(target: impl Into<String>, attributes: ExtensionAttributes) -> Self {
        Self {
            kind: NewBlockKind::Image {
                target: target.into(),
            },
            attributes,
            discarded_captions: Vec::new(),
        }
    }

    pub fn listing(source: impl Into<String>, attributes: ExtensionAttributes) -> Self {
        Self {
            kind: NewBlockKind::Listing {
                source: source.into(),
            },
            attributes,
            discarded_captions: Vec::new(),
        }
    }

    pub fn literal(source: impl Into<String>, attributes: ExtensionAttributes) -> Self {
        Self {
            kind: NewBlockKind::Literal {
                source: source.into(),
            },
            attributes,
            discarded_captions: Vec::new(),
        }
    }

    pub fn pass(source: impl Into<String>, attributes: ExtensionAttributes) -> Self {
        Self {
            kind: NewBlockKind::Pass {
                source: source.into(),
            },
            attributes,
            discarded_captions: Vec::new(),
        }
    }
}

/// What an [`InlineMacroProcessor`] puts in place of the macro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewInlineKind {
    /// An inline image of `target`, resolved like an `image:` macro's
    /// (`imagesdir`, `data-uri`, SVG options). The attributes give `alt`,
    /// `width`, `height`, `title`, `link`, `window`, `role`, `float`,
    /// `imagesdir` and `*-option` flags.
    Image { target: String },
    /// Quoted text (Ruby's `:quoted` inline). Like Ruby's, the text is
    /// written as is, without substitutions or escaping; the `role`
    /// attribute becomes the element's class.
    Quoted {
        format: crate::AttributeFormat,
        text: String,
    },
}

/// An inline node created by an [`InlineMacroProcessor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInline {
    pub kind: NewInlineKind,
    pub attributes: ExtensionAttributes,
}

impl NewInline {
    pub fn image(target: impl Into<String>, attributes: ExtensionAttributes) -> Self {
        Self {
            kind: NewInlineKind::Image {
                target: target.into(),
            },
            attributes,
        }
    }

    pub fn quoted(
        format: crate::AttributeFormat,
        text: impl Into<String>,
        attributes: ExtensionAttributes,
    ) -> Self {
        Self {
            kind: NewInlineKind::Quoted {
                format,
                text: text.into(),
            },
            attributes,
        }
    }
}

/// Handles a block whose style is its [name](BlockProcessor::name).
pub trait BlockProcessor: Send + Sync {
    /// The style the processor is registered for.
    fn name(&self) -> &str;
    /// The contexts it accepts (Ruby's `contexts`/`on_contexts`).
    fn contexts(&self) -> &[BlockContext];
    /// Names for the positional attributes after the style (Ruby's
    /// `name_positional_attributes`): the first names the second field.
    fn positional_attributes(&self) -> &[&str] {
        &[]
    }
    /// Attributes set unless the block has them (Ruby's `default_attrs`).
    fn default_attributes(&self) -> &[(&str, &str)] {
        &[]
    }
    fn content_model(&self) -> ContentModel {
        ContentModel::Simple
    }
    /// Create the replacement block, or `None` to drop the block.
    fn process(&self, request: BlockRequest, context: &mut ProcessContext<'_>) -> Option<NewBlock>;
}

/// Handles `name::target[attrlist]` lines.
pub trait BlockMacroProcessor: Send + Sync {
    fn name(&self) -> &str;
    /// Names for the positional attributes in the brackets.
    fn positional_attributes(&self) -> &[&str] {
        &[]
    }
    /// Attributes set unless the macro has them.
    fn default_attributes(&self) -> &[(&str, &str)] {
        &[]
    }
    fn content(&self) -> MacroContent {
        MacroContent::Attributes
    }
    /// Create the replacement block, or `None` to drop the macro.
    fn process(
        &self,
        request: BlockMacroRequest,
        context: &mut ProcessContext<'_>,
    ) -> Option<NewBlock>;
}

/// Handles `name:target[attrlist]` macros in text. Ruby runs custom inline
/// macros first in its macros substitution, each over the text the previous
/// one left, in registration order: `\\?name:(\S+?)\[(|.*?[^\\])\]` on one
/// line (`name:[…]` for [`InlineMacroFormat::Short`]); a leading backslash
/// escapes the macro.
pub trait InlineMacroProcessor: Send + Sync {
    fn name(&self) -> &str;
    fn format(&self) -> InlineMacroFormat {
        InlineMacroFormat::Long
    }
    /// Names for the positional attributes in the brackets.
    fn positional_attributes(&self) -> &[&str] {
        &[]
    }
    /// Attributes set before the bracket text is parsed (Ruby's
    /// `default_attrs`).
    fn default_attributes(&self) -> &[(&str, &str)] {
        &[]
    }
    fn content(&self) -> MacroContent {
        MacroContent::Attributes
    }
    /// Create the replacement, or `None` to remove the macro.
    fn process(
        &self,
        request: InlineMacroRequest,
        context: &mut ProcessContext<'_>,
    ) -> Option<NewInline>;
}

/// Runs on the finished document (after reading and conversion-time
/// substitutions), before a backend converts it.
pub trait TreeProcessor: Send + Sync {
    fn name(&self) -> &str;
    fn process(&self, document: &mut Document, context: &mut ProcessContext<'_>);
}

/// The processors a host registers for parsing.
#[derive(Default, Clone)]
pub struct Registry {
    blocks: Vec<Arc<dyn BlockProcessor>>,
    block_macros: Vec<Arc<dyn BlockMacroProcessor>>,
    inline_macros: Vec<Arc<dyn InlineMacroProcessor>>,
    tree_processors: Vec<Arc<dyn TreeProcessor>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field(
                "blocks",
                &self.blocks.iter().map(|p| p.name()).collect::<Vec<_>>(),
            )
            .field(
                "block_macros",
                &self
                    .block_macros
                    .iter()
                    .map(|p| p.name())
                    .collect::<Vec<_>>(),
            )
            .field(
                "inline_macros",
                &self
                    .inline_macros
                    .iter()
                    .map(|p| p.name())
                    .collect::<Vec<_>>(),
            )
            .field(
                "tree_processors",
                &self
                    .tree_processors
                    .iter()
                    .map(|p| p.name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_block(&mut self, processor: impl BlockProcessor + 'static) -> &mut Self {
        self.blocks.push(Arc::new(processor));
        self
    }

    pub fn register_block_macro(
        &mut self,
        processor: impl BlockMacroProcessor + 'static,
    ) -> &mut Self {
        self.block_macros.push(Arc::new(processor));
        self
    }

    /// Register an inline macro processor. Like Ruby's hash of inline
    /// macros, a second registration of a name replaces the first in its
    /// place.
    pub fn register_inline_macro(
        &mut self,
        processor: impl InlineMacroProcessor + 'static,
    ) -> &mut Self {
        let processor: Arc<dyn InlineMacroProcessor> = Arc::new(processor);
        match self
            .inline_macros
            .iter_mut()
            .find(|p| p.name() == processor.name())
        {
            Some(slot) => *slot = processor,
            None => self.inline_macros.push(processor),
        }
        self
    }

    pub fn register_tree_processor(
        &mut self,
        processor: impl TreeProcessor + 'static,
    ) -> &mut Self {
        self.tree_processors.push(Arc::new(processor));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
            && self.block_macros.is_empty()
            && self.inline_macros.is_empty()
            && self.tree_processors.is_empty()
    }

    /// Ruby's `registered_for_block?`: the last registration wins.
    pub fn block(&self, name: &str, context: BlockContext) -> Option<&Arc<dyn BlockProcessor>> {
        self.blocks
            .iter()
            .rev()
            .find(|p| p.name() == name && p.contexts().contains(&context))
    }

    /// Ruby's `registered_for_block_macro?`: the last registration wins.
    pub fn block_macro(&self, name: &str) -> Option<&Arc<dyn BlockMacroProcessor>> {
        self.block_macros.iter().rev().find(|p| p.name() == name)
    }

    /// The inline macro processors in the order Ruby applies them.
    pub fn inline_macros(&self) -> impl Iterator<Item = &Arc<dyn InlineMacroProcessor>> {
        self.inline_macros.iter()
    }

    pub fn tree_processors(&self) -> impl Iterator<Item = &Arc<dyn TreeProcessor>> {
        self.tree_processors.iter()
    }
}
