//! Native benchmark driver. File I/O and preflight are outside timed regions.
//! Invoked by scripts/performance.py; no benchmark/framework dependencies.
use adoctra_core::{
    parse_resource, tokens_resource, Backend, MemoryResources, ParseOptions, ParseResult, SafeMode,
    Severity, SourceMap, Token,
};
use adoctra_json::JsonBackend;
use std::{env, error::Error, fs, hint::black_box, path::PathBuf, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug)]
struct Config {
    source: PathBuf,
    resources: Vec<(String, PathBuf)>,
    attributes: Vec<(String, String)>,
    safe: SafeMode,
    warmup: usize,
    samples: usize,
    iterations: usize,
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut source = None;
        let mut config = Self {
            source: PathBuf::new(),
            resources: Vec::new(),
            attributes: Vec::new(),
            safe: SafeMode::Secure,
            warmup: 2,
            samples: 5,
            iterations: 3,
        };
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--source" if source.is_none() => source = Some(PathBuf::from(value)),
                "--resource" => config.resources.push((
                    value,
                    PathBuf::from(args.next().ok_or("missing resource path")?),
                )),
                "--attribute" => config
                    .attributes
                    .push((value, args.next().ok_or("missing attribute value")?)),
                "--safe-mode" => {
                    config.safe = match value.as_str() {
                        "safe" => SafeMode::Safe,
                        "secure" => SafeMode::Secure,
                        _ => return Err("safe mode must be safe or secure".into()),
                    }
                }
                "--warmup" | "--samples" | "--iterations" => {
                    let count = value.parse::<usize>()?;
                    if count > 10_000 || (count == 0 && flag != "--warmup") {
                        return Err(format!("invalid count for {flag}").into());
                    }
                    match flag.as_str() {
                        "--warmup" => config.warmup = count,
                        "--samples" => config.samples = count,
                        _ => config.iterations = count,
                    }
                }
                _ => return Err(format!("unknown or duplicate argument: {flag}").into()),
            }
        }
        config.source = source.ok_or("--source is required")?;
        Ok(config)
    }
}

struct Input {
    source: String,
    resources: MemoryResources,
    resource_bytes: usize,
    options: ParseOptions,
}

impl Input {
    fn load(config: &Config) -> Result<Self> {
        let source = fs::read_to_string(&config.source)?;
        let mut resources = MemoryResources::new();
        let mut resource_bytes = 0;
        for (name, path) in &config.resources {
            let content = fs::read_to_string(path)?;
            resource_bytes += content.len();
            if resources.insert(name, content)?.is_some() {
                return Err(format!("duplicate resource: {name}").into());
            }
        }
        let mut options = ParseOptions {
            embedded: true,
            safe: config.safe,
            ..Default::default()
        };
        options.attributes.set("showtitle", "");
        for (name, value) in &config.attributes {
            options.attributes.set(name, value);
        }
        Ok(Self {
            source,
            resources,
            resource_bytes,
            options,
        })
    }

    // A fresh map prevents include/source accumulation between iterations.
    // Source registration, cloning and line indexing are part of the timings.
    fn sources(&self) -> SourceMap {
        let mut sources = SourceMap::new();
        let id = sources.add("/main.adoc", self.source.clone());
        assert_eq!(id, self.options.source_id);
        sources
    }

    fn parse(&self) -> ParseResult {
        parse_resource(&mut self.sources(), &self.options, &self.resources)
            .expect("registered primary source")
    }

    fn tokens(&self) -> Vec<Token> {
        tokens_resource(&mut self.sources(), &self.options, &self.resources)
            .expect("registered primary source")
    }
}

fn measure<T>(config: &Config, mut operation: impl FnMut() -> T) -> Vec<u128> {
    for _ in 0..config.warmup {
        drop(black_box(operation()));
    }
    (0..config.samples)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..config.iterations {
                // Include destruction/allocation costs; don't accumulate results.
                drop(black_box(operation()));
            }
            start.elapsed().as_nanos()
        })
        .collect()
}

fn run(config: &Config, input: &Input) -> Result<String> {
    let parsed = input.parse();
    let json = JsonBackend.convert(&parsed.document)?;
    let tokens = input.tokens();
    let mut diagnostics = [0usize; 3];
    for diagnostic in &parsed.diagnostics {
        diagnostics[match diagnostic.severity {
            Severity::Info => 0,
            Severity::Warning => 1,
            Severity::Error => 2,
        }] += 1;
    }
    let parse = measure(config, || input.parse());
    let tokenize = measure(config, || input.tokens());
    let serialize = measure(config, || {
        JsonBackend
            .convert(black_box(&parsed.document))
            .expect("JSON conversion")
    });
    let parse_json = measure(config, || {
        let parsed = input.parse();
        JsonBackend
            .convert(&parsed.document)
            .expect("JSON conversion")
    });
    Ok(format!(
        r#"{{"schema":"adoctra-benchmark-driver-v1","debug_assertions":{},"source_bytes":{},"resource_bytes":{},"settings":{{"warmup":{},"samples":{},"iterations":{}}},"observations":{{"root_blocks":{},"tokens":{},"json_bytes":{},"diagnostics":{{"info":{},"warning":{},"error":{}}}}},"elapsed_ns":{{"parse":{:?},"tokens":{:?},"json":{:?},"parse-json":{:?}}}}}"#,
        cfg!(debug_assertions),
        input.source.len(),
        input.resource_bytes,
        config.warmup,
        config.samples,
        config.iterations,
        parsed.document.blocks.len(),
        tokens.len(),
        json.len(),
        diagnostics[0],
        diagnostics[1],
        diagnostics[2],
        parse,
        tokenize,
        serialize,
        parse_json,
    ))
}

fn main() -> Result<()> {
    let config = Config::parse(env::args().skip(1))?;
    let input = Input::load(&config)?;
    println!("{}", run(&config, &input)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_options_are_rejected_before_loading_files() {
        for args in [
            vec![],
            vec!["--source"],
            vec!["--source", "a", "--samples", "0"],
            vec!["--source", "a", "--iterations", "-1"],
            vec!["--source", "a", "--samples", "10001"],
            vec!["--source", "a", "--safe-mode", "unsafe"],
            vec!["--source", "a", "--resource", "x"],
            vec!["--source", "a", "--source", "b"],
            vec!["--unknown", "x"],
        ] {
            assert!(Config::parse(args.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn warmups_and_samples_execute_and_drop_every_result() {
        use std::cell::Cell;
        struct Value<'a>(&'a Cell<usize>);
        impl Drop for Value<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let config = Config::parse(
            [
                "--source",
                "unused",
                "--warmup",
                "2",
                "--samples",
                "3",
                "--iterations",
                "4",
            ]
            .map(str::to_owned),
        )
        .unwrap();
        let dropped = Cell::new(0);
        let samples = measure(&config, || Value(&dropped));
        assert_eq!(samples.len(), 3);
        assert_eq!(dropped.get(), 2 + 3 * 4);
    }

    #[test]
    fn includes_attributes_and_counters_repeat_without_state_accumulation() {
        let mut resources = MemoryResources::new();
        resources
            .insert("nested/note.adoc", "Ж {who} {counter:n}.\n")
            .unwrap();
        let mut options = ParseOptions {
            embedded: true,
            safe: SafeMode::Safe,
            ..Default::default()
        };
        options.attributes.set("who", "World");
        let input = Input {
            source: "include::nested/note.adoc[]\n".into(),
            resources,
            resource_bytes: 0,
            options,
        };
        let first = input.parse();
        assert!(first.diagnostics.is_empty());
        let expected = JsonBackend.convert(&first.document).unwrap();
        assert!(String::from_utf8_lossy(&expected).contains("World"));
        assert_eq!(first.document.final_attributes.get("n"), Some("1"));
        for _ in 0..3 {
            assert_eq!(
                JsonBackend.convert(&input.parse().document).unwrap(),
                expected
            );
            assert!(input.tokens().iter().any(|t| t.span.source_id.0 == 1));
        }
    }
}
