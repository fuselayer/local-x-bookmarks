//! Reading posts back out.
//!
//! ## Why reads re-parse instead of selecting parsed columns
//!
//! `raw_tweet` holds X's original subtree. Building a [`PostView`] means
//! running the current parser over those bytes at read time rather than
//! trusting the denormalised columns.
//!
//! That costs a little CPU per read (microseconds — the rows are small) and
//! buys the property the whole storage design exists for: **a parser fix
//! applies retroactively to everything already captured**, with no
//! re-capture, no migration, and no backfill job. If X renames a field
//! tomorrow, the fix ships and yesterday's posts render correctly.
//!
//! The denormalised columns are still there, but they are for *filtering and
//! sorting* — `WHERE created_at > ?`, `ORDER BY sort_index` — not for
//! rendering.

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::error::Result;
use crate::model::{PostView, Tweet};
use crate::x;

/// Rebuild a [`Tweet`] from a stored `raw_tweet` column.
///
/// Two shapes are accepted, deliberately:
///
/// 1. X's original `tweet_results.result` subtree — the normal case.
/// 2. Our own serialised [`Tweet`] — only written by paths that never held a
///    raw payload.
///
/// Accepting both means a change to what we store cannot brick existing rows.
pub fn parse_stored_tweet(raw: &str) -> Option<Tweet> {
    let value: Value = serde_json::from_str(raw).ok()?;
    if let Some(t) = x::parse_tweet_result(&value) {
        return Some(t);
    }
    serde_json::from_value(value).ok()
}

const POST_COLUMNS: &str = "
    p.id, p.raw_tweet, p.first_seen_at, p.last_seen_at, p.removed_at, p.source,
    b.bookmarked_at, b.sort_index";

fn row_to_view(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<PostView>> {
    let raw: Option<String> = row.get(1)?;
    let Some(raw) = raw else { return Ok(None) };
    let Some(tweet) = parse_stored_tweet(&raw) else {
        // A row we cannot interpret. Skipped rather than fatal: one bad row
        // must not make the library unopenable.
        return Ok(None);
    };

    Ok(Some(PostView {
        tweet,
        bookmarked_at: row.get(6)?,
        sort_index: row.get(7)?,
        first_seen_at: row.get(2)?,
        last_seen_at: row.get(3)?,
        removed_at: row.get(4)?,
        source: row.get(5)?,
    }))
}

/// One post by ID, whether or not it is bookmarked.
pub fn get_post(conn: &Connection, id: &str) -> Result<Option<PostView>> {
    let sql = format!(
        "SELECT {POST_COLUMNS}
           FROM posts p
           LEFT JOIN bookmarks b ON b.post_id = p.id
          WHERE p.id = ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(row_to_view(row)?),
        None => Ok(None),
    }
}

/// The library, newest bookmark first.
///
/// Ordering is by `sort_index` — the bookmark ordering key X itself assigns —
/// descending, so the list matches what the user sees on x.com. It is
/// deliberately **not** `bookmarked_at`: that column is null for everything
/// captured from a backfill scroll, which is most of an established library,
/// and sorting by it would silently bury the oldest and most valuable rows
/// (PRD §6.3).
pub fn list_bookmarks(conn: &Connection, limit: usize, offset: usize) -> Result<Vec<PostView>> {
    let sql = format!(
        "SELECT {POST_COLUMNS}
           FROM bookmarks b
           JOIN posts p ON p.id = b.post_id
          ORDER BY CAST(b.sort_index AS INTEGER) DESC NULLS LAST,
                   p.created_at DESC
          LIMIT ?1 OFFSET ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![limit as i64, offset as i64])?;

    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        if let Some(view) = row_to_view(row)? {
            out.push(view);
        }
    }
    Ok(out)
}

/// Aggregate counts for the status bar and the CLI summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    pub posts: i64,
    pub bookmarks: i64,
    pub authors: i64,
    pub media: i64,
    pub payloads: i64,
    /// Posts flagged as no longer present in a later capture.
    pub removed: i64,
}

pub fn stats(conn: &Connection) -> Result<LibraryStats> {
    let one = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
    Ok(LibraryStats {
        posts: one("SELECT count(*) FROM posts")?,
        bookmarks: one("SELECT count(*) FROM bookmarks")?,
        authors: one("SELECT count(*) FROM authors")?,
        media: one("SELECT count(*) FROM media")?,
        payloads: one("SELECT count(*) FROM capture_payloads")?,
        removed: one("SELECT count(*) FROM posts WHERE removed_at IS NOT NULL")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::write::{store_payload, SOURCE_IMPORT};
    use serde_json::json;

    fn tweet_result(id: &str, text: &str) -> Value {
        json!({
            "__typename": "Tweet",
            "rest_id": id,
            "core": { "user_results": { "result": {
                "rest_id": "42",
                "core": { "name": "Ada Lovelace", "screen_name": "ada" },
                "avatar": { "image_url": "https://pbs.twimg.com/a.jpg" }
            }}},
            "legacy": {
                "id_str": id,
                "full_text": text,
                "created_at": "Wed Oct 10 20:19:24 +0000 2018",
                "entities": { "urls": [] }
            }
        })
    }

    fn single_tweet_payload(id: &str, text: &str) -> String {
        json!({ "data": { "tweetResult": { "result": tweet_result(id, text) } } }).to_string()
    }

    /// Two bookmarks, in a real bookmarks-page envelope with sortIndex.
    fn seeded() -> crate::db::Library {
        let l = crate::db::Library::open_in_memory().unwrap();
        let body = json!({
            "data": { "bookmark_timeline_v2": { "timeline": { "instructions": [
                { "type": "TimelineAddEntries", "entries": [
                    { "entryId": "tweet-1", "sortIndex": "900",
                      "content": { "entryType": "TimelineTimelineItem",
                        "itemContent": { "tweet_results": { "result": tweet_result("1", "first") } } } },
                    { "entryId": "tweet-2", "sortIndex": "800",
                      "content": { "entryType": "TimelineTimelineItem",
                        "itemContent": { "tweet_results": { "result": tweet_result("2", "second") } } } }
                ]}
            ]}}}
        })
        .to_string();

        store_payload(l.conn(), &body, Some("Bookmarks"), SOURCE_IMPORT, 1000).unwrap();
        l
    }

    #[test]
    fn round_trips_a_single_tweet() {
        let l = crate::db::Library::open_in_memory().unwrap();
        store_payload(
            l.conn(),
            &single_tweet_payload("123", "hello"),
            Some("TweetDetail"),
            SOURCE_IMPORT,
            1000,
        )
        .unwrap();

        let p = get_post(l.conn(), "123").unwrap().expect("post should exist");
        assert_eq!(p.tweet.text, "hello");
        assert_eq!(p.tweet.author.handle, "ada");
        assert_eq!(p.tweet.author.name, "Ada Lovelace");
        assert_eq!(p.tweet.created_at, Some(1_539_202_764));
        assert_eq!(p.source, "file_import");
        // Not bookmarked: TweetDetail is enrichment, not a bookmark source.
        assert!(p.bookmarked_at.is_none());
    }

    #[test]
    fn lists_bookmarks_in_x_ordering() {
        let l = seeded();
        let posts = list_bookmarks(l.conn(), 10, 0).unwrap();
        assert_eq!(posts.len(), 2);
        // sortIndex 900 > 800, so "first" leads — matching x.com's order.
        assert_eq!(posts[0].tweet.text, "first");
        assert_eq!(posts[1].tweet.text, "second");
        assert_eq!(posts[0].sort_index.as_deref(), Some("900"));
    }

    #[test]
    fn pagination_does_not_repeat_or_skip() {
        let l = seeded();
        let page1 = list_bookmarks(l.conn(), 1, 0).unwrap();
        let page2 = list_bookmarks(l.conn(), 1, 1).unwrap();
        assert_eq!(page1[0].tweet.text, "first");
        assert_eq!(page2[0].tweet.text, "second");
    }

    #[test]
    fn a_missing_post_is_none_not_an_error() {
        let l = crate::db::Library::open_in_memory().unwrap();
        assert!(get_post(l.conn(), "does-not-exist").unwrap().is_none());
    }

    #[test]
    fn parses_the_original_subtree_preferentially() {
        // X's shape.
        let x_subtree = r#"{"__typename":"Tweet","rest_id":"5",
            "legacy":{"id_str":"5","full_text":"from x"}}"#;
        assert_eq!(parse_stored_tweet(x_subtree).unwrap().text, "from x");

        // Our own model, for rows written by a path with no raw payload.
        let ours = r#"{"id":"6","author":{"id":"1","handle":"h","name":"n",
            "avatarUrl":null,"verified":false,"blueVerified":false,"verifiedType":null},
            "text":"from us","truncatedText":null,"isLongForm":false,
            "entities":{"units":"utf16","urls":[],"mentions":[],"hashtags":[],
                        "symbols":[],"mediaIndices":[]},
            "createdAt":null,"lang":null,"conversationId":null,"inReplyToId":null,
            "inReplyToHandle":null,
            "metrics":{"likes":0,"reposts":0,"replies":0,"quotes":0,"bookmarks":0,"views":null},
            "media":[],"card":null,"quoted":null,"parserVersion":1}"#;
        assert_eq!(parse_stored_tweet(ours).unwrap().text, "from us");
    }

    #[test]
    fn garbage_in_raw_tweet_does_not_break_reads() {
        assert!(parse_stored_tweet("not json").is_none());
        assert!(parse_stored_tweet("{}").is_none());
    }

    #[test]
    fn stats_counts_what_the_ui_shows() {
        let l = seeded();
        let s = stats(l.conn()).unwrap();
        assert_eq!(s.bookmarks, 2);
        assert_eq!(s.posts, 2);
        assert_eq!(s.authors, 1);
        assert_eq!(s.payloads, 1);
        assert_eq!(s.removed, 0);
    }
}
