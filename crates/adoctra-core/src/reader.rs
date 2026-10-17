//! Lazy, source-preserving conditional and include preprocessing. Attribute mutations belong
//! to the parser; each line is evaluated once, when that parser requests it.
use crate::context::ParseContext;
use crate::include_indent::Line;
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
    max_depth: usize,
    next: usize,
    preprocess: bool,
    occurrence: usize,
    generated_at: Option<Span>,
}

impl Input {
    fn new(source: &str, source_id: crate::SourceId, preprocess: bool, occurrence: usize) -> Self {
        let lines = Line::from_source(source);
        Self {
            source_id,
            lines,
            max_depth: usize::MAX,
            next: 0,
            preprocess,
            occurrence,
            generated_at: None,
        }
    }
}

struct Resources<'a> {
    sources: &'a mut crate::SourceMap,
    provider: &'a dyn crate::ResourceProvider,
    limits: crate::IncludeLimits,
    bytes: usize,
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
    inputs: Vec<Input>,
    resources: Option<Resources<'a>>,
    origins: Vec<Origin>,
    next_offset: usize,
    next_occurrence: usize,
    pub loaded: Vec<(usize, Arc<str>)>,
    pub attribute_entries: Vec<(Span, Option<crate::context::AttributeEntry>)>,
    /// Early metadata substitutions, including values later overwritten or orphaned.
    pub metadata_references: Vec<Span>,
    /// Lines of body section titles (ATX, setext title and underline), for highlighting.
    pub heading_lines: Vec<Span>,
    /// Table blocks (fence to fence) and their cell separators with specs,
    /// for highlighting.
    pub tables: Vec<Span>,
    pub table_separators: Vec<Span>,
    /// List markers and description list delimiters, for highlighting.
    pub list_markers: Vec<Span>,
    endings: Vec<(usize, String)>,
    pub limit: usize,
    stack: Vec<Conditional>,
    pub comments: Vec<Span>,
    pub directives: Vec<Span>,
    pub includes: Vec<Span>,
    /// Ruby's `catalog[:includes]`, for interdocument references.
    pub include_catalog: crate::xref_target::Includes,
    pub inactive: Vec<Span>,
    pub diagnostics: Vec<Diagnostic>,
    pub resource_diagnostics: Vec<Diagnostic>,
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
            inputs: vec![Input::new(source, crate::SourceId::default(), false, 0)],
            resources: None,
            origins: Vec::new(),
            next_offset: 0,
            next_occurrence: 1,
            loaded: Vec::new(),
            attribute_entries: Vec::new(),
            metadata_references: Vec::new(),
            heading_lines: Vec::new(),
            tables: Vec::new(),
            table_separators: Vec::new(),
            list_markers: Vec::new(),
            endings: Vec::new(),
            limit: usize::MAX,
            stack: Vec::new(),
            comments: Vec::new(),
            directives: Vec::new(),
            includes: Vec::new(),
            include_catalog: Default::default(),
            inactive: Vec::new(),
            diagnostics: Vec::new(),
            resource_diagnostics: Vec::new(),
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
        self.tables.extend(nested.tables);
        self.table_separators.extend(nested.table_separators);
        self.table_separators.sort_by_key(|span| span.start);
        self.list_markers.extend(nested.list_markers);
        self.list_markers.sort_by_key(|span| span.start);
        self.diagnostics.extend(nested.diagnostics);
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
        let mut reader = Self::new(source.text());
        reader.inputs[0].source_id = options.source_id;
        reader.inputs[0].max_depth = options.includes.max_depth;
        reader.resources = Some(Resources {
            sources,
            provider,
            limits: options.includes,
            bytes: 0,
        });
        Ok(reader)
    }

    pub fn set_primary_id(&mut self, id: crate::SourceId) {
        self.inputs[0].source_id = id;
    }

    /// Clone the shared line so callers may advance the reader while using it.
    /// The line ending of a loaded line.
    pub fn eol(&self, index: usize) -> String {
        self.endings[index].1.clone()
    }

    pub fn line(&self, index: usize) -> (usize, Arc<str>) {
        self.loaded[index].clone()
    }

    pub fn has(&mut self, index: usize, context: &ParseContext) -> bool {
        self.ensure(index, Some(&context.attributes))
    }

    pub fn raw_has(&mut self, index: usize) -> bool {
        self.ensure(index, None)
    }

    fn ensure(&mut self, index: usize, attributes: Option<&Attributes>) -> bool {
        if index >= self.limit {
            return false;
        }
        while self.loaded.len() <= index {
            if !self.step(attributes) {
                return false;
            }
        }
        true
    }

    /// Read the next line of the innermost input (dropping exhausted inputs);
    /// false at the end of every input.
    fn step(&mut self, attributes: Option<&Attributes>) -> bool {
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
            if let Some(attributes) = attributes {
                let eol_start = off + line.len();
                if let Some(emitted) = self.process(off, line, attributes) {
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
        attributes: &Attributes,
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
        let max_depth = self
            .inputs
            .first()
            .map_or(usize::MAX, |input| input.max_depth);
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
                next: 0,
                preprocess: index == 0,
                occurrence: self.next_occurrence,
                generated_at: None,
            });
            self.next_occurrence += 1;
        }
        let loaded = self.loaded.len();
        let stack = std::mem::take(&mut self.stack);
        while self.inputs.len() > depth {
            self.step(Some(attributes));
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

    fn error(&mut self, span: Span, message: String) {
        self.diagnostics.push(Diagnostic {
            span,
            severity: Severity::Error,
            message,
        });
    }

    fn process(
        &mut self,
        off: usize,
        line: Arc<str>,
        attributes: &Attributes,
    ) -> Option<(usize, Arc<str>)> {
        let active = self.stack.last().map_or(true, |c| c.active);
        let span = Span::new(off, off + line.len());
        let directive_line = line.trim_end();
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
            if self.resources.is_none() {
                return Some((off, line));
            }
            self.includes.push(span);
            if self.expand_include(target, text, span, attributes) {
                return None;
            }
            return Some((off, line));
        }
        let target = target.to_lowercase();
        if keyword == "endif" {
            self.directives.push(span);
            if !text.is_empty() {
                self.error(
                    span,
                    "malformed endif directive: text is not permitted".into(),
                );
            } else if let Some(open) = self.stack.last() {
                if target.is_empty() || target == open.target {
                    self.stack.pop();
                } else {
                    self.error(
                        span,
                        format!(
                            "mismatched endif::{target}[]; expected endif::{}[]",
                            open.target
                        ),
                    );
                }
            } else {
                self.error(span, "unmatched endif directive".into());
            }
            return None;
        }
        if keyword == "ifeval" {
            self.directives.push(span);
            let include = if !active {
                // Skipped branches still balance every ifeval, even malformed ones.
                false
            } else if !target.is_empty() {
                self.error(
                    span,
                    "malformed ifeval directive: target is not permitted".into(),
                );
                return None;
            } else if let Some(include) = crate::conditional_expression::evaluate(text, attributes)
            {
                include
            } else {
                self.error(
                    span,
                    format!(
                        "malformed ifeval directive: {} expression",
                        if text.is_empty() {
                            "missing"
                        } else {
                            "invalid"
                        }
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
            self.error(
                span,
                format!("malformed {keyword} directive: missing target"),
            );
            return None;
        }
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
            let text = text.trim_end();
            self.directives.push(Span::new(off, start));
            self.directives
                .push(Span::new(start + text.len(), span.end));
            Some((start, Arc::from(text)))
        } else {
            self.directives.push(span);
            None
        }
    }

    fn expand_include(
        &mut self,
        target: &str,
        attrs: &str,
        span: Span,
        attributes: &Attributes,
    ) -> bool {
        let policy = attributes.get("attribute-missing").unwrap_or("skip");
        let target_policy = if policy == "warn" {
            "drop-line"
        } else {
            policy
        };
        let (expanded_target, _) =
            crate::include_attributes::substitute(target, attributes, target_policy);
        let blank_target = target.contains('{') && expanded_target.is_empty();
        // The extra space distinguishes a missing reference that drops its line
        // from a defined attribute whose value is empty (also for multiline values).
        let missing_target = blank_target
            && matches!(policy, "drop-line" | "warn")
            && crate::include_attributes::substitute(
                &format!("{target} "),
                attributes,
                "drop-line",
            )
            .0
            .is_empty();
        if missing_target && policy == "drop-line" {
            return true;
        }
        let (attrs, warnings) = crate::include_attributes::parse(attrs, attributes);
        self.diagnostics
            .extend(warnings.into_iter().map(|message| Diagnostic {
                span,
                severity: Severity::Warning,
                message,
            }));
        let optional = attrs.contains_key("optional-option");
        if blank_target {
            if optional {
                return true;
            }
            self.diagnostics.push(Diagnostic {
                span,
                severity: Severity::Warning,
                message: format!(
                    "include dropped {}: {target}",
                    if missing_target {
                        "due to missing attribute"
                    } else {
                        "because resolved target is blank"
                    }
                ),
            });
            // Keep the physical directive for partial output, as for provider
            // failures. Ruby's path-bearing fallback is outside this profile.
            return false;
        }
        // Resources are immutable UTF-8 snapshots supplied by the host. Keep
        // rejecting transcoding requests until the provider contract defines it.
        if attrs.contains_key("encoding") {
            self.error(span, "unsupported include options: encoding".into());
            return false;
        }
        let selection = crate::include_selection::Selection::parse(&attrs);
        let target = expanded_target;
        if optional && target.is_empty() {
            return true;
        }
        let parent = self.inputs.last().expect("current input");
        let current = parent.source_id;
        let inherited_depth = parent.max_depth;
        let resources = self.resources.as_mut().expect("explicit resource provider");
        if self.inputs.len() > inherited_depth {
            self.error(span, "maximum include depth exceeded".into());
            return false;
        }
        let id = match resources
            .sources
            .load(&target, Some(current), resources.provider)
        {
            Ok(id) => id,
            Err(error) if optional && error.kind == crate::ResourceErrorKind::NotFound => {
                return true;
            }
            Err(error) => {
                self.error(span, format!("include failed ({:?}): {error}", error.kind));
                return false;
            }
        };
        let file = resources.sources.get(id).expect("loaded source");
        if self.inputs.iter().any(|input| {
            resources
                .sources
                .get(input.source_id)
                .is_some_and(|s| s.name() == file.name())
        }) {
            self.error(span, format!("cyclic include: {target}"));
            return false;
        }
        if file.text().len() > resources.limits.max_bytes.saturating_sub(resources.bytes) {
            self.error(span, "maximum included byte count exceeded".into());
            return false;
        }
        resources.bytes += file.text().len();
        // The reference preprocesses directives only in AsciiDoc resources.
        let preprocess = [".adoc", ".asciidoc", ".asc", ".ad", ".txt"]
            .iter()
            .any(|ext| file.name().ends_with(ext));
        let mut input = Input::new(file.text(), id, preprocess, self.next_occurrence);
        input.max_depth = attrs.get("depth").map_or(inherited_depth, |value| {
            let relative = crate::include_selection::integer_prefix(value).max(0);
            let relative = usize::try_from(relative).unwrap_or(usize::MAX);
            self.inputs
                .len()
                .saturating_add(relative)
                .min(resources.limits.max_depth)
        });
        let full = selection.full();
        let whole = matches!(selection, crate::include_selection::Selection::All);
        let selected = selection.select(
            input.lines.iter().map(|line| {
                (
                    line.original.expect("physical selection line"),
                    line.text.as_ref(),
                )
            }),
            id,
        );
        self.resource_diagnostics.extend(selected.diagnostics);
        for tag in selected.missing {
            self.diagnostics.push(Diagnostic {
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
            if let (Some(document), Some(include)) = (path(primary), path(id)) {
                crate::xref_target::register_include(
                    &mut self.include_catalog,
                    crate::xref_target::include_key(&document, &include),
                    full,
                );
            }
        }
        let mut keep = selected.keep.into_iter();
        input.lines.retain(|_| keep.next().unwrap_or(false));
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
                    self.error(
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
        if let Some(offset) = attrs.get("leveloffset").filter(|_| !input.lines.is_empty()) {
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

    pub fn finish(&mut self) {
        for open in std::mem::take(&mut self.stack) {
            self.error(
                open.span,
                format!(
                    "unterminated conditional directive: missing endif::{}[]",
                    open.target
                ),
            );
        }
    }

    /// Capture a container before parsing its children, just as the reference
    /// reader does. Attribute entries in that body cannot affect this read-ahead.
    pub fn bound(
        &mut self,
        start: usize,
        context: &ParseContext,
        stop: impl Fn(&str) -> bool,
    ) -> usize {
        let old = self.limit;
        let mut end = start;
        while self.has(end, context) {
            if stop(&self[end].1) {
                break;
            }
            end += 1;
        }
        self.limit = end;
        old
    }

    pub fn content(&self, start: usize, end: usize) -> FilteredSource {
        let mut fragments = Vec::new();
        let first = self
            .loaded
            .partition_point(|(off, line)| off + line.len() < start);
        for (index, (off, line)) in self.loaded.iter().enumerate().skip(first) {
            let off = *off;
            if off >= end {
                break;
            }
            let left = start.saturating_sub(off).min(line.len());
            let right = end.saturating_sub(off).min(line.len());
            if right > left {
                fragments.push((off + left, &line[left..right]));
            }
            if off + line.len() < end {
                let (eol_start, eol) = &self.endings[index];
                if *eol_start < end {
                    fragments.push((*eol_start, eol.as_str()));
                }
            }
        }
        FilteredSource::from_fragments(fragments)
    }

    pub fn scan(
        &self,
        start: usize,
        end: usize,
        attrs: &Attributes,
        fold: bool,
        context: &mut ParseContext,
    ) -> Vec<Inline> {
        let content = self.content(start, end);
        context.body_subs.defer(content, None, attrs, fold)
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
        let content = self.content(start, end);
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
    if keyword != "include" && target.chars().any(char::is_whitespace) {
        return None;
    }
    Some((keyword, target, text))
}
