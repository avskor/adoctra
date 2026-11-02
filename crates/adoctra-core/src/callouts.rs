//! Callouts: Ruby's `CalloutSourceRx` marks at the ends of verbatim lines
//! (`sub_callouts`), the document's `Callouts` catalog and callout list items
//! (`CalloutListRx`).

/// A callout mark found on a line: its byte range (from any guard through
/// the mark), raw number (`1` or `.`), guard and form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark {
    pub start: usize,
    pub end: usize,
    pub number: String,
    pub guard: Option<String>,
    pub xml: bool,
    /// An escaped mark (`\<1>`) only loses its backslash at `start`.
    pub escaped: bool,
}

/// The spelling of a mark's angle brackets: the source form (`<1>`,
/// Ruby's `CalloutScanRx`) or the form after special characters
/// (`&lt;1&gt;`, Ruby's `CalloutSourceRx`).
#[derive(Clone, Copy)]
pub(crate) struct Form {
    lt: &'static str,
    gt: &'static str,
}

pub(crate) const SOURCE: Form = Form { lt: "<", gt: ">" };
pub(crate) const LEXICAL: Form = Form {
    lt: "&lt;",
    gt: "&gt;",
};

/// A mark at `at` (its `<`): `<!?(|--)(\d+|\.)\3>` by default, `<(\d+|\.)>`
/// with a `line-comment`. Returns its end, number and whether it is the
/// `<!--N-->` form (`dashes`).
fn mark_at(line: &str, at: usize, custom: bool, form: Form) -> Option<(usize, String, bool)> {
    if !line[at..].starts_with(form.lt) {
        return None;
    }
    let bytes = line.as_bytes();
    let mut p = at + form.lt.len();
    let mut dashes = false;
    if !custom {
        if bytes.get(p) == Some(&b'!') {
            p += 1;
        }
        if line[p..].starts_with("--") {
            dashes = true;
            p += 2;
        }
    }
    let digits = bytes[p..].iter().take_while(|b| b.is_ascii_digit()).count();
    let len = if digits > 0 {
        digits
    } else if bytes.get(p) == Some(&b'.') {
        1
    } else {
        return None;
    };
    let number = line[p..p + len].to_owned();
    p += len;
    if dashes {
        if !line[p..].starts_with("--") {
            return None;
        }
        p += 2;
    }
    line[p..]
        .starts_with(form.gt)
        .then_some((p + form.gt.len(), number, dashes))
}

/// Whether only further marks (each after an optional space and backslash)
/// follow `at` to the end of the line, with the same `<!--` form.
fn chain_to_end(line: &str, mut at: usize, custom: bool, dashes: bool, form: Form) -> bool {
    loop {
        if at == line.len() {
            return true;
        }
        let mut p = at;
        if line.as_bytes()[p] == b' ' {
            p += 1;
        }
        if line.as_bytes().get(p) == Some(&b'\\') {
            p += 1;
        }
        match mark_at(line, p, custom, form) {
            Some((end, _, d)) if d == dashes => at = end,
            _ => return false,
        }
    }
}

/// Ruby's `sub_callouts` on one line: the trailing marks with their guards.
/// `line_comment` is the `line-comment` attribute. Its empty value makes the
/// guard group of `CalloutSourceRxMap` empty, so every mark has the guard
/// `""` (not `nil`).
pub(crate) fn scan_line(line: &str, line_comment: Option<&str>) -> Vec<Mark> {
    scan_line_in(line, line_comment, SOURCE)
}

pub(crate) fn scan_line_in(line: &str, line_comment: Option<&str>, form: Form) -> Vec<Mark> {
    let custom = line_comment.is_some();
    let mut marks = Vec::new();
    let mut at = 0;
    while let Some(found) = line[at..].find(form.lt) {
        let p = at + found;
        if let Some((end, number, dashes)) = mark_at(line, p, custom, form) {
            if chain_to_end(line, end, custom, dashes, form) {
                let escaped = p > 0 && line.as_bytes()[p - 1] == b'\\';
                let mark_start = p - usize::from(escaped);
                let before = &line[..mark_start];
                let guard = match line_comment {
                    Some("") => Some(String::new()),
                    Some(comment) => [format!("{comment} "), comment.to_owned()]
                        .into_iter()
                        .find(|g| before.ends_with(g.as_str())),
                    None => ["// ", "//", "# ", "#", "-- ", "--", ";; ", ";;"]
                        .into_iter()
                        .find(|g| before.ends_with(g))
                        .map(str::to_owned),
                }
                // A later mark of the chain never takes a guard.
                .filter(|_| marks.is_empty());
                let start = mark_start - guard.as_ref().map_or(0, String::len);
                marks.push(Mark {
                    start,
                    end,
                    number,
                    guard,
                    xml: dashes,
                    escaped,
                });
                // The rest of the line is the chain.
                let mut q = end;
                while q < line.len() {
                    let mut p = q;
                    if line.as_bytes()[p] == b' ' {
                        p += 1;
                    }
                    let escaped = line.as_bytes().get(p) == Some(&b'\\');
                    let mark = p + usize::from(escaped);
                    let (end, number, dashes) =
                        mark_at(line, mark, custom, form).expect("chain mark");
                    marks.push(Mark {
                        start: p,
                        end,
                        number,
                        // `()?` of an empty `line-comment` matches every mark.
                        guard: (line_comment == Some("")).then(String::new),
                        xml: dashes,
                        escaped,
                    });
                    q = end;
                }
                break;
            }
        }
        at = p + 1;
    }
    marks
}

/// Ruby's `catalog_callouts`: register the source marks of a block whose
/// substitutions include callouts in the current list (an escaped mark is
/// found but not registered). Returns whether any mark was found, the
/// registered ids in order and the ranges of those marks (with any guard).
pub(crate) fn catalog(
    text: &str,
    catalog: &mut Catalog,
) -> (bool, Vec<String>, Vec<std::ops::Range<usize>>) {
    let (mut found, mut ids, mut ranges, mut autonum) = (false, Vec::new(), Vec::new(), 0u64);
    if !text.contains('<') {
        return (found, ids, ranges);
    }
    for (offset, line) in crate::scanner::lines_with_offsets(text) {
        for mark in scan_line(line, None) {
            found = true;
            if mark.escaped {
                continue;
            }
            let ordinal = if mark.number == "." {
                autonum += 1;
                autonum
            } else {
                mark.number.parse().unwrap_or(u64::MAX)
            };
            ids.push(catalog.register(ordinal));
            ranges.push(offset + mark.start..offset + mark.end);
        }
    }
    (found, ids, ranges)
}

/// The `icons` mode of callout `number` with the attributes at conversion
/// and, when icons are images, its icon (`icon_uri "callouts/N"`, which
/// `image_uri` resolves under `iconsdir`), like Ruby's
/// `convert_inline_callout` and `convert_colist`.
pub(crate) fn icon(attrs: &crate::Attributes, number: &str) -> (Option<String>, Option<String>) {
    let icons = attrs.get("icons").map(str::to_owned);
    let target = match icons.as_deref() {
        Some("font") | None => None,
        Some(_) => Some(crate::image::icon_target(
            None,
            &format!("callouts/{number}"),
            attrs.get("icontype"),
        )),
    };
    (icons, target)
}

/// Ruby's `sub_callouts` over the lexical result of the earlier stages: a
/// mark after special characters (`&lt;1&gt;`) becomes a callout part and an
/// escaped one loses its backslash. Ids are assigned by the caller.
pub(crate) fn apply_parts(
    parts: Vec<crate::AttributePart>,
    line_comment: Option<&str>,
    attrs: &crate::substitution_context::Evaluation<'_>,
    span: crate::Span,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> Vec<crate::AttributePart> {
    use crate::substitution_text::Text;
    let text = Text::from_parts(&parts);
    if !text.text.contains(LEXICAL.lt) {
        return parts;
    }
    let mut out = Text::default();
    let (mut copied, mut autonum) = (0, 0u64);
    for (offset, line) in crate::scanner::lines_with_offsets(&text.text) {
        for mark in scan_line_in(line, line_comment, LEXICAL) {
            let (start, end) = (offset + mark.start, offset + mark.end);
            if mark.escaped {
                let slash = start + mark.guard.as_ref().map_or(0, String::len);
                text.copy(copied..slash, &mut out);
                copied = slash + 1;
                continue;
            }
            text.copy(copied..start, &mut out);
            let number = if mark.number == "." {
                autonum += 1;
                autonum.to_string()
            } else {
                mark.number
            };
            let (icons, target) = icon(attrs, &number);
            let src = target.map_or_else(String::new, |target| {
                attrs.image_uri(&target, attrs.get("iconsdir"), span, diagnostics)
            });
            let callout = crate::InlineCallout {
                number,
                guard: mark.guard,
                xml: mark.xml,
                id: None,
                icons,
                src,
            };
            text.replace_parts(
                start..end,
                &[crate::AttributePart::Callout(Box::new(callout))],
                &mut out,
            );
            copied = end;
        }
    }
    text.copy(copied..text.text.len(), &mut out);
    out.into_parts()
}

/// Ruby's `CalloutListRx`: `^<(\d+|\.)>[ \t]+(.*)$`.
pub(crate) fn list_item(line: &str) -> Option<crate::lists::ItemMatch> {
    let (end, number, _) = mark_at(line, 0, true, SOURCE)?;
    let ws = line[end..]
        .bytes()
        .take_while(|&b| b == b' ' || b == b'\t')
        .count();
    (ws > 0 && end + ws < line.len()).then(|| crate::lists::ItemMatch {
        marker: number,
        marker_range: 0..end,
        text: Some(end + ws..line.len()),
        term: None,
    })
}

/// Ruby's `Document#callouts`: the marks registered per callout list, and
/// the read position of `read_next_id` in the current list.
#[derive(Default)]
pub(crate) struct Catalog {
    lists: Vec<Vec<(u64, String)>>,
    read: usize,
}

impl Catalog {
    fn current(&mut self) -> &mut Vec<(u64, String)> {
        if self.lists.is_empty() {
            self.lists.push(Vec::new());
        }
        self.lists.last_mut().unwrap()
    }

    /// Register a mark for list item `ordinal`; returns its id (`CO1-1`).
    pub fn register(&mut self, ordinal: u64) -> String {
        let list = self.lists.len().max(1);
        let current = self.current();
        let id = format!("CO{list}-{}", current.len() + 1);
        current.push((ordinal, id.clone()));
        id
    }

    /// The marks of the current list explaining item `ordinal`.
    pub fn ids(&mut self, ordinal: u64) -> Vec<String> {
        self.current()
            .iter()
            .filter(|(o, _)| *o == ordinal)
            .map(|(_, id)| id.clone())
            .collect()
    }

    /// Ruby's `read_next_id`: the id of the next registered mark of the
    /// current list, in order, for each converted mark. Registration
    /// (`catalog_callouts`) and conversion (`sub_callouts` with any
    /// `line-comment`) match differently, so the ids a block's converted
    /// marks read may belong to other marks; `None` past the list.
    pub fn read_next(&mut self) -> Option<String> {
        let index = self.read;
        self.read += 1;
        self.current().get(index).map(|(_, id)| id.clone())
    }

    pub fn next_list(&mut self) {
        self.current();
        self.lists.push(Vec::new());
        self.read = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(line: &str) -> Vec<(String, Option<String>, bool, &str)> {
        scan_line(line, None)
            .into_iter()
            .map(|m| {
                (
                    m.number.clone(),
                    m.guard.clone(),
                    m.escaped,
                    &line[m.start..m.end],
                )
            })
            .collect()
    }

    #[test]
    fn marks_end_the_line_with_their_guards() {
        assert_eq!(
            marks("puts 2 // <2> <3>"),
            [
                ("2".into(), Some("// ".into()), false, "// <2>"),
                ("3".into(), None, false, "<3>")
            ]
        );
        assert_eq!(marks("x <1> y"), []);
        assert_eq!(marks("a <1> <2>")[0].3, "<1>");
        assert_eq!(marks("y <!--5-->"), [("5".into(), None, false, "<!--5-->")]);
        assert_eq!(marks("x = \\<4>"), [("4".into(), None, true, "\\<4>")]);
        assert_eq!(marks("z <.>")[0].0, ".");
        assert!(marks("<1-->").is_empty());
        let custom = scan_line("x %% <1>", Some("%%"));
        assert_eq!(custom[0].guard.as_deref(), Some("%% "));
        assert!(scan_line("y <!--5-->", Some("%%")).is_empty());
        // An empty `line-comment` gives every mark of the chain the guard "".
        let empty = scan_line("a // <1> <2>", Some(""));
        assert_eq!(
            empty
                .iter()
                .map(|m| (m.number.as_str(), m.guard.as_deref(), m.start))
                .collect::<Vec<_>>(),
            [("1", Some(""), 5), ("2", Some(""), 9)]
        );
        assert!(scan_line("b <!--3-->", Some("")).is_empty());
    }

    #[test]
    fn converted_marks_read_the_registered_ids_in_order() {
        // Ruby's `read_next_id`: a cursor over the current list, reset by
        // `next_list`, independent of which marks registered the ids.
        let mut catalog = Catalog::default();
        catalog.register(1);
        catalog.register(3);
        assert_eq!(catalog.read_next().as_deref(), Some("CO1-1"));
        assert_eq!(catalog.read_next().as_deref(), Some("CO1-2"));
        assert_eq!(catalog.read_next(), None);
        catalog.next_list();
        catalog.register(1);
        assert_eq!(catalog.read_next().as_deref(), Some("CO2-1"));
    }

    #[test]
    fn catalog_ids_follow_ruby() {
        let mut catalog = Catalog::default();
        assert_eq!(catalog.register(1), "CO1-1");
        assert_eq!(catalog.register(1), "CO1-2");
        assert_eq!(catalog.ids(1), ["CO1-1", "CO1-2"]);
        catalog.next_list();
        assert_eq!(catalog.register(1), "CO2-1");
        assert!(list_item("<1> text").is_some());
        assert!(list_item(" <1> text").is_none());
        assert!(list_item("<1>text").is_none());
    }
}
