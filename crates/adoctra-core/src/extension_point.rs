//! Where the parser hands blocks to extension processors (Ruby's
//! `next_block`/`build_block` with `extensions`), see [`crate::extension`].

use crate::ast::{BlankEdges, Block, BlockKind, Span};
use crate::context::ParseContext;
use crate::extension::{
    AttrKey, BlockContext, BlockMacroProcessor, BlockMacroRequest, BlockProcessor, BlockRequest,
    ExtensionAttributes, MacroContent, NewBlock, NewBlockKind, ProcessContext,
};
use crate::parser::{PendingMeta, PendingTitle};
use std::ops::Range;
use std::sync::Arc;

/// A block the parser read for a processor; completion invokes it.
pub(crate) enum Pending {
    Block {
        processor: Arc<dyn BlockProcessor>,
        name: String,
        context: BlockContext,
        lines: Vec<String>,
    },
    Macro {
        processor: Arc<dyn BlockMacroProcessor>,
        line: String,
        off: usize,
        name: Range<usize>,
        target: Range<usize>,
        content: Option<Range<usize>>,
    },
}

/// The name, target and bracket content of a custom block macro line.
pub(crate) type MacroRanges = (Range<usize>, Range<usize>, Option<Range<usize>>);

/// Ruby's `CustomBlockMacroRx`:
/// `^(\p{Word}[\p{Word}-]*)::(|\S|\S.*?\S)\[(.+)?\]$`, returning the name,
/// target and bracket content ranges.
pub(crate) fn custom_block_macro(line: &str) -> Option<MacroRanges> {
    let mut chars = line.char_indices();
    let (_, first) = chars.next()?;
    if !crate::unicode_word::is_word(first) {
        return None;
    }
    let name_end = line
        .char_indices()
        .find(|&(_, c)| !(crate::unicode_word::is_word(c) || c == '-'))
        .map_or(line.len(), |(at, _)| at);
    let rest = line[name_end..].strip_prefix("::")?;
    let start = name_end + 2;
    let close = line.len().checked_sub(1)?;
    if !line.ends_with(']') || close < start {
        return None;
    }
    let blank = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c');
    // The lazy target ends at the first `[` that leaves a valid target.
    let open = rest
        .match_indices('[')
        .map(|(at, _)| start + at)
        .filter(|&at| at < close)
        .find(|&at| {
            let target = &line[start..at];
            target.is_empty()
                || (!target.starts_with(blank) && !target.chars().next_back().is_some_and(blank))
        })?;
    let content = (open + 1 < close).then_some(open + 1..close);
    Some((0..name_end, start..open, content))
}

/// The pending metadata as Ruby's attribute hash, in insertion order.
fn metadata_attributes(pending: &PendingMeta) -> ExtensionAttributes {
    let mut attributes = ExtensionAttributes::new();
    for key in &pending.list.order {
        let value = match key {
            AttrKey::Positional(index) => pending.list.value(*index).map(|v| v.text.clone()),
            AttrKey::Named(name) => match name.as_str() {
                "title" => pending.title.as_ref().map(|t| t.text.clone()),
                "id" => pending.id.clone(),
                "role" => (!pending.roles.is_empty()).then(|| pending.roles.join(" ")),
                "reftext" => pending.reftext.as_ref().map(|r| r.value.clone()),
                name => match name.strip_suffix("-option") {
                    Some(option) if pending.attrs.options.iter().any(|o| o == option) => {
                        Some(String::new())
                    }
                    _ => pending.list.named(name).map(str::to_owned),
                },
            },
        };
        match (key, value) {
            (_, Some(value)) => attributes.set_key(key.clone(), value),
            // An empty positional value is Ruby's `nil` under its index.
            (AttrKey::Positional(_), None) => attributes.set_value(key.clone(), None),
            (AttrKey::Named(_), None) => {}
        }
    }
    attributes
}

/// Ruby's `AttributeList.rekey(attributes, [nil, *positional])` and
/// `default_attrs` (`||=`).
fn rekey(attributes: &mut ExtensionAttributes, positional: &[&str], defaults: &[(&str, &str)]) {
    for (index, name) in positional.iter().enumerate() {
        if let Some(value) = attributes.positional(index + 2).map(str::to_owned) {
            attributes.set(*name, value);
        }
    }
    for (name, value) in defaults {
        if !attributes.contains(name) {
            attributes.set(*name, *value);
        }
    }
}

/// Invoke the processor for a block the parser read for it. Returns the
/// replacement block and its metadata, or `None` to drop the block.
pub(crate) fn run(
    pending: Pending,
    block: Block,
    metadata: &mut PendingMeta,
    includes: &crate::xref_target::Includes,
    context: &mut ParseContext,
) -> Option<Block> {
    let span = block.span;
    let new = match pending {
        Pending::Block {
            processor,
            name,
            context: cloaked,
            lines,
        } => {
            let mut attributes = metadata_attributes(metadata);
            rekey(
                &mut attributes,
                processor.positional_attributes(),
                processor.default_attributes(),
            );
            attributes.set("cloaked-context", cloaked.as_str());
            let request = BlockRequest {
                name,
                context: cloaked,
                lines,
                attributes,
                span,
            };
            invoke(context, span, |ctx| processor.process(request, ctx))
        }
        Pending::Macro {
            processor,
            line,
            off,
            name,
            target,
            content,
        } => {
            // Ruby expands the target first; an empty result from
            // `drop-line` drops the macro and its metadata.
            let mut value = line[target.clone()].to_owned();
            if value.contains('{') {
                let source = crate::image::target_text(&value, off + target.start);
                let (expanded, refs) = context.block_attribute_line(source);
                if expanded.text.is_empty() && refs.iter().any(|r| r.drops_line) {
                    return None;
                }
                value = expanded.text;
            }
            let mut attributes = metadata_attributes(metadata);
            match (processor.content(), content) {
                (MacroContent::Attributes, Some(content)) => {
                    let attr_line = crate::block_attributes::AttrLine {
                        inner: line[content.clone()].to_owned(),
                        base: off + content.start,
                    };
                    let (list, _) = crate::block_attributes::macro_list(
                        attr_line,
                        processor.positional_attributes(),
                        context,
                        includes,
                    );
                    for (key, value) in metadata_attributes(&list).iter() {
                        attributes.set_value(key.clone(), value.map(str::to_owned));
                    }
                    // The macro's own metadata replaces the preceding lines'.
                    if list.title.is_some() {
                        metadata.title = list.title;
                    }
                    if list.id.is_some() {
                        metadata.id = list.id;
                    }
                    if !list.roles.is_empty() {
                        metadata.roles = list.roles;
                    }
                }
                (MacroContent::Attributes, None) => {}
                (MacroContent::Text, content) => {
                    let text = content.map_or("", |range| &line[range]);
                    attributes.set("text", text);
                }
            }
            rekey(&mut attributes, &[], processor.default_attributes());
            let request = BlockMacroRequest {
                name: line[name].to_owned(),
                target: value,
                attributes,
                span,
            };
            invoke(context, span, |ctx| processor.process(request, ctx))
        }
    }?;
    Some(new_block(new, span, metadata))
}

fn invoke(
    context: &mut ParseContext,
    span: Span,
    process: impl FnOnce(&mut ProcessContext<'_>) -> Option<NewBlock>,
) -> Option<NewBlock> {
    let attributes = Arc::clone(&context.attributes);
    let base_dir = context.base_dir.clone();
    let output = context.output_target().cloned();
    let mut ctx = ProcessContext::new(
        &attributes,
        context.safe_mode(),
        base_dir.as_deref(),
        output.as_ref(),
        span,
    );
    let new = process(&mut ctx);
    context.diagnostics.extend(ctx.into_diagnostics());
    new
}

/// Ruby's `attributes.replace block.attributes`: the new block's attributes
/// become the metadata completion applies.
fn new_block(new: NewBlock, span: Span, metadata: &mut PendingMeta) -> Block {
    let previous = std::mem::take(metadata);
    let mut attrs = crate::ast::BlockAttrs::default();
    for (key, value) in new.attributes.iter() {
        // Ruby's converter reads named values; positional keys and `nil`
        // values do not reach the output.
        let (AttrKey::Named(name), Some(value)) = (key, value) else {
            continue;
        };
        match name.as_str() {
            "title" => {
                metadata.title = Some(match &previous.title {
                    Some(title) if title.text == value => title.clone(),
                    _ => PendingTitle {
                        text: value.to_owned(),
                        base: span.start,
                        origin: Some(span),
                    },
                });
            }
            "id" => metadata.id = Some(value.to_owned()),
            "role" => {
                metadata.roles = value
                    .split(crate::ruby_strip::is_ruby_space)
                    .filter(|role| !role.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
            "style" => attrs.style = Some(value.to_owned()),
            "reftext" => metadata.reftext = previous.reftext.clone(),
            // Text the processor created gets no further substitutions.
            "subs" => {}
            name => match name.strip_suffix("-option") {
                Some(option) => attrs.options.push(option.to_owned()),
                None => {
                    attrs.named.insert(name.to_owned(), value.to_owned());
                }
            },
        }
    }
    metadata.attrs = attrs;
    // Ruby's `Block.new(…, source: string)`: `prepare_source_string` strips
    // trailing whitespace from every line.
    let prepare = |source: String| {
        source
            .lines()
            .map(|line| line.trim_end_matches([' ', '\t', '\r', '\n', '\x0b', '\x0c', '\0']))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let kind = match new.kind {
        NewBlockKind::Image { target } => BlockKind::Image {
            target,
            src: String::new(),
            alt: Vec::new(),
            svg: None,
        },
        NewBlockKind::Listing { source } => BlockKind::Listing {
            text: prepare(source),
            callouts: Vec::new(),
            inlines: None,
            blank_edges: BlankEdges::default(),
        },
        NewBlockKind::Literal { source } => BlockKind::Literal {
            text: prepare(source),
            callouts: Vec::new(),
            inlines: None,
        },
        NewBlockKind::Pass { source } => BlockKind::Passthrough {
            text: prepare(source),
            inlines: None,
        },
    };
    Block::new(span, kind)
}

/// The lines of a block read for a processor (Ruby drops line comments from
/// a paragraph read with `skip_line_comments`).
pub(crate) fn block_lines(text: &str, paragraph: bool) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .filter(|line| !(paragraph && line.starts_with("//") && !line[2..].starts_with('/')))
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(line: &str) -> Option<(&str, &str, Option<&str>)> {
        custom_block_macro(line)
            .map(|(name, target, content)| (&line[name], &line[target], content.map(|c| &line[c])))
    }

    #[test]
    fn custom_block_macro_follows_rubys_pattern() {
        assert_eq!(
            parts("plantuml::a.puml[format=svg]"),
            Some(("plantuml", "a.puml", Some("format=svg")))
        );
        assert_eq!(parts("gist::[]"), Some(("gist", "", None)));
        assert_eq!(parts("my-ext::x[a]"), Some(("my-ext", "x", Some("a"))));
        // The lazy target stops at the first `[` leaving a valid target.
        assert_eq!(parts("m::a[b][c]"), Some(("m", "a", Some("b][c"))));
        assert_eq!(parts("m::a b[c]"), Some(("m", "a b", Some("c"))));
        assert_eq!(parts("m:: a[c]"), None);
        assert_eq!(parts("m::a [c]"), None);
        assert_eq!(parts("-m::a[c]"), None);
        assert_eq!(parts("m::a[c] "), None);
        assert_eq!(parts("m:a[c]"), None);
    }

    #[test]
    fn paragraph_lines_drop_line_comments() {
        assert_eq!(
            block_lines("a\n// c\n/// d\n//\nb", true),
            vec!["a", "/// d", "b"]
        );
        assert_eq!(block_lines("a\n// c", false), vec!["a", "// c"]);
        assert!(block_lines("", false).is_empty());
    }
}
