//! Turning one `tweet_results.result` subtree into a [`Tweet`].
//!
//! ## Tolerance policy
//!
//! Every field is optional and every lookup is a `pointer()` that returns
//! `Option`. This is not laziness — X ships response-shape changes on its own
//! schedule, and the failure mode we must avoid is a parser that throws and
//! loses a payload we already hold. A tweet with a missing view count is fine.
//! A tweet that fails to parse is data loss.
//!
//! The only hard requirement is an ID, because without one we cannot dedupe
//! or key the row.

use serde_json::Value;

use crate::model::{
    Author, Card, Entities, EntityIndexUnits, Media, MediaKind, MentionEntity, Metrics, TagEntity,
    Tweet, UrlEntity,
};
use crate::x::date::parse_x_timestamp;

/// Bump when the output of this module changes meaning. Stored on each row so
/// a later fix can find the records that predate it.
pub const PARSER_VERSION: u32 = 1;

// ── JSON helpers ─────────────────────────────────────────────────────────────
// Small and boring on purpose. `pointer()` returns Option, which is exactly
// the shape we want; these just save repeating the type dance.

fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr)?.as_str().filter(|s| !s.is_empty())
}

fn i(v: &Value, ptr: &str) -> Option<i64> {
    let n = v.pointer(ptr)?;
    n.as_i64().or_else(|| n.as_str().and_then(|s| s.parse().ok()))
}

fn b(v: &Value, ptr: &str) -> bool {
    v.pointer(ptr).and_then(Value::as_bool).unwrap_or(false)
}

fn indices(v: &Value) -> Option<(usize, usize)> {
    let a = v.get("indices")?.as_array()?;
    if a.len() != 2 {
        return None;
    }
    Some((a[0].as_u64()? as usize, a[1].as_u64()? as usize))
}

/// `https://github.com/foo/bar` -> `github.com`
///
/// Hand-rolled rather than pulling in the `url` crate: this is the only URL
/// work core does, and it must not fail on the malformed strings that show up
/// in real payloads.
fn domain_of(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = after_scheme.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    // Strip a leading `www.` the way X does when it shows the card footer.
    let host = host.strip_prefix("www.").unwrap_or(host);
    Some(host.to_ascii_lowercase())
}

// ── Entry point ──────────────────────────────────────────────────────────────

/// Parse a `tweet_results.result` value.
///
/// Handles the two wrappers X puts around tweets, and returns `None` for the
/// tombstones it uses for deleted or withheld posts.
pub fn parse_tweet_result(value: &Value) -> Option<Tweet> {
    let result = match value.get("__typename").and_then(Value::as_str) {
        // A visibility-filtered tweet is a normal tweet one level down. Not
        // unwrapping this is the single most common cause of "my parser
        // randomly returns nothing".
        Some("TweetWithVisibilityResults") => value.get("tweet")?,
        // Deleted, or withheld in the viewer's region. Nothing to store.
        Some("TweetTombstone") => return None,
        _ => value,
    };

    parse_tweet(result)
}

/// Parse an unwrapped tweet object (the thing with `legacy` on it).
pub fn parse_tweet(v: &Value) -> Option<Tweet> {
    let id = s(v, "/rest_id")
        .or_else(|| s(v, "/legacy/id_str"))
        .map(str::to_owned)?;

    let legacy = v.get("legacy").unwrap_or(v);

    // ── long-form text ───────────────────────────────────────────────────────
    // `note_tweet` is the real body of a long post. `full_text` for such a
    // post is truncated with an ellipsis and a t.co link to the rest, so
    // preferring the wrong one silently loses the end of every long tweet.
    let note_text = s(v, "/note_tweet/note_tweet_results/result/text").map(str::to_owned);
    let full_text = s(legacy, "/full_text").map(str::to_owned);
    let is_long_form = note_text.is_some();
    let text = note_text.clone().or_else(|| full_text.clone())?;

    // Entities must come from whichever text we chose — mixing a note body
    // with legacy indices misplaces every link in it.
    //
    // Note the shape difference between the two branches, which is a real
    // trap: a note's `entity_set` *is* the entity container, while a legacy
    // tweet nests the same fields one level deeper under `entities`. Passing
    // `legacy` where `legacy.entities` was meant yields an empty entity list
    // rather than an error, so every link, mention and hashtag silently loses
    // its styling.
    let entities = if is_long_form {
        parse_entities(
            v.pointer("/note_tweet/note_tweet_results/result/entity_set")
                .unwrap_or(&Value::Null),
            legacy,
        )
    } else {
        parse_entities(legacy.get("entities").unwrap_or(legacy), legacy)
    };

    Some(Tweet {
        id,
        author: parse_author(v),
        text,
        truncated_text: if is_long_form { full_text } else { None },
        is_long_form,
        entities,
        created_at: s(legacy, "/created_at").and_then(parse_x_timestamp),
        lang: s(legacy, "/lang").map(str::to_owned),
        conversation_id: s(legacy, "/conversation_id_str").map(str::to_owned),
        in_reply_to_id: s(legacy, "/in_reply_to_status_id_str").map(str::to_owned),
        in_reply_to_handle: s(legacy, "/in_reply_to_screen_name").map(str::to_owned),
        metrics: parse_metrics(v, legacy),
        media: parse_media(legacy),
        card: parse_card(v),
        quoted: v
            .pointer("/quoted_status_result/result")
            .and_then(parse_tweet_result)
            .map(Box::new),
        parser_version: PARSER_VERSION,
    })
}

// ── Author ───────────────────────────────────────────────────────────────────

fn parse_author(v: &Value) -> Author {
    // X moved these from `legacy` to `core` in 2024 and kept both for a
    // while. Read `core` first, fall back to `legacy`; that ordering matters
    // because on some responses `legacy` is stale.
    let user = v.pointer("/core/user_results/result").unwrap_or(&Value::Null);

    let handle = s(user, "/core/screen_name")
        .or_else(|| s(user, "/legacy/screen_name"))
        .unwrap_or("unknown")
        .to_owned();

    let name = s(user, "/core/name")
        .or_else(|| s(user, "/legacy/name"))
        .unwrap_or(&handle)
        .to_owned();

    let avatar_url = s(user, "/avatar/image_url")
        .or_else(|| s(user, "/legacy/profile_image_url_https"))
        .map(str::to_owned);

    let verified_type = s(user, "/verification/verified_type")
        .or_else(|| s(user, "/legacy/verified_type"))
        .map(str::to_owned);

    Author {
        id: s(user, "/rest_id").unwrap_or_default().to_owned(),
        handle,
        name,
        avatar_url,
        verified: b(user, "/legacy/verified") || b(user, "/verification/verified"),
        blue_verified: b(user, "/is_blue_verified"),
        verified_type,
    }
}

// ── Metrics ──────────────────────────────────────────────────────────────────

fn parse_metrics(v: &Value, legacy: &Value) -> Metrics {
    Metrics {
        likes: i(legacy, "/favorite_count").unwrap_or(0),
        reposts: i(legacy, "/retweet_count").unwrap_or(0),
        replies: i(legacy, "/reply_count").unwrap_or(0),
        quotes: i(legacy, "/quote_count").unwrap_or(0),
        bookmarks: i(legacy, "/bookmark_count").unwrap_or(0),
        // Arrives as a *string* and is sometimes absent entirely when the
        // author has hidden view counts. Not an error, just missing.
        views: i(v, "/views/count"),
    }
}

// ── Entities ─────────────────────────────────────────────────────────────────

/// Build the entity set.
///
/// `text_source` is where the text's own entities live (either `legacy` or the
/// note's `entity_set`); `media_source` is always `legacy`, because media is
/// never part of a note's entity set.
fn parse_entities(text_source: &Value, media_source: &Value) -> Entities {
    let urls = text_source
        .get("urls")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|u| {
                    let (start, end) = indices(u)?;
                    Some(UrlEntity {
                        url: s(u, "/url").unwrap_or_default().to_owned(),
                        display: s(u, "/display_url")
                            .or_else(|| s(u, "/expanded_url"))
                            .unwrap_or_default()
                            .to_owned(),
                        // The real destination. This is what replaces t.co
                        // unfurling — see model.rs and PRD §7.7.
                        expanded: s(u, "/expanded_url")
                            .or_else(|| s(u, "/url"))
                            .unwrap_or_default()
                            .to_owned(),
                        start,
                        end,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let mentions = text_source
        .get("user_mentions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let (start, end) = indices(m)?;
                    Some(MentionEntity {
                        handle: s(m, "/screen_name").unwrap_or_default().to_owned(),
                        name: s(m, "/name").map(str::to_owned),
                        start,
                        end,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let tags = |key: &str| -> Vec<TagEntity> {
        text_source
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        let (start, end) = indices(t)?;
                        Some(TagEntity {
                            text: s(t, "/text").unwrap_or_default().to_owned(),
                            start,
                            end,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let media_indices = media_source
        .pointer("/extended_entities/media")
        .or_else(|| media_source.pointer("/entities/media"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(indices).collect())
        .unwrap_or_default();

    Entities {
        // The web client slices with JS String.slice, so UTF-16 is the likelier
        // convention. The renderer verifies each slice regardless and falls
        // back to plain text on a mismatch, so a wrong guess here degrades
        // rather than corrupts.
        units: Some(EntityIndexUnits::Utf16),
        urls,
        mentions,
        hashtags: tags("hashtags"),
        symbols: tags("symbols"),
        media_indices,
    }
}

// ── Media ────────────────────────────────────────────────────────────────────

fn parse_media(legacy: &Value) -> Vec<Media> {
    // `extended_entities` is the one with video variants; `entities.media` is
    // the degraded copy. Prefer the former, fall back to the latter.
    let items = legacy
        .pointer("/extended_entities/media")
        .or_else(|| legacy.pointer("/entities/media"))
        .and_then(Value::as_array);

    let Some(items) = items else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|m| {
            let kind = match s(m, "/type") {
                Some("photo") => MediaKind::Photo,
                Some("video") => MediaKind::Video,
                Some("animated_gif") => MediaKind::Gif,
                _ => return None,
            };

            // Highest-bitrate progressive MP4. X itself plays the HLS
            // manifest, but we are archiving a file, not streaming.
            let video_url = m
                .pointer("/video_info/variants")
                .and_then(Value::as_array)
                .and_then(|vs| {
                    vs.iter()
                        .filter(|v| s(v, "/content_type") == Some("video/mp4"))
                        .max_by_key(|v| i(v, "/bitrate").unwrap_or(0))
                })
                .and_then(|v| s(v, "/url"))
                .map(str::to_owned);

            let url = s(m, "/media_url_https").or_else(|| s(m, "/media_url"))?.to_owned();

            // Prefer `original_info` (true pixel dimensions) over `sizes`,
            // which describes display buckets rather than the image.
            let width = i(m, "/original_info/width").or_else(|| i(m, "/sizes/large/w"));
            let height = i(m, "/original_info/height").or_else(|| i(m, "/sizes/large/h"));

            Some(Media {
                kind,
                url,
                alt_text: s(m, "/ext_alt_text").map(str::to_owned),
                width,
                height,
                video_url,
                duration_ms: i(m, "/video_info/duration_millis"),
            })
        })
        .collect()
}

// ── Card ─────────────────────────────────────────────────────────────────────

/// Parse a link-preview card.
///
/// The card is a `binding_values` list of key/value pairs with a type tag per
/// value, which is why this looks like a lookup table rather than a struct
/// read. Image keys differ per card type, so several are tried in order.
fn parse_card(v: &Value) -> Option<Card> {
    let card = v.get("card")?;
    let bindings = card.pointer("/legacy/binding_values").and_then(Value::as_array)?;

    let string_at = |key: &str| -> Option<String> {
        bindings
            .iter()
            .find(|b| s(b, "/key") == Some(key))
            .and_then(|b| s(b, "/value/string_value"))
            .map(str::to_owned)
    };

    let image_at = |key: &str| -> Option<(String, Option<i64>, Option<i64>)> {
        let val = bindings
            .iter()
            .find(|b| s(b, "/key") == Some(key))?
            .pointer("/value/image_value")?;
        Some((s(val, "/url")?.to_owned(), i(val, "/width"), i(val, "/height")))
    };

    // Order matters: the first key present wins, largest image first.
    let image = [
        "photo_image_full_size_original",
        "summary_photo_image_original",
        "thumbnail_image_original",
        "player_image_original",
        "thumbnail_image",
    ]
    .iter()
    .find_map(|k| image_at(k));

    let card = Card {
        name: s(card, "/name").map(str::to_owned),
        title: string_at("title"),
        description: string_at("description"),
        url: string_at("card_url")
            .or_else(|| string_at("vanity_url"))
            .or_else(|| string_at("domain")),
        domain: string_at("vanity_url")
            .as_deref()
            .and_then(domain_of)
            .or_else(|| string_at("card_url").as_deref().and_then(domain_of)),
        image_url: image.as_ref().map(|(u, _, _)| u.clone()),
        image_width: image.as_ref().and_then(|(_, w, _)| *w),
        image_height: image.as_ref().and_then(|(_, _, h)| *h),
    };

    // A card with nothing in it is noise; do not store an empty object that
    // the UI then has to special-case.
    if card.title.is_none() && card.description.is_none() && card.image_url.is_none() {
        return None;
    }
    Some(card)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_tweet() -> Value {
        json!({
            "__typename": "Tweet",
            "rest_id": "1000",
            "core": {
                "user_results": {
                    "result": {
                        "rest_id": "42",
                        "core": { "name": "Ada", "screen_name": "ada" },
                        "avatar": { "image_url": "https://pbs.twimg.com/a.jpg" },
                        "is_blue_verified": true
                    }
                }
            },
            "legacy": {
                "id_str": "1000",
                "full_text": "hello world",
                "created_at": "Wed Oct 10 20:19:24 +0000 2018",
                "lang": "en",
                "favorite_count": 7,
                "retweet_count": 2,
                "reply_count": 1,
                "quote_count": 0,
                "bookmark_count": 3,
                "entities": { "urls": [], "user_mentions": [], "hashtags": [] }
            },
            "views": { "count": "1234" }
        })
    }

    #[test]
    fn parses_a_minimal_tweet() {
        let t = parse_tweet_result(&minimal_tweet()).expect("should parse");
        assert_eq!(t.id, "1000");
        assert_eq!(t.text, "hello world");
        assert_eq!(t.author.handle, "ada");
        assert_eq!(t.author.name, "Ada");
        assert!(t.author.blue_verified);
        assert_eq!(t.metrics.likes, 7);
        assert_eq!(t.metrics.views, Some(1234)); // string in, number out
        assert_eq!(t.created_at, Some(1_539_202_764));
        assert!(!t.is_long_form);
    }

    #[test]
    fn unwraps_visibility_limited_tweets() {
        // Without this unwrap the parser silently returns nothing, which is
        // the most common "my exporter misses tweets" bug.
        let wrapped = json!({
            "__typename": "TweetWithVisibilityResults",
            "tweet": minimal_tweet()
        });
        let t = parse_tweet_result(&wrapped).expect("wrapper should be unwrapped");
        assert_eq!(t.id, "1000");
    }

    #[test]
    fn tombstones_are_skipped_not_errored() {
        let tomb = json!({ "__typename": "TweetTombstone", "tombstone": { "text": "gone" } });
        assert!(parse_tweet_result(&tomb).is_none());
    }

    #[test]
    fn prefers_note_tweet_text_for_long_posts() {
        let mut v = minimal_tweet();
        v["note_tweet"] = json!({
            "note_tweet_results": {
                "result": {
                    "text": "the full long body",
                    "entity_set": { "urls": [], "user_mentions": [], "hashtags": [] }
                }
            }
        });
        let t = parse_tweet_result(&v).unwrap();
        assert_eq!(t.text, "the full long body");
        assert_eq!(t.truncated_text.as_deref(), Some("hello world"));
        assert!(t.is_long_form);
    }

    #[test]
    fn a_missing_id_is_the_only_hard_failure() {
        let mut v = minimal_tweet();
        v.as_object_mut().unwrap().remove("rest_id");
        v["legacy"].as_object_mut().unwrap().remove("id_str");
        assert!(parse_tweet_result(&v).is_none());
    }

    #[test]
    fn reads_card_bindings() {
        let mut v = minimal_tweet();
        v["card"] = json!({
            "name": "summary_large_image",
            "legacy": {
                "binding_values": [
                    { "key": "title", "value": { "string_value": "A Title" } },
                    { "key": "description", "value": { "string_value": "A desc" } },
                    { "key": "card_url", "value": { "string_value": "https://www.example.com/x" } },
                    { "key": "summary_photo_image_original",
                      "value": { "image_value": { "url": "https://pbs.twimg.com/c.jpg", "width": 800, "height": 419 } } }
                ]
            }
        });
        let c = parse_tweet_result(&v).unwrap().card.unwrap();
        assert_eq!(c.title.as_deref(), Some("A Title"));
        assert_eq!(c.image_url.as_deref(), Some("https://pbs.twimg.com/c.jpg"));
        assert_eq!(c.domain.as_deref(), Some("example.com")); // www. stripped
        assert_eq!(c.image_width, Some(800));
    }

    #[test]
    fn empty_cards_are_dropped() {
        let mut v = minimal_tweet();
        v["card"] = json!({ "name": "summary", "legacy": { "binding_values": [] } });
        assert!(parse_tweet_result(&v).unwrap().card.is_none());
    }

    #[test]
    fn picks_highest_bitrate_mp4_for_video() {
        let mut v = minimal_tweet();
        v["legacy"]["extended_entities"] = json!({
            "media": [{
                "type": "video",
                "media_url_https": "https://pbs.twimg.com/thumb.jpg",
                "original_info": { "width": 1280, "height": 720 },
                "video_info": {
                    "duration_millis": 4200,
                    "variants": [
                        { "bitrate": 256000, "content_type": "video/mp4", "url": "https://v/low.mp4" },
                        { "bitrate": 2176000, "content_type": "video/mp4", "url": "https://v/high.mp4" },
                        { "content_type": "application/x-mpegURL", "url": "https://v/playlist.m3u8" }
                    ]
                }
            }]
        });
        let t = parse_tweet_result(&v).unwrap();
        assert_eq!(t.media.len(), 1);
        assert_eq!(t.media[0].kind, MediaKind::Video);
        assert_eq!(t.media[0].video_url.as_deref(), Some("https://v/high.mp4"));
        assert_eq!(t.media[0].duration_ms, Some(4200));
    }

    #[test]
    fn parses_entities_with_indices() {
        let mut v = minimal_tweet();
        v["legacy"]["full_text"] = json!("see https://t.co/abc now");
        v["legacy"]["entities"] = json!({
            "urls": [{
                "url": "https://t.co/abc",
                "display_url": "example.com/page",
                "expanded_url": "https://example.com/page",
                "indices": [4, 20]
            }],
            "user_mentions": [], "hashtags": []
        });
        let t = parse_tweet_result(&v).unwrap();
        assert_eq!(t.entities.urls.len(), 1);
        assert_eq!(t.entities.urls[0].display, "example.com/page");
        assert_eq!(t.entities.urls[0].expanded, "https://example.com/page");
        assert_eq!((t.entities.urls[0].start, t.entities.urls[0].end), (4, 20));
        // The slice the renderer will verify actually matches. "see " is four
        // characters and "https://t.co/abc" is sixteen, so the entity occupies
        // [4, 20) — end is exclusive.
        assert_eq!(&t.text[4..20], "https://t.co/abc");
    }

    #[test]
    fn malformed_entity_is_skipped_without_losing_the_tweet() {
        let mut v = minimal_tweet();
        v["legacy"]["entities"] = json!({
            "urls": [{ "url": "https://t.co/x" }],   // no indices
            "user_mentions": [], "hashtags": []
        });
        let t = parse_tweet_result(&v).unwrap();
        assert!(t.entities.urls.is_empty());
        assert_eq!(t.text, "hello world");
    }

    #[test]
    fn domain_extraction_handles_awkward_input() {
        assert_eq!(domain_of("https://www.example.com/a/b?c=d").as_deref(), Some("example.com"));
        assert_eq!(domain_of("http://sub.example.co.uk").as_deref(), Some("sub.example.co.uk"));
        assert_eq!(domain_of("not a url").as_deref(), Some("not a url"));
        assert_eq!(domain_of(""), None);
    }
}
