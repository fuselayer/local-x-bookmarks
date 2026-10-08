//! Storing captured payloads.
//!
//! All functions here take `&Connection` rather than `&mut Connection`, so a
//! caller can wrap a whole file import in one transaction (via `Transaction`,
//! which derefs to `Connection`) without this module knowing about it. That
//! matters: a 10k-post import inside one transaction is roughly two orders of
//! magnitude faster than one commit per post.
//!
//! ## Idempotency
//!
//! Every write is an upsert against a natural key, and every child table is
//! cleared-and-rewritten for the post rather than appended to. Importing the
//! same file twice must leave the database byte-identical in content. There
//! is a test for exactly that, because the userscript's buffer-and-retry
//! behaviour means duplicate deliveries are expected, not exceptional.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::model::{Author, Media, Tweet};
use crate::x::{self, PARSER_VERSION};

/// Where a record came from. Stored verbatim in `posts.source`.
pub const SOURCE_CAPTURE: &str = "capture";
pub const SOURCE_IMPORT: &str = "file_import";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IngestStats {
    /// Tweets present in the payload.
    pub seen: usize,
    /// Tweets not previously in the library.
    pub new: usize,
    /// Tweets already present that we refreshed.
    pub updated: usize,
    /// Page envelopes written (0 or 1 per payload).
    pub payloads: usize,
}

impl IngestStats {
    pub fn merge(&mut self, other: Self) {
        self.seen += other.seen;
        self.new += other.new;
        self.updated += other.updated;
        self.payloads += other.payloads;
    }
}

/// Content-address a raw payload body.
///
/// This is the primary key of `capture_payloads`, which means re-capturing a
/// page we already have costs one hash comparison instead of 200 row writes.
pub fn payload_id(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// Store one GraphQL payload: the envelope once, plus every tweet in it.
///
/// `raw` is the exact response body text, needed unmodified so the hash is
/// reproducible. `observed_at` is Unix seconds.
pub fn store_payload(
    conn: &Connection,
    raw: &str,
    op: Option<&str>,
    source: &str,
    observed_at: i64,
) -> Result<IngestStats> {
    let value: Value = serde_json::from_str(raw)?;
    store_payload_value(conn, &value, raw, op, source, observed_at)
}

/// As [`store_payload`], but for a payload already parsed into a `Value`.
///
/// `raw_text` is still required so the envelope can be content-addressed and
/// stored verbatim — re-serializing the `Value` would produce different bytes
/// (key order, float formatting) and break the hash's stability.
pub fn store_payload_value(
    conn: &Connection,
    value: &Value,
    raw_text: &str,
    op: Option<&str>,
    source: &str,
    observed_at: i64,
) -> Result<IngestStats> {
    let extracted = x::extract(value);

    let mut stats = IngestStats {
        seen: extracted.tweets.len(),
        ..Default::default()
    };

    if extracted.tweets.is_empty() {
        // Nothing worth keeping. Storing an empty envelope would just be
        // disk for no replay value.
        return Ok(stats);
    }

    // ── the envelope, once ───────────────────────────────────────────────
    let pid = payload_id(raw_text);
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO capture_payloads
           (id, op, captured_at, cursor_after, bytes, tweet_count, raw)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            pid,
            op,
            observed_at,
            extracted.bottom_cursor,
            raw_text.len() as i64,
            extracted.tweets.len() as i64,
            raw_text
        ],
    )?;
    stats.payloads = inserted;

    // ── the tweets ───────────────────────────────────────────────────────
    for tweet in &extracted.tweets {
        let is_new = upsert_tweet(
            conn,
            tweet,
            extracted.raw_of(&tweet.id),
            source,
            observed_at,
            0,
        )?;
        if is_new {
            stats.new += 1;
        } else {
            stats.updated += 1;
        }

        conn.execute(
            "INSERT OR IGNORE INTO post_payloads (post_id, payload_id) VALUES (?1, ?2)",
            params![tweet.id, pid],
        )?;

        // Being present in a bookmarks-family payload is what makes a post a
        // bookmark. `TweetDetail` and home-timeline captures enrich posts but
        // must not claim the user bookmarked them.
        let is_bookmarks_op = op.map(is_bookmark_operation).unwrap_or(true);
        if is_bookmarks_op {
            mark_bookmarked(
                conn,
                &tweet.id,
                extracted.sort_index_of(&tweet.id),
                None,
                observed_at,
            )?;
        }
    }

    Ok(stats)
}

/// True for operations whose results *are* the user's bookmarks.
///
/// `TweetDetail`, `HomeTimeline` and the rest are enrichment sources only.
/// Getting this wrong is the difference between a library of your bookmarks
/// and a library of everything you ever scrolled past.
pub fn is_bookmark_operation(op: &str) -> bool {
    op.starts_with("Bookmark")
}

/// Store a single tweet from its `tweet_results.result` subtree.
///
/// Unlike [`store_payload_value`] this writes no `capture_payloads` row: the
/// caller has one tweet, not a page envelope, and inventing an envelope to
/// wrap it in would put fabricated bytes in the table whose entire purpose is
/// holding X's real bytes.
///
/// Returns `None` if the value is not a tweet we can parse.
#[allow(clippy::too_many_arguments)]
pub fn store_tweet_value(
    conn: &Connection,
    raw_tweet: &Value,
    source: &str,
    observed_at: i64,
    bookmarked: bool,
    sort_index: Option<&str>,
    bookmarked_at: Option<i64>,
) -> Result<Option<bool>> {
    let Some(tweet) = crate::x::parse_tweet_result(raw_tweet) else {
        return Ok(None);
    };

    let is_new = upsert_tweet(conn, &tweet, Some(raw_tweet), source, observed_at, 0)?;

    if bookmarked {
        mark_bookmarked(conn, &tweet.id, sort_index, bookmarked_at, observed_at)?;
    }

    Ok(Some(is_new))
}

/// Insert or refresh a tweet and everything hanging off it.
///
/// `raw` is X's original `tweet_results.result` subtree for this tweet, stored
/// verbatim so a later parser fix can re-derive every field from the bytes X
/// actually sent. It is `None` only on paths that never had a raw payload
/// (a hand-built record), in which case `read` falls back to interpreting the
/// column as our own model.
///
/// Returns `true` if the post row was newly created.
pub fn upsert_tweet(
    conn: &Connection,
    tweet: &Tweet,
    raw: Option<&Value>,
    source: &str,
    observed_at: i64,
    depth: u32,
) -> Result<bool> {
    upsert_author(conn, &tweet.author, observed_at)?;

    let raw_json = match raw {
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        // No raw subtree available. Store our model instead so the row is
        // never unreadable; `read` distinguishes the two shapes.
        None => serde_json::to_string(tweet).unwrap_or_default(),
    };
    let media_count = tweet.media.len() as i64;
    let has_video = tweet
        .media
        .iter()
        .any(|m| m.video_url.is_some()) as i64;
    let card_kind = tweet.card.as_ref().and_then(|c| c.name.clone());
    let card_domain = tweet.card.as_ref().and_then(|c| c.domain.clone());

    // The update rule, which is the subtle part.
    //
    // A post can arrive twice from different sources with different fidelity:
    // a timeline capture gives the truncated `full_text`, while a
    // `TweetDetail` capture gives the full `note_tweet` body. Last-write-wins
    // would let the truncated version overwrite the complete one, silently
    // truncating the user's long posts.
    //
    // So: take the incoming text when it is long-form, or when what we
    // already hold is not. Otherwise keep what we have.
    let existed: Option<String> = conn
        .query_row("SELECT id FROM posts WHERE id = ?1", params![tweet.id], |r| {
            r.get(0)
        })
        .optional()?;
    let is_new = existed.is_none();

    conn.execute(
        "INSERT INTO posts (
            id, author_id, text, note_text, is_long_form, created_at, lang,
            conversation_id, in_reply_to_id, in_reply_to_handle,
            like_count, repost_count, reply_count, quote_count, bookmark_count,
            view_count, has_media, has_video, quoted_id, card_kind, card_domain,
            parser_version, raw_tweet, first_seen_at, last_seen_at, removed_at, source
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?24, NULL, ?25
         )
         ON CONFLICT(id) DO UPDATE SET
            author_id       = excluded.author_id,
            text            = CASE
                                WHEN excluded.is_long_form = 1 OR posts.is_long_form = 0
                                THEN excluded.text ELSE posts.text END,
            note_text       = COALESCE(excluded.note_text, posts.note_text),
            is_long_form    = MAX(posts.is_long_form, excluded.is_long_form),
            created_at      = COALESCE(excluded.created_at, posts.created_at),
            lang            = COALESCE(excluded.lang, posts.lang),
            conversation_id = COALESCE(excluded.conversation_id, posts.conversation_id),
            in_reply_to_id  = COALESCE(excluded.in_reply_to_id, posts.in_reply_to_id),
            in_reply_to_handle = COALESCE(excluded.in_reply_to_handle, posts.in_reply_to_handle),
            like_count      = excluded.like_count,
            repost_count    = excluded.repost_count,
            reply_count     = excluded.reply_count,
            quote_count     = excluded.quote_count,
            bookmark_count  = excluded.bookmark_count,
            view_count      = COALESCE(excluded.view_count, posts.view_count),
            has_media       = excluded.has_media,
            has_video       = excluded.has_video,
            quoted_id       = COALESCE(excluded.quoted_id, posts.quoted_id),
            card_kind       = COALESCE(excluded.card_kind, posts.card_kind),
            card_domain     = COALESCE(excluded.card_domain, posts.card_domain),
            parser_version  = excluded.parser_version,
            raw_tweet       = excluded.raw_tweet,
            last_seen_at    = excluded.last_seen_at,
            -- Seen again means present again, so a removal flag is cleared.
            removed_at      = NULL",
        params![
            tweet.id,
            tweet.author.id,
            tweet.text,
            if tweet.is_long_form { Some(tweet.text.as_str()) } else { None },
            tweet.is_long_form as i64,
            tweet.created_at,
            tweet.lang,
            tweet.conversation_id,
            tweet.in_reply_to_id,
            tweet.in_reply_to_handle,
            tweet.metrics.likes,
            tweet.metrics.reposts,
            tweet.metrics.replies,
            tweet.metrics.quotes,
            tweet.metrics.bookmarks,
            tweet.metrics.views,
            media_count,
            has_video,
            tweet.quoted.as_ref().map(|q| q.id.clone()),
            card_kind,
            card_domain,
            PARSER_VERSION as i64,
            raw_json,
            observed_at,
            source,
        ],
    )?;

    replace_media(conn, tweet)?;
    replace_links(conn, tweet)?;
    refresh_search_doc(conn, tweet)?;

    // ── the quoted post ──────────────────────────────────────────────────
    // Stored as a real post so it renders inline, but it is NOT a bookmark:
    // quoting something is not saving it. Recursion is capped at one level,
    // which is all X itself renders.
    if let Some(quoted) = &tweet.quoted {
        if depth == 0 {
            // Recover the quoted post's own subtree from the parent's raw, so
            // it too is stored verbatim rather than as our interpretation.
            let quoted_raw = raw.and_then(|r| r.pointer("/quoted_status_result/result"));
            upsert_tweet(conn, quoted, quoted_raw, source, observed_at, depth + 1)?;
        }
        conn.execute(
            "INSERT INTO refs (post_id, kind, ref_post_id) VALUES (?1, 'quoted', ?2)
             ON CONFLICT(post_id, kind) DO UPDATE SET ref_post_id = excluded.ref_post_id",
            params![tweet.id, quoted.id],
        )?;
    }

    if let Some(parent) = &tweet.in_reply_to_id {
        conn.execute(
            "INSERT INTO refs (post_id, kind, ref_post_id) VALUES (?1, 'replied_to', ?2)
             ON CONFLICT(post_id, kind) DO UPDATE SET ref_post_id = excluded.ref_post_id",
            params![tweet.id, parent],
        )?;
    }

    Ok(is_new)
}

fn upsert_author(conn: &Connection, author: &Author, now: i64) -> Result<()> {
    if author.id.is_empty() {
        // A tweet whose author object we could not read. The post is still
        // worth keeping; it just has no author row to point at.
        return Ok(());
    }

    conn.execute(
        "INSERT INTO authors (id, handle, name, avatar_url, verified, blue_verified, verified_type, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            handle        = excluded.handle,
            name          = excluded.name,
            -- Handles and display names change; avatars change more often.
            -- A missing avatar in a later payload must not blank a good one.
            avatar_url    = COALESCE(excluded.avatar_url, authors.avatar_url),
            verified      = excluded.verified,
            blue_verified = excluded.blue_verified,
            verified_type = excluded.verified_type,
            fetched_at    = excluded.fetched_at",
        params![
            author.id,
            author.handle,
            author.name,
            author.avatar_url,
            author.verified as i64,
            author.blue_verified as i64,
            author.verified_type,
            now,
        ],
    )?;
    Ok(())
}

/// Clear and rewrite a post's media.
///
/// Replace rather than merge, because `position` is meaningful: the first
/// image in a four-image grid must stay first. A merge would leave a stale
/// image at position 2 if the tweet was edited.
fn replace_media(conn: &Connection, tweet: &Tweet) -> Result<()> {
    conn.execute("DELETE FROM media WHERE post_id = ?1", params![tweet.id])?;

    let mut stmt = conn.prepare_cached(
        "INSERT INTO media (post_id, position, kind, url, video_url, alt_text, width, height, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )?;

    for (position, m) in tweet.media.iter().enumerate() {
        stmt.execute(params![
            tweet.id,
            position as i64,
            media_kind_str(m),
            m.url,
            m.video_url,
            m.alt_text,
            m.width,
            m.height,
            m.duration_ms,
        ])?;
    }
    Ok(())
}

fn media_kind_str(m: &Media) -> &'static str {
    use crate::model::MediaKind::*;
    match m.kind {
        Photo => "photo",
        Video => "video",
        Gif => "gif",
    }
}

/// Rewrite a post's link rows from its URL entities and card.
fn replace_links(conn: &Connection, tweet: &Tweet) -> Result<()> {
    conn.execute("DELETE FROM links WHERE post_id = ?1", params![tweet.id])?;

    let card_title = tweet.card.as_ref().and_then(|c| c.title.clone());
    let card_desc = tweet.card.as_ref().and_then(|c| c.description.clone());

    let mut stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO links (post_id, tco, expanded, display, domain, title, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;

    for u in &tweet.entities.urls {
        // The domain drives the `link:` filter, so derive it from the expanded
        // URL rather than from the t.co shortener — indexing `t.co` as a
        // domain would make every link look like it points at X.
        let domain = domain_from_url(&u.expanded);

        stmt.execute(params![
            tweet.id,
            u.url,
            u.expanded,
            u.display,
            domain,
            card_title,
            card_desc,
        ])?;
    }

    // A card with no matching URL entity still deserves a row, so
    // `has:link` and domain filters see it.
    if tweet.entities.urls.is_empty() {
        if let Some(card) = &tweet.card {
            if card.url.is_some() || card.title.is_some() {
                stmt.execute(params![
                    tweet.id,
                    Option::<String>::None,
                    card.url,
                    card.domain,
                    card.domain,
                    card.title,
                    card.description,
                ])?;
            }
        }
    }

    Ok(())
}

/// `https://github.com/a/b` -> `github.com`. Mirrors `x::tweet`'s helper;
/// duplicated here only to avoid making that one public API.
fn domain_from_url(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.strip_prefix("www.").unwrap_or(host).to_ascii_lowercase())
}

/// Rebuild the flattened FTS projection for one post.
///
/// An UPSERT, deliberately — when a stub post is later enriched with a
/// `note_tweet`, this fires the AFTER UPDATE trigger and the index follows.
/// That path is exactly why the FTS tables are external-content rather than
/// contentless (PRD §7.6).
fn refresh_search_doc(conn: &Connection, tweet: &Tweet) -> Result<()> {
    // Split the two text fields so a long post is not indexed twice: `text`
    // holds the short form, `note_text` the long body.
    let (short_text, note_text) = if tweet.is_long_form {
        (
            tweet.truncated_text.clone().unwrap_or_default(),
            tweet.text.clone(),
        )
    } else {
        (tweet.text.clone(), String::new())
    };

    let alt_text = tweet
        .media
        .iter()
        .filter_map(|m| m.alt_text.as_deref())
        .collect::<Vec<_>>()
        .join(" ");

    // Titles come from the card; the display URLs are worth indexing too so
    // "github.com" is findable even when the card has no title.
    let mut titles: Vec<String> = Vec::new();
    if let Some(card) = &tweet.card {
        if let Some(t) = &card.title {
            titles.push(t.clone());
        }
        if let Some(d) = &card.description {
            titles.push(d.clone());
        }
    }
    titles.extend(tweet.entities.urls.iter().map(|u| u.display.clone()));
    let link_titles = titles.join(" ");

    conn.execute(
        "INSERT INTO search_docs (post_id, text, note_text, author_handle, author_name, alt_text, link_titles)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(post_id) DO UPDATE SET
            text          = excluded.text,
            note_text     = excluded.note_text,
            author_handle = excluded.author_handle,
            author_name   = excluded.author_name,
            alt_text      = excluded.alt_text,
            link_titles   = excluded.link_titles",
        params![
            tweet.id,
            short_text,
            note_text,
            tweet.author.handle,
            tweet.author.name,
            alt_text,
            link_titles,
        ],
    )?;

    Ok(())
}

/// Record that a post is (still) in the user's bookmarks.
///
/// `sort_index` is only written when we actually have one, and `bookmarked_at`
/// keeps the earliest observation: seeing the same bookmark twice must not
/// keep pushing its save time forward.
pub fn mark_bookmarked(
    conn: &Connection,
    post_id: &str,
    sort_index: Option<&str>,
    bookmarked_at: Option<i64>,
    observed_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO bookmarks (post_id, bookmarked_at, sort_index, observed_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(post_id) DO UPDATE SET
            observed_at   = excluded.observed_at,
            sort_index    = COALESCE(excluded.sort_index, bookmarks.sort_index),
            -- Only continuous capture supplies a real save time, and only the
            -- first one is the truth. COALESCE keeps it (PRD §6.3).
            bookmarked_at = COALESCE(bookmarks.bookmarked_at, excluded.bookmarked_at)",
        params![post_id, bookmarked_at, sort_index, observed_at],
    )?;
    Ok(())
}

/// Flag a post as no longer present in a later capture.
///
/// Deliberately never a `DELETE`: the archive exists to outlive X. And
/// deliberately worded as "no longer in your bookmarks" in the UI, because
/// absence cannot distinguish a deletion from an unbookmark, from filtering,
/// or from a partial capture (PRD §7.6).
pub fn flag_removed(conn: &Connection, post_id: &str, when: i64) -> Result<()> {
    conn.execute(
        "UPDATE posts SET removed_at = ?2 WHERE id = ?1 AND removed_at IS NULL",
        params![post_id, when],
    )?;
    Ok(())
}

/// Start a capture run and return its id.
pub fn begin_run(conn: &Connection, kind: &str, started_at: i64) -> Result<i64> {
    conn.execute(
        "INSERT INTO capture_runs (started_at, kind) VALUES (?1, ?2)",
        params![started_at, kind],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Finish a capture run.
pub fn finish_run(
    conn: &Connection,
    run_id: i64,
    finished_at: i64,
    stats: IngestStats,
    last_cursor: Option<&str>,
    stopped_reason: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE capture_runs
            SET finished_at = ?2, records_seen = ?3, records_new = ?4,
                last_cursor = COALESCE(?5, last_cursor), stopped_reason = ?6
          WHERE id = ?1",
        params![
            run_id,
            finished_at,
            stats.seen as i64,
            stats.new as i64,
            last_cursor,
            stopped_reason
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> crate::db::Library {
        crate::db::Library::open_in_memory().unwrap()
    }

    fn payload(text: &str) -> String {
        format!(
            r#"{{"data":{{"tweetResult":{{"result":{{
              "__typename":"Tweet","rest_id":"123",
              "core":{{"user_results":{{"result":{{
                "rest_id":"42",
                "core":{{"name":"Ada","screen_name":"ada"}},
                "avatar":{{"image_url":"https://pbs.twimg.com/a.jpg"}}
              }}}}}},
              "legacy":{{"id_str":"123","full_text":"{text}",
                "created_at":"Wed Oct 10 20:19:24 +0000 2018",
                "entities":{{"urls":[]}}}}
            }}}}}}}}"#
        )
    }

    #[test]
    fn stores_a_single_tweet() {
        let l = lib();
        let stats = store_payload(l.conn(), &payload("hello"), Some("TweetDetail"), SOURCE_IMPORT, 1000).unwrap();
        assert_eq!(stats.seen, 1);
        assert_eq!(stats.new, 1);
        assert_eq!(stats.payloads, 1);

        let text: String = l
            .conn()
            .query_row("SELECT text FROM posts WHERE id='123'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn re_importing_the_same_payload_changes_nothing() {
        let mut l = lib();
        let body = payload("hello");

        let tx = l.conn_mut().transaction().unwrap();
        let a = store_payload(&tx, &body, Some("Bookmarks"), SOURCE_IMPORT, 1000).unwrap();
        tx.commit().unwrap();

        let tx = l.conn_mut().transaction().unwrap();
        let b = store_payload(&tx, &body, Some("Bookmarks"), SOURCE_IMPORT, 2000).unwrap();
        tx.commit().unwrap();

        assert_eq!(a.new, 1);
        assert_eq!(b.new, 0, "second import created a duplicate post");
        assert_eq!(b.updated, 1);
        assert_eq!(b.payloads, 0, "envelope was stored twice");

        let posts: i64 = l.conn().query_row("SELECT count(*) FROM posts", [], |r| r.get(0)).unwrap();
        let media: i64 = l.conn().query_row("SELECT count(*) FROM media", [], |r| r.get(0)).unwrap();
        let docs: i64 = l.conn().query_row("SELECT count(*) FROM search_docs", [], |r| r.get(0)).unwrap();
        assert_eq!((posts, media, docs), (1, 0, 1));
    }

    #[test]
    fn a_truncated_capture_never_overwrites_a_long_form_body() {
        // The failure this prevents: scroll past a long post (truncated
        // full_text), then open it (full note_tweet), then scroll past again —
        // and silently lose the end of the post.
        let l = lib();
        let short = payload("the beginning\u{2026}");
        let long = r#"{"data":{"tweetResult":{"result":{
          "__typename":"Tweet","rest_id":"123",
          "core":{"user_results":{"result":{"rest_id":"42",
            "core":{"name":"Ada","screen_name":"ada"}}}},
          "legacy":{"id_str":"123","full_text":"the beginning\u2026",
            "entities":{"urls":[]}},
          "note_tweet":{"note_tweet_results":{"result":{
            "text":"the beginning and the whole long ending",
            "entity_set":{"urls":[],"user_mentions":[],"hashtags":[]}}}}
        }}}}"#;

        store_payload(l.conn(), &long, Some("TweetDetail"), SOURCE_IMPORT, 1000).unwrap();
        store_payload(l.conn(), &short, Some("Bookmarks"), SOURCE_IMPORT, 2000).unwrap();

        let (text, is_long): (String, i64) = l
            .conn()
            .query_row("SELECT text, is_long_form FROM posts WHERE id='123'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(text, "the beginning and the whole long ending");
        assert_eq!(is_long, 1);
    }

    #[test]
    fn only_bookmark_operations_create_bookmarks() {
        let l = lib();
        store_payload(l.conn(), &payload("from a timeline"), Some("HomeTimeline"), SOURCE_CAPTURE, 1000).unwrap();
        assert_eq!(l.count_bookmarks().unwrap(), 0, "a home timeline post became a bookmark");

        store_payload(l.conn(), &payload("from bookmarks"), Some("Bookmarks"), SOURCE_CAPTURE, 1000).unwrap();
        assert_eq!(l.count_bookmarks().unwrap(), 1);
    }

    #[test]
    fn an_empty_payload_stores_nothing_not_even_an_envelope() {
        let l = lib();
        let stats = store_payload(l.conn(), r#"{"data":{}}"#, Some("Bookmarks"), SOURCE_IMPORT, 1000).unwrap();
        assert_eq!(stats.seen, 0);
        assert_eq!(stats.payloads, 0);
        let payloads: i64 = l
            .conn()
            .query_row("SELECT count(*) FROM capture_payloads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(payloads, 0);
    }

    #[test]
    fn search_index_follows_an_enrichment_update() {
        // Guards the external-content choice: contentless FTS5 would reject
        // this UPDATE outright.
        let l = lib();
        let short = payload("stub text");
        let long = r#"{"data":{"tweetResult":{"result":{
          "__typename":"Tweet","rest_id":"123",
          "core":{"user_results":{"result":{"rest_id":"42",
            "core":{"name":"Ada","screen_name":"ada"}}}},
          "legacy":{"id_str":"123","full_text":"stub text","entities":{"urls":[]}},
          "note_tweet":{"note_tweet_results":{"result":{
            "text":"stub text with a searchable needle inside",
            "entity_set":{"urls":[],"user_mentions":[],"hashtags":[]}}}}
        }}}}"#;

        store_payload(l.conn(), &short, Some("CreateBookmark"), SOURCE_CAPTURE, 1000).unwrap();
        let before: i64 = l
            .conn()
            .query_row(
                "SELECT count(*) FROM posts_fts_uni WHERE posts_fts_uni MATCH 'needle'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, 0);

        store_payload(l.conn(), &long, Some("TweetDetail"), SOURCE_CAPTURE, 2000).unwrap();
        let after: i64 = l
            .conn()
            .query_row(
                "SELECT count(*) FROM posts_fts_uni WHERE posts_fts_uni MATCH 'needle'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, 1, "the FTS index did not follow the enrichment");
    }

    #[test]
    fn payload_id_is_stable_and_content_addressed() {
        assert_eq!(payload_id("abc"), payload_id("abc"));
        assert_ne!(payload_id("abc"), payload_id("abd"));
        assert!(payload_id("abc").starts_with("sha256:"));
    }

    #[test]
    fn removal_is_flagged_never_deleted_and_clears_on_reappearance() {
        let l = lib();
        store_payload(l.conn(), &payload("x"), Some("Bookmarks"), SOURCE_CAPTURE, 1000).unwrap();

        flag_removed(l.conn(), "123", 2000).unwrap();
        let (n, removed): (i64, Option<i64>) = l
            .conn()
            .query_row("SELECT count(*), max(removed_at) FROM posts", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(n, 1, "the post was deleted instead of flagged");
        assert_eq!(removed, Some(2000));

        // Seeing it again means it is back.
        store_payload(l.conn(), &payload("x"), Some("Bookmarks"), SOURCE_CAPTURE, 3000).unwrap();
        let removed: Option<i64> = l
            .conn()
            .query_row("SELECT removed_at FROM posts WHERE id='123'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(removed, None);
    }

    #[test]
    fn bookmarked_at_keeps_the_first_observation() {
        // Continuous capture can observe the same save more than once (the
        // page re-renders, the buffer retries). The save time must stay the
        // first one, or every bookmark slowly claims it was saved "just now".
        let l = lib();
        store_payload(l.conn(), &payload("x"), Some("CreateBookmark"), SOURCE_CAPTURE, 1000).unwrap();

        mark_bookmarked(l.conn(), "123", Some("900"), Some(5000), 5000).unwrap();
        mark_bookmarked(l.conn(), "123", Some("950"), Some(9999), 9999).unwrap();

        let (bookmarked_at, sort_index, observed_at): (Option<i64>, Option<String>, i64) = l
            .conn()
            .query_row(
                "SELECT bookmarked_at, sort_index, observed_at FROM bookmarks WHERE post_id='123'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(bookmarked_at, Some(5000), "save time was overwritten");
        assert_eq!(sort_index.as_deref(), Some("950"), "ordering should refresh");
        assert_eq!(observed_at, 9999, "last-seen should refresh");
    }

    #[test]
    fn a_backfill_capture_leaves_bookmarked_at_null() {
        // X exposes no bookmark timestamp, so a bulk scroll can never supply
        // one. The column must stay NULL rather than being filled with the
        // capture time, which would be a plausible-looking lie.
        let l = lib();
        store_payload(l.conn(), &payload("x"), Some("Bookmarks"), SOURCE_CAPTURE, 1000).unwrap();

        let bookmarked_at: Option<i64> = l
            .conn()
            .query_row("SELECT bookmarked_at FROM bookmarks WHERE post_id='123'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(bookmarked_at, None);
    }
}
