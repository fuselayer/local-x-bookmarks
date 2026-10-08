//! Schema definition and migration.
//!
//! ## Migration mechanism
//!
//! `PRAGMA user_version` holds the applied migration index. Migrations are an
//! append-only array of SQL batches; each runs once, in a transaction, and the
//! version bumps. There is no down-migration and no migration table — for a
//! local single-file database owned by one user, the ability to roll *back* a
//! schema is worth less than the simplicity of not having it.
//!
//! ## Design rules this schema follows
//!
//! 1. **`raw_tweet` is the source of truth.** The parsed columns exist for
//!    indexing and filtering, never as the only copy. A parser fix must be
//!    able to re-derive everything from stored payloads without re-capturing
//!    (PRD §9.2).
//! 2. **The page envelope is stored once, not per post.** A 200-tweet
//!    bookmarks page duplicated into 200 rows wastes an order of magnitude of
//!    disk for nothing.
//! 3. **Flag removals, never `DELETE`.** The whole point is outliving X.
//! 4. **Re-import must be idempotent.** Every child table therefore has a
//!    natural unique key, so a second import of the same file is a no-op
//!    rather than a duplication.

/// Increment when `MIGRATIONS` gains an entry.
pub const SCHEMA_VERSION: i32 = 1;

/// Migration 1 — the full initial schema.
const M001_INITIAL: &str = r#"
-- ── content ──────────────────────────────────────────────────────────────

CREATE TABLE authors (
  id            TEXT PRIMARY KEY,
  handle        TEXT NOT NULL,
  name          TEXT NOT NULL,
  avatar_url    TEXT,
  avatar_hash   TEXT,                     -- content address of a cached avatar
  verified      INTEGER NOT NULL DEFAULT 0,
  blue_verified INTEGER NOT NULL DEFAULT 0,
  verified_type TEXT,
  fetched_at    INTEGER
);

CREATE TABLE posts (
  id              TEXT PRIMARY KEY,       -- snowflake
  author_id       TEXT REFERENCES authors(id),
  text            TEXT NOT NULL,          -- note_tweet text when present
  note_text       TEXT,
  is_long_form    INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER,                -- unix seconds, NULL if unparseable
  lang            TEXT,
  conversation_id TEXT,
  in_reply_to_id  TEXT,
  in_reply_to_handle TEXT,
  like_count      INTEGER NOT NULL DEFAULT 0,
  repost_count    INTEGER NOT NULL DEFAULT 0,
  reply_count     INTEGER NOT NULL DEFAULT 0,
  quote_count     INTEGER NOT NULL DEFAULT 0,
  bookmark_count  INTEGER NOT NULL DEFAULT 0,
  view_count      INTEGER,                -- NULL when hidden, not 0
  has_media       INTEGER NOT NULL DEFAULT 0,
  has_video       INTEGER NOT NULL DEFAULT 0,
  quoted_id       TEXT,
  card_kind       TEXT,
  card_domain     TEXT,
  parser_version  INTEGER NOT NULL DEFAULT 1,
  -- The tweet_results.result subtree ONLY. Never the page envelope.
  raw_tweet       TEXT,
  first_seen_at   INTEGER NOT NULL,
  last_seen_at    INTEGER,
  -- Flagged, never deleted: absence from a later capture cannot distinguish
  -- a deletion from an unbookmark or a filtered timeline.
  removed_at      INTEGER,
  source          TEXT NOT NULL           -- 'capture' | 'file_import'
);

CREATE INDEX posts_by_author  ON posts(author_id);
CREATE INDEX posts_by_created ON posts(created_at DESC);
CREATE INDEX posts_by_seen    ON posts(first_seen_at DESC);

-- The full GraphQL page envelope, stored ONCE per captured page.
-- Posts reference it; it is never duplicated into post rows.
CREATE TABLE capture_payloads (
  id           TEXT PRIMARY KEY,          -- 'sha256:<hex>' of the raw body
  op           TEXT,                      -- 'Bookmarks' | 'TweetDetail' | …
  captured_at  INTEGER NOT NULL,
  cursor_after TEXT,                      -- bottom cursor this page ended on
  bytes        INTEGER,
  tweet_count  INTEGER NOT NULL DEFAULT 0,
  raw          TEXT NOT NULL              -- the whole envelope, exactly once
);

CREATE TABLE post_payloads (              -- which page(s) a post was seen on
  post_id    TEXT NOT NULL REFERENCES posts(id),
  payload_id TEXT NOT NULL REFERENCES capture_payloads(id),
  PRIMARY KEY (post_id, payload_id)
);

-- ── bookmark state (user-specific) ───────────────────────────────────────

CREATE TABLE bookmarks (
  post_id       TEXT PRIMARY KEY REFERENCES posts(id),
  -- NULL for backfill capture, permanently. X exposes no bookmark timestamp
  -- anywhere in the payload; only continuous capture can fill this, having
  -- observed the save. UI labels it "saved (observed)" and never sorts by it.
  bookmarked_at INTEGER,
  sort_index    TEXT,                     -- entries[].sortIndex, entry level
  observed_at   INTEGER NOT NULL
);

CREATE INDEX bookmarks_by_sort ON bookmarks(sort_index DESC);

-- Many-to-many: a bookmark can live in several folders.
CREATE TABLE folders (
  id             TEXT PRIMARY KEY,
  name           TEXT NOT NULL,
  last_synced_at INTEGER
);

CREATE TABLE folder_posts (
  folder_id TEXT NOT NULL REFERENCES folders(id),
  post_id   TEXT NOT NULL REFERENCES posts(id),
  PRIMARY KEY (folder_id, post_id)
);
CREATE INDEX folder_posts_by_post ON folder_posts(post_id);

-- ── media & links ────────────────────────────────────────────────────────

CREATE TABLE media (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  post_id     TEXT NOT NULL REFERENCES posts(id),
  position    INTEGER NOT NULL DEFAULT 0,
  kind        TEXT NOT NULL,              -- 'photo' | 'video' | 'gif'
  url         TEXT NOT NULL,
  video_url   TEXT,
  alt_text    TEXT,
  width       INTEGER,
  height      INTEGER,
  duration_ms INTEGER,
  blob_hash   TEXT,                       -- sha256 once cached locally
  bytes       INTEGER,
  UNIQUE (post_id, url)                   -- makes re-import idempotent
);
CREATE INDEX media_by_post ON media(post_id);
CREATE INDEX media_by_hash ON media(blob_hash);

CREATE TABLE links (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  post_id    TEXT NOT NULL REFERENCES posts(id),
  tco        TEXT,
  expanded   TEXT,
  display    TEXT,
  domain     TEXT,
  title      TEXT,                        -- from the card, when present
  description TEXT,
  UNIQUE (post_id, tco)
);
CREATE INDEX links_by_post   ON links(post_id);
CREATE INDEX links_by_domain ON links(domain);

CREATE TABLE refs (
  post_id     TEXT NOT NULL REFERENCES posts(id),
  kind        TEXT NOT NULL,              -- 'quoted' | 'replied_to'
  ref_post_id TEXT,
  PRIMARY KEY (post_id, kind)
);
CREATE INDEX refs_by_ref ON refs(ref_post_id);

-- ── user-owned annotations ───────────────────────────────────────────────

CREATE TABLE tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE);
CREATE TABLE post_tags (
  post_id TEXT NOT NULL REFERENCES posts(id),
  tag_id  INTEGER NOT NULL REFERENCES tags(id),
  PRIMARY KEY (post_id, tag_id)
);
CREATE TABLE notes (
  post_id    TEXT PRIMARY KEY REFERENCES posts(id),
  md         TEXT,
  updated_at INTEGER
);

-- ── sync/ingest bookkeeping ──────────────────────────────────────────────

CREATE TABLE capture_runs (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at    INTEGER NOT NULL,
  finished_at   INTEGER,
  kind          TEXT NOT NULL,            -- 'passive' | 'assisted' | 'import' | 'live'
  records_seen  INTEGER NOT NULL DEFAULT 0,
  records_new   INTEGER NOT NULL DEFAULT 0,
  scrolls       INTEGER NOT NULL DEFAULT 0,
  minutes       REAL,
  last_cursor   TEXT,
  stopped_reason TEXT,
  note          TEXT
);

-- ── search ───────────────────────────────────────────────────────────────
--
-- External-content FTS5 over a flattened projection.
--
-- Why not contentless (content='')? Two hard blockers:
--   • highlight()/snippet() need stored text, and contentless tables have
--     none — match highlighting would be lost entirely.
--   • Contentless tables reject UPDATE/DELETE without contentless_delete=1,
--     which breaks us exactly when a note_tweet arrives later and a stub post
--     is enriched. Last-write-wins on a stub is a normal operation here.
--
-- Why not duplicate the text into two plain FTS tables? Also fine. At 10k
-- posts that is a few MB. External-content is chosen so there is one source
-- of truth for the triggers and rebuilds, not for disk.

CREATE TABLE search_docs (
  post_id       TEXT PRIMARY KEY REFERENCES posts(id),
  text          TEXT NOT NULL DEFAULT '',
  note_text     TEXT NOT NULL DEFAULT '',
  author_handle TEXT NOT NULL DEFAULT '',
  author_name   TEXT NOT NULL DEFAULT '',
  alt_text      TEXT NOT NULL DEFAULT '',
  link_titles   TEXT NOT NULL DEFAULT ''
);

CREATE VIRTUAL TABLE posts_fts_uni USING fts5(
  text, note_text, author_handle, author_name, alt_text, link_titles,
  content='search_docs', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);

-- `trigram` gives substring matching and CJK — NOT typo tolerance. Typos come
-- from the embedding index in M4. Both are retrievers over the same documents
-- and get fused with RRF, not an ad-hoc score blend.
CREATE VIRTUAL TABLE posts_fts_sub USING fts5(
  text, note_text, author_handle, author_name, alt_text, link_titles,
  content='search_docs', content_rowid='rowid',
  tokenize='trigram'
);

CREATE TRIGGER search_docs_ai AFTER INSERT ON search_docs BEGIN
  INSERT INTO posts_fts_uni(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
  INSERT INTO posts_fts_sub(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
END;

CREATE TRIGGER search_docs_ad AFTER DELETE ON search_docs BEGIN
  INSERT INTO posts_fts_uni(posts_fts_uni, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
  INSERT INTO posts_fts_sub(posts_fts_sub, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
END;

CREATE TRIGGER search_docs_au AFTER UPDATE ON search_docs BEGIN
  INSERT INTO posts_fts_uni(posts_fts_uni, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
  INSERT INTO posts_fts_uni(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
  INSERT INTO posts_fts_sub(posts_fts_sub, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
  INSERT INTO posts_fts_sub(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
END;

-- ── semantic (optional, added in M4) ─────────────────────────────────────
-- CREATE VIRTUAL TABLE post_vecs USING vec0(post_id TEXT PRIMARY KEY, embedding FLOAT[384]);
"#;

/// SQL batches, applied in order. Append only — never edit a shipped entry,
/// because a database in the wild has already run it.
pub const MIGRATIONS: &[&str] = &[M001_INITIAL];

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn migrations_apply_cleanly_and_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::configure(&conn).unwrap();

        let applied = crate::db::migrate(&mut conn).unwrap();
        assert_eq!(applied, MIGRATIONS.len());

        // Running again must be a no-op, not an error.
        let again = crate::db::migrate(&mut conn).unwrap();
        assert_eq!(again, 0, "second migrate() re-ran migrations");

        let v: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn the_schema_constant_matches_the_migration_list() {
        // Guards the easy mistake of appending a migration without bumping
        // SCHEMA_VERSION, which would leave the version pragma lying.
        assert_eq!(SCHEMA_VERSION as usize, MIGRATIONS.len());
    }
}
