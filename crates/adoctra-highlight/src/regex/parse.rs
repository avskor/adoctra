//! Parser for the Onigmo (Ruby) regexp syntax Rouge's lexers use.
//!
//! The pattern becomes a [`Node`] tree with case-insensitivity resolved per
//! node. Onigmo rules that differ from other dialects are kept: an option
//! group without a colon (`a(?i)b|c`) applies to the rest of the enclosing
//! group *including* later alternatives; `x{n,m}+` repeats the interval (it is
//! not possessive); `{,m}` is `{0,m}`; a `{` that does not start a valid
//! interval is a literal; `\w`/`\d`/`\s`/`\h` are ASCII while `\b` uses the
//! Unicode Word property; a one-character class is the literal character.

use super::unicode::{self, Table};
use super::Error;

/// Regexp options, as `Regexp#options` reports them.
pub const IGNORECASE: u32 = 1;
pub const EXTENDED: u32 = 2;
pub const MULTILINE: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Flags {
    pub fold: bool,
    pub extended: bool,
    pub dotall: bool,
}

impl Flags {
    fn of(options: u32) -> Flags {
        Flags {
            fold: options & IGNORECASE != 0,
            extended: options & EXTENDED != 0,
            dotall: options & MULTILINE != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Assertion {
    /// `^`: the subject start or after a LF that is not the last character.
    LineStart,
    /// `$`: the subject end or before a LF.
    LineEnd,
    /// `\A`
    TextStart,
    /// `\z`
    TextEnd,
    /// `\Z`: the subject end or before a final LF.
    TextEndNewline,
    /// `\b`
    WordBoundary,
    /// `\B`
    NotWordBoundary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Look {
    Ahead,
    NotAhead,
    Behind,
    NotBehind,
}

#[derive(Clone, Debug)]
pub(super) enum Node {
    Empty,
    Char {
        c: char,
        fold: bool,
    },
    /// `.` (`dotall`: also a LF).
    Any {
        dotall: bool,
    },
    Class {
        class: Box<Class>,
        fold: bool,
    },
    Assert(Assertion),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    /// A capturing group (1-based index).
    Group {
        index: usize,
        node: Box<Node>,
    },
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    },
    /// `(?>…)`, also a possessive quantifier.
    Atomic(Box<Node>),
    Look {
        kind: Look,
        node: Box<Node>,
    },
    Backref {
        index: usize,
        fold: bool,
    },
    /// `\g<0>`: the whole pattern, called recursively.
    Recurse,
}

/// A bracket expression (or a class escape such as `\w` or `\p{L}`).
#[derive(Clone, Debug, Default)]
pub(super) struct Class {
    pub negated: bool,
    /// Operands of `&&`: a character belongs to the class when it belongs to
    /// every operand (one operand without `&&`).
    pub operands: Vec<Vec<Item>>,
}

#[derive(Clone, Debug)]
pub(super) enum Item {
    Range(char, char),
    Table { table: Table, negated: bool },
    Nested(Class),
}

impl Class {
    fn single(items: Vec<Item>) -> Class {
        Class {
            negated: false,
            operands: vec![items],
        }
    }

    /// Membership without case folding.
    pub fn contains(&self, c: char) -> bool {
        self.inside(c, false) != self.negated
    }

    /// Membership under `/i`, as Onigmo builds a folded class: case variants
    /// of the members are added, but a variant below U+0100 lands in the
    /// single-byte bitmap only when it is ASCII, so a non-ASCII Latin-1 scalar
    /// matches only directly (`/[à-ÿ]/i` does not match `É`). The ASCII
    /// class escapes `\w`, `\d`, `\s` and `\h` contribute no variants.
    pub fn contains_folded(&self, c: char) -> bool {
        let inside = self.inside(c, false)
            || ((c as u32) < 0x80 || (c as u32) >= 0x100)
                && unicode::variants(c).any(|variant| self.inside(variant, true));
        inside != self.negated
    }

    /// Membership before negation; `variant` leaves out the ASCII escapes.
    fn inside(&self, c: char, variant: bool) -> bool {
        self.operands
            .iter()
            .all(|items| items.iter().any(|item| item.contains(c, variant)))
    }
}

impl Item {
    fn contains(&self, c: char, variant: bool) -> bool {
        match self {
            Item::Range(lo, hi) => *lo <= c && c <= *hi,
            Item::Table { table, .. } if variant && table.is_ascii_escape() => false,
            Item::Table { table, negated } => table.contains(c) != *negated,
            Item::Nested(class) => class.inside(c, variant) != class.negated,
        }
    }
}

pub(super) struct Parsed {
    pub node: Node,
    pub groups: usize,
}

pub(super) fn parse(source: &str, options: u32) -> Result<Parsed, Error> {
    let mut parser = Parser {
        chars: source.chars().collect(),
        pos: 0,
        groups: 0,
    };
    let node = parser.alternation(Flags::of(options))?;
    if parser.pos < parser.chars.len() {
        return Err(parser.error("unmatched close parenthesis"));
    }
    Ok(Parsed {
        node,
        groups: parser.groups,
    })
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    groups: usize,
}

impl Parser {
    fn error(&self, message: &str) -> Error {
        Error {
            message: message.to_string(),
            offset: self.pos,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Result<char, Error> {
        let c = self
            .peek()
            .ok_or_else(|| self.error("premature end of regular expression"))?;
        self.pos += 1;
        Ok(c)
    }

    /// Skips whitespace and `#` comments in extended mode.
    fn skip_extended(&mut self, flags: Flags) {
        if !flags.extended {
            return;
        }
        while let Some(c) = self.peek() {
            if c == '#' {
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    if c == '\n' {
                        break;
                    }
                }
            } else if c.is_ascii_whitespace() || c == '\u{b}' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn alternation(&mut self, flags: Flags) -> Result<Node, Error> {
        let mut alternatives = vec![self.concat(flags)?];
        while self.eat('|') {
            alternatives.push(self.concat(flags)?);
        }
        Ok(if alternatives.len() == 1 {
            alternatives.pop().unwrap()
        } else {
            Node::Alt(alternatives)
        })
    }

    fn concat(&mut self, mut flags: Flags) -> Result<Node, Error> {
        let mut items = Vec::new();
        loop {
            self.skip_extended(flags);
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            // An option group without a body applies to the rest of the
            // enclosing group, alternatives included.
            if let Some(next) = self.option_switch(flags)? {
                flags = next;
                items.push(self.alternation(flags)?);
                break;
            }
            let atom = self.atom(flags)?;
            let atom = self.quantifiers(atom, flags)?;
            items.push(atom);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        })
    }

    /// Parses `(?imx-imx)` at the cursor, returning the new flags.
    fn option_switch(&mut self, flags: Flags) -> Result<Option<Flags>, Error> {
        if self.peek() != Some('(') || self.peek_at(1) != Some('?') {
            return Ok(None);
        }
        let start = self.pos;
        self.pos += 2;
        match self.option_letters(flags) {
            Some((next, ')')) => Ok(Some(next)),
            _ => {
                self.pos = start;
                Ok(None)
            }
        }
    }

    /// Reads `imx-imx` and the terminating `)` or `:`.
    fn option_letters(&mut self, mut flags: Flags) -> Option<(Flags, char)> {
        let mut on = true;
        loop {
            let c = self.peek()?;
            self.pos += 1;
            match c {
                'i' => flags.fold = on,
                'm' => flags.dotall = on,
                'x' => flags.extended = on,
                '-' if on => on = false,
                ')' | ':' => return Some((flags, c)),
                _ => return None,
            }
        }
    }

    fn quantifiers(&mut self, mut atom: Node, flags: Flags) -> Result<Node, Error> {
        loop {
            self.skip_extended(flags);
            let (min, max, interval) = match self.peek() {
                Some('*') => {
                    self.pos += 1;
                    (0, None, false)
                }
                Some('+') => {
                    self.pos += 1;
                    (1, None, false)
                }
                Some('?') => {
                    self.pos += 1;
                    (0, Some(1), false)
                }
                Some('{') => match self.interval() {
                    Some((min, max)) => (min, max, true),
                    None => return Ok(atom),
                },
                _ => return Ok(atom),
            };
            let mut greedy = true;
            let mut possessive = false;
            if self.eat('?') {
                greedy = false;
            } else if !interval && self.eat('+') {
                possessive = true;
            }
            atom = Node::Repeat {
                node: Box::new(atom),
                min,
                max,
                greedy,
            };
            if possessive {
                atom = Node::Atomic(Box::new(atom));
            }
        }
    }

    /// Parses `{n}`, `{n,}`, `{,m}` or `{n,m}`; otherwise leaves the cursor.
    fn interval(&mut self) -> Option<(u32, Option<u32>)> {
        let start = self.pos;
        self.pos += 1;
        let low = self.digits();
        let result = if self.eat(',') {
            let high = self.digits();
            if low.is_none() && high.is_none() {
                None
            } else {
                Some((low.unwrap_or(0), high))
            }
        } else {
            low.map(|n| (n, Some(n)))
        };
        match result {
            Some(bounds) if self.eat('}') => Some(bounds),
            _ => {
                self.pos = start;
                None
            }
        }
    }

    fn digits(&mut self) -> Option<u32> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(d);
            self.pos += 1;
        }
        (self.pos > start).then_some(value)
    }

    fn atom(&mut self, flags: Flags) -> Result<Node, Error> {
        let c = self.next()?;
        Ok(match c {
            '(' => self.group(flags)?,
            '[' => self.class_node(flags)?,
            '.' => Node::Any {
                dotall: flags.dotall,
            },
            '^' => Node::Assert(Assertion::LineStart),
            '$' => Node::Assert(Assertion::LineEnd),
            '\\' => self.escape(flags)?,
            _ => Node::Char {
                c,
                fold: flags.fold,
            },
        })
    }

    fn group(&mut self, flags: Flags) -> Result<Node, Error> {
        if !self.eat('?') {
            self.groups += 1;
            let index = self.groups;
            let node = self.alternation(flags)?;
            self.close()?;
            return Ok(Node::Group {
                index,
                node: Box::new(node),
            });
        }
        let kind = match self.peek() {
            Some(':') => {
                self.pos += 1;
                None
            }
            Some('=') => {
                self.pos += 1;
                Some(Look::Ahead)
            }
            Some('!') => {
                self.pos += 1;
                Some(Look::NotAhead)
            }
            Some('>') => {
                self.pos += 1;
                let node = self.alternation(flags)?;
                self.close()?;
                return Ok(Node::Atomic(Box::new(node)));
            }
            Some('#') => {
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    if c == ')' {
                        return Ok(Node::Empty);
                    }
                }
                return Err(self.error("end pattern in group"));
            }
            Some('<') if matches!(self.peek_at(1), Some('=') | Some('!')) => {
                let negated = self.peek_at(1) == Some('!');
                self.pos += 2;
                Some(if negated {
                    Look::NotBehind
                } else {
                    Look::Behind
                })
            }
            _ => {
                // `(?imx-imx:…)`; `(?imx)` was handled by the caller.
                match self.option_letters(flags) {
                    Some((inner, ':')) => {
                        let node = self.alternation(inner)?;
                        self.close()?;
                        return Ok(node);
                    }
                    _ => return Err(self.error("undefined group option")),
                }
            }
        };
        let node = self.alternation(flags)?;
        self.close()?;
        Ok(match kind {
            None => node,
            Some(kind) => Node::Look {
                kind,
                node: Box::new(node),
            },
        })
    }

    fn close(&mut self) -> Result<(), Error> {
        if self.eat(')') {
            Ok(())
        } else {
            Err(self.error("end pattern with unmatched parenthesis"))
        }
    }

    fn escape(&mut self, flags: Flags) -> Result<Node, Error> {
        let c = self.next()?;
        let table = |name: &str| -> Node {
            Node::Class {
                class: Box::new(Class::single(vec![Item::Table {
                    table: unicode::builtin(name),
                    negated: false,
                }])),
                fold: flags.fold,
            }
        };
        let negated_table = |name: &str| -> Node {
            Node::Class {
                class: Box::new(Class::single(vec![Item::Table {
                    table: unicode::builtin(name),
                    negated: true,
                }])),
                fold: flags.fold,
            }
        };
        Ok(match c {
            'A' => Node::Assert(Assertion::TextStart),
            'z' => Node::Assert(Assertion::TextEnd),
            'Z' => Node::Assert(Assertion::TextEndNewline),
            'b' => Node::Assert(Assertion::WordBoundary),
            'B' => Node::Assert(Assertion::NotWordBoundary),
            'w' => table("w"),
            'W' => negated_table("w"),
            'd' => table("d"),
            'D' => negated_table("d"),
            's' => table("s"),
            'S' => negated_table("s"),
            'h' => table("h"),
            'H' => negated_table("h"),
            'p' | 'P' => {
                let (table, negated) = self.property(c == 'P')?;
                Node::Class {
                    class: Box::new(Class::single(vec![Item::Table { table, negated }])),
                    fold: flags.fold,
                }
            }
            'g' => {
                let name = self.bracketed_name()?;
                if name != "0" {
                    return Err(self.error("unsupported subexpression call"));
                }
                Node::Recurse
            }
            'k' => {
                let name = self.bracketed_name()?;
                let index = name.parse().map_err(|_| self.error("invalid backref"))?;
                Node::Backref {
                    index,
                    fold: flags.fold,
                }
            }
            '1'..='9' => {
                // `\1`-`\9` refer to a group; a longer number without such
                // a group is an octal escape (`\103` is `C`).
                self.pos -= 1;
                let start = self.pos;
                let index = self.digits().unwrap_or(0) as usize;
                if index >= 10 && index > self.groups && c <= '7' {
                    self.pos = start;
                    Node::Char {
                        c: self.escaped_char()?,
                        fold: flags.fold,
                    }
                } else {
                    Node::Backref {
                        index,
                        fold: flags.fold,
                    }
                }
            }
            _ => {
                self.pos -= 1;
                Node::Char {
                    c: self.escaped_char()?,
                    fold: flags.fold,
                }
            }
        })
    }

    /// `\p{Name}`, `\p{^Name}` and `\P{…}`.
    fn property(&mut self, mut negated: bool) -> Result<(Table, bool), Error> {
        if !self.eat('{') {
            return Err(self.error("invalid character property name {p}"));
        }
        if self.eat('^') {
            negated = !negated;
        }
        let mut name = String::new();
        while let Some(c) = self.peek() {
            self.pos += 1;
            if c == '}' {
                let table = unicode::property(&name).ok_or_else(|| {
                    self.error(&format!("invalid character property name {{{name}}}"))
                })?;
                return Ok((table, negated));
            }
            name.push(c);
        }
        Err(self.error("invalid character property name"))
    }

    fn bracketed_name(&mut self) -> Result<String, Error> {
        let close = match self.next()? {
            '<' => '>',
            '\'' => '\'',
            _ => return Err(self.error("invalid group name")),
        };
        let mut name = String::new();
        loop {
            let c = self.next()?;
            if c == close {
                return Ok(name);
            }
            name.push(c);
        }
    }

    /// A character escape (the cursor is after the backslash): control,
    /// hexadecimal, Unicode and octal escapes, else the character itself.
    fn escaped_char(&mut self) -> Result<char, Error> {
        let c = self.next()?;
        Ok(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'f' => '\u{c}',
            'v' => '\u{b}',
            'a' => '\u{7}',
            'e' => '\u{1b}',
            'x' => {
                if self.eat('{') {
                    let value = self.hex(8);
                    if !self.eat('}') {
                        return Err(self.error("invalid code point value"));
                    }
                    self.scalar(value)?
                } else {
                    let value = self.hex(2);
                    self.scalar(value)?
                }
            }
            'u' => {
                if self.eat('{') {
                    let value = self.hex(8);
                    if !self.eat('}') {
                        return Err(self.error("invalid Unicode escape"));
                    }
                    self.scalar(value)?
                } else {
                    let start = self.pos;
                    let value = self.hex(4);
                    if self.pos - start != 4 {
                        return Err(self.error("invalid Unicode escape"));
                    }
                    self.scalar(value)?
                }
            }
            '0'..='7' => {
                let mut value = c.to_digit(8).unwrap();
                for _ in 0..2 {
                    match self.peek().and_then(|d| d.to_digit(8)) {
                        Some(d) => {
                            value = value * 8 + d;
                            self.pos += 1;
                        }
                        None => break,
                    }
                }
                self.scalar(value)?
            }
            'c' => {
                let control = self.next()?;
                self.scalar(control as u32 & 0x1f)?
            }
            _ => c,
        })
    }

    fn hex(&mut self, max: usize) -> u32 {
        let mut value = 0;
        for _ in 0..max {
            match self.peek().and_then(|c| c.to_digit(16)) {
                Some(d) => {
                    value = value * 16 + d;
                    self.pos += 1;
                }
                None => break,
            }
        }
        value
    }

    fn scalar(&self, value: u32) -> Result<char, Error> {
        char::from_u32(value).ok_or_else(|| self.error("invalid code point value"))
    }

    fn class_node(&mut self, flags: Flags) -> Result<Node, Error> {
        let class = self.class()?;
        // Onigmo compiles a one-character class to the character itself.
        if !class.negated && class.operands.len() == 1 && class.operands[0].len() == 1 {
            if let Item::Range(lo, hi) = class.operands[0][0] {
                if lo == hi {
                    return Ok(Node::Char {
                        c: lo,
                        fold: flags.fold,
                    });
                }
            }
        }
        Ok(Node::Class {
            class: Box::new(class),
            fold: flags.fold,
        })
    }

    /// Parses a bracket expression after its `[`.
    fn class(&mut self) -> Result<Class, Error> {
        let negated = self.eat('^');
        let mut operands = Vec::new();
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = self
                .peek()
                .ok_or_else(|| self.error("premature end of char-class"))?;
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;
            if c == '&' && self.peek_at(1) == Some('&') {
                self.pos += 2;
                operands.push(std::mem::take(&mut items));
                continue;
            }
            if c == '[' {
                if let Some(item) = self.posix()? {
                    items.push(item);
                    continue;
                }
                self.pos += 1;
                items.push(Item::Nested(self.class()?));
                continue;
            }
            let start = match self.class_atom()? {
                ClassAtom::Item(item) => {
                    items.push(item);
                    continue;
                }
                ClassAtom::Char(c) => c,
            };
            // A range needs a character on both sides; `-` before `]` or
            // `&&` is literal.
            if self.peek() == Some('-')
                && !matches!(self.peek_at(1), Some(']') | None)
                && !(self.peek_at(1) == Some('&') && self.peek_at(2) == Some('&'))
                && self.peek_at(1) != Some('[')
            {
                let save = self.pos;
                self.pos += 1;
                match self.class_atom()? {
                    ClassAtom::Char(end) => {
                        if end < start {
                            return Err(self.error("empty range in char class"));
                        }
                        items.push(Item::Range(start, end));
                        continue;
                    }
                    ClassAtom::Item(_) => {
                        self.pos = save;
                    }
                }
            }
            items.push(Item::Range(start, start));
        }
        operands.push(items);
        Ok(Class { negated, operands })
    }

    /// `[:name:]` or `[:^name:]` at the cursor.
    fn posix(&mut self) -> Result<Option<Item>, Error> {
        if self.peek_at(1) != Some(':') {
            return Ok(None);
        }
        let start = self.pos;
        self.pos += 2;
        let negated = self.eat('^');
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() {
                name.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.eat(':') && self.eat(']') {
            let table = unicode::posix(&name)
                .ok_or_else(|| self.error(&format!("invalid POSIX bracket type [:{name}:]")))?;
            return Ok(Some(Item::Table { table, negated }));
        }
        self.pos = start;
        Ok(None)
    }

    fn class_atom(&mut self) -> Result<ClassAtom, Error> {
        let c = self.next()?;
        if c != '\\' {
            return Ok(ClassAtom::Char(c));
        }
        let table = |name: &str, negated: bool| {
            ClassAtom::Item(Item::Table {
                table: unicode::builtin(name),
                negated,
            })
        };
        Ok(
            match self
                .peek()
                .ok_or_else(|| self.error("premature end of char-class"))?
            {
                'w' | 'W' | 'd' | 'D' | 's' | 'S' | 'h' | 'H' => {
                    let c = self.next()?;
                    table(&c.to_ascii_lowercase().to_string(), c.is_ascii_uppercase())
                }
                'p' | 'P' => {
                    let negated = self.next()? == 'P';
                    let (table, negated) = self.property(negated)?;
                    ClassAtom::Item(Item::Table { table, negated })
                }
                'b' => {
                    self.pos += 1;
                    ClassAtom::Char('\u{8}')
                }
                _ => ClassAtom::Char(self.escaped_char()?),
            },
        )
    }
}

enum ClassAtom {
    Char(char),
    Item(Item),
}
