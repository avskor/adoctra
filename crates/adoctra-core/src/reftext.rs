//! Block-anchor reftext has an early a-stage, followed by c/q/r rather than
//! normal inline substitutions. Keep the early source map and reference owners
//! through protection and formatting; generated syntax is never rescanned as a.
use crate::substitution_context::Evaluation;
use crate::substitution_text::Text;
use crate::{Diagnostic, Inline, Span};

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
    /// A value already expanded by an attribute list's early a-stage.
    pub fn expanded(expanded: Text, refs: Vec<crate::inline_attributes::Reference>) -> Self {
        Read {
            value: expanded.text.clone(),
            references: refs.iter().map(|r| r.span).collect(),
            expanded,
            refs,
        }
    }

    pub fn finish(
        self,
        attributes: &crate::Attributes,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<Inline> {
        let mut protected = Text::default();
        for fragment in self.expanded.into_fragments() {
            protected.append_source(&fragment.text, false, fragment.owner, fragment.span);
        }
        let mut quotes = Vec::new();
        // Reftext's quote metadata still uses the existing read-only context;
        // on-demand reference-label evaluation is a separate conversion concern.
        let mut attributes = Evaluation::read_only(attributes);
        let text = crate::attribute_quotes::apply_text(
            protected,
            attributes.get("compat-mode").is_some(),
            &mut attributes,
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
