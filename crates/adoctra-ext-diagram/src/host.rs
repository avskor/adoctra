//! Everything the diagram extension does outside the process: running
//! tools, reading and writing files, HTTP. A host chooses the policy by
//! implementing [`DiagramHost`]; [`SystemHost`] uses the real system
//! (without HTTP unless given a client).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// A tool invocation (Ruby's `Open3.capture3(env, *args, chdir:,
/// stdin_data:)`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub chdir: Option<String>,
    pub stdin: Option<Vec<u8>>,
}

/// A finished tool run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// An HTTP request to a diagram server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    /// `None` is a GET; otherwise a POST with this body.
    pub body: Option<Vec<u8>>,
    pub content_type: Option<String>,
}

/// Sends requests to a diagram server; the host decides whether network
/// access is allowed.
pub trait HttpClient: Send + Sync {
    /// The response body of a successful request, or the error message.
    fn send(&self, request: &HttpRequest) -> Result<Vec<u8>, String>;
}

/// The diagram extension's access to the outside world.
pub trait DiagramHost: Send + Sync {
    /// Run a tool to completion.
    fn run(&self, command: &Command) -> Result<Output, String>;
    /// Ruby's `Which.which`: an executable `name` on `PATH`.
    fn which(&self, name: &str) -> Option<String>;
    fn is_executable(&self, path: &str) -> bool;
    /// A directory for temporary files.
    fn temp_dir(&self) -> PathBuf;
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
    /// Write a file, creating its directory.
    fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()>;
    fn exists(&self, path: &str) -> bool;
    fn modified(&self, path: &str) -> Option<SystemTime>;
    fn remove(&self, path: &str);
    /// Hard-link `from` to `to` (replacing it), falling back to a copy.
    fn link(&self, from: &str, to: &str) -> std::io::Result<()>;
    /// The client for `diagram-server-url`, if the host allows HTTP.
    fn http(&self) -> Option<&dyn HttpClient> {
        None
    }
    /// An environment variable (e.g. `DIAGRAM_PLANTUML_CLASSPATH`).
    fn env(&self, name: &str) -> Option<String>;
    /// The names of the entries of a directory (structurizr's exported
    /// views); none by default.
    fn list_dir(&self, _path: &str) -> Vec<String> {
        Vec::new()
    }
    /// Remove an empty directory; nothing by default.
    fn remove_dir(&self, _path: &str) {}
}

/// The real system: processes, files and the process environment. HTTP
/// only with a client.
#[derive(Default, Clone)]
pub struct SystemHost {
    http: Option<Arc<dyn HttpClient>>,
}

impl SystemHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allow diagram servers through this client.
    pub fn with_http(mut self, client: Arc<dyn HttpClient>) -> Self {
        self.http = Some(client);
        self
    }
}

impl DiagramHost for SystemHost {
    fn run(&self, command: &Command) -> Result<Output, String> {
        use std::io::Write;
        let mut process = std::process::Command::new(&command.program);
        process
            .args(&command.args)
            .envs(&command.env)
            .stdin(if command.stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(dir) = &command.chdir {
            process.current_dir(dir);
        }
        let mut child = process.spawn().map_err(|e| e.to_string())?;
        let writer = command.stdin.clone().and_then(|data| {
            let mut stdin = child.stdin.take()?;
            Some(std::thread::spawn(move || {
                let _ = stdin.write_all(&data);
            }))
        });
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        Ok(Output {
            status: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn which(&self, name: &str) -> Option<String> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| executable(candidate))
            .map(|candidate| candidate.to_string_lossy().into_owned())
    }

    fn is_executable(&self, path: &str) -> bool {
        executable(Path::new(path))
    }

    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }

    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        if let Some(dir) = Path::new(path).parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, data)
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn modified(&self, path: &str) -> Option<SystemTime> {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    }

    fn remove(&self, path: &str) {
        let _ = std::fs::remove_file(path);
    }

    fn link(&self, from: &str, to: &str) -> std::io::Result<()> {
        if let Some(dir) = Path::new(to).parent() {
            std::fs::create_dir_all(dir)?;
        }
        let _ = std::fs::remove_file(to);
        std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
    }

    fn http(&self) -> Option<&dyn HttpClient> {
        self.http.as_deref()
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn list_dir(&self, path: &str) -> Vec<String> {
        std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn remove_dir(&self, path: &str) {
        let _ = std::fs::remove_dir(path);
    }
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}
