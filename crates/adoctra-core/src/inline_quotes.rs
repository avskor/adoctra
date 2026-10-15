//! Source edits from the shared ordered quote matcher. The later syntax scan
//! reads original coordinates and never scans generated text as new quotes.
use crate::attribute_quotes::Quote;
use crate::inline_pass::Passes;
use crate::{AttributeFormat as F, Attributes, Inline, Span};
use std::collections::BTreeMap;

pub(crate) struct Edit {
    pub end: usize,
    pub nodes: Vec<Inline>,
}

pub(crate) struct Quotes {
    pub edits: BTreeMap<usize, Edit>,
    pub matches: Vec<Quote>,
    pub passes: Passes,
}

impl Quotes {
    pub fn new(src: &str, base: usize, attrs: &Attributes, compat: bool) -> Self {
        let (text, passes) = Passes::extract(src, base, attrs, compat);
        let mut matches = Vec::new();
        let text = crate::attribute_quotes::apply_text(text, compat, attrs, &mut matches, true);
        let mut edits: BTreeMap<usize, Edit> = BTreeMap::new();
        let mut covered = base;
        for fragment in text.into_fragments() {
            let span = fragment.span;
            if span.start > covered {
                edits.insert(
                    covered,
                    Edit {
                        end: span.start,
                        nodes: Vec::new(),
                    },
                );
            }
            covered = covered.max(span.end);
            let value = fragment.decoded.unwrap_or(fragment.text);
            let node = if let Some(part) = fragment.part {
                Inline::Substituted {
                    value: String::new(),
                    parts: vec![part],
                    span,
                }
            } else {
                if !fragment.raw
                    && src.get(span.start - base..span.end - base) == Some(value.as_str())
                {
                    continue;
                }
                if fragment.raw {
                    Inline::Raw { value, span }
                } else {
                    Inline::Text { value, span }
                }
            };
            // A decorated typographic opener emits both a wrapper and a quote
            // from overlapping source ranges. Apply both in one source edit.
            let start = edits
                .range(..=span.start)
                .next_back()
                .filter(|(_, edit)| edit.end > span.start)
                .map_or(span.start, |(&start, _)| start);
            let edit = edits.entry(start).or_insert_with(|| Edit {
                end: span.end,
                nodes: Vec::new(),
            });
            edit.end = edit.end.max(span.end);
            edit.nodes.push(node);
        }
        if covered < base + src.len() {
            edits.insert(
                covered,
                Edit {
                    end: base + src.len(),
                    nodes: Vec::new(),
                },
            );
        }
        // Metadata can consume {name} before normal attribute expansion. Keep
        // an empty source node just as the later link matcher does, so tools
        // can still locate the declaration use without rendering it twice.
        for (at, _) in src.match_indices('{') {
            if passes.contains(base + at) {
                continue;
            }
            let Some(close) = src[at + 1..].find('}').map(|n| at + 1 + n) else {
                continue;
            };
            let name = &src[at + 1..close];
            if name.is_empty() || name.contains(char::is_whitespace) {
                continue;
            }
            if let Some((_, edit)) = edits.range_mut(..=base + at).next_back() {
                if base + close < edit.end {
                    edit.nodes.push(Inline::AttributeRef {
                        name: name.into(),
                        value: Some(String::new()),
                        parts: Vec::new(),
                        span: Span::new(base + at, base + close + 1),
                    });
                }
            }
        }
        matches.sort_by_key(|q| q.span.start);
        Self {
            edits,
            matches,
            passes,
        }
    }

    pub fn rebuild(&self, nodes: Vec<Inline>, range: std::ops::Range<usize>) -> Vec<Inline> {
        let start = self.matches.partition_point(|q| q.span.start < range.start);
        let templates = self.matches[start..]
            .iter()
            .take_while(|q| q.span.start < range.end)
            .filter(|q| !q.decorated && q.span.end <= range.end)
            .filter_map(|q| node(q.format?, q.span))
            .collect();
        crate::attribute_macros::rebuild(nodes, templates)
    }

    pub fn syntax(&self) -> impl Iterator<Item = Inline> + '_ {
        self.matches.iter().filter_map(|q| node(q.format?, q.span))
    }
}

fn node(kind: F, span: Span) -> Option<Inline> {
    let children = Vec::new();
    Some(match kind {
        F::Strong => Inline::Strong { children, span },
        F::Emphasis => Inline::Emphasis { children, span },
        F::Monospace => Inline::Monospace { children, span },
        F::Mark | F::Span => Inline::Mark { children, span },
        F::Superscript => Inline::Superscript { children, span },
        F::Subscript => Inline::Subscript { children, span },
    })
}
