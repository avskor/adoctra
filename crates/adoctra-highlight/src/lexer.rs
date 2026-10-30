//! Rouge's lexer runtime (`Rouge::Lexer`/`RegexLexer`).
//!
//! A [`LexerDef`] is a lexer class: its states, each an ordered list of rules
//! (a regexp with an action, or a mixed-in state). [`Lexer`] is an instance
//! with its state stack, options and lexer-specific variables. Lexing follows
//! `RegexLexer#stream_tokens`/`#step`: at each position the rules of the top
//! state are tried in order; a match advances the scanner, then its action
//! yields tokens and changes the stack; without a match one character is an
//! `Error` token. `Lexer#continue_lex` merges adjacent tokens of one type.
//!
//! Generated actions are token/next-state pairs or callbacks; a callback is
//! a Ruby block ported to Rust (translated by `scripts/generate_rouge.rb` or
//! written by hand) operating on a [`Scan`] like the block on the lexer.

use crate::regex::{Captures, Regex};
use crate::token::{Token, ERROR};
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// `RegexLexer::MAX_NULL_SCANS`.
const MAX_NULL_SCANS: usize = 5;

/// A Ruby block run when its rule matches.
pub type Callback = fn(&mut Scan<'_, '_>, &Match<'_>) -> Result<(), LexError>;
/// Creates a delegate lexer for its parent (`Klass.new(options)`).
pub type MakeSub = fn(&Lexer) -> Result<Lexer, LexError>;
/// A `start` block, an `initialize` override or a dynamic hook.
pub type Hook = fn(&mut Lexer) -> Result<(), LexError>;
/// A lexer whose `stream_tokens` is not `RegexLexer`'s.
pub type Stream = fn(&mut Lexer, &str, &mut Vec<(Token, String)>) -> Result<(), LexError>;

pub struct LexerDef {
    pub tag: &'static str,
    pub states: &'static [StateDef],
    /// `start` blocks, run at every `reset!`.
    pub start: &'static [Hook],
    /// `initialize` (after the options are stored).
    pub init: Option<Hook>,
    /// An overridden `reset!`, run after the `start` blocks.
    pub reset: Option<Hook>,
    /// A custom `stream_tokens`.
    pub stream: Option<Stream>,
    /// Ruby blocks/hooks of this class not yet ported (source locations);
    /// such a lexer is not used.
    pub missing: &'static [&'static str],
}

pub struct StateDef {
    pub name: &'static str,
    pub rules: &'static [Rule],
}

pub enum Rule {
    /// `rule re, …`: a regexp (an index into the lexer table's regexps),
    /// whether its source starts with `^` (Rouge then requires the scanner
    /// to be at the beginning of a line) and the action.
    Match {
        regex: usize,
        bol: bool,
        action: Action,
    },
    /// `mixin :state` (an index into the lexer's states).
    Mixin(usize),
}

pub enum Action {
    Emit(Token, Next),
    Call(Callback),
}

pub enum Next {
    Stay,
    Pop,
    /// `:push`: the current state again.
    Push,
    State(usize),
    Many(&'static [Next]),
    /// A state the lexer does not define: lexing fails when the rule fires.
    Unknown(&'static str),
}

/// Raised where Rouge raises (`empty stack!`, an unknown state …): Ruby's
/// conversion would fail; callers decide how to recover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError(pub String);

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LexError {}

fn empty_stack() -> LexError {
    LexError("empty stack!".into())
}

/// The generated tables every lexer refers to.
pub struct Tables {
    pub regexes: &'static [(&'static str, u32)],
    pub compiled: &'static [OnceLock<Regex>],
    pub lexers: &'static [&'static LexerDef],
    /// `Lexer.registry`: tag or alias to lexer.
    pub registry: &'static [(&'static str, usize)],
}

fn tables() -> &'static Tables {
    &crate::generated::lexers::TABLES
}

fn regex(index: usize) -> &'static Regex {
    let tables = tables();
    tables.compiled[index].get_or_init(|| {
        let (source, options) = tables.regexes[index];
        // The corpus test compiles every rule regexp.
        Regex::new(source, options)
            .unwrap_or_else(|error| panic!("rouge regexp /{source}/: {error}"))
    })
}

/// A lexer option value (`Lexer#options`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionValue {
    Bool(bool),
    Str(String),
    List(Vec<String>),
}

impl OptionValue {
    /// `Lexer#as_bool`.
    pub fn as_bool(&self) -> bool {
        match self {
            OptionValue::Bool(value) => *value,
            OptionValue::Str(value) => !matches!(value.as_str(), "0" | "false" | "off"),
            OptionValue::List(values) => values
                .last()
                .map_or(true, |last| !matches!(last.as_str(), "0" | "false" | "off")),
        }
    }

    /// `Lexer#as_string`.
    pub fn as_string(&self) -> Option<String> {
        match self {
            OptionValue::Bool(value) => Some(value.to_string()),
            OptionValue::Str(value) => Some(value.clone()),
            OptionValue::List(values) => values.last().cloned(),
        }
    }
}

pub type Options = BTreeMap<String, OptionValue>;

/// A lexer instance.
pub struct Lexer {
    pub def: &'static LexerDef,
    pub(crate) stack: Vec<&'static StateDef>,
    /// `@stack` is nil: the next use initializes it to `[:root]`.
    fresh: bool,
    pub options: Options,
    /// Lexer-specific instance variables, set by `init`/`start` hooks.
    pub vars: Option<Box<dyn Any>>,
    /// Persistent delegate lexers (`@ruby ||= Ruby.new(options)`).
    pub subs: BTreeMap<&'static str, Lexer>,
}

/// The ported lexers.
pub fn all() -> impl Iterator<Item = &'static LexerDef> {
    tables()
        .lexers
        .iter()
        .copied()
        .filter(|def| def.missing.is_empty())
}

/// Every lexer class of the pinned Rouge, ported or not.
pub fn every() -> impl Iterator<Item = &'static LexerDef> {
    tables().lexers.iter().copied()
}

/// `Rouge::Lexer.find` (a tag or alias, case sensitive), including lexers
/// whose port is incomplete.
pub fn find_def(name: &str) -> Option<&'static LexerDef> {
    let tables = tables();
    tables
        .registry
        .binary_search_by(|(key, _)| (*key).cmp(name))
        .ok()
        .map(|index| tables.lexers[tables.registry[index].1])
}

/// The plaintext lexer, the adapter's fallback.
pub fn plaintext() -> &'static LexerDef {
    find_def("plaintext").expect("plaintext lexer")
}

impl Lexer {
    /// `Lexer.new(options)`.
    pub fn new(def: &'static LexerDef, options: Options) -> Result<Lexer, LexError> {
        let mut lexer = Lexer {
            def,
            stack: Vec::new(),
            fresh: true,
            options,
            vars: None,
            subs: BTreeMap::new(),
        };
        if let Some(init) = def.init {
            init(&mut lexer)?;
        }
        Ok(lexer)
    }

    /// `Rouge::Lexer.find(name)&.new` for a ported lexer.
    pub fn find(name: &str) -> Option<Lexer> {
        let def = find_def(name).filter(|def| def.missing.is_empty())?;
        Lexer::new(def, Options::new()).ok()
    }

    pub fn tag(&self) -> &'static str {
        self.def.tag
    }

    /// `Lexer#lex`: reset, then lex.
    pub fn lex(&mut self, text: &str) -> Result<Vec<(Token, String)>, LexError> {
        self.reset()?;
        let mut out = Vec::new();
        self.continue_lex(text, &mut out)?;
        Ok(out)
    }

    /// `RegexLexer#reset!`: the stack goes back to `[:root]` (lazily, like
    /// `@stack = nil`) and the `start` blocks run.
    pub fn reset(&mut self) -> Result<(), LexError> {
        self.stack.clear();
        self.fresh = true;
        for start in self.def.start {
            start(self)?;
        }
        if let Some(reset) = self.def.reset {
            reset(self)?;
        }
        Ok(())
    }

    /// `Lexer#continue_lex`, appending to `out`: empty values are dropped
    /// and adjacent tokens of one type merged.
    pub fn continue_lex(
        &mut self,
        text: &str,
        out: &mut Vec<(Token, String)>,
    ) -> Result<(), LexError> {
        let mut raw = Vec::new();
        let result = match self.def.stream {
            Some(stream) => stream(self, text, &mut raw),
            None => self.stream_tokens(text, &mut raw),
        };
        for (token, value) in raw {
            push_merged(out, token, value);
        }
        result
    }

    /// The state stack, initialized to `[:root]` like `RegexLexer#stack`
    /// when it is nil (not when a pop emptied it).
    pub fn stack(&mut self) -> &mut Vec<&'static StateDef> {
        if self.fresh {
            self.fresh = false;
            self.stack.clear();
            self.stack
                .push(self.state_named("root").expect("root state"));
        }
        &mut self.stack
    }

    /// `get_state(name)`.
    pub fn state_named(&self, name: &str) -> Option<&'static StateDef> {
        self.def.states.iter().find(|state| state.name == name)
    }

    /// `push :name` from a hook (`start { push :bol }`).
    pub fn push_named(&mut self, name: &str) -> Result<(), LexError> {
        let state = self
            .state_named(name)
            .ok_or_else(|| LexError(format!("unknown state: :{name}")))?;
        self.stack().push(state);
        Ok(())
    }

    /// A delegate lexer `tag` with this lexer's options (`Klass.new(options)`).
    pub fn sub_for(&self, tag: &str) -> Result<Lexer, LexError> {
        let def = find_def(tag).ok_or_else(|| LexError(format!("unknown lexer {tag}")))?;
        Lexer::new(def, self.options.clone())
    }

    /// `@key = make(self)`.
    pub fn set_sub(&mut self, key: &'static str, make: MakeSub) -> Result<(), LexError> {
        let sub = make(self)?;
        self.subs.insert(key, sub);
        Ok(())
    }

    /// `@key.reset!`, creating a memoized delegate first (`key.reset!` of
    /// `def key; @key ||= …; end`); without `make` an unset `@key` is nil.
    pub fn reset_sub(&mut self, key: &'static str, make: Option<MakeSub>) -> Result<(), LexError> {
        if !self.subs.contains_key(key) {
            let make = make
                .ok_or_else(|| LexError(format!("undefined method `reset!' for nil (@{key})")))?;
            let sub = make(self)?;
            self.subs.insert(key, sub);
        }
        self.subs.get_mut(key).expect("delegate lexer").reset()
    }

    /// The lexer-specific variables of type `T`, created on first use.
    pub fn vars<T: Any + Default>(&mut self) -> &mut T {
        if !self.vars.as_ref().is_some_and(|vars| vars.is::<T>()) {
            self.vars = Some(Box::new(T::default()));
        }
        self.vars
            .as_mut()
            .and_then(|vars| vars.downcast_mut())
            .expect("lexer variables")
    }

    fn stream_tokens(
        &mut self,
        text: &str,
        out: &mut Vec<(Token, String)>,
    ) -> Result<(), LexError> {
        let mut scan = Scan {
            lexer: self,
            text,
            pos: 0,
            null_steps: 0,
            out,
        };
        // Ruby's `stack` initializes `[:root]` before the first step.
        scan.lexer.stack();
        while scan.pos < text.len() {
            let state = *scan.lexer.stack.last().ok_or_else(empty_stack)?;
            if !scan.step(state)? {
                let c = text[scan.pos..].chars().next().unwrap();
                scan.out.push((ERROR, c.to_string()));
                scan.pos += c.len_utf8();
            }
        }
        Ok(())
    }
}

/// The action of a Ruby block that is not ported (its lexer is unused).
pub(crate) fn unported(_: &mut Scan<'_, '_>, _: &Match<'_>) -> Result<(), LexError> {
    Err(LexError("unported Rouge rule".into()))
}

/// A `start` block that is not ported.
pub(crate) fn unported_hook(_: &mut Lexer) -> Result<(), LexError> {
    Err(LexError("unported Rouge hook".into()))
}

/// `collection.include?(value)` for a generated sorted collection.
pub(crate) fn contains(set: &[&str], value: &str) -> bool {
    set.binary_search(&value).is_ok()
}

/// `value =~ /re/` for a generated regexp.
pub(crate) fn search(index: usize, value: &str) -> bool {
    regex(index).search(value, 0).is_some()
}

fn push_merged(out: &mut Vec<(Token, String)>, token: Token, value: String) {
    if value.is_empty() {
        return;
    }
    match out.last_mut() {
        Some((last, text)) if *last == token => text.push_str(&value),
        _ => out.push((token, value)),
    }
}

/// The current match of a rule (the scanner's `m[i]`).
pub struct Match<'t> {
    rest: &'t str,
    captures: Captures,
}

impl<'t> Match<'t> {
    /// Group `index` (`m[index]`), `None` when it did not participate.
    pub fn get(&self, index: usize) -> Option<&'t str> {
        self.captures.text(self.rest, index)
    }

    /// The whole match (`m[0]`).
    pub fn text(&self) -> &'t str {
        self.get(0).unwrap_or("")
    }

    /// `m[index]` or the empty string, for `token Tok, m[i]` (Rouge skips
    /// both nil and empty values).
    pub fn group(&self, index: usize) -> &'t str {
        self.get(index).unwrap_or("")
    }
}

/// A lexer scanning a string: what a Ruby block sees as `self`.
pub struct Scan<'l, 't> {
    pub lexer: &'l mut Lexer,
    pub text: &'t str,
    /// The scanner position (already after the current match).
    pub pos: usize,
    null_steps: usize,
    out: &'l mut Vec<(Token, String)>,
}

impl<'l, 't> Scan<'l, 't> {
    fn step(&mut self, state: &'static StateDef) -> Result<bool, LexError> {
        for rule in state.rules {
            match rule {
                Rule::Mixin(index) => {
                    if self.step(&self.lexer.def.states[*index])? {
                        return Ok(true);
                    }
                }
                Rule::Match {
                    regex: index,
                    bol,
                    action,
                } => {
                    if *bol && !self.beginning_of_line() {
                        continue;
                    }
                    let rest = &self.text[self.pos..];
                    let Some(captures) = regex(*index).match_at(rest, 0) else {
                        continue;
                    };
                    let size = captures.end();
                    self.pos += size;
                    let m = Match { rest, captures };
                    match action {
                        Action::Emit(token, next) => {
                            self.out.push((*token, m.text().to_string()));
                            self.next(next)?;
                        }
                        Action::Call(callback) => callback(self, &m)?,
                    }
                    if size == 0 {
                        self.null_steps += 1;
                        if self.null_steps > MAX_NULL_SCANS {
                            return Ok(false);
                        }
                    } else {
                        self.null_steps = 0;
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn next(&mut self, next: &Next) -> Result<(), LexError> {
        match next {
            Next::Stay => {}
            Next::Pop => {
                self.lexer.stack.pop().ok_or_else(empty_stack)?;
            }
            Next::Push => {
                let top = *self.lexer.stack.last().ok_or_else(empty_stack)?;
                self.lexer.stack.push(top);
            }
            Next::State(index) => {
                let state = &self.lexer.def.states[*index];
                self.lexer.stack.push(state);
            }
            Next::Unknown(name) => return Err(LexError(format!("unknown state: :{name}"))),
            Next::Many(steps) => {
                for step in *steps {
                    match step {
                        // `next @stack.pop` ignores an empty stack here.
                        Next::Pop => {
                            self.lexer.stack.pop();
                        }
                        other => self.next(other)?,
                    }
                }
            }
        }
        Ok(())
    }

    /// `StringScanner#beginning_of_line?`.
    pub fn beginning_of_line(&self) -> bool {
        self.pos == 0 || self.text.as_bytes()[self.pos - 1] == b'\n'
    }

    /// `token tok, value` (nil/empty values are skipped).
    pub fn token(&mut self, token: Token, value: &str) {
        if !value.is_empty() {
            self.out.push((token, value.to_string()));
        }
    }

    /// `groups tok1, tok2, …`: one token per group.
    pub fn groups(&mut self, m: &Match<'_>, tokens: &[Token]) {
        for (index, token) in tokens.iter().enumerate() {
            self.token(*token, m.group(index + 1));
        }
    }

    /// `push :state`.
    pub fn push(&mut self, name: &str) -> Result<(), LexError> {
        let state = self
            .lexer
            .state_named(name)
            .ok_or_else(|| LexError(format!("unknown state: :{name}")))?;
        self.lexer.stack.push(state);
        Ok(())
    }

    /// `push` without a state: the current state again.
    pub fn push_current(&mut self) -> Result<(), LexError> {
        let top = *self.lexer.stack.last().ok_or_else(empty_stack)?;
        self.lexer.stack.push(top);
        Ok(())
    }

    /// `push` of a state defined in Rust (Ruby's `push do … end`).
    pub fn push_state(&mut self, state: &'static StateDef) {
        self.lexer.stack.push(state);
    }

    /// `pop!(times)`.
    pub fn pop(&mut self, times: usize) -> Result<(), LexError> {
        if self.lexer.stack.is_empty() {
            return Err(empty_stack());
        }
        let keep = self.lexer.stack.len().saturating_sub(times);
        self.lexer.stack.truncate(keep);
        Ok(())
    }

    /// `goto :state`.
    pub fn goto(&mut self, name: &str) -> Result<(), LexError> {
        let state = self
            .lexer
            .state_named(name)
            .ok_or_else(|| LexError(format!("unknown state: :{name}")))?;
        *self.lexer.stack.last_mut().ok_or_else(empty_stack)? = state;
        Ok(())
    }

    /// `reset_stack`.
    pub fn reset_stack(&mut self) {
        self.lexer.fresh = true;
        self.lexer.stack();
    }

    /// `in_state?(:name)`.
    pub fn in_state(&self, name: &str) -> bool {
        self.lexer.stack.iter().any(|state| state.name == name)
    }

    /// `state?(:name)`: the top of the stack.
    pub fn state_is(&self, name: &str) -> bool {
        self.lexer
            .stack
            .last()
            .is_some_and(|state| state.name == name)
    }

    /// `delegate(LexerClass, text)`: a fresh instance (`Class.lex`, no
    /// options) lexes `text`.
    pub fn delegate_class(&mut self, tag: &str, text: &str) -> Result<(), LexError> {
        let def = find_def(tag).ok_or_else(|| LexError(format!("unknown lexer {tag}")))?;
        let mut lexer = Lexer::new(def, Options::new())?;
        let tokens = lexer.lex(text)?;
        self.out.extend(tokens);
        Ok(())
    }

    /// `recurse(text)`: `delegate(self.class, text)`.
    pub fn recurse(&mut self, text: &str) -> Result<(), LexError> {
        self.delegate_class(self.lexer.def.tag, text)
    }

    /// `delegate(@sub, text)` for a persistent instance variable `key`,
    /// created by `make` on first use (`@sub ||= …`); it keeps its stack.
    pub fn delegate_sub(
        &mut self,
        key: &'static str,
        make: MakeSub,
        text: &str,
    ) -> Result<(), LexError> {
        if !self.lexer.subs.contains_key(key) {
            let sub = make(self.lexer)?;
            self.lexer.subs.insert(key, sub);
        }
        let sub = self.lexer.subs.get_mut(key).expect("delegate lexer");
        let mut tokens = Vec::new();
        sub.continue_lex(text, &mut tokens)?;
        self.out.extend(tokens);
        Ok(())
    }

    /// `delegate @key, text` for a delegate set by a hook; nil fails.
    pub fn delegate_existing(&mut self, key: &'static str, text: &str) -> Result<(), LexError> {
        let sub =
            self.lexer.subs.get_mut(key).ok_or_else(|| {
                LexError(format!("undefined method `continue_lex' for nil (@{key})"))
            })?;
        let mut tokens = Vec::new();
        sub.continue_lex(text, &mut tokens)?;
        self.out.extend(tokens);
        Ok(())
    }

    /// Delegates to a lexer instance owned by the caller.
    pub fn delegate_to(&mut self, lexer: &mut Lexer, text: &str) -> Result<(), LexError> {
        let mut tokens = Vec::new();
        lexer.continue_lex(text, &mut tokens)?;
        self.out.extend(tokens);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{ERROR, KEYWORD, NAME, TEXT};

    fn regex_index(source: &str) -> usize {
        tables()
            .regexes
            .iter()
            .position(|(s, o)| *s == source && *o == 0)
            .expect("regexp in the table")
    }

    fn leak<T>(value: T) -> &'static T {
        Box::leak(Box::new(value))
    }

    fn def(states: Vec<StateDef>) -> &'static LexerDef {
        leak(LexerDef {
            tag: "test",
            states: Box::leak(states.into_boxed_slice()),
            start: &[],
            init: None,
            reset: None,
            stream: None,
            missing: &[],
        })
    }

    fn rule(source: &str, action: Action) -> Rule {
        Rule::Match {
            regex: regex_index(source),
            bol: false,
            action,
        }
    }

    fn lex(def: &'static LexerDef, text: &str) -> Result<Vec<(Token, String)>, LexError> {
        Lexer::new(def, Options::new())?.lex(text)
    }

    #[test]
    fn unmatched_characters_are_errors_and_tokens_merge() {
        let rules = leak([
            rule(r"\d+", Action::Emit(NAME, Next::Stay)),
            rule(r"\s+", Action::Emit(NAME, Next::Stay)),
        ]);
        let def = def(vec![StateDef {
            name: "root",
            rules,
        }]);
        let tokens = lex(def, "1 2xy").unwrap();
        assert_eq!(
            tokens,
            vec![(NAME, "1 2".to_string()), (ERROR, "xy".to_string())]
        );
    }

    #[test]
    fn empty_matches_give_up_after_five_null_steps() {
        // Like RegexLexer#step: the sixth successive empty match fails the
        // step, so the character becomes an Error token.
        let rules = leak([rule("", Action::Emit(TEXT, Next::Stay))]);
        let def = def(vec![StateDef {
            name: "root",
            rules,
        }]);
        assert_eq!(lex(def, "ab").unwrap(), vec![(ERROR, "ab".to_string())]);
    }

    #[test]
    fn mixins_and_state_changes() {
        let keywords = leak([rule(r"\w+", Action::Emit(KEYWORD, Next::Stay))]);
        let word = leak([rule(r"\w+", Action::Emit(NAME, Next::Pop))]);
        let root = leak([
            rule(r"\s+", Action::Emit(TEXT, Next::State(2))),
            Rule::Mixin(1),
        ]);
        let def = def(vec![
            StateDef {
                name: "root",
                rules: root,
            },
            StateDef {
                name: "keywords",
                rules: keywords,
            },
            StateDef {
                name: "word",
                rules: word,
            },
        ]);
        let tokens = lex(def, "a b c").unwrap();
        let expected = [
            (KEYWORD, "a"),
            (TEXT, " "),
            (NAME, "b"),
            (TEXT, " "),
            (NAME, "c"),
        ];
        assert_eq!(
            tokens,
            expected.map(|(token, text)| (token, text.to_string()))
        );
    }

    #[test]
    fn popping_an_empty_stack_fails_like_rouge() {
        let rules = leak([rule(r"\w+", Action::Emit(KEYWORD, Next::Pop))]);
        let def = def(vec![StateDef {
            name: "root",
            rules,
        }]);
        assert_eq!(
            lex(def, "a b").unwrap_err(),
            LexError("empty stack!".into())
        );
        let unknown = leak([rule(
            r"\w+",
            Action::Emit(KEYWORD, Next::Unknown("nowhere")),
        )]);
        let def = super::tests::def(vec![StateDef {
            name: "root",
            rules: unknown,
        }]);
        assert_eq!(
            lex(def, "a").unwrap_err(),
            LexError("unknown state: :nowhere".into())
        );
    }

    #[test]
    fn many_ignores_popping_an_empty_stack() {
        static STEPS: [Next; 3] = [Next::Pop, Next::Pop, Next::State(0)];
        let rules = leak([rule(r"\w+", Action::Emit(KEYWORD, Next::Many(&STEPS)))]);
        let def = def(vec![StateDef {
            name: "root",
            rules,
        }]);
        assert_eq!(lex(def, "ab").unwrap(), vec![(KEYWORD, "ab".to_string())]);
    }

    #[test]
    fn delegate_lexers_persist_and_unset_ones_fail_like_nil() {
        use crate::token::LITERAL_NUMBER_INTEGER;
        fn json(l: &Lexer) -> Result<Lexer, LexError> {
            l.sub_for("json")
        }
        let mut lexer = Lexer::find("java").unwrap();
        // `@json.reset!` of an unset ivar is nil's NoMethodError; a memo
        // method creates the delegate first.
        assert!(lexer.reset_sub("json", None).is_err());
        lexer.reset_sub("json", Some(json)).unwrap();
        assert!(lexer.subs.contains_key("json"));
        lexer.set_sub("other", json).unwrap();
        let mut out = Vec::new();
        let text = "1";
        let mut scan = Scan {
            lexer: &mut lexer,
            text,
            pos: 0,
            null_steps: 0,
            out: &mut out,
        };
        scan.delegate_existing("other", "12").unwrap();
        assert!(scan.delegate_existing("missing", "x").is_err());
        assert_eq!(out, vec![(LITERAL_NUMBER_INTEGER, "12".to_string())]);
    }

    #[test]
    fn finds_ported_lexers_by_tag_and_alias() {
        assert_eq!(Lexer::find("java").map(|lexer| lexer.tag()), Some("java"));
        assert_eq!(
            Lexer::find("js").map(|lexer| lexer.tag()),
            Some("javascript")
        );
        assert!(Lexer::find("JAVA").is_none());
        assert!(find_def("no-such-language").is_none());
    }
}
