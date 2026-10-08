//! The NDJSON handoff format.
//!
//! ## Why newline-delimited JSON
//!
//! This is the wire format between the userscript and the app, and the fallback
//! file format. It is append-only and line-oriented, which buys three things a
//! single JSON array cannot:
//!
//! 1. **It can be appended to.** The userscript buffers captures and flushes
//!    them; appending a line needs no read-modify-write of the whole file.
//! 2. **A truncated file is still mostly readable.** If the app is killed
//!    mid-flush, every complete line before the cut survives. A truncated
//!    array is unparseable and loses everything.
//! 3. **It streams.** Records can be ingested as they arrive rather than
//!    after the last one.
//!
//! ## The two record kinds
//!
//! ```text
//! {"v":1,"kind":"page",    ..., "raw":      { …full GraphQL envelope, once… }}
//! {"v":1,"kind":"bookmark",..., "raw_tweet":{ …tweet_results.result subtree… }}
//! ```
//!
//! A `page` record carries a whole timeline response and everything in it. A
//! `bookmark` record carries one tweet on its own — which is what continuous
//! capture produces, because a single save is not a page.
//!
//! Splitting them this way is what keeps the "envelope stored once, not per
//! post" rule true on the wire as well as in the database.

use std::path::Path;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

use crate::db::write::{self, SOURCE_CAPTURE};
use crate::error::{Error, Result};
use crate::x;

use super::ImportSummary;

/// The current wire version. Bump only for a breaking change; a reader that
/// sees a higher number should warn and try anyway rather than refuse, since
/// an old app meeting a new script is the recoverable direction.
pub const WIRE_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Line {
    /// A whole captured page.
    Page {
        #[serde(default)]
        v: Option<u32>,
        #[serde(default)]
        captured_at: Option<Value>,
        #[serde(default)]
        op: Option<String>,
        raw: Value,
    },
    /// A single tweet, e.g. from continuous capture.
    Bookmark {
        #[serde(default)]
        v: Option<u32>,
        #[serde(default)]
        captured_at: Option<Value>,
        #[serde(default)]
        op: Option<String>,
        #[serde(default)]
        raw_tweet: Option<Value>,
        /// Our normalised form, when the script had no raw payload. Accepted
        /// so a future stub-only protocol does not need a new wire version.
        #[serde(default)]
        record: Option<Value>,
        #[serde(default)]
        sort_index: Option<String>,
        /// Only continuous capture can supply this — we watched the save.
        #[serde(default)]
        bookmarked_at: Option<i64>,
    },
}

/// Import an NDJSON file.
pub fn import_file(conn: &Connection, path: &Path, now: i64) -> Result<ImportSummary> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    import_str(conn, &text, now)
}

/// Import NDJSON from a string.
pub fn import_str(conn: &Connection, text: &str, now: i64) -> Result<ImportSummary> {
    let mut summary = ImportSummary {
        files: 1,
        ..Default::default()
    };

    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match serde_json::from_str::<Line>(line) {
            Ok(Line::Page {
                v,
                captured_at,
                op,
                raw,
            }) => {
                if let Some(w) = warn_version(v) {
                    summary.problems.push(format!("line {}: {w}", lineno + 1));
                }
                let at = stamp(captured_at, now);
                // Re-serialize the envelope. It round-trips through serde_json
                // faithfully for the types X actually sends (no floats with
                // exotic precision in a bookmark payload), and this is the
                // import path, not the live-capture path, so the stored
                // envelope being a re-serialization is acceptable.
                let body = serde_json::to_string(&raw)?;
                match write::store_payload(
                    conn,
                    &body,
                    op.as_deref().or(Some("Bookmarks")),
                    SOURCE_CAPTURE,
                    at,
                ) {
                    Ok(s) => summary.stats.merge(s),
                    Err(e) => summary.problems.push(format!("line {}: {e}", lineno + 1)),
                }
            }

            Ok(Line::Bookmark {
                v,
                captured_at,
                op,
                raw_tweet,
                record,
                sort_index,
                bookmarked_at,
            }) => {
                if let Some(w) = warn_version(v) {
                    summary.problems.push(format!("line {}: {w}", lineno + 1));
                }
                let at = stamp(captured_at, now);

                // Prefer the raw subtree: it is X's bytes, and storing it means
                // this record benefits from every future parser fix.
                let value = raw_tweet.or(record);
                let Some(value) = value else {
                    summary
                        .problems
                        .push(format!("line {}: bookmark with no raw_tweet or record", lineno + 1));
                    continue;
                };

                // A `bookmark` record is by definition something the user
                // saved, regardless of which operation produced it.
                let _ = op;
                match write::store_tweet_value(
                    conn,
                    &value,
                    SOURCE_CAPTURE,
                    at,
                    true,
                    sort_index.as_deref(),
                    bookmarked_at,
                ) {
                    Ok(Some(is_new)) => {
                        summary.stats.seen += 1;
                        if is_new {
                            summary.stats.new += 1;
                        } else {
                            summary.stats.updated += 1;
                        }
                    }
                    Ok(None) => summary
                        .problems
                        .push(format!("line {}: not a recognisable tweet", lineno + 1)),
                    Err(e) => summary.problems.push(format!("line {}: {e}", lineno + 1)),
                }
            }

            Err(e) => {
                // A malformed line does not abort the file. With an append-only
                // format the realistic corruption is a half-written final line
                // after a crash, and losing the other 4,000 records to it would
                // be absurd.
                summary
                    .problems
                    .push(format!("line {}: {e}", lineno + 1));
            }
        }
    }

    if summary.stats.seen == 0 && summary.problems.is_empty() {
        return Err(Error::NoTweets {
            context: "NDJSON file".into(),
        });
    }

    Ok(summary)
}

/// `captured_at` may arrive as an ISO string or as Unix seconds depending on
/// which side of the wire wrote it.
fn stamp(value: Option<Value>, fallback: i64) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(fallback),
        Some(Value::String(s)) => x::parse_x_timestamp(&s).unwrap_or(fallback),
        _ => fallback,
    }
}

fn warn_version(v: Option<u32>) -> Option<String> {
    match v {
        Some(n) if n > WIRE_VERSION => Some(format!(
            "record claims wire version {n}, this build understands {WIRE_VERSION}; imported anyway"
        )),
        _ => None,
    }
}

/// Serialize a record for writing, used by the CLI's `--emit` and by tests.
pub fn page_line(op: &str, captured_at: i64, raw: &Value) -> Result<String> {
    Ok(serde_json::to_string(&serde_json::json!({
        "v": WIRE_VERSION,
        "kind": "page",
        "captured_at": captured_at,
        "op": op,
        "raw": raw,
    }))?)
}

/// As [`page_line`], for a single tweet.
pub fn bookmark_line(raw_tweet: &Value, captured_at: i64) -> Result<String> {
    Ok(serde_json::to_string(&serde_json::json!({
        "v": WIRE_VERSION,
        "kind": "bookmark",
        "captured_at": captured_at,
        "raw_tweet": raw_tweet,
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::read;
    use serde_json::json;

    fn tweet(id: &str, text: &str) -> Value {
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
    fn imports_a_page_record() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let line = page_line(
            "Bookmarks",
            1000,
            &json!({ "data": { "tweetResult": { "result": tweet("1", "page tweet") } } }),
        )
        .unwrap();

        let s = import_str(l.conn(), &line, 2000).unwrap();
        assert_eq!(s.stats.new, 1);
        assert_eq!(l.count_bookmarks().unwrap(), 1);
    }

    #[test]
    fn imports_a_single_bookmark_record_and_keeps_its_observed_save_time() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let line = json!({
            "v": 1, "kind": "bookmark", "captured_at": 5000,
            "raw_tweet": tweet("2", "saved live"),
            "bookmarked_at": 4999
        })
        .to_string();

        import_str(l.conn(), &line, 6000).unwrap();

        let post = read::get_post(l.conn(), "2").unwrap().unwrap();
        // Continuous capture is the only source of a real save time.
        assert_eq!(post.bookmarked_at, Some(4999));
        assert!(post.tweet.text.contains("saved live"));
    }

    #[test]
    fn a_backfill_page_leaves_the_save_time_null() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let line = page_line(
            "Bookmarks",
            1000,
            &json!({ "data": { "tweetResult": { "result": tweet("3", "bulk scrolled") } } }),
        )
        .unwrap();

        import_str(l.conn(), &line, 2000).unwrap();

        let post = read::get_post(l.conn(), "3").unwrap().unwrap();
        assert_eq!(post.bookmarked_at, None, "backfill must not invent a save time");
    }

    #[test]
    fn accepts_an_iso_timestamp_as_well_as_unix() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let line = json!({
            "v": 1, "kind": "bookmark",
            "captured_at": "2018-10-10T20:19:24Z",
            "raw_tweet": tweet("4", "iso stamped")
        })
        .to_string();

        import_str(l.conn(), &line, 6000).unwrap();
        let post = read::get_post(l.conn(), "4").unwrap().unwrap();
        assert_eq!(post.first_seen_at, 1_539_202_764);
    }

    #[test]
    fn a_corrupt_line_does_not_lose_the_rest_of_the_file() {
        // The realistic corruption: a half-written final line after a crash.
        let l = crate::db::Library::open_in_memory().unwrap();
        let good1 = bookmark_line(&tweet("10", "first"), 1000).unwrap();
        let good2 = bookmark_line(&tweet("11", "second"), 1001).unwrap();
        let text = format!("{good1}\n{good2}\n{{\"v\":1,\"kind\":\"bookm");

        let s = import_str(l.conn(), &text, 2000).unwrap();
        assert_eq!(s.stats.new, 2, "good lines should survive a corrupt one");
        assert_eq!(s.problems.len(), 1);
        assert!(s.problems[0].starts_with("line 3"));
    }

    #[test]
    fn blank_lines_and_comments_are_ignored() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let text = format!(
            "# a comment the script wrote\n\n{}\n\n",
            bookmark_line(&tweet("12", "x"), 1000).unwrap()
        );
        let s = import_str(l.conn(), &text, 2000).unwrap();
        assert_eq!(s.stats.new, 1);
        assert!(s.problems.is_empty());
    }

    #[test]
    fn a_newer_wire_version_warns_but_still_imports() {
        // An old app meeting a new script is the recoverable direction, so we
        // try rather than refuse.
        let l = crate::db::Library::open_in_memory().unwrap();
        let line = json!({
            "v": 99, "kind": "bookmark", "captured_at": 1000,
            "raw_tweet": tweet("13", "from the future")
        })
        .to_string();

        let s = import_str(l.conn(), &line, 2000).unwrap();
        assert_eq!(s.stats.new, 1, "a future version should still import");
        assert!(s.problems[0].contains("wire version 99"));
    }

    #[test]
    fn an_unknown_record_kind_is_reported_not_fatal() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let text = format!(
            "{}\n{}\n",
            json!({ "v": 1, "kind": "some_future_thing", "data": 1 }),
            bookmark_line(&tweet("14", "fine"), 1000).unwrap()
        );
        let s = import_str(l.conn(), &text, 2000).unwrap();
        assert_eq!(s.stats.new, 1);
        assert_eq!(s.problems.len(), 1);
    }

    #[test]
    fn a_completely_empty_file_is_reported() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let err = import_str(l.conn(), "\n\n# nothing here\n", 1000).unwrap_err();
        assert!(matches!(err, Error::NoTweets { .. }));
    }

    #[test]
    fn re_importing_the_same_ndjson_is_a_no_op() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let text = format!(
            "{}\n{}\n",
            bookmark_line(&tweet("15", "one"), 1000).unwrap(),
            bookmark_line(&tweet("16", "two"), 1001).unwrap()
        );

        let a = import_str(l.conn(), &text, 2000).unwrap();
        let b = import_str(l.conn(), &text, 3000).unwrap();
        assert_eq!(a.stats.new, 2);
        assert_eq!(b.stats.new, 0);
        assert_eq!(b.stats.updated, 2);
        assert_eq!(l.count_bookmarks().unwrap(), 2);
    }
}
