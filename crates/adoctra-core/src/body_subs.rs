//! Conversion-time inline evaluation. Reading records source and evaluated
//! declarations; only surviving body fields execute after the reader finishes.
use crate::scanner::FilteredSource;
use crate::substitution_context::Evaluation;
use crate::{AttributePart, Attributes, Block, BlockKind, Diagnostic, Inline, Span};
use std::collections::{BTreeMap, BTreeSet};

struct Pending {
    source: FilteredSource,
    filtered: Option<FilteredSource>,
    fold: bool,
    hardbreaks: bool,
}

type Declaration = (usize, String, Option<Vec<AttributePart>>);

#[derive(Default)]
pub(crate) struct BodySubs {
    pending: BTreeMap<(usize, usize), Pending>,
    pub entries: Vec<Declaration>,
}

impl BodySubs {
    pub fn defer(
        &mut self,
        source: FilteredSource,
        filtered: Option<FilteredSource>,
        attrs: &Attributes,
        fold: bool,
    ) -> Vec<Inline> {
        if filtered.as_ref().unwrap_or(&source).text.is_empty() {
            return Vec::new();
        }
        let span = source.original_span(Span::new(0, source.text.len()));
        self.pending.insert(
            (span.start, span.end),
            Pending {
                source,
                filtered,
                fold,
                hardbreaks: attrs.local_hardbreaks,
            },
        );
        // Private placeholder; replaced before source mapping or public output.
        vec![Inline::Text {
            value: String::new(),
            span,
        }]
    }

    pub fn execute(
        mut self,
        blocks: &mut [Block],
        attributes: Attributes,
        locked: &BTreeSet<String>,
        counters: &mut BTreeMap<String, String>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let mut execution = Execution {
            pending: &mut self.pending,
            entries: self.entries.into_iter().peekable(),
            attributes,
            locked,
            counters,
            diagnostics,
        };
        execution.blocks(blocks);
    }
}

struct Execution<'a> {
    pending: &'a mut BTreeMap<(usize, usize), Pending>,
    entries: std::iter::Peekable<std::vec::IntoIter<Declaration>>,
    attributes: Attributes,
    locked: &'a BTreeSet<String>,
    counters: &'a mut BTreeMap<String, String>,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl Execution<'_> {
    fn replay(&mut self, at: usize) {
        while self.entries.peek().is_some_and(|e| e.0 <= at) {
            let (_, key, parts) = self.entries.next().unwrap();
            if let Some(parts) = parts {
                self.attributes.set_parts(key, parts);
            } else {
                self.attributes.remove(&key);
            }
        }
    }

    fn inlines(&mut self, nodes: &mut Vec<Inline>) {
        let [Inline::Text { span, .. }] = nodes.as_slice() else {
            return;
        };
        let Some(pending) = self.pending.remove(&(span.start, span.end)) else {
            return;
        };
        self.replay(span.start);
        self.attributes.local_hardbreaks = pending.hardbreaks;
        let compat = self.attributes.get("compat-mode").is_some();
        let mut evaluation =
            Evaluation::definition(&self.attributes, self.locked, self.counters, compat);
        let source = pending.filtered.as_ref().unwrap_or(&pending.source);
        let before = self.diagnostics.len();
        *nodes = crate::inline::scan_inlines_evaluated(
            &source.text,
            0,
            &mut evaluation,
            pending.fold,
            self.diagnostics,
        );
        if let Some(attributes) = evaluation.changed_attributes() {
            self.attributes = attributes;
        }
        if let Some(filtered) = &pending.filtered {
            filtered.restore_inlines(nodes);
            for diagnostic in &mut self.diagnostics[before..] {
                diagnostic.span = filtered.original_span(diagnostic.span);
            }
        }
        pending.source.restore_inlines(nodes);
        for diagnostic in &mut self.diagnostics[before..] {
            diagnostic.span = pending.source.original_span(diagnostic.span);
        }
    }

    fn blocks(&mut self, blocks: &mut [Block]) {
        for block in blocks {
            self.replay(block.span.start);
            match &mut block.kind {
                BlockKind::Paragraph { inlines } | BlockKind::Verse { inlines, .. } => {
                    self.inlines(inlines)
                }
                BlockKind::List { items, .. } => {
                    for item in items {
                        self.inlines(&mut item.inlines);
                        self.blocks(&mut item.blocks);
                    }
                }
                BlockKind::Admonition { blocks, .. }
                | BlockKind::Example { blocks }
                | BlockKind::Quote { blocks, .. }
                | BlockKind::Sidebar { blocks }
                | BlockKind::Open { blocks } => self.blocks(blocks),
                _ => {}
            }
        }
    }
}
