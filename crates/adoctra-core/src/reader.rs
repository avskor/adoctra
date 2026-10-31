//! Lazy, source-preserving conditional and include preprocessing. Attribute mutations belong
//! to the parser; each line is evaluated once, when that parser requests it.
use crate::context::ParseContext;
use crate::include_indent::Line;
use crate::ruby_strip::RubyStrip;
use crate::scanner::FilteredSource;
use crate::{Attributes, Diagnostic, Inline, Severity, Span};
use std::{ops::Index, sync::Arc};

struct Conditional {
    target: String,
    active: bool,
    span: Span,
}

struct Input {
    source_id: crate::SourceId,
    lines: Vec<Line>,
    /// Ruby's `@maxdepth[:curr]` (compared to the include stack size) and
    /// `[:rel]` (the depth its error reports).
    max_depth: usize,
    depth_label: usize,
    next: usize,
    preprocess: bool,
    occurrence: usize,
    generated_at: Option<Span>,
    start_line: isize,
}

impl Input {
    /// Ruby's `skip_front_matter!` on prepared lines: a first `---` line,
    /// the lines up to the next `---` and that line are skipped, and their
    /// text (joined with LF) is returned. Without a closing line nothing is.
    fn skip_front_matter(&mut self) -> Option<String> {
        let lines = &self.lines[self.next..];
        if lines.first().map(|line| &*line.text) != Some("---") {
            return None;
        }
        let close = lines[1..].iter().position(|line| &*line.text == "---")? + 1;
        let front_matter = lines[1..close]
            .iter()
            .map(|line| &*line.text)
            .collect::<Vec<_>>()
            .join("\n");
        self.next += close + 1;
        Some(front_matter)
    }

    fn new(source: &str, source_id: crate::SourceId, preprocess: bool, occurrence: usize) -> Self {
        let lines = Line::from_source(source);
        Self {
            source_id,
            lines,
            max_depth: usize::MAX,
            depth_label: usize::MAX,
            next: 0,
            preprocess,
            occurrence,
            generated_at: None,
            start_line: 1,
        }
    }
}

struct Resources<'a> {
    sources: &'a mut crate::SourceMap,
    provider: &'a dyn crate::ResourceProvider,
    limits: crate::IncludeLimits,
    /// Ruby's `@maxdepth[:abs]`: `max-include-depth` from the host attributes
    /// (default 64), bounded by the host limit; `None` (a value that is not
    /// positive) disables includes.
    max_depth: Option<usize>,
    bytes: usize,
    /// Ruby's `@base_dir`: the host's absolute `ParseOptions::base_dir`, else
    /// an absolute `docdir` (the primary file's directory, or the host's for
    /// standard input). With it, include targets resolve like Ruby's
    /// `normalize_system_path`.
    base_dir: Option<String>,
    safe: crate::SafeMode,
}

struct Origin {
    start: usize,
    end: usize,
    span: Span,
    occurrence: usize,
    generated: bool,
    // Expanded tabs map as a unit; unchanged text maps byte for byte.
    exact: bool,
}

pub(crate) struct Reader<'a> {
    report_cursors: Vec<(usize, crate::DiagnosticCursor)>,
    primary_eof: crate::DiagnosticCursor,
    inputs: Vec<Input>,
    /// Offsets of Ruby's list continuation placeholders in a list item's
    /// lines: empty lines that still count as a `+` marker when a nested
    /// list reads them (Ruby keeps the marker object).
    pub placeholders: std::collections::BTreeSet<usize>,
    resources: Option<Resources<'a>>,
    /// Ruby's `@maxdepth` is set (a positive host `max-include-depth`) for a
    /// reader without a provider: nothing is read, but a URI target Ruby may
    /// not read still becomes a link (see `expand_include`).
    unread_includes: bool,
    origins: Vec<Origin>,
    next_offset: usize,
    next_occurrence: usize,
    pub loaded: Vec<(usize, Arc<str>)>,
    pub attribute_entries: Vec<(Span, Option<crate::context::AttributeEntry>)>,
    /// Early metadata substitutions, including values later overwritten or orphaned.
    pub metadata_references: Vec<Span>,
    /// Lines of body section titles (ATX, setext title and underline), for highlighting.
    pub heading_lines: Vec<Span>,
    /// Table cell separators with specs, for highlighting.
    pub table_separators: Vec<Span>,
    /// List markers and description list delimiters, for highlighting.
    pub list_markers: Vec<Span>,
    /// The `>` markers of Markdown blockquote lines.
    pub quote_markers: Vec<Span>,
    /// Source classifications recorded during structural parsing: metadata,
    /// header lines, block fences, macros and verbatim/raw bodies.
    pub block_tokens: Vec<crate::Token>,
    /// Whole raw/verbatim bodies, including the line endings between tokens.
    pub verbatim_ranges: Vec<Span>,
    /// Callout marks converted by custom substitutions, for highlighting.
    pub callout_marks: Vec<Span>,
    endings: Vec<(usize, String)>,
    pub limit: usize,
    stack: Vec<Conditional>,
    pub comments: Vec<Span>,
    pub directives: Vec<Span>,
    pub includes: Vec<Span>,
    /// Ruby's `catalog[:includes]`, for interdocument references.
    pub include_catalog: crate::xref_target::Includes,
    pub inactive: Vec<Span>,
}

impl<'a> Reader<'a> {
    pub fn new(source: &str) -> Self {
        let mut reader = Self::plain(source);
        let input = &mut reader.inputs[0];
        input.preprocess = true;
        for line in &mut input.lines {
            line.normalize();
        }
        // Only the primary reader condenses trailing blank lines. Included
        // files retain them, since they can separate content in the parent.
        while input.lines.last().is_some_and(|line| line.text.is_empty()) {
            input.lines.pop();
        }
        reader
    }

    /// Already processed content must not be normalized again: it can contain
    /// raw includes whose trailing whitespace is significant.
    pub fn plain(source: &str) -> Self {
        Self {
            report_cursors: Vec::new(),
            primary_eof: crate::DiagnosticCursor {
                source_id: crate::SourceId::default(),
                line: 1,
            },
            inputs: vec![Input::new(source, crate::SourceId::default(), false, 0)],
            placeholders: Default::default(),
            resources: None,
            unread_includes: false,
            origins: Vec::new(),
            next_offset: 0,
            next_occurrence: 1,
            loaded: Vec::new(),
            attribute_entries: Vec::new(),
            metadata_references: Vec::new(),
            heading_lines: Vec::new(),
            table_separators: Vec::new(),
            list_markers: Vec::new(),
            quote_markers: Vec::new(),
            block_tokens: Vec::new(),
            verbatim_ranges: Vec::new(),
            callout_marks: Vec::new(),
            endings: Vec::new(),
            limit: usize::MAX,
            stack: Vec::new(),
            comments: Vec::new(),
            directives: Vec::new(),
            includes: Vec::new(),
            include_catalog: Default::default(),
            inactive: Vec::new(),
        }
    }

    /// The lines of an AsciiDoc table cell, already read by the parent (Ruby
    /// gives the nested document a plain `Reader`), at their reader offsets.
    pub fn nested(
        lines: Vec<(usize, Arc<str>, String)>,
        includes: crate::xref_target::Includes,
    ) -> Self {
        let mut reader = Self::plain("");
        reader.inputs.clear();
        reader.include_catalog = includes;
        for (off, line, eol) in lines {
            reader.endings.push((off + line.len(), eol));
            reader.loaded.push((off, line));
        }
        reader
    }

    /// The lines of a cell's text (characters at reader offsets) with their
    /// line endings.
    pub fn cell_lines(text: &[(usize, char)]) -> Vec<(usize, Arc<str>, String)> {
        let mut lines = Vec::new();
        let mut start = text.first().map_or(0, |(off, _)| *off);
        let mut line = String::new();
        for &(off, c) in text {
            if c == '\n' {
                lines.push((start, Arc::from(std::mem::take(&mut line)), "\n".into()));
                start = off + 1;
            } else {
                if line.is_empty() {
                    start = off;
                }
                line.push(c);
            }
        }
        if !text.is_empty() {
            lines.push((start, Arc::from(line), String::new()));
        }
        lines
    }

    /// Keep a sub-reader's highlighting records. The attribute entries of a
    /// nested document never apply to the parent; a list item's do.
    pub fn absorb(&mut self, nested: Reader<'_>, document: bool) {
        self.attribute_entries.extend(
            nested
                .attribute_entries
                .into_iter()
                .map(|(span, entry)| (span, entry.filter(|_| !document))),
        );
        self.attribute_entries.sort_by_key(|(span, _)| span.start);
        self.metadata_references.extend(nested.metadata_references);
        self.heading_lines.extend(nested.heading_lines);
        self.heading_lines.sort_by_key(|span| span.start);
        self.table_separators.extend(nested.table_separators);
        self.table_separators.sort_by_key(|span| span.start);
        self.list_markers.extend(nested.list_markers);
        self.block_tokens.extend(nested.block_tokens);
        self.verbatim_ranges.extend(nested.verbatim_ranges);
        self.callout_marks.extend(nested.callout_marks);
        self.list_markers.sort_by_key(|span| span.start);
        self.quote_markers.extend(nested.quote_markers);
        self.quote_markers.sort_by_key(|span| span.start);
    }

    pub fn with_resources(
        sources: &'a mut crate::SourceMap,
        options: &crate::ParseOptions,
        provider: &'a dyn crate::ResourceProvider,
    ) -> crate::ResourceResult<Self> {
        let source = sources.get(options.source_id).ok_or_else(|| {
            crate::ResourceError::new(
                crate::ResourceErrorKind::InvalidTarget,
                options.source_id.0.to_string(),
                "unknown source id",
            )
        })?;
        if !source.omitted_lines().is_empty() {
            return Err(crate::ResourceError::new(
                crate::ResourceErrorKind::InvalidEncoding,
                source.name(),
                "primary source contains omitted undecodable lines",
            ));
        }
        let mut reader = Self::new(source.text());
        reader.inputs[0].source_id = options.source_id;
        let max_depth = host_max_include_depth(options);
        let max = max_depth.unwrap_or(0);
        reader.inputs[0].max_depth = max;
        reader.inputs[0].depth_label = max;
        reader.resources = Some(Resources {
            sources,
            provider,
            limits: options.includes,
            max_depth,
            bytes: 0,
            base_dir: options
                .base_dir
                .as_deref()
                .or_else(|| options.attributes.get("docdir"))
                .filter(|dir| dir.starts_with('/'))
                .map(str::to_owned),
            safe: options.safe,
        });
        Ok(reader)
    }

    /// The provider and the primary document's canonical name, for image
    /// reads while converting (`data-uri`).
    pub fn image_source(&self) -> Option<crate::data_uri::ImageSource<'a>> {
        let resources = self.resources.as_ref()?;
        let primary = resources.sources.get(self.inputs[0].source_id)?;
        let referrer_dir = resources
            .provider
            .document_path(primary.name())
            .and_then(|path| path.rsplit_once('/').map(|(dir, _)| dir.to_owned()))
            .map(|dir| if dir.is_empty() { "/".to_owned() } else { dir });
        Some(crate::data_uri::ImageSource {
            provider: resources.provider,
            referrer: primary.name().to_owned(),
            referrer_dir,
            base_dir: resources.base_dir.clone(),
            safe: resources.safe,
        })
    }

    /// Record the opening and (when present) closing fence of a delimited block.
    pub fn record_fences(&mut self, opening: Span, closing: Option<Span>, kind: crate::TokenKind) {
        for span in std::iter::once(opening).chain(closing) {
            self.block_tokens.push(crate::Token { span, kind });
        }
    }

    /// Record a verbatim block's fences and each non-empty body line.
    pub fn record_verbatim(
        &mut self,
        opening: Span,
        closing: Option<Span>,
        (body_start, body_end): (usize, usize),
        fence: crate::TokenKind,
        text: crate::TokenKind,
    ) {
        self.record_fences(opening, closing, fence);
        self.record_text(body_start, body_end, text);
    }

    /// Classify verbatim source using the body's parser-owned boundaries.
    pub fn record_text(&mut self, body_start: usize, body_end: usize, text: crate::TokenKind) {
        self.verbatim_ranges.push(Span::new(body_start, body_end));
        let source = self.content(body_start, body_end);
        for (offset, line) in crate::scanner::lines_with_offsets(&source.text) {
            if !line.is_empty() {
                self.block_tokens.push(crate::Token {
                    span: source.original_span(Span::new(offset, offset + line.len())),
                    kind: text,
                });
            }
        }
    }

    pub fn set_primary_id(&mut self, id: crate::SourceId) {
        self.inputs[0].source_id = id;
        self.primary_eof.source_id = id;
    }

    pub fn report_cursor(&self, offset: usize) -> Option<crate::DiagnosticCursor> {
        let index = self
            .report_cursors
            .partition_point(|(start, _)| *start <= offset);
        index.checked_sub(1).map(|i| self.report_cursors[i].1)
    }

    /// Ruby's section test peeks two preprocessed lines, then pushes them back
    /// into the currently active input. Crossing include EOF changes that input.
    pub fn section_cursor(&self, offset: usize) -> Option<crate::DiagnosticCursor> {
        self.report_cursor(offset)?;
        let next = self.loaded.partition_point(|(start, _)| *start <= offset);
        let mut cursor = self
            .loaded
            .get(next)
            .and_then(|(start, _)| self.report_cursor(*start))
            .unwrap_or(self.primary_eof);
        cursor.line = cursor.line.saturating_sub(1).max(1);
        Some(cursor)
    }

    pub fn map_diagnostic(&self, diagnostic: &mut Diagnostic, provenance: &mut Vec<Vec<Span>>) {
        if diagnostic.report.location == crate::DiagnosticLocation::Source {
            if let Some(cursor) = self.report_cursor(diagnostic.span.start) {
                diagnostic.report.location = crate::DiagnosticLocation::Cursor(cursor);
            }
        }
        diagnostic.span = self.map_span(diagnostic.span, provenance);
    }

    /// Clone the shared line so callers may advance the reader while using it.
    /// The line ending of a loaded line.
    pub fn eol(&self, index: usize) -> String {
        self.endings[index].1.clone()
    }

    pub fn line(&self, index: usize) -> (usize, Arc<str>) {
        self.loaded[index].clone()
    }

    pub fn has(&mut self, index: usize, context: &mut ParseContext) -> bool {
        self.ensure(index, Some(context))
    }

    pub fn raw_has(&mut self, index: usize) -> bool {
        self.ensure(index, None)
    }

    fn ensure(&mut self, index: usize, mut context: Option<&mut ParseContext>) -> bool {
        if index >= self.limit {
            return false;
        }
        while self.loaded.len() <= index {
            if !self.step(context.as_deref_mut()) {
                return false;
            }
        }
        true
    }

    /// Read the next line of the innermost input (dropping exhausted inputs);
    /// false at the end of every input.
    fn step(&mut self, context: Option<&mut ParseContext>) -> bool {
        let Some(input) = self.inputs.last_mut() else {
            return false;
        };
        let Some(source_line) = input.lines.get(input.next) else {
            self.inputs.pop();
            return true;
        };
        input.next += 1;
        let line = source_line.text.clone();
        let eol = source_line.eol.clone();
        let off = self.next_offset;
        self.report_cursors.push((
            off,
            crate::DiagnosticCursor {
                source_id: input.source_id,
                line: (input.start_line + input.next as isize - 1).max(1) as usize,
            },
        ));
        if input.occurrence == 0 {
            self.primary_eof.line = (input.start_line + input.next as isize).max(1) as usize;
        }
        self.next_offset += line.len() + eol.len();
        let source_id = input.source_id;
        let preprocess = input.preprocess;
        for fragment in &source_line.fragments {
            self.origins.push(Origin {
                start: off + fragment.start,
                end: off + fragment.end,
                span: Span::in_source(source_id, fragment.original_start, fragment.original_end),
                occurrence: input.occurrence,
                generated: fragment.generated,
                exact: fragment.exact,
            });
        }
        if source_line.original.is_none() {
            self.origins.push(Origin {
                start: off,
                end: self.next_offset,
                span: input
                    .generated_at
                    .expect("generated include line has an origin"),
                occurrence: input.occurrence,
                generated: true,
                exact: false,
            });
        }
        if preprocess {
            if let Some(context) = context {
                let eol_start = off + line.len();
                if let Some(emitted) = self.process(off, line, context) {
                    self.endings.push((eol_start, eol));
                    self.loaded.push(emitted);
                }
                return true;
            }
        }
        self.endings.push((off + line.len(), eol));
        self.loaded.push((off, line));
        true
    }

    /// Ruby preprocesses the first line of an AsciiDoc table cell again with a
    /// fresh reader (`a|include::…[]`, a single-line `ifdef`), with its own
    /// conditional stack. The cell's lines are read again as inputs after
    /// every loaded line, so the nested document keeps increasing reader
    /// offsets; only the first line is preprocessed. `None` when the first
    /// line is unchanged.
    pub fn preprocess_cell(
        &mut self,
        text: &[(usize, char)],
        context: &mut ParseContext,
    ) -> Option<Vec<(usize, Arc<str>, String)>> {
        // Each line's characters and the offset of the line feed ending it.
        let mut lines: Vec<(crate::table::Chars, Option<usize>)> = vec![(Vec::new(), None)];
        for &(off, c) in text {
            if c == '\n' {
                lines.last_mut().unwrap().1 = Some(off);
                lines.push((Vec::new(), None));
            } else {
                lines.last_mut().unwrap().0.push((off, c));
            }
        }
        let first: String = lines[0].0.iter().map(|(_, c)| c).collect();
        if !first.contains("::") {
            return None;
        }
        let depth = self.inputs.len();
        let (max_depth, depth_label) = self
            .inputs
            .first()
            .map_or((usize::MAX, usize::MAX), |input| {
                (input.max_depth, input.depth_label)
            });
        let count = lines.len();
        for (index, (chars, feed)) in lines.into_iter().enumerate().rev() {
            let line: String = chars.iter().map(|(_, c)| c).collect();
            let mut fragments = Vec::new();
            let mut source_id = self
                .inputs
                .first()
                .map_or_else(Default::default, |i| i.source_id);
            for (run_start, run) in crate::table::fragments(&chars) {
                let at = line.len()
                    - chars
                        .iter()
                        .skip_while(|(o, _)| *o != run_start)
                        .map(|(_, c)| c.len_utf8())
                        .sum::<usize>();
                let physical = self.original_spans(Span::new(run_start, run_start + run.len()));
                let exact = physical.len() == 1 && physical[0].len() == run.len();
                source_id = physical[0].source_id;
                fragments.push(crate::include_indent::Fragment {
                    start: at,
                    end: at + run.len(),
                    original_start: physical[0].start,
                    original_end: physical.last().map_or(physical[0].end, |p| p.end),
                    exact,
                    generated: false,
                });
            }
            let eol = if feed.is_some() { "\n" } else { "" };
            if let Some(feed) = feed {
                let physical = self.original_spans(Span::new(feed, feed + 1))[0];
                fragments.push(crate::include_indent::Fragment {
                    start: line.len(),
                    end: line.len() + 1,
                    original_start: physical.start,
                    original_end: physical.end,
                    exact: physical.len() == 1,
                    generated: false,
                });
            }
            let original = fragments.first().map(|f| f.original_start);
            self.inputs.push(Input {
                source_id,
                lines: vec![Line {
                    original: original.or(Some(0)),
                    text: Arc::from(line),
                    eol: eol.into(),
                    fragments,
                }],
                max_depth: max_depth.max(depth + 1),
                depth_label,
                next: 0,
                preprocess: index == 0,
                occurrence: self.next_occurrence,
                generated_at: None,
                start_line: 1,
            });
            self.next_occurrence += 1;
        }
        let loaded = self.loaded.len();
        let stack = std::mem::take(&mut self.stack);
        while self.inputs.len() > depth {
            self.step(Some(context));
        }
        self.stack = stack;
        let endings = self.endings.split_off(loaded);
        let processed: Vec<_> = self
            .loaded
            .split_off(loaded)
            .into_iter()
            .zip(endings)
            .map(|((off, line), (_, eol))| (off, line, eol))
            .collect();
        let changed = processed.len() != count
            || processed
                .first()
                .is_some_and(|(_, line, _)| **line != *first);
        changed.then_some(processed)
    }

    /// Physical ranges in reading order. A return to the same file or a repeated
    /// inclusion remains a separate occurrence, even if the offsets coincide.
    pub fn original_spans(&self, span: Span) -> Vec<Span> {
        let first = self.origins.partition_point(|o| o.end <= span.start);
        let mut out: Vec<Span> = Vec::new();
        let mut occurrence = None;
        let mut generated_at = None;
        for origin in &self.origins[first..] {
            if origin.start >= span.end && !(span.is_empty() && origin.start == span.start) {
                break;
            }
            if origin.generated {
                generated_at = Some(origin.span);
                occurrence = None;
                continue;
            }
            let (start, end) = if origin.exact {
                (
                    origin.span.start
                        + span
                            .start
                            .saturating_sub(origin.start)
                            .min(origin.span.len()),
                    origin.span.start
                        + span.end.saturating_sub(origin.start).min(origin.span.len()),
                )
            } else {
                (origin.span.start, origin.span.end)
            };
            let mapped = Span::in_source(origin.span.source_id, start, end);
            if occurrence == Some(origin.occurrence)
                && out
                    .last()
                    .is_some_and(|previous| previous.end == mapped.start)
            {
                let previous = out.last_mut().expect("same occurrence has a range");
                previous.end = mapped.end;
            } else {
                out.push(mapped);
                occurrence = Some(origin.occurrence);
            }
        }
        if out.is_empty() {
            if let Some(origin) = generated_at {
                out.push(origin);
            } else if let Some(origin) = self.origins.last() {
                out.push(Span::in_source(
                    origin.span.source_id,
                    origin.span.end,
                    origin.span.end,
                ));
            } else {
                out.push(span);
            }
        }
        out
    }

    /// Generated leveloffset entries and indent padding participate in parsing,
    /// but have no physical text to highlight. Mixed tokens retain only their physical source fragments.
    pub fn token_spans(&self, span: Span) -> Vec<Span> {
        let first = self.origins.partition_point(|o| o.end <= span.start);
        if self.origins[first..]
            .iter()
            .take_while(|o| o.start < span.end)
            .all(|o| o.generated)
        {
            Vec::new()
        } else {
            self.original_spans(span)
        }
    }

    pub fn map_span(&self, span: Span, provenance: &mut Vec<Vec<Span>>) -> Span {
        let fragments = self.original_spans(span);
        let mut primary = fragments[0];
        if fragments.len() > 1 {
            let index = provenance
                .iter()
                .position(|p| *p == fragments)
                .unwrap_or_else(|| {
                    provenance.push(fragments);
                    provenance.len() - 1
                });
            primary.provenance = Some(index);
        }
        primary
    }

    fn error(context: &mut ParseContext, span: Span, message: String) {
        context.diagnostics.push(Diagnostic {
            omitted_span: None,
            report: Default::default(),
            span,
            severity: Severity::Error,
            message,
        });
    }

    /// An error whose conversion report spells Ruby's message verbatim.
    fn error_reported(context: &mut ParseContext, span: Span, message: String, ruby: String) {
        context.diagnostics.push(Diagnostic {
            omitted_span: None,
            report: crate::DiagnosticReport {
                message: Some(ruby),
                ..Default::default()
            },
            span,
            severity: Severity::Error,
            message,
        });
    }

    fn process(
        &mut self,
        off: usize,
        line: Arc<str>,
        context: &mut ParseContext,
    ) -> Option<(usize, Arc<str>)> {
        let active = self.stack.last().map_or(true, |c| c.active);
        let span = Span::new(off, off + line.len());
        let directive_line = line.ruby_rstrip();
        let (escaped, candidate) = directive_line
            .strip_prefix('\\')
            .map_or((false, directive_line), |s| (true, s));
        let Some((keyword, target, text)) = directive(candidate) else {
            if active {
                return Some((off, line));
            }
            self.inactive.push(span);
            return None;
        };
        if escaped {
            // The reference reader unescapes a recognized directive even while
            // skipping a branch. Preserve that observable reader behavior.
            return Some((off + 1, Arc::from(candidate)));
        }
        if keyword == "include" {
            if !active {
                self.inactive.push(span);
                return None;
            }
            // Secure mode turns the directive into a link, even without a
            // provider, and so does a URI target Ruby may not read (no I/O);
            // otherwise only a provider expands includes.
            if self.resources.is_none()
                && crate::context::SafeMode::of(&context.attributes)
                    < crate::context::SafeMode::Secure
                && !(self.unread_includes && crate::image::uri_prefix(target).is_some())
            {
                return Some((off, line));
            }
            self.includes.push(span);
            if self.expand_include(target, text, span, context) {
                return None;
            }
            return Some((off, line));
        }
        let target = target.to_lowercase();
        if keyword == "endif" {
            self.directives.push(span);
            if !text.is_empty() {
                Self::error_reported(
                    context,
                    span,
                    "malformed endif directive: text is not permitted".into(),
                    format!(
                        "malformed preprocessor directive - text not permitted: endif::{target}[{text}]"
                    ),
                );
            } else if let Some(open) = self.stack.last() {
                if target.is_empty() || target == open.target {
                    self.stack.pop();
                } else {
                    let expected = open.target.clone();
                    Self::error_reported(
                        context,
                        span,
                        format!("mismatched endif::{target}[]; expected endif::{expected}[]"),
                        format!(
                            "mismatched preprocessor directive: endif::{target}[], expected endif::{expected}[]"
                        ),
                    );
                }
            } else {
                context.diagnostics.push(Diagnostic {
                    omitted_span: None,
                    span,
                    severity: Severity::Error,
                    message: "unmatched endif directive".into(),
                    report: crate::DiagnosticReport {
                        message: Some(format!(
                            "unmatched preprocessor directive: {directive_line}"
                        )),
                        ..Default::default()
                    },
                });
            }
            return None;
        }
        if keyword == "ifeval" {
            self.directives.push(span);
            let include = if !active {
                // Skipped branches still balance every ifeval, even malformed ones.
                false
            } else if !target.is_empty() {
                Self::error_reported(
                    context,
                    span,
                    "malformed ifeval directive: target is not permitted".into(),
                    format!(
                        "malformed preprocessor directive - target not permitted: ifeval::{target}[{text}]"
                    ),
                );
                return None;
            } else if let Some(include) = {
                // Ruby's `resolve_expr_val`: `sub_attributes` with the `drop`
                // policy, executing set/counter expressions while reading.
                let mut diagnostics = Vec::new();
                let include = crate::conditional_expression::evaluate(text, &mut |value| {
                    context.preprocessor_subs(value, Some("drop"), span, &mut diagnostics)
                });
                Self::directive_diagnostics(context, diagnostics);
                include
            } {
                include
            } else {
                let problem = if text.is_empty() {
                    "missing"
                } else {
                    "invalid"
                };
                Self::error_reported(
                    context,
                    span,
                    format!("malformed ifeval directive: {problem} expression"),
                    format!(
                        "malformed preprocessor directive - {problem} expression: ifeval::[{text}]"
                    ),
                );
                return None;
            };
            self.stack.push(Conditional {
                target,
                active: include,
                span,
            });
            return None;
        }
        if target.is_empty() && active {
            self.directives.push(span);
            Self::error_reported(
                context,
                span,
                format!("malformed {keyword} directive: missing target"),
                format!("malformed preprocessor directive - missing target: {keyword}::[{text}]"),
            );
            return None;
        }
        let attributes = &context.attributes;
        // Ruby's regexp selects the first separator. Mixed operators do not
        // introduce precedence: the other separator remains part of a name.
        let defined = match target.chars().find(|c| matches!(c, ',' | '+')) {
            Some(',') => target.split(',').any(|name| attributes.get(name).is_some()),
            Some('+') => target.split('+').all(|name| attributes.get(name).is_some()),
            _ => attributes.get(&target).is_some(),
        };
        let include = active
            && if keyword == "ifndef" {
                !defined
            } else {
                defined
            };
        if text.is_empty() {
            self.directives.push(span);
            self.stack.push(Conditional {
                target,
                active: include,
                span,
            });
            None
        } else if include {
            let start = off + candidate.len() - text.len() - 1;
            let text = text.ruby_rstrip();
            self.directives.push(Span::new(off, start));
            self.directives
                .push(Span::new(start + text.len(), span.end));
            // Ruby reprocesses the line only when it looks like an include
            // directive (`@look_ahead -= 1 if text.start_with? 'include::'`).
            if text.starts_with("include::") {
                return self.process(start, Arc::from(text), context);
            }
            Some((start, Arc::from(text)))
        } else {
            self.directives.push(span);
            None
        }
    }

    /// Ruby's `replace_next_line %(link:#{target}[role=include])` (a target
    /// with spaces in `pass:c[…]`): a generated line with the directive as
    /// its origin, read like any other line.
    fn include_link(&mut self, target: &str, span: Span) -> bool {
        let target = if target.contains(' ') {
            format!("pass:c[{target}]")
        } else {
            target.to_owned()
        };
        let source_id = self.inputs.last().expect("current input").source_id;
        let mut input = Input::new("", source_id, true, self.next_occurrence);
        input.lines = vec![Line::generated(format!("link:{target}[role=include]"))];
        input.generated_at = Some(self.original_spans(span)[0]);
        self.inputs.push(input);
        self.next_occurrence += 1;
        true
    }

    /// Ruby's `replace_next_line %(Unresolved directive in #{@path} -
    /// include::#{target}[#{attrlist}])` for an include that cannot be read:
    /// a generated line with the directive as its origin. `@path` is the
    /// primary document's file name (`<stdin>` without one) or an include's
    /// path relative to the document directory.
    fn unresolved_include(&mut self, target: &str, attrlist: &str, span: Span) -> bool {
        let current = self.inputs.last().expect("current input").source_id;
        let primary = self.inputs[0].source_id;
        let path = self.resources.as_ref().and_then(|resources| {
            let document_path = |id| {
                resources
                    .sources
                    .get(id)
                    .and_then(|source| resources.provider.document_path(source.name()))
            };
            let document = document_path(primary);
            if current == primary {
                document.map(|path| path.rsplit('/').next().unwrap_or_default().to_owned())
            } else {
                match (document, document_path(current)) {
                    (Some(document), Some(include)) => {
                        Some(crate::xref_target::relative_path(&document, &include))
                    }
                    _ => resources
                        .sources
                        .get(current)
                        .map(|source| source.name().to_owned()),
                }
            }
        });
        let path = path.unwrap_or_else(|| "<stdin>".to_owned());
        let mut input = Input::new("", current, true, self.next_occurrence);
        input.lines = vec![Line::generated(format!(
            "Unresolved directive in {path} - include::{target}[{attrlist}]"
        ))];
        input.generated_at = Some(self.original_spans(span)[0]);
        self.inputs.push(input);
        self.next_occurrence += 1;
        true
    }

    /// Ruby's `parse_attributes attrlist, [], sub_input: true`: the list's
    /// a-stage executes set/counter expressions with the document's
    /// attribute-missing policy before the AttributeList is parsed.
    fn include_attributes(
        &mut self,
        attrlist: &str,
        span: Span,
        context: &mut ParseContext,
    ) -> std::collections::BTreeMap<String, String> {
        if !attrlist.contains('{') {
            return crate::include_attributes::parse(attrlist);
        }
        let mut diagnostics = Vec::new();
        let input = context.preprocessor_subs(attrlist, None, span, &mut diagnostics);
        Self::directive_diagnostics(context, diagnostics);
        crate::include_attributes::parse(&input)
    }

    /// Diagnostics of a directive's substitutions. The reader reports what
    /// happens to the directive itself, so the informational drop-line
    /// messages (hidden by Ruby's default log level) are not repeated.
    fn directive_diagnostics(context: &mut ParseContext, diagnostics: Vec<Diagnostic>) {
        context.diagnostics.extend(
            diagnostics
                .into_iter()
                .filter(|diagnostic| diagnostic.severity != Severity::Info),
        );
    }

    fn expand_include(
        &mut self,
        target: &str,
        attrlist: &str,
        span: Span,
        context: &mut ParseContext,
    ) -> bool {
        let policy = context
            .attributes
            .get("attribute-missing")
            .unwrap_or("skip")
            .to_owned();
        let target_policy = if policy == "warn" {
            "drop-line"
        } else {
            &policy
        };
        // Every `sub_attributes` call of Ruby's branches below can execute
        // set/counter expressions, so substitute exactly where it does.
        let substituted = target.contains('{');
        let expanded_target = if substituted {
            let mut diagnostics = Vec::new();
            let expanded =
                context.preprocessor_subs(target, Some(target_policy), span, &mut diagnostics);
            Self::directive_diagnostics(context, diagnostics);
            expanded
        } else {
            target.to_owned()
        };
        if substituted && expanded_target.is_empty() {
            // The extra space distinguishes a missing reference that drops its
            // line from a defined attribute whose value is empty (also for
            // multiline values). Ruby ignores its drop-line messages.
            let missing = |context: &mut ParseContext| {
                context
                    .preprocessor_subs(
                        &format!("{target} "),
                        Some("drop-line"),
                        span,
                        &mut Vec::new(),
                    )
                    .is_empty()
            };
            if policy == "drop-line" && missing(context) {
                return true;
            }
            if self
                .include_attributes(attrlist, span, context)
                .contains_key("optional-option")
            {
                return true;
            }
            let missing_target = policy == "warn" && missing(context);
            let reason = if missing_target {
                "due to missing attribute"
            } else {
                "because resolved target is blank"
            };
            context.diagnostics.push(Diagnostic {
                omitted_span: None,
                // Ruby's log names the whole directive.
                report: crate::DiagnosticReport {
                    message: Some(format!(
                        "include dropped {reason}: include::{target}[{attrlist}]"
                    )),
                    ..Default::default()
                },
                span,
                severity: Severity::Warning,
                message: format!("include dropped {reason}: {target}"),
            });
            // Ruby names the unexpanded target here.
            return self.unresolved_include(target, attrlist, span);
        }
        // Ruby's secure mode does not process the directive; it leaves a
        // link to the target instead.
        if crate::context::SafeMode::of(&context.attributes) >= crate::context::SafeMode::Secure {
            return self.include_link(&expanded_target, span);
        }
        // Without a positive `max-include-depth` Ruby leaves the directive.
        let Some(abs_depth) = self.resources.as_ref().and_then(|r| r.max_depth) else {
            // Without a provider nothing is read. Ruby links a URI target it
            // may not read without any I/O (after parsing the parameters).
            if self.unread_includes
                && crate::image::uri_prefix(&expanded_target).is_some()
                && context.attributes.get("allow-uri-read").is_none()
            {
                self.include_attributes(attrlist, span, context);
                return self.include_link(&expanded_target, span);
            }
            return false;
        };
        // Ruby checks the depth before it parses the parameter list.
        let current = self.inputs.last().expect("current input");
        if self.inputs.len() > current.max_depth {
            let message = format!("maximum include depth of {} exceeded", current.depth_label);
            Self::error(context, span, message);
            return false;
        }
        let attrs = self.include_attributes(attrlist, span, context);
        let attributes = Arc::clone(&context.attributes);
        let optional = attrs.contains_key("optional-option");
        let selection = crate::include_selection::Selection::parse(&attrs);
        let target = expanded_target;
        if optional && target.is_empty() {
            return true;
        }
        let parent = self.inputs.last().expect("current input");
        let current = parent.source_id;
        let inherited_depth = (parent.max_depth, parent.depth_label);
        let resources = self.resources.as_mut().expect("explicit resource provider");
        // A URI (or a target inside a URI include) is read only with
        // `allow-uri-read`; otherwise Ruby links to it.
        let parent_name = resources
            .sources
            .get(current)
            .map(|source| source.name().to_owned());
        let uri_target = if crate::image::uri_prefix(&target).is_some() {
            Some(target.clone())
        } else {
            parent_name
                .filter(|name| crate::image::uri_prefix(name).is_some())
                .map(|name| match name.rfind('/') {
                    Some(slash) => format!("{}/{target}", &name[..slash]),
                    None => target.clone(),
                })
        };
        if let (Some(uri), None) = (&uri_target, attributes.get("allow-uri-read")) {
            let uri = uri.clone();
            return self.include_link(&uri, span);
        }
        let resources = self.resources.as_mut().expect("explicit resource provider");
        if uri_target.is_some() && attributes.get("cache-uri").is_some() {
            resources.provider.enable_uri_cache();
        }
        // Ruby's resolve_include_path: relative to the current include's
        // directory (the base directory within the primary document),
        // confined to the base directory from safe mode up.
        // The current resource's own directory: the provider resolves a
        // request relative to it, whatever directory Ruby starts from.
        let referrer_dir = resources
            .sources
            .get(current)
            .and_then(|source| resources.provider.document_path(source.name()))
            .and_then(|path| path.rsplit_once('/').map(|(dir, _)| dir.to_owned()))
            .map(|dir| if dir.is_empty() { "/".to_owned() } else { dir });
        let system_path = match (&resources.base_dir, &uri_target) {
            (Some(base_dir), None) => {
                let dir = if current == self.inputs[0].source_id {
                    Some(base_dir.clone())
                } else {
                    referrer_dir.clone()
                };
                dir.map(|dir| {
                    let dir = if dir.is_empty() { "/".to_owned() } else { dir };
                    let resolved = crate::system_path::normalize_system_path(
                        &target,
                        Some(&dir),
                        base_dir,
                        resources.safe,
                        "include file",
                    );
                    let from = referrer_dir.clone().unwrap_or(dir);
                    (resolved, from)
                })
            }
            _ => None,
        };
        // Ruby reads (and reports) the resolved path; the provider takes it
        // relative to the current resource, its referrer.
        let path = system_path
            .as_ref()
            .map(|(resolved, _)| resolved.path.clone());
        let request = system_path.as_ref().map(|(resolved, from)| {
            crate::xref_target::relative_path(&format!("{from}/_"), &resolved.path)
        });
        for warning in system_path
            .map(|(resolved, _)| resolved.warnings)
            .unwrap_or_default()
        {
            context.diagnostics.push(Diagnostic {
                omitted_span: None,
                span,
                severity: Severity::Warning,
                message: warning,
                report: crate::DiagnosticReport {
                    location: crate::DiagnosticLocation::Unlocated,
                    ..Default::default()
                },
            });
        }
        let resources = self.resources.as_mut().expect("explicit resource provider");
        let (id, prepared_selection) = match resources.sources.load_selected_include(
            request.as_deref().unwrap_or(&target),
            Some(current),
            resources.provider,
            attrs.get("encoding").map(String::as_str),
            &selection,
        ) {
            Ok(id) => id,
            Err(error)
                if optional
                    && uri_target.is_none()
                    && (error.kind == crate::ResourceErrorKind::NotFound
                        || (path.is_some()
                            && error.kind == crate::ResourceErrorKind::InvalidTarget)) =>
            {
                return true;
            }
            Err(error) => {
                if uri_target.is_some() || error.kind == crate::ResourceErrorKind::InvalidEncoding {
                    let kind = if uri_target.is_some() { "uri" } else { "file" };
                    context.diagnostics.push(Diagnostic {
                        omitted_span: None,
                        span,
                        severity: Severity::Error,
                        message: format!("include failed ({:?}): {error}", error.kind),
                        report: crate::DiagnosticReport {
                            message: Some(format!(
                                "include {kind} not readable: {}",
                                uri_target.as_deref().unwrap_or(&error.target)
                            )),
                            ..Default::default()
                        },
                    });
                    return self.unresolved_include(&target, attrlist, span);
                }
                let message = format!("include failed ({:?}): {error}", error.kind);
                match (&path, error.kind) {
                    // Ruby's `File.file?` check: a missing path or a
                    // directory is not found.
                    (
                        Some(path),
                        crate::ResourceErrorKind::NotFound
                        | crate::ResourceErrorKind::InvalidTarget,
                    ) => Self::error_reported(
                        context,
                        span,
                        message,
                        format!("include file not found: {path}"),
                    ),
                    _ => Self::error(context, span, message),
                }
                // Ruby cannot tell a denied target (its jail resolves it to
                // another path) from a missing or unreadable one.
                return self.unresolved_include(&target, attrlist, span);
            }
        };
        let file = resources.sources.get(id).expect("loaded source");
        if self.inputs.iter().any(|input| {
            resources
                .sources
                .get(input.source_id)
                .is_some_and(|s| s.name() == file.name())
        }) {
            Self::error(context, span, format!("cyclic include: {target}"));
            return false;
        }
        if file.stored_bytes() > resources.limits.max_bytes.saturating_sub(resources.bytes) {
            Self::error(context, span, "maximum included byte count exceeded".into());
            return false;
        }
        resources.bytes += file.stored_bytes();
        // The reference preprocesses directives only in AsciiDoc resources.
        let preprocess = [".adoc", ".asciidoc", ".asc", ".ad", ".txt"]
            .iter()
            .any(|ext| file.name().ends_with(ext));
        let mut input = Input::new(file.text(), id, preprocess, self.next_occurrence);
        if !file.omitted_lines().is_empty() || file.has_line_frames() {
            input.lines = file.include_lines();
        }
        // Like Ruby's push_include: a positive relative depth beyond the
        // absolute maximum is replaced by it, otherwise nothing nests.
        (input.max_depth, input.depth_label) =
            attrs.get("depth").map_or(inherited_depth, |value| {
                let stack = self.inputs.len();
                let relative = crate::include_selection::integer_prefix(value);
                if relative <= 0 {
                    return (stack, 0);
                }
                match usize::try_from(relative)
                    .ok()
                    .and_then(|r| Some((stack.checked_add(r)?, r)))
                {
                    Some((depth, relative)) if depth <= abs_depth => (depth, relative),
                    _ => (abs_depth, abs_depth),
                }
            });
        let full = selection.full();
        let whole = matches!(selection, crate::include_selection::Selection::All);
        let selected = prepared_selection
            .unwrap_or_else(|| selection.select(&input.lines, file.omitted_lines(), id));
        let mut selection_diagnostics = selected.diagnostics;
        let missing_report = format!(
            "tag{} '{}' not found in include {}: {}",
            if selected.missing.len() > 1 { "s" } else { "" },
            selected.missing.join(", "),
            if file.name().contains("://") {
                "uri"
            } else {
                "file"
            },
            file.name()
        );
        for diagnostic in &mut selection_diagnostics {
            if let Some(message) = &mut diagnostic.report.message {
                let kind = if file.name().contains("://") {
                    "uri"
                } else {
                    "file"
                };
                message.push_str(&format!(" of include {kind}: {}", file.name()));
            }
        }
        let include_cursor = self.report_cursor(span.start);
        for diagnostic in &mut selection_diagnostics {
            if let Some(cursor) = include_cursor {
                diagnostic.report.location = crate::DiagnosticLocation::Cursor(cursor);
            }
        }
        // Selection warnings already have physical ranges and cursors.
        let stamp = context.stamp();
        for diagnostic in selection_diagnostics {
            context.place(stamp, diagnostic, true);
        }
        for (index, tag) in selected.missing.into_iter().enumerate() {
            context.diagnostics.push(Diagnostic {
                omitted_span: None,
                report: crate::DiagnosticReport {
                    message: Some(missing_report.clone()),
                    suppressed: index != 0,
                    ..Default::default()
                },
                span,
                severity: Severity::Warning,
                message: format!("tag '{tag}' not found in include: {target}"),
            });
        }
        // Ruby registers an AsciiDoc include once it pushes any lines.
        if preprocess && (whole || selected.keep.contains(&true)) {
            let resources = self.resources.as_ref().expect("explicit resource provider");
            let primary = self.inputs[0].source_id;
            let path = |id| {
                resources
                    .sources
                    .get(id)
                    .and_then(|source| resources.provider.document_path(source.name()))
            };
            // Ruby's relpath is relative to the base directory, also for
            // standard input (a document path inside it stands in).
            let document = path(primary).or_else(|| {
                resources
                    .base_dir
                    .as_ref()
                    .map(|dir| format!("{}/<stdin>", dir.trim_end_matches('/')))
            });
            if let (Some(document), Some(include)) = (document, path(id)) {
                crate::xref_target::register_include(
                    &mut self.include_catalog,
                    crate::xref_target::include_key(&document, &include),
                    full,
                );
            }
        }
        input.start_line = selected.keep.iter().position(|keep| *keep).unwrap_or(0) as isize + 1;
        let mut keep = selected.keep.into_iter();
        input.lines.retain(|_| keep.next().unwrap_or(false));
        if let Some(first) = input.lines.first_mut() {
            first.strip_bom();
        }
        if let Some(indent) = attrs.get("indent") {
            let indent = crate::include_selection::integer_prefix(indent);
            let tabsize = attributes
                .get("tabsize")
                .map_or(0, crate::include_selection::integer_prefix);
            let resources = self.resources.as_mut().expect("explicit resource provider");
            match crate::include_indent::adjust(
                &mut input.lines,
                indent,
                tabsize,
                resources.limits.max_bytes.saturating_sub(resources.bytes),
                preprocess,
            ) {
                Ok(growth) => resources.bytes += growth,
                Err(()) => {
                    Self::error(
                        context,
                        span,
                        "maximum included byte count exceeded by indentation".into(),
                    );
                    return false;
                }
            }
        } else if preprocess {
            for line in &mut input.lines {
                line.normalize();
            }
        }
        // Ruby's `prepare_lines` skips front matter in included files too,
        // with the attributes current at the directive (a reading effect).
        if attributes.get("skip-front-matter").is_some() {
            if let Some(front_matter) = input.skip_front_matter() {
                Arc::make_mut(&mut context.attributes).set("front-matter", front_matter);
            }
        }
        let attributes = &context.attributes;
        if let Some(offset) = attrs.get("leveloffset").filter(|_| !input.lines.is_empty()) {
            input.start_line -= 2;
            // Ruby injects these entries even in raw/verbatim input. They must
            // be parsed in context (or kept as literal text), not applied here:
            // containers may be captured before their attributes are processed.
            input.generated_at = Some(self.original_spans(span)[0]);
            let restore = attributes.get("leveloffset").map_or_else(
                || ":leveloffset!:".to_owned(),
                |value| format!(":leveloffset: {value}"),
            );
            input.lines.insert(0, Line::generated(String::new()));
            input
                .lines
                .insert(0, Line::generated(format!(":leveloffset: {offset}")));
            input.lines.push(Line::generated(String::new()));
            input.lines.push(Line::generated(restore));
        }
        self.inputs.push(input);
        self.next_occurrence += 1;
        true
    }

    /// Ruby's front matter (`skip-front-matter`) of the primary document,
    /// before its header is read: the `front-matter` attribute.
    pub fn skip_primary_front_matter(&mut self, context: &mut ParseContext) {
        if context.attributes.get("skip-front-matter").is_none() {
            return;
        }
        if let Some(front_matter) = self.inputs[0].skip_front_matter() {
            Arc::make_mut(&mut context.attributes).set("front-matter", front_matter);
        }
    }

    /// For a reader without a provider (string APIs): whether Ruby's
    /// built-in include handling is enabled by the host attributes.
    pub fn enable_unread_includes(&mut self, options: &crate::ParseOptions) {
        self.unread_includes =
            self.resources.is_none() && host_max_include_depth(options).is_some();
    }

    pub fn finish(&mut self, context: &mut ParseContext) {
        for open in std::mem::take(&mut self.stack) {
            Self::error(
                context,
                open.span,
                format!(
                    "unterminated conditional directive: missing endif::{}[]",
                    open.target
                ),
            );
            context.diagnostics.last_mut().unwrap().report.suppressed = true;
        }
    }

    /// Capture a container before parsing its children, just as the reference
    /// reader does. Attribute entries in that body cannot affect this read-ahead.
    pub fn bound(
        &mut self,
        start: usize,
        context: &mut ParseContext,
        stop: impl Fn(&str) -> bool,
    ) -> usize {
        self.bound_until(start, context, stop).0
    }

    /// [`Self::bound`], also telling whether a `stop` line ends the body
    /// (false when reading reached the end of the input or outer bound).
    pub fn bound_until(
        &mut self,
        start: usize,
        context: &mut ParseContext,
        stop: impl Fn(&str) -> bool,
    ) -> (usize, bool) {
        let old = self.limit;
        let mut end = start;
        let mut found = false;
        while self.has(end, context) {
            if stop(&self[end].1) {
                found = true;
                break;
            }
            end += 1;
        }
        self.limit = end;
        (old, found)
    }

    pub fn content(&self, start: usize, end: usize) -> FilteredSource {
        let mut fragments = Vec::new();
        self.content_fragments(start, end, &mut fragments);
        FilteredSource::from_fragments(fragments)
    }

    /// Ruby's `fold_first`: a list item's text and its adjacent paragraph
    /// joined by the text line's ending; block metadata lines between them
    /// are not text.
    pub fn folded_content(&self, text: Span, folded: Span) -> FilteredSource {
        let next = self.loaded.partition_point(|(off, _)| *off <= text.end);
        let gap_start = self.loaded.get(next).map_or(folded.start, |(off, _)| *off);
        let gap_end = self.line_start(folded.start);
        let mut fragments = Vec::new();
        if gap_start < gap_end {
            self.content_fragments(text.start, gap_start, &mut fragments);
            self.content_fragments(gap_end, folded.end, &mut fragments);
        } else {
            self.content_fragments(text.start, folded.end, &mut fragments);
        }
        FilteredSource::from_fragments(fragments)
    }

    fn content_fragments<'s>(
        &'s self,
        start: usize,
        end: usize,
        fragments: &mut Vec<(usize, &'s str)>,
    ) {
        let first = self
            .loaded
            .partition_point(|(off, line)| off + line.len() < start);
        for (index, (off, line)) in self.loaded.iter().enumerate().skip(first) {
            let off = *off;
            if off >= end {
                break;
            }
            let left = start.saturating_sub(off).min(line.len());
            let right = end.saturating_sub(off).min(line.len()).max(left);
            if right > left {
                fragments.push((off + left, &line[left..right]));
            }
            if off + line.len() < end {
                let (eol_start, eol) = &self.endings[index];
                if *eol_start < end {
                    // Ruby's reader normalizes every line ending to LF; the
                    // LF maps to where the source ending (CRLF too) starts.
                    fragments.push((*eol_start, if eol.is_empty() { "" } else { "\n" }));
                }
            }
        }
    }

    /// Defer a verse body: verbatim lines (adjusted like Ruby's `build_block`)
    /// that keep their line breaks.
    pub fn scan_verse(
        &self,
        start: usize,
        end: usize,
        attrs: &Attributes,
        indentation: crate::indentation::Indentation,
        context: &mut ParseContext,
    ) -> Vec<Inline> {
        let content = indentation.apply(&self.content(start, end));
        context.body_subs.defer(content, None, attrs, false)
    }

    /// Catalog inline anchors from the same comment-filtered text that the
    /// paragraph body converts later. `leading` restricts it to an anchor that
    /// starts a list item.
    pub fn catalog_anchors(
        &self,
        start: usize,
        end: usize,
        leading: bool,
        context: &mut ParseContext,
    ) {
        let content = self.content(start, end);
        let filtered = crate::comments::filtered_text(&content.text, 0);
        let map = |range: std::ops::Range<usize>| {
            content.original_span(filtered.original_span(Span::new(range.start, range.end)))
        };
        if leading {
            context.catalog_leading_anchor(&filtered.text, map);
        } else {
            context.catalog_inline_anchors(&filtered.text, map);
        }
    }

    /// A description list term's leading anchor (Ruby's `catalog_inline_anchor`
    /// with the rest of the term as the default reftext).
    pub fn catalog_term_anchor(&self, start: usize, end: usize, context: &mut ParseContext) {
        let content = self.content(start, end);
        let map = |range: std::ops::Range<usize>| {
            content.original_span(Span::new(range.start, range.end))
        };
        context.catalog_term_anchor(&content.text, map);
    }

    pub fn scan_content(
        &self,
        start: usize,
        end: usize,
        attrs: &Attributes,
        context: &mut ParseContext,
    ) -> Vec<Inline> {
        self.scan_filtered(self.content(start, end), attrs, context)
    }

    /// The processed offset where the loaded line containing `pos` starts.
    pub fn line_start(&self, pos: usize) -> usize {
        let index = self.loaded.partition_point(|(off, _)| *off <= pos);
        index
            .checked_sub(1)
            .map_or(pos, |index| self.loaded[index].0)
    }

    /// [`Reader::scan_content`] over an already prepared content.
    pub fn scan_filtered(
        &self,
        content: crate::scanner::FilteredSource,
        attrs: &Attributes,
        context: &mut ParseContext,
    ) -> Vec<Inline> {
        let filtered = crate::comments::filtered_text(&content.text, 0);
        context
            .comments
            .extend(filtered.removed.iter().map(|s| content.original_span(*s)));
        context
            .body_subs
            .defer(content, Some(filtered), attrs, true)
    }
}

impl<'a> Index<usize> for Reader<'a> {
    type Output = (usize, Arc<str>);
    fn index(&self, index: usize) -> &Self::Output {
        &self.loaded[index]
    }
}

fn directive(line: &str) -> Option<(&str, &str, &str)> {
    let (keyword, rest) = line.split_once("::")?;
    if !matches!(keyword, "ifdef" | "ifndef" | "ifeval" | "endif" | "include") {
        return None;
    }
    let rest = rest.strip_suffix(']')?;
    let (target, text) = rest.split_once('[')?;
    // Ruby's ConditionalDirectiveRx (`\S*?` targets) and IncludeDirectiveRx
    // (a target neither empty nor starting or ending with whitespace).
    let valid = match keyword {
        "include" => {
            !target.is_empty()
                && !target.starts_with(char::is_whitespace)
                && !target.ends_with(char::is_whitespace)
        }
        _ => !target.chars().any(char::is_whitespace),
    };
    valid.then_some((keyword, target, text))
}

/// Ruby's `@maxdepth[:abs]`, read once when the reader is created, so only
/// the API attributes count: `max-include-depth` (Ruby `to_i`, default 64)
/// bounded by the host limit, or None when it disables includes.
fn host_max_include_depth(options: &crate::ParseOptions) -> Option<usize> {
    let host_depth = options
        .attributes
        .iter()
        .map(|(key, value)| crate::context::host_attribute(key, value))
        .filter(|(key, _, _)| *key == "max-include-depth")
        .last()
        .and_then(|(_, value, _)| value);
    match host_depth {
        Some(value) => usize::try_from(crate::include_selection::integer_prefix(value))
            .ok()
            .filter(|depth| *depth > 0)
            .map(|depth| depth.min(options.includes.max_depth)),
        None => Some(options.includes.max_depth),
    }
}
