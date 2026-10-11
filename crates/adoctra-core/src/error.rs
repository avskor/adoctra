//! Error type for the adoctra toolchain.
//!
//! Hand-rolled for now so the workspace builds with zero dependencies.
//! Swap for `thiserror` once you add it to `[workspace.dependencies]`.

use std::fmt;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Parsing failed at a given (1-based) line.
    Parse { line: usize, message: String },
    /// A backend was asked to render something it does not support yet.
    Unsupported(String),
    /// Wrapped I/O error from a backend or the CLI.
    Io(std::io::Error),
    /// A host resource could not be loaded.
    Resource(crate::ResourceError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse { line, message } => {
                write!(f, "parse error at line {line}: {message}")
            }
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Resource(e) => write!(f, "resource error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Resource(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<crate::ResourceError> for Error {
    fn from(e: crate::ResourceError) -> Self {
        Error::Resource(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
