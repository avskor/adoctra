//! A backtracking regexp engine for the Onigmo (Ruby) syntax Rouge uses.
//!
//! Rouge lexes with `StringScanner#skip`, whose subject is the *rest* of the
//! input: `\A`, `^` and `\b` see the scan position as the subject start and a
//! lookbehind cannot look before it. [`Regex::match_at`] reproduces this
//! anchored match on a slice; [`Regex::search`] is Ruby's `=~` for the hand
//! ported lexer code.
//!
//! Lookarounds, atomic groups (and possessive quantifiers) and the recursive
//! `\g<0>` run as sub-matches that commit to their first result, as Onigmo
//! does for the first three. A sub-match nests only as deep as the pattern
//! (recursion: as deep as the input nesting, bounded by [`MAX_RECURSION`]).
//! Everything else backtracks on an explicit stack. A step budget
//! ([`STEP_LIMIT`]) turns catastrophic backtracking into a failed match;
//! Ruby's memoizing matcher would finish such a match instead.

mod parse;
mod unicode;

use parse::{Assertion, Class, Look, Node};
pub use parse::{EXTENDED, IGNORECASE, MULTILINE};

/// The backtracking steps one match may take.
pub const STEP_LIMIT: usize = 5_000_000;
/// The nesting of `\g<0>` calls one match may reach.
pub const MAX_RECURSION: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    /// Character offset in the pattern.
    pub offset: usize,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at {})", self.message, self.offset)
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub struct Regex {
    programs: Vec<Program>,
    subs: Vec<Sub>,
    classes: Vec<(Class, bool)>,
    groups: usize,
}

/// The byte ranges of a match and its groups, relative to the haystack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captures {
    slots: Vec<Option<usize>>,
}

impl Captures {
    /// Group `index` (0: the whole match) as a byte range.
    pub fn get(&self, index: usize) -> Option<(usize, usize)> {
        match (self.slots.get(2 * index)?, self.slots.get(2 * index + 1)?) {
            (Some(start), Some(end)) => Some((*start, *end)),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The end of the whole match.
    pub fn end(&self) -> usize {
        self.get(0).map_or(0, |(_, end)| end)
    }

    /// The text of group `index`.
    pub fn text<'h>(&self, haystack: &'h str, index: usize) -> Option<&'h str> {
        self.get(index).map(|(start, end)| &haystack[start..end])
    }
}

#[derive(Clone, Copy, Debug)]
enum Inst {
    Char(char),
    /// A character compared by simple case fold (the folded form).
    CharFold(char),
    Class(usize),
    Any,
    AnyNewline,
    Assert(Assertion),
    /// Try the first target, backtrack to the second.
    Split(usize, usize),
    Jump(usize),
    Save(usize),
    /// Remember the position in a loop register (undone on backtrack).
    Mark(usize),
    /// Repeat the loop at the target unless the iteration was empty.
    Again {
        register: usize,
        target: usize,
    },
    Backref {
        group: usize,
        fold: bool,
    },
    /// Run a sub-program, see [`Sub`].
    Sub(usize),
    Recurse,
    Match,
}

#[derive(Debug)]
enum Sub {
    Look {
        kind: Look,
        program: usize,
    },
    /// Lookbehind alternatives, each with its fixed length in characters.
    Behind {
        negated: bool,
        alternatives: Vec<(usize, usize)>,
    },
    Atomic(usize),
}

#[derive(Debug, Default)]
struct Program {
    insts: Vec<Inst>,
    registers: usize,
}

impl Regex {
    /// Compiles `source` with `Regexp#options` bits ([`IGNORECASE`],
    /// [`EXTENDED`], [`MULTILINE`]).
    pub fn new(source: &str, options: u32) -> Result<Regex, Error> {
        let parsed = parse::parse(source, options)?;
        let mut compiler = Compiler {
            programs: vec![Program::default()],
            subs: Vec::new(),
            classes: Vec::new(),
        };
        compiler.emit(0, &parsed.node)?;
        compiler.programs[0].insts.push(Inst::Match);
        let Compiler {
            programs,
            subs,
            classes,
        } = compiler;
        Ok(Regex {
            programs,
            subs,
            classes,
            groups: parsed.groups,
        })
    }

    /// The number of capturing groups.
    pub fn groups(&self) -> usize {
        self.groups
    }

    /// An anchored match starting at byte `start` of `haystack`, like
    /// `StringScanner#skip` on a scanner positioned there when `haystack`
    /// is the scanner's rest.
    pub fn match_at(&self, haystack: &str, start: usize) -> Option<Captures> {
        let mut slots = vec![None; 2 * (self.groups + 1)];
        let mut budget = STEP_LIMIT;
        let end = self.run(0, haystack, start, &mut slots, &mut budget, 0)?;
        slots[0] = Some(start);
        slots[1] = Some(end);
        Some(Captures { slots })
    }

    /// The leftmost match at or after byte `start`, like `=~`.
    pub fn search(&self, haystack: &str, start: usize) -> Option<Captures> {
        let mut position = start;
        loop {
            if let Some(captures) = self.match_at(haystack, position) {
                return Some(captures);
            }
            let c = haystack[position..].chars().next()?;
            position += c.len_utf8();
        }
    }

    pub fn is_match(&self, haystack: &str) -> bool {
        self.search(haystack, 0).is_some()
    }

    fn run(
        &self,
        program: usize,
        haystack: &str,
        start: usize,
        slots: &mut [Option<usize>],
        budget: &mut usize,
        depth: usize,
    ) -> Option<usize> {
        let program_ref = &self.programs[program];
        let insts = &program_ref.insts;
        let mut registers = vec![usize::MAX; program_ref.registers];
        let mut stack: Vec<Frame> = Vec::new();
        let mut pc = 0;
        let mut pos = start;
        let bytes = haystack.as_bytes();
        loop {
            if *budget == 0 {
                return None;
            }
            *budget -= 1;
            let ok = match insts[pc] {
                Inst::Char(c) => match next_char(haystack, pos) {
                    Some(d) if d == c => {
                        pos += c.len_utf8();
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::CharFold(c) => match next_char(haystack, pos) {
                    Some(d) if unicode::fold(d) == c => {
                        pos += d.len_utf8();
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::Class(index) => match next_char(haystack, pos) {
                    Some(d) => {
                        let (class, fold) = &self.classes[index];
                        let inside = if *fold {
                            class.contains_folded(d)
                        } else {
                            class.contains(d)
                        };
                        if inside {
                            pos += d.len_utf8();
                            pc += 1;
                        }
                        inside
                    }
                    None => false,
                },
                Inst::Any => match next_char(haystack, pos) {
                    Some(d) if d != '\n' => {
                        pos += d.len_utf8();
                        pc += 1;
                        true
                    }
                    _ => false,
                },
                Inst::AnyNewline => match next_char(haystack, pos) {
                    Some(d) => {
                        pos += d.len_utf8();
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Inst::Assert(assertion) => {
                    let ok = assert(assertion, haystack, pos, bytes);
                    pc += 1;
                    ok
                }
                Inst::Split(first, second) => {
                    stack.push(Frame::Alternative { pc: second, pos });
                    pc = first;
                    true
                }
                Inst::Jump(target) => {
                    pc = target;
                    true
                }
                Inst::Save(slot) => {
                    stack.push(Frame::Slot {
                        slot,
                        old: slots[slot],
                    });
                    slots[slot] = Some(pos);
                    pc += 1;
                    true
                }
                Inst::Mark(register) => {
                    stack.push(Frame::Register {
                        register,
                        old: registers[register],
                    });
                    registers[register] = pos;
                    pc += 1;
                    true
                }
                Inst::Again { register, target } => {
                    pc = if pos != registers[register] {
                        target
                    } else {
                        pc + 1
                    };
                    true
                }
                Inst::Backref { group, fold } => match (
                    slots.get(2 * group).copied().flatten(),
                    slots.get(2 * group + 1).copied().flatten(),
                ) {
                    (Some(from), Some(to)) => {
                        match matches_text(&haystack[from..to], haystack, pos, fold) {
                            Some(end) => {
                                pos = end;
                                pc += 1;
                                true
                            }
                            None => false,
                        }
                    }
                    _ => false,
                },
                Inst::Sub(index) => {
                    let ok = self.sub(index, haystack, &mut pos, slots, &mut stack, budget, depth);
                    pc += 1;
                    ok
                }
                Inst::Recurse => {
                    if depth >= MAX_RECURSION {
                        false
                    } else {
                        // The called pattern's own groups do not leak into the
                        // caller's (Ruby keeps the outer captures).
                        let mut inner = slots.to_vec();
                        match self.run(0, haystack, pos, &mut inner, budget, depth + 1) {
                            Some(end) => {
                                pos = end;
                                pc += 1;
                                true
                            }
                            None => false,
                        }
                    }
                }
                Inst::Match => return Some(pos),
            };
            if !ok {
                loop {
                    match stack.pop() {
                        None => return None,
                        Some(Frame::Slot { slot, old }) => slots[slot] = old,
                        Some(Frame::Register { register, old }) => registers[register] = old,
                        Some(Frame::Alternative { pc: next, pos: at }) => {
                            pc = next;
                            pos = at;
                            break;
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sub(
        &self,
        index: usize,
        haystack: &str,
        pos: &mut usize,
        slots: &mut [Option<usize>],
        stack: &mut Vec<Frame>,
        budget: &mut usize,
        depth: usize,
    ) -> bool {
        match &self.subs[index] {
            Sub::Atomic(program) => {
                let mut inner = slots.to_vec();
                match self.run(*program, haystack, *pos, &mut inner, budget, depth) {
                    Some(end) => {
                        commit(slots, inner, stack);
                        *pos = end;
                        true
                    }
                    None => false,
                }
            }
            Sub::Look { kind, program } => {
                let mut inner = slots.to_vec();
                let found = self
                    .run(*program, haystack, *pos, &mut inner, budget, depth)
                    .is_some();
                match kind {
                    Look::Ahead if found => {
                        commit(slots, inner, stack);
                        true
                    }
                    Look::NotAhead => !found,
                    _ => false,
                }
            }
            Sub::Behind {
                negated,
                alternatives,
            } => {
                let mut found = None;
                for &(length, program) in alternatives {
                    let Some(from) = back(haystack, *pos, length) else {
                        continue;
                    };
                    let mut inner = slots.to_vec();
                    if self.run(program, haystack, from, &mut inner, budget, depth) == Some(*pos) {
                        found = Some(inner);
                        break;
                    }
                }
                match (found, negated) {
                    (Some(inner), false) => {
                        commit(slots, inner, stack);
                        true
                    }
                    (None, true) => true,
                    _ => false,
                }
            }
        }
    }
}

/// Applies a committed sub-match's captures, undoable by later backtracking.
fn commit(slots: &mut [Option<usize>], inner: Vec<Option<usize>>, stack: &mut Vec<Frame>) {
    for (slot, value) in inner.into_iter().enumerate() {
        if slots[slot] != value {
            stack.push(Frame::Slot {
                slot,
                old: slots[slot],
            });
            slots[slot] = value;
        }
    }
}

enum Frame {
    Alternative { pc: usize, pos: usize },
    Slot { slot: usize, old: Option<usize> },
    Register { register: usize, old: usize },
}

fn next_char(haystack: &str, pos: usize) -> Option<char> {
    haystack.get(pos..)?.chars().next()
}

/// The subject is the whole haystack: Rouge passes the scanner's rest, and
/// `search` lets a lookbehind see the text before its start position.
fn prev_char(haystack: &str, pos: usize) -> Option<char> {
    haystack[..pos].chars().next_back()
}

/// The byte offset `length` characters before `pos`.
fn back(haystack: &str, pos: usize, length: usize) -> Option<usize> {
    let mut at = pos;
    for _ in 0..length {
        let c = prev_char(haystack, at)?;
        at -= c.len_utf8();
    }
    Some(at)
}

fn assert(assertion: Assertion, haystack: &str, pos: usize, bytes: &[u8]) -> bool {
    let len = bytes.len();
    match assertion {
        Assertion::LineStart => pos == 0 || (bytes[pos - 1] == b'\n' && pos < len),
        Assertion::LineEnd => pos == len || bytes[pos] == b'\n',
        Assertion::TextStart => pos == 0,
        Assertion::TextEnd => pos == len,
        Assertion::TextEndNewline => pos == len || (pos + 1 == len && bytes[pos] == b'\n'),
        Assertion::WordBoundary | Assertion::NotWordBoundary => {
            let before = prev_char(haystack, pos).is_some_and(unicode::is_word);
            let after = next_char(haystack, pos).is_some_and(unicode::is_word);
            (before != after) == (assertion == Assertion::WordBoundary)
        }
    }
}

/// Matches `text` at `pos` (by simple case fold with `fold`).
fn matches_text(text: &str, haystack: &str, pos: usize, fold: bool) -> Option<usize> {
    let rest = haystack.get(pos..)?;
    if !fold {
        return rest.starts_with(text).then_some(pos + text.len());
    }
    let mut chars = rest.chars();
    let mut end = pos;
    for expected in text.chars() {
        let actual = chars.next()?;
        if unicode::fold(actual) != unicode::fold(expected) {
            return None;
        }
        end += actual.len_utf8();
    }
    Some(end)
}

struct Compiler {
    programs: Vec<Program>,
    subs: Vec<Sub>,
    classes: Vec<(Class, bool)>,
}

impl Compiler {
    fn push(&mut self, program: usize, inst: Inst) -> usize {
        let insts = &mut self.programs[program].insts;
        insts.push(inst);
        insts.len() - 1
    }

    fn here(&self, program: usize) -> usize {
        self.programs[program].insts.len()
    }

    fn patch(&mut self, program: usize, at: usize, inst: Inst) {
        self.programs[program].insts[at] = inst;
    }

    fn new_program(&mut self, node: &Node) -> Result<usize, Error> {
        self.programs.push(Program::default());
        let program = self.programs.len() - 1;
        self.emit(program, node)?;
        self.push(program, Inst::Match);
        Ok(program)
    }

    fn emit(&mut self, program: usize, node: &Node) -> Result<(), Error> {
        match node {
            Node::Empty => {}
            Node::Char { c, fold } => {
                let inst = if *fold {
                    Inst::CharFold(unicode::fold(*c))
                } else {
                    Inst::Char(*c)
                };
                self.push(program, inst);
            }
            Node::Any { dotall } => {
                self.push(program, if *dotall { Inst::AnyNewline } else { Inst::Any });
            }
            Node::Class { class, fold } => {
                self.classes.push(((**class).clone(), *fold));
                let index = self.classes.len() - 1;
                self.push(program, Inst::Class(index));
            }
            Node::Assert(assertion) => {
                self.push(program, Inst::Assert(*assertion));
            }
            Node::Concat(nodes) => {
                for node in nodes {
                    self.emit(program, node)?;
                }
            }
            Node::Alt(alternatives) => {
                let mut jumps = Vec::new();
                for (i, alternative) in alternatives.iter().enumerate() {
                    if i + 1 < alternatives.len() {
                        let split = self.push(program, Inst::Split(0, 0));
                        self.emit(program, alternative)?;
                        jumps.push(self.push(program, Inst::Jump(0)));
                        let next = self.here(program);
                        self.patch(program, split, Inst::Split(split + 1, next));
                    } else {
                        self.emit(program, alternative)?;
                    }
                }
                let end = self.here(program);
                for jump in jumps {
                    self.patch(program, jump, Inst::Jump(end));
                }
            }
            Node::Group { index, node } => {
                self.push(program, Inst::Save(2 * index));
                self.emit(program, node)?;
                self.push(program, Inst::Save(2 * index + 1));
            }
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.repeat(program, node, *min, *max, *greedy)?,
            Node::Atomic(node) => {
                let inner = self.new_program(node)?;
                self.subs.push(Sub::Atomic(inner));
                let index = self.subs.len() - 1;
                self.push(program, Inst::Sub(index));
            }
            Node::Look { kind, node } => {
                let sub = match kind {
                    Look::Ahead | Look::NotAhead => Sub::Look {
                        kind: *kind,
                        program: self.new_program(node)?,
                    },
                    Look::Behind | Look::NotBehind => {
                        let branches: Vec<&Node> = match &**node {
                            Node::Alt(alternatives) => alternatives.iter().collect(),
                            other => vec![other],
                        };
                        let mut alternatives = Vec::new();
                        for branch in branches {
                            let length = fixed_length(branch).ok_or(Error {
                                message: "invalid pattern in look-behind".to_string(),
                                offset: 0,
                            })?;
                            alternatives.push((length, self.new_program(branch)?));
                        }
                        Sub::Behind {
                            negated: *kind == Look::NotBehind,
                            alternatives,
                        }
                    }
                };
                self.subs.push(sub);
                let index = self.subs.len() - 1;
                self.push(program, Inst::Sub(index));
            }
            Node::Backref { index, fold } => {
                self.push(
                    program,
                    Inst::Backref {
                        group: *index,
                        fold: *fold,
                    },
                );
            }
            Node::Recurse => {
                self.push(program, Inst::Recurse);
            }
        }
        Ok(())
    }

    fn repeat(
        &mut self,
        program: usize,
        node: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
    ) -> Result<(), Error> {
        for _ in 0..min {
            self.emit(program, node)?;
        }
        match max {
            None => {
                // L: split body/exit; body: mark, node, again(L).
                let register = self.programs[program].registers;
                self.programs[program].registers += 1;
                let head = self.push(program, Inst::Split(0, 0));
                self.push(program, Inst::Mark(register));
                self.emit(program, node)?;
                self.push(
                    program,
                    Inst::Again {
                        register,
                        target: head,
                    },
                );
                let exit = self.here(program);
                let split = if greedy {
                    Inst::Split(head + 1, exit)
                } else {
                    Inst::Split(exit, head + 1)
                };
                self.patch(program, head, split);
            }
            Some(max) => {
                let mut splits = Vec::new();
                for _ in min..max {
                    splits.push(self.push(program, Inst::Split(0, 0)));
                    self.emit(program, node)?;
                }
                let exit = self.here(program);
                for split in splits {
                    let inst = if greedy {
                        Inst::Split(split + 1, exit)
                    } else {
                        Inst::Split(exit, split + 1)
                    };
                    self.patch(program, split, inst);
                }
            }
        }
        Ok(())
    }
}

/// The length in characters every match of `node` has, if fixed.
fn fixed_length(node: &Node) -> Option<usize> {
    match node {
        Node::Empty | Node::Assert(_) | Node::Look { .. } => Some(0),
        Node::Char { .. } | Node::Any { .. } | Node::Class { .. } => Some(1),
        Node::Concat(nodes) => nodes.iter().map(fixed_length).sum(),
        Node::Alt(alternatives) => {
            let first = fixed_length(alternatives.first()?)?;
            alternatives
                .iter()
                .all(|a| fixed_length(a) == Some(first))
                .then_some(first)
        }
        Node::Group { node, .. } | Node::Atomic(node) => fixed_length(node),
        Node::Repeat { node, min, max, .. } if Some(*min) == *max => {
            Some(fixed_length(node)? * *min as usize)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `StringScanner#skip` at byte `pos`: the subject is the rest.
    fn skip(source: &str, options: u32, text: &str, pos: usize) -> Option<usize> {
        Regex::new(source, options)
            .unwrap()
            .match_at(&text[pos..], 0)
            .map(|captures| captures.end())
    }

    // Expected values are the pinned Ruby's (Onigmo) results.
    #[test]
    fn option_switch_covers_later_alternatives() {
        assert_eq!(skip("a(?i)b|c", 0, "C", 0), None);
        assert_eq!(skip("a(?i)b|c", 0, "aB", 0), Some(2));
        assert_eq!(skip("(?i:A)a", 0, "aa", 0), Some(2));
        assert_eq!(skip("(?i:A)a", 0, "aA", 0), None);
    }

    #[test]
    fn intervals_follow_ruby() {
        // `{n}+` repeats the interval instead of being possessive.
        assert_eq!(skip("x{2}+", 0, "xxxxx", 0), Some(4));
        assert_eq!(skip("a{,2}", 0, "aaa", 0), Some(2));
        assert_eq!(skip("a{x", 0, "a{x", 0), Some(3));
    }

    #[test]
    fn atomic_groups_and_possessive_quantifiers_commit() {
        assert_eq!(skip("a*+a", 0, "aaa", 0), None);
        assert_eq!(skip("(?>a|ab)c", 0, "abc", 0), None);
    }

    #[test]
    fn backreferences_and_recursion() {
        assert_eq!(skip(r"(a)\1", IGNORECASE, "aA", 0), Some(2));
        let regex = Regex::new(r"(\{(?:[^\{\}]|(\g<0>))+\})", 0).unwrap();
        let captures = regex.match_at("{a{b}c}d", 0).unwrap();
        assert_eq!(captures.get(1), Some((0, 7)));
        assert_eq!(captures.get(2), Some((2, 5)));
    }

    #[test]
    fn the_subject_starts_at_the_scan_position() {
        assert_eq!(skip("(?<=a)b", 0, "ab", 1), None);
        assert_eq!(skip("^b", 0, "ab", 1), Some(1));
        assert_eq!(skip(r"\Z", 0, "a\n", 1), Some(0));
        // `^` does not match after a final LF; `search` sees the whole text.
        assert!(Regex::new("\n^", 0).unwrap().search("a\n", 0).is_none());
        assert_eq!(
            Regex::new("(?<=a)b", 0)
                .unwrap()
                .search("ab", 0)
                .map(|c| c.get(0)),
            Some(Some((1, 2)))
        );
    }

    #[test]
    fn empty_iterations_end_a_loop() {
        assert_eq!(skip("(a|)*b", 0, "aab", 0), Some(3));
    }

    #[test]
    fn classes() {
        assert_eq!(skip("[a-z&&[^aeiou]]+", 0, "xyzab", 0), Some(3));
        assert_eq!(skip("[]a]+", 0, "]a]", 0), Some(3));
        assert_eq!(skip(r"[\b]", 0, "\u{8}", 0), Some(1));
        assert_eq!(skip(r"\p{^L}", 0, "1", 0), Some(1));
        assert_eq!(skip(r"\x41\x42\103", 0, "ABC", 0), Some(3));
        assert_eq!(skip("a b # c\n  c", EXTENDED, "abc", 0), Some(3));
    }

    #[test]
    fn case_folding_in_classes_follows_onigmo() {
        // A Latin-1 variant is not added to a folded class; others are.
        assert_eq!(skip("[à-ÿ]", IGNORECASE, "É", 0), None);
        assert_eq!(skip("[é]", IGNORECASE, "É", 0), Some(2));
        assert_eq!(skip("[α-ω]", IGNORECASE, "Σ", 0), Some(2));
        assert_eq!(skip("[a-z]", IGNORECASE, "\u{212a}", 0), Some(3));
        // ASCII class escapes are never folded.
        assert_eq!(skip(r"\w", IGNORECASE, "\u{212a}", 0), None);
        assert_eq!(skip(r"[\w.]", IGNORECASE, "\u{212a}", 0), None);
        assert_eq!(skip(r"\W", IGNORECASE, "\u{212a}", 0), Some(3));
        assert_eq!(skip(r"[[:alpha:]]", IGNORECASE, "\u{212a}", 0), Some(3));
        // `\w` is ASCII, `\b` uses the Unicode Word property.
        assert_eq!(skip(r"\w", 0, "é", 0), None);
        assert_eq!(skip(r"a\b", 0, "aé", 0), None);
    }

    #[test]
    fn catastrophic_backtracking_is_bounded() {
        let text = "a".repeat(40);
        assert_eq!(skip("(a|a)*b", 0, &text, 0), None);
    }

    #[test]
    fn invalid_patterns_are_errors() {
        assert!(Regex::new("(a", 0).is_err());
        assert!(Regex::new(r"\p{Bogus}", 0).is_err());
        assert!(Regex::new("(?<=a+)b", 0).is_err());
    }
}
