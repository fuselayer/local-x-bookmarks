//! Finding tweets inside any payload X hands us.
//!
//! ## Why this is a walker and not a fixed path
//!
//! The same tweet arrives in structurally different envelopes depending on
//! where the user was:
//!
//! | Context | Path |
//! |---|---|
//! | Bookmarks page | `data.bookmark_timeline_v2.timeline.instructions[]` |
//! | A folder | `data.bookmark_collection_timeline...` |
//! | Tweet detail / permalink | `data.tweetResult.result` |
//! | Some timelines | `data.home.home_timeline_urt.instructions[]` |
//! | A file someone exported | any of the above, or a bare `tweet_results` |
//!
//! Hardcoding one path means the importer works for exactly one of these. So
//! we do two passes: a *structured* pass that understands timeline entries
//! (and therefore recovers `sortIndex` and cursors, which only exist at entry
//! level), then a *loose* pass that recursively finds anything tweet-shaped
//! the structured pass missed.
//!
//! The loose pass is what makes "import a single tweet" work without a
//! special code path. A single tweet is just an envelope with one tweet in it.

use std::collections::HashMap;

use serde_json::Value;

use crate::model::Tweet;
use crate::x::tweet::parse_tweet_result;

/// Everything we managed to pull out of one payload.
#[derive(Debug, Default, Clone)]
pub struct Extracted {
    pub tweets: Vec<Tweet>,
    /// Tweet ID -> `entries[].sortIndex`.
    ///
    /// Only populated from top-level timeline entries. See the note on
    /// modules in `walk_instructions` for why modules are excluded.
    pub sort_indices: HashMap<String, String>,
    /// Tweet ID -> the original `tweet_results.result` subtree, verbatim.
    ///
    /// Stored so a future parser fix can re-derive every field from the bytes
    /// X actually sent, rather than from our interpretation of them (PRD
    /// §9.2). Without this, "we can re-parse retroactively" would only be
    /// true for posts whose page envelope we still hold.
    pub raw_by_id: HashMap<String, Value>,
    /// The pagination handle. This is the honest progress signal.
    pub bottom_cursor: Option<String>,
    pub top_cursor: Option<String>,
}

impl Extracted {
    pub fn is_empty(&self) -> bool {
        self.tweets.is_empty()
    }

    pub fn sort_index_of(&self, tweet_id: &str) -> Option<&str> {
        self.sort_indices.get(tweet_id).map(String::as_str)
    }

    pub fn raw_of(&self, tweet_id: &str) -> Option<&Value> {
        self.raw_by_id.get(tweet_id)
    }
}

/// Extract every tweet from a parsed JSON payload.
pub fn extract(root: &Value) -> Extracted {
    let mut out = Extracted::default();

    // Pass 1 — structured. Walks timeline instructions, which is the only
    // place sortIndex and cursors exist.
    walk_instructions(root, &mut out);

    // Pass 2 — loose. Catches `data.tweetResult.result` and bare payloads,
    // plus anything inside a shape we do not know about.
    let mut loose: Vec<(Tweet, Value)> = Vec::new();
    collect_tweets(root, &mut loose, 0);

    // Merge, deduping by ID. Structured results win because they carry
    // sort_index and were reached first; loose results only add posts we had
    // not seen.
    let mut seen: HashMap<String, ()> = out.tweets.iter().map(|t| (t.id.clone(), ())).collect();
    for (t, raw) in loose {
        if seen.insert(t.id.clone(), ()).is_none() {
            out.raw_by_id.entry(t.id.clone()).or_insert(raw);
            out.tweets.push(t);
        }
    }

    out
}

// ── Pass 1: timeline instructions ────────────────────────────────────────────

/// Find every `instructions` array anywhere in the tree and walk it.
///
/// Recursive rather than path-based so `bookmark_timeline_v2`,
/// `bookmark_collection_timeline` and `home_timeline_urt` all work without
/// naming any of them.
fn walk_instructions(node: &Value, out: &mut Extracted) {
    match node {
        Value::Object(map) => {
            if let Some(Value::Array(instructions)) = map.get("instructions") {
                for instruction in instructions {
                    walk_instruction(instruction, out);
                }
            }
            for v in map.values() {
                walk_instructions(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                walk_instructions(v, out);
            }
        }
        _ => {}
    }
}

fn walk_instruction(instruction: &Value, out: &mut Extracted) {
    let Some(entries) = instruction.get("entries").and_then(Value::as_array) else {
        // `TimelineAddToModule` carries its tweets under `moduleItems`.
        if let Some(items) = instruction.get("moduleItems").and_then(Value::as_array) {
            for item in items {
                add_loose_tweet(item.pointer("/item/itemContent/tweet_results/result"), out, None);
            }
        }
        return;
    };

    for entry in entries {
        // sortIndex is a SIBLING of content, not a field under legacy. Reading
        // `legacy.sort_index` yields None on every record, silently and
        // forever — see PRD §6.3.
        let sort_index = entry.get("sortIndex").and_then(Value::as_str);

        let content = entry.get("content").unwrap_or(&Value::Null);
        match content.get("entryType").and_then(Value::as_str) {
            Some("TimelineTimelineItem") => {
                add_loose_tweet(
                    content.pointer("/itemContent/tweet_results/result"),
                    out,
                    sort_index,
                );
            }
            Some("TimelineTimelineCursor") => {
                let cursor_type = content.get("cursorType").and_then(Value::as_str);
                let value = content.get("value").and_then(Value::as_str);
                match (cursor_type, value) {
                    (Some("Bottom"), Some(v)) => out.bottom_cursor = Some(v.to_owned()),
                    (Some("Top"), Some(v)) => out.top_cursor = Some(v.to_owned()),
                    _ => {}
                }
            }
            // A conversation module — a thread shown inline. The PRD says skip
            // these rather than fail, and we do skip the *sort_index* because
            // a module's ordering is thread order, not bookmark order, and
            // letting it into the bookmark ordering would scramble the
            // library. But the tweets inside are real posts the user can see,
            // so we keep them, just without an ordering claim.
            Some("TimelineTimelineModule") => {
                if let Some(items) = content.get("items").and_then(Value::as_array) {
                    for item in items {
                        add_loose_tweet(
                            item.pointer("/item/itemContent/tweet_results/result"),
                            out,
                            None,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn add_loose_tweet(result: Option<&Value>, out: &mut Extracted, sort_index: Option<&str>) {
    let Some(result) = result else { return };
    let Some(tweet) = parse_tweet_result(result) else { return };

    if let Some(si) = sort_index {
        out.sort_indices.insert(tweet.id.clone(), si.to_owned());
    }
    if !out.tweets.iter().any(|t| t.id == tweet.id) {
        out.raw_by_id.insert(tweet.id.clone(), result.clone());
        out.tweets.push(tweet);
    }
}

// ── Pass 2: loose recursive scan ─────────────────────────────────────────────

/// Depth cap. Payloads nest deeply but not unboundedly, and a malformed file
/// should not be able to blow the stack.
const MAX_DEPTH: usize = 64;

fn collect_tweets(node: &Value, out: &mut Vec<(Tweet, Value)>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match node {
        Value::Object(map) => {
            if looks_like_tweet(node) {
                if let Some(t) = parse_tweet_result(node) {
                    out.push((t, node.clone()));
                }
                // Do not descend into a tweet: `quoted_status_result` is
                // handled by the tweet parser, and recursing here would
                // surface the quoted post as a top-level result, which would
                // then be stored as a bookmark in its own right.
                return;
            }
            for v in map.values() {
                collect_tweets(v, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_tweets(v, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Is this object a tweet?
///
/// Must NOT match a user object — `core.user_results.result` also has
/// `rest_id` and `legacy`, and treating it as a tweet would import every
/// author as a post. The discriminator is that a tweet's `legacy` carries
/// `full_text`, which no user object has.
fn looks_like_tweet(v: &Value) -> bool {
    match v.get("__typename").and_then(Value::as_str) {
        Some("Tweet") | Some("TweetWithVisibilityResults") => true,
        Some("TweetTombstone") | Some("User") | Some("UserUnavailable") => false,
        _ => v.pointer("/legacy/full_text").is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tweet(id: &str, text: &str) -> Value {
        json!({
            "__typename": "Tweet",
            "rest_id": id,
            "core": { "user_results": { "result": {
                "rest_id": "42",
                "core": { "name": "Ada", "screen_name": "ada" },
                "legacy": { "screen_name": "ada", "name": "Ada" }
            } } },
            "legacy": { "id_str": id, "full_text": text, "entities": { "urls": [] } }
        })
    }

    fn bookmarks_page() -> Value {
        json!({
            "data": { "bookmark_timeline_v2": { "timeline": { "instructions": [
                { "type": "TimelineAddEntries", "entries": [
                    { "entryId": "cursor-top-1",
                      "content": { "entryType": "TimelineTimelineCursor",
                                   "cursorType": "Top", "value": "TOP_CURSOR" } },
                    { "entryId": "tweet-1", "sortIndex": "900",
                      "content": { "entryType": "TimelineTimelineItem",
                                   "itemContent": { "tweet_results": { "result": tweet("1", "first") } } } },
                    { "entryId": "tweet-2", "sortIndex": "800",
                      "content": { "entryType": "TimelineTimelineItem",
                                   "itemContent": { "tweet_results": { "result": tweet("2", "second") } } } },
                    { "entryId": "cursor-bottom-1",
                      "content": { "entryType": "TimelineTimelineCursor",
                                   "cursorType": "Bottom", "value": "BOTTOM_CURSOR" } }
                ]}
            ]}}}
        })
    }

    #[test]
    fn extracts_a_bookmarks_page_with_ordering_and_cursors() {
        let e = extract(&bookmarks_page());
        assert_eq!(e.tweets.len(), 2);
        assert_eq!(e.sort_index_of("1"), Some("900"));
        assert_eq!(e.sort_index_of("2"), Some("800"));
        // The bottom cursor is the pagination handle; the top one is not.
        assert_eq!(e.bottom_cursor.as_deref(), Some("BOTTOM_CURSOR"));
        assert_eq!(e.top_cursor.as_deref(), Some("TOP_CURSOR"));
    }

    #[test]
    fn sort_index_is_read_from_the_entry_not_from_legacy() {
        // The regression this guards: reading legacy.sort_index gives None
        // forever, and the failure looks like "X doesn't provide ordering".
        let mut page = bookmarks_page();
        page["data"]["bookmark_timeline_v2"]["timeline"]["instructions"][0]["entries"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e.get("entryId").and_then(Value::as_str) != Some("cursor-top-1"));
        let e = extract(&page);
        assert!(
            !e.sort_indices.is_empty(),
            "sortIndex was not recovered — is it being read from legacy?"
        );
    }

    #[test]
    fn a_single_tweet_tweetdetail_envelope_yields_one_tweet() {
        // The "import a single tweet" case, with no special code path.
        let payload = json!({
            "data": { "tweetResult": { "result": tweet("99", "just one") } }
        });
        let e = extract(&payload);
        assert_eq!(e.tweets.len(), 1);
        assert_eq!(e.tweets[0].id, "99");
        assert_eq!(e.tweets[0].text, "just one");
        assert!(e.bottom_cursor.is_none());
    }

    #[test]
    fn a_bare_tweet_result_subtree_works() {
        let e = extract(&tweet("7", "bare"));
        assert_eq!(e.tweets.len(), 1);
        assert_eq!(e.tweets[0].id, "7");
    }

    #[test]
    fn a_user_object_is_not_mistaken_for_a_tweet() {
        // user_results.result has rest_id AND legacy. Getting this wrong
        // imports every author as a post.
        let payload = json!({
            "data": { "user": { "result": {
                "__typename": "User",
                "rest_id": "42",
                "legacy": { "screen_name": "ada", "name": "Ada" }
            } } }
        });
        assert!(extract(&payload).tweets.is_empty());
    }

    #[test]
    fn quoted_tweets_do_not_become_top_level_bookmarks() {
        let mut t = tweet("1", "look at this");
        t["quoted_status_result"] = json!({ "result": tweet("2", "the quoted one") });
        let e = extract(&json!({ "data": { "tweetResult": { "result": t } } }));

        assert_eq!(e.tweets.len(), 1, "the quoted post leaked in as a bookmark");
        assert_eq!(e.tweets[0].id, "1");
        assert_eq!(e.tweets[0].quoted.as_ref().unwrap().id, "2");
    }

    #[test]
    fn conversation_modules_contribute_tweets_but_not_ordering() {
        let payload = json!({
            "data": { "threaded_conversation_with_injections_v2": { "instructions": [
                { "type": "TimelineAddEntries", "entries": [
                    { "entryId": "tweet-1", "sortIndex": "500",
                      "content": { "entryType": "TimelineTimelineItem",
                                   "itemContent": { "tweet_results": { "result": tweet("1", "root") } } } },
                    { "entryId": "module-1",
                      "content": { "entryType": "TimelineTimelineModule", "items": [
                          { "item": { "itemContent": { "tweet_results": { "result": tweet("2", "reply") } } } }
                      ] } }
                ]}
            ]}}
        });
        let e = extract(&payload);
        assert_eq!(e.tweets.len(), 2, "module tweets should be captured");
        assert_eq!(e.sort_index_of("1"), Some("500"));
        assert_eq!(
            e.sort_index_of("2"),
            None,
            "module ordering must not enter bookmark ordering"
        );
    }

    #[test]
    fn tolerates_an_unknown_instruction_type() {
        let payload = json!({
            "data": { "some_timeline": { "timeline": { "instructions": [
                { "type": "TimelineSomethingNew", "whatever": [1, 2, 3] },
                { "type": "TimelineAddEntries", "entries": [
                    { "entryId": "tweet-1", "sortIndex": "1",
                      "content": { "entryType": "TimelineTimelineItem",
                                   "itemContent": { "tweet_results": { "result": tweet("1", "survived") } } } }
                ]}
            ]}}}
        });
        let e = extract(&payload);
        assert_eq!(e.tweets.len(), 1, "an unknown instruction broke the walk");
    }

    #[test]
    fn an_empty_payload_is_empty_not_an_error() {
        assert!(extract(&json!({})).is_empty());
        assert!(extract(&json!({ "data": {} })).is_empty());
    }

    #[test]
    fn deep_nesting_does_not_blow_the_stack() {
        let mut v = json!({ "leaf": 1 });
        for _ in 0..500 {
            v = json!({ "nested": v });
        }
        assert!(extract(&v).is_empty());
    }
}
