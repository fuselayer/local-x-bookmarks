//! Importing raw payload files: single JSON responses and HAR captures.

use rusqlite::Connection;
use serde_json::Value;

use crate::db::write::{self, IngestStats, SOURCE_IMPORT};
use crate::error::{Error, Result};
use crate::x;

use super::ImportSummary;

/// Import a JSON payload from text.
///
/// `op` is the GraphQL operation name when the caller knows it. It decides
/// whether the tweets become **bookmarks** or merely **posts that exist** —
/// see `write::is_bookmark_operation`. When unknown we assume a bookmarks
/// source, because the overwhelmingly common case for importing a bare
/// payload by hand is "here is a tweet I saved".
pub fn import_json_text(
    conn: &Connection,
    text: &str,
    op: Option<&str>,
    now: i64,
) -> Result<ImportSummary> {
    let value: Value = serde_json::from_str(text)?;

    // Some exports wrap the payload in an array of responses. Try each.
    if let Value::Array(items) = &value {
        let mut summary = ImportSummary::default();
        for item in items {
            let body = serde_json::to_string(item)?;
            summary.stats.merge(write::store_payload(conn, &body, op, SOURCE_IMPORT, now)?);
        }
        return Ok(summary);
    }

    let stats = write::store_payload(conn, text, op, SOURCE_IMPORT, now)?;
    Ok(ImportSummary {
        files: 1,
        stats,
        problems: Vec::new(),
    })
}

/// Import a HAR (HTTP Archive) file.
///
/// ## Why this path exists even though we have a userscript
///
/// A HAR needs no install, no userscript manager, and no trust in our code —
/// the user exports it themselves from DevTools. That makes it the honest
/// answer to "the userscript broke and I need my data out", which PRD §9.4
/// requires. It is also, usefully, the easiest way for someone to hand us a
/// single real payload for parser development.
///
/// ## Tolerance
///
/// HARs are full of analytics beacons, image fetches and telemetry. Most
/// entries contain no tweets at all, which is the normal case and not an
/// error. Entries whose bodies are truncated or binary are skipped. Only if
/// the whole file fails to parse do we report a problem.
pub fn import_har(conn: &Connection, text: &str, now: i64) -> Result<ImportSummary> {
    let har: Value = serde_json::from_str(text)?;

    let entries = har
        .pointer("/log/entries")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::UnrecognisedPayload("HAR has no log.entries array".into()))?;

    let mut summary = ImportSummary {
        files: 1,
        ..Default::default()
    };

    for entry in entries {
        let url = entry
            .pointer("/request/url")
            .and_then(Value::as_str)
            .unwrap_or_default();

        // GraphQL responses are the only ones that can carry tweets. Skipping
        // the rest early avoids parsing megabytes of image data.
        if !x::is_graphql_url(url) {
            continue;
        }

        let Some(body) = entry.pointer("/response/content/text").and_then(Value::as_str) else {
            continue;
        };
        if body.trim().is_empty() {
            continue;
        }

        // The operation name from the URL is what decides bookmark-ness, so
        // a HAR of a bookmarks scroll correctly produces bookmarks and a HAR
        // of the home timeline correctly does not.
        let op = x::operation_name(url);

        match write::store_payload(conn, body, op, SOURCE_IMPORT, now) {
            Ok(stats) => summary.stats.merge(stats),
            // A truncated or non-JSON body is expected in a real HAR.
            Err(Error::Json(_)) => continue,
            Err(e) => summary.problems.push(format!("{url}: {e}")),
        }
    }

    if summary.stats.seen == 0 && summary.problems.is_empty() {
        return Err(Error::NoTweets {
            context: "HAR (no GraphQL responses contained tweets)".into(),
        });
    }

    Ok(summary)
}

/// Convenience for importing a payload already parsed into a `Value`.
pub fn import_json_value(
    conn: &Connection,
    value: &Value,
    op: Option<&str>,
    now: i64,
) -> Result<IngestStats> {
    let text = serde_json::to_string(value)?;
    write::store_payload(conn, &text, op, SOURCE_IMPORT, now)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn har_operation_name_decides_bookmark_status() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let har = json!({
            "log": { "entries": [
                { "request": { "url": "https://x.com/i/api/graphql/h1/HomeTimeline" },
                  "response": { "content": { "text": json!({
                      "data": { "tweetResult": { "result": tweet("20", "scrolled past") } }
                  }).to_string() } } },
                { "request": { "url": "https://x.com/i/api/graphql/h2/Bookmarks" },
                  "response": { "content": { "text": json!({
                      "data": { "tweetResult": { "result": tweet("21", "actually saved") } }
                  }).to_string() } } }
            ]}
        })
        .to_string();

        import_har(l.conn(), &har, 1000).unwrap();

        // Both posts exist, which is right — we saw both.
        assert!(crate::db::read::get_post(l.conn(), "20").unwrap().is_some());
        assert!(crate::db::read::get_post(l.conn(), "21").unwrap().is_some());
        // But only one is a bookmark.
        assert_eq!(l.count_bookmarks().unwrap(), 1);
    }

    #[test]
    fn har_without_any_tweets_is_reported_not_silently_empty() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let har = json!({ "log": { "entries": [
            { "request": { "url": "https://example.com/api" },
              "response": { "content": { "text": "{}" } } }
        ]}})
        .to_string();

        let err = import_har(l.conn(), &har, 1000).unwrap_err();
        assert!(matches!(err, Error::NoTweets { .. }));
    }

    #[test]
    fn har_without_entries_is_unrecognised() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let err = import_har(l.conn(), r#"{"log":{}}"#, 1000).unwrap_err();
        assert!(matches!(err, Error::UnrecognisedPayload(_)));
    }

    #[test]
    fn an_array_of_payloads_imports_every_element() {
        let l = crate::db::Library::open_in_memory().unwrap();
        let arr = json!([
            { "data": { "tweetResult": { "result": tweet("30", "one") } } },
            { "data": { "tweetResult": { "result": tweet("31", "two") } } }
        ])
        .to_string();

        let s = import_json_text(l.conn(), &arr, Some("Bookmarks"), 1000).unwrap();
        assert_eq!(s.stats.new, 2);
    }
}
