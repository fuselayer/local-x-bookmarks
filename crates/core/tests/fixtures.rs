//! Fixture validation.
//!
//! ## Why this exists
//!
//! Entity indices in these fixtures are hand-written, and hand-written indices
//! are wrong more often than not. The renderer *verifies* every slice before
//! styling it and silently degrades to plain text on a mismatch — which is the
//! right runtime behaviour, but it means a bad fixture renders a tweet with no
//! links and reports no error anywhere.
//!
//! So the fixtures get checked here instead. If someone edits a `full_text`
//! without recomputing its indices, this fails loudly.
//!
//! ## Three index conventions, and why the helpers below exist
//!
//! This is the subtle part, and getting it wrong makes the whole test
//! meaningless:
//!
//! | Consumer | Unit |
//! |---|---|
//! | JavaScript `String.slice` (what actually renders) | UTF-16 code units |
//! | X's public API documentation | Unicode code points |
//! | Rust `str` indexing | **UTF-8 bytes** |
//!
//! Rust's `String::len()` and `str` slicing use *bytes*, so naively slicing a
//! Rust string with X's indices tests nothing at all — it agrees with both
//! other conventions on ASCII and silently diverges on everything else. Since
//! the renderer is JavaScript, these tests must reproduce JavaScript's
//! behaviour exactly, which is what `js_slice` does.

use std::path::PathBuf;

use xdl_core::model::Tweet;
use xdl_core::x;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

fn one_tweet(name: &str) -> Tweet {
    let value = fixture(name);
    let extracted = x::extract(&value);
    assert_eq!(
        extracted.tweets.len(),
        1,
        "{name} should contain exactly one tweet"
    );
    extracted.tweets.into_iter().next().unwrap()
}

/// Convert a UTF-16 offset (what JavaScript uses) into a UTF-8 byte offset.
///
/// Returns `None` if the offset lands in the middle of a surrogate pair, which
/// is itself a useful signal that the index convention is wrong.
fn utf16_to_byte(s: &str, u16_offset: usize) -> Option<usize> {
    let mut seen = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if seen == u16_offset {
            return Some(byte_idx);
        }
        seen += ch.len_utf16();
        if seen > u16_offset {
            return None; // mid-surrogate
        }
    }
    (seen == u16_offset).then_some(s.len())
}

/// Convert a code-point index into a UTF-16 offset.
fn code_point_to_utf16(s: &str, cp_index: usize) -> usize {
    s.chars().take(cp_index).map(char::len_utf16).sum()
}

/// Slice a Rust string the way JavaScript's `String.slice` would.
fn js_slice(s: &str, start: usize, end: usize) -> Option<&str> {
    let a = utf16_to_byte(s, start)?;
    let b = utf16_to_byte(s, end)?;
    s.get(a..b)
}

/// Every entity must resolve under at least one index convention, and the
/// resulting slice must equal the entity's own literal text.
///
/// This mirrors the frontend's verification guard exactly. If it passes here
/// but the app shows unstyled links, the two implementations have drifted.
fn assert_entities_resolve(t: &Tweet, name: &str) {
    let mut checked = 0;

    let mut check = |start: usize, end: usize, expected: &str, what: &str| {
        let as_utf16 = js_slice(&t.text, start, end);

        let (a, b) = (
            code_point_to_utf16(&t.text, start),
            code_point_to_utf16(&t.text, end),
        );
        let as_code_points = js_slice(&t.text, a, b);

        assert!(
            as_utf16 == Some(expected) || as_code_points == Some(expected),
            "{name}: {what} at [{start}, {end}) resolves under NEITHER convention.\n  \
             expected      : {expected:?}\n  \
             as utf16      : {as_utf16:?}\n  \
             as code points: {as_code_points:?}\n  \
             full_text     : {:?}",
            t.text
        );
        checked += 1;
    };

    for u in &t.entities.urls {
        check(u.start, u.end, &u.url, "url entity");
    }
    for m in &t.entities.mentions {
        check(m.start, m.end, &format!("@{}", m.handle), "mention entity");
    }
    for h in &t.entities.hashtags {
        check(h.start, h.end, &format!("#{}", h.text), "hashtag entity");
    }

    assert!(
        checked > 0,
        "{name} has no entities to check; is the fixture stale?"
    );
}

#[test]
fn tweet_simple_parses_and_its_entities_resolve() {
    let t = one_tweet("tweet-simple.json");

    assert_eq!(t.id, "1843712994563928064");
    assert_eq!(t.author.handle, "adalovelace");
    assert_eq!(t.author.name, "Ada Lovelace");
    assert!(t.author.blue_verified);
    assert!(!t.is_long_form);

    // 2025-10-08T14:12:30Z
    assert_eq!(t.created_at, Some(1_759_932_750));

    assert_eq!(t.metrics.likes, 1284);
    assert_eq!(t.metrics.reposts, 213);
    assert_eq!(t.metrics.views, Some(48_213));

    assert_eq!(t.entities.urls.len(), 1);
    assert_eq!(t.entities.mentions.len(), 1);
    assert_eq!(t.entities.hashtags.len(), 1);

    // The card, which is what makes the link preview render.
    let card = t.card.as_ref().expect("fixture should carry a card");
    assert_eq!(
        card.title.as_deref(),
        Some("A local-first archive of your own X bookmarks")
    );
    assert_eq!(card.domain.as_deref(), Some("example.com"));
    assert_eq!(card.image_width, Some(1200));

    assert_entities_resolve(&t, "tweet-simple.json");
}

#[test]
fn tweet_media_parses_including_the_visibility_wrapper() {
    let t = one_tweet("tweet-media.json");

    // Arrived wrapped in TweetWithVisibilityResults; failing to unwrap is the
    // classic "my parser randomly returns nothing" bug.
    assert_eq!(t.id, "1843712994563928100");
    assert_eq!(t.author.handle, "marguerite");
    // Legacy verified, not Premium blue. X renders these differently.
    assert!(t.author.verified);
    assert!(!t.author.blue_verified);

    assert_eq!(t.media.len(), 4, "all four photos should parse");
    assert_eq!(t.media[0].width, Some(2400));
    assert_eq!(t.media[0].height, Some(1600));
    assert!(t.media[0]
        .alt_text
        .as_deref()
        .unwrap()
        .contains("Portland"));
    // The third photo has no alt text, which is normal and must not fail.
    assert_eq!(t.media[2].alt_text, None);

    let quoted = t.quoted.as_ref().expect("quoted post should parse");
    assert_eq!(quoted.id, "1843712994563928000");
    assert_eq!(quoted.author.handle, "pdxfilmlab");
    assert_eq!(quoted.metrics.likes, 96);

    assert_entities_resolve(&t, "tweet-media.json");
}

#[test]
fn the_emoji_fixture_actually_exercises_the_utf16_codepoint_divergence() {
    // If these two ever agree, the fixture has stopped testing what it is for
    // and someone should either restore the emoji or delete this test.
    let t = one_tweet("tweet-media.json");

    let astral: Vec<char> = t.text.chars().filter(|c| c.len_utf16() == 2).collect();
    assert_eq!(
        astral.len(),
        1,
        "fixture should contain exactly one astral character, found {astral:?}"
    );
    assert_eq!(
        t.text.chars().count(),
        t.text.chars().map(char::len_utf16).sum::<usize>() - 1,
        "exactly one character should occupy two UTF-16 units"
    );

    let mention = &t.entities.mentions[0];
    assert_eq!(
        js_slice(&t.text, mention.start, mention.end),
        Some("@ilfordphoto"),
        "the fixture is written with UTF-16 indices, which is what the web client uses"
    );

    // The code-point reading lands one character early — exactly the
    // corruption the frontend's verification guard exists to catch.
    let (a, b) = (
        code_point_to_utf16(&t.text, mention.start),
        code_point_to_utf16(&t.text, mention.end),
    );
    assert_ne!(
        (a, b),
        (mention.start, mention.end),
        "the two conventions should disagree here"
    );
    assert_ne!(js_slice(&t.text, a, b), Some("@ilfordphoto"));
}

#[test]
fn byte_offsets_are_not_a_valid_reading_of_these_indices() {
    // Guards the mistake this file used to make: slicing a Rust string
    // directly. On the emoji fixture that reading must be wrong, which is what
    // makes the helpers above load-bearing rather than ceremony.
    let t = one_tweet("tweet-media.json");
    let mention = &t.entities.mentions[0];

    let by_bytes = t.text.get(mention.start..mention.end);
    assert_ne!(
        by_bytes,
        Some("@ilfordphoto"),
        "byte slicing agreed with UTF-16 slicing, so this fixture cannot detect \
         a unit mismatch — re-check the fixture text"
    );
}

#[test]
fn fixtures_are_importable_end_to_end() {
    // The whole point of a fixture: it goes in through the real importer and
    // comes back out through the real read path.
    let lib = xdl_core::db::Library::open_in_memory().unwrap();

    for name in ["tweet-simple.json", "tweet-media.json"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        let summary = xdl_core::import::import_file(lib.conn(), &path, 1_700_000_000)
            .unwrap_or_else(|e| panic!("importing {name} failed: {e}"));
        assert_eq!(summary.stats.new, 1, "{name} did not import a new post");
        assert!(summary.problems.is_empty(), "{name}: {:?}", summary.problems);
    }

    let stats = xdl_core::db::read::stats(lib.conn()).unwrap();
    assert_eq!(stats.posts, 3, "two roots plus one quoted post");
    assert_eq!(stats.bookmarks, 2, "quoted posts are not bookmarks");
    assert_eq!(stats.media, 4);

    // And the text is searchable through both retrievers.
    let hits = xdl_core::search::search(lib.conn(), "localfirst", 10).unwrap();
    assert_eq!(hits.len(), 1, "hashtag text should be searchable");
    let sub = xdl_core::search::search(lib.conn(), "ilmphotog", 10).unwrap();
    assert_eq!(
        sub.len(),
        1,
        "trigram substring search should find the hashtag"
    );
}
