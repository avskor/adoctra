//! Lazy, source-preserving conditional preprocessing. Attribute mutations belong
//! to the parser; each line is evaluated once, when that parser requests it.
use crate::context::ParseContext;
use crate::scanner::{lines_with_offsets, FilteredSource};
use crate::{Attributes, Diagnostic, Inline, Severity, Span};
use std::ops::Index;

struct Conditional {
    target: String,
    active: bool,
    span: Span,
}

pub(crate) struct Reader<'a> {
    source: &'a str,
    raw: Vec<(usize, &'a str)>,
    next: usize,
    pub loaded: Vec<(usize, &'a str)>,
    pub limit: usize,
    stack: Vec<Conditional>,
    pub directives: Vec<Span>,
    pub inactive: Vec<Span>,
    pub diagnostics: Vec<Diagnostic>,
    preprocess: bool,
}

impl<'a> Reader<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            raw: lines_with_offsets(source),
            next: 0,
            loaded: Vec::new(),
            limit: usize::MAX,
            stack: Vec::new(),
            directives: Vec::new(),
            inactive: Vec::new(),
            diagnostics: Vec::new(),
            preprocess: true,
        }
    }

    pub fn plain(source: &'a str) -> Self {
        Self {
            preprocess: false,
            ..Self::new(source)
        }
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
            let Some(&(off, line)) = self.raw.get(self.next) else {
                return false;
            };
            self.next += 1;
            if self.preprocess {
                if let Some(attributes) = attributes {
                    if let Some(emitted) = self.process(off, line, attributes) {
                        self.loaded.push(emitted);
                    }
                    continue;
                }
            }
            self.loaded.push((off, line));
        }
        true
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
        line: &'a str,
        attributes: &Attributes,
    ) -> Option<(usize, &'a str)> {
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
            return Some((off + 1, candidate));
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
            Some((start, text))
        } else {
            self.directives.push(span);
            None
        }
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
            if stop(self[end].1) {
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
        for &(off, line) in &self.loaded[first..] {
            if off >= end {
                break;
            }
            let left = start.saturating_sub(off).min(line.len());
            let right = end.saturating_sub(off).min(line.len());
            if right > left {
                fragments.push((off + left, &line[left..right]));
            }
            if off + line.len() < end {
                // An inline conditional's closing bracket is replaced by its
                // physical line ending, never included in semantic text.
                let rest = &self.source[off + line.len()..];
                if let Some(nl) = rest.find('\n') {
                    let eol = off + line.len() + nl;
                    let eol_start = if eol > 0 && self.source.as_bytes()[eol - 1] == b'\r' {
                        eol - 1
                    } else {
                        eol
                    };
                    if eol < end {
                        fragments.push((eol_start, &self.source[eol_start..eol + 1]));
                    }
                }
            }
        }
        FilteredSource::from_fragments(fragments)
    }

    pub fn scan(&self, start: usize, end: usize, attrs: &Attributes, fold: bool) -> Vec<Inline> {
        let content = self.content(start, end);
        let mut inlines = crate::inline::scan_inlines_with(&content.text, 0, attrs, fold);
        content.restore_inlines(&mut inlines);
        inlines
    }

    pub fn scan_content(
        &self,
        start: usize,
        end: usize,
        attrs: &Attributes,
        context: &mut ParseContext,
    ) -> Vec<Inline> {
        let content = self.content(start, end);
        let before = context.comments.len();
        let mut inlines = crate::comments::scan_content(&content.text, 0, attrs, context);
        for span in &mut context.comments[before..] {
            *span = content.original_span(*span);
        }
        content.restore_inlines(&mut inlines);
        inlines
    }
}

impl<'a> Index<usize> for Reader<'a> {
    type Output = (usize, &'a str);
    fn index(&self, index: usize) -> &Self::Output {
        &self.loaded[index]
    }
}

fn directive(line: &str) -> Option<(&str, &str, &str)> {
    let (keyword, rest) = line.split_once("::")?;
    if !matches!(keyword, "ifdef" | "ifndef" | "endif") {
        return None;
    }
    let rest = rest.strip_suffix(']')?;
    let (target, text) = rest.split_once('[')?;
    if target.chars().any(char::is_whitespace) {
        return None;
    }
    Some((keyword, target, text))
}
