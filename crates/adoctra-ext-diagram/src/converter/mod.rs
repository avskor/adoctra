//! Diagram converters (asciidoctor-diagram's `*Converter` classes) and the
//! shared helpers of `CliGenerator`, `Cli` and `find_command`.

pub(crate) mod graphviz;
pub(crate) mod http;
pub(crate) mod mermaid;
pub(crate) mod plantuml;

use crate::host::{Command, DiagramHost};
use crate::source::{expand_path, Inherit, Source};
use std::sync::atomic::{AtomicU64, Ordering};

/// A value of a converter's options hash (stored in the cache metadata and
/// compared to decide regeneration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OptValue {
    Null,
    Str(String),
    Bool(bool),
    Int(i64),
}

impl From<Option<String>> for OptValue {
    fn from(value: Option<String>) -> Self {
        value.map_or(OptValue::Null, OptValue::Str)
    }
}

pub(crate) type Options = Vec<(&'static str, OptValue)>;

/// Ruby's `JSON.dump` of the options hash.
pub(crate) fn options_json(options: &Options) -> String {
    let fields: Vec<String> = options
        .iter()
        .map(|(key, value)| {
            let value = match value {
                OptValue::Null => "null".to_owned(),
                OptValue::Bool(b) => b.to_string(),
                OptValue::Int(n) => n.to_string(),
                OptValue::Str(s) => json_string(s),
            };
            format!("{}:{value}", json_string(key))
        })
        .collect();
    format!("{{{}}}", fields.join(","))
}

pub(crate) fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A diagram converter.
pub(crate) trait Converter: Send + Sync {
    fn supported_formats(&self) -> &'static [&'static str];
    /// Whether the tool scales itself (no `scale` factor on the size).
    fn native_scaling(&self) -> bool {
        false
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String>;
    /// The code the tool receives (`wrap_source`), computed when converting.
    fn code(&self, source: &Source<'_, '_>, _host: &dyn DiagramHost) -> Result<String, String> {
        Ok(source.loaded_code().to_owned())
    }
    /// The code an HTTP server receives (the wrapped code without local
    /// preprocessing unless the converter needs it).
    fn server_code(
        &self,
        source: &Source<'_, '_>,
        host: &dyn DiagramHost,
    ) -> Result<String, String> {
        self.code(source, host)
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String>;
}

/// `find_command(cmd, attrs:/alt_attrs:, alt_cmds:, raise_on_error:)`.
pub(crate) struct Find<'n> {
    pub cmd: &'n str,
    pub attrs: Option<&'n [&'n str]>,
    pub alt_attrs: &'n [&'n str],
    pub alt_cmds: &'n [&'n str],
    pub raise: bool,
}

impl<'n> Find<'n> {
    pub fn new(cmd: &'n str) -> Self {
        Self {
            cmd,
            attrs: None,
            alt_attrs: &[],
            alt_cmds: &[],
            raise: true,
        }
    }
}

pub(crate) fn find_command(
    source: &Source<'_, '_>,
    host: &dyn DiagramHost,
    find: Find<'_>,
) -> Result<Option<String>, String> {
    let attr_names: Vec<&str> = match find.attrs {
        Some(attrs) => attrs.to_vec(),
        None => find
            .alt_attrs
            .iter()
            .copied()
            .chain(std::iter::once(find.cmd))
            .collect(),
    };
    let cmd_names: Vec<&str> = std::iter::once(find.cmd)
        .chain(find.alt_cmds.iter().copied())
        .collect();
    let cwd = crate::source::current_dir();
    let from_attrs = attr_names.iter().find_map(|name| {
        let value = source.attr_inherit(&[name], Inherit::Document)?;
        let path = expand_path(&value, &cwd);
        host.is_executable(&path).then_some(path)
    });
    let path = from_attrs.or_else(|| cmd_names.iter().find_map(|name| host.which(name)));
    if path.is_none() && find.raise {
        let names: Vec<String> = cmd_names.iter().map(|c| format!("'{c}'")).collect();
        return Err(format!(
            "Could not find the {} executable in PATH; add it to the PATH or specify its \
             location using the '{}' document attribute",
            names.join(", "),
            attr_names[0]
        ));
    }
    Ok(path)
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `Cli.run`: a non-zero exit is `"<tool> failed: <stderr or stdout>"`.
pub(crate) fn run(
    host: &dyn DiagramHost,
    command: &Command,
) -> Result<crate::host::Output, String> {
    let output = host
        .run(command)
        .map_err(|e| format!("{}: {e}", command.program))?;
    if output.status != 0 {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "{} failed: {}",
            basename(&command.program),
            if stderr.is_empty() { stdout } else { stderr }
        ));
    }
    Ok(output)
}

/// `generate`: the tool's output must not be empty.
fn generated(
    command: &Command,
    data: Vec<u8>,
    output: &crate::host::Output,
) -> Result<Vec<u8>, String> {
    if data.is_empty() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} failed: {}",
            command.program,
            if stdout.is_empty() { stderr } else { stdout }
        ));
    }
    Ok(data)
}

static TEMP: AtomicU64 = AtomicU64::new(0);

/// A fresh temporary path like Ruby's `Tempfile.new([prefix, suffix])`.
fn temp_path(host: &dyn DiagramHost, prefix: &str, suffix: &str) -> String {
    let n = TEMP.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    host.temp_dir()
        .join(format!(
            "{prefix}{}-{n}-{nanos}{suffix}",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned()
}

/// `generate_stdin`: the code on stdin, the result in a temporary file the
/// arguments name.
pub(crate) fn generate_stdin(
    host: &dyn DiagramHost,
    tool: &str,
    format: &str,
    code: &str,
    build: impl FnOnce(&str) -> Command,
) -> Result<Vec<u8>, String> {
    let target = temp_path(host, basename(tool), &format!(".{format}"));
    let mut command = build(&target);
    command.stdin = Some(code.as_bytes().to_vec());
    let result = run(host, &command).and_then(|output| {
        let data = host.read(&target).unwrap_or_default();
        generated(&command, data, &output)
    });
    host.remove(&target);
    result
}

/// `generate_stdin_stdout`: the code on stdin, the result on stdout.
pub(crate) fn generate_stdin_stdout(
    host: &dyn DiagramHost,
    code: &str,
    mut command: Command,
) -> Result<Vec<u8>, String> {
    command.stdin = Some(code.as_bytes().to_vec());
    let output = run(host, &command)?;
    let data = output.stdout.clone();
    generated(&command, data, &output)
}

/// `generate_file`: the code in a temporary input file, the result in a
/// temporary output file, both named by the arguments.
pub(crate) fn generate_file(
    host: &dyn DiagramHost,
    tool: &str,
    input_ext: &str,
    output_ext: &str,
    code: &str,
    build: impl FnOnce(&str, &str) -> Result<Command, String>,
) -> Result<Vec<u8>, String> {
    let input = temp_path(host, basename(tool), &format!(".{input_ext}"));
    host.write(&input, code.as_bytes())
        .map_err(|e| format!("{input}: {e}"))?;
    let target = temp_path(host, basename(tool), &format!(".{output_ext}"));
    let result = build(&input, &target).and_then(|command| {
        let output = run(host, &command)?;
        let data = host.read(&target).unwrap_or_default();
        generated(&command, data, &output)
    });
    host.remove(&target);
    host.remove(&input);
    host.remove(&format!("{input}.json"));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_dump_like_ruby_json() {
        let options: Options = vec![
            ("layout", OptValue::Null),
            ("size_limit", OptValue::Str("4096".into())),
            ("smetana", OptValue::Bool(true)),
            ("css", OptValue::Str("a\"b".into())),
        ];
        assert_eq!(
            options_json(&options),
            r#"{"layout":null,"size_limit":"4096","smetana":true,"css":"a\"b"}"#
        );
    }
}
