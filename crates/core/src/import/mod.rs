//! Importing from files.
//!
//! Three input shapes, because three things actually exist in the world:
//!
//! - **A single JSON payload** — one saved GraphQL response, or any envelope
//!   containing tweets. This is the path for "import one tweet".
//! - **NDJSON** — the userscript's handoff format (PRD §7.4). One record per
//!   line, append-only, stream-friendly. Never a single giant array, because
//!   an array cannot be appended to and a truncated one cannot be recovered.
//! - **HAR** — a browser network-log export. Zero-install fallback: anyone can
//!   produce one from DevTools without installing our userscript, which makes
//!   it the answer to "the script broke, what now".
//!
//! Format detection is by extension first and content second, so a `.txt` full
//! of JSON still imports rather than being rejected on a technicality.

pub mod ndjson;
pub mod payload;

use std::path::Path;

use rusqlite::Connection;

use crate::db::write::IngestStats;
use crate::error::{Error, Result};

#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub files: usize,
    pub stats: IngestStats,
    /// One entry per file that could not be read. Non-fatal by design: a
    /// directory import should bring in what it can and report the rest,
    /// rather than aborting on the first odd file.
    pub problems: Vec<String>,
}

impl ImportSummary {
    fn merge(&mut self, other: ImportSummary) {
        self.files += other.files;
        self.stats.merge(other.stats);
        self.problems.extend(other.problems);
    }
}

/// Import one file, detecting its format.
pub fn import_file(conn: &Connection, path: &Path, now: i64) -> Result<ImportSummary> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if name.ends_with(".ndjson") || name.ends_with(".jsonl") {
        return ndjson::import_file(conn, path, now);
    }

    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;

    // A HAR is recognisable by its shape, not just its extension.
    if name.ends_with(".har") || text.trim_start().starts_with("{\"log\"") {
        return payload::import_har(conn, &text, now);
    }

    let mut summary = payload::import_json_text(conn, &text, None, now)?;
    summary.files = 1;
    Ok(summary)
}

/// Import a path, recursing into directories.
pub fn import_path(conn: &Connection, path: &Path, now: i64) -> Result<ImportSummary> {    if path.is_dir() {
        let mut summary = ImportSummary::default();
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| Error::io(path, e))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .collect();
        // Stable order so an import is reproducible.
        entries.sort();

        for entry in entries {
            if entry.is_dir() {
                continue; // one level; deeper trees are not a supported layout
            }
            match import_file(conn, &entry, now) {
                Ok(s) => summary.merge(s),
                Err(e) => summary
                    .problems
                    .push(format!("{}: {e}", entry.file_name().unwrap_or_default().to_string_lossy())),
            }
        }
        return Ok(summary);
    }

    import_file(conn, path, now)
}

/// Import a JSON payload from a string. Used by the CLI and the paste box.
///
/// Delegates to [`payload::import_json_text`], which is the real
/// implementation; this re-export exists so callers do not need to know which
/// submodule owns it.
pub use payload::import_json_text;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::read;
    use serde_json::json;

    fn tmp(name: &str, contents: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("xdl-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }

    fn tweet(id: &str, text: &str) -> serde_json::Value {
        json!({
            "__typename": "Tweet", "rest_id": id,
            "core": { "user_results": { "result": {
                "rest_id": "42",
                "core": { "name": "Ada", "screen_name": "ada" },
                "legacy": { "name": "Ada", "screen_name": "ada" }
            }}},
            "legacy": { "id_str": id, "full_text": text, "entities": { "urls": [] } }
        })
    }

    #[test]
    fn imports_a_single_tweet_json_file() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let body = json!({ "data": { "tweetResult": { "result": tweet("1", "just one") } } }).to_string();
        let p = tmp("one.json", &body);

        let s = import_file(l.conn(), &p, 1000).unwrap();
        assert_eq!(s.files, 1);
        assert_eq!(s.stats.new, 1);

        let post = read::get_post(l.conn(), "1").unwrap().unwrap();
        assert_eq!(post.tweet.text, "just one");
    }

    #[test]
    fn a_json_file_with_no_tweets_reports_cleanly() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let p = tmp("empty.json", r#"{"data":{"no_tweets_here":true}}"#);
        let s = import_file(l.conn(), &p, 1000).unwrap();
        assert_eq!(s.stats.seen, 0);
        assert_eq!(s.stats.payloads, 0);
    }

    #[test]
    fn imports_a_har_by_extension() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let har = json!({
            "log": { "entries": [
                { "request": { "url": "https://x.com/i/api/graphql/abc/Bookmarks" },
                  "response": { "content": { "text": json!({
                      "data": { "tweetResult": { "result": tweet("7", "from a har") } }
                  }).to_string() } } },
                { "request": { "url": "https://x.com/i/api/1.1/analytics" },
                  "response": { "content": { "text": "{\"not\":\"a tweet\"}" } } }
            ]}
        })
        .to_string();
        let p = tmp("capture.har", &har);

        let s = import_file(l.conn(), &p, 1000).unwrap();
        assert_eq!(s.stats.new, 1, "the tweet in the HAR was not imported");
        assert!(read::get_post(l.conn(), "7").unwrap().is_some());
    }

    #[test]
    fn a_truncated_har_imports_what_it_can() {
        let l = crate::db::Library::open_in_memory().unwrap();
        // Second GraphQL entry's body is truncated mid-JSON — the realistic
        // shape of a capture that was cut off. The first must still import.
        let har = json!({
            "log": { "entries": [
                { "request": { "url": "https://x.com/i/api/graphql/abc/Bookmarks" },
                  "response": { "content": { "text": json!({
                    "data": { "tweetResult": { "result": tweet("8", "good") } }
                }).to_string() } } },
                { "request": { "url": "https://x.com/i/api/graphql/def/TweetDetail" },
                  "response": { "content": { "text": "{\"data\":{\"trunc" } } }
            ]}
        })
        .to_string();
        let p = tmp("truncated.har", &har);

        let s = import_file(l.conn(), &p, 1000).unwrap();
        assert_eq!(s.stats.new, 1, "one good entry should still import");
        assert!(s.problems.is_empty(), "a JSON error is expected, not a problem");
    }

    #[test]
    fn har_entries_without_a_graphql_url_are_skipped_silently() {
        // A real HAR is mostly image and telemetry traffic. Skipping it is the
        // normal path, not an error worth reporting.
        let l = crate::db::Library::open_in_memory().unwrap();
        let har = json!({
            "log": { "entries": [
                { "request": { "url": "https://pbs.twimg.com/media/abc.jpg" },
                  "response": { "content": { "text": "binary" } } },
                { "request": { "url": "https://x.com/i/api/1.1/analytics" },
                  "response": { "content": { "text": "{}" } } },
                { "request": { "url": "https://x.com/i/api/graphql/ghi/Bookmarks" },
                  "response": { "content": { "text": json!({
                      "data": { "tweetResult": { "result": tweet("9", "kept") } }
                  }).to_string() } } }
            ]}
        })
        .to_string();
        let p = tmp("mixed.har", &har);

        let s = import_file(l.conn(), &p, 1000).unwrap();
        assert_eq!(s.stats.new, 1);
        assert!(s.problems.is_empty());
    }

    #[test]
    fn imports_a_directory_and_reports_bad_files_without_aborting() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("xdl-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("a.json"),
            json!({ "data": { "tweetResult": { "result": tweet("10", "a") } } }).to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            json!({ "data": { "tweetResult": { "result": tweet("11", "b") } } }).to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("c.json"), "{ this is not json").unwrap();

        let s = import_path(l.conn(), &dir, 1000).unwrap();
        assert_eq!(s.stats.new, 2, "good files should import");
        assert_eq!(s.problems.len(), 1, "the bad file should be reported");
        assert!(s.problems[0].starts_with("c.json"));
    }
}
