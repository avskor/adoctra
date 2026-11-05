//! asciidoctor-diagram's `DiagramProcessor#process` and
//! `create_image_block`/`create_literal_block` for the block, block macro
//! and inline macro processors.

use crate::cache::Metadata;
use crate::converter::{http::Http, options_json, Converter};
use crate::host::DiagramHost;
use crate::source::{expand_path, parent_dir, Commands, Origin, Source};
use adoctra_core::{
    AttrKey, AttributeFormat, BlockContext, BlockMacroProcessor, BlockMacroRequest, BlockProcessor,
    BlockRequest, ExtensionAttributes, InlineMacroProcessor, InlineMacroRequest, NewBlock,
    NewBlockKind, NewInline, OutputTarget, ProcessContext, SafeMode,
};
use std::sync::Arc;

const TEXT_FORMATS: [&str; 3] = ["txt", "atxt", "utxt"];

/// One diagram type (`plantuml`, `graphviz`, …) with its converter.
pub(crate) struct Diagram {
    pub name: &'static str,
    /// Ruby's processor class name prefix (`PlantUml`, `Graphviz`, …).
    pub class: &'static str,
    pub converter: Box<dyn Converter>,
    pub host: Arc<dyn DiagramHost>,
    /// The macros' positional attributes (`format`; meme's `top bottom
    /// target format`).
    pub macro_positional: &'static [&'static str],
    /// meme: a macro's target is the `background` image, the code empty.
    pub meme: bool,
    /// barcode: a macro's target is the code, unless `external`.
    pub barcode: bool,
}

impl Diagram {
    /// A macro's source: its file (`FileSource`), or for meme the
    /// `background` with no code (`MemeProcessor#create_source`).
    fn origin(&self, file: String, target: String) -> Origin {
        if self.meme {
            Origin::Meme(file)
        } else if self.barcode {
            Origin::Barcode { file, code: target }
        } else {
            Origin::File(Some(file))
        }
    }
}

/// Each processor instance has its own `config` cache of tool paths.
pub(crate) struct DiagramBlock(pub Arc<Diagram>, pub Commands);
pub(crate) struct DiagramBlockMacro(pub Arc<Diagram>, pub Commands);
pub(crate) struct DiagramInlineMacro(pub Arc<Diagram>, pub Commands);

/// Ruby's `StructurizrBlockProcessor`/`StructurizrBlockMacroProcessor`:
/// a `StructurizrConvert…Processor` turns the workspace into the text of a
/// view (format `txt`), which the renderer's block processor (named
/// `structurizr`) draws with the block's attributes.
pub(crate) struct Structurizr {
    pub convert: Arc<Diagram>,
    /// `d2`, `graphviz`, `mermaid`, `plantuml` (also for `plantuml-c4`).
    pub renderers: [Arc<Diagram>; 4],
}

/// Each processor instance owns its convert and renderer instances (and
/// their tool caches), as Ruby's `initialize` creates them.
pub(crate) struct StructurizrBlock(pub Arc<Structurizr>, pub Commands, pub [Commands; 4]);
pub(crate) struct StructurizrBlockMacro(pub Arc<Structurizr>, pub Commands, pub [Commands; 4]);

impl Structurizr {
    /// Render the converted text: `Renderers#get_renderer` from the
    /// block's `renderer` attribute (`structurizr-renderer` inherited,
    /// `plantuml-c4` by default).
    fn render(
        &self,
        first: NewBlock,
        attributes: ExtensionAttributes,
        caches: &[Commands; 4],
        context: &mut ProcessContext<'_>,
    ) -> NewBlock {
        let renderer = attributes
            .get("renderer")
            .map(str::to_owned)
            .or_else(|| context.attribute("structurizr-renderer").map(str::to_owned))
            .unwrap_or_else(|| crate::converter::java_tools::DEFAULT_RENDERER.to_owned());
        let index = match renderer.as_str() {
            "d2" => 0,
            "graphviz" => 1,
            "mermaid" => 2,
            "plantuml" | "plantuml-c4" => 3,
            other => {
                // Ruby raises outside the processor's error handling, which
                // ends the conversion; adoctra reports it like `on-error=log`.
                let text = format!("Failed to generate image: Unsupported renderer: '{other}'");
                context.error(text.clone());
                return NewBlock::listing(text, attributes);
            }
        };
        // The renderer reads the block's lines (`prepare_source_string`).
        // A converted (literal) block was titled and given a figure caption
        // first, which advanced the figure counter.
        let (text, converted) = match first.kind {
            NewBlockKind::Literal { source } => (source, true),
            NewBlockKind::Listing { source } => (source, false),
            _ => (String::new(), false),
        };
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|line| line.trim_end().to_owned())
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        let diagram = &self.renderers[index];
        let class = format!("Asciidoctor::Diagram::{}BlockProcessor", diagram.class);
        let mut block = process(
            diagram,
            &caches[index],
            &class,
            2,
            attributes,
            Origin::Lines(lines),
            context,
            Kind::Block,
        );
        if converted {
            block.discarded_captions.push("figure".to_owned());
        }
        block
    }
}

impl BlockProcessor for StructurizrBlock {
    fn name(&self) -> &str {
        "structurizr"
    }
    fn contexts(&self) -> &[BlockContext] {
        &[
            BlockContext::Listing,
            BlockContext::Literal,
            BlockContext::Open,
        ]
    }
    fn positional_attributes(&self) -> &[&str] {
        &["target", "format"]
    }
    fn process(&self, request: BlockRequest, context: &mut ProcessContext<'_>) -> Option<NewBlock> {
        let mut attributes = request.attributes.clone();
        attributes.set("format", "txt");
        let first = process(
            &self.0.convert,
            &self.1,
            "Asciidoctor::Diagram::StructurizrConvertBlockProcessor",
            2,
            attributes,
            Origin::Lines(request.lines),
            context,
            Kind::Block,
        );
        Some(self.0.render(first, request.attributes, &self.2, context))
    }
}

impl BlockMacroProcessor for StructurizrBlockMacro {
    fn name(&self) -> &str {
        "structurizr"
    }
    fn positional_attributes(&self) -> &[&str] {
        &["format"]
    }
    fn process(
        &self,
        request: BlockMacroRequest,
        context: &mut ProcessContext<'_>,
    ) -> Option<NewBlock> {
        let mut warnings = Vec::new();
        let file = Paths::new(context).normalize(&request.target, None, &mut warnings);
        for warning in warnings {
            context.warning_unlocated(warning);
        }
        let mut attributes = request.attributes.clone();
        attributes.set("format", "txt");
        let first = process(
            &self.0.convert,
            &self.1,
            "Asciidoctor::Diagram::StructurizrConvertBlockMacroProcessor",
            1,
            attributes,
            Origin::File(Some(file)),
            context,
            Kind::Block,
        );
        Some(self.0.render(first, request.attributes, &self.2, context))
    }
}

/// Which processor runs `process`: blocks re-apply the listing's `subs` to an
/// error, inline macros support no text formats.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Block,
    Inline,
}

impl BlockProcessor for DiagramBlock {
    fn name(&self) -> &str {
        self.0.name
    }
    fn contexts(&self) -> &[BlockContext] {
        &[
            BlockContext::Listing,
            BlockContext::Literal,
            BlockContext::Open,
        ]
    }
    fn positional_attributes(&self) -> &[&str] {
        &["target", "format"]
    }
    fn process(&self, request: BlockRequest, context: &mut ProcessContext<'_>) -> Option<NewBlock> {
        let class = format!("Asciidoctor::Diagram::{}BlockProcessor", self.0.class);
        Some(process(
            &self.0,
            &self.1,
            &class,
            2,
            request.attributes,
            Origin::Lines(request.lines),
            context,
            Kind::Block,
        ))
    }
}

impl BlockMacroProcessor for DiagramBlockMacro {
    fn name(&self) -> &str {
        self.0.name
    }
    fn positional_attributes(&self) -> &[&str] {
        self.0.macro_positional
    }
    fn process(
        &self,
        request: BlockMacroRequest,
        context: &mut ProcessContext<'_>,
    ) -> Option<NewBlock> {
        let class = format!("Asciidoctor::Diagram::{}BlockMacroProcessor", self.0.class);
        // `apply_target_subs`: the target (attribute references are expanded
        // by core) as a system path from the document's base directory.
        let mut warnings = Vec::new();
        let file = Paths::new(context).normalize(&request.target, None, &mut warnings);
        for warning in warnings {
            // The path resolver logs without a source location.
            context.warning_unlocated(warning);
        }
        Some(process(
            &self.0,
            &self.1,
            &class,
            self.0.macro_positional.len(),
            request.attributes,
            self.0.origin(file, request.target.clone()),
            context,
            Kind::Block,
        ))
    }
}

impl InlineMacroProcessor for DiagramInlineMacro {
    fn name(&self) -> &str {
        self.0.name
    }
    fn positional_attributes(&self) -> &[&str] {
        self.0.macro_positional
    }
    /// `DiagramInlineMacroProcessor#process`: the block `generate_block`
    /// creates becomes an inline image, or (an error listing) monospaced
    /// text, with the block's attributes minus `target` and `subs`.
    fn process(
        &self,
        request: InlineMacroRequest,
        context: &mut ProcessContext<'_>,
    ) -> Option<NewInline> {
        let class = format!("Asciidoctor::Diagram::{}InlineMacroProcessor", self.0.class);
        // `apply_target_subs`: the macros stage target goes through
        // `sub_attributes` again (missing references warn), then becomes a
        // system path from the document's base directory.
        let mut warnings = Vec::new();
        let target = crate::source::sub_attributes_warn(
            &request.target,
            context.attributes(),
            &mut warnings,
        );
        let file = Paths::new(context).normalize(&target, None, &mut warnings);
        let code = target.clone();
        for warning in warnings {
            context.warning_unlocated(warning);
        }
        let block = process(
            &self.0,
            &self.1,
            &class,
            self.0.macro_positional.len(),
            request.attributes,
            self.0.origin(file, code),
            context,
            Kind::Inline,
        );
        let mut attributes = block.attributes;
        let target = attributes.remove("target");
        attributes.remove("subs");
        // Ruby assigns these as the block's title and caption, not as
        // attributes the inline node receives.
        attributes.remove("title");
        attributes.remove("caption");
        Some(match block.kind {
            NewBlockKind::Image { target: image } => {
                NewInline::image(target.unwrap_or(image), attributes)
            }
            NewBlockKind::Listing { source }
            | NewBlockKind::Literal { source }
            | NewBlockKind::Pass { source } => {
                // `Block#source`: the lines of `prepare_source_string`.
                let source: Vec<&str> = source
                    .split_inclusive('\n')
                    .map(|line| {
                        line.trim_end_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c', '\0'])
                    })
                    .collect();
                NewInline::quoted(AttributeFormat::Monospace, source.join("\n"), attributes)
            }
        })
    }
}

/// The document's base directory and jail (`normalize_system_path`).
pub(crate) struct Paths {
    base_dir: String,
    safe: SafeMode,
}

impl Paths {
    pub(crate) fn new(context: &ProcessContext<'_>) -> Self {
        let cwd = crate::source::current_dir();
        let base_dir = context
            .base_dir()
            .map(str::to_owned)
            .or_else(|| context.attribute("docdir").map(str::to_owned))
            .filter(|dir| !dir.is_empty())
            .map(|dir| expand_path(&dir, &cwd))
            .unwrap_or(cwd);
        Self {
            base_dir,
            safe: context.safe(),
        }
    }

    /// `normalize_system_path(target, start)` with automatic recovery; the
    /// jail is the base directory from safe mode up.
    pub(crate) fn normalize(
        &self,
        target: &str,
        start: Option<&str>,
        warnings: &mut Vec<String>,
    ) -> String {
        let (start, jail) = if self.safe == SafeMode::Unsafe {
            let start = match start {
                Some(start) if adoctra_core::is_root_path(start) => start.to_owned(),
                Some(start) => format!("{}/{start}", self.base_dir),
                None => self.base_dir.clone(),
            };
            (start, None)
        } else {
            (
                start.map_or_else(|| self.base_dir.clone(), str::to_owned),
                Some(self.base_dir.as_str()),
            )
        };
        match adoctra_core::resolve_system_path(target, &start, jail, "path", true) {
            Ok(resolved) => {
                warnings.extend(resolved.warnings);
                resolved.path
            }
            Err(message) => {
                warnings.push(message);
                expand_path(target, &start)
            }
        }
    }
}

/// `resolve_path(parent, path, base_dir)`: a system path without a jail.
fn resolve_path(path: Option<&str>, base_dir: Option<&str>) -> String {
    let cwd = crate::source::current_dir();
    let start = base_dir.map_or_else(|| cwd.clone(), |dir| expand_path(dir, &cwd));
    match path {
        Some(path) => expand_path(path, &start),
        None => start,
    }
}

/// `output_dir`: the `outdir` attribute or the document's `:to_dir`.
fn output_dir(source: &Source<'_, '_>, context: &ProcessContext<'_>) -> String {
    let to_dir = context.output().map(|output| match output {
        OutputTarget::File(file) => parent_dir(file),
        OutputTarget::Directory(dir) => dir.clone(),
    });
    let outdir = source
        .context
        .attribute("outdir")
        .map(str::to_owned)
        .or(to_dir);
    resolve_path(outdir.as_deref(), None)
}

#[allow(clippy::too_many_arguments)]
fn process(
    diagram: &Diagram,
    commands: &Commands,
    class: &str,
    positional: usize,
    attributes: ExtensionAttributes,
    origin: Origin,
    context: &mut ProcessContext<'_>,
    kind: Kind,
) -> NewBlock {
    // Ruby normalises the keys and turns positional values after the named
    // ones into `<value>=true` keys.
    let mut normalised = ExtensionAttributes::new();
    for (key, value) in attributes.iter() {
        let key = match key {
            AttrKey::Named(name) => AttrKey::Named(name.to_lowercase()),
            key => key.clone(),
        };
        normalised.set_value(key, value.map(str::to_owned));
    }
    let mut index = positional + 1;
    while let Some(value) = attributes.positional(index) {
        normalised.set(value.to_owned(), "true");
        index += 1;
    }
    // meme's source adds the background after the normalisation.
    let origin = match origin {
        Origin::Meme(background) => {
            normalised.set("background", background);
            Origin::Lines(Vec::new())
        }
        // `BarcodeMacroProcessor#create_source`: the target is the code
        // unless the `external` attribute names it a file.
        Origin::Barcode { file, code } => {
            if normalised.get("external").is_some() {
                Origin::File(Some(file))
            } else {
                Origin::Lines(vec![code])
            }
        }
        origin => origin,
    };
    let host = Arc::clone(&diagram.host);
    let mut warnings = Vec::new();
    let (result, code) = {
        let mut source =
            Source::new(diagram.name, normalised, origin, context).with_commands(commands);
        let code = source.code(host.as_ref()).map(str::to_owned);
        let result = match &code {
            Ok(_) => generate(
                diagram,
                class,
                &mut source,
                host.as_ref(),
                &mut warnings,
                kind,
            ),
            Err(message) => Err(message.clone()),
        };
        // Converters resolve their paths after the image paths.
        warnings.append(&mut source.warnings.borrow_mut());
        (result, code)
    };
    for warning in warnings {
        // The path resolver logs without a source location.
        context.warning_unlocated(warning);
    }
    match result {
        Ok(generated) => generated,
        Err(message) => {
            // `on-error` `abort` raises in Ruby, which ends the conversion;
            // adoctra reports the error like `log`.
            let text = format!("Failed to generate image: {message}");
            context.error(text.clone());
            // The listing keeps the block's attributes: core applies its
            // `subs` like Ruby's `commit_subs` (to the substituted code); an
            // inline macro takes the listing's unconverted source.
            let source = format!("{text}\n{}", code.unwrap_or_default());
            NewBlock::listing(source, attributes)
        }
    }
}

fn generate(
    diagram: &Diagram,
    class: &str,
    source: &mut Source<'_, '_>,
    host: &dyn DiagramHost,
    warnings: &mut Vec<String>,
    kind: Kind,
) -> Result<NewBlock, String> {
    let converter = diagram.converter.as_ref();
    let mut formats: Vec<&str> = converter.supported_formats().to_vec();
    if kind == Kind::Inline {
        formats.retain(|format| !TEXT_FORMATS.contains(format));
    }
    let html = source
        .context
        .attribute("backend")
        .is_some_and(|b| b.to_lowercase().contains("html"));
    if html {
        if let Some(at) = formats.iter().position(|f| *f == "pdf") {
            let pdf = formats.remove(at);
            formats.push(pdf);
        }
    }
    let format = match source.attributes.remove("format") {
        Some(format) => format,
        None => source
            .global_attr("format")
            .unwrap_or_else(|| formats[0].to_owned()),
    };
    if format.is_empty() {
        return Err("Format undefined".to_owned());
    }
    if !formats.contains(&format.as_str()) {
        return Err(format!("{class} does not support output format {format}"));
    }
    let title = source.attributes.remove("title");
    let caption = source.attributes.remove("caption");
    let mut block = if TEXT_FORMATS.contains(&format.as_str()) {
        literal_block(converter, source, &format, host)?
    } else {
        image_block(converter, source, &format, html, host, warnings)?
    };
    if let Some(title) = title {
        block.attributes.set("title", title);
    }
    if let Some(caption) = caption {
        block.attributes.set("caption", caption);
    }
    Ok(block)
}

fn image_block(
    converter: &dyn Converter,
    source: &mut Source<'_, '_>,
    format: &str,
    html: bool,
    host: &dyn DiagramHost,
    warnings: &mut Vec<String>,
) -> Result<NewBlock, String> {
    let context = source.context;
    let image_name = format!("{}.{format}", source.image_name());
    let output = output_dir(source, context);
    let image_dir = match source.context.attribute("imagesoutdir") {
        Some(dir) => resolve_path(Some(dir), None),
        None => resolve_path(source.context.attribute("imagesdir"), Some(&output)),
    };
    let cache_dir = match source.global_attr("cachedir") {
        Some(dir) => resolve_path(Some(&dir), None),
        None => resolve_path(Some(".asciidoctor/diagram"), Some(&output)),
    };
    let paths = Paths::new(context);
    let image_file = paths.normalize(&image_name, Some(&image_dir), warnings);
    let metadata_file = paths.normalize(&format!("{image_name}.cache"), Some(&cache_dir), warnings);

    let use_cache = !source.global_opt("nocache");
    let mut metadata = Metadata::default();
    let mut cached_image_file = image_file.clone();
    if use_cache {
        if host.exists(&metadata_file) {
            if let Ok(data) = host.read(&metadata_file) {
                metadata = Metadata::parse(&String::from_utf8_lossy(&data));
            }
        }
        if source.global_opt("cache-images") {
            cached_image_file = paths.normalize(&image_name, Some(&cache_dir), warnings);
        }
    }

    let options = converter.collect_options(source)?;
    let regenerate = !host.exists(&cached_image_file)
        || source.should_process(host, &cached_image_file, metadata.checksum.as_deref())
        || metadata.options.as_deref() != Some(options_json(&options).as_str());

    let mut server_used = false;
    if regenerate {
        let server = source.global_attr("server-url").map(|url| Http {
            base_uri: url,
            server_type: source.global_attr("server-type").unwrap_or_default(),
            converter,
        });
        server_used = server.is_some();
        let (options, result) = match &server {
            Some(server) => {
                let options = Http::collect_options(source);
                let result = server.convert(source, format, &options, host);
                (options, result)
            }
            None => {
                let result = converter.convert(source, format, &options, host);
                (converter.stored_options(&options, format, source), result)
            }
        };
        let processed =
            crate::image::post_process(format, result?, !source.global_opt("nooptimise"))?;
        metadata = Metadata {
            checksum: Some(source.checksum()),
            options: Some(options_json(&options)),
            width: processed.width,
            height: processed.height,
        };
        host.write(&cached_image_file, &processed.data)
            .map_err(|e| format!("{cached_image_file}: {e}"))?;
        if use_cache {
            host.write(&metadata_file, metadata.to_json().as_bytes())
                .map_err(|e| format!("{metadata_file}: {e}"))?;
        } else {
            host.remove(&metadata_file);
        }
    }

    if cached_image_file != image_file && (!host.exists(&image_file) || regenerate) {
        host.link(&cached_image_file, &image_file)
            .map_err(|e| format!("{image_file}: {e}"))?;
    }

    let mut attributes = source.attributes.clone();
    // Ruby's `converter` is the `HttpConverter` (no native scaling) when it
    // generated the image through a server.
    let native_scaling = converter.native_scaling() && !(regenerate && server_used);
    let scale_factor = match attributes.get("scale") {
        Some(scale) if !native_scaling => leading_number(scale).unwrap_or(1.0),
        _ => 1.0,
    };
    if html {
        attributes.remove("scale");
        if let (Some(width), false) = (metadata.width, attributes.contains("width")) {
            attributes.set("width", width.scaled(scale_factor).to_string());
        }
        if let (Some(height), false) = (metadata.height, attributes.contains("height")) {
            attributes.set("height", height.scaled(scale_factor).to_string());
        }
    }

    let alt = match attributes.get("alt") {
        Some(alt) => alt.to_owned(),
        None => match attributes.get("title").or(attributes.get("target")) {
            Some(text) if attributes.get("title").is_some() => text.to_owned(),
            Some(target) => crate::source::basename_without_ext(target).replace(['_', '-'], " "),
            None => "Diagram".to_owned(),
        },
    };
    attributes.set("alt", alt);

    let mut absolute = source.attr_document("data-uri").is_some();
    if format == "svg" {
        let svg_type = if attributes.has_option("inline") {
            Some("inline".to_owned())
        } else if attributes.has_option("interactive") {
            Some("interactive".to_owned())
        } else {
            source.global_attr("svg-type")
        };
        match svg_type.as_deref() {
            None | Some("static") => {}
            Some("inline") => {
                attributes.set("inline-option", "");
                absolute = true;
            }
            Some("interactive") => attributes.set("interactive-option", ""),
            Some(other) => return Err(format!("Unsupported SVG type: {other}")),
        }
    }

    let target = if absolute {
        image_file.clone()
    } else {
        if source.global_attr("autoimagesdir").is_some() {
            let relative = relative_dir(&parent_dir(&image_file), &output);
            attributes.set("imagesdir", relative);
        }
        image_name.clone()
    };
    attributes.set("target", target.clone());
    Ok(NewBlock::image(target, attributes))
}

fn literal_block(
    converter: &dyn Converter,
    source: &mut Source<'_, '_>,
    format: &str,
    host: &dyn DiagramHost,
) -> Result<NewBlock, String> {
    let mut attributes = source.attributes.clone();
    attributes.remove("target");
    let options = converter.collect_options(source)?;
    let result = converter.convert(source, format, &options, host)?;
    Ok(NewBlock::literal(
        String::from_utf8_lossy(&result).into_owned(),
        attributes,
    ))
}

/// `/([0-9]+(?:\.[0-9]+)?)/` anywhere in the value.
fn leading_number(text: &str) -> Option<f64> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let rest = &text[start..];
    let mut end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if rest[end..].starts_with('.') {
        let frac = rest[end + 1..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len() - end - 1);
        if frac > 0 {
            end += 1 + frac;
        }
    }
    rest[..end].parse().ok()
}

/// `Pathname#relative_path_from(base)` for absolute paths.
fn relative_dir(path: &str, base: &str) -> String {
    let path: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let base: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    let common = path.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = std::iter::repeat_n("..", base.len() - common).collect();
    parts.extend(&path[common..]);
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_and_relative_paths_follow_ruby() {
        assert_eq!(leading_number("2"), Some(2.0));
        assert_eq!(leading_number("x1.5y"), Some(1.5));
        assert_eq!(leading_number("3."), Some(3.0));
        assert_eq!(leading_number("none"), None);
        assert_eq!(relative_dir("/out/images", "/out"), "images");
        assert_eq!(relative_dir("/out", "/out"), ".");
        assert_eq!(relative_dir("/a/img", "/out/x"), "../../a/img");
    }
}
