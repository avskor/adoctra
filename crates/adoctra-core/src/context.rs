//! Parse configuration and attribute state shared by recursive block parsing.

use crate::attributes::parts_text;
use crate::{AttributePart, Attributes, SourceId};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Debug, Clone, Default)]
pub struct ParseOptions {
    /// SourceMap id for this input. The convenience APIs use source zero.
    pub source_id: SourceId,
    /// Hard attribute overrides: document assignments cannot change these keys.
    /// This initial API supports set overrides; soft defaults and locked unsets
    /// will be added alongside the remaining Asciidoctor option semantics.
    pub attributes: Attributes,
    /// Limits for explicit resource parsing; ordinary string parsing performs no loads.
    pub includes: IncludeLimits,
    /// Ruby's `standalone: false`: the document is converted as embedded
    /// content. It sets the `embedded` attribute and hides the title unless
    /// the host sets `showtitle`; a standalone document (the default) gets the
    /// `copycss`, `iconfont-remote`, `stylesheet` and `webfonts` defaults.
    pub embedded: bool,
    /// The clock and the input's modification time, for Ruby's date
    /// attributes (`localdate`, `docdate`, …). Core reads no clock: without
    /// them those attributes stay unset, which keeps output deterministic.
    pub dates: Option<DocumentDates>,
}

/// A wall-clock time with its UTC offset, as the host's clock reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Seconds east of UTC (`0` formats as `UTC`, like Ruby).
    pub utc_offset: i32,
}

impl DateTime {
    /// Ruby's `strftime '%F'`.
    fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Ruby's `strftime %(%T #{utc_offset == 0 ? 'UTC' : '%z'})`.
    fn time(&self) -> String {
        let zone = match self.utc_offset {
            0 => "UTC".to_owned(),
            offset => {
                let minutes = offset.unsigned_abs() / 60;
                let sign = if offset < 0 { '-' } else { '+' };
                format!("{sign}{:02}{:02}", minutes / 60, minutes % 60)
            }
        };
        format!(
            "{:02}:{:02}:{:02} {zone}",
            self.hour, self.minute, self.second
        )
    }
}

/// The times behind Ruby's `fill_datetime_attributes`: `local` is now
/// (`localdate`, `localtime`, …), `document` the input's modification time
/// (`docdate`, `doctime`, …). A host honouring `SOURCE_DATE_EPOCH` passes
/// that instant in UTC for both, and the current time as `document` when
/// the input has no file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentDates {
    pub local: DateTime,
    pub document: DateTime,
}

/// Ruby's `fill_datetime_attributes`: each value fills only an unset key,
/// so host overrides win and a preset date derives its year.
fn fill_dates(attributes: &mut Attributes, dates: &DocumentDates) {
    for (prefix, time) in [("local", &dates.local), ("doc", &dates.document)] {
        let date = match attributes.get(&format!("{prefix}date")) {
            Some(date) => {
                let date = date.to_owned();
                if attributes.get(&format!("{prefix}year")).is_none() && date.find('-') == Some(4) {
                    attributes.set(format!("{prefix}year"), &date[..4]);
                }
                date
            }
            None => {
                attributes.set(format!("{prefix}date"), time.date());
                if attributes.get(&format!("{prefix}year")).is_none() {
                    attributes.set(format!("{prefix}year"), time.year.to_string());
                }
                time.date()
            }
        };
        let clock = match attributes.get(&format!("{prefix}time")) {
            Some(clock) => clock.to_owned(),
            None => {
                attributes.set(format!("{prefix}time"), time.time());
                time.time()
            }
        };
        if attributes.get(&format!("{prefix}datetime")).is_none() {
            attributes.set(format!("{prefix}datetime"), format!("{date} {clock}"));
        }
    }
}

impl ParseOptions {
    /// File-derived attributes are hard overrides, including over caller
    /// attributes, matching the pinned reference's file loading entry point.
    pub(crate) fn for_resource(
        &self,
        sources: &crate::SourceMap,
        provider: &dyn crate::ResourceProvider,
    ) -> Self {
        let mut options = self.clone();
        if let Some(path) = sources
            .get(self.source_id)
            .and_then(|source| provider.document_path(source.name()))
        {
            if let Some((directory, basename)) = path.rsplit_once('/') {
                let dot = basename.rfind('.');
                let suffix = dot.map_or("", |i| &basename[i..]);
                // Helpers.extname treats a dotfile as an extension, whereas
                // File.basename refuses to strip the entire filename.
                let name = dot.filter(|&i| i != 0).map_or(basename, |i| &basename[..i]);
                // Keep the separator for both POSIX and Windows drive roots.
                let directory = if directory.is_empty()
                    || (directory.len() == 2
                        && directory.as_bytes()[0].is_ascii_alphabetic()
                        && directory.ends_with(':'))
                {
                    &path[..directory.len() + 1]
                } else {
                    directory
                };
                options.attributes.set("docdir", directory);
                options.attributes.set("docname", name);
                options.attributes.set("docfilesuffix", suffix);
                options.attributes.set("docfile", path);
            }
        }
        options
    }
}

/// The caption context of a numbered block kind: `example` or `figure`.
pub(crate) fn caption_context(kind: &crate::BlockKind) -> Option<&'static str> {
    match kind {
        crate::BlockKind::Example { .. } => Some("example"),
        crate::BlockKind::Image { .. } => Some("figure"),
        crate::BlockKind::Table(_) => Some("table"),
        _ => None,
    }
}

pub(crate) struct ParseContext {
    pub attributes: Arc<Attributes>,
    /// Quote/link mode is cached at document creation/header completion in Ruby;
    /// entries in the header and later body mutations do not update it eagerly.
    pub quote_compat: bool,
    /// Reader range of the last accepted doctitle entry; None for host values.
    pub doctitle_span: Option<crate::Span>,
    locked: BTreeSet<String>,
    counters: BTreeMap<String, String>,
    replay: Option<Arc<Attributes>>,
    pub body_subs: crate::body_subs::BodySubs,
    pub comments: Vec<crate::Span>,
    pub comment_diagnostics: Vec<crate::Diagnostic>,
    pub inline_diagnostics: Vec<crate::Diagnostic>,
    /// Inline anchors cataloged while reading, in reading order.
    pub anchors: Vec<(String, crate::anchor_catalog::Entry)>,
    /// Early-expanded block reftext keyed by the completed block's reader
    /// range; references format it again during conversion.
    pub reftexts: BTreeMap<(usize, usize), crate::reftext::Read>,
    /// Caption numerals of numbered blocks, keyed by their reader range
    /// (Ruby's `@numeral`), for `xrefstyle`.
    numerals: BTreeMap<(usize, usize), String>,
    /// Blockquotes whose attribution is a credit line still to split into
    /// author and citetitle, keyed by their reader range.
    pub credits: BTreeSet<(usize, usize)>,
    /// Section titles read in the body, keyed by the heading's reader range;
    /// the hierarchy assigns their numerals.
    pub sections: BTreeMap<(usize, usize), crate::section_numbers::Section>,
    /// Whether the doctype is book, read once the header is complete.
    pub book: bool,
    /// Whether the doctype is manpage (outside nested documents), read once
    /// the header is complete.
    pub manpage: bool,
    /// Levels and keys of the sections still open while reading.
    open_sections: Vec<(u8, (usize, usize))>,
    /// Cross-references generated by attribute definitions, in reading order
    /// (indexed by `Xref::definition`).
    definitions: Vec<crate::resolve::Definition>,
    /// A section-title anchor's reftext, handed from the heading parser to
    /// block completion.
    pub section_reftext: Option<crate::reftext::Read>,
    /// Metadata from a block macro's brackets (`toc::[id=…]`), handed from the
    /// block parser to block completion, where it replaces the block's own.
    pub(crate) block_macro: Option<crate::parser::MacroMeta>,
    /// `outfilesuffix` pinned when the header is complete (Ruby's
    /// `Document#outfilesuffix`), for interdocument references.
    outfilesuffix: Option<String>,
    /// Reading an AsciiDoc table cell's nested document.
    pub nested: bool,
    /// The root document's reading attributes and locks while nested
    /// documents are read: their counters delegate to it.
    root: Option<(Arc<Attributes>, BTreeSet<String>)>,
    /// Footnotes registered while reading (section titles, anchored block
    /// titles, attribute definitions), handed to conversion.
    footnotes: crate::substitution_context::Footnotes,
    /// Ruby's `Document#callouts`, shared with nested documents.
    pub callouts: crate::callouts::Catalog,
    /// The body of each verbatim/raw block read from a fence or an indented
    /// paragraph (or a verbatim paragraph form), by the block's start: its
    /// reader range and how its lines are adjusted (Ruby's
    /// `adjust_indentation!`). A `subs` attribute converts it on completion.
    pub verbatim_bodies: BTreeMap<usize, (usize, usize, crate::indentation::Indentation)>,
    /// Ruby's `@backend`/`@doctype`, which own the derived attributes.
    profile: crate::builtin_attributes::Profile,
    /// Whether the header changed `htmlsyntax` since the last `backend`
    /// assignment (Ruby's `@attributes_modified.delete? 'htmlsyntax'`).
    htmlsyntax_modified: bool,
}

fn root_of(
    root: &mut Option<(Arc<Attributes>, BTreeSet<String>)>,
) -> Option<(&mut Attributes, &BTreeSet<String>)> {
    root.as_mut()
        .map(|(attributes, locked)| (Arc::make_mut(attributes), &*locked))
}

/// The parent document's reading state while a nested document is read.
pub(crate) struct Outer {
    attributes: Arc<Attributes>,
    replay: Option<Arc<Attributes>>,
    locked: BTreeSet<String>,
    doctitle_span: Option<crate::Span>,
    open_sections: Vec<(u8, (usize, usize))>,
    entries: Vec<crate::body_subs::Declaration>,
    book: bool,
    manpage: bool,
    outfilesuffix: Option<String>,
    nested: bool,
    /// Whether this level made the parent the root document.
    root: bool,
    footnotes: crate::substitution_context::Footnotes,
    profile: crate::builtin_attributes::Profile,
    htmlsyntax_modified: bool,
}

/// Ruby's `DEFAULT_ATTRIBUTES` that adoctra models, with the compliance
/// defaults Ruby assigns before them.
fn default_attributes() -> Attributes {
    let mut attributes = Attributes::new();
    attributes.set("attribute-undefined", "drop-line");
    attributes.set("attribute-missing", "skip");
    attributes.set("example-caption", "Example");
    attributes.set("figure-caption", "Figure");
    attributes.set("table-caption", "Table");
    // Ruby's DEFAULT_ATTRIBUTES for section and appendix signifiers.
    attributes.set("appendix-caption", "Appendix");
    for (name, value) in [
        ("appendix", "Appendix"),
        ("chapter", "Chapter"),
        ("part", "Part"),
        ("section", "Section"),
    ] {
        attributes.set(format!("{name}-refsig"), value);
    }
    // Ruby's DEFAULT_ATTRIBUTES for the table of contents.
    attributes.set("toc-placement", "auto");
    attributes.set("toc-title", "Table of Contents");
    // Ruby's DEFAULT_ATTRIBUTES: verbatim blocks wrap unless unset.
    attributes.set("prewrap", "");
    // Ruby's DEFAULT_ATTRIBUTES: sections get automatic ids unless unset.
    attributes.set("sectids", "");
    // Labels of Ruby's standalone header and footer.
    attributes.set("last-update-label", "Last updated");
    attributes.set("untitled-label", "Untitled");
    attributes.set("version-label", "Version");
    for kind in [
        crate::ast::AdmonitionKind::Note,
        crate::ast::AdmonitionKind::Tip,
        crate::ast::AdmonitionKind::Important,
        crate::ast::AdmonitionKind::Warning,
        crate::ast::AdmonitionKind::Caution,
    ] {
        attributes.set(format!("{}-caption", kind.as_str()), kind.label());
    }
    attributes
}

impl ParseContext {
    pub fn new(options: &ParseOptions) -> Self {
        let mut attributes = default_attributes();
        let mut locked = BTreeSet::new();
        // Ruby's standalone defaults, before the host overrides.
        if !options.embedded {
            for key in ["copycss", "iconfont-remote", "stylesheet", "webfonts"] {
                attributes.set(key, "");
            }
        }
        for (key, value) in options.attributes.iter() {
            let effective_key = attribute_name(key);
            attributes.set_parts(
                effective_key,
                options
                    .attributes
                    .raw_parts(key)
                    .map_or_else(|| vec![AttributePart::Raw(value.to_owned())], <[_]>::to_vec),
            );
            locked.insert(effective_key.to_string());
        }
        // Ruby syncs the `embedded` override with the mode; embedded content
        // hides the title unless the host shows it, and a host `showtitle` or
        // `notitle` locks the other key unset.
        locked.insert("embedded".to_owned());
        if options.embedded {
            attributes.set("embedded", "");
            if locked.contains("showtitle") {
                locked.insert("notitle".to_owned());
            } else if locked.contains("notitle") {
                locked.insert("showtitle".to_owned());
            } else {
                attributes.set("notitle", "");
            }
        } else {
            attributes.remove("embedded");
        }
        // Ruby's initial backend and doctype (the backend's `outfilesuffix`
        // is set before the header); the manpage backend locks the doctype.
        let mut profile = crate::builtin_attributes::Profile::default();
        if profile.initialize(&mut attributes, locked.contains("outfilesuffix")) {
            locked.insert("doctype".to_owned());
        }
        // Ruby's fallback stylesheet directory.
        if attributes.get("stylesdir").is_none() {
            attributes.set("stylesdir", ".");
        }
        // Ruby's fallback icon directory, from the host's `imagesdir` only.
        if attributes.get("iconsdir").is_none() {
            let images = attributes.get("imagesdir").unwrap_or("./images");
            attributes.set("iconsdir", format!("{images}/icons"));
        }
        if let Some(dates) = &options.dates {
            fill_dates(&mut attributes, dates);
        }
        Self {
            quote_compat: options.attributes.get("compat-mode").is_some(),
            attributes: Arc::new(attributes),
            doctitle_span: None,
            locked,
            counters: BTreeMap::new(),
            replay: None,
            body_subs: Default::default(),
            comments: Vec::new(),
            comment_diagnostics: Vec::new(),
            inline_diagnostics: Vec::new(),
            anchors: Vec::new(),
            reftexts: BTreeMap::new(),
            numerals: BTreeMap::new(),
            credits: BTreeSet::new(),
            sections: BTreeMap::new(),
            book: false,
            manpage: false,
            open_sections: Vec::new(),
            definitions: Vec::new(),
            section_reftext: None,
            block_macro: None,
            outfilesuffix: None,
            nested: false,
            root: None,
            footnotes: Default::default(),
            callouts: Default::default(),
            verbatim_bodies: BTreeMap::new(),
            profile,
            htmlsyntax_modified: false,
        }
    }

    /// Start reading a nested document (Ruby's `Document.new` with a parent):
    /// the parent's attributes (without `doctitle`, `notitle`, `showtitle`,
    /// `toc` and `toc-position`), all locked except
    /// `compat-mode`, `doctype` and `toc-placement`. Like Ruby (`unless
    /// parent_doc`), defaults are not applied again, so an attribute the parent
    /// unset stays unset. Counters and catalogs stay shared.
    pub fn enter_nested(&mut self) -> Outer {
        let mut attributes = (*self.attributes).clone();
        for key in ["doctitle", "notitle", "showtitle", "toc", "toc-position"] {
            attributes.remove(key);
        }
        let mut locked = self.locked.clone();
        locked.extend(
            attributes
                .iter()
                .map(|(key, _)| key.to_owned())
                .filter(|key| !matches!(key.as_str(), "compat-mode" | "doctype" | "toc-placement")),
        );
        if attributes.get("toc-placement").is_none() {
            attributes.set("toc-placement", "auto");
        }
        // After the locks: the derived keys it removes stay locked, the ones
        // it adds do not (Ruby updates them after copying the overrides).
        let profile = crate::builtin_attributes::Profile::nested(&mut attributes);
        let root = self.root.is_none();
        let parent = std::mem::replace(&mut self.attributes, Arc::new(attributes));
        if root {
            self.root = Some((Arc::clone(&parent), self.locked.clone()));
        }
        Outer {
            footnotes: std::mem::take(&mut self.footnotes),
            root,
            attributes: parent,
            replay: self.replay.take(),
            locked: std::mem::replace(&mut self.locked, locked),
            doctitle_span: self.doctitle_span.take(),
            open_sections: std::mem::take(&mut self.open_sections),
            entries: std::mem::take(&mut self.body_subs.entries),
            book: self.book,
            manpage: self.manpage,
            outfilesuffix: self.outfilesuffix.clone(),
            nested: std::mem::replace(&mut self.nested, true),
            profile: std::mem::replace(&mut self.profile, profile),
            htmlsyntax_modified: std::mem::take(&mut self.htmlsyntax_modified),
        }
    }

    /// Finish a nested document read for the cell at `key`: keep its
    /// conversion state and restore the parent's reading state.
    /// `header` is the nested header snapshot taken when its body started.
    pub fn exit_nested(&mut self, outer: Outer, key: (usize, usize), header: Attributes) {
        let nested = crate::body_subs::Nested {
            attributes: header,
            entries: std::mem::replace(&mut self.body_subs.entries, outer.entries),
            locked: std::mem::replace(&mut self.locked, outer.locked),
            footnotes: std::mem::replace(&mut self.footnotes, outer.footnotes),
        };
        self.body_subs.nested.insert(key, nested);
        self.attributes = if outer.root {
            self.root
                .take()
                .map_or(outer.attributes, |(attributes, _)| attributes)
        } else {
            outer.attributes
        };
        self.replay = outer.replay;
        self.doctitle_span = outer.doctitle_span;
        self.open_sections = outer.open_sections;
        self.book = outer.book;
        self.manpage = outer.manpage;
        self.outfilesuffix = outer.outfilesuffix;
        self.nested = outer.nested;
        self.profile = outer.profile;
        self.htmlsyntax_modified = outer.htmlsyntax_modified;
    }

    /// Ruby's `Document#doctype`: the doctype the header last assigned,
    /// even if the attribute was unset since.
    pub fn doctype(&self) -> Option<&str> {
        self.profile.doctype.as_deref()
    }

    pub fn start_body(&mut self) {
        self.replay = Some(Arc::clone(&self.attributes));
        // Ruby reads the doctype (`@doctype`, not the attribute) once the
        // header is complete.
        let doctype = self.profile.doctype.as_deref();
        self.book = doctype == Some("book");
        self.manpage = doctype == Some("manpage") && !self.nested;
        self.outfilesuffix = self.attributes.get("outfilesuffix").map(str::to_owned);
    }

    fn xrefs<'a>(
        outfilesuffix: &'a Option<String>,
        includes: &'a crate::xref_target::Includes,
    ) -> crate::xref_target::Scope<'a> {
        crate::xref_target::Scope {
            includes,
            outfilesuffix: outfilesuffix.as_deref(),
        }
    }

    pub fn render_attributes(&self) -> Arc<Attributes> {
        Arc::clone(self.replay.as_ref().unwrap_or(&self.attributes))
    }

    /// Build the reference registry after reading (see resolve.rs).
    pub fn register_references(
        &mut self,
        document: &mut crate::Document,
        includes: &crate::xref_target::Includes,
        diagnostics: &mut Vec<crate::Diagnostic>,
    ) -> crate::resolve::Registered {
        let read = crate::resolve::Read {
            anchors: std::mem::take(&mut self.anchors),
            reftexts: std::mem::take(&mut self.reftexts),
            numerals: std::mem::take(&mut self.numerals),
            credits: std::mem::take(&mut self.credits),
            sections: &self.sections,
            definitions: std::mem::take(&mut self.definitions),
        };
        let (body_subs, locked, counters) = (&mut self.body_subs, &self.locked, &self.counters);
        let inline_diagnostics = &mut self.inline_diagnostics;
        let xrefs = Self::xrefs(&self.outfilesuffix, includes);
        crate::resolve::register(
            document,
            read,
            &mut |key, attributes| {
                body_subs.convert_early(
                    key,
                    attributes,
                    locked,
                    counters,
                    xrefs,
                    inline_diagnostics,
                )
            },
            diagnostics,
        )
    }

    /// Convert deferred fields and resolve cross-references in document order.
    /// Unresolved-reference warnings go to `references`.
    pub fn execute_body(
        &mut self,
        document: &mut crate::Document,
        (registry, early_titles, fixed): crate::resolve::Registered,
        includes: &crate::xref_target::Includes,
        references: &mut Vec<crate::Diagnostic>,
    ) {
        document.final_attributes = std::mem::take(&mut self.body_subs).execute(
            &mut document.blocks,
            document.attributes.clone(),
            crate::body_subs::Conversion {
                locked: &self.locked,
                counters: &mut self.counters,
                diagnostics: &mut self.inline_diagnostics,
                registry: &registry,
                early_titles,
                fixed,
                sections: &self.sections,
                references,
                xrefs: Self::xrefs(&self.outfilesuffix, includes),
                footnotes: std::mem::take(&mut self.footnotes),
            },
        );
    }

    pub fn implicit_doctitle(&mut self, value: &str, span: crate::Span) -> String {
        let mut evaluation = crate::substitution_context::Evaluation::definition(
            &self.attributes,
            &self.locked,
            &mut self.counters,
            self.quote_compat,
        )
        .with_root(root_of(&mut self.root))
        .with_footnotes(&mut self.footnotes);
        let parts = crate::inline_attributes::implicit_title(
            value,
            &mut evaluation,
            span,
            &mut self.inline_diagnostics,
        );
        if let Some(attributes) = evaluation.changed_attributes() {
            self.attributes = Arc::new(attributes);
        }
        parts_text(&parts)
    }

    /// Keep the read reftext for references; the model gets a snapshot.
    pub fn attach_reftext(&mut self, block: &mut crate::Block, value: crate::reftext::Read) {
        block.reftext = Some(value.snapshot(&self.attributes));
        self.reftexts
            .insert((block.span.start, block.span.end), value);
    }

    pub fn block_reftext(&mut self, value: &str, base: usize) -> crate::reftext::Read {
        let mut evaluation = crate::substitution_context::Evaluation::definition(
            &self.attributes,
            &self.locked,
            &mut self.counters,
            self.quote_compat,
        )
        .with_root(root_of(&mut self.root))
        .with_footnotes(&mut self.footnotes);
        let result =
            crate::reftext::read(value, base, &mut evaluation, &mut self.inline_diagnostics);
        if let Some(attributes) = evaluation.changed_attributes() {
            self.attributes = Arc::new(attributes);
        }
        result
    }

    /// The early a-stage of a block attribute line (`sub_input`): the whole
    /// line, with the document missing policy and stateful expressions.
    pub fn block_attribute_line(
        &mut self,
        source: crate::substitution_text::Text,
    ) -> (
        crate::substitution_text::Text,
        Vec<crate::inline_attributes::Reference>,
    ) {
        let mut evaluation = crate::substitution_context::Evaluation::definition(
            &self.attributes,
            &self.locked,
            &mut self.counters,
            self.quote_compat,
        )
        .with_root(root_of(&mut self.root))
        .with_footnotes(&mut self.footnotes);
        let result =
            crate::inline_attributes::apply(source, &mut evaluation, &mut self.inline_diagnostics);
        if let Some(attributes) = evaluation.changed_attributes() {
            self.attributes = Arc::new(attributes);
        }
        result
    }

    /// Normal substitutions of a single-quoted attribute list value while
    /// reading. A value changed by the early a-stage no longer has exact source
    /// offsets: its nodes and diagnostics take the field's physical range.
    pub fn attribute_value(
        &mut self,
        value: &str,
        span: crate::Span,
        exact: bool,
        includes: &crate::xref_target::Includes,
    ) -> Vec<crate::Inline> {
        let before = self.inline_diagnostics.len();
        let attributes = Arc::clone(&self.attributes);
        let mut inlines = self.reading_title(value, span.start, &attributes, includes);
        if !exact {
            struct Origin(crate::Span);
            impl crate::visit::VisitorMut for Origin {
                fn visit_span(&mut self, span: &mut crate::Span) {
                    *span = self.0;
                }
            }
            crate::visit::walk_inlines_mut(&mut Origin(span), &mut inlines);
            for diagnostic in &mut self.inline_diagnostics[before..] {
                diagnostic.span = span;
            }
        }
        inlines
    }

    /// Section titles and anchored block titles are cached during reading.
    /// Their effects are not declarations to replay at conversion.
    pub fn reading_title(
        &mut self,
        value: &str,
        base: usize,
        attributes: &Attributes,
        includes: &crate::xref_target::Includes,
    ) -> Vec<crate::Inline> {
        let mut evaluation = crate::substitution_context::Evaluation::definition(
            attributes,
            &self.locked,
            &mut self.counters,
            attributes.get("compat-mode").is_some(),
        )
        .with_root(root_of(&mut self.root))
        .with_footnotes(&mut self.footnotes)
        .with_xrefs(Self::xrefs(&self.outfilesuffix, includes));
        let inlines = crate::inline::scan_inlines_evaluated(
            value,
            base,
            &mut evaluation,
            true,
            &mut self.inline_diagnostics,
        );
        if let Some(mut attributes) = evaluation.changed_attributes() {
            attributes.local_hardbreaks = false;
            self.attributes = Arc::new(attributes);
        }
        inlines
    }

    /// An admonition's label while reading: its own `caption`, else the
    /// document's `<kind>-caption` (empty when unset).
    pub fn admonition_label(
        &self,
        kind: crate::ast::AdmonitionKind,
        attrs: Option<&crate::ast::BlockAttrs>,
    ) -> Vec<crate::AttributePart> {
        match attrs.and_then(|attrs| attrs.named.get("caption")) {
            Some(caption) => vec![crate::AttributePart::Raw(caption.clone())],
            None => self
                .attributes
                .value_parts(&format!("{}-caption", kind.as_str()))
                .unwrap_or_default(),
        }
    }

    /// Example and figure captions are fixed in child-before-parent completion
    /// order (Ruby's `assign_caption` with `CAPTION_ATTRIBUTE_NAMES`). Their
    /// counter assignment is replayed when entering the owning block,
    /// independently of source-ordered explicit attribute declarations.
    pub fn assign_caption(&mut self, block: &mut crate::Block) {
        let Some(context) = caption_context(&block.kind).filter(|_| block.title.is_some()) else {
            return;
        };
        // Ruby's `:example` branch of `next_block` sets `caption` to '' for a
        // collapsible example, so it is never numbered.
        if context == "example" && crate::parser::has_option(&block.attrs, "collapsible") {
            block.caption = Some(String::new());
            return;
        }
        if let Some(caption) = block
            .attrs
            .named
            .get("caption")
            .map(String::as_str)
            .or_else(|| self.attributes.get("caption"))
        {
            block.caption = Some(caption.to_owned());
            return;
        }
        let Some(prefix) = self
            .attributes
            .get(&format!("{context}-caption"))
            .map(str::to_owned)
        else {
            return;
        };
        let mut evaluation = crate::substitution_context::Evaluation::definition(
            &self.attributes,
            &self.locked,
            &mut self.counters,
            self.quote_compat,
        )
        .with_root(root_of(&mut self.root))
        .with_footnotes(&mut self.footnotes);
        let effect = evaluation.execute(
            &format!("counter:{context}-number"),
            block.span,
            &mut None,
            &mut self.inline_diagnostics,
        );
        let number = parts_text(&effect.parts);
        if let Some(attributes) = evaluation.changed_attributes() {
            self.attributes = Arc::new(attributes);
        }
        block.caption = Some(format!("{prefix} {number}. "));
        self.numerals
            .insert((block.span.start, block.span.end), number);
        self.body_subs
            .caption_counters
            .insert((block.span.start, block.span.end), (context, effect.parts));
    }

    pub fn bibliography_section(&self) -> bool {
        self.open_sections
            .last()
            .and_then(|(_, key)| self.sections.get(key))
            .is_some_and(|section| section.sectname == "bibliography")
    }

    /// Read a section title line at `key` (after `close_sections`, before its
    /// title is converted): decide its kind and numbering with the current
    /// attributes and the enclosing section; returns the section's level.
    pub fn read_section(
        &mut self,
        key: (usize, usize),
        style: Option<&str>,
        level: u8,
        title: &str,
    ) -> u8 {
        let parent = self
            .open_sections
            .last()
            .and_then(|(_, parent)| self.sections.get(parent));
        // Ruby compares the source title (`casecmp`) of an unstyled section.
        let synopsis = self.manpage && title.eq_ignore_ascii_case("synopsis");
        let section = crate::section_numbers::Section::read(
            style,
            level,
            self.book,
            synopsis,
            &self.attributes,
            parent,
        );
        let level = section.level;
        self.sections.insert(key, section);
        self.open_sections.push((level, key));
        level
    }

    /// A section or discrete heading without an id gets one generated after
    /// its title converts: like Ruby's `Section.generate_id`, a longer
    /// `idseparator` is cut to its first character for later reading
    /// (whatever the locks; conversion starts again from the header).
    pub fn generating_id(&mut self) {
        if self.attributes.get("sectids").is_none() {
            return;
        }
        let Some(separator) = self.attributes.get("idseparator") else {
            return;
        };
        let mut chars = separator.chars();
        if let (Some(first), Some(_)) = (chars.next(), chars.next()) {
            Arc::make_mut(&mut self.attributes).set("idseparator", first.to_string());
        }
    }

    /// Close open sections at `level` or deeper (all with level 0 at the end
    /// of the body), before the next section title at `at` is read. A closed chapter
    /// takes the next `chapter-number` counter value and an appendix the next
    /// `appendix-number` (from `A`) and its caption, like Ruby's
    /// `assign_numeral`; the effect reaches later reading only (conversion
    /// starts again from the header attributes, as in Ruby).
    pub fn close_sections(&mut self, level: u8, at: usize) {
        while self
            .open_sections
            .last()
            .is_some_and(|open| open.0 >= level)
        {
            let (_, key) = self.open_sections.pop().unwrap();
            let Some(section) = self.sections.get_mut(&key) else {
                continue;
            };
            section.closed_at = at;
            let appendix = section.counts_appendix();
            let counter = if appendix {
                "counter:appendix-number:A"
            } else if section.counts_chapter() {
                "counter:chapter-number"
            } else {
                continue;
            };
            let mut evaluation = crate::substitution_context::Evaluation::definition(
                &self.attributes,
                &self.locked,
                &mut self.counters,
                self.quote_compat,
            )
            .with_root(root_of(&mut self.root))
            .with_footnotes(&mut self.footnotes);
            let effect = evaluation.execute(
                counter,
                crate::Span::new(key.0, key.1),
                &mut None,
                &mut self.inline_diagnostics,
            );
            if let Some(attributes) = evaluation.changed_attributes() {
                self.attributes = Arc::new(attributes);
            }
            let numeral = parts_text(&effect.parts);
            let caption = appendix.then(|| match self.attributes.get("appendix-caption") {
                Some(caption) => format!("{caption} {numeral}: "),
                None => format!("{numeral}. "),
            });
            if let Some(section) = self.sections.get_mut(&key) {
                section.numeral = Some(numeral);
                section.caption = caption;
            }
        }
    }

    pub fn apply(&mut self, entry: AttributeEntry) {
        self.apply_at(entry, crate::Span::new(0, 0));
    }

    fn apply_at(&mut self, entry: AttributeEntry, span: crate::Span) {
        let (key, value) = match entry {
            AttributeEntry::Set(key, value) => (key, Some(value)),
            AttributeEntry::Unset(key) => (key, None),
        };
        // Ruby's `store_attribute` stores the inverse `notitle` first.
        if key == "showtitle" {
            let notitle = match value {
                Some(_) => AttributeEntry::Unset("notitle".to_owned()),
                None => AttributeEntry::Set("notitle".to_owned(), String::new()),
            };
            self.apply_at(notitle, span);
        }
        if self.locked.contains(&key) {
            return;
        }
        let mut value = value.map(|v| {
            let mut evaluation = crate::substitution_context::Evaluation::definition(
                &self.attributes,
                &self.locked,
                &mut self.counters,
                self.quote_compat,
            )
            .with_root(root_of(&mut self.root))
            .with_footnotes(&mut self.footnotes);
            let parts = attribute_entry_parts_report(
                &v,
                &mut evaluation,
                self.quote_compat,
                span,
                &mut self.inline_diagnostics,
            );
            if let Some(attributes) = evaluation.changed_attributes() {
                self.attributes = Arc::new(attributes);
            }
            let value = parts_text(&parts);
            if key == "leveloffset" {
                let current = self
                    .attributes
                    .get("leveloffset")
                    .map_or(0, crate::include_selection::integer_prefix);
                if let Some(relative) = value.strip_prefix('+') {
                    return vec![AttributePart::Text(
                        current
                            .saturating_add(crate::include_selection::integer_prefix(relative))
                            .to_string(),
                    )];
                }
                if let Some(relative) = value.strip_prefix('-') {
                    return vec![AttributePart::Text(
                        current
                            .saturating_sub(crate::include_selection::integer_prefix(relative))
                            .to_string(),
                    )];
                }
            }
            parts
        });
        if let Some(parts) = &mut value {
            self.fix_xrefs(parts, span);
        }
        // Conversion replays the declared result, not side effects incurred
        // while computing it during reading. Header side effects are already
        // captured when start_body creates this independent snapshot.
        if let Some(replay) = &mut self.replay {
            self.body_subs
                .entries
                .push((span.start, key.clone(), value.clone()));
            let attrs = Arc::make_mut(replay);
            if let Some(parts) = &value {
                attrs.set_parts(&key, parts.clone());
            } else {
                attrs.remove(&key);
            }
        }
        let attributes = Arc::make_mut(&mut self.attributes);
        // In the header, Ruby's set_attribute updates the backend and doctype
        // with their derived attributes.
        if self.replay.is_none() {
            match (key.as_str(), &value) {
                ("backend", Some(parts)) => {
                    let backend = parts_text(parts);
                    let init = std::mem::take(&mut self.htmlsyntax_modified)
                        && self.profile.backend.as_deref() == Some(backend.as_str());
                    let locked = self.locked.contains("outfilesuffix");
                    self.profile
                        .update_backend(attributes, &backend, init, locked);
                    return;
                }
                ("doctype", Some(parts)) => {
                    self.profile.update_doctype(attributes, &parts_text(parts));
                    return;
                }
                ("htmlsyntax", _) => self.htmlsyntax_modified = true,
                _ => {}
            }
        }
        match value {
            Some(value) => attributes.set_parts(key, value),
            None => {
                attributes.remove(&key);
            }
        }
    }

    /// Tag the references this definition generated so that registration
    /// resolves them at the definition (references copied from other values
    /// keep their own tag).
    fn fix_xrefs(&mut self, parts: &mut [AttributePart], span: crate::Span) {
        for i in 0..parts.len() {
            let textless = matches!(parts.get(i + 1), Some(AttributePart::CloseXref));
            if let AttributePart::OpenXref(xref) = &mut parts[i] {
                if xref.definition.is_none() {
                    xref.definition = Some(self.definitions.len());
                    self.definitions.push(crate::resolve::Definition {
                        span,
                        attributes: Arc::clone(&self.attributes),
                        xref: xref.clone(),
                        textless,
                    });
                }
            }
        }
    }

    pub fn section_level(&self, level: u8) -> i128 {
        self.attributes
            .get("leveloffset")
            .map_or(0, crate::include_selection::integer_prefix)
            .saturating_add(i128::from(level))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum AttributeEntry {
    Set(String, String),
    Unset(String),
}

/// Consume an entry at a parser-approved block/header boundary. Preprocessing
/// sees the old context until the entire value has been collected, as in the
/// pinned Parser.process_attribute_entry. Record each physical reader line so
/// highlighting does not have to guess continuation or container boundaries.
pub(crate) fn consume_attribute_entry(
    lines: &mut crate::reader::Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
) -> bool {
    let Some(mut entry) = parse_attribute_entry(&lines[*i].1) else {
        return false;
    };
    let first = lines.attribute_entries.len();
    let mut value = lines[*i].1[1..]
        .split_once(':')
        .unwrap()
        .1
        .trim_start_matches([' ', '\t'])
        .to_owned();
    let continuation = [" \\", " +"].into_iter().find(|con| value.ends_with(con));
    loop {
        let (off, line) = lines.line(*i);
        lines
            .attribute_entries
            .push((crate::Span::new(off, off + line.len()), None));
        *i += 1;
        let Some(con) = continuation else { break };
        value.truncate(value.len() - con.len());
        value.truncate(value.trim_end_matches(ruby_space).len());
        if !lines.has(*i, context) {
            break;
        }
        if lines[*i].1.is_empty() {
            // Ruby advances past the blank line that terminated continuation.
            *i += 1;
            break;
        }
        let next = lines[*i].1.trim_start_matches(ruby_space);
        value.push(if value.ends_with(" +") { '\n' } else { ' ' });
        value.push_str(next);
        if !next.ends_with(con) {
            let (off, line) = lines.line(*i);
            lines
                .attribute_entries
                .push((crate::Span::new(off, off + line.len()), None));
            *i += 1;
            break;
        }
    }
    if let AttributeEntry::Set(_, ref mut current) = entry {
        *current = value;
    }
    lines.attribute_entries[first].1 = Some(entry.clone());
    if matches!(&entry, AttributeEntry::Set(key, _) | AttributeEntry::Unset(key) if key == "doctitle")
        && !context.locked.contains("doctitle")
    {
        context.doctitle_span = Some(crate::Span::new(
            lines.attribute_entries[first].0.start,
            lines.attribute_entries.last().unwrap().0.end,
        ));
    }
    context.apply_at(
        entry,
        crate::Span::new(
            lines.attribute_entries[first].0.start,
            lines.attribute_entries.last().unwrap().0.end,
        ),
    );
    true
}

fn ruby_space(c: char) -> bool {
    matches!(c, '\0' | '\t'..='\r' | ' ')
}

// Ruby aliases both document entries and host overrides before locking keys.
fn attribute_name(key: &str) -> &str {
    match key {
        "hardbreaks" => "hardbreaks-option",
        "numbered" => "sectnums",
        _ => key,
    }
}

pub(crate) fn parse_attribute_entry(line: &str) -> Option<AttributeEntry> {
    let rest = line.trim_end().strip_prefix(':')?;
    let (key, value) = rest.split_once(':')?;
    if !value.is_empty() && !value.starts_with([' ', '\t']) {
        return None;
    }
    let (key, unset) = if let Some(key) = key.strip_suffix('!') {
        (key, true)
    } else if let Some(key) = key.strip_prefix('!') {
        (key, true)
    } else {
        (key, false)
    };
    let first = key.chars().next()?;
    if !crate::unicode_word::is_word(first) {
        return None;
    }
    // Ruby Parser.sanitize_attribute_name removes punctuation and spaces.
    let key: String = key
        .chars()
        .filter(|&c| crate::unicode_word::is_word(c) || c == '-')
        .collect();
    let key = key.to_lowercase();
    let key = attribute_name(&key).to_owned();
    Some(if unset {
        AttributeEntry::Unset(key)
    } else {
        AttributeEntry::Set(key, value.trim().to_owned())
    })
}

/// A whole-value pass macro can select literal protection and attribute
/// substitution, quotes, replacements and link macros independently.
pub(crate) fn attribute_entry_parts_report(
    value: &str,
    attributes: &mut crate::substitution_context::Evaluation<'_>,
    compat: bool,
    span: crate::Span,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> Vec<AttributePart> {
    if let Some((subs, body)) = value
        .strip_prefix("pass:")
        .and_then(|v| v.split_once('['))
        .and_then(|(subs, body)| body.strip_suffix(']').map(|body| (subs, body)))
        .filter(|(subs, _)| crate::pass_subs::syntax(subs))
    {
        return crate::inline_pass::Passes::selected(
            body,
            subs,
            attributes,
            compat,
            span,
            diagnostics,
        );
    }
    let policy = attributes
        .get("attribute-missing")
        .unwrap_or("skip")
        .to_owned();
    crate::inline_attributes::parts(
        &[AttributePart::Text(value.into())],
        attributes,
        span,
        &policy,
        diagnostics,
    )
}

pub(crate) fn substitute_attribute_parts(
    value: &str,
    attributes: &Attributes,
    raw: bool,
) -> Vec<AttributePart> {
    // String-only header callers explicitly retain unknown references. Actual
    // assignments use attribute_entry_parts_report and the document policy.
    crate::inline_attributes::parts(
        &[if raw {
            AttributePart::Raw(value.into())
        } else {
            AttributePart::Text(value.into())
        }],
        &mut crate::substitution_context::Evaluation::read_only(attributes),
        crate::Span::new(0, 0),
        "skip",
        &mut Vec::new(),
    )
}

/// Bounds for recursive resource expansion (the primary source is excluded).
#[derive(Debug, Clone, Copy)]
pub struct IncludeLimits {
    pub max_depth: usize,
    /// Loaded snapshots plus indentation/tab expansion growth, counted per include.
    pub max_bytes: usize,
}

impl Default for IncludeLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

pub(crate) fn intrinsic(name: &str) -> Option<&'static str> {
    Some(match name {
        "startsb" => "[",
        "endsb" => "]",
        "vbar" => "|",
        "caret" => "^",
        "asterisk" => "*",
        "tilde" => "~",
        "plus" => "&#43;",
        "backslash" => "\\",
        "backtick" => "`",
        "blank" | "empty" => "",
        "sp" => " ",
        "two-colons" => "::",
        "two-semicolons" => ";;",
        "nbsp" => "&#160;",
        "deg" => "&#176;",
        "zwsp" => "&#8203;",
        "quot" => "&#34;",
        "apos" => "&#39;",
        "lsquo" => "&#8216;",
        "rsquo" => "&#8217;",
        "ldquo" => "&#8220;",
        "rdquo" => "&#8221;",
        "wj" => "&#8288;",
        "brvbar" => "&#166;",
        "pp" => "&#43;&#43;",
        "cpp" => "C&#43;&#43;",
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        _ => return None,
    })
}
