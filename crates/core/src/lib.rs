//! # xitter-dl core
//!
//! Schema, X payload parsing, import, and search. **No Tauri dependency** —
//! that is what makes the CLI, the test suite and the importers usable
//! without a GUI, and it is the split the PRD takes from gyotaku (PRD §7.2).
//!
//! ## Layout
//!
//! | Module | Owns |
//! |---|---|
//! | [`x`] | Understanding what X sent us |
//! | [`db`] | Storing and retrieving it |
//! | [`import`] | Getting bytes in from files |
//! | [`bridge`] | Getting bytes in from the userscript, over loopback |
//! | [`search`] | Finding things again |
//! | [`model`] | The contract with the frontend |
//!
//! ## The two invariants worth knowing before reading anything else
//!
//! 1. **`raw_tweet` holds X's bytes, and reads re-parse them.** A parser fix
//!    therefore applies retroactively to everything already captured, with no
//!    migration and no re-capture (PRD §9.2). If you are about to store a
//!    parsed value where a raw one would do, stop.
//! 2. **Nothing in this crate originates a request to X.** No HTTP *client* is
//!    linked. That is not an accident of scope; it is the §4.3 boundary, and
//!    a dependency added here that can open a socket to an X-owned host is a
//!    product change, not a refactor. [`bridge`] is the mirror image and does
//!    not weaken this: it *listens* on loopback and never dials out.

pub mod bridge;
pub mod db;
pub mod error;
pub mod import;
pub mod model;
pub mod search;
pub mod x;

pub use error::{Error, Result};

/// Unix seconds, the timestamp unit used throughout the schema.
///
/// A clock before the epoch would mean the system clock is wrong; clamping to
/// zero is friendlier than panicking in a library.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Where the library lives when the caller does not say.
///
/// Lives in `core` rather than in either frontend so the CLI and the app
/// cannot disagree about which file holds the user's bookmarks — they open the
/// same library, which is the entire point of having a CLI.
///
/// `XITTER_DL_DB` overrides everything. That is how the test suite and CI
/// keep their data out of a real library, and it is the escape hatch for
/// anyone who wants the file somewhere specific.
pub fn default_library_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("XITTER_DL_DB") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }

    let dir = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
    };

    dir.unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("xitter-dl")
        .join("library.sqlite")
}

#[cfg(test)]
mod tests {
    #[test]
    fn now_is_plausible() {
        // Guards against a unit mix-up (millis vs seconds): a seconds
        // timestamp in 2025 is around 1.7e9, a millisecond one is 1.7e12.
        let t = super::now();
        assert!(t > 1_700_000_000, "now() looks like it is not in seconds: {t}");
        assert!(t < 4_000_000_000, "now() is implausibly far in the future: {t}");
    }
}
