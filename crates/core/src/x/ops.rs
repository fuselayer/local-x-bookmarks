//! GraphQL operation-name matching.
//!
//! This module is the single source of truth for "is this response one we
//! care about", and it is mirrored verbatim in the userscript. That
//! duplication is deliberate and load-bearing — see the note at the bottom.
//!
//! ## Why operation names and not `doc_id`
//!
//! X rotates GraphQL `doc_id` hashes on a 2–4 week cadence. A matcher built
//! on the hash breaks every time. The operation *name* in the URL path has
//! been stable for years:
//!
//! ```text
//! /i/api/graphql/<rotating-hash>/Bookmarks
//!                                  ^^^^^^^^^ this part
//! ```
//!
//! We never construct requests, so rotation cannot break us — we only have to
//! *recognise* what the page already fetched.

/// Operations whose responses contain posts we want, ordered roughly by how
/// valuable they are.
pub const CAPTURE_OPS: &[&str] = &[
    "Bookmarks",
    "BookmarkSearch",
    "BookmarkFoldersSlice",
    "BookmarkFolderTimeline",
    "TweetDetail",
    "TweetResultByRestId",
    "HomeTimeline",
    "HomeLatestTimeline",
    "UserTweets",
    "ListLatestTweetsTimeline",
    "SearchTimeline",
];

/// Operations that are structurally interesting but do not contain a post
/// body. `CreateBookmark` is here because its *request variables* carry the
/// tweet ID — the response is only a status stub (see PRD §6.5).
pub const SIGNAL_OPS: &[&str] = &["CreateBookmark", "DeleteBookmark", "BookmarkFolderCreate"];

/// True if the URL is a GraphQL call at all.
///
/// Kept separate from the name check so callers can count "GraphQL traffic we
/// saw" versus "GraphQL traffic we understood" — the ratio is the honest
/// health signal for the parser.
pub fn is_graphql_url(url: &str) -> bool {
    url.contains("/i/api/graphql/")
}

/// Extract the operation name from a GraphQL URL.
///
/// Returns the last path segment, which is where X puts it, ignoring any
/// query string. `None` for a URL that is not a GraphQL call.
pub fn operation_name(url: &str) -> Option<&str> {
    if !is_graphql_url(url) {
        return None;
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next()?;
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// True if this URL's response should be parsed for posts.
pub fn is_capture_operation(url: &str) -> bool {
    match operation_name(url) {
        Some(name) => CAPTURE_OPS
            .iter()
            .any(|op| name.eq_ignore_ascii_case(op) || name.starts_with(op)),
        None => false,
    }
}

/// True if this URL is one whose *request* we want to inspect.
pub fn is_signal_operation(url: &str) -> bool {
    match operation_name(url) {
        Some(name) => SIGNAL_OPS.iter().any(|op| name.eq_ignore_ascii_case(op)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real-shaped URLs. The hash is fake but the path layout is what matters.
    const BOOKMARKS: &str =
        "https://x.com/i/api/graphql/abc123DEF456/Bookmarks?variables=%7B%7D&features=%7B%7D";
    const TWEET_DETAIL: &str =
        "https://x.com/i/api/graphql/xyz789/TweetDetail?variables=%7B%7D";
    const CREATE_BOOKMARK: &str =
        "https://x.com/i/api/graphql/qrs456/CreateBookmark";

    #[test]
    fn extracts_operation_name_ignoring_query_string() {
        assert_eq!(operation_name(BOOKMARKS), Some("Bookmarks"));
        assert_eq!(operation_name(TWEET_DETAIL), Some("TweetDetail"));
    }

    #[test]
    fn matches_capture_operations() {
        assert!(is_capture_operation(BOOKMARKS));
        assert!(is_capture_operation(TWEET_DETAIL));
    }

    #[test]
    fn create_bookmark_is_a_signal_not_a_capture() {
        // The distinction that PRD §6.5 is built on: we read the *request*,
        // because the response has no post in it.
        assert!(is_signal_operation(CREATE_BOOKMARK));
        assert!(!is_capture_operation(CREATE_BOOKMARK));
    }

    #[test]
    fn ignores_non_graphql_traffic() {
        assert!(!is_capture_operation("https://x.com/i/api/1.1/dm/inbox.json"));
        assert!(!is_capture_operation("https://pbs.twimg.com/media/abc.jpg"));
        assert_eq!(operation_name("https://x.com/home"), None);
        assert!(!is_graphql_url("https://x.com/home"));
    }

    #[test]
    fn survives_doc_id_rotation() {
        // Same operation, completely different hash. Both must match — this
        // is the entire point of matching on the name.
        let a = "https://x.com/i/api/graphql/AAA/Bookmarks";
        let b = "https://x.com/i/api/graphql/ZZZ999/Bookmarks";
        assert!(is_capture_operation(a) && is_capture_operation(b));
    }
}
