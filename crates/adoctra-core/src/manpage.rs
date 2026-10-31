//! Manual pages (doctype `manpage`), following Asciidoctor 2.0.23
//! `Parser.parse_manpage_header`: the document title gives `mantitle` and
//! `manvolnum`, and the header consumes the NAME section into `manname`,
//! `mannames` and `manpurpose` unless both name attributes are already set.
use crate::attributes::reference_text;
use crate::context::ParseContext;
use crate::parser::PendingMeta;
use crate::reader::Reader;
use crate::ruby_strip::RubyStrip;
use crate::substitution_text::Text;
use crate::{AttributePart, Block, BlockKind, Diagnostic, Manpage, NameSection, Severity, Span};
use std::ops::Range;
use std::sync::Arc;

/// Read the manual page names at the end of the header (`*i` is the first
/// line after it). Like Ruby, metadata lines before the NAME section belong
/// to the first body block (`pending`). When the section is missing or does
/// not conform, the reader returns to the first line after the blank lines
/// and reads the metadata and the section again as body content.
pub(crate) fn read(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    pending: &mut PendingMeta,
    doc_id: Option<&str>,
) -> Manpage {
    read_title(lines, context);
    if let (Some(name), Some(_)) = (
        context.attributes.value_parts("manname"),
        context.attributes.get("manpurpose"),
    ) {
        let attributes = Arc::make_mut(&mut context.attributes);
        if attributes.get("manname-title").is_none() {
            attributes.set("manname-title", "Name");
        }
        let names = vec![reference_text(&name)];
        attributes.set_parts("mannames", raw(inspect(&names)));
        return Manpage {
            names,
            ..Manpage::default()
        };
    }
    while lines.has(*i, context) && lines[*i].1.is_ruby_blank() {
        *i += 1;
    }
    let save = *i;
    loop {
        if !crate::parser::consume_block_metadata(lines, i, context, pending) {
            break;
        }
        while lines.has(*i, context) && lines[*i].1.is_ruby_blank() {
            *i += 1;
        }
    }
    let (manpage, error) = match name_section(lines, i, context, doc_id) {
        Ok(manpage) => (manpage, None),
        Err((name_section, error)) => (
            Manpage {
                name_section: name_section.map(|section| *section),
                ..Manpage::default()
            },
            Some(error),
        ),
    };
    let Some(error) = error else {
        return manpage;
    };
    let span = if lines.has(save, context) {
        let (off, line) = lines.line(save);
        Span::new(off, off + line.len())
    } else {
        let end = save
            .checked_sub(1)
            .map_or(0, |last| lines[last].0 + lines[last].1.len());
        Span::new(end, end)
    };
    context.inline_diagnostics.push(Diagnostic {
        omitted_span: None,
        report: Default::default(),
        span,
        severity: Severity::Error,
        message: error.into(),
    });
    *i = save;
    let name = context
        .attributes
        .value_parts("docname")
        .map_or_else(|| "command".to_owned(), |parts| reference_text(&parts));
    let attributes = Arc::make_mut(&mut context.attributes);
    attributes.set_parts("manname", raw(name.clone()));
    let names = vec![name];
    attributes.set_parts("mannames", raw(inspect(&names)));
    Manpage { names, ..manpage }
}

/// `mantitle` and `manvolnum` from the `doctitle` attribute.
fn read_title(lines: &mut Reader<'_>, context: &mut ParseContext) {
    let span = context.doctitle_span.unwrap_or_else(|| Span::new(0, 0));
    let doctitle = context.attributes.value_parts("doctitle");
    let matched = doctitle.as_ref().and_then(|parts| {
        let text = Text::from_parts_at(parts, span);
        let (title, volnum) = title_volnum(&text.text)?;
        Some((text, title, volnum))
    });
    let (mantitle, manvolnum) = match matched {
        Some((text, title, volnum)) => {
            let manvolnum = text.text[volnum].to_owned();
            let mut value = Text::default();
            text.copy(title, &mut value);
            if value.text.contains('{') {
                value = context.block_attribute_line(value).0;
            }
            let lexical = reference_text(&value.into_parts());
            (
                lexical.chars().flat_map(char::to_lowercase).collect(),
                manvolnum,
            )
        }
        None => {
            let span = if lines.has(0, context) {
                let (off, line) = lines.line(0);
                Span::new(off, off + line.len())
            } else {
                Span::new(0, 0)
            };
            context.inline_diagnostics.push(Diagnostic {
                omitted_span: None,
                report: Default::default(),
                span,
                severity: Severity::Error,
                message: "non-conforming manpage title".into(),
            });
            let mantitle = ["doctitle", "docname"]
                .into_iter()
                .find_map(|key| context.attributes.value_parts(key))
                .map_or_else(|| "command".to_owned(), |parts| reference_text(&parts));
            (mantitle, "1".to_owned())
        }
    };
    let attributes = Arc::make_mut(&mut context.attributes);
    attributes.set_parts("manvolnum", raw(manvolnum));
    attributes.set_parts("mantitle", raw(mantitle));
}

/// The NAME section at `*i`: a level-1 section title, then the lines up to a
/// blank line (line comments skipped, each left-stripped, joined with a
/// space) as `name - purpose`. A nonconforming body keeps the section's
/// registration, as in Ruby.
fn name_section(
    lines: &mut Reader<'_>,
    i: &mut usize,
    context: &mut ParseContext,
    doc_id: Option<&str>,
) -> Result<Manpage, (Option<Box<NameSection>>, &'static str)> {
    if !lines.has(*i, context) {
        return Err((None, "name section expected"));
    }
    let (off, line) = lines.line(*i);
    let underline = lines.has(*i + 1, context).then(|| lines.line(*i + 1));
    let heading = crate::parser::parse_heading(&line)
        .map(|(level, title)| (level, title, off + line.len(), 1))
        .or_else(|| {
            let (underline_off, underline) = underline?;
            let level = crate::header::setext_level(&line, &underline)?;
            Some((level, line.as_ref(), underline_off + underline.len(), 2))
        });
    let Some((level, title, end, consumed)) = heading else {
        return Err((None, "name section expected"));
    };
    if level != 1 {
        return Err((None, "name section must be at level 1"));
    }
    let anchor = crate::parser::section_anchor(title);
    let title = anchor.as_ref().map_or(title, |anchor| anchor.title);
    let base = off + (title.as_ptr() as usize - line.as_ptr() as usize);
    let attributes = Arc::clone(&context.attributes);
    let inlines = context.reading_title(title, base, &attributes, &lines.include_catalog);
    if anchor.is_none() {
        context.generating_id();
    }
    let mut heading = Block::new(Span::new(off, end), BlockKind::Heading { level, inlines });
    if let Some(anchor) = &anchor {
        heading.id = Some(anchor.id.to_owned());
        if let Some(reftext) = anchor.reftext {
            let base = off + (reftext.as_ptr() as usize - line.as_ptr() as usize);
            context.attach_reftext(&mut heading, crate::reftext::Read::source(reftext, base));
        }
    }
    heading.attribute_context = Arc::clone(&context.attributes);
    let title_lines = (*i..*i + consumed)
        .map(|index| {
            let (off, line) = lines.line(index);
            Span::new(off, off + line.len())
        })
        .collect::<Vec<_>>();
    *i += consumed;
    while lines.has(*i, context) && lines[*i].1.is_ruby_blank() {
        *i += 1;
    }
    // Ruby's `read_lines_until break_on_blank_lines: true, skip_line_comments: true`.
    let mut buffer = Text::default();
    let mut body: Option<Span> = None;
    while lines.has(*i, context) && !lines[*i].1.is_ruby_blank() {
        let (off, line) = lines.line(*i);
        let span = Span::new(off, off + line.len());
        *i += 1;
        if crate::comments::is_line_comment(&line) {
            context.comments.push(span);
            continue;
        }
        let text = line.ruby_lstrip();
        let start = off + line.len() - text.len();
        if let Some(previous) = body {
            buffer.append_source(" ", true, 0, Span::new(previous.end, previous.end));
        }
        buffer.append_source(text, true, 0, Span::new(start, span.end));
        body = Some(Span::new(
            body.map_or(span.start, |body| body.start),
            span.end,
        ));
    }
    let Some((name, purpose)) = name_purpose(&buffer.text) else {
        let section = NameSection {
            heading,
            body: None,
        };
        return Err((Some(Box::new(section)), "non-conforming name section body"));
    };
    lines.heading_lines.extend(title_lines);
    let mut text = Text::default();
    buffer.copy(name, &mut text);
    if text.text.contains('{') {
        let (expanded, references) = context.block_attribute_line(text);
        lines
            .metadata_references
            .extend(references.into_iter().map(|r| r.span));
        text = expanded;
    }
    let mut names = reference_text(&text.into_parts())
        .split(',')
        .map(|name| name.ruby_lstrip().to_owned())
        .collect::<Vec<_>>();
    // Ruby's `String#split` drops trailing empty fields.
    while names.last().is_some_and(String::is_empty) {
        names.pop();
    }
    let mut text = Text::default();
    buffer.copy(purpose, &mut text);
    if text.text.contains('{') {
        let (expanded, references) = context.block_attribute_line(text);
        lines
            .metadata_references
            .extend(references.into_iter().map(|r| r.span));
        text = expanded;
    }
    let purpose = text.into_parts();
    let id = heading
        .id
        .clone()
        .or_else(|| crate::resolve::section_id(&heading, doc_id));
    let title = match &heading.kind {
        BlockKind::Heading { inlines, .. } => crate::resolve::label_parts(inlines),
        _ => unreachable!(),
    };
    let attributes = Arc::make_mut(&mut context.attributes);
    let section_title = attributes.get("manname-title").is_none();
    if section_title {
        attributes.set_parts("manname-title", title);
    }
    if let Some(id) = id {
        attributes.set("manname-id", id);
    }
    if let Some(name) = names.first() {
        attributes.set_parts("manname", raw(name.clone()));
    }
    attributes.set_parts("mannames", raw(inspect(&names)));
    attributes.set_parts("manpurpose", purpose);
    Ok(Manpage {
        names,
        name_section: Some(NameSection { heading, body }),
        section_title,
    })
}

fn raw(value: String) -> Vec<AttributePart> {
    vec![AttributePart::Raw(value)]
}

/// Ruby's `ManpageTitleVolnumRx` (`^(.+?) *\( *(.+?) *\)$`): the title and
/// volume number ranges.
fn title_volnum(s: &str) -> Option<(Range<usize>, Range<usize>)> {
    let inner_end = s.strip_suffix(')')?.len();
    for (p, _) in s.char_indices().skip(1) {
        let rest = &s[p..];
        let open = p + rest.len() - rest.trim_start_matches(' ').len();
        if !s[open..].starts_with('(') || open >= inner_end {
            continue;
        }
        let inner = &s[open + 1..inner_end];
        if inner.is_empty() {
            continue;
        }
        // The lazy number keeps at least one character, then gives the
        // trailing spaces to ` *`.
        let lead = inner.len() - inner.trim_start_matches(' ').len();
        let start = open + 1 + lead.min(inner.len() - 1);
        let end = (open + 1 + inner.trim_end_matches(' ').len()).max(start + 1);
        return Some((0..p, start..end));
    }
    None
}

/// Ruby's `ManpageNamePurposeRx` (`^(.+?) +- +(.+)$`): the name and purpose
/// ranges.
fn name_purpose(s: &str) -> Option<(Range<usize>, Range<usize>)> {
    for (p, _) in s.char_indices().skip(1) {
        let rest = &s[p..];
        let after = rest.trim_start_matches(' ');
        let Some(dash) = after.strip_prefix('-').filter(|_| after.len() < rest.len()) else {
            continue;
        };
        let purpose = dash.trim_start_matches(' ');
        if purpose.len() == dash.len() {
            continue;
        }
        let start = match purpose.is_empty() {
            false => s.len() - purpose.len(),
            // `.+` takes back the last of at least two spaces.
            true if dash.len() > 1 => s.len() - 1,
            true => continue,
        };
        return Some((0..p, start..s.len()));
    }
    None
}

/// Ruby's `Array#inspect` of the names: the value a `{mannames}` reference
/// shows.
fn inspect(names: &[String]) -> String {
    let items = names
        .iter()
        .map(|name| {
            let mut out = String::from('"');
            let mut chars = name.chars().peekable();
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '#' if matches!(chars.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    '\x0c' => out.push_str("\\f"),
                    '\x0b' => out.push_str("\\v"),
                    '\x07' => out.push_str("\\a"),
                    '\x08' => out.push_str("\\b"),
                    '\x1b' => out.push_str("\\e"),
                    ch if ch.is_control() => out.push_str(&format!("\\u{:04X}", ch as u32)),
                    ch => out.push(ch),
                }
            }
            out.push('"');
            out
        })
        .collect::<Vec<_>>();
    format!("[{}]", items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Inline;

    fn slices(s: &str, ranges: Option<(Range<usize>, Range<usize>)>) -> Option<(&str, &str)> {
        ranges.map(|(a, b)| (&s[a], &s[b]))
    }

    #[test]
    fn title_and_volume_follow_the_lazy_ruby_expression() {
        let cases = [
            ("git-foo(1)", Some(("git-foo", "1"))),
            ("Foo-Bar ( 1 )", Some(("Foo-Bar", "1"))),
            ("a(b)(1)", Some(("a", "b)(1"))),
            ("a(  )", Some(("a", " "))),
            ("(1)", None),
            ("a()", None),
            ("foo", None),
            ("foo(1) x", None),
        ];
        for (input, expected) in cases {
            assert_eq!(slices(input, title_volnum(input)), expected, "{input}");
        }
    }

    #[test]
    fn name_and_purpose_split_at_the_first_spaced_dash() {
        let cases = [
            ("foo - bar", Some(("foo", "bar"))),
            ("a  x - b - c", Some(("a  x", "b - c"))),
            ("foo-bar - baz", Some(("foo-bar", "baz"))),
            ("foo -bar", None),
            ("- bar", None),
            ("foo bar", None),
            ("", None),
        ];
        for (input, expected) in cases {
            assert_eq!(slices(input, name_purpose(input)), expected, "{input}");
        }
    }

    fn heading_id(block: &Block) -> Option<&str> {
        match &block.kind {
            BlockKind::Section { blocks, .. } => blocks[0].id.as_deref(),
            _ => None,
        }
    }

    #[test]
    fn the_name_section_follows_the_id_policy() {
        let src = "= prog(1)\n:doctype: manpage\n:idprefix: x-\n:idseparator: -+\n\n\
                   == The Name\n\nprog - does things\n\n== After {idseparator}\n";
        let doc = crate::parse(src).document;
        assert_eq!(doc.attributes.get("manname-id"), Some("x-the-name"));
        let section = doc.manpage.as_ref().unwrap().name_section.as_ref().unwrap();
        assert_eq!(section.heading.id.as_deref(), Some("x-the-name"));
        // Generating the NAME section's id cut the separator for later reading.
        let BlockKind::Section { blocks, .. } = &doc.blocks[1].kind else {
            panic!("{:?}", doc.blocks[1].kind)
        };
        assert_eq!(blocks[0].id.as_deref(), Some("x-after"));

        // Without `sectids` neither the NAME section nor `manname-id` has one.
        let src =
            "= prog(1)\n:doctype: manpage\n:sectids!:\n\n== NAME\n\nprog - does\n\n== SYNOPSIS\n";
        let doc = crate::parse(src).document;
        assert_eq!(doc.attributes.get("manname-id"), None);
        let section = doc.manpage.as_ref().unwrap().name_section.as_ref().unwrap();
        assert_eq!(section.heading.id, None);
        assert_eq!(heading_id(&doc.blocks[1]), None);
    }

    #[test]
    fn the_header_reads_the_name_section() {
        let src = "= Git-Foo(1)\n:doctype: manpage\n\n[[first]]\n== NAME\n\n\
                   git-foo, git-bar - does {doctype}\n// skipped\n  *foo*\n\n== SYNOPSIS\n\ntext\n";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let doc = &result.document;
        let attribute = |key| doc.attributes.get(key).unwrap();
        assert_eq!(attribute("mantitle"), "git-foo");
        assert_eq!(attribute("manvolnum"), "1");
        assert_eq!(attribute("manname"), "git-foo");
        assert_eq!(attribute("mannames"), r#"["git-foo", "git-bar"]"#);
        // Only attribute references are substituted; the value is lexical.
        assert_eq!(attribute("manpurpose"), "does manpage *foo*");
        assert_eq!(attribute("manname-title"), "NAME");
        assert_eq!(attribute("manname-id"), "_name");
        let manpage = doc.manpage.as_ref().unwrap();
        assert_eq!(manpage.names, ["git-foo", "git-bar"]);
        assert!(manpage.section_title);
        let section = manpage.name_section.as_ref().unwrap();
        let start = src.find("== NAME").unwrap();
        assert_eq!(section.heading.span, Span::new(start, start + 7));
        assert_eq!(section.heading.id.as_deref(), Some("_name"));
        let BlockKind::Heading { level: 1, inlines } = &section.heading.kind else {
            panic!("{:?}", section.heading.kind)
        };
        assert_eq!(inlines[0].span(), Span::new(start + 3, start + 7));
        let body = src.find("git-foo,").unwrap();
        let end = src.find("*foo*").unwrap() + 5;
        assert_eq!(section.body, Some(Span::new(body, end)));
        // The body starts with the next section, which takes the metadata
        // read before the NAME section.
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(heading_id(&doc.blocks[1]), Some("first"));
        let comment = src.find("// skipped").unwrap();
        assert!(result.comments.contains(&Span::new(comment, comment + 10)));
    }

    #[test]
    fn name_attributes_resolve_references_and_spans() {
        let src = "= x(1)\n:doctype: manpage\n\n== Name *A* [[nm]]\n\nx - {missing} y\n\n== B\n\n<<nm>>\n";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let doc = &result.document;
        assert_eq!(doc.attributes.get("manname-id"), Some("nm"));
        assert_eq!(doc.attributes.get("manname-title"), Some("Name A"));
        assert_eq!(doc.attributes.get("manpurpose"), Some("{missing} y"));
        let refs = doc.flat_blocks();
        let crate::BlockKind::Paragraph { inlines } = &refs.last().unwrap().kind else {
            panic!()
        };
        let Inline::CrossReference { children, .. } = &inlines[0] else {
            panic!("{inlines:?}")
        };
        assert!(
            matches!(&children[..], [Inline::Text { value, .. }, Inline::Strong { .. }] if value == "Name "),
            "{children:?}"
        );
        // A warning about a missing reference names its physical range.
        let src =
            "= x(1)\n:doctype: manpage\n:attribute-missing: warn\n\n== NAME\n\nx - {missing}\n";
        let result = crate::parse(src);
        let start = src.find("{missing}").unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(result.diagnostics[0].span, Span::new(start, start + 9));
        assert_eq!(
            result.diagnostics[0].message,
            "skipping reference to missing attribute: missing"
        );
    }

    #[test]
    fn header_attributes_name_the_page_without_consuming_the_section() {
        let src =
            "= Foo (8)\n:doctype: manpage\n:manname: zz\n:manpurpose: p\n\n== NAME\n\nfoo - bar\n";
        let doc = crate::parse(src).document;
        assert_eq!(doc.attributes.get("mantitle"), Some("foo"));
        assert_eq!(doc.attributes.get("manvolnum"), Some("8"));
        assert_eq!(doc.attributes.get("manname-title"), Some("Name"));
        assert_eq!(doc.attributes.get("mannames"), Some(r#"["zz"]"#));
        assert_eq!(doc.attributes.get("manname-id"), None);
        let manpage = doc.manpage.as_ref().unwrap();
        assert_eq!(
            (manpage.names.clone(), manpage.name_section.clone()),
            (vec!["zz".to_owned()], None)
        );
        assert_eq!(heading_id(&doc.blocks[1]), Some("_name"));
    }

    #[test]
    fn nonconforming_headers_report_errors_and_fall_back() {
        let error = |src: &str| {
            let result = crate::parse(src);
            let messages = result
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| (d.message.clone(), d.span))
                .collect::<Vec<_>>();
            (result.document, messages)
        };
        let (doc, errors) = error("// c\n:doctype: manpage\n\n== NAME\n\nfoo - bar\n");
        assert_eq!(
            errors,
            [("non-conforming manpage title".to_owned(), Span::new(0, 4))]
        );
        assert_eq!(doc.attributes.get("mantitle"), Some("command"));
        assert_eq!(doc.attributes.get("manvolnum"), Some("1"));
        assert_eq!(doc.attributes.get("manname"), Some("foo"));
        // Missing or misplaced sections stay body content.
        let src = "= foo(1)\n:doctype: manpage\n\n[.r]\nfoo bar\n";
        let (doc, errors) = error(src);
        assert_eq!(
            errors,
            [("name section expected".to_owned(), Span::new(28, 32))]
        );
        assert_eq!(doc.attributes.get("manname"), Some("command"));
        assert_eq!(doc.manpage.as_ref().unwrap().names, ["command"]);
        // Like Ruby, the metadata is read twice.
        assert_eq!(doc.blocks[1].roles, ["r", "r"]);
        let (doc, errors) = error("= foo(1)\n:doctype: manpage\n\n=== NAME\n\nx - y\n");
        assert_eq!(errors[0].0, "name section must be at level 1");
        assert!(doc.manpage.as_ref().unwrap().name_section.is_none());
        // A nonconforming body keeps the section registered under its id.
        let src = "= foo(1)\n:doctype: manpage\n\n== NAME\n\nfoo bar\n\n== D\n\n<<_name>>\n";
        let (doc, errors) = error(src);
        assert_eq!(
            errors,
            [(
                "non-conforming name section body".to_owned(),
                Span::new(28, 35)
            )]
        );
        let manpage = doc.manpage.as_ref().unwrap();
        let section = manpage.name_section.as_ref().unwrap();
        assert_eq!(
            (section.heading.id.as_deref(), section.body),
            (Some("_name"), None)
        );
        assert_eq!(doc.attributes.get("manname-id"), None);
        assert_eq!(heading_id(&doc.blocks[1]), Some("_name_2"));
    }

    #[test]
    fn synopsis_sections_are_special_in_manual_pages_only() {
        let src = "= x(1)\n:doctype: manpage\n:sectnums:\n\n== NAME\n\nx - y\n\n\
                   == Synopsis\n\n=== Nested\n\n== Other\n";
        let result = crate::parse(src);
        let start = src.find("=== Nested").unwrap();
        assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
        assert_eq!(
            result.diagnostics[0].message,
            "synopsis sections do not support nested sections"
        );
        assert_eq!(result.diagnostics[0].span, Span::new(start, start + 10));
        let captions = result
            .document
            .flat_blocks()
            .iter()
            .filter_map(|block| block.caption.clone())
            .collect::<Vec<_>>();
        assert_eq!(captions, ["1. "]);
        let result = crate::parse("= x(1)\n\n== Synopsis\n\n=== Nested\n");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        // A nested document is never a manual page.
        let src = "= x(1)\n:doctype: manpage\n\n== NAME\n\nx - y\n\n|===\na|\n== SYNOPSIS\n\n=== S\n|===\n";
        let result = crate::parse(src);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn highlighting_keeps_only_attribute_references_of_the_name() {
        let src = "= x(1)\n:doctype: manpage\n\n== NAME\n\n*x* - {doctype}\n";
        let tokens = crate::tokens(src)
            .into_iter()
            .map(|token| (token.kind, &src[token.span.start..token.span.end]))
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            [
                (crate::TokenKind::Heading, "= x(1)"),
                (crate::TokenKind::AttributeEntry, ":doctype: manpage"),
                (crate::TokenKind::Heading, "== NAME"),
                (crate::TokenKind::AttributeRef, "{doctype}"),
            ]
        );
    }

    #[test]
    fn names_inspect_like_ruby_arrays() {
        assert_eq!(
            inspect(&[
                "a\"1".into(),
                "#{x}".into(),
                "\t\u{1}é".into(),
                String::new()
            ]),
            r#"["a\"1", "\#{x}", "\t\u0001é", ""]"#
        );
    }
}
