//! Error type for the whole core crate.
//!
//! Deliberately one enum rather than a per-module error type. The callers are
//! a CLI and an IPC layer; both want to show a human a sentence, and neither
//! wants to match on a taxonomy. Variants exist to distinguish *what failed*,
//! not to be exhaustively handled.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The file parsed as JSON but contains no recognisable tweet. This is the
    /// expected outcome for a HAR file full of analytics beacons, not a bug.
    #[error("no tweets found in {context}")]
    NoTweets { context: String },

    /// A payload we could open but whose shape we do not recognise. Kept
    /// distinct from `NoTweets` because it means the parser is out of date
    /// rather than the input being irrelevant — worth a different UI message.
    #[error("unrecognised payload shape: {0}")]
    UnrecognisedPayload(String),

    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path: path.into(), source }
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}
