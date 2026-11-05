//! asciidoctor-diagram's converters that run one command-line tool through
//! `CliGenerator` (`generate_stdin`, `generate_stdin_stdout`,
//! `generate_file`, `generate_file_stdout`, `generate_stdin_file`), each a
//! port of its `<type>/converter.rb`.

use super::{
    find_command, generate_file, generate_file_stdout, generate_stdin, generate_stdin_file,
    generate_stdin_stdout, java, option, temp_path, truthy, Converter, Find, Gen, OptValue,
    Options,
};
use crate::host::{Command, DiagramHost};
use crate::source::{basename_without_ext, parent_dir, Source};
use adoctra_core::SafeMode;

/// A command with `:args` (the tool first) and `:chdir`.
fn command(args: Vec<String>, chdir: Option<String>) -> Command {
    let mut args = args.into_iter();
    Command {
        program: args.next().unwrap_or_default(),
        args: args.collect(),
        chdir,
        ..Command::default()
    }
}

fn strings<const N: usize>(items: [&str; N]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn find(source: &Source<'_, '_>, host: &dyn DiagramHost, cmd: &str) -> Result<String, String> {
    Ok(find_command(source, host, Find::new(cmd))?.unwrap_or_default())
}

/// `options[:name]` as Ruby's `JSON.dump` keeps it.
fn opt(source: &Source<'_, '_>, name: &str) -> OptValue {
    source.attr(name).into()
}

macro_rules! formats {
    ($($format:literal),*) => {
        fn supported_formats(&self) -> &'static [&'static str] {
            &[$($format),*]
        }
    };
}

/// `AsciiToSvgConverter` (`a2s`).
pub(crate) struct AsciiToSvg;

impl Converter for AsciiToSvg {
    formats!("svg", "txt");
    fn native_scaling(&self) -> bool {
        true
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("sx", opt(source, "scalex")),
            ("sy", opt(source, "scaley")),
            ("scale", opt(source, "scale")),
            (
                "noblur",
                OptValue::Bool(source.attr("noblur").as_deref() == Some("true")),
            ),
            ("font", opt(source, "fontfamily")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let code = self.code(source, host)?;
        if format == "txt" {
            return Ok(code.into_bytes());
        }
        let tool = find(source, host, "a2s")?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![tool.clone(), "-o".to_owned(), output.to_owned()];
            match (
                option(options, "sx"),
                option(options, "sy"),
                option(options, "scale"),
            ) {
                (Some(sx), Some(sy), _) => args.extend(["-s".to_owned(), format!("{sx},{sy}")]),
                (_, _, Some(scale)) => args.extend(["-s".to_owned(), format!("{scale},{scale}")]),
                _ => {}
            }
            if truthy(options, "noblur") {
                args.push("-b".to_owned());
            }
            if let Some(font) = option(options, "font") {
                args.extend(["-f".to_owned(), font.to_owned()]);
            }
            command(args, Some(source.base_dir()))
        })
    }
}

/// The blockdiag family (`BlockDiagConverter`, `SeqDiagConverter`, …):
/// `<tool>` or `<tool>3`.
pub(crate) struct BlockDiag {
    pub tool: &'static str,
    pub alt_tool: &'static str,
}

impl Converter for BlockDiag {
    formats!("png", "pdf", "svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![("font_path", opt(source, "fontpath"))])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find_command(
            source,
            host,
            Find {
                alt_cmds: &[self.alt_tool],
                ..Find::new(self.tool)
            },
        )?
        .unwrap_or_default();
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![
                tool.clone(),
                "-a".to_owned(),
                "-o".to_owned(),
                output.to_owned(),
                format!("-T{format}"),
            ];
            if let Some(font) = option(options, "font_path") {
                args.push(format!("-f{font}"));
            }
            args.push("-".to_owned());
            command(args, Some(source.base_dir()))
        })
    }
}

/// `BpmnConverter` (`bpmn-js`).
pub(crate) struct Bpmn;

impl Converter for Bpmn {
    formats!("png", "svg", "pdf", "jpeg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("width", opt(source, "width")),
            ("height", opt(source, "height")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "bpmn-js")?;
        let code = self.code(source, host)?;
        generate_file(
            host,
            &tool,
            "bpmn",
            format,
            code.as_bytes(),
            |input, output| {
                let mut args = vec![
                    tool.clone(),
                    input.to_owned(),
                    "-o".to_owned(),
                    output.to_owned(),
                    "-t".to_owned(),
                    format.to_owned(),
                ];
                if let Some(width) = option(options, "width") {
                    args.extend(["--width".to_owned(), width.to_owned()]);
                }
                if let Some(height) = option(options, "height") {
                    args.extend(["--height".to_owned(), height.to_owned()]);
                }
                Ok(command(args, Some(source.base_dir())))
            },
        )
    }
}

/// `BytefieldConverter` (`bytefield-svg`).
pub(crate) struct Bytefield;

impl Converter for Bytefield {
    formats!("svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "bytefield-svg")?;
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            command(
                vec![tool.clone(), "--output".to_owned(), output.to_owned()],
                Some(source.base_dir()),
            )
        })
    }
}

/// `D2Converter`: each option a `--flag` (`sketch` without a value unless
/// it is `false`, fonts as paths).
pub(crate) struct D2;

impl Converter for D2 {
    formats!("svg", "png", "pdf");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("layout", opt(source, "layout")),
            ("theme", opt(source, "theme")),
            ("pad", opt(source, "pad")),
            ("animate_interval", opt(source, "animate-interval")),
            ("sketch", opt(source, "sketch")),
            ("font_regular", opt(source, "font-regular")),
            ("font_italic", opt(source, "font-italic")),
            ("font_bold", opt(source, "font-bold")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "d2")?;
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = strings(["", "--browser", "false"]);
            args[0] = tool.clone();
            for (key, value) in options {
                let OptValue::Str(value) = value else {
                    continue;
                };
                let flag = format!("--{}", key.replace('_', "-"));
                if *key == "sketch" && value != "false" {
                    args.push(flag);
                } else {
                    args.extend([flag, value.clone()]);
                }
            }
            args.extend(["-".to_owned(), output.to_owned()]);
            command(args, Some(source.base_dir()))
        })
    }
}

/// `DbmlConverter` (`dbml-renderer`, stdin to stdout).
pub(crate) struct Dbml;

impl Converter for Dbml {
    formats!("svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        _: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "dbml-renderer")?;
        let code = self.code(source, host)?;
        generate_stdin_stdout(
            host,
            code.as_bytes(),
            command(vec![tool], Some(source.base_dir())),
        )
    }
}

/// `DiagramsConverter`: Python with `filename` and `outformat` added to
/// the first `Diagram(…)` call.
pub(crate) struct Diagrams;

impl Converter for Diagrams {
    formats!("png", "svg", "pdf");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let python = find_command(
            source,
            host,
            Find {
                attrs: Some(&["diagrams-python"]),
                alt_cmds: &["python"],
                ..Find::new("python3")
            },
        )?
        .unwrap_or_default();
        let code = self.code(source, host)?;
        // `/Diagram\((.*?)\)/`: `.` does not cross a line feed.
        let (start, args_start, args_end) = code
            .match_indices("Diagram(")
            .find_map(|(at, call)| {
                let from = at + call.len();
                let line = &code[from..];
                let line = &line[..line.find('\n').unwrap_or(line.len())];
                line.find(')').map(|close| (at, from, from + close))
            })
            .ok_or_else(|| "Could not find Diagram constructor".to_owned())?;
        // Ruby's `strip` (whitespace and trailing NUL).
        let args = code[args_start..args_end]
            .trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b')
            .trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b' || c == '\0');
        let target = temp_path(host, "diagrams", "");
        let mut diagram = format!("Diagram({args}");
        if !args.is_empty() {
            diagram.push(',');
        }
        diagram.push_str(&format!("filename=\"{target}\",outformat=\"{format}\")"));
        let code = format!("{}{diagram}{}", &code[..start], &code[args_end + 1..]);
        let result =
            generate_stdin_file(host, code.as_bytes(), &format!("{target}.{format}"), || {
                command(
                    vec![python.clone(), "-".to_owned()],
                    Some(source.base_dir()),
                )
            });
        host.remove(&format!("{target}.{format}"));
        result
    }
}

/// `DpicConverter`: the code between `.PS` and `.PE`, as a file, SVG on
/// stdout.
pub(crate) struct Dpic;

impl Converter for Dpic {
    formats!("svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "dpic")?;
        let mut code = self.code(source, host)?;
        if !code.starts_with(".PS\n") {
            code.insert_str(0, ".PS\n");
        }
        // Ruby tests `start_with?("\n.PE")`, so `.PE` is nearly always added.
        if !code.starts_with("\n.PE") {
            code.push_str("\n.PE");
        }
        generate_file_stdout(host, &tool, format, code.as_bytes(), |input| {
            command(
                vec![
                    tool.clone(),
                    "-v".to_owned(),
                    "-z".to_owned(),
                    input.to_owned(),
                ],
                Some(source.base_dir()),
            )
        })
    }
}

/// `ErdConverter`: `erd` (or `erd-go`) writes dot, Graphviz renders it.
pub(crate) struct Erd;

impl Converter for Erd {
    formats!("png", "svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let erd = find_command(
            source,
            host,
            Find {
                alt_cmds: &["erd-go"],
                ..Find::new("erd")
            },
        )?
        .unwrap_or_default();
        let dot = find_command(
            source,
            host,
            Find {
                alt_attrs: &["graphvizdot"],
                ..Find::new("dot")
            },
        )?
        .unwrap_or_default();
        let code = self.code(source, host)?;
        // An array of arguments: no `chdir`.
        let dot_code = generate_stdin(host, &erd, format, code.as_bytes(), |output| {
            command(
                vec![
                    erd.clone(),
                    "-o".to_owned(),
                    output.to_owned(),
                    "-f".to_owned(),
                    "dot".to_owned(),
                ],
                None,
            )
        })?;
        generate_stdin(host, &dot, format, &dot_code, |output| {
            command(
                vec![dot.clone(), format!("-o{output}"), format!("-T{format}")],
                Some(source.base_dir()),
            )
        })
    }
}

/// `GnuplotConverter`: a `set term` line before the code, the image on
/// stdout (`dumb` for text).
pub(crate) struct Gnuplot;

impl Converter for Gnuplot {
    formats!("png", "svg", "gif", "txt", "literal");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("width", opt(source, "width")),
            ("height", opt(source, "height")),
            ("transparent", opt(source, "transparent")),
            ("crop", opt(source, "crop")),
            ("font", opt(source, "font")),
            ("fontscale", opt(source, "fontscale")),
            ("background", opt(source, "background")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let terminal = match format {
            "txt" | "literal" => "dumb",
            format => format,
        };
        let mut code = format!("set term {terminal}");
        if let (Some(width), Some(height)) = (option(options, "width"), option(options, "height")) {
            code.push_str(&format!(" size {width},{height}"));
        }
        // Any value is truthy in Ruby, `false` included.
        if option(options, "transparent").is_some() {
            code.push_str(" transparent");
        }
        if option(options, "crop").is_some() {
            code.push_str(" crop");
        }
        if let Some(font) = option(options, "font") {
            code.push_str(&format!(" font \"{font}\""));
        }
        if let Some(scale) = option(options, "fontscale") {
            code.push_str(&format!(" fontscale {scale}"));
        }
        if let Some(background) = option(options, "background") {
            code.push_str(&format!(" background \"{background}\""));
        }
        code.push('\n');
        code.push_str(&self.code(source, host)?);
        code.push('\n');
        let tool = find(source, host, "gnuplot")?;
        generate_stdin_stdout(
            host,
            code.as_bytes(),
            command(vec![tool], Some(source.base_dir())),
        )
    }
}

/// `GoATConverter` (`goat`, stdin to stdout).
pub(crate) struct GoAT;

impl Converter for GoAT {
    formats!("svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("sds", opt(source, "svg-color-dark-scheme")),
            ("sls", opt(source, "svg-color-light-scheme")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        _: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "goat")?;
        let mut args = vec![tool];
        if let Some(sds) = option(options, "sds") {
            args.extend(["-sds".to_owned(), sds.to_owned()]);
        }
        if let Some(sls) = option(options, "sls") {
            args.extend(["-sls".to_owned(), sls.to_owned()]);
        }
        let code = self.code(source, host)?;
        generate_stdin_stdout(
            host,
            code.as_bytes(),
            command(args, Some(source.base_dir())),
        )
    }
}

/// `GraphvizPyConverter` (`graphviz-py`).
pub(crate) struct GraphvizPy;

impl Converter for GraphvizPy {
    formats!("png", "pdf", "svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("layout", opt(source, "layout")),
            ("argument", opt(source, "argument")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "graphviz-py")?;
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![tool.clone(), format!("-o{output}"), format!("-T{format}")];
            if let Some(layout) = option(options, "layout") {
                args.push(format!("-K{layout}"));
            }
            if let Some(argument) = option(options, "argument") {
                args.extend(["-a".to_owned(), argument.to_owned()]);
            }
            command(args, Some(source.base_dir()))
        })
    }
}

/// `LilypondConverter`: a paper block without headers and footers, the
/// cropped image (`<output>.cropped.<format>`).
pub(crate) struct Lilypond;

const LILYPOND_PAPER: &str = "\\paper{\n  oddFooterMarkup=##f\n  oddHeaderMarkup=##f\n  \
     bookTitleMarkup=##f\n  scoreTitleMarkup=##f\n}\n\n";

impl Converter for Lilypond {
    formats!("png", "pdf");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![("resolution", opt(source, "resolution"))])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let code = format!("{LILYPOND_PAPER}{}", self.code(source, host)?);
        let tool = find(source, host, "lilypond")?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![
                tool.clone(),
                "-daux-files=#f".to_owned(),
                "-dbackend=eps".to_owned(),
                "-dno-gs-load-fonts".to_owned(),
                "-dinclude-eps-fonts".to_owned(),
                "-o".to_owned(),
                output.to_owned(),
                "-f".to_owned(),
                format.to_owned(),
                "-dcrop=#t".to_owned(),
            ];
            if let Some(resolution) = option(options, "resolution") {
                args.push(format!("-dresolution={resolution}"));
            }
            if format == "png" {
                args.push("-dpixmap-format=pngalpha".to_owned());
            }
            args.push("-".to_owned());
            Gen {
                command: command(args, Some(source.base_dir())),
                out_file: Some(format!("{output}.cropped.{format}")),
            }
        })
    }
}

/// `MscgenConverter` (`mscgen` or `mscgen_js`).
pub(crate) struct Msc;

impl Converter for Msc {
    formats!("png", "svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![("font", opt(source, "font"))])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find_command(
            source,
            host,
            Find {
                alt_cmds: &["mscgen_js"],
                ..Find::new("mscgen")
            },
        )?
        .unwrap_or_default();
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![
                tool.clone(),
                "-o".to_owned(),
                output.to_owned(),
                "-T".to_owned(),
                format.to_owned(),
            ];
            if let Some(font) = option(options, "font") {
                args.extend(["-F".to_owned(), font.to_owned()]);
            }
            args.push("-".to_owned());
            command(args, Some(source.base_dir()))
        })
    }
}

/// `NomnomlConverter` (input and output files).
pub(crate) struct Nomnoml;

impl Converter for Nomnoml {
    formats!("svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "nomnoml")?;
        let code = self.code(source, host)?;
        generate_file(
            host,
            &tool,
            "nomnoml",
            format,
            code.as_bytes(),
            |input, output| {
                Ok(command(
                    vec![tool.clone(), input.to_owned(), output.to_owned()],
                    Some(source.base_dir()),
                ))
            },
        )
    }
}

/// `OxdrawConverter`: options as flags; SVG sizes scaled here, as Ruby does.
pub(crate) struct Oxdraw;

impl Converter for Oxdraw {
    formats!("svg", "png");
    fn native_scaling(&self) -> bool {
        true
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("background", opt(source, "background")),
            ("scale", opt(source, "scale")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "oxdraw")?;
        let code = self.code(source, host)?;
        let data = generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![tool.clone()];
            for (key, value) in options {
                if let OptValue::Str(value) = value {
                    args.extend([format!("--{}", key.replace('_', "-")), value.clone()]);
                }
            }
            args.extend([
                "--input".to_owned(),
                "-".to_owned(),
                "--output".to_owned(),
                output.to_owned(),
                "--output-format".to_owned(),
                format.to_owned(),
                "--quiet".to_owned(),
            ]);
            command(args, Some(source.base_dir()))
        })?;
        match option(options, "scale") {
            Some(scale) if format == "svg" => {
                let scale = ruby_to_f(scale);
                let text = String::from_utf8_lossy(&data).into_owned();
                let text = scale_svg_attribute(&text, "width", scale);
                Ok(scale_svg_attribute(&text, "height", scale).into_bytes())
            }
            _ => Ok(data),
        }
    }
}

/// Ruby's `String#to_f`: the leading decimal number, else 0.
fn ruby_to_f(text: &str) -> f64 {
    let text = text.trim_start();
    let mut end = 0;
    let bytes = text.as_bytes();
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b'_')
            .count()
    };
    end += digits(end);
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end += 1 + digits(end + 1);
    }
    text[..end].replace('_', "").parse().unwrap_or(0.0)
}

/// `data.sub!(/(<svg.*NAME=")([0-9]+)(".*>)/) { $1 + ($2.to_i * scale).to_i.to_s + $3 }`:
/// the first `<svg` whose line has a `NAME="<digits>"` followed later on the
/// line by `>`; greedy `.*` takes the last such attribute.
fn scale_svg_attribute(text: &str, name: &str, scale: f64) -> String {
    let needle = format!("{name}=\"");
    for (start, _) in text.match_indices("<svg") {
        let line_end = text[start..].find('\n').map_or(text.len(), |at| start + at);
        let line = &text[start..line_end];
        let found = line.rmatch_indices(needle.as_str()).find_map(|(at, _)| {
            let digits_start = at + needle.len();
            let digits = line[digits_start..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            let close = digits_start + digits;
            (digits > 0 && line[close..].starts_with('"') && line[close + 1..].contains('>'))
                .then_some((digits_start, close))
        });
        if let Some((from, to)) = found {
            let value: f64 = line[from..to].parse().unwrap_or(0.0);
            let scaled = (value * scale) as i64;
            return format!("{}{scaled}{}", &text[..start + from], &text[start + to..]);
        }
    }
    text.to_owned()
}

/// `PenroseConverter` (`roger trio` with the domain and style files).
pub(crate) struct Penrose;

impl Converter for Penrose {
    formats!("svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("domain", opt(source, "domain_file")),
            ("style", opt(source, "style_file")),
            ("variation", opt(source, "variation")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let domain = option(options, "domain").ok_or("Domain file is required")?;
        let style = option(options, "style").ok_or("Style file is required")?;
        let tool = find(source, host, "roger")?;
        let code = self.code(source, host)?;
        generate_file(
            host,
            &tool,
            "substance",
            format,
            code.as_bytes(),
            |input, output| {
                let mut args = vec![
                    tool.clone(),
                    "trio".to_owned(),
                    "-o".to_owned(),
                    output.to_owned(),
                ];
                if let Some(variation) = option(options, "variation") {
                    args.extend(["-v".to_owned(), variation.to_owned()]);
                }
                args.extend([
                    "--path".to_owned(),
                    "/".to_owned(),
                    "--trio".to_owned(),
                    input.to_owned(),
                    source.resolve_path(domain, None),
                    source.resolve_path(style, None),
                    "--".to_owned(),
                ]);
                Ok(command(args, Some(source.base_dir())))
            },
        )
    }
}

/// `PikchrConverter`: SVG on stdout, anything else is the error text
/// without its tags.
pub(crate) struct Pikchr;

impl Converter for Pikchr {
    formats!("svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "pikchr")?;
        let code = self.code(source, host)?;
        let output = generate_file_stdout(host, &tool, format, code.as_bytes(), |input| {
            command(
                vec![tool.clone(), "--svg-only".to_owned(), input.to_owned()],
                Some(source.base_dir()),
            )
        })?;
        if output.starts_with(b"<svg") {
            Ok(output)
        } else {
            Err(strip_tags(&String::from_utf8_lossy(&output)))
        }
    }
}

/// `gsub(/<\/?[a-z]+>\n?/i, '')`.
fn strip_tags(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let after_slash = tail.strip_prefix('/').unwrap_or(tail);
        let letters = after_slash
            .bytes()
            .take_while(u8::is_ascii_alphabetic)
            .count();
        if letters > 0 && after_slash[letters..].starts_with('>') {
            let consumed = (tail.len() - after_slash.len()) + letters + 1;
            rest = &tail[consumed..];
            rest = rest.strip_prefix('\n').unwrap_or(rest);
        } else {
            out.push('<');
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// `PintoraConverter` (`pintora render`).
pub(crate) struct Pintora;

impl Converter for Pintora {
    formats!("png", "svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("width", opt(source, "width")),
            ("theme", opt(source, "theme")),
            ("background_color", opt(source, "background-color")),
            ("pixel_ratio", opt(source, "pixel-ratio")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "pintora")?;
        let code = self.code(source, host)?;
        generate_file(
            host,
            &tool,
            "pintora",
            format,
            code.as_bytes(),
            |input, output| {
                let mut args = vec![
                    tool.clone(),
                    "render".to_owned(),
                    "-i".to_owned(),
                    input.to_owned(),
                    "-o".to_owned(),
                    output.to_owned(),
                ];
                for (key, flag) in [
                    ("width", "-w"),
                    ("theme", "-t"),
                    ("pixel_ratio", "-p"),
                    ("background_color", "-b"),
                ] {
                    if let Some(value) = option(options, key) {
                        args.extend([flag.to_owned(), value.to_owned()]);
                    }
                }
                Ok(command(args, Some(source.base_dir())))
            },
        )
    }
}

/// `ShaapeConverter`.
pub(crate) struct Shaape;

impl Converter for Shaape {
    formats!("png", "svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "shaape")?;
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            command(
                vec![
                    tool.clone(),
                    "-o".to_owned(),
                    output.to_owned(),
                    "-t".to_owned(),
                    format.to_owned(),
                    "-".to_owned(),
                ],
                Some(source.base_dir()),
            )
        })
    }
}

/// `SmcatConverter` (state-machine-cat).
pub(crate) struct Smcat;

impl Converter for Smcat {
    formats!("svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            ("direction", opt(source, "direction")),
            ("engine", opt(source, "engine")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "smcat")?;
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![
                tool.clone(),
                "-o".to_owned(),
                output.to_owned(),
                "-T".to_owned(),
                format.to_owned(),
            ];
            if let Some(direction) = option(options, "direction") {
                args.extend(["-d".to_owned(), direction.to_owned()]);
            }
            if let Some(engine) = option(options, "engine") {
                args.extend(["-E".to_owned(), engine.to_owned()]);
            }
            args.push("-".to_owned());
            command(args, Some(source.base_dir()))
        })
    }
}

/// Options read as `attr(name) || attr(name, nil, '<type>-option')`.
fn prefixed_option(source: &Source<'_, '_>, name: &str, prefix: &str) -> OptValue {
    source
        .attr(name)
        .or_else(|| source.attr_inherit(&[name], crate::source::Inherit::Prefix(prefix)))
        .into()
}

/// `SvgbobConverter` (`svgbob` or `svgbob_cli`).
pub(crate) struct Svgbob;

impl Converter for Svgbob {
    formats!("svg", "txt");
    fn native_scaling(&self) -> bool {
        true
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        Ok(vec![
            (
                "font_family",
                prefixed_option(source, "font-family", "svgbob-option"),
            ),
            (
                "font_size",
                prefixed_option(source, "font-size", "svgbob-option"),
            ),
            (
                "stroke_width",
                prefixed_option(source, "stroke-width", "svgbob-option"),
            ),
            ("scale", prefixed_option(source, "scale", "svgbob-option")),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let code = self.code(source, host)?;
        if format == "txt" {
            return Ok(code.into_bytes());
        }
        let mut flags = Vec::new();
        for (key, flag) in [
            ("font_family", "--font-family"),
            ("font_size", "--font-size"),
            ("stroke_width", "--stroke-width"),
            ("scale", "--scale"),
        ] {
            if let Some(value) = option(options, key) {
                flags.extend([flag.to_owned(), value.to_owned()]);
            }
        }
        let tool = find_command(
            source,
            host,
            Find {
                alt_cmds: &["svgbob_cli"],
                ..Find::new("svgbob")
            },
        )?
        .unwrap_or_default();
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            let mut args = vec![tool.clone(), "-o".to_owned(), output.to_owned()];
            args.extend(flags);
            command(args, Some(source.base_dir()))
        })
    }
}

/// `SymbolatorConverter`.
pub(crate) struct Symbolator;

impl Converter for Symbolator {
    formats!("png", "pdf", "svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let tool = find(source, host, "symbolator")?;
        let code = self.code(source, host)?;
        generate_stdin(host, &tool, format, code.as_bytes(), |output| {
            command(
                vec![
                    tool.clone(),
                    "-i-".to_owned(),
                    format!("-o{output}"),
                    format!("-f{format}"),
                ],
                Some(source.base_dir()),
            )
        })
    }
}

/// `SyntraxConverter` through the `syntrax` command (without the JSyntrax
/// JARs).
pub(crate) struct Syntrax;

impl Converter for Syntrax {
    formats!("png", "svg");
    fn native_scaling(&self) -> bool {
        true
    }
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        let style = source.attr("style-file").or_else(|| {
            source.attr_inherit(
                &[&format!("{}-style", source.diagram_type)],
                crate::source::Inherit::Document,
            )
        });
        Ok(vec![
            ("heading", opt(source, "heading")),
            ("scale", opt(source, "scale")),
            ("transparent", opt(source, "transparent")),
            ("style", style.into()),
        ])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let mut shared = Vec::new();
        if let Some(title) = option(options, "heading") {
            shared.extend(["--title".to_owned(), title.to_owned()]);
        }
        if let Some(scale) = option(options, "scale") {
            shared.extend(["--scale".to_owned(), scale.to_owned()]);
        }
        if option(options, "transparent") == Some("true") {
            shared.push("--transparent".to_owned());
        }
        if let Some(style) = option(options, "style") {
            shared.extend(["--style".to_owned(), style.to_owned()]);
        }
        let tool = find(source, host, "syntrax")?;
        let code = self.code(source, host)?;
        generate_file(
            host,
            &tool,
            "spec",
            format,
            code.as_bytes(),
            |input, output| {
                let mut args = vec![
                    tool.clone(),
                    "-i".to_owned(),
                    input.to_owned(),
                    "-o".to_owned(),
                    output.to_owned(),
                ];
                args.extend(shared);
                Ok(command(args, Some(source.base_dir())))
            },
        )
    }
}

/// `TikZConverter`: a standalone LaTeX document through `pdflatex`, then
/// `pdf2svg` for SVG.
pub(crate) struct TikZ;

const TIKZ_HEAD: &str = "\\documentclass[border=2bp, tikz]{standalone}\n\\usepackage{tikz}\n";
const TIKZ_BEGIN: &str =
    "\\begin{document}\n\\begingroup\n\\tikzset{every picture/.style={scale=1}}\n";
const TIKZ_END: &str = "\\endgroup\n\\end{document}\n";

impl Converter for TikZ {
    formats!("pdf", "svg");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        // `opt('preamble') || attr('preamble') == 'true'`.
        let preamble = match source.attr("preamble-option") {
            Some(value) => OptValue::Str(value),
            None => OptValue::Bool(source.attr("preamble").as_deref() == Some("true")),
        };
        Ok(vec![("preamble", preamble)])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let latex_path = find(source, host, "pdflatex")?;
        let svg_path = if format == "svg" {
            Some(find(source, host, "pdf2svg")?)
        } else {
            None
        };
        let code = self.code(source, host)?;
        let (preamble, body) = if truthy(options, "preamble") {
            split_preamble(&code)
        } else {
            (String::new(), code)
        };
        let latex = format!("{TIKZ_HEAD}{preamble}{TIKZ_BEGIN}{body}{TIKZ_END}");
        let pdf = generate_file(
            host,
            &latex_path,
            "tex",
            "pdf",
            latex.as_bytes(),
            |input, output| {
                Ok(Gen {
                    command: command(
                        vec![
                            latex_path.clone(),
                            "-shell-escape".to_owned(),
                            "-file-line-error".to_owned(),
                            "-interaction=nonstopmode".to_owned(),
                            "-output-directory".to_owned(),
                            parent_dir(output),
                            input.to_owned(),
                        ],
                        None,
                    ),
                    out_file: Some(format!(
                        "{}/{}.pdf",
                        parent_dir(input),
                        basename_without_ext(input)
                    )),
                })
            },
        )?;
        match svg_path {
            Some(svg_path) => {
                generate_file(host, &svg_path, "pdf", "svg", &pdf, |input, output| {
                    Ok(command(
                        vec![svg_path.clone(), input.to_owned(), output.to_owned()],
                        Some(source.base_dir()),
                    ))
                })
            }
            None => Ok(pdf),
        }
    }
}

/// `code.split(/^~~~~$/, 2)`: the preamble before a `~~~~` line, or all of
/// the code as the body.
fn split_preamble(code: &str) -> (String, String) {
    let mut at = 0;
    while let Some(found) = code[at..].find("~~~~") {
        let start = at + found;
        let end = start + 4;
        let line_start = start == 0 || code.as_bytes()[start - 1] == b'\n';
        let line_end = end == code.len() || code.as_bytes()[end] == b'\n';
        if line_start && line_end {
            return (code[..start].to_owned(), code[end..].to_owned());
        }
        at = start + 1;
    }
    (String::new(), code.to_owned())
}

/// `UmletConverter`: `java -jar umlet.jar -action=convert …`.
pub(crate) struct Umlet;

impl Converter for Umlet {
    formats!("svg", "png", "pdf", "gif");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let java = java(host)?;
        let mut umlet = find(source, host, "umlet")?;
        if extname(&umlet) != ".jar" {
            umlet = crate::source::expand_path(
                &format!("{}.jar", basename_without_ext(&umlet)),
                &parent_dir(&umlet),
            );
        }
        let code = self.code(source, host)?;
        generate_file(
            host,
            &java,
            "uxf",
            format,
            code.as_bytes(),
            |input, output| {
                Ok(command(
                    vec![
                        java.clone(),
                        "-jar".to_owned(),
                        umlet.clone(),
                        "-action=convert".to_owned(),
                        format!("-format={format}"),
                        format!("-filename={input}"),
                        format!("-output={output}"),
                    ],
                    Some(source.base_dir()),
                ))
            },
        )
    }
}

/// Ruby's `File.extname`.
fn extname(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rfind('.') {
        Some(at) if at > 0 && at + 1 < base.len() => &base[at..],
        _ => "",
    }
}

/// `VegaConverter` (`vega` and `vegalite`): Vega-Lite through `vl2vg`,
/// then `vg2<format>`.
pub(crate) struct Vega;

impl Converter for Vega {
    formats!("svg", "png");
    fn collect_options(&self, source: &Source<'_, '_>) -> Result<Options, String> {
        let vegalite = if source.diagram_type.contains("lite") {
            OptValue::Bool(true)
        } else {
            opt(source, "vegalite")
        };
        Ok(vec![("vegalite", vegalite)])
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        options: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let base_dir = source.base_dir();
        let code = self.code(source, host)?;
        let vega_code = if code.contains("/schema/vega-lite/") || truthy(options, "vegalite") {
            let vl2vg = find(source, host, "vl2vg")?;
            // A bare tool: no `chdir`.
            generate_stdin_stdout(host, code.as_bytes(), command(vec![vl2vg], None))?
        } else {
            code.into_bytes()
        };
        let tool = find(source, host, &format!("vg2{format}"))?;
        let mut args = vec![tool, "--base".to_owned(), base_dir.clone()];
        if format == "svg" {
            args.push("--header".to_owned());
        }
        generate_stdin_stdout(host, &vega_code, command(args, Some(base_dir)))
    }
}

/// `VhsConverter` (`tape`): runs terminal commands, so only in unsafe mode.
pub(crate) struct Vhs;

impl Converter for Vhs {
    formats!("gif", "svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let safe = source.context.safe();
        if safe != SafeMode::Unsafe {
            return Err(format!(
                "vhs diagram generation is not allowed in safe mode {}. vhs executes terminal \
                 commands and requires UNSAFE mode (0).",
                safe_level(safe)
            ));
        }
        let tool = find(source, host, "vhs")?;
        let code = self.code(source, host)?;
        generate_file(
            host,
            &tool,
            "tape",
            format,
            code.as_bytes(),
            |input, output| {
                Ok(command(
                    vec![
                        tool.clone(),
                        input.to_owned(),
                        "-o".to_owned(),
                        output.to_owned(),
                    ],
                    Some(source.base_dir()),
                ))
            },
        )
    }
}

/// Ruby's `SafeMode` integers.
fn safe_level(safe: SafeMode) -> u8 {
    match safe {
        SafeMode::Unsafe => 0,
        SafeMode::Safe => 1,
        SafeMode::Server => 10,
        SafeMode::Secure => 20,
    }
}

/// `WavedromConverter`: `wavedrom-cli`, the WaveDromEditor, or PhantomJS
/// with a `wavedrom` script.
pub(crate) struct Wavedrom;

impl Converter for Wavedrom {
    formats!("png", "svg");
    fn collect_options(&self, _: &Source<'_, '_>) -> Result<Options, String> {
        Ok(Vec::new())
    }
    fn convert(
        &self,
        source: &Source<'_, '_>,
        format: &str,
        _: &Options,
        host: &dyn DiagramHost,
    ) -> Result<Vec<u8>, String> {
        let quiet = |find: Find<'_>| {
            find_command(
                source,
                host,
                Find {
                    raise: false,
                    ..find
                },
            )
        };
        // The first two lookups share Ruby's `cmd-wavedrom` cache entry.
        let mut wavedrom = quiet(Find {
            attrs: Some(&["wavedrom"]),
            ..Find::new("wavedrom-cli")
        })?;
        if wavedrom.is_none() {
            wavedrom = quiet(Find::new("wavedrom"))?;
        }
        if wavedrom.is_none() {
            wavedrom = quiet(Find::new("WaveDromEditor"))?;
        }
        let wavedrom = match wavedrom {
            Some(wavedrom) => wavedrom,
            None => find_command(
                source,
                host,
                Find {
                    attrs: Some(&["wavedrom"]),
                    ..Find::new("wavedrom-cli")
                },
            )?
            .unwrap_or_default(),
        };
        let code = self.code(source, host)?;
        let base_dir = source.base_dir();
        if wavedrom.contains("-cli") {
            generate_file(
                host,
                &wavedrom,
                "wvd",
                format,
                code.as_bytes(),
                |input, output| {
                    Ok(command(
                        vec![
                            wavedrom.clone(),
                            "--input".to_owned(),
                            input.to_owned(),
                            format!("--{format}"),
                            output.to_owned(),
                        ],
                        Some(base_dir.clone()),
                    ))
                },
            )
        } else if wavedrom.contains("WaveDromEditor") {
            generate_file(
                host,
                &wavedrom,
                "wvd",
                format,
                code.as_bytes(),
                |input, output| {
                    Ok(command(
                        vec![
                            wavedrom.clone(),
                            "source".to_owned(),
                            input.to_owned(),
                            format.to_owned(),
                            output.to_owned(),
                        ],
                        Some(base_dir.clone()),
                    ))
                },
            )
        } else {
            generate_file(
                host,
                &wavedrom,
                "wvd",
                format,
                code.as_bytes(),
                |input, output| {
                    let phantomjs = find_command(
                        source,
                        host,
                        Find {
                            alt_attrs: &["phantomjs_2"],
                            ..Find::new("phantomjs")
                        },
                    )?
                    .unwrap_or_default();
                    Ok(command(
                        vec![
                            phantomjs,
                            wavedrom.clone(),
                            "-i".to_owned(),
                            input.to_owned(),
                            format!("-{}", &format[..1]),
                            output.to_owned(),
                        ],
                        Some(base_dir.clone()),
                    ))
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oxdraw_scales_the_svg_like_rubys_sub() {
        assert_eq!(
            scale_svg_attribute("<svg width=\"10\" height=\"20\">", "width", 1.5),
            "<svg width=\"15\" height=\"20\">"
        );
        // Greedy `.*`: the last attribute of the line; another line is not
        // reached.
        assert_eq!(
            scale_svg_attribute(
                "<svg a width=\"4\" b width=\"6\">\nwidth=\"8\">",
                "width",
                2.0
            ),
            "<svg a width=\"4\" b width=\"12\">\nwidth=\"8\">"
        );
        assert_eq!(
            scale_svg_attribute("<svg width=\"x\">", "width", 2.0),
            "<svg width=\"x\">"
        );
        assert_eq!(ruby_to_f("2.5x"), 2.5);
        assert_eq!(ruby_to_f("x"), 0.0);
        assert_eq!(ruby_to_f(" 3"), 3.0);
    }

    #[test]
    fn pikchr_errors_lose_their_tags() {
        assert_eq!(
            strip_tags("<div>\nERROR: x < y</div>\n<p>z</P>"),
            "ERROR: x < yz"
        );
    }

    #[test]
    fn tikz_preamble_ends_at_a_tilde_line() {
        assert_eq!(
            split_preamble("\\usepackage{x}\n~~~~\n\\draw;"),
            ("\\usepackage{x}\n".to_owned(), "\n\\draw;".to_owned())
        );
        assert_eq!(
            split_preamble("a ~~~~\n\\draw;"),
            (String::new(), "a ~~~~\n\\draw;".to_owned())
        );
    }
}
