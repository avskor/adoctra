//! Block-anchor reftext has an early a-stage, followed by c/q/r rather than
//! normal inline substitutions. Keep the early source map and reference owners
//! through protection and formatting; generated syntax is never rescanned as a.
//! Like Ruby's `reftext`, a reference formats the read value again each time,
//! with the conversion state at that reference (see body_subs.rs).
use crate::substitution_context::Evaluation;
use crate::substitution_text::Text;
use crate::{Diagnostic, Inline, Span};

#[derive(Clone)]
pub(crate) struct Read {
    expanded: Text,
    refs: Vec<crate::inline_attributes::Reference>,
    pub value: String,
    pub references: Vec<Span>,
}

pub(crate) fn read(
    value: &str,
    base: usize,
    evaluation: &mut Evaluation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Read {
    let mut source = Text::default();
    // sub_attributes does not protect special characters before expansion.
    source.append_source(value, true, 0, Span::new(base, base + value.len()));
    let (expanded, refs) = crate::inline_attributes::apply(source, evaluation, diagnostics);
    let references = refs.iter().map(|r| r.span).collect();
    let lexical = expanded.text.clone();
    Read {
        expanded,
        refs,
        value: lexical,
        references,
    }
}

impl Read {
    /// Source reftext without the early a-stage (a section-title anchor).
    pub fn source(value: &str, base: usize) -> Self {
        let mut expanded = Text::default();
        expanded.append_source(value, true, 0, Span::new(base, base + value.len()));
        Self::expanded(expanded, Vec::new())
    }

    /// A value already expanded by an attribute list's early a-stage.
    pub fn expanded(expanded: Text, refs: Vec<crate::inline_attributes::Reference>) -> Self {
        Read {
            value: expanded.text.clone(),
            references: refs.iter().map(|r| r.span).collect(),
            expanded,
            refs,
        }
    }

    /// The model's reftext, formatted once at block/header completion with
    /// the reading attributes. Ruby formats only for a reference, so this
    /// projection neither executes expressions nor reports diagnostics.
    pub fn snapshot(&self, attributes: &crate::Attributes) -> Vec<Inline> {
        self.clone()
            .finish(&mut Evaluation::read_only(attributes), &mut Vec::new())
    }

    /// Ruby's `apply_reftext_subs`: c/q/r over the early-expanded value. Quote
    /// metadata sees (and may change) the evaluation's attributes and counters.
    pub fn finish(
        self,
        evaluation: &mut Evaluation<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<Inline> {
        let mut protected = Text::default();
        for fragment in self.expanded.into_fragments() {
            protected.append_source(&fragment.text, false, fragment.owner, fragment.span);
        }
        let mut quotes = Vec::new();
        let compat = evaluation.get("compat-mode").is_some();
        let text = crate::attribute_quotes::apply_text(
            protected,
            compat,
            evaluation,
            &mut quotes,
            true,
            diagnostics,
        );
        let text = crate::replacements::apply_text(text, true, true);
        let nodes = crate::inline_attributes::nodes(text, self.refs);
        let templates = quotes
            .into_iter()
            .filter(|q| !q.decorated)
            .filter_map(|q| crate::inline_quotes::node(q.format?, q.span))
            .collect();
        crate::attribute_macros::rebuild(nodes, templates)
    }
}
