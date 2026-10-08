/**
 * Generate `fixtures/bookmarks-page.json` and its expectations.
 *
 * Run:  node scripts/make-fixture-bookmarks.mjs
 *
 * ## Why this is generated rather than hand-written
 *
 * Hand-computed fixture values have been wrong repeatedly in this project —
 * entity indices, timestamps, post counts. Every one of those was a number a
 * human typed and a parser disagreed with.
 *
 * So the shape is built here from a plain data array, and the *expectations*
 * are derived from that same array rather than from the emitted JSON. If
 * someone hand-edits the fixture, the two stop agreeing and both the Rust and
 * JavaScript tests fail — which is the point.
 *
 * The payload is synthetic. It is modelled on the documented shape (PRD §6.3)
 * and on what the Rust parser's own unit tests construct, and it contains no
 * real tweet, account or capture.
 */

import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

/** Deterministic, so regenerating never produces a spurious diff. */
const CREATED = '2026-10-08T01:12:44.000Z';

function user(id, handle, name) {
  return {
    __typename: 'User',
    rest_id: id,
    core: { name, screen_name: handle, created_at: CREATED },
    legacy: { name, screen_name: handle, verified: false },
  };
}

function tweet(id, text, userId, handle, name, extra = {}) {
  return {
    __typename: 'Tweet',
    rest_id: id,
    core: { user_results: { result: user(userId, handle, name) } },
    legacy: {
      id_str: id,
      full_text: text,
      created_at: 'Wed Oct 08 01:12:44 +0000 2026',
      lang: 'en',
      conversation_id_str: id,
      entities: { urls: [], user_mentions: [], hashtags: [], symbols: [] },
      ...(extra.legacy || {}),
    },
    ...(extra.top || {}),
  };
}

// ── the source of truth ──────────────────────────────────────────────────────
//
// `sortIndex` is attached here at the ENTRY level, because that is where X puts
// it. Reading `legacy.sort_index` yields null on every record, forever.

const POSTS = [
  { id: '1843712994563928064', sortIndex: '100', text: 'A plain bookmarked post.', kind: 'tweet' },
  { id: '1843712994563928065', sortIndex: '99', text: 'A visibility-limited post.', kind: 'visibility' },
  { id: '1843712994563928066', sortIndex: '98', text: 'Short. ', kind: 'note' },
  { id: '1843712994563928067', sortIndex: null, text: 'Inside a conversation module.', kind: 'module' },
];

const TOP_CURSOR = 'TOP_CURSOR_SHOULD_BE_IGNORED';
const BOTTOM_CURSOR = 'BOTTOM_CURSOR_IS_THE_PAGINATION_HANDLE';

function buildTweet(post, i) {
  const base = tweet(
    post.id,
    post.text,
    String(900 + i),
    `author${i}`,
    `Author ${i}`
  );
  if (post.kind === 'visibility') {
    // X wraps some tweets; the real one is one level down under `.tweet`.
    return { __typename: 'TweetWithVisibilityResults', tweet: base, limitedActionResults: {} };
  }
  if (post.kind === 'note') {
    // Long-form: `full_text` is the truncated version, `note_tweet` is the real
    // text. A parser that reads full_text silently truncates every long post.
    return {
      ...base,
      note_tweet: {
        note_tweet_results: {
          result: {
            id: post.id,
            text: 'This is the real, untruncated body of a long-form post. '.repeat(8).trim(),
            entity_set: { urls: [] },
          },
        },
      },
    };
  }
  return base;
}

function itemEntry(post, i) {
  return {
    entryId: `tweet-${post.id}`,
    sortIndex: post.sortIndex,
    content: {
      entryType: 'TimelineTimelineItem',
      __typename: 'TimelineTimelineItem',
      itemContent: {
        itemType: 'TimelineTweet',
        __typename: 'TimelineTweet',
        tweet_results: { result: buildTweet(post, i) },
      },
    },
  };
}

function moduleEntry(post, i) {
  return {
    entryId: `module-${post.id}`,
    sortIndex: post.sortIndex,
    content: {
      entryType: 'TimelineTimelineModule',
      __typename: 'TimelineTimelineModule',
      items: [
        {
          entryId: `module-item-${post.id}`,
          item: {
            itemContent: {
              itemType: 'TimelineTweet',
              tweet_results: { result: buildTweet(post, i) },
            },
          },
        },
      ],
    },
  };
}

function cursorEntry(cursorType, value, sortIndex) {
  return {
    entryId: `cursor-${cursorType.toLowerCase()}`,
    sortIndex,
    content: {
      entryType: 'TimelineTimelineCursor',
      __typename: 'TimelineTimelineCursor',
      cursorType,
      value,
    },
  };
}

const entries = [];
POSTS.forEach((post, i) => {
  entries.push(post.kind === 'module' ? moduleEntry(post, i) : itemEntry(post, i));
});
// Top cursor first, bottom cursor last — as X orders them.
entries.push(cursorEntry('Top', TOP_CURSOR, '101'));
entries.push(cursorEntry('Bottom', BOTTOM_CURSOR, '1'));

const payload = {
  data: {
    bookmark_timeline_v2: {
      timeline: {
        instructions: [
          { type: 'TimelineClearCache' },
          { type: 'TimelineAddEntries', entries },
        ],
        responseObjects: { feedbackActions: [] },
      },
    },
  },
};

// ── expectations, derived from POSTS rather than from `payload` ──────────────

const expected = {
  note: 'Generated by scripts/make-fixture-bookmarks.mjs — do not hand-edit.',
  op: 'Bookmarks',
  // Every post contributes a bookmark, including the one inside a conversation
  // module. That is a deliberate deviation: the module tweet is captured with a
  // null sort_index rather than skipped, so it is never lost.
  bookmarkCount: POSTS.length,
  orderedIds: POSTS.filter((p) => p.sortIndex !== null).map((p) => p.id),
  sortIndexes: Object.fromEntries(
    POSTS.filter((p) => p.sortIndex !== null).map((p) => [p.id, p.sortIndex])
  ),
  unsortedIds: POSTS.filter((p) => p.sortIndex === null).map((p) => p.id),
  topCursor: TOP_CURSOR,
  bottomCursor: BOTTOM_CURSOR,
  noteTweetId: POSTS.find((p) => p.kind === 'note').id,
  visibilityWrappedId: POSTS.find((p) => p.kind === 'visibility').id,
};

mkdirSync(join(root, 'fixtures'), { recursive: true });
writeFileSync(join(root, 'fixtures', 'bookmarks-page.json'), JSON.stringify(payload, null, 2) + '\n');
writeFileSync(
  join(root, 'fixtures', 'bookmarks-page.expected.json'),
  JSON.stringify(expected, null, 2) + '\n'
);

console.log('wrote fixtures/bookmarks-page.json');
console.log('wrote fixtures/bookmarks-page.expected.json');
console.log(`  ${expected.bookmarkCount} bookmarks, ${expected.orderedIds.length} with a sortIndex`);
console.log(`  bottom cursor: ${expected.bottomCursor}`);
