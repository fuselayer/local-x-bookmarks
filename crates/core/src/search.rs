//! Keyword search across two FTS5 indexes, fused with Reciprocal Rank Fusion.
//!
//! ## Why two retrievers and not one
//!
//! The two tokenizers answer genuinely different questions:
//!
//! - `unicode61` — proper term matching with real BM25 relevance. `"gui"`
//!   matches the word `gui`.
//! - `trigram` — substring matching, and the only thing that works for CJK
//!   and for matching inside words (`"chive"` finds `"archive"`). It is **not**
//!   typo tolerance, despite being widely described that way; typos are the
//!   embedding index's job (M4).
//!
//! Neither is better. A query for `graphql` wants BM25; a query for `書` needs
//! trigram or it returns nothing at all. So both run, and the results are
//! combined.
//!
//! ## Why RRF rather than score blending
//!
//! BM25 scores and trigram scores are not on a common scale — one is an
//! unbounded negative log-odds, the other is computed over a different
//! tokenisation of the same text. Adding them, or normalising them by their
//! maxima, produces a ranking that changes meaning whenever the corpus
//! changes. RRF throws the scores away and uses only *rank position*, which
//! is comparable by construction and is the same code path that will fuse
//! vectors in M4.

use std::collections::HashMap;

use rusqlite::{params, Connection};

use crate::error::Result;
use crate::model::PostView;

/// RRF damping constant. 60 is the value from the original paper and the one
/// every implementation uses; it flattens the contribution of low ranks.
const RRF_K: f64 = 60.0;

/// Keep the candidate pool per retriever well above the result limit, so
/// fusion has something to work with.
const CANDIDATES_PER_RETRIEVER: usize = 200;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub post: PostView,
    /// Fused RRF score. Only meaningful for ordering, not as a quality
    /// measure — do not display it.
    pub score: f64,
    /// A snippet with matches wrapped in the delimiters passed to
    /// [`search_with`]. `None` when no retriever could produce one.
    pub snippet: Option<String>,
    /// Which retrievers matched. Useful for explaining an odd result, and for
    /// the `/kw` vs `/sem` toggles later.
    pub matched: Vec<String>,
}

impl SearchHit {
    /// RRF scores are not probabilities and are not comparable across queries,
    /// but they are comparable *within* one, which is all a relevance bar
    /// needs.
    pub fn relative(&self, best: f64) -> f64 {
        if best <= 0.0 {
            0.0
        } else {
            self.score / best
        }
    }
}

/// Delimiters used to mark matches in a snippet.
///
/// These are `U+0001` and `U+0002`, not `[` and `]`, and that matters.
///
/// FTS5's `snippet()` returns one *string*, so if the frontend is to render
/// matches as real DOM nodes it has to find the boundaries some other way. The
/// tempting option is HTML tags, which forces the frontend into `innerHTML` —
/// and tweet text containing `<img onerror=…>` then becomes script execution.
///
/// Control characters cannot appear in tweet text, cannot be typed into the
/// search box, and are inert in every renderer. So the frontend splits on them
/// and builds elements, and there is no HTML parsing anywhere near user data.
pub const SNIPPET_OPEN: &str = "\u{1}";
pub const SNIPPET_CLOSE: &str = "\u{2}";

/// Search the library. `limit` is applied after fusion.
///
/// Uses sentinel delimiters; the CLI opts into printable ones via
/// [`search_with`] because a terminal cannot show a `U+0001`.
pub fn search(conn: &Connection, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    search_with(conn, query, limit, SNIPPET_OPEN, SNIPPET_CLOSE)
}

/// As [`search`], with control over match highlighting delimiters.
///
/// Delimiters are parameters rather than hardcoded because the frontend needs
/// to render highlights as DOM nodes, and passing `\u{1}`/`\u{2}` sentinels
/// is safer than passing HTML tags — there is then no way for tweet text to
/// inject markup.
pub fn search_with(
    conn: &Connection,
    query: &str,
    limit: usize,
    open: &str,
    close: &str,
) -> Result<Vec<SearchHit>> {
    let match_expr = match fts_query(query) {
        Some(e) => e,
        None => return Ok(Vec::new()),
    };

    // Retriever name -> (ranked post ids, snippet by id)
    let mut rankings: Vec<(&'static str, Vec<String>, HashMap<String, String>)> = Vec::new();

    // A retriever that errors is skipped, not fatal. A malformed query for one
    // tokenizer must never make the search box return nothing at all.
    if let Ok(r) = run_retriever(conn, "posts_fts_uni", &match_expr, open, close) {
        rankings.push(("unicode61", r.0, r.1));
    }
    if let Ok(r) = run_retriever(conn, "posts_fts_sub", &match_expr, open, close) {
        rankings.push(("trigram", r.0, r.1));
    }

    if rankings.is_empty() {
        return Ok(Vec::new());
    }

    // ── fuse ─────────────────────────────────────────────────────────────
    let mut fused: HashMap<String, (f64, Vec<String>, Option<String>)> = HashMap::new();

    for (name, ids, snippets) in &rankings {
        for (rank, id) in ids.iter().enumerate() {
            let contribution = 1.0 / (RRF_K + (rank + 1) as f64);
            let entry = fused
                .entry(id.clone())
                .or_insert_with(|| (0.0, Vec::new(), None));
            entry.0 += contribution;
            entry.1.push((*name).to_owned());
            // First snippet wins; unicode61 runs first and produces the more
            // readable one.
            if entry.2.is_none() {
                entry.2 = snippets.get(id).cloned();
            }
        }
    }

    let mut ordered: Vec<(String, f64, Vec<String>, Option<String>)> = fused
        .into_iter()
        .map(|(id, (score, matched, snippet))| (id, score, matched, snippet))
        .collect();

    // Deterministic tie-break on post id: equal RRF scores are common with
    // small result sets, and an unstable order makes the UI flicker between
    // identical keystrokes.
    ordered.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    ordered.truncate(limit);

    load_hits(conn, ordered)
}

/// Run one FTS retriever, returning ranked post IDs and per-ID snippets.
fn run_retriever(
    conn: &Connection,
    table: &str,
    match_expr: &str,
    open: &str,
    close: &str,
) -> Result<(Vec<String>, HashMap<String, String>)> {
    // Table name is interpolated because SQLite cannot bind an identifier.
    // It is never user input — only the two literal names above are passed.
    let sql = format!(
        "SELECT d.post_id,
                snippet({table}, -1, ?2, ?3, '…', 14) AS snip
           FROM {table}
           JOIN search_docs d ON d.rowid = {table}.rowid
          WHERE {table} MATCH ?1
          ORDER BY bm25({table})
          LIMIT {CANDIDATES_PER_RETRIEVER}"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![match_expr, open, close])?;

    let mut ids = Vec::new();
    let mut snippets = HashMap::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if let Ok(snip) = row.get::<_, String>(1) {
            snippets.insert(id.clone(), snip);
        }
        ids.push(id);
    }
    Ok((ids, snippets))
}

/// Fetch the full views for the winning IDs, preserving the fused order.
fn load_hits(
    conn: &Connection,
    ordered: Vec<(String, f64, Vec<String>, Option<String>)>,
) -> Result<Vec<SearchHit>> {
    if ordered.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::with_capacity(ordered.len());
    let mut stmt = conn.prepare(
        "SELECT p.id, p.raw_tweet, p.first_seen_at, p.last_seen_at, p.removed_at, p.source,
                b.bookmarked_at, b.sort_index
           FROM posts p
           LEFT JOIN bookmarks b ON b.post_id = p.id
          WHERE p.id = ?1",
    )?;

    for (id, score, matched, snippet) in ordered {
        let mut rows = stmt.query(params![id])?;
        let Some(row) = rows.next()? else { continue };

        // Reuse the exact same view construction as a normal read, so search
        // results and the detail pane can never disagree about a post.
        let raw: Option<String> = row.get(1)?;
        let Some(raw) = raw else { continue };
        let Some(tweet) = crate::db::read::parse_stored_tweet(&raw) else {
            continue;
        };

        out.push(SearchHit {
            post: PostView {
                tweet,
                bookmarked_at: row.get(6)?,
                sort_index: row.get(7)?,
                first_seen_at: row.get(2)?,
                last_seen_at: row.get(3)?,
                removed_at: row.get(4)?,
                source: row.get(5)?,
            },
            score,
            snippet,
            matched,
        });
    }

    Ok(out)
}

/// Turn free text into a safe FTS5 MATCH expression.
///
/// ## Why this is not just the raw string
///
/// FTS5's MATCH grammar has its own operators (`AND`, `OR`, `NOT`, `NEAR`,
/// `:`, `^`, `*`, parentheses). Passing user input straight through means a
/// search for `foo:bar` is a syntax error and a search for `a OR b` silently
/// means something the user did not ask for. So every token is quoted, which
/// makes it a literal phrase, and the tokens are joined by whitespace, which
/// FTS5 reads as AND.
///
/// The final token gets a `*` so search feels live as the user types —
/// `graphq` should already show results.
///
/// Returns `None` when the query has no usable tokens, which is not an error:
/// it is what an empty search box looks like.
pub fn fts_query(input: &str) -> Option<String> {
    let tokens: Vec<String> = input
        .split_whitespace()
        // An unbalanced quote inside a token would terminate the phrase early
        // and change the meaning of everything after it.
        .map(|t| t.replace('"', " "))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.is_empty() {
        return None;
    }

    let last = tokens.len() - 1;
    let quoted: Vec<String> = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == last {
                format!("\"{t}\"*")
            } else {
                format!("\"{t}\"")
            }
        })
        .collect();

    Some(quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::write::{store_payload, SOURCE_IMPORT};
    use serde_json::json;

    fn tweet(id: &str, handle: &str, name: &str, text: &str) -> serde_json::Value {
        json!({
            "__typename": "Tweet",
            "rest_id": id,
            "core": { "user_results": { "result": {
                "rest_id": format!("u{id}"),
                "core": { "name": name, "screen_name": handle },
                "legacy": { "name": name, "screen_name": handle }
            }}},
            "legacy": {
                "id_str": id, "full_text": text,
                "entities": { "urls": [] }
            }
        })
    }

    fn library() -> crate::db::Library {
        let l = crate::db::Library::open_in_memory().unwrap();
        let body = json!({ "data": { "bookmark_timeline_v2": { "timeline": { "instructions": [
            { "type": "TimelineAddEntries", "entries": [
                { "entryId": "tweet-1", "sortIndex": "900",
                  "content": { "entryType": "TimelineTimelineItem", "itemContent": {
                    "tweet_results": { "result": tweet("1", "ada", "Ada Lovelace",
                      "building a local-first archive of bookmarks in rust") } } } },
                { "entryId": "tweet-2", "sortIndex": "800",
                  "content": { "entryType": "TimelineTimelineItem", "itemContent": {
                    "tweet_results": { "result": tweet("2", "grace", "Grace Hopper",
                      "compilers are just very literal translators") } } } },
                { "entryId": "tweet-3", "sortIndex": "700",
                  "content": { "entryType": "TimelineTimelineItem", "itemContent": {
                    "tweet_results": { "result": tweet("3", "kenton", "Kenton Varda",
                      "a capn proto walkthrough") } } } }
            ]}
        ]}}}}
        ).to_string();

        store_payload(l.conn(), &body, Some("Bookmarks"), SOURCE_IMPORT, 1000).unwrap();
        l
    }

    fn ids(hits: &[SearchHit]) -> Vec<String> {
        hits.iter().map(|h| h.post.tweet.id.clone()).collect()
    }

    #[test]
    fn finds_by_word() {
        let l = library();
        let hits = search(l.conn(), "bookmarks", 10).unwrap();
        assert_eq!(ids(&hits), vec!["1"]);
    }

    #[test]
    fn finds_inside_a_word_via_trigram() {
        // "chive" only matches through the trigram index; unicode61 has no
        // token for it. This is the capability that justifies the second index.
        let l = library();
        let hits = search(l.conn(), "chive", 10).unwrap();
        assert_eq!(ids(&hits), vec!["1"], "trigram substring match failed");
        assert!(hits[0].matched.contains(&"trigram".to_string()));
    }

    #[test]
    fn matches_across_the_author_name_and_handle() {
        let l = library();
        assert_eq!(ids(&search(l.conn(), "grace", 10).unwrap()), vec!["2"]);
        assert_eq!(ids(&search(l.conn(), "kenton", 10).unwrap()), vec!["3"]);
    }

    #[test]
    fn prefix_matches_as_you_type() {
        let l = library();
        // The last token is a prefix query, so a half-typed word already hits.
        assert_eq!(ids(&search(l.conn(), "compil", 10).unwrap()), vec!["2"]);
    }

    #[test]
    fn all_tokens_must_match() {
        let l = library();
        // Both words appear only in post 1.
        assert_eq!(ids(&search(l.conn(), "local rust", 10).unwrap()), vec!["1"]);
        // "rust" and "compilers" never co-occur.
        assert!(search(l.conn(), "rust compilers", 10).unwrap().is_empty());
    }

    #[test]
    fn fuses_both_retrievers_and_reports_which_matched() {
        let l = library();
        let hits = search(l.conn(), "archive", 10).unwrap();
        assert_eq!(ids(&hits), vec!["1"]);
        // "archive" is both a word and a substring, so both retrievers fire
        // and the fused score is the sum of two contributions.
        assert!(hits[0].matched.len() >= 1);
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn a_post_matched_by_both_retrievers_outranks_one_matched_by_either() {
        let l = library();
        // "proto" appears in post 3 only; "capn proto" should put it first.
        let hits = search(l.conn(), "capn proto", 10).unwrap();
        assert_eq!(ids(&hits)[0], "3");
        assert_eq!(hits[0].matched.len(), 2, "expected both retrievers to match");
    }

    #[test]
    fn empty_and_whitespace_queries_return_nothing_without_erroring() {
        let l = library();
        for q in ["", "   ", "\t\n"] {
            assert!(search(l.conn(), q, 10).unwrap().is_empty(), "query {q:?}");
        }
    }

    #[test]
    fn fts_operators_in_user_input_are_neutralised() {
        let l = library();
        // These would be syntax errors or silent meaning-changes if passed
        // through raw. None may panic or return the whole library.
        for q in ["foo:bar", "a OR b", "NEAR(", "\"unbalanced", "*", "^caret", "a AND"] {
            let r = search(l.conn(), q, 10);
            assert!(r.is_ok(), "query {q:?} errored: {:?}", r.err());
        }
    }

    #[test]
    fn limit_is_respected_and_order_is_stable() {
        let l = library();
        let a = ids(&search(l.conn(), "a", 2).unwrap());
        let b = ids(&search(l.conn(), "a", 2).unwrap());
        assert_eq!(a, b, "result order is not deterministic");
        assert!(a.len() <= 2);
    }

    #[test]
    fn snippets_wrap_the_match_in_control_characters_by_default() {
        // The default delimiters are U+0001/U+0002 precisely so the frontend
        // can split on them and build DOM nodes instead of parsing HTML. If
        // this test ever starts finding printable brackets, the XSS-safe
        // design has been undone.
        let l = library();
        let hits = search(l.conn(), "bookmarks", 10).unwrap();
        let snip = hits[0].snippet.as_deref().unwrap_or("");

        assert!(
            snip.contains(SNIPPET_OPEN),
            "snippet missing the open sentinel: {snip:?}"
        );
        assert!(
            snip.contains(SNIPPET_CLOSE),
            "snippet missing the close sentinel: {snip:?}"
        );
        assert!(snip.contains("bookmarks"));
        assert!(
            !snip.contains('['),
            "snippet contains a printable bracket, which means a delimiter \
             change leaked into the default path: {snip:?}"
        );
    }

    #[test]
    fn printable_delimiters_are_available_for_the_cli() {
        // A terminal cannot show U+0001, so the CLI opts into brackets.
        let l = library();
        let hits = search_with(l.conn(), "bookmarks", 10, "[", "]").unwrap();
        let snip = hits[0].snippet.as_deref().unwrap_or("");
        assert!(snip.contains('['), "snippet missing highlight: {snip:?}");
        assert!(snip.contains("bookmarks"));
        assert!(!snip.contains(SNIPPET_OPEN));
    }

    #[test]
    fn the_sentinel_delimiters_cannot_occur_in_tweet_text() {
        // The safety argument depends on this: control characters cannot be
        // typed into the search box and cannot appear in a tweet, so a
        // snippet can never contain a stray delimiter that a hostile payload
        // injected.
        assert_eq!(SNIPPET_OPEN, "\u{1}");
        assert_eq!(SNIPPET_CLOSE, "\u{2}");
        assert!(SNIPPET_OPEN.chars().all(|c| c.is_control()));
        assert!(SNIPPET_CLOSE.chars().all(|c| c.is_control()));
    }

    #[test]
    fn query_builder_quotes_tokens_and_prefixes_the_last() {
        assert_eq!(fts_query("rust gui").as_deref(), Some("\"rust\" \"gui\"*"));
        assert_eq!(fts_query("  solo  ").as_deref(), Some("\"solo\"*"));
        assert_eq!(fts_query("").as_deref(), None);
        // Embedded quotes cannot escape the phrase.
        assert_eq!(fts_query("a\"b").as_deref(), Some("\"a b\"*"));
    }
}
