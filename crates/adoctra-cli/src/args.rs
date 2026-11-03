//! Command-line parsing like Ruby's CLI (`Asciidoctor::Cli::Options`, an
//! `OptionParser`): long options by unique prefix (`--em`), `--name=value`,
//! clustered short flags (`-sn`) and attached values (`-Ssafe`), enumerated
//! values by unique case-sensitive prefix (`-d b`), and Ruby's messages for
//! invalid, ambiguous and missing arguments. `-h` and `-V` act when parsed.
//! adoctra adds `--to`, `--stdout`, `--json-attributes`, `--json-locations`,
//! `--json-references` and `--diagnostics`.

use std::path::PathBuf;

/// Ruby's log levels for `--failure-level` (and the maximum logged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
    Fatal,
}

/// Ruby's `:verbose` option: 0 (`-q`, a NullLogger), 1, 2 (`-v`, DEBUG).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Quiet,
    Normal,
    Verbose,
}

#[derive(Debug, Clone)]
pub struct Args {
    /// The input files (`-` alone for standard input), after Ruby's
    /// globbing; validated to exist.
    pub inputs: Vec<String>,
    /// The output format: `html` (Ruby's html5/xhtml backends), or
    /// adoctra's `json`/`docx`.
    pub format: String,
    /// `-o`: a file, or `-` for standard output.
    pub output: Option<String>,
    /// `-D`: Ruby's `:to_dir`.
    pub to_dir: Option<String>,
    /// `-R`: the source root whose structure `-D` keeps.
    pub source_dir: Option<String>,
    pub to_stdout: bool,
    pub embedded: bool,
    pub attributes: adoctra::Attributes,
    pub safe: adoctra::SafeMode,
    pub json_attributes: bool,
    pub json_locations: bool,
    pub json_references: bool,
    pub editor_diagnostics: bool,
    pub base_dir: Option<PathBuf>,
    pub failure_level: Level,
    pub verbosity: Verbosity,
    pub timings: bool,
    /// A `-b` backend adoctra cannot convert: the run fails like Ruby
    /// creating its converter.
    pub unavailable_backend: Option<String>,
    /// The input being converted (set per input by the caller).
    pub input: String,
}

/// What the command line asks for.
pub enum Command {
    /// The options are large; boxed to keep the other variants small.
    Convert(Box<Args>),
    /// `-h`: the usage on stdout, success.
    Help,
    /// `-V` (or a lone `-v`): the version on stdout, success.
    Version,
    /// No input: the usage on stderr, failure.
    Usage,
    /// A usage error: Ruby prints the message on stderr and the usage on
    /// stdout, and fails.
    Invalid(String),
    /// A missing or non-file input: the message on stderr, failure.
    Failed(String),
}

/// How an option takes its value.
#[derive(Clone, Copy, PartialEq)]
enum Value {
    None,
    Required,
    Optional,
    /// One of these (by unique prefix).
    Choice(&'static [&'static str]),
}

struct Spec {
    short: Option<char>,
    long: &'static str,
    value: Value,
}

const DOCTYPES: &[&str] = &["article", "book", "manpage", "inline"];
const SAFE_MODES: &[&str] = &["unsafe", "safe", "server", "secure"];
const ERUBY: &[&str] = &["erb", "erubi", "erubis"];
const FAILURE_LEVELS: &[&str] = &[
    "info", "INFO", "warning", "WARNING", "error", "ERROR", "fatal", "FATAL",
];

const SPECS: &[Spec] = &[
    spec(Some('b'), "backend", Value::Required),
    spec(Some('d'), "doctype", Value::Choice(DOCTYPES)),
    spec(Some('e'), "embedded", Value::None),
    spec(Some('o'), "out-file", Value::Required),
    spec(None, "safe", Value::None),
    spec(Some('S'), "safe-mode", Value::Choice(SAFE_MODES)),
    spec(Some('s'), "no-header-footer", Value::None),
    spec(Some('n'), "section-numbers", Value::None),
    spec(None, "eruby", Value::Choice(ERUBY)),
    spec(Some('a'), "attribute", Value::Required),
    spec(Some('T'), "template-dir", Value::Required),
    spec(Some('E'), "template-engine", Value::Required),
    spec(Some('B'), "base-dir", Value::Required),
    spec(Some('R'), "source-dir", Value::Required),
    spec(Some('D'), "destination-dir", Value::Required),
    spec(Some('I'), "load-path", Value::Required),
    spec(Some('r'), "require", Value::Required),
    spec(None, "failure-level", Value::Choice(FAILURE_LEVELS)),
    spec(Some('q'), "quiet", Value::None),
    spec(None, "trace", Value::None),
    spec(Some('v'), "verbose", Value::None),
    spec(Some('w'), "warnings", Value::None),
    spec(Some('t'), "timings", Value::None),
    spec(Some('h'), "help", Value::Optional),
    spec(Some('V'), "version", Value::None),
    // adoctra's own options.
    spec(None, "to", Value::Required),
    spec(None, "stdout", Value::None),
    spec(None, "json-attributes", Value::None),
    spec(None, "json-locations", Value::None),
    spec(None, "json-references", Value::None),
    spec(None, "diagnostics", Value::Choice(&["compat", "editor"])),
];

const fn spec(short: Option<char>, long: &'static str, value: Value) -> Spec {
    Spec { short, long, value }
}

/// Ruby's `OptionParser` completion: the exact name, else the only one
/// starting with it.
fn complete<'a>(key: &str, names: impl Iterator<Item = &'a str> + Clone) -> Result<&'a str, bool> {
    if let Some(exact) = names.clone().find(|name| *name == key) {
        return Ok(exact);
    }
    let mut matches = names.filter(|name| name.starts_with(key));
    match (matches.next(), matches.next()) {
        (Some(only), None) => Ok(only),
        (Some(_), Some(_)) => Err(true),
        _ => Err(false),
    }
}

struct State {
    args: Args,
    operands: Vec<String>,
}

/// The settings of one option, as Ruby applies them in order.
fn apply(state: &mut State, spec: &Spec, switch: &str, value: Option<String>) -> Option<Command> {
    let args = &mut state.args;
    let value = match spec.value {
        Value::Choice(choices) => {
            let value = value.unwrap_or_default();
            match complete(&value, choices.iter().copied()) {
                Ok(choice) => Some(choice.to_owned()),
                Err(true) => {
                    return Some(Command::Invalid(format!(
                        "ambiguous argument: {switch} {value}"
                    )))
                }
                Err(false) => {
                    return Some(Command::Invalid(format!(
                        "invalid argument: {switch} {value}"
                    )))
                }
            }
        }
        _ => value,
    };
    let value = value.unwrap_or_default();
    match spec.long {
        "backend" => match value.as_str() {
            "html5" | "html" | "xhtml5" | "xhtml" => {
                args.format = "html".into();
                set_attribute(&mut args.attributes, "backend", &value);
            }
            "json" | "docx" => args.format = value,
            _ => {
                args.unavailable_backend = Some(value.clone());
                set_attribute(&mut args.attributes, "backend", &value);
            }
        },
        "doctype" => set_attribute(&mut args.attributes, "doctype", &value),
        "embedded" | "no-header-footer" => args.embedded = true,
        "out-file" => args.output = Some(value),
        "safe" => args.safe = adoctra::SafeMode::Safe,
        "safe-mode" => {
            args.safe = adoctra::SafeMode::from_name(&value).expect("a listed safe mode")
        }
        "section-numbers" => set_attribute(&mut args.attributes, "sectnums", ""),
        "attribute" => {
            // Ruby skips blank entries and `=`.
            let entry = value.trim_end();
            if !entry.is_empty() && entry != "=" {
                let (name, value) = entry.split_once('=').unwrap_or((entry, ""));
                if !name.trim_matches(['!', '@']).is_empty() {
                    set_attribute(&mut args.attributes, name, value);
                }
            }
        }
        "template-dir" | "template-engine" | "load-path" | "require" => {
            return Some(Command::Failed(format!(
                "FAILED: option {switch} is not supported: templates and Ruby libraries are outside adoctra's scope"
            )))
        }
        "base-dir" => args.base_dir = Some(PathBuf::from(value)),
        "source-dir" => args.source_dir = Some(value),
        "destination-dir" => args.to_dir = Some(value),
        "failure-level" => {
            args.failure_level = match value.to_ascii_lowercase().as_str() {
                "info" => Level::Info,
                "warning" => Level::Warn,
                "error" => Level::Error,
                _ => Level::Fatal,
            }
        }
        "quiet" => args.verbosity = Verbosity::Quiet,
        "verbose" => args.verbosity = Verbosity::Verbose,
        "timings" => args.timings = true,
        // Ruby's interpreter warnings, backtraces and template engine.
        "warnings" | "trace" | "eruby" => {}
        "help" => return Some(Command::Help),
        "version" => return Some(Command::Version),
        "to" => args.format = value,
        "stdout" => args.to_stdout = true,
        "json-attributes" => args.json_attributes = true,
        "json-locations" => args.json_locations = true,
        "json-references" => args.json_references = true,
        "diagnostics" => args.editor_diagnostics = value == "editor",
        other => unreachable!("unhandled option {other}"),
    }
    None
}

/// Like Ruby's `-a`, `-b`, `-d` and `-n`, a later setting of a name wins,
/// whatever its spelling (`x!`, `x@`).
fn set_attribute(attributes: &mut adoctra::Attributes, name: &str, value: &str) {
    let previous: Vec<_> = attributes
        .iter()
        .filter(|(key, _)| override_name(key) == override_name(name))
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in previous {
        attributes.remove(&key);
    }
    attributes.set(name, value);
}

pub fn override_name(name: &str) -> String {
    let name = name.strip_suffix('@').unwrap_or(name);
    name.strip_prefix('!')
        .or_else(|| name.strip_suffix('!'))
        .unwrap_or(name)
        .to_lowercase()
}

/// Parse the arguments (without the program name).
pub fn parse(arguments: impl IntoIterator<Item = String>) -> Command {
    let mut state = State {
        args: Args {
            inputs: Vec::new(),
            format: "html".into(),
            output: None,
            to_dir: None,
            source_dir: None,
            to_stdout: false,
            embedded: false,
            attributes: adoctra::Attributes::new(),
            // adoctra's CLI defaults to safe mode (Ruby's to unsafe): paths
            // stay in the input's directory.
            safe: adoctra::SafeMode::Safe,
            json_attributes: false,
            json_locations: false,
            json_references: false,
            editor_diagnostics: false,
            base_dir: None,
            failure_level: Level::Fatal,
            verbosity: Verbosity::Normal,
            timings: false,
            unavailable_backend: None,
            input: String::new(),
        },
        operands: Vec::new(),
    };
    let mut it = arguments.into_iter().peekable();
    while let Some(arg) = it.next() {
        if arg == "--" {
            state.operands.extend(it.by_ref());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (key, inline) = match long.split_once('=') {
                Some((key, value)) => (key, Some(value.to_owned())),
                None => (long, None),
            };
            let spec = match complete(key, SPECS.iter().map(|spec| spec.long)) {
                Ok(name) => SPECS.iter().find(|spec| spec.long == name).expect("listed"),
                Err(true) => return Command::Invalid(format!("ambiguous option: --{key}")),
                Err(false) => return Command::Invalid(format!("invalid option: --{key}")),
            };
            let switch = format!("--{}", spec.long);
            let value = match spec.value {
                Value::None if inline.is_some() => {
                    return Command::Invalid(format!("needless argument: {arg}"))
                }
                Value::None => None,
                Value::Optional => inline.or_else(|| it.next_if(|next| !next.starts_with('-'))),
                Value::Required | Value::Choice(_) => match inline.or_else(|| it.next()) {
                    Some(value) => Some(value),
                    None => return Command::Invalid(format!("option missing argument: {switch}")),
                },
            };
            if let Some(command) = apply(&mut state, spec, &switch, value) {
                return command;
            }
            continue;
        }
        if arg.len() > 1 && arg.starts_with('-') {
            let cluster: Vec<char> = arg[1..].chars().collect();
            let mut index = 0;
            while index < cluster.len() {
                let letter = cluster[index];
                let Some(spec) = SPECS.iter().find(|spec| spec.short == Some(letter)) else {
                    return Command::Invalid(format!("invalid option: -{letter}"));
                };
                let switch = format!("-{letter}");
                let rest: String = cluster[index + 1..].iter().collect();
                let value = match spec.value {
                    Value::None => {
                        index += 1;
                        None
                    }
                    Value::Optional => {
                        index = cluster.len();
                        match rest.is_empty() {
                            false => Some(rest),
                            true => it.next_if(|next| !next.starts_with('-')),
                        }
                    }
                    Value::Required | Value::Choice(_) => {
                        index = cluster.len();
                        match rest.is_empty() {
                            false => Some(rest),
                            true => match it.next() {
                                Some(value) => Some(value),
                                None => {
                                    return Command::Invalid(format!(
                                        "option missing argument: {switch}"
                                    ))
                                }
                            },
                        }
                    }
                };
                if let Some(command) = apply(&mut state, spec, &switch, value) {
                    return command;
                }
            }
            continue;
        }
        state.operands.push(arg);
    }
    inputs(state)
}

/// Ruby's input files: `-` alone is standard input; another argument
/// starting with `-` warns; a file that does not exist is globbed (kept
/// when nothing matches); then each must be a readable file.
fn inputs(mut state: State) -> Command {
    if state.operands.is_empty() {
        return match state.args.verbosity {
            Verbosity::Verbose => Command::Version,
            _ => Command::Usage,
        };
    }
    let mut inputs = Vec::new();
    if state.operands.len() == 1 && state.operands[0] == "-" {
        inputs.push("-".to_owned());
    } else {
        for file in &state.operands {
            if file.starts_with('-') {
                // Ruby warns on stderr (not through its logger) and proceeds
                // with the other inputs.
                eprintln!(
                    "adoctra: warning: extra arguments detected (unparsed arguments: '{}') or incorrect usage of stdin",
                    state.operands.join("', '")
                );
            } else if std::path::Path::new(file).is_file() {
                inputs.push(file.clone());
            } else {
                match crate::glob::glob(file) {
                    matches if matches.is_empty() => inputs.push(file.clone()),
                    matches => inputs.extend(matches),
                }
            }
        }
    }
    for file in inputs.iter().filter(|file| *file != "-") {
        match std::fs::metadata(file) {
            Ok(meta) if meta.is_file() || is_pipe(&meta) => {}
            Ok(meta) => {
                let kind = if meta.is_dir() {
                    "directory"
                } else {
                    "special file"
                };
                return Command::Failed(format!(
                    "FAILED: input path {file} is a {kind}, not a file"
                ));
            }
            Err(_) => return Command::Failed(format!("FAILED: input file {file} is missing")),
        }
    }
    state.args.inputs = inputs;
    Command::Convert(Box::new(state.args))
}

#[cfg(unix)]
pub fn is_pipe(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt;
    meta.file_type().is_fifo()
}

#[cfg(not(unix))]
pub fn is_pipe(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Command {
        parse(args.iter().map(|arg| arg.to_string()))
    }

    fn invalid(args: &[&str]) -> String {
        match parse_strs(args) {
            Command::Invalid(message) => message,
            _ => panic!("expected a usage error for {args:?}"),
        }
    }

    fn converted(args: &[&str]) -> Args {
        match parse_strs(args) {
            Command::Convert(args) => *args,
            _ => panic!("expected a conversion for {args:?}"),
        }
    }

    #[test]
    fn option_parser_forms() {
        let args = converted(&["-sn", "-Ssecure", "-ofoo.html", "--em", "-d", "b", "-"]);
        assert!(args.embedded);
        assert_eq!(args.safe, adoctra::SafeMode::Secure);
        assert_eq!(args.output.as_deref(), Some("foo.html"));
        assert_eq!(args.attributes.get("doctype"), Some("book"));
        assert_eq!(args.attributes.get("sectnums"), Some(""));
        assert_eq!(args.inputs, ["-"]);
        let args = converted(&[
            "--safe-mode=server",
            "--failure-level",
            "warning",
            "-q",
            "-v",
            "-",
        ]);
        assert_eq!(args.safe, adoctra::SafeMode::Server);
        assert_eq!(args.failure_level, Level::Warn);
        assert_eq!(args.verbosity, Verbosity::Verbose);
        // -b picks the HTML converter for Ruby's names and sets the backend.
        let args = converted(&["-b", "xhtml5", "-a", "", "-a", "=", "-a", "x=1 ", "-"]);
        assert_eq!(args.format, "html");
        assert_eq!(args.attributes.get("backend"), Some("xhtml5"));
        assert_eq!(args.attributes.get("x"), Some("1"));
        assert_eq!(args.attributes.iter().count(), 2);
        let args = converted(&["-b", "unknownBackend", "-"]);
        assert_eq!(args.unavailable_backend.as_deref(), Some("unknownBackend"));
    }

    #[test]
    fn ruby_usage_errors() {
        assert_eq!(invalid(&["-d", "B", "a"]), "invalid argument: -d B");
        assert_eq!(invalid(&["-S", "s", "a"]), "ambiguous argument: -S s");
        assert_eq!(invalid(&["-S", "SAFE", "a"]), "invalid argument: -S SAFE");
        assert_eq!(invalid(&["a", "-o"]), "option missing argument: -o");
        assert_eq!(invalid(&["--bogus", "a"]), "invalid option: --bogus");
        assert_eq!(invalid(&["-sx", "a"]), "invalid option: -x");
        assert_eq!(invalid(&["--s", "a"]), "ambiguous option: --s");
        assert_eq!(
            invalid(&["--failure-level", "foo", "a"]),
            "invalid argument: --failure-level foo"
        );
        assert!(matches!(parse_strs(&[]), Command::Usage));
        assert!(matches!(parse_strs(&["-v"]), Command::Version));
        assert!(matches!(parse_strs(&["-V", "--bogus"]), Command::Version));
        assert!(matches!(parse_strs(&["a", "-h"]), Command::Help));
        assert!(matches!(
            parse_strs(&["-r", "x", "a"]),
            Command::Failed(message) if message.contains("-r is not supported")
        ));
        assert!(matches!(
            parse_strs(&["no/such.adoc"]),
            Command::Failed(message) if message == "FAILED: input file no/such.adoc is missing"
        ));
        assert!(matches!(
            parse_strs(&["."]),
            Command::Failed(message) if message == "FAILED: input path . is a directory, not a file"
        ));
    }
}
