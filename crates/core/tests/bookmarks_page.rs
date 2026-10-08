//! End-to-end test of the generated bookmarks-page fixture.
//!
//! ## Why this and the userscript test share a fixture
//!
//! `userscript/test/capture.test.mjs` and this file read the same payload and
//! the same expectations document. That pins the two halves of the wire
//! together: the JavaScript that recognises and ships a page, and the Rust that
//! parses it. If either drifts, one of the two suites fails.
//!
//! The fixture is produced by `scripts/make-fixture-bookmarks.mjs`, and its
//! expectations are derived from the generator's input array rather than from
//! the emitted JSON — so a hand-edit to the fixture breaks these tests instead
//! of silently redefining them.

use std::path::PathBuf;

use xdl_core::db::{read, Library};
use xdl_core::import;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

fn expected() -> serde_json::Value {
    serde_json::from_str(&fixture("bookmarks-page.expected.json")).expect("expectations are JSON")
}

fn import_page(lib: &Library) -> xdl_core::import::ImportSummary {
    let text = fixture("bookmarks-page.json");
    import::payload::import_json_text(lib.conn(), &text, Some("Bookmarks"), 1_700_000_000)
        .expect("the fixture should import")
}

#[test]
fn the_expectations_file_is_internally_consistent() {
    // Guards the guard: if the generator ever emits a contradictory document,
    // every assertion below would be testing nonsense.
    let e = expected();
    let ordered = e["orderedIds"].as_array().unwrap().len();
    let unsorted = e["unsortedIds"].as_array().unwrap().len();
    assert_eq!(
        ordered + unsorted,
        e["bookmarkCount"].as_u64().unwrap() as usize,
        "bookmarkCount must equal every id listed"
    );
    assert_eq!(
        e["sortIndexes"].as_object().unwrap().len(),
        ordered,
        "every ordered id needs a sortIndex"
    );
    assert_ne!(e["topCursor"], e["bottomCursor"]);
}

#[test]
fn the_parser_finds_every_bookmark_the_generator_put_in() {
    let lib = Library::open_in_memory().unwrap();
    let e = expected();

    let summary = import_page(&lib);

    assert_eq!(
        summary.stats.new,
        e["bookmarkCount"].as_u64().unwrap() as usize,
        "every post in the page should become a bookmark, including the one \
         inside a conversation module"
    );
    assert_eq!(lib.count_bookmarks().unwrap(), summary.stats.new as i64);
    assert!(summary.problems.is_empty(), "{:?}", summary.problems);
}

#[test]
fn ordering_comes_from_the_entry_level_sort_index() {
    // The regression this guards is the nastiest one in the project: reading
    // `legacy.sort_index` instead of `entries[].sortIndex` yields None on every
    // record, silently and forever, and looks like "ordering just isn't
    // available" rather than like a bug.
    let lib = Library::open_in_memory().unwrap();
    let e = expected();
    import_page(&lib);

    for (id, want) in e["sortIndexes"].as_object().unwrap() {
        let post = read::get_post(lib.conn(), id)
            .unwrap()
            .unwrap_or_else(|| panic!("post {id} should exist"));
        assert_eq!(
            post.sort_index.as_deref(),
            want.as_str(),
            "post {id} has the wrong ordering key"
        );
    }

    // A conversation-module tweet has no entry-level sortIndex of its own. It
    // is captured with None rather than skipped, so it is never lost.
    for id in e["unsortedIds"].as_array().unwrap() {
        let id = id.as_str().unwrap();
        let post = read::get_post(lib.conn(), id).unwrap().unwrap();
        assert_eq!(post.sort_index, None, "post {id} should have no ordering key");
    }
}

#[test]
fn the_bottom_cursor_is_recorded_as_the_pagination_handle() {
    let lib = Library::open_in_memory().unwrap();
    let e = expected();
    import_page(&lib);

    let cursor: Option<String> = lib
        .conn()
        .query_row(
            "SELECT cursor_after FROM capture_payloads ORDER BY captured_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(
        cursor.as_deref(),
        e["bottomCursor"].as_str(),
        "the bottom cursor is what a resume would need"
    );
    assert_ne!(
        cursor.as_deref(),
        e["topCursor"].as_str(),
        "the top cursor is noise and must not be mistaken for progress"
    );
}

#[test]
fn a_long_post_is_stored_with_its_untruncated_body() {
    let lib = Library::open_in_memory().unwrap();
    let e = expected();
    import_page(&lib);

    let id = e["noteTweetId"].as_str().unwrap();
    let post = read::get_post(lib.conn(), id).unwrap().unwrap();

    // `full_text` for this post is a 7-character stub; the real body lives in
    // note_tweet. Reading the wrong one truncates every long post silently.
    assert!(
        post.tweet.text.len() > 100,
        "expected the note_tweet body, got {:?}",
        post.tweet.text
    );
}

#[test]
fn a_visibility_wrapped_post_is_unwrapped_rather_than_lost() {
    let lib = Library::open_in_memory().unwrap();
    let e = expected();
    import_page(&lib);

    let id = e["visibilityWrappedId"].as_str().unwrap();
    let post = read::get_post(lib.conn(), id)
        .unwrap()
        .unwrap_or_else(|| panic!("{id} was wrapped in TweetWithVisibilityResults and dropped"));

    assert_eq!(post.tweet.id, id);
}

#[test]
fn the_raw_envelope_is_kept_once_for_the_whole_page() {
    // Storage rule from PRD §6.3: the envelope is stored once per captured page,
    // not duplicated into every post row.
    let lib = Library::open_in_memory().unwrap();
    let e = expected();
    import_page(&lib);

    let payloads: i64 = lib
        .conn()
        .query_row("SELECT COUNT(*) FROM capture_payloads", [], |r| r.get(0))
        .unwrap();
    assert_eq!(payloads, 1, "one page means one stored envelope");

    let posts = e["bookmarkCount"].as_u64().unwrap() as i64;
    assert!(posts > 1, "the fixture must have several posts for this to mean anything");
}

#[test]
fn re_importing_the_same_page_changes_nothing() {
    let lib = Library::open_in_memory().unwrap();
    let before = import_page(&lib).stats.new;
    let after = import_page(&lib);

    assert!(before > 0);
    assert_eq!(after.stats.new, 0, "a repeat capture must not duplicate");
    assert_eq!(lib.count_bookmarks().unwrap(), before as i64);
}
