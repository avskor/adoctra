//! asciidoctor-diagram's `DiagramSource`/`BasicSource`/`ReaderSource`/
//! `FileSource`: the diagram code, attribute lookup with inheritance from
//! document attributes, and the checksum that names generated images.

use crate::md5::Md5;
use adoctra_core::{AttrKey, ExtensionAttributes, ProcessContext};

/// Where the code comes from.
#[derive(Debug, Clone)]
pub(crate) enum Origin {
    /// A block's lines (`ReaderSource`).
    Lines(Vec<String>),
    /// A block macro's file (`FileSource`), an absolute path or none.
    File(Option<String>),
}

/// A processor's `config` cache of tool paths (`cmd-<attr>`).
pub(crate) type Commands = std::sync::Mutex<std::collections::HashMap<String, Option<String>>>;

pub(crate) struct Source<'c, 'a> {
    /// The processor name, downcased (`diagram_type`).
    pub diagram_type: String,
    /// The normalised attributes (`@attributes`).
    pub attributes: ExtensionAttributes,
    pub origin: Origin,
    pub context: &'c ProcessContext<'a>,
    code: Option<String>,
    checksum: Option<String>,
    commands: Option<&'c Commands>,
    /// Path resolution warnings (logged without a location).
    pub warnings: std::cell::RefCell<Vec<String>>,
}

impl<'c, 'a> Source<'c, 'a> {
    pub fn new(
        diagram_type: &str,
        attributes: ExtensionAttributes,
        origin: Origin,
        context: &'c ProcessContext<'a>,
    ) -> Self {
        Self {
            diagram_type: diagram_type.to_lowercase(),
            attributes,
            origin,
            context,
            code: None,
            checksum: None,
            commands: None,
            warnings: Default::default(),
        }
    }

    /// Cache tool paths in the processor's `config`.
    pub fn with_commands(mut self, commands: &'c Commands) -> Self {
        self.commands = Some(commands);
        self
    }

    /// A tool path cached under `cmd_var` (`Some(None)`: cached `nil`).
    pub fn cached_command(&self, cmd_var: &str) -> Option<Option<String>> {
        let commands = self.commands?.lock().ok()?;
        commands.get(cmd_var).cloned()
    }

    pub fn cache_command(&self, cmd_var: String, path: Option<String>) {
        if let Some(Ok(mut commands)) = self.commands.map(|c| c.lock()) {
            commands.insert(cmd_var, path);
        }
    }

    /// `resolve_path(target, start = base_dir)`: `normalize_system_path`.
    pub fn resolve_path(&self, target: &str, start: Option<&str>) -> String {
        let base_dir = self.base_dir();
        let start = start.unwrap_or(&base_dir);
        crate::processor::Paths::new(self.context).normalize(
            target,
            Some(start),
            &mut self.warnings.borrow_mut(),
        )
    }

    /// `attr(name, default, inherit = diagram_type)`: the block attribute,
    /// else the document attribute `<type>-<name>`, else `default`.
    pub fn attr(&self, name: &str) -> Option<String> {
        self.attr_inherit(&[name], Inherit::Prefix(&self.diagram_type))
    }

    /// `attr([names…])`: the first of several names.
    pub fn attr_any(&self, names: &[&str]) -> Option<String> {
        self.attr_inherit(names, Inherit::Prefix(&self.diagram_type))
    }

    /// `attr(name, nil, true)`: the block attribute, else the document's.
    pub fn attr_document(&self, name: &str) -> Option<String> {
        self.attr_inherit(&[name], Inherit::Document)
    }

    pub fn attr_inherit(&self, names: &[&str], inherit: Inherit<'_>) -> Option<String> {
        if let Some(value) = names.iter().find_map(|n| self.attributes.get(n)) {
            return Some(value.to_owned());
        }
        names.iter().find_map(|name| {
            let key = match inherit {
                Inherit::Prefix(prefix) => format!("{prefix}-{name}"),
                Inherit::Document => (*name).to_owned(),
                Inherit::None => return None,
            };
            self.context.attribute(&key).map(str::to_owned)
        })
    }

    /// `global_attr(name, default)`: `attr(name) || attr(name, default,
    /// 'diagram')`.
    pub fn global_attr(&self, name: &str) -> Option<String> {
        self.attr(name)
            .or_else(|| self.attr_inherit(&[name], Inherit::Prefix("diagram")))
    }

    /// `global_opt(opt)`.
    pub fn global_opt(&self, option: &str) -> bool {
        self.global_attr(&format!("{option}-option")).is_some()
    }

    /// `opt(opt)`.
    pub fn opt(&self, option: &str) -> bool {
        self.attr(&format!("{option}-option")).is_some()
    }

    /// `base_dir`: the file's directory for a file source, else the
    /// expanded `docdir`.
    pub fn base_dir(&self) -> String {
        if let Origin::File(Some(file)) = &self.origin {
            return parent_dir(file);
        }
        let docdir = self.attr_document("docdir").unwrap_or_default();
        expand_path(&docdir, &current_dir())
    }

    /// The diagram code (`load_code`), read once, with the block's `subs`
    /// (`resolve_block_subs` from no defaults): `attributes` and
    /// `specialcharacters` are applied; other substitutions are not
    /// supported and leave the code unchanged.
    pub fn code(&mut self, host: &dyn crate::DiagramHost) -> Result<&str, String> {
        if self.code.is_none() {
            let code = match &self.origin {
                Origin::Lines(lines) => lines.join("\n"),
                Origin::File(None) => String::new(),
                Origin::File(Some(path)) => {
                    let data = host
                        .read(path)
                        .map_err(|e| format!("{} @ rb_sysopen - {path}", errno_message(&e)))?;
                    prepare_source_array(&data).join("\n")
                }
            };
            let code = match self.attributes.get("subs") {
                Some(subs) => apply_subs(&code, &resolve_subs(subs), self.context.attributes()),
                None => code,
            };
            self.code = Some(code);
        }
        Ok(self.code.as_deref().unwrap())
    }

    /// The code once read.
    pub fn loaded_code(&self) -> &str {
        self.code.as_deref().unwrap_or_default()
    }

    /// `checksum`: `<type>-md5-<hex>` over the code and every attribute key
    /// and value as they are when the image is named; a `nil` value (an
    /// empty positional, `[,svg]`) adds only its key, like Ruby's
    /// `md5 << v.to_s if v`.
    pub fn checksum(&mut self) -> String {
        if let Some(checksum) = &self.checksum {
            return checksum.clone();
        }
        let mut md5 = Md5::new();
        md5.update(self.loaded_code().as_bytes());
        for (key, value) in self.attributes.iter() {
            match key {
                AttrKey::Positional(index) => md5.update(index.to_string().as_bytes()),
                AttrKey::Named(name) => md5.update(name.as_bytes()),
            }
            if let Some(value) = value {
                md5.update(value.as_bytes());
            }
        }
        let checksum = format!("{}-md5-{}", self.diagram_type, md5.hexdigest());
        self.checksum = Some(checksum.clone());
        checksum
    }

    /// `image_name` (without the format extension).
    pub fn image_name(&mut self) -> String {
        match &self.origin {
            Origin::File(file) if self.attributes.get("target").is_none() => match file {
                Some(file) => basename_without_ext(file),
                None => self.checksum(),
            },
            _ => match self.attr("target") {
                Some(target) => target,
                None => format!("diag-{}", self.checksum()),
            },
        }
    }

    /// `should_process?(image_file, metadata)`.
    pub fn should_process(
        &mut self,
        host: &dyn crate::DiagramHost,
        image_file: &str,
        cached_checksum: Option<&str>,
    ) -> bool {
        if let Origin::File(Some(file)) = &self.origin {
            if let (Some(source), Some(image)) = (host.modified(file), host.modified(image_file)) {
                if source > image {
                    return true;
                }
            }
        }
        cached_checksum != Some(self.checksum().as_str())
    }
}

/// Ruby's `resolve_block_subs(subs, nil, 'diagram')`: comma-separated
/// names and groups, `+`/`-` modifiers relative to the (empty) defaults.
pub(crate) fn resolve_subs(spec: &str) -> Vec<&'static str> {
    const NORMAL: [&str; 6] = [
        "specialcharacters",
        "quotes",
        "attributes",
        "replacements",
        "macros",
        "post_replacements",
    ];
    let expand = |name: &str| -> Vec<&'static str> {
        match name {
            "none" => vec![],
            "normal" => NORMAL.to_vec(),
            "verbatim" => vec!["specialcharacters", "callouts"],
            "a" | "attributes" => vec!["attributes"],
            "c" | "specialchars" | "specialcharacters" => vec!["specialcharacters"],
            "q" | "quotes" => vec!["quotes"],
            "r" | "replacements" => vec!["replacements"],
            "m" | "macros" => vec!["macros"],
            "p" | "post_replacements" => vec!["post_replacements"],
            "callouts" => vec!["callouts"],
            _ => vec![],
        }
    };
    let mut subs: Vec<&'static str> = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        if let Some(name) = item.strip_prefix('+') {
            subs.extend(expand(name));
        } else if let Some(name) = item.strip_suffix('+') {
            let mut front = expand(name);
            front.append(&mut subs);
            subs = front;
        } else if let Some(name) = item.strip_prefix('-') {
            let removed = expand(name);
            subs.retain(|s| !removed.contains(s));
        } else {
            subs.extend(expand(item));
        }
    }
    let mut unique = Vec::new();
    for sub in subs {
        if !unique.contains(&sub) {
            unique.push(sub);
        }
    }
    unique
}

/// The supported substitutions in order: special characters and Ruby's
/// `sub_attributes` (escaped references unescaped; a missing attribute per
/// `attribute-missing`: kept, dropped, or its line dropped).
pub(crate) fn apply_subs(
    code: &str,
    subs: &[&str],
    attributes: &adoctra_core::Attributes,
) -> String {
    let mut text = code.to_owned();
    for sub in subs {
        match *sub {
            "specialcharacters" => {
                text = text
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            }
            "attributes" if text.contains('{') => {
                let missing = attributes.get("attribute-missing").unwrap_or("skip");
                let lines: Vec<String> = text
                    .split('\n')
                    .filter_map(|line| {
                        sub_attribute_line(line, attributes, missing, &mut Vec::new())
                    })
                    .collect();
                text = lines.join("\n");
            }
            _ => {}
        }
    }
    text
}

/// `sub_attributes(text, attribute_missing: 'warn')`: a missing reference
/// stays and logs Ruby's warning.
pub(crate) fn sub_attributes_warn(
    text: &str,
    attributes: &adoctra_core::Attributes,
    warnings: &mut Vec<String>,
) -> String {
    let lines: Vec<String> = text
        .split('\n')
        .filter_map(|line| sub_attribute_line(line, attributes, "warn", warnings))
        .collect();
    lines.join("\n")
}

fn sub_attribute_line(
    line: &str,
    attributes: &adoctra_core::Attributes,
    missing: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let mut out = String::new();
    let mut rest = line;
    let mut dropped = false;
    while let Some(open) = rest.find('{') {
        let escaped = rest[..open].ends_with('\\');
        let Some(close) = rest[open..].find('}').map(|c| open + c) else {
            break;
        };
        let name = &rest[open + 1..close];
        let close_escaped = name.ends_with('\\');
        let bare = name.trim_end_matches('\\');
        let word = bare
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
            && bare
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
        if !word {
            out.push_str(&rest[..open + 1]);
            rest = &rest[open + 1..];
            continue;
        }
        if escaped || close_escaped {
            out.push_str(&rest[..open - usize::from(escaped)]);
            out.push('{');
            out.push_str(bare);
            out.push('}');
        } else {
            out.push_str(&rest[..open]);
            match attributes.get(&bare.to_lowercase()) {
                Some(value) => out.push_str(value),
                None => match missing {
                    "drop" => {}
                    "drop-line" => dropped = true,
                    "warn" => {
                        warnings.push(format!("skipping reference to missing attribute: {bare}"));
                        out.push_str(&rest[open..=close]);
                    }
                    _ => out.push_str(&rest[open..=close]),
                },
            }
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    (!dropped).then_some(out)
}

#[derive(Clone, Copy)]
pub(crate) enum Inherit<'p> {
    Prefix(&'p str),
    Document,
    #[allow(dead_code)]
    None,
}

/// `prepare_source_array`: decode a UTF-16/UTF-8 BOM, split lines and
/// strip trailing whitespace from each.
fn prepare_source_array(data: &[u8]) -> Vec<String> {
    if data.is_empty() {
        return Vec::new();
    }
    let text = if let Some(rest) = data.strip_prefix(&[0xff, 0xfe]) {
        String::from_utf16_lossy(
            &rest
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
    } else if let Some(rest) = data.strip_prefix(&[0xfe, 0xff]) {
        String::from_utf16_lossy(
            &rest
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        String::from_utf8_lossy(data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data)).into_owned()
    };
    // Ruby's `lines` keeps each separator; `rstrip` removes it with any
    // trailing whitespace (and NUL).
    text.split_inclusive('\n')
        .map(|line| {
            line.trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\0')
                .to_owned()
        })
        .collect()
}

/// Ruby's `SystemCallError#message` prefix for the common errors.
fn errno_message(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory",
        std::io::ErrorKind::PermissionDenied => "Permission denied",
        _ => "Input/output error",
    }
}

pub(crate) fn current_dir() -> String {
    std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_owned())
}

/// Ruby's `File.expand_path(path, base)` for POSIX paths (no `~`).
pub(crate) fn expand_path(path: &str, base: &str) -> String {
    let joined = if path.starts_with('/') {
        path.to_owned()
    } else if path.is_empty() {
        base.to_owned()
    } else {
        format!("{}/{path}", base.trim_end_matches('/'))
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            segment => parts.push(segment),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Ruby's `File.dirname`.
pub(crate) fn parent_dir(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) => "/".to_owned(),
        Some(at) => path[..at].to_owned(),
        None => ".".to_owned(),
    }
}

/// Ruby's `File.basename(path, File.extname(path))`.
pub(crate) fn basename_without_ext(path: &str) -> String {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    match base.rfind('.') {
        Some(at) if at > 0 && at + 1 < base.len() => base[..at].to_owned(),
        _ => base.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_ruby() {
        assert_eq!(expand_path("a/../b", "/x/y"), "/x/y/b");
        assert_eq!(expand_path("/abs/./f", "/x"), "/abs/f");
        assert_eq!(expand_path("", "/x/y/"), "/x/y");
        assert_eq!(parent_dir("/a/b/c.puml"), "/a/b");
        assert_eq!(parent_dir("/c.puml"), "/");
        assert_eq!(basename_without_ext("/a/foo.puml"), "foo");
        assert_eq!(basename_without_ext("/a/.hidden"), ".hidden");
        assert_eq!(basename_without_ext("/a/noext"), "noext");
    }

    #[test]
    fn block_subs_resolve_and_apply_like_ruby() {
        assert_eq!(resolve_subs("attributes+"), ["attributes"]);
        assert_eq!(
            resolve_subs("+attributes,specialchars"),
            ["attributes", "specialcharacters"]
        );
        assert_eq!(
            resolve_subs("normal,-quotes,-macros")[..3],
            ["specialcharacters", "attributes", "replacements"]
        );
        let mut attributes = adoctra_core::Attributes::new();
        attributes.set("node", "start");
        assert_eq!(
            apply_subs(
                "a {node} \\{node} {nope} {x y}",
                &["attributes"],
                &attributes
            ),
            "a start {node} {nope} {x y}"
        );
        attributes.set("attribute-missing", "drop-line");
        assert_eq!(
            apply_subs("keep\n{nope} gone", &["attributes"], &attributes),
            "keep"
        );
        assert_eq!(
            apply_subs("a<b", &["specialcharacters"], &attributes),
            "a&lt;b"
        );
    }

    #[test]
    fn source_lines_are_decoded_and_stripped() {
        assert_eq!(
            prepare_source_array(b"\xef\xbb\xbfA -> B  \r\nC\t\n"),
            vec!["A -> B", "C"]
        );
        assert_eq!(
            prepare_source_array(&[0xff, 0xfe, b'x', 0, b'\n', 0, b'y', 0]),
            vec!["x", "y"]
        );
        assert!(prepare_source_array(b"").is_empty());
    }
}
