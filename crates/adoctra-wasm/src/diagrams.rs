//! Diagrams in the editor through a diagram server (Kroki by default): the
//! diagram extension's processors with a browser host. A browser cannot run
//! tools or make a blocking request, so a parse lists the server requests
//! it still needs; the host fetches them, passes the responses back and
//! parses again. Generated images come back as `data:` URIs.
//!
//! ```text
//! const ws = new DiagramWorkspace("https://kroki.io/", "kroki_io");
//! let result = JSON.parse(ws.parse_json(text, 0));
//! for (const r of result.diagrams.requests) {
//!   const response = await fetch(r.url, r.body === undefined ? {} :
//!     { method: "POST", body: r.body, headers: { "Content-Type": r.content_type } });
//!   if (response.ok) ws.set_response(r.key, new Uint8Array(await response.arrayBuffer()));
//!   else ws.set_error(r.key, `${response.status} "${response.statusText}"`);
//! }
//! result = JSON.parse(ws.parse_json(text, 0));   // images in result.diagrams.images
//! ```

use adoctra_core::json::escape_into as write_str;
use adoctra_ext_diagram::{Command, DiagramHost, HttpClient, HttpRequest, Output};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Responses the host fetched, by request key.
type Responses = BTreeMap<String, Result<Vec<u8>, String>>;

/// A parse session for documents with diagrams: the server and the
/// responses fetched so far.
pub struct DiagramWorkspace {
    server_url: String,
    server_type: String,
    responses: Responses,
}

/// The key of a request: its method, URL and, for a POST, its body.
fn request_key(request: &HttpRequest) -> String {
    match &request.body {
        None => format!("GET {}", request.url),
        Some(body) => format!("POST {}\n{}", request.url, String::from_utf8_lossy(body)),
    }
}

/// The browser: files in memory, no programs, the server through the
/// fetched responses.
struct BrowserHost {
    files: Mutex<BTreeMap<String, (Vec<u8>, u64)>>,
    clock: Mutex<u64>,
    responses: Responses,
    pending: Mutex<Vec<HttpRequest>>,
}

impl DiagramHost for BrowserHost {
    fn run(&self, command: &Command) -> Result<Output, String> {
        Err(format!(
            "{}: cannot run programs in the browser",
            command.program
        ))
    }
    fn which(&self, _: &str) -> Option<String> {
        None
    }
    fn is_executable(&self, _: &str) -> bool {
        false
    }
    fn temp_dir(&self) -> PathBuf {
        PathBuf::from("/tmp")
    }
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .map(|(data, _)| data.clone())
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        let mut clock = self.clock.lock().unwrap();
        *clock += 1;
        self.files
            .lock()
            .unwrap()
            .insert(path.to_owned(), (data.to_vec(), *clock));
        Ok(())
    }
    fn exists(&self, path: &str) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }
    fn modified(&self, path: &str) -> Option<SystemTime> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .map(|(_, t)| SystemTime::UNIX_EPOCH + Duration::from_secs(*t))
    }
    fn remove(&self, path: &str) {
        self.files.lock().unwrap().remove(path);
    }
    fn link(&self, from: &str, to: &str) -> std::io::Result<()> {
        let data = self.read(from)?;
        self.write(to, &data)
    }
    fn http(&self) -> Option<&dyn HttpClient> {
        Some(self)
    }
    fn env(&self, _: &str) -> Option<String> {
        None
    }
    fn list_dir(&self, path: &str) -> Vec<String> {
        let prefix = format!("{}/", path.trim_end_matches('/'));
        self.files
            .lock()
            .unwrap()
            .keys()
            .filter_map(|p| p.strip_prefix(&prefix))
            .filter(|name| !name.contains('/'))
            .map(str::to_owned)
            .collect()
    }
}

impl HttpClient for BrowserHost {
    fn send(&self, request: &HttpRequest) -> Result<Vec<u8>, String> {
        match self.responses.get(&request_key(request)) {
            Some(response) => response.clone(),
            None => {
                let mut pending = self.pending.lock().unwrap();
                if !pending.contains(request) {
                    pending.push(request.clone());
                }
                Err("waiting for the diagram server".to_owned())
            }
        }
    }
}

/// The documents' (virtual) location: images are written next to it.
const DOCUMENT: &str = "/index.html";

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn media_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        "jpeg" | "jpg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

impl DiagramWorkspace {
    /// A session with a diagram server: its URL (`diagram-server-url`) and
    /// type (`diagram-server-type`, `kroki_io` or `plantuml`). A document
    /// may set its own.
    pub fn new(server_url: &str, server_type: &str) -> Self {
        Self {
            server_url: server_url.to_owned(),
            server_type: server_type.to_owned(),
            responses: Responses::new(),
        }
    }

    /// The body the server answered for the request `key`.
    pub fn set_response(&mut self, key: &str, body: &[u8]) {
        self.responses.insert(key.to_owned(), Ok(body.to_vec()));
    }

    /// The request `key` failed (the message diagrams report, e.g.
    /// `400 "Bad Request"`).
    pub fn set_error(&mut self, key: &str, message: &str) {
        self.responses
            .insert(key.to_owned(), Err(message.to_owned()));
    }

    /// Forget the fetched responses.
    pub fn clear(&mut self) {
        self.responses.clear();
    }

    /// Parse with the diagram processors: the editor JSON of
    /// [`crate::parse_json`] plus `diagrams`: the `requests` still to fetch
    /// (`key`, `url`, a POST's `body` and `content_type`) and the
    /// generated `images` (path relative to the document → `data:` URI).
    pub fn parse_json(&mut self, src: &str, source_id: usize) -> String {
        let host = Arc::new(BrowserHost {
            files: Mutex::default(),
            clock: Mutex::default(),
            responses: self.responses.clone(),
            pending: Mutex::default(),
        });
        let mut registry = adoctra_core::Registry::new();
        adoctra_ext_diagram::register(&mut registry, Arc::clone(&host) as Arc<dyn DiagramHost>);
        let mut attributes = adoctra_core::Attributes::new();
        // Soft settings: the document's own server wins.
        attributes.set("diagram-server-url", format!("{}@", self.server_url));
        attributes.set("diagram-server-type", format!("{}@", self.server_type));
        let options = adoctra_core::ParseOptions {
            source_id: adoctra_core::SourceId(source_id),
            attributes,
            base_dir: Some("/".to_owned()),
            output: Some(adoctra_core::OutputTarget::File(DOCUMENT.to_owned())),
            extensions: Some(Arc::new(registry)),
            ..Default::default()
        };
        let result = adoctra_core::parse_with_options(src, &options);
        let mut json = crate::serialize_parse_result(&result);
        json.pop();
        json.push_str(",\"diagrams\":{\"requests\":[");
        for (i, request) in host.pending.lock().unwrap().iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str("{\"key\":");
            write_str(&request_key(request), &mut json);
            json.push_str(",\"url\":");
            write_str(&request.url, &mut json);
            if let Some(body) = &request.body {
                json.push_str(",\"body\":");
                write_str(&String::from_utf8_lossy(body), &mut json);
            }
            if let Some(content_type) = &request.content_type {
                json.push_str(",\"content_type\":");
                write_str(content_type, &mut json);
            }
            json.push('}');
        }
        json.push_str("],\"images\":{");
        let files = host.files.lock().unwrap();
        let images = files
            .iter()
            .filter(|(path, _)| !path.ends_with(".cache") && !path.contains("/.asciidoctor/"));
        for (i, (path, (data, _))) in images.enumerate() {
            if i > 0 {
                json.push(',');
            }
            write_str(path.trim_start_matches('/'), &mut json);
            json.push(':');
            write_str(
                &format!("data:{};base64,{}", media_type(path), base64(data)),
                &mut json,
            );
        }
        json.push_str("}}}");
        json
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(js_name = DiagramWorkspace)]
    pub struct Workspace(super::DiagramWorkspace);

    #[wasm_bindgen(js_class = DiagramWorkspace)]
    impl Workspace {
        #[wasm_bindgen(constructor)]
        pub fn new(server_url: &str, server_type: &str) -> Self {
            Self(super::DiagramWorkspace::new(server_url, server_type))
        }
        pub fn set_response(&mut self, key: &str, body: &[u8]) {
            self.0.set_response(key, body);
        }
        pub fn set_error(&mut self, key: &str, message: &str) {
            self.0.set_error(key, message);
        }
        pub fn clear(&mut self) {
            self.0.clear();
        }
        pub fn parse_json(&mut self, src: &str, source_id: usize) -> String {
            self.0.parse_json(src, source_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_media_types() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(media_type("a.svg"), "image/svg+xml");
        assert_eq!(media_type("a.png"), "image/png");
    }
}
