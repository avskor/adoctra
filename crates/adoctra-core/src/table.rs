//! Tables, parsed like Ruby's `Parser.parse_table`, `Table::ParserContext`,
//! `Table::Cell` and `Table::Column`: psv cells with their specs (spans,
//! duplication, alignments and styles), csv/dsv/tsv data, the implicit header
//! row, the footer row and the column widths.
//!
//! Cell text is kept as characters with their reader offsets, so stripping,
//! unescaping and CSV unquoting keep the physical source of every character.
use crate::ast::{CellStyle, HAlign, Span, VAlign};
use crate::diagnostic::{Diagnostic, Severity};
use crate::ruby_strip::RubyStrip;
use std::collections::VecDeque;

/// Characters with their reader offsets.
pub(crate) type Chars = Vec<(usize, char)>;

/// Ruby's `DELIMITED_BLOCKS` table entries (`|===`, `,===`, `:===`, `!===`)
/// with a longer run of `=` allowed: the format hint character.
pub(crate) fn delimiter(line: &str) -> Option<char> {
    let mut chars = line.chars();
    let hint = chars
        .next()
        .filter(|c| matches!(c, '|' | ',' | ':' | '!'))?;
    let rest = chars.as_str();
    (rest.len() >= 3 && rest.bytes().all(|b| b == b'=')).then_some(hint)
}

/// The fragments of `chars` with contiguous offsets.
pub(crate) fn fragments(chars: &[(usize, char)]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for &(off, c) in chars {
        match out.last_mut() {
            Some((start, text)) if *start + text.len() == off => text.push(c),
            _ => out.push((off, c.to_string())),
        }
    }
    out
}

/// The characters as source text mapped to their offsets.
pub(crate) fn source(chars: &[(usize, char)]) -> crate::scanner::FilteredSource {
    let fragments = fragments(chars);
    crate::scanner::FilteredSource::from_fragments(
        fragments.iter().map(|(off, text)| (*off, text.as_str())),
    )
}

pub(crate) fn text(chars: &[(usize, char)]) -> String {
    chars.iter().map(|(_, c)| c).collect()
}

fn strip_start(chars: &mut Chars) {
    let n = chars.iter().take_while(|(_, c)| is_space(*c)).count();
    chars.drain(..n);
}

fn strip_end(chars: &mut Chars) {
    while chars.last().is_some_and(|(_, c)| is_space(*c)) {
        chars.pop();
    }
}

/// Ruby's `String#strip` whitespace (and NUL).
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\0')
}

/// Ruby's `String#to_i`: optional sign and leading digits, else 0.
pub(crate) fn to_i(value: &str) -> i64 {
    let value = value.ruby_lstrip();
    let (sign, digits) = match value.as_bytes().first() {
        Some(b'-') => (-1, &value[1..]),
        Some(b'+') => (1, &value[1..]),
        _ => (1, value),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    match digits[..end].parse::<i64>() {
        Ok(n) => sign * n,
        Err(_) if end > 0 => sign * i64::MAX,
        Err(_) => 0,
    }
}

/// Ruby's `tablepcwidth` from the `width` attribute.
pub(crate) fn table_width(width: Option<&str>) -> u32 {
    let Some(width) = width else {
        return 100;
    };
    match to_i(width) {
        0 if width == "0" || width == "0%" => 0,
        n @ 1..=100 => n as u32,
        _ => 100,
    }
}

/// A Ruby number: Integer or Float, printed as Ruby prints it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn value(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    fn add(self, other: Num) -> Num {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Num::Int(a + b),
            (a, b) => Num::Float(a.value() + b.value()),
        }
    }

    fn sub(self, other: Num) -> Num {
        match (self, other) {
            (Num::Int(a), Num::Int(b)) => Num::Int(a - b),
            (a, b) => Num::Float(a.value() - b.value()),
        }
    }

    /// `x.to_i == x ? x.to_i : x` after a truncation.
    fn integral(value: f64) -> Num {
        if value.trunc() == value && value.abs() < 1e15 {
            Num::Int(value as i64)
        } else {
            Num::Float(value)
        }
    }

    /// Ruby's `Integer#round(4)` (identity) and `Float#round(4)`
    /// (`round_half_up`).
    fn round4(self) -> Num {
        match self {
            Num::Int(_) => self,
            Num::Float(x) => {
                let s = 10_000.0;
                let mut f = (x * s).round();
                if x > 0.0 && (f + 0.5) / s <= x {
                    f += 1.0;
                } else if x < 0.0 && (f - 0.5) / s >= x {
                    f -= 1.0;
                }
                Num::Float(f / s)
            }
        }
    }

    fn spelled(self) -> String {
        match self {
            Num::Int(i) => i.to_string(),
            Num::Float(f) if f.is_finite() && f.trunc() == f => format!("{f:.1}"),
            Num::Float(f) => f.to_string(),
        }
    }
}

/// Ruby's `Float#truncate(4)` for the widths (`rb_float_floor` on positive
/// values; widths are never negative).
fn truncate4(x: f64) -> f64 {
    if x < 0.0 {
        return -truncate4(-x);
    }
    let f = 10_000.0;
    let mul = (x * f).floor();
    let res = (mul + 1.0) / f;
    if res > x {
        mul / f
    } else {
        res
    }
}

/// A column spec (Ruby's colspec Hash).
#[derive(Debug, Clone, Default)]
struct ColSpec {
    width: i64,
    halign: Option<HAlign>,
    valign: Option<VAlign>,
    style: Option<CellStyle>,
}

#[derive(Debug, Clone)]
pub(crate) struct Column {
    pub halign: HAlign,
    pub valign: VAlign,
    pub style: Option<CellStyle>,
    width: Num,
    pub autowidth: bool,
    pcwidth: Num,
}

impl Column {
    fn new(spec: ColSpec) -> Self {
        Column {
            halign: spec.halign.unwrap_or(HAlign::Left),
            valign: spec.valign.unwrap_or(VAlign::Top),
            style: spec.style,
            width: Num::Int(spec.width),
            autowidth: false,
            pcwidth: Num::Int(0),
        }
    }

    fn implied() -> Self {
        Column::new(ColSpec {
            width: 1,
            ..ColSpec::default()
        })
    }

    pub fn width(&self) -> String {
        self.pcwidth.spelled()
    }

    fn assign_width(&mut self, pcwidth: Option<Num>, width_base: Option<Num>) -> Num {
        let pcwidth = match width_base {
            Some(base) => Num::integral(truncate4(self.width.value() * 100.0 / base.value())),
            None => pcwidth.unwrap_or(Num::Int(0)),
        };
        self.pcwidth = pcwidth;
        pcwidth
    }
}

fn halign(c: char) -> Option<HAlign> {
    match c {
        '<' => Some(HAlign::Left),
        '>' => Some(HAlign::Right),
        '^' => Some(HAlign::Center),
        _ => None,
    }
}

fn valign(c: char) -> Option<VAlign> {
    match c {
        '<' => Some(VAlign::Top),
        '>' => Some(VAlign::Bottom),
        '^' => Some(VAlign::Middle),
        _ => None,
    }
}

/// The alignment group shared by the column and cell specs:
/// `[<^>](?:\.[<^>]?)?|(?:[<^>]?\.)?[<^>]`, matched at the start of `s`.
/// Returns its length (0 when absent).
fn align_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mark = |i: usize| b.get(i).is_some_and(|c| matches!(c, b'<' | b'^' | b'>'));
    if mark(0) {
        if b.get(1) == Some(&b'.') {
            Some(if mark(2) { 3 } else { 2 })
        } else {
            Some(1)
        }
    } else if b.first() == Some(&b'.') {
        mark(1).then_some(2)
    } else {
        Some(0)
    }
}

/// Ruby's `colspec, rowspec = m.split '.'` on an alignment group.
fn alignments(group: &str) -> (Option<HAlign>, Option<VAlign>) {
    let mut parts = group.split('.');
    let h = parts.next().and_then(|p| p.chars().next()).and_then(halign);
    let v = parts.next().and_then(|p| p.chars().next()).and_then(valign);
    (h, v)
}

/// Ruby's `parse_colspecs`.
fn parse_colspecs(records: &str) -> Vec<ColSpec> {
    let records: String = records.chars().filter(|&c| c != ' ').collect();
    if !records.is_empty() && to_i(&records).to_string() == records {
        let count = to_i(&records).clamp(0, 10_000) as usize;
        return vec![
            ColSpec {
                width: 1,
                ..ColSpec::default()
            };
            count
        ];
    }
    if records.is_empty() {
        return Vec::new();
    }
    let separator = if records.contains(',') { ',' } else { ';' };
    let mut specs = Vec::new();
    for record in records.split(separator) {
        if record.is_empty() {
            specs.push(ColSpec {
                width: 1,
                ..ColSpec::default()
            });
            continue;
        }
        let Some((repeat, spec)) = column_spec(record) else {
            continue;
        };
        for _ in 0..repeat {
            specs.push(spec.clone());
        }
    }
    specs
}

/// Ruby's `ColumnSpecRx`:
/// `^(?:(\d+)\*)?(align)?(\d+%?|~)?([a-z])?$`.
fn column_spec(record: &str) -> Option<(usize, ColSpec)> {
    let mut rest = record;
    let mut repeat = 1;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && rest.as_bytes().get(digits) == Some(&b'*') {
        repeat = to_i(&rest[..digits]).clamp(0, 10_000) as usize;
        rest = &rest[digits + 1..];
    }
    let mut spec = ColSpec::default();
    let align = align_len(rest)?;
    (spec.halign, spec.valign) = alignments(&rest[..align]);
    rest = &rest[align..];
    spec.width = if let Some(r) = rest.strip_prefix('~') {
        rest = r;
        -1
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let width = to_i(&rest[..digits]);
            rest = &rest[digits..];
            rest = rest.strip_prefix('%').unwrap_or(rest);
            width
        } else {
            1
        }
    };
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (None, _) => {}
        (Some(c), None) if c.is_ascii_lowercase() => spec.style = CellStyle::from_letter(c),
        _ => return None,
    }
    Some((repeat, spec))
}

/// A cell spec (Ruby's cellspec Hash) and where it starts.
#[derive(Debug, Clone, Default)]
struct CellSpec {
    colspan: Option<u32>,
    rowspan: Option<u32>,
    repeat: Option<u32>,
    halign: Option<HAlign>,
    valign: Option<VAlign>,
    style: Option<CellStyle>,
    start: usize,
}

fn count(value: &str) -> u32 {
    to_i(value).clamp(0, u32::MAX as i64) as u32
}

/// The spec groups of Ruby's `CellSpecStartRx`/`CellSpecEndRx` after the
/// leading whitespace: `(?:(span)([*+]))?(align)?([a-z])?`, matching all of
/// `s`.
fn spec_tail(s: &str, start: usize) -> Option<CellSpec> {
    let mut spec = CellSpec {
        start,
        ..CellSpec::default()
    };
    let mut rest = s;
    if let Some(at) = rest.find(['*', '+']) {
        let number = &rest[..at];
        let numeric = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
        // `\d+(?:\.\d*)?|(?:\d*\.)?\d+`
        let valid = match number.split_once('.') {
            None => !number.is_empty() && numeric(number),
            Some((a, b)) => {
                numeric(a) && numeric(b) && !b.contains('.') && !(a.is_empty() && b.is_empty())
            }
        };
        if !valid {
            return None;
        }
        let (col, row) = number.split_once('.').unwrap_or((number, ""));
        let col = if col.is_empty() { 1 } else { count(col) };
        let row = if row.is_empty() { 1 } else { count(row) };
        if rest.as_bytes()[at] == b'+' {
            spec.colspan = (col != 1).then_some(col);
            spec.rowspan = (row != 1).then_some(row);
        } else {
            spec.repeat = (col != 1).then_some(col);
        }
        rest = &rest[at + 1..];
    }
    let align = align_len(rest)?;
    (spec.halign, spec.valign) = alignments(&rest[..align]);
    rest = &rest[align..];
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (None, _) => {}
        (Some(c), None) if c.is_ascii_lowercase() => spec.style = CellStyle::from_letter(c),
        _ => return None,
    }
    Some(spec)
}

/// Ruby's `parse_cellspec` at the start of a line: `None` when the text
/// before the first delimiter is not a spec (the cell continues).
fn cellspec_start(line: &str, off: usize, delimiter: &str) -> Option<(CellSpec, usize)> {
    let at = line.find(delimiter)?;
    let part = &line[..at];
    let ws = part.len() - part.trim_start_matches([' ', '\t']).len();
    let spec = spec_tail(&part[ws..], off)?;
    Some((spec, at + delimiter.len()))
}

/// Ruby's `parse_cellspec` at the end of a cell (`CellSpecEndRx`, leftmost
/// match): the spec and the length of the remaining text.
fn cellspec_end(text: &str, off: usize) -> (CellSpec, usize) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b' ' | b'\t') {
            let run = bytes[i..]
                .iter()
                .take_while(|b| matches!(b, b' ' | b'\t'))
                .count();
            if let Some(spec) = spec_tail(&text[i + run..], off + i + run) {
                if i + run == text.len() {
                    // Only whitespace: Ruby keeps the rstripped text.
                    return (
                        CellSpec {
                            start: off + text.len(),
                            ..CellSpec::default()
                        },
                        i,
                    );
                }
                return (spec, i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    (
        CellSpec {
            start: off + text.len(),
            ..CellSpec::default()
        },
        text.len(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Psv,
    Csv,
    Dsv,
}

/// Ruby's `has_header_option`: false, true, `:implicit` or nil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Header {
    False,
    True,
    Implicit,
    Nil,
}

/// A created cell (Ruby's `Table::Cell`), with the arguments needed to
/// create it again when an implicit header row turns out to be body.
#[derive(Debug, Clone)]
pub(crate) struct Cell {
    pub text: Chars,
    pub style: Option<CellStyle>,
    pub halign: HAlign,
    pub valign: VAlign,
    pub colspan: Option<u32>,
    pub rowspan: Option<u32>,
    pub literal: bool,
    pub span: Span,
    /// When Ruby creates the cell's side effects (an AsciiDoc cell's nested
    /// document) or catalogs its leading anchor: cells are completed in this
    /// order.
    pub order: usize,
    /// How many diagnostics were reported when Ruby created the cell (or
    /// cataloged its deferred anchor): its own diagnostics come after them.
    pub mark: usize,
    /// A normal psv cell whose leading inline anchor is cataloged.
    pub catalog: bool,
    /// The merged column and spec style (Ruby's `@attributes['style']`).
    merged_style: Option<CellStyle>,
    /// A normal psv cell in the header row catalogs its anchor later.
    deferred_anchor: bool,
    reinitialize: Option<(Option<usize>, Chars, Option<CellSpec>)>,
}

pub(crate) struct Options<'a> {
    pub cols: Option<&'a str>,
    pub format: Option<&'a str>,
    pub separator: Option<&'a str>,
    pub header: bool,
    pub noheader: bool,
    pub footer: bool,
    /// Inside an AsciiDoc cell, psv data uses `!` (Ruby's `!sv`).
    pub nested: bool,
}

pub(crate) struct Parsed {
    pub columns: Vec<Column>,
    pub head: Vec<Vec<Cell>>,
    pub body: Vec<Vec<Cell>>,
    pub foot: Vec<Vec<Cell>>,
    /// Cells Ruby creates and then drops (an incomplete or overlong row):
    /// their side effects still happen, they are not output.
    pub dropped: Vec<Cell>,
    /// Cell separators (with their specs) for highlighting.
    pub separators: Vec<Span>,
}

struct Parser<'a> {
    lines: &'a [(usize, &'a str)],
    pos: usize,
    format: Format,
    delimiter: String,
    colcount: i64,
    buffer: Chars,
    cellspecs: VecDeque<CellSpec>,
    cell_open: bool,
    active_rowspans: VecDeque<u32>,
    column_visits: u32,
    current_row: Vec<Cell>,
    linenum: i64,
    columns: Vec<Column>,
    body: Vec<Vec<Cell>>,
    dropped: Vec<Cell>,
    header: Header,
    /// The next [`Cell::order`].
    order: usize,
    separators: Vec<Span>,
    /// The table's first line, for the missing separator error.
    start: Span,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn read_line(&mut self) -> Option<(usize, &'a str)> {
        let line = self.lines.get(self.pos).copied();
        self.pos += 1;
        line
    }

    fn has_more_lines(&self) -> bool {
        self.pos < self.lines.len()
    }

    /// Ruby's `Reader#skip_blank_lines`: `None` at the end of the input.
    fn skip_blank_lines(&mut self) -> Option<usize> {
        let mut skipped = 0;
        while let Some((_, line)) = self.lines.get(self.pos) {
            if !line.is_empty() {
                return Some(skipped);
            }
            self.pos += 1;
            skipped += 1;
        }
        None
    }

    fn error(&mut self, span: Span, message: &str) {
        self.diagnostics.push(Diagnostic {
            omitted_span: None,
            report: Default::default(),
            span,
            severity: Severity::Error,
            message: message.to_owned(),
        });
    }

    fn push(&mut self, text: &str, off: usize) {
        self.buffer
            .extend(text.char_indices().map(|(i, c)| (off + i, c)));
    }

    fn header_row(&self) -> Option<Header> {
        matches!(self.header, Header::True | Header::Implicit)
            .then_some(self.header)
            .filter(|_| self.body.is_empty())
    }

    /// Ruby's `buffer_has_unclosed_quotes?` for CSV data.
    fn buffer_has_unclosed_quotes(&self, append: Option<&str>) -> bool {
        let mut record = text(&self.buffer);
        if let Some(append) = append {
            record.push_str(append);
        }
        let record = record.trim_matches(is_space);
        if record == "\"" {
            return true;
        }
        if !record.starts_with('"') {
            return false;
        }
        let trailing = record.ends_with('"');
        if (trailing && record.ends_with("\"\"")) || record.starts_with("\"\"") {
            let record = record.replace("\"\"", "");
            record.starts_with('"') && !record.ends_with('"')
        } else {
            !trailing
        }
    }

    fn close_open_cell(&mut self, spec: CellSpec) {
        self.cellspecs.push_back(spec);
        if self.cell_open {
            self.close_cell(true);
        }
        self.linenum += 1;
    }

    fn close_cell(&mut self, eol: bool) {
        let mut text = std::mem::take(&mut self.buffer);
        let spec;
        let repeat;
        if self.format == Format::Psv {
            match self.cellspecs.pop_front() {
                Some(mut found) => {
                    repeat = found.repeat.take().unwrap_or(1);
                    spec = Some(found);
                }
                None => {
                    self.error(
                        self.start,
                        "table missing leading separator; recovering automatically",
                    );
                    spec = Some(CellSpec {
                        start: text.first().map_or(self.start.start, |(off, _)| *off),
                        ..CellSpec::default()
                    });
                    repeat = 1;
                }
            }
        } else {
            strip_start(&mut text);
            strip_end(&mut text);
            spec = None;
            repeat = 1;
            if self.format == Format::Csv && text.iter().any(|(_, c)| *c == '"') {
                let quoted = text.first().is_some_and(|(_, c)| *c == '"')
                    && text.last().is_some_and(|(_, c)| *c == '"');
                if quoted && text.len() == 1 {
                    let span = Span::new(text[0].0, text[0].0 + 1);
                    self.error(span, "unclosed quote in CSV data; setting cell to empty");
                    text.clear();
                } else {
                    if quoted {
                        text = text[1..text.len() - 1].to_vec();
                        strip_start(&mut text);
                        strip_end(&mut text);
                    }
                    text.dedup_by(|b, a| a.1 == '"' && b.1 == '"');
                }
            }
        }
        for i in 1..=repeat {
            let column = if self.colcount == -1 {
                self.columns.push(Column::implied());
                let index = self.columns.len() - 1;
                let extra = spec
                    .as_ref()
                    .and_then(|s| s.colspan)
                    .map_or(0, |c| c.saturating_sub(1));
                for _ in 0..extra {
                    self.columns.push(Column::implied());
                }
                Some(index)
            } else {
                Some(self.current_row.len()).filter(|&i| i < self.columns.len())
            };
            let header_row = self.header_row();
            let cell = self.new_cell(column, text.clone(), spec.clone(), header_row);
            if let Some(rowspan) = cell.rowspan.filter(|&r| r != 1) {
                let colspan = cell.colspan.unwrap_or(1);
                for j in 1..rowspan as usize {
                    while self.active_rowspans.len() <= j {
                        self.active_rowspans.push_back(0);
                    }
                    self.active_rowspans[j] += colspan;
                }
            }
            self.column_visits += cell.colspan.unwrap_or(1);
            let span = cell.span;
            self.current_row.push(cell);
            let status = self.end_of_row();
            if status > -1 && (self.colcount != -1 || self.linenum > 0 || (eol && i == repeat)) {
                if status > 0 {
                    self.error(
                        span,
                        "dropping cell because it exceeds specified number of columns",
                    );
                    self.close_row(true);
                } else {
                    self.close_row(false);
                }
            }
        }
        self.cell_open = false;
    }

    fn end_of_row(&self) -> i64 {
        if self.colcount == -1 {
            return 0;
        }
        let visits = i64::from(self.column_visits) + i64::from(self.active_rowspans[0]);
        (visits - self.colcount).signum()
    }

    fn close_row(&mut self, drop: bool) {
        let row = std::mem::take(&mut self.current_row);
        if drop {
            self.dropped.extend(row);
        } else {
            self.body.push(row);
        }
        if self.colcount == -1 {
            self.colcount = i64::from(self.column_visits);
        }
        self.column_visits = 0;
        self.active_rowspans.pop_front();
        if self.active_rowspans.is_empty() {
            self.active_rowspans.push_back(0);
        }
    }

    /// Ruby's `Table::Cell#initialize`.
    fn new_cell(
        &mut self,
        column: Option<usize>,
        mut text: Chars,
        spec: Option<CellSpec>,
        header_row: Option<Header>,
    ) -> Cell {
        let col = column.map(|i| &self.columns[i]);
        // The span covers the text too: without a leading separator Ruby
        // takes the next separator's spec, which follows the text.
        let first = text.first().map(|(off, _)| *off);
        let start = match (spec.as_ref().map(|s| s.start), first) {
            (Some(spec), Some(first)) => spec.min(first),
            (spec, first) => spec.or(first).unwrap_or(self.start.start),
        };
        let mut reinitialize = None;
        let mut cell_style = None;
        if col.is_some() {
            match header_row {
                Some(Header::Implicit) => {
                    let style = col
                        .and_then(|c| c.style)
                        .or(spec.as_ref().and_then(|s| s.style));
                    if matches!(style, Some(CellStyle::Asciidoc | CellStyle::Literal)) {
                        reinitialize = Some((column, text.clone(), spec.clone()));
                    }
                }
                Some(_) => {}
                None => cell_style = col.and_then(|c| c.style),
            }
        }
        let mut halign = col.map_or(HAlign::Left, |c| c.halign);
        let mut valign = col.map_or(VAlign::Top, |c| c.valign);
        let mut merged_style = col.and_then(|c| c.style);
        let (mut colspan, mut rowspan) = (None, None);
        let mut literal = false;
        let mut normal = false;
        if let Some(spec) = &spec {
            colspan = spec.colspan;
            rowspan = spec.rowspan;
            if header_row.is_none() {
                cell_style = spec.style.or(cell_style);
            }
            halign = spec.halign.unwrap_or(halign);
            valign = spec.valign.unwrap_or(valign);
            merged_style = spec.style.or(merged_style);
            match cell_style {
                Some(CellStyle::Asciidoc) => {
                    strip_end(&mut text);
                    if text.first().is_some_and(|(_, c)| *c == '\n') {
                        let n = text.iter().take_while(|(_, c)| *c == '\n').count();
                        text.drain(..n);
                    } else {
                        strip_start(&mut text);
                    }
                }
                Some(CellStyle::Literal) => {
                    literal = true;
                    strip_end(&mut text);
                    let n = text.iter().take_while(|(_, c)| *c == '\n').count();
                    text.drain(..n);
                }
                _ => {
                    normal = true;
                    strip_start(&mut text);
                    strip_end(&mut text);
                }
            }
        }
        let deferred_anchor = normal && header_row.is_some();
        let order = self.order;
        self.order += 1;
        let end = text
            .last()
            .map_or(start, |(off, c)| off + c.len_utf8())
            .max(start);
        Cell {
            text,
            style: cell_style,
            halign,
            valign,
            colspan,
            rowspan,
            literal,
            span: Span::new(start, end),
            order,
            mark: self.diagnostics.len(),
            catalog: normal && header_row.is_none(),
            merged_style,
            deferred_anchor,
            reinitialize,
        }
    }

    /// Ruby's `Cell#reinitialize`.
    fn reinitialize(&mut self, mut cell: Cell, has_header: bool) -> Cell {
        if has_header {
            cell.reinitialize = None;
        } else if let Some((column, text, spec)) = cell.reinitialize.take() {
            let span = cell.span;
            let mut cell = self.new_cell(column, text, spec, None);
            cell.span = span;
            return cell;
        } else {
            cell.style = cell.merged_style;
        }
        if std::mem::take(&mut cell.deferred_anchor) {
            cell.catalog = true;
            cell.order = self.order;
            cell.mark = self.diagnostics.len();
            self.order += 1;
        }
        cell
    }
}

/// Ruby's `Parser.parse_table` over the table's lines (without line
/// comments). `hint` is the delimiter's first character.
pub(crate) fn parse(
    lines: &[(usize, &str)],
    hint: char,
    options: &Options,
    start: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Parsed {
    let mut columns = Vec::new();
    let mut explicit = false;
    if let Some(cols) = options.cols {
        let specs = parse_colspecs(cols);
        if !specs.is_empty() {
            columns = create_columns(specs, start, diagnostics);
            explicit = true;
        }
    }
    let format = match options.format {
        Some(format) => format,
        None if matches!(hint, ',') => "csv",
        None if matches!(hint, ':') => "dsv",
        None => "psv",
    };
    let (format, xsv) = match format {
        "psv" if options.nested => (Format::Psv, "!sv"),
        "psv" => (Format::Psv, "psv"),
        "csv" => (Format::Csv, "csv"),
        "dsv" => (Format::Dsv, "dsv"),
        "tsv" => (Format::Csv, "tsv"),
        other => {
            diagnostics.push(Diagnostic {
                omitted_span: None,
                report: Default::default(),
                span: start,
                severity: Severity::Error,
                message: format!("illegal table format: {other}"),
            });
            (Format::Psv, if options.nested { "!sv" } else { "psv" })
        }
    };
    let default = match xsv {
        "csv" => ",",
        "dsv" => ":",
        "tsv" => "\t",
        "!sv" => "!",
        _ => "|",
    };
    let delimiter = match options.separator {
        None | Some("") => default,
        Some("\\t") => "\t",
        Some(separator) => separator,
    };
    let first = lines
        .first()
        .map_or(start, |(off, l)| Span::new(*off, off + l.len()));
    let mut parser = Parser {
        lines,
        pos: 0,
        format,
        delimiter: delimiter.to_owned(),
        colcount: if columns.is_empty() {
            -1
        } else {
            columns.len() as i64
        },
        buffer: Vec::new(),
        cellspecs: VecDeque::new(),
        cell_open: false,
        active_rowspans: VecDeque::from([0]),
        column_visits: 0,
        current_row: Vec::new(),
        linenum: -1,
        columns,
        body: Vec::new(),
        dropped: Vec::new(),
        header: Header::False,
        order: 0,
        separators: Vec::new(),
        start: first,
        diagnostics,
    };
    let skipped = parser.skip_blank_lines().unwrap_or(0);
    let mut implicit_header = false;
    if options.header {
        parser.header = Header::True;
    } else if skipped == 0 && !options.noheader {
        parser.header = Header::Implicit;
        implicit_header = true;
    }
    let mut loop_idx: i64 = -1;
    let mut boundary: Option<i64> = None;
    let delimiter = parser.delimiter.clone();
    while let Some((off, raw)) = parser.read_line() {
        let line_end = off + raw.len();
        loop_idx += 1;
        let beyond_first = loop_idx > 0;
        // The rest of the line still to be read, at its offset.
        let mut line: Option<(usize, &str)> = Some((off, raw));
        if beyond_first && raw.is_empty() {
            line = None;
            boundary = boundary.map(|b| b + 1);
        } else if format == Format::Psv {
            if raw.starts_with(delimiter.as_str()) {
                // Ruby slices one character, whatever the separator's length.
                let skip = raw.chars().next().map_or(0, char::len_utf8);
                parser.separators.push(Span::new(off, off + skip));
                parser.close_open_cell(CellSpec {
                    start: off,
                    ..CellSpec::default()
                });
                line = Some((off + skip, &raw[skip..]));
                boundary = None;
            } else if let Some((spec, consumed)) = cellspec_start(raw, off, &delimiter) {
                parser.separators.push(Span::new(off, off + consumed));
                parser.close_open_cell(spec);
                line = Some((off + consumed, &raw[consumed..]));
                boundary = None;
            } else if boundary == Some(loop_idx) {
                parser.header = Header::Nil;
                implicit_header = false;
                boundary = None;
            }
        }
        if !beyond_first && implicit_header {
            if parser
                .lines
                .get(parser.pos)
                .is_some_and(|(_, next)| next.is_empty())
            {
                boundary = Some(1);
            } else {
                parser.header = Header::Nil;
                implicit_header = false;
            }
        }
        loop {
            let found = line.and_then(|(o, l)| l.find(delimiter.as_str()).map(|at| (o, l, at)));
            let Some((o, l, at)) = found else {
                // No other delimiters on this line: append it to the buffer.
                if let Some((o, l)) = line {
                    parser.push(l, o);
                }
                parser.buffer.push((line_end, '\n'));
                match format {
                    Format::Csv => {
                        if parser.buffer_has_unclosed_quotes(None) {
                            if boundary.is_some() && loop_idx == 0 {
                                parser.header = Header::Nil;
                                implicit_header = false;
                                boundary = None;
                            }
                            parser.cell_open = true;
                        } else {
                            parser.close_cell(true);
                        }
                    }
                    Format::Dsv => parser.close_cell(true),
                    Format::Psv => parser.cell_open = true,
                }
                break;
            };
            let (pre, post) = (&l[..at], &l[at + delimiter.len()..]);
            let post_off = o + at + delimiter.len();
            match format {
                Format::Csv => {
                    if parser.buffer_has_unclosed_quotes(Some(pre)) {
                        parser.push(pre, o);
                        parser.push(&delimiter, o + at);
                        if post.is_empty() {
                            break;
                        }
                        line = Some((post_off, post));
                        continue;
                    }
                    parser.push(pre, o);
                    parser.separators.push(Span::new(o + at, post_off));
                }
                Format::Dsv | Format::Psv if pre.ends_with('\\') => {
                    parser.push(&pre[..pre.len() - 1], o);
                    parser.push(&delimiter, o + at);
                    if post.is_empty() {
                        parser.buffer.push((line_end, '\n'));
                        parser.cell_open = true;
                        break;
                    }
                    line = Some((post_off, post));
                    continue;
                }
                Format::Dsv => {
                    parser.push(pre, o);
                    parser.separators.push(Span::new(o + at, post_off));
                }
                Format::Psv => {
                    let (spec, kept) = cellspec_end(pre, o);
                    parser
                        .separators
                        .push(Span::new(spec.start.min(o + at), post_off));
                    parser.cellspecs.push_back(spec);
                    parser.push(&pre[..kept], o);
                }
            }
            // An empty rest still closes the cell found at the end of the line.
            line = (!post.is_empty()).then_some((post_off, post));
            parser.close_cell(false);
        }
        if parser.cell_open {
            if !parser.has_more_lines() {
                parser.close_cell(true);
            }
        } else if parser.skip_blank_lines().is_none() {
            break;
        }
    }
    if parser.column_visits != 0 {
        let span = parser
            .lines
            .last()
            .map_or(start, |(off, l)| Span::new(*off, off + l.len()));
        parser.error(
            span,
            "dropping cells from incomplete row detected end of table",
        );
        let row = std::mem::take(&mut parser.current_row);
        parser.dropped.extend(row);
    }
    if !parser.columns.is_empty() && !explicit {
        assign_widths(&mut parser.columns, None, &[], start, parser.diagnostics);
    }
    if implicit_header {
        parser.header = Header::True;
    }
    // Ruby's `partition_header_footer`.
    let mut body = std::mem::take(&mut parser.body);
    let mut head = Vec::new();
    let mut foot = Vec::new();
    let mut rows = body.len();
    if rows > 0 {
        match parser.header {
            Header::True | Header::Implicit => {
                let row = body.remove(0);
                head.push(
                    row.into_iter()
                        .map(|cell| parser.reinitialize(cell, true))
                        .collect(),
                );
                rows -= 1;
            }
            Header::Nil => {
                parser.header = Header::False;
                let row = body.remove(0);
                let row = row
                    .into_iter()
                    .map(|cell| parser.reinitialize(cell, false))
                    .collect();
                body.insert(0, row);
            }
            Header::False => {}
        }
    }
    if rows > 0 && options.footer {
        foot.extend(body.pop());
    }
    Parsed {
        columns: parser.columns,
        head,
        body,
        foot,
        dropped: parser.dropped,
        separators: parser.separators,
    }
}

/// Ruby's `Table#create_columns`.
fn create_columns(
    specs: Vec<ColSpec>,
    start: Span,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Column> {
    let mut columns = Vec::new();
    let mut autowidth = Vec::new();
    let mut base = 0i64;
    for spec in specs {
        let width = spec.width;
        columns.push(Column::new(spec));
        if width < 0 {
            autowidth.push(columns.len() - 1);
        } else {
            base += width;
        }
    }
    if !columns.is_empty() {
        let base = (base > 0 || !autowidth.is_empty()).then_some(base);
        assign_widths(&mut columns, base, &autowidth, start, diagnostics);
    }
    columns
}

/// Ruby's `Table#assign_column_widths`: percentages truncated to four
/// decimals, the balance donated to the last column.
fn assign_widths(
    columns: &mut [Column],
    base: Option<i64>,
    autowidth: &[usize],
    start: Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut total = Num::Int(0);
    let mut last = Num::Int(0);
    if let Some(mut base) = base.map(Num::Int) {
        if !autowidth.is_empty() {
            let width = if base.value() > 100.0 {
                diagnostics.push(Diagnostic {
                    omitted_span: None,
                    report: Default::default(),
                    span: start,
                    severity: Severity::Warning,
                    message: format!(
                        "total column width must not exceed 100% when using autowidth columns; got {}%",
                        base.spelled()
                    ),
                });
                Num::Int(0)
            } else {
                let width =
                    Num::integral(truncate4((100.0 - base.value()) / autowidth.len() as f64));
                base = Num::Int(100);
                width
            };
            for &i in autowidth {
                columns[i].width = width;
                columns[i].autowidth = true;
            }
        }
        for column in columns.iter_mut() {
            last = column.assign_width(None, Some(base));
            total = total.add(last);
        }
    } else {
        last = Num::integral(truncate4(100.0 / columns.len() as f64));
        for column in columns.iter_mut() {
            total = total.add(column.assign_width(Some(last), None));
        }
    }
    if total.value() != 100.0 {
        let balance = Num::Int(100).sub(total).add(last).round4();
        if let Some(column) = columns.last_mut() {
            column.assign_width(Some(balance), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(cols: &str) -> Vec<String> {
        parse_colspecs_widths(cols)
    }

    fn parse_colspecs_widths(cols: &str) -> Vec<String> {
        let columns = create_columns(parse_colspecs(cols), Span::new(0, 0), &mut Vec::new());
        columns.iter().map(Column::width).collect()
    }

    #[test]
    fn delimiters_allow_longer_runs() {
        assert_eq!(delimiter("|==="), Some('|'));
        assert_eq!(delimiter(",====="), Some(','));
        assert_eq!(delimiter(":==="), Some(':'));
        assert_eq!(delimiter("!==="), Some('!'));
        assert_eq!(delimiter("|=="), None);
        assert_eq!(delimiter("|===x"), None);
        assert_eq!(delimiter(" |==="), None);
    }

    #[test]
    fn column_widths_are_ruby_percentages() {
        assert_eq!(widths("1,2,3"), ["16.6666", "33.3333", "50.0001"]);
        assert_eq!(widths("1,1"), ["50", "50"]);
        assert_eq!(widths("3"), ["33.3333", "33.3333", "33.3334"]);
        assert_eq!(widths("2*,~"), ["1", "1", "98"]);
        assert_eq!(widths("30,~"), ["30", "70"]);
        assert_eq!(widths("~,~"), ["50", "50"]);
        assert_eq!(widths("25%,75%"), ["25", "75"]);
        assert_eq!(widths("0,0"), ["50", "50"]);
    }

    #[test]
    fn autowidth_over_100_warns() {
        let mut diagnostics = Vec::new();
        let columns = create_columns(parse_colspecs("60,60,~"), Span::new(0, 0), &mut diagnostics);
        let widths: Vec<_> = columns.iter().map(Column::width).collect();
        assert_eq!(widths, ["50", "50", "0"]);
        assert_eq!(
            diagnostics[0].message,
            "total column width must not exceed 100% when using autowidth columns; got 120%"
        );
    }

    #[test]
    fn column_specs_follow_column_spec_rx() {
        let specs = parse_colspecs("2*^.>3e, <.^m,x9,.<");
        assert_eq!(specs.len(), 4, "the invalid record is skipped");
        assert_eq!(specs[0].halign, Some(HAlign::Center));
        assert_eq!(specs[0].valign, Some(VAlign::Bottom));
        assert_eq!(specs[0].width, 3);
        assert_eq!(specs[0].style, Some(CellStyle::Emphasis));
        assert_eq!(specs[2].style, Some(CellStyle::Monospaced));
        assert_eq!(specs[2].valign, Some(VAlign::Middle));
        assert_eq!(specs[3].halign, None);
        assert_eq!(specs[3].valign, Some(VAlign::Top));
        assert!(parse_colspecs("").is_empty());
        assert!(parse_colspecs("0").is_empty());
    }

    #[test]
    fn cell_specs_follow_cell_spec_rx() {
        let spec = spec_tail("2.3+^.>m", 0).unwrap();
        assert_eq!((spec.colspan, spec.rowspan), (Some(2), Some(3)));
        assert_eq!(spec.halign, Some(HAlign::Center));
        assert_eq!(spec.style, Some(CellStyle::Monospaced));
        assert_eq!(spec_tail(".2+", 0).unwrap().rowspan, Some(2));
        assert_eq!(spec_tail("3*", 0).unwrap().repeat, Some(3));
        assert!(spec_tail("x y", 0).is_none());
        assert!(spec_tail(".+", 0).is_none());
        // The leftmost whitespace run that ends in a spec ends the text.
        let (spec, kept) = cellspec_end("a b c 2+", 0);
        assert_eq!((kept, spec.colspan), (5, Some(2)));
        let (spec, kept) = cellspec_end("a b", 0);
        assert_eq!((kept, spec.style), (1, None), "Ruby drops ` b`");
        let (spec, kept) = cellspec_end("text 2+", 0);
        assert_eq!((kept, spec.colspan, spec.start), (4, Some(2), 5));
        assert_eq!(cellspec_end("text  ", 0).1, 4);
    }

    #[test]
    fn widths_print_like_ruby_floats() {
        assert_eq!(Num::Float(50.0).spelled(), "50.0");
        assert_eq!(Num::Float(33.3334).spelled(), "33.3334");
        assert_eq!(truncate4(100.0 / 6.0), 16.6666);
        assert_eq!(
            Num::Float(100.0 - 99.9999 + 33.3333).round4(),
            Num::Float(33.3334)
        );
    }

    #[test]
    fn table_width_clamps_like_ruby() {
        assert_eq!(table_width(None), 100);
        assert_eq!(table_width(Some("50%")), 50);
        assert_eq!(table_width(Some("0%")), 0);
        assert_eq!(table_width(Some("0px")), 100);
        assert_eq!(table_width(Some("150")), 100);
        assert_eq!(table_width(Some("x")), 100);
    }
}
