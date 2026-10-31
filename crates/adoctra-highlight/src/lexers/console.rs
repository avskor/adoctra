//! `Rouge::Lexers::ConsoleLexer` and its subclass `IRBLexer` (`irb`): a
//! shell session line by line. A prompt line yields the prompt and lexes the
//! rest with the language lexer (which keeps its state across prompts);
//! snips and comments are comments; other lines are output.
//!
//! Delegates (console): shell, plaintext.
//! Delegates (irb): ruby, irb_output.

use crate::lexer::{find_def, LexError, Lexer, OptionValue, Options};
use crate::regex::{Regex, EXTENDED};
use crate::token::{Token, COMMENT, GENERIC_ERROR, GENERIC_PROMPT, TEXT_WHITESPACE};
use std::sync::OnceLock;

#[derive(Default)]
struct Vars {
    prompt: Vec<String>,
    /// `@comments`: `None` is `:guess`.
    comments: Option<bool>,
    error: Vec<String>,
    prompt_regex: Option<Regex>,
    error_regex: Option<Regex>,
}

fn irb(lexer: &Lexer) -> bool {
    lexer.def.tag == "irb"
}

/// `Lexer#as_list`: strings split at commas (Ruby's `split` drops trailing
/// empty fields), lists flattened, anything else empty.
fn as_list(value: Option<OptionValue>) -> Vec<String> {
    let split = |text: &str| {
        let mut parts: Vec<String> = text.split(',').map(str::to_string).collect();
        while parts.last().is_some_and(String::is_empty) {
            parts.pop();
        }
        parts
    };
    match value {
        Some(OptionValue::Str(text)) => split(&text),
        Some(OptionValue::List(items)) => items.iter().flat_map(|item| split(item)).collect(),
        _ => Vec::new(),
    }
}

/// `Lexer#as_lexer` of a name: the lexer with the current options, if found.
fn as_lexer(value: Option<OptionValue>, options: &Options) -> Option<Result<Lexer, LexError>> {
    let name = match value? {
        OptionValue::Str(name) => name,
        OptionValue::List(names) => names.last()?.clone(),
        OptionValue::Bool(_) => return None,
    };
    find_def(&name).map(|def| Lexer::new(def, options.clone()))
}

/// `initialize`: the options and the language/output lexers.
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    let prompt = as_list(lexer.options.remove("prompt"));
    let lang = lexer
        .options
        .remove("lang")
        .unwrap_or(OptionValue::Str("shell".into()));
    let lang = as_lexer(Some(lang), &lexer.options).transpose()?;
    let output = match lexer.options.remove("output") {
        Some(value) => as_lexer(Some(value), &lexer.options).transpose()?,
        None => None,
    };
    let comments = lexer.options.get("comments").map(OptionValue::as_bool);
    let error = as_list(lexer.options.remove("error"));

    // `lang_lexer`/`output_lexer` (irb: Ruby and IRBOutputLexer).
    let lang = if irb(lexer) {
        lexer.sub_for("ruby")?
    } else {
        match lang {
            Some(lang) => lang,
            None => lexer.sub_for("shell")?,
        }
    };
    let output = if irb(lexer) {
        lexer.sub_for("irb_output")?
    } else {
        match output {
            Some(output) => output,
            None => {
                let mut options = Options::new();
                options.insert("token".into(), OptionValue::Str("Generic.Output".into()));
                Lexer::new(find_def("plaintext").expect("plaintext lexer"), options)?
            }
        }
    };
    lexer.subs.insert("lang_lexer", lang);
    lexer.subs.insert("output_lexer", output);

    let vars = lexer.vars::<Vars>();
    vars.prompt = prompt;
    vars.comments = comments;
    vars.error = error;
    let prompt_regex = prompt_regex(lexer)?;
    let error_regex = error_regex(lexer)?;
    let vars = lexer.vars::<Vars>();
    vars.prompt_regex = Some(prompt_regex);
    vars.error_regex = error_regex;
    Ok(())
}

fn compile(source: &str, options: u32) -> Result<Regex, LexError> {
    Regex::new(source, options)
        .map_err(|error| LexError(format!("console regexp /{source}/: {error}")))
}

/// `allow_comments?`.
fn allow_comments(lexer: &mut Lexer) -> bool {
    if irb(lexer) {
        return true;
    }
    let vars = lexer.vars::<Vars>();
    match vars.comments {
        Some(comments) => comments,
        None => !vars.prompt.is_empty() && !end_chars(lexer).iter().any(|c| c == "#"),
    }
}

/// `end_chars`.
fn end_chars(lexer: &mut Lexer) -> Vec<String> {
    let prompt = lexer.vars::<Vars>().prompt.clone();
    if !prompt.is_empty() {
        prompt.into_iter().filter(|c| !c.is_empty()).collect()
    } else if allow_comments(lexer) {
        vec!["$".into(), ">".into(), ";".into()]
    } else {
        vec!["$".into(), "#".into(), ">".into(), ";".into()]
    }
}

/// `prompt_regex`: a prompt prefix (`prompt_prefix_regex`, interpolated as
/// `(?m-ix:…)`) up to one of the end characters.
fn prompt_regex(lexer: &mut Lexer) -> Result<Regex, LexError> {
    if irb(lexer) {
        return compile("\n          ^.*?\n          (\n            (irb|pry).*?[>\"*] |\n            [>\"*]>\n          )\n        ", EXTENDED);
    }
    let prefix = if allow_comments(lexer) {
        "[^<#]*?"
    } else {
        ".*?"
    };
    let ends: Vec<String> = end_chars(lexer)
        .iter()
        .map(|c| crate::lexer::regexp_escape(c))
        .collect();
    compile(&format!("^(?m-ix:{prefix})(?:{})", ends.join("|")), 0)
}

/// `error_regex`: nil without `error` option values.
fn error_regex(lexer: &mut Lexer) -> Result<Option<Regex>, LexError> {
    let error = lexer.vars::<Vars>().error.clone();
    if error.is_empty() {
        return Ok(None);
    }
    let alternatives: Vec<String> = error
        .iter()
        .map(|e| crate::lexer::regexp_escape(e))
        .collect();
    compile(&format!("^(?:{})", alternatives.join("|")), 0).map(Some)
}

fn static_regex(cell: &'static OnceLock<Regex>, source: &str, options: u32) -> &'static Regex {
    cell.get_or_init(|| Regex::new(source, options).expect("console regexp"))
}

fn reset(lexer: &mut Lexer, key: &'static str) -> Result<(), LexError> {
    lexer.subs.get_mut(key).expect("console delegate").reset()
}

fn continue_lex(
    lexer: &mut Lexer,
    key: &'static str,
    text: &str,
    out: &mut Vec<(Token, String)>,
) -> Result<(), LexError> {
    lexer
        .subs
        .get_mut(key)
        .expect("console delegate")
        .continue_lex(text, out)
}

/// `stream_tokens`: `process_line` until the input ends.
pub(crate) fn stream(
    lexer: &mut Lexer,
    text: &str,
    out: &mut Vec<(Token, String)>,
) -> Result<(), LexError> {
    static SNIP: OnceLock<Regex> = OnceLock::new();
    static COMMENT_REGEX: OnceLock<Regex> = OnceLock::new();
    static WHITESPACE: OnceLock<Regex> = OnceLock::new();
    let snip = static_regex(&SNIP, r"\A\s*(?:<[.]+>|[.]+)\s*\z", 0);
    let comment = static_regex(&COMMENT_REGEX, r"\A\s*?#", 0);
    let whitespace = static_regex(&WHITESPACE, r"\A\s*", 0);

    reset(lexer, "lang_lexer")?;
    reset(lexer, "output_lexer")?;
    let mut pos = 0;
    while pos < text.len() {
        // `input.scan(/(.*?)(\n|$)/)`: through the next LF, or to the end.
        let end = text[pos..].find('\n').map_or(text.len(), |at| pos + at + 1);
        let line = &text[pos..end];
        pos = end;

        let prompt = lexer
            .vars::<Vars>()
            .prompt_regex
            .as_ref()
            .and_then(|re| re.search(line, 0));
        if snip.is_match(line) {
            reset(lexer, "output_lexer")?;
            reset(lexer, "lang_lexer")?;
            out.push((COMMENT, line.to_string()));
        } else if let Some(prompt) = prompt {
            reset(lexer, "output_lexer")?;
            let (start, end) = prompt.get(0).unwrap_or((0, 0));
            out.push((GENERIC_PROMPT, line[start..end].to_string()));
            let rest = &line[end..];
            let space = whitespace.match_at(rest, 0).map_or(0, |m| m.end());
            if space > 0 {
                out.push((TEXT_WHITESPACE, rest[..space].to_string()));
            }
            continue_lex(lexer, "lang_lexer", &rest[space..], out)?;
        } else if comment.is_match(
            line.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\u{b}' || c == '\0'),
        ) {
            reset(lexer, "output_lexer")?;
            reset(lexer, "lang_lexer")?;
            out.push((COMMENT, line.to_string()));
        } else if lexer
            .vars::<Vars>()
            .error_regex
            .as_ref()
            .is_some_and(|re| re.is_match(line))
        {
            reset(lexer, "output_lexer")?;
            reset(lexer, "lang_lexer")?;
            out.push((GENERIC_ERROR, line.to_string()));
        } else {
            reset(lexer, "lang_lexer")?;
            continue_lex(lexer, "output_lexer", line, out)?;
        }
    }
    Ok(())
}
