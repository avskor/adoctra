//! The diagram processors against a fake host: command lines, image names,
//! the cache, errors and server requests as asciidoctor-diagram 3.2.1 makes
//! them (names, sizes and messages checked against Ruby with the real
//! tools; see compatibility/diagram).

use adoctra_core::{
    parse_with_options, Backend, BlockKind, OutputTarget, ParseOptions, ParseResult, Registry,
    SafeMode, Severity,
};
use adoctra_ext_diagram::{Command, DiagramHost, HttpClient, HttpRequest, Output};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 83, 0, 0, 0,
    155, 8, 6, 0, 0, 0,
];
const SVG: &str =
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"50pt\" height=\"20pt\"><g/></svg>";

#[derive(Default)]
struct State {
    files: BTreeMap<String, (Vec<u8>, SystemTime)>,
    commands: Vec<Command>,
    requests: Vec<HttpRequest>,
    clock: u64,
}

/// An in-memory system: executables are the paths listed, tools write
/// `PNG`/`SVG` (or fail when the code contains `FAIL`).
#[derive(Clone, Default)]
struct Fake {
    state: Arc<Mutex<State>>,
    executables: Vec<String>,
    env: BTreeMap<String, String>,
    http: bool,
}

impl Fake {
    fn with(executables: &[&str]) -> Self {
        Self {
            executables: executables.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }

    fn put(&self, path: &str, data: &[u8]) {
        let mut state = self.state.lock().unwrap();
        state.clock += 1;
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(state.clock);
        state.files.insert(path.to_owned(), (data.to_vec(), time));
    }

    fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .files
            .get(path)
            .map(|f| f.0.clone())
    }

    fn paths(&self) -> Vec<String> {
        self.state.lock().unwrap().files.keys().cloned().collect()
    }

    fn commands(&self) -> Vec<Command> {
        std::mem::take(&mut self.state.lock().unwrap().commands)
    }

    fn requests(&self) -> Vec<HttpRequest> {
        std::mem::take(&mut self.state.lock().unwrap().requests)
    }

    /// The temporary file a tool writes: `-o<path>`, `-o <path>`, else the
    /// last argument naming a temporary file that does not exist yet.
    fn target_of(&self, command: &Command) -> Option<String> {
        command
            .args
            .iter()
            .find_map(|a| {
                a.strip_prefix("-o")
                    .filter(|p| p.starts_with('/'))
                    .map(str::to_owned)
            })
            .or_else(|| {
                command
                    .args
                    .iter()
                    .skip_while(|a| *a != "-o")
                    .nth(1)
                    .cloned()
            })
            .or_else(|| {
                command
                    .args
                    .iter()
                    .rev()
                    .find(|a| a.starts_with("/tmp/") && !self.exists(a))
                    .cloned()
            })
    }

    fn output_for(&self, command: &Command, target: Option<&str>) -> Vec<u8> {
        let stdin = String::from_utf8_lossy(command.stdin.as_deref().unwrap_or_default());
        let format = command
            .args
            .iter()
            .find_map(|a| {
                a.strip_prefix("-T")
                    .or_else(|| a.strip_prefix("-t"))
                    .filter(|f| !f.is_empty())
            })
            .map(str::to_owned)
            .or_else(|| {
                // ditaa's command line: PNG unless `--svg`.
                command
                    .args
                    .iter()
                    .any(|a| a.ends_with("CommandLineConverter"))
                    .then(|| {
                        if command.args.iter().any(|a| a == "--svg") {
                            "svg".to_owned()
                        } else {
                            "png".to_owned()
                        }
                    })
            })
            .or_else(|| target.and_then(|o| o.rsplit('.').next().map(str::to_owned)))
            .or_else(|| {
                stdin
                    .strip_prefix("set term ")
                    .and_then(|rest| rest.split([' ', '\n']).next())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        match format.as_str() {
            "png" => PNG.to_vec(),
            "svg" => SVG.as_bytes().to_vec(),
            "utxt" | "txt" => b"ASCII ART  \nline\n".to_vec(),
            "jpeg" => b"JPEG".to_vec(),
            _ => Vec::new(),
        }
    }
}

impl DiagramHost for Fake {
    fn run(&self, command: &Command) -> Result<Output, String> {
        self.state.lock().unwrap().commands.push(command.clone());
        let stdin =
            String::from_utf8_lossy(command.stdin.as_deref().unwrap_or_default()).into_owned();
        if stdin.contains("FAIL") {
            return Ok(Output {
                status: 1,
                stdout: Vec::new(),
                stderr: b"Error: syntax error in line 1".to_vec(),
            });
        }
        // structurizr-cli: one file per view in the `-o` directory.
        if command.args.iter().any(|a| a == "export") {
            let out = command
                .args
                .iter()
                .skip_while(|a| *a != "-o")
                .nth(1)
                .cloned()
                .unwrap_or_default();
            for key in ["land", "ctx"] {
                self.put(
                    &format!("{out}/structurizr-{key}.dot"),
                    format!("digraph{{{key}}}").as_bytes(),
                );
            }
            return Ok(Output::default());
        }
        if command.args.iter().any(|a| a == "-preproc") {
            return Ok(Output {
                status: 0,
                stdout: format!("{stdin}\n").into_bytes(),
                stderr: Vec::new(),
            });
        }
        let target = self.target_of(command);
        let data = self.output_for(command, target.as_deref());
        // LilyPond writes `<output>.cropped.<format>`.
        let target = match (command.program.ends_with("lilypond"), target) {
            (true, Some(target)) => Some(format!("{target}.cropped.png")),
            (_, target) => target,
        };
        match target {
            Some(target) => {
                self.put(&target, &data);
                Ok(Output::default())
            }
            None => Ok(Output {
                status: 0,
                stdout: data,
                stderr: Vec::new(),
            }),
        }
    }
    fn which(&self, name: &str) -> Option<String> {
        let path = format!("/usr/bin/{name}");
        self.executables.contains(&path).then_some(path)
    }
    fn is_executable(&self, path: &str) -> bool {
        self.executables.iter().any(|e| e == path)
    }
    fn temp_dir(&self) -> PathBuf {
        PathBuf::from("/tmp")
    }
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.file(path)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        self.put(path, data);
        Ok(())
    }
    fn exists(&self, path: &str) -> bool {
        self.file(path).is_some()
    }
    fn modified(&self, path: &str) -> Option<SystemTime> {
        self.state.lock().unwrap().files.get(path).map(|f| f.1)
    }
    fn remove(&self, path: &str) {
        self.state.lock().unwrap().files.remove(path);
    }
    fn link(&self, from: &str, to: &str) -> std::io::Result<()> {
        let data = self.read(from)?;
        self.put(to, &data);
        Ok(())
    }
    fn http(&self) -> Option<&dyn HttpClient> {
        self.http.then_some(self as &dyn HttpClient)
    }
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
    fn list_dir(&self, path: &str) -> Vec<String> {
        let prefix = format!("{path}/");
        self.paths()
            .iter()
            .filter_map(|p| p.strip_prefix(&prefix))
            .filter(|name| !name.contains('/'))
            .map(str::to_owned)
            .collect()
    }
}

impl HttpClient for Fake {
    fn send(&self, request: &HttpRequest) -> Result<Vec<u8>, String> {
        self.state.lock().unwrap().requests.push(request.clone());
        if request.url.contains("/fail/") {
            return Err("400 \"Bad Request\"".to_owned());
        }
        Ok(SVG.as_bytes().to_vec())
    }
}

fn options(host: &Fake, safe: SafeMode) -> ParseOptions {
    let mut registry = Registry::new();
    adoctra_ext_diagram::register(&mut registry, Arc::new(host.clone()));
    let mut attributes = adoctra_core::Attributes::new();
    attributes.set("docdir", "/doc");
    ParseOptions {
        extensions: Some(Arc::new(registry)),
        attributes,
        safe,
        base_dir: Some("/doc".to_owned()),
        output: Some(OutputTarget::File("/doc/d.html".to_owned())),
        embedded: true,
        ..ParseOptions::default()
    }
}

fn parse(host: &Fake, source: &str) -> ParseResult {
    parse_with_options(source, &options(host, SafeMode::Safe))
}

fn html(result: &ParseResult) -> String {
    String::from_utf8(
        adoctra_html::HtmlBackend::embedded()
            .convert(&result.document)
            .unwrap(),
    )
    .unwrap()
}

fn errors(result: &ParseResult) -> Vec<String> {
    result
        .diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}: {}",
                match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    _ => "other",
                },
                d.message
            )
        })
        .collect()
}

#[test]
fn graphviz_block_names_and_sizes_the_image_like_ruby() {
    let host = Fake::with(&["/usr/bin/dot"]);
    // Ruby names this exact block `diag-graphviz-md5-1332…` (its code and
    // attributes `1=graphviz`, `cloaked-context=literal`).
    let result = parse(&host, "[graphviz]\n....\ndigraph { a -> b }\n....\n");
    let name = "diag-graphviz-md5-13328802c64df661e756f74bef55deb8.png";
    assert_eq!(
        html(&result),
        format!(
            "<figure class=\"imageblock\">\n<img src=\"{name}\" alt=\"Diagram\" width=\"83\" \
             height=\"155\">\n</figure>\n"
        )
    );
    let commands = host.commands();
    assert_eq!(commands.len(), 1);
    let dot = &commands[0];
    assert_eq!(dot.program, "/usr/bin/dot");
    assert!(dot.args[0].starts_with("-o/tmp/dot"), "{:?}", dot.args);
    assert!(dot.args[0].ends_with(".png"));
    assert_eq!(dot.args[1], "-Tpng");
    assert_eq!(dot.chdir.as_deref(), Some("/doc"));
    assert_eq!(dot.stdin.as_deref(), Some(&b"digraph { a -> b }"[..]));
    assert_eq!(host.file(&format!("/doc/{name}")).as_deref(), Some(PNG));
    assert_eq!(
        String::from_utf8(
            host.file(&format!("/doc/.asciidoctor/diagram/{name}.cache"))
                .unwrap()
        )
        .unwrap(),
        "{\"checksum\":\"graphviz-md5-13328802c64df661e756f74bef55deb8\",\
         \"options\":{\"layout\":null},\"width\":83,\"height\":155}"
    );
    // The temporary output file is removed.
    assert!(host.paths().iter().all(|p| !p.starts_with("/tmp/")));
}

#[test]
fn cached_images_are_reused_until_the_code_or_options_change() {
    let host = Fake::with(&["/usr/bin/dot"]);
    let source = "[graphviz, g, svg]\n----\ndigraph { a }\n----\n";
    parse(&host, source);
    assert_eq!(host.commands().len(), 1);
    let again = parse(&host, source);
    assert!(host.commands().is_empty(), "the cache is used");
    // The size comes from the cache metadata (50pt × 1.33).
    assert!(
        html(&again).contains("width=\"66\" height=\"26\""),
        "{}",
        html(&again)
    );
    parse(
        &host,
        "[graphviz, g, svg, layout=neato]\n----\ndigraph { a }\n----\n",
    );
    let commands = host.commands();
    assert_eq!(commands.len(), 1, "changed options regenerate");
    assert_eq!(commands[0].args[2], "-Kneato");
    parse(
        &host,
        "[graphviz, g, svg, nocache-option]\n----\ndigraph { a }\n----\n",
    );
    assert_eq!(host.commands().len(), 1);
    assert!(!host.exists("/doc/.asciidoctor/diagram/g.svg.cache"));
}

#[test]
fn errors_become_listings_with_rubys_message() {
    let host = Fake::with(&["/usr/bin/dot"]);
    let result = parse(
        &host,
        ".T\n[[gid]]\n[graphviz, t, svg]\n----\nFAIL\n----\n\n[mermaid]\n----\ngraph TD\n----\n\n\
         [graphviz, x, jpg]\n----\na\n----\n",
    );
    assert_eq!(
        errors(&result),
        [
            "error: Failed to generate image: dot failed: Error: syntax error in line 1",
            "error: Failed to generate image: Could not find the 'mmdc' executable in PATH; add it \
             to the PATH or specify its location using the 'mmdc' document attribute",
            "error: Failed to generate image: Asciidoctor::Diagram::GraphvizBlockProcessor does \
             not support output format jpg",
        ]
    );
    // The listing keeps the block's metadata and shows the code.
    let listing = &result.document.blocks[0];
    assert_eq!(listing.id.as_deref(), Some("gid"));
    match &listing.kind {
        BlockKind::Listing { text, .. } => assert_eq!(
            text,
            "Failed to generate image: dot failed: Error: syntax error in line 1\nFAIL"
        ),
        other => panic!("expected a listing, got {other:?}"),
    }
    assert!(html(&result).contains("<div class=\"title\">T</div>"));
    // The error is reported at the block's fence.
    assert_eq!(
        result.diagnostics[0].span.start,
        ".T\n[[gid]]\n[graphviz, t, svg]\n".len()
    );
}

#[test]
fn block_macros_read_their_file_and_regenerate_when_it_changes() {
    let host = Fake::with(&["/usr/bin/dot"]);
    host.put("/doc/flows/a_b.dot", b"\xef\xbb\xbfdigraph { x }  \r\n");
    let result = parse(&host, ":dir: flows\n\ngraphviz::{dir}/a_b.dot[svg]\n");
    assert!(
        html(&result).contains("<img src=\"a_b.svg\" alt=\"Diagram\""),
        "{}",
        html(&result)
    );
    let commands = host.commands();
    assert_eq!(commands[0].stdin.as_deref(), Some(&b"digraph { x }"[..]));
    assert_eq!(commands[0].chdir.as_deref(), Some("/doc/flows"));
    parse(&host, "graphviz::flows/a_b.dot[svg]\n");
    assert!(host.commands().is_empty());
    host.put("/doc/flows/a_b.dot", b"digraph { x }\n");
    parse(&host, "graphviz::flows/a_b.dot[svg]\n");
    assert_eq!(host.commands().len(), 1, "a newer source regenerates");
    // A missing file: Ruby aborts the whole conversion; adoctra reports it.
    let result = parse(&host, "graphviz::nope.dot[]\n");
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: No such file or directory @ rb_sysopen - /doc/nope.dot"]
    );
}

#[test]
fn plantuml_runs_like_rubys_native_path() {
    let mut host = Fake::with(&["/opt/plantuml-full", "/usr/bin/dot"]);
    host.env
        .insert("DIAGRAM_PLANTUML_CLASSPATH".into(), "/jars/a.jar".into());
    let result = parse(
        &host,
        ":plantuml-native: /opt/plantuml-full\n\n[plantuml, seq, svg, theme=sky]\n----\nA -> B\n----\n",
    );
    assert!(html(&result).contains("<img src=\"seq.svg\" alt=\"seq\" width=\"66\" height=\"26\">"));
    let commands = host.commands();
    let preprocess = &commands[0];
    assert_eq!(preprocess.program, "/opt/plantuml-full");
    assert_eq!(
        preprocess.args,
        [
            "-pipe",
            "-preproc",
            "-failfast2",
            "-stdrpt:1",
            "-filedir",
            "/doc",
            "-theme",
            "sky",
            "-filedir",
            "/doc",
            "-theme",
            "sky"
        ]
    );
    assert_eq!(
        preprocess.stdin.as_deref(),
        Some(&b"@startuml\nA -> B\n@enduml"[..])
    );
    let render = &commands[1];
    assert_eq!(
        render.args,
        [
            "-pipe",
            "-failfast2",
            "-stdrpt:1",
            "-tsvg",
            "-filedir",
            "/doc",
            "-theme",
            "sky",
            "-graphvizdot",
            "/usr/bin/dot"
        ]
    );
    assert_eq!(
        render.env.get("PLANTUML_LIMIT_SIZE").map(String::as_str),
        Some("4096")
    );
    // The preprocessed code loses the final line break, like Ruby's server.
    assert_eq!(
        render.stdin.as_deref(),
        Some(&b"@startuml\nA -> B\n@enduml"[..])
    );
}

#[test]
fn plantuml_jars_run_through_java_and_salt_wraps_its_tag() {
    let mut host = Fake::with(&["/usr/bin/java"]);
    host.env.insert(
        "DIAGRAM_PLANTUML_CLASSPATH".into(),
        "/jars/a.jar:/jars/b.jar".into(),
    );
    let result = parse(
        &host,
        "[salt, s, png, preprocess=false]\n----\n{ Login }\n----\n",
    );
    assert!(
        html(&result).contains("<img src=\"s.png\""),
        "{}",
        html(&result)
    );
    let commands = host.commands();
    assert_eq!(commands.len(), 1, "no preprocessing");
    assert_eq!(commands[0].program, "/usr/bin/java");
    assert_eq!(
        commands[0].args,
        [
            "-Djava.awt.headless=true",
            "-cp",
            "/jars/a.jar:/jars/b.jar",
            "net.sourceforge.plantuml.Run",
            "-pipe",
            "-failfast2",
            "-stdrpt:1",
            "-tpng",
            "-filedir",
            "/doc",
            "-Playout=smetana"
        ]
    );
    assert_eq!(
        commands[0].stdin.as_deref(),
        Some(&b"@startsalt\n{ Login }\n@endsalt"[..])
    );
    // Without any PlantUML: Ruby's message.
    let result = parse(&Fake::with(&[]), "[plantuml]\n----\nA -> B\n----\n");
    assert!(errors(&result)[0].starts_with(
        "error: Failed to generate image: Could not load PlantUML. Either require \
         'asciidoctor-diagram-plantuml'"
    ));
}

#[test]
fn plantuml_text_formats_become_literal_blocks() {
    let host = Fake::with(&["/usr/bin/plantuml"]);
    let result = parse(&host, ".Seq\n[plantuml, t, txt]\n----\nA -> B\n----\n");
    match &result.document.blocks[0].kind {
        BlockKind::Literal { text, .. } => assert_eq!(text, "ASCII ART\nline"),
        other => panic!("expected a literal, got {other:?}"),
    }
    assert_eq!(result.document.blocks[0].caption, None);
    assert!(host.commands()[1].args.contains(&"-tutxt".to_owned()));
}

#[test]
fn mermaid_passes_its_options_to_mmdc() {
    let host = Fake::with(&["/usr/bin/mmdc"]);
    let result = parse(
        &host,
        "[mermaid, m, svg, theme=dark, background=fff, width=300, scale=2]\n----\ngraph TD\n----\n",
    );
    // Mermaid scales natively and keeps the explicit width.
    assert!(
        html(&result).contains("<img src=\"m.svg\" alt=\"m\" width=\"300\" height=\"26\">"),
        "{}",
        html(&result)
    );
    let command = &host.commands()[0];
    assert_eq!(command.program, "/usr/bin/mmdc");
    assert_eq!(command.args[0], "-i");
    assert!(command.args[1].ends_with(".mmd"));
    assert_eq!(
        &command.args[4..],
        [
            "--theme",
            "dark",
            "--width",
            "300",
            "--scale",
            "2",
            "--backgroundColor",
            "#fff"
        ]
    );
    assert_eq!(
        command.env.get("NODE_OPTIONS").map(String::as_str),
        Some("--unhandled-rejections=strict")
    );
    let result = parse(
        &host,
        "[mermaid, n, svg, scale=1.5]\n----\ngraph TD\n----\n",
    );
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: Mermaid only supports integer scale factors: 1.5"]
    );
}

#[test]
fn diagram_servers_receive_compressed_code() {
    let mut host = Fake::with(&[]);
    host.http = true;
    let source =
        ":diagram-server-url: https://kroki.example/base\n:diagram-server-type: kroki_io\n\n\
                  [graphviz, k, svg]\n----\ndigraph { a }\n----\n";
    let result = parse(&host, source);
    assert!(
        html(&result).contains("<img src=\"k.svg\""),
        "{}",
        html(&result)
    );
    let request = &host.requests()[0];
    assert!(request
        .url
        .starts_with("https://kroki.example/base/graphviz/svg/eN"));
    assert_eq!(request.body, None);
    // Longer than `max-get-size`: a POST of the plain code.
    let long = (0..400u64)
        .map(|i| {
            format!(
                "n{} -> m{}\n",
                i * 2_654_435_761 % 100_003,
                i * 40_503 % 99_991
            )
        })
        .collect::<String>();
    let result = parse(
        &host,
        &format!(
            ":diagram-server-url: http://127.0.0.1:8000\n:diagram-server-type: kroki_io\n\n\
             [graphviz, l, svg]\n----\ndigraph {{\n{long}}}\n----\n"
        ),
    );
    assert!(errors(&result).is_empty(), "{:?}", errors(&result));
    let request = &host.requests()[0];
    assert_eq!(request.url, "http://127.0.0.1:8000/graphviz/svg");
    assert_eq!(
        request.content_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );
    // A PlantUML server gets PlantUML's alphabet; plantuml.com never a POST.
    parse(
        &host,
        ":diagram-server-url: https://www.plantuml.com/plantuml\n:diagram-server-type: plantuml\n\n\
         [plantuml, p, svg, preprocess=false]\n----\nA -> B\n----\n",
    );
    let request = &host.requests()[0];
    assert!(request
        .url
        .starts_with("https://www.plantuml.com/plantuml/svg/"));
    let data = request.url.rsplit('/').next().unwrap();
    assert!(data
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=')));
    // A server error and a host without HTTP.
    let result = parse(
        &host,
        ":diagram-server-url: http://h/fail/\n:diagram-server-type: kroki_io\n\n[graphviz]\n----\na\n----\n",
    );
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: 400 \"Bad Request\""]
    );
    let result = parse(&Fake::with(&[]), source);
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: HTTP access to diagram servers is not allowed"]
    );
}

#[test]
fn svg_types_data_uri_and_autoimagesdir_set_the_target() {
    let host = Fake::with(&["/usr/bin/dot"]);
    let result = parse(&host, "[graphviz, i, svg, opts=inline]\n----\na\n----\n");
    match &result.document.blocks[0].kind {
        BlockKind::Image { target, .. } => assert_eq!(target, "/doc/i.svg"),
        other => panic!("expected an image, got {other:?}"),
    }
    assert!(result.document.blocks[0]
        .attrs
        .options
        .contains(&"inline".to_owned()));
    let result = parse(&host, ":data-uri:\n\n[graphviz, d, png]\n----\na\n----\n");
    match &result.document.blocks[0].kind {
        BlockKind::Image { target, .. } => assert_eq!(target, "/doc/d.png"),
        other => panic!("expected an image, got {other:?}"),
    }
    let result = parse(
        &host,
        ":imagesdir: imgs\n:diagram-autoimagesdir:\n\n[graphviz, a, svg]\n----\na\n----\n",
    );
    assert!(
        html(&result).contains("<img src=\"imgs/a.svg\""),
        "{}",
        html(&result)
    );
    assert!(host.exists("/doc/imgs/a.svg"));
    assert_eq!(
        result.document.blocks[0]
            .attrs
            .named
            .get("imagesdir")
            .map(String::as_str),
        Some("imgs")
    );
    let result = parse(&host, "[graphviz, b, svg, svg-type=bogus]\n----\na\n----\n");
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: Unsupported SVG type: bogus"]
    );
    let result = parse(
        &host,
        ":graphviz-svg-type: bogus\n\n[graphviz, c, svg]\n----\na\n----\n",
    );
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: Unsupported SVG type: bogus"]
    );
}

#[test]
fn image_paths_follow_imagesoutdir_cachedir_and_the_jail() {
    let host = Fake::with(&["/usr/bin/dot"]);
    parse(
        &host,
        ":imagesoutdir: /doc/out/img\n:diagram-cachedir: /doc/cache\n:diagram-cache-images-option:\n\n\
         [graphviz, p, png, scale=2]\n----\na\n----\n",
    );
    assert!(host.exists("/doc/out/img/p.png"));
    assert!(host.exists("/doc/cache/p.png"), "{:?}", host.paths());
    assert!(host.exists("/doc/cache/p.png.cache"));
    // Outside the base directory in safe mode, Ruby recovers into the jail
    // root (checked with asciidoctor -S safe).
    let result = parse(
        &host,
        ":imagesoutdir: /elsewhere\n\n[graphviz, q, png]\n----\na\n----\n",
    );
    assert!(host.exists("/doc/q.png"), "{:?}", host.paths());
    assert_eq!(
        errors(&result),
        ["warning: path is outside of jail; recovering automatically"]
    );
    let unsafe_options = options(&host, SafeMode::Unsafe);
    parse_with_options(
        ":imagesoutdir: /elsewhere\n\n[graphviz, r, png]\n----\na\n----\n",
        &unsafe_options,
    );
    assert!(host.exists("/elsewhere/r.png"));
}

#[test]
fn scale_multiplies_the_html_size() {
    let host = Fake::with(&["/usr/bin/dot"]);
    let result = parse(&host, "[graphviz, s, png, scale=2]\n----\na\n----\n");
    assert!(
        html(&result).contains("width=\"166\" height=\"310\""),
        "{}",
        html(&result)
    );
    assert!(!result.document.blocks[0].attrs.named.contains_key("scale"));
}

#[test]
fn inline_macros_become_inline_images_or_error_code() {
    let host = Fake::with(&["/usr/bin/dot"]);
    host.put("/doc/flows/g.dot", b"digraph { x }\n");
    host.put("/doc/bad.dot", b"FAIL <x>\n");
    let result = parse(
        &host,
        ":dir: flows\n\nA graphviz:{dir}/g.dot[svg,width=40,role=r,title=T] B \\graphviz:g.dot[] \
         C graphviz:bad.dot[role=q] D graphviz:flows/g.dot[txt] E graphviz:{nope}/g.dot[].\n\n\
         .Cap\nlast\n",
    );
    // Ruby: the image's attributes without the title, the error listing's
    // source as unescaped monospaced text, no text formats inline.
    assert_eq!(
        html(&result),
        "<p>A <span class=\"image r\"><img src=\"g.svg\" alt=\"Diagram\" width=\"40\" \
         height=\"26\"></span> B graphviz:g.dot[] C <code class=\"q\">Failed to generate image: \
         dot failed: Error: syntax error in line 1\nFAIL <x></code> D <code>Failed to generate \
         image: Asciidoctor::Diagram::GraphvizInlineMacroProcessor does not support output \
         format txt\ndigraph { x }</code> E <code>Failed to generate image: No such file or \
         directory @ rb_sysopen - /doc/{nope}/g.dot</code>.</p>\n<div class=\"title\">Cap</div>\n\
         <p>last</p>\n"
    );
    assert_eq!(
        errors(&result),
        [
            "error: Failed to generate image: dot failed: Error: syntax error in line 1",
            "error: Failed to generate image: Asciidoctor::Diagram::GraphvizInlineMacroProcessor \
             does not support output format txt",
            "warning: skipping reference to missing attribute: nope",
            "error: Failed to generate image: No such file or directory @ rb_sysopen - \
             /doc/{nope}/g.dot",
        ]
    );
    // Errors sit at Ruby's document reader mark (the last block, line 6);
    // the target warning has no location.
    let cursor = |line| {
        adoctra_core::DiagnosticLocation::Cursor(adoctra_core::DiagnosticCursor {
            source_id: adoctra_core::SourceId::default(),
            line,
        })
    };
    assert_eq!(result.diagnostics[0].report.location, cursor(6));
    assert_eq!(
        result.diagnostics[2].report.location,
        adoctra_core::DiagnosticLocation::Unlocated
    );
    assert!(host.file("/doc/g.svg").is_some());
    assert!(host.file("/doc/.asciidoctor/diagram/g.svg.cache").is_some());
}

#[test]
fn empty_positional_values_enter_the_checksum_as_rubys_nil_keys() {
    let host = Fake::with(&["/usr/bin/dot"]);
    host.put("/doc/g.dot", b"digraph { x }\n");
    let result = parse(
        &host,
        "[graphviz,,svg]\n....\ndigraph { a -> b }\n....\n\n\
         [graphviz,format=png,]\n....\ndigraph { a -> b }\n....\n\n\
         A graphviz:g.dot[,] B\n",
    );
    // Ruby's names: `2=nil` (and `3=nil`) add their keys to the MD5 without
    // a value; an empty positional is not a `<value>=true` key.
    let out = html(&result);
    for name in [
        "diag-graphviz-md5-977f928ff602470e5857712e01052479.svg",
        "diag-graphviz-md5-829bdcce65cf2e9cc46e8392f243929b.png",
        "g.png",
    ] {
        assert!(out.contains(&format!("src=\"{name}\"")), "{name}: {out}");
    }
    let cache =
        String::from_utf8(host.file("/doc/.asciidoctor/diagram/g.png.cache").unwrap()).unwrap();
    assert!(
        cache.starts_with("{\"checksum\":\"graphviz-md5-30ba37dad309f17166bd206973b6f08c\","),
        "{cache}"
    );
}

/// The arguments of the recorded commands, temporary paths as `<tmp>`.
fn command_lines(host: &Fake) -> Vec<String> {
    host.commands()
        .iter()
        .map(|command| {
            let line = std::iter::once(command.program.as_str())
                .chain(command.args.iter().map(String::as_str))
                .map(|arg| match arg.find("/tmp/") {
                    Some(at) => format!("{}<tmp>", &arg[..at]),
                    None => arg.to_owned(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            match &command.stdin {
                Some(stdin) => format!("{line} <<{}", String::from_utf8_lossy(stdin)),
                None => line,
            }
        })
        .collect()
}

#[test]
fn command_line_tools_run_like_rubys_converters() {
    // Command lines compared with Ruby through diagram-v1's recording tools.
    let host = Fake::with(&[
        "/usr/bin/d2",
        "/usr/bin/a2s",
        "/usr/bin/gnuplot",
        "/usr/bin/lilypond",
        "/usr/bin/svgbob_cli",
    ]);
    let result = parse(
        &host,
        "[d2,d1,svg,layout=elk,sketch=false,font-regular=f.ttf]\n----\na -> b\n----\n\n\
         [d2,d2b,png,sketch=yes]\n----\nx\n----\n\n\
         [a2s,a1,svg,scalex=2,scaley=3,noblur=true]\n----\n+-+\n----\n\n\
         [gnuplot,g1,svg,width=3,height=2,transparent=false,font=Arial]\n----\nplot x\n----\n\n\
         [lilypond,l1,png,resolution=150]\n----\n{ c' }\n----\n\n\
         [svgbob,s1,txt]\n----\n-->\n----\n",
    );
    assert!(errors(&result).is_empty(), "{:?}", errors(&result));
    assert_eq!(
        command_lines(&host),
        [
            "/usr/bin/d2 --browser false --layout elk --sketch false --font-regular f.ttf - <tmp> \
             <<a -> b",
            "/usr/bin/d2 --browser false --sketch - <tmp> <<x",
            "/usr/bin/a2s -o <tmp> -s 2,3 -b <<+-+",
            "/usr/bin/gnuplot <<set term svg size 3,2 transparent font \"Arial\"\nplot x\n",
            "/usr/bin/lilypond -daux-files=#f -dbackend=eps -dno-gs-load-fonts \
             -dinclude-eps-fonts -o <tmp> -f png -dcrop=#t -dresolution=150 \
             -dpixmap-format=pngalpha - <<\\paper{\n  oddFooterMarkup=##f\n  \
             oddHeaderMarkup=##f\n  bookTitleMarkup=##f\n  scoreTitleMarkup=##f\n}\n\n{ c' }",
        ]
    );
    // LilyPond's cropped file is the image; svgbob's text format is the code.
    assert!(host.file("/doc/l1.png").is_some());
    assert!(
        html(&result).contains("<pre class=\"literalblock\">--&gt;</pre>"),
        "{}",
        html(&result)
    );
}

#[test]
fn tool_paths_are_cached_per_processor_like_rubys_config() {
    // Only `wavedrom` is installed: Ruby's first lookup caches `nil` under
    // `cmd-wavedrom`, which the second lookup (for `wavedrom`) reuses.
    let host = Fake::with(&["/usr/bin/wavedrom", "/usr/bin/phantomjs"]);
    let result = parse(&host, "[wavedrom]\n----\n{}\n----\n");
    assert_eq!(
        errors(&result),
        [
            "error: Failed to generate image: Could not find the 'wavedrom-cli' executable in \
          PATH; add it to the PATH or specify its location using the 'wavedrom' document \
          attribute"
        ]
    );
    let host = Fake::with(&["/usr/bin/wavedrom-cli"]);
    parse(&host, "[wavedrom,w,svg]\n----\n{}\n----\n");
    assert_eq!(
        command_lines(&host),
        ["/usr/bin/wavedrom-cli --input <tmp> --svg <tmp>"]
    );
}

#[test]
fn ruby_failures_after_the_tool_ran_become_error_listings() {
    let host = Fake::with(&["/usr/bin/bpmn-js"]);
    let result = parse(&host, "[bpmn,p,jpeg]\n----\n<x/>\n----\n");
    // The tool runs; Ruby has no `IMAGE_PARAMS` for `jpeg`.
    assert_eq!(host.commands().len(), 1);
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: undefined method `[]' for nil"]
    );
    let host = Fake::with(&["/usr/bin/vhs"]);
    let result = parse(&host, "[tape]\n----\nType x\n----\n");
    assert_eq!(
        errors(&result),
        [
            "error: Failed to generate image: vhs diagram generation is not allowed in safe \
          mode 1. vhs executes terminal commands and requires UNSAFE mode (0)."
        ]
    );
    assert!(host.commands().is_empty());
}

#[test]
fn server_generated_images_scale_even_for_native_scaling_tools() {
    // Ruby's `HttpConverter` has no native scaling.
    let host = Fake {
        http: true,
        ..Fake::default()
    };
    let result = parse(
        &host,
        ":diagram-server-url: http://kroki/\n:diagram-server-type: kroki_io\n\n\
         [svgbob,k,scale=2]\n----\n-->\n----\n",
    );
    assert!(
        html(&result).contains("width=\"133\" height=\"53\""),
        "{}",
        html(&result)
    );
    assert_eq!(host.requests()[0].url.split('/').nth(3), Some("svgbob"));
}

#[test]
fn ditaa_runs_its_command_line_with_the_servers_arguments() {
    let mut host = Fake::with(&["/usr/bin/java"]);
    let result = parse(&host, "[ditaa]\n----\n+-+\n----\n");
    assert_eq!(
        errors(&result),
        [
            "error: Failed to generate image: Could not load Ditaa. Either require \
          'asciidoctor-diagram-ditaamini' or specify the location of the Ditaa JAR(s) using \
          the 'DIAGRAM_DITAA_CLASSPATH' environment variable."
        ]
    );
    host.env
        .insert("DIAGRAM_DITAA_CLASSPATH".into(), "/j/ditaa.jar".into());
    host.put("/j/ditaa.jar", b"jar");
    let result = parse(
        &host,
        "[ditaa,d,svg,scale=2,shadows=false,bullet-characters=* o]\n----\n+-+\n----\n\n\
         [ditaa,t,txt]\n----\n+-+\n----\n",
    );
    assert!(errors(&result).is_empty(), "{:?}", errors(&result));
    // The adapter's `--encoding`, `--svg` and `X-Options` (split at
    // spaces), stdin to stdout; a text format is the code.
    assert_eq!(
        command_lines(&host),
        [
            "/usr/bin/java -Djava.awt.headless=true -Djava.net.useSystemProxies=true \
          -Dfile.encoding=UTF-8 -cp /j/ditaa.jar \
          org.stathissideris.ditaa.core.CommandLineConverter --encoding UTF-8 --svg --scale 2 \
          --no-shadows --bullet-characters * o - - <<+-+"
        ]
    );
    assert!(
        html(&result).contains("<img src=\"d.svg\""),
        "{}",
        html(&result)
    );
    // A classpath item that does not exist stops the server, as in Ruby.
    host.env
        .insert("DIAGRAM_DITAA_CLASSPATH".into(), "/j/missing.jar".into());
    let result = parse(&host, "[ditaa,m]\n----\n+-+\n----\n");
    assert_eq!(
        errors(&result),
        ["error: Failed to generate image: Classpath item /j/missing.jar does not exist"]
    );
}

#[test]
fn structurizr_exports_a_view_and_renders_it() {
    let mut host = Fake::with(&["/usr/bin/java", "/usr/bin/dot"]);
    host.env
        .insert("DIAGRAM_STRUCTURIZRCLI_HOME".into(), "/s".into());
    host.put("/s/lib/b.jar", b"jar");
    host.put("/s/lib/a.jar", b"jar");
    let result = parse(
        &host,
        "[structurizr,renderer=graphviz]\n----\nworkspace {}\n----\n\n\
         .Land\n[structurizr,l,svg,view=land,renderer=graphviz]\n----\nworkspace {}\n----\n\n\
         [structurizr,n,view=none,renderer=graphviz]\n----\n{}\n----\n",
    );
    let lines = command_lines(&host);
    // The workspace file, its export, then Graphviz on the chosen view: the
    // smallest key by default.
    assert!(
        lines[0].contains("-cp /s/lib/a.jar:/s/lib/b.jar"),
        "{lines:?}"
    );
    assert!(lines[0]
        .contains("com.structurizr.cli.StructurizrCliApplication export -w /doc/.structurizr"));
    assert!(lines[0].ends_with(".dsl -f dot -o <tmp>"), "{lines:?}");
    assert!(lines[4].ends_with(".json -f dot -o <tmp>"), "{lines:?}");
    assert!(lines[1].starts_with("/usr/bin/dot") && lines[1].ends_with("<<digraph{ctx}"));
    assert!(lines[3].ends_with("<<digraph{land}"), "{lines:?}");
    let out = html(&result);
    // The image block is named after the `structurizr` renderer; the
    // titled one is figure 2 (Ruby captions the converted block too).
    assert!(out.contains("src=\"diag-structurizr-md5-"), "{out}");
    assert!(out.contains("Figure 2. Land"), "{out}");
    // A missing view: Ruby's server error; the renderer then draws the
    // error listing's text, as Ruby's renderer reads that block.
    assert!(
        lines[5].contains("<<Failed to generate image: Structurizr"),
        "{lines:?}"
    );
    assert_eq!(
        errors(&result)[0],
        "error: Failed to generate image: Structurizr code generation failed: \
         java.lang.NullPointerException: Cannot invoke \"Object.getClass()\" because \"view\" \
         is null"
    );
    // Temporary files are removed, the workspace (written to the base
    // directory for relative includes) too.
    assert!(
        host.paths()
            .iter()
            .all(|p| !p.starts_with("/tmp/") && !p.starts_with("/doc/.structurizr")),
        "{:?}",
        host.paths()
    );
}
