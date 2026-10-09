/**
 * Tests for the capture userscript.
 *
 * ## Why this runs the real file
 *
 * The script is an IIFE with no exports, and that is deliberate — a userscript
 * you can read in one sitting should not grow an export surface for the benefit
 * of its tests. So this harness builds a stubbed browser, evaluates the actual
 * `xitter-dl-capture.user.js` inside it, and drives it through the public thing
 * it actually does: hooking `fetch`.
 *
 * That means these tests fail if the hooks stop being installed, if `clone()`
 * stops being called, or if the wire format drifts from the app's parser —
 * which is exactly the set of things worth protecting.
 *
 * Run:  node --test userscript/test/
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..', '..');

const SCRIPT = readFileSync(join(root, 'userscript', 'xitter-dl-capture.user.js'), 'utf8');
const PAGE = readFileSync(join(root, 'fixtures', 'bookmarks-page.json'), 'utf8');
const EXPECTED = JSON.parse(
  readFileSync(join(root, 'fixtures', 'bookmarks-page.expected.json'), 'utf8')
);

/** A URL for the fixture page, with a rotating doc_id that must not matter. */
const BOOKMARKS_URL = (hash) =>
  `https://x.com/i/api/graphql/${hash}/Bookmarks?variables=%7B%7D&features=%7B%7D`;

const settle = () => new Promise((r) => setTimeout(r, 20));

/**
 * Build a world containing the script and just enough browser to run it.
 *
 * `fetchImpl` is installed *before* the script evaluates, because the script
 * captures `window.fetch` at hook time — as it must, to run at document-start.
 */
function makeWorld({ fetchImpl, xhrImpl } = {}) {
  const values = new Map();
  const requests = [];

  const win = {
    crypto: {
      getRandomValues(a) {
        for (let i = 0; i < a.length; i++) a[i] = (i * 37 + 11) & 255;
        return a;
      },
    },
    // No real DOM. The script must survive that, because the pill is a
    // convenience and capture is the product.
    document: {
      documentElement: {},
      body: {},
      addEventListener() {},
      createElement() {
        throw new Error('this stub has no DOM');
      },
    },
    // The hooks are scoped to the bookmarks timeline, so this world has to say
    // it is on it. See `syncRouteHook` in the script, and the route tests in
    // pill.test.mjs for the other half of that behaviour.
    location: { pathname: '/i/bookmarks' },
    MutationObserver: class {
      observe() {}
      disconnect() {}
    },
    fetch: fetchImpl,
    XMLHttpRequest: function () {},
  };
  win.XMLHttpRequest.prototype = { open() {}, send() {} };

  const ctx = vm.createContext({
    unsafeWindow: win,
    GM_getValue: (k, d) => (values.has(k) ? values.get(k) : d),
    GM_setValue: (k, v) => void values.set(k, v),
    GM_xmlhttpRequest(opts) {
      requests.push({
        method: opts.method,
        url: opts.url,
        headers: opts.headers || {},
        data: opts.data,
      });
      setTimeout(() => {
        try {
          if (opts.url.includes('/v1/hello')) {
            const probe = new URL(opts.url).searchParams.get('probe');
            opts.onload({
              status: 200,
              responseText: JSON.stringify({ app: 'xitter-dl', v: 1, probe }),
            });
          } else if (opts.url.includes('/v1/capture')) {
            opts.onload({
              status: 200,
              responseText: JSON.stringify({ ok: true, seen: 1, new: 1, updated: 0 }),
            });
          } else if (opts.url.includes('/v1/pair')) {
            opts.onload({ status: 401, responseText: JSON.stringify({ error: 'nope' }) });
          } else {
            opts.onload({ status: 404, responseText: '{}' });
          }
        } catch (e) {
          opts.onerror && opts.onerror(e);
        }
      }, 0);
    },
    console: { info() {}, warn() {}, log() {}, error() {} },
    setTimeout,
    clearTimeout,
    setInterval: () => 0,
    clearInterval: () => {},
    URL,
  });

  vm.runInContext(SCRIPT, ctx);
  return { win, values, requests, ctx };
}

/** A stand-in for a fetch Response that records whether it was cloned. */
function fakeResponse(body, { ok = true } = {}) {
  const res = {
    ok,
    cloned: 0,
    clone() {
      res.cloned++;
      return { text: async () => body };
    },
  };
  return res;
}

function queued(values) {
  return (values.get('queue') || []).map((l) => JSON.parse(l));
}

// ── the wire format ──────────────────────────────────────────────────────────

test('captures a bookmarks page through the fetch hook', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();

  const lines = queued(world.values);
  assert.equal(lines.length, 1, 'exactly one page record should be queued');
  assert.equal(lines[0].kind, 'page');
  assert.equal(lines[0].op, EXPECTED.op);
  assert.equal(lines[0].v, 1);
  assert.equal(typeof lines[0].captured_at, 'number');
});

test("ships X's bytes unchanged, so a parser fix applies retroactively", async () => {
  // The script deliberately does NOT re-implement the envelope parser. The
  // app's Rust parser is the tested one, and storing the raw envelope is what
  // lets it be improved later without re-capturing anything.
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();

  const [line] = queued(world.values);
  assert.deepEqual(line.raw, JSON.parse(PAGE), 'the envelope must round-trip exactly');
});

test('clones the response before reading, so the page still gets its body', async () => {
  // The single most important line in the script. Reading the original would
  // break x.com, and a capture script that breaks the page is worse than none.
  const response = fakeResponse(PAGE);
  const world = makeWorld({ fetchImpl: async () => response });

  const got = await world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();

  assert.equal(response.cloned, 1, 'must clone exactly once');
  assert.equal(got, response, 'the page must still receive its own response object');
});

// ── operation matching ───────────────────────────────────────────────────────

test('survives doc_id rotation, because it matches on the operation name', async () => {
  // X rotates the hash every 2-4 weeks. Both hashes must be captured; a matcher
  // built on the hash would break on the second one.
  const second = PAGE.replace('1843712994563928064', '1843712994563929999');
  assert.notEqual(second, PAGE, 'the two pages must differ, or dedupe would hide the test');

  const bodies = [PAGE, second];
  let i = 0;
  const world = makeWorld({ fetchImpl: async () => fakeResponse(bodies[i++]) });

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();
  world.win.fetch(BOOKMARKS_URL('ZZZ999'));
  await settle();

  assert.equal(queued(world.values).length, 2, 'both hashes must be captured');
});

test('ignores operations that are not bookmarks', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });

  for (const op of ['HomeTimeline', 'TweetDetail', 'SearchTimeline', 'UserTweets']) {
    world.win.fetch(`https://x.com/i/api/graphql/HASH/${op}?variables=%7B%7D`);
  }
  // And traffic that is not GraphQL at all.
  world.win.fetch('https://x.com/i/api/1.1/dm/inbox.json');
  world.win.fetch('https://pbs.twimg.com/media/abc.jpg');
  await settle();

  assert.equal(queued(world.values).length, 0, 'nothing should have been captured');
});

test('captures every bookmark operation, not just the main timeline', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });

  for (const op of ['Bookmarks', 'BookmarkSearch', 'BookmarkFolderTimeline', 'BookmarkFoldersSlice']) {
    world.win.fetch(`https://x.com/i/api/graphql/HASH/${op}?variables=%7B%7D`);
  }
  await settle();

  assert.equal(queued(world.values).length, 1, 'same body, so deduped to one');
});

// ── robustness: the script must never break the page ─────────────────────────

test('a repeated page is not sent twice', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();
  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();

  assert.equal(queued(world.values).length, 1);
});

test('a malformed body queues nothing and throws nothing', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse('{"data": { truncated…') });

  await world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();

  assert.equal(queued(world.values).length, 0);
});

test('a body with no timeline instructions queues nothing', async () => {
  // An error envelope, a rate-limit notice, an empty response — all normal.
  const world = makeWorld({
    fetchImpl: async () => fakeResponse(JSON.stringify({ errors: [{ message: 'nope' }] })),
  });

  await world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();

  assert.equal(queued(world.values).length, 0);
});

test('a failed response is ignored rather than parsed', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse('{}', { ok: false }) });

  await world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();

  assert.equal(queued(world.values).length, 0);
});

test('a rejected fetch still returns a rejecting promise to the page', async () => {
  // Swallowing the rejection would change the page's control flow.
  const world = makeWorld({
    fetchImpl: async () => {
      throw new Error('network down');
    },
  });

  await assert.rejects(() => world.win.fetch(BOOKMARKS_URL('AAA111')));
});

test('an empty body is ignored', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse('') });
  await world.win.fetch(BOOKMARKS_URL('AAA111'));
  await settle();
  assert.equal(queued(world.values).length, 0);
});

// ── the loopback contract ────────────────────────────────────────────────────

test('every request it makes goes to loopback, and only loopback', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });
  // A secret means the flush path will actually run.
  world.values.set('secret', 'a'.repeat(64));

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  // The flush is deliberately delayed so a fast scroll coalesces.
  await new Promise((r) => setTimeout(r, 1400));

  assert.ok(world.requests.length >= 2, `expected a hello and a capture, got ${world.requests.length}`);
  for (const r of world.requests) {
    assert.match(
      r.url,
      /^http:\/\/127\.0\.0\.1:\d+\//,
      `the script must never contact anything but loopback, saw ${r.url}`
    );
  }
});

test('discovers the port by echo rather than by broadcasting the secret', async () => {
  // The app answers /v1/hello with the probe we invented. Without that echo the
  // script would have to send the install secret to every port in the range to
  // find the app, handing it to whatever happened to be squatting on the wrong
  // one.
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });
  const secret = 'b'.repeat(64);
  world.values.set('secret', secret);

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await new Promise((r) => setTimeout(r, 1400));

  const hello = world.requests.find((r) => r.url.includes('/v1/hello'));
  assert.ok(hello, 'should have probed /v1/hello');
  assert.equal(hello.method, 'GET');
  assert.doesNotMatch(hello.url, new RegExp(secret), 'the secret must not appear in a probe');

  const capture = world.requests.find((r) => r.url.includes('/v1/capture'));
  assert.ok(capture, 'should have posted the capture');
  assert.equal(capture.headers.Authorization, `Bearer ${secret}`);
  assert.equal(capture.method, 'POST');
});

test('the capture body is NDJSON, one record per line', async () => {
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });
  world.values.set('secret', 'c'.repeat(64));

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await new Promise((r) => setTimeout(r, 1400));

  const capture = world.requests.find((r) => r.url.includes('/v1/capture'));
  assert.ok(capture, 'expected a capture POST');
  assert.ok(capture.data.endsWith('\n'), 'NDJSON is newline-terminated');
  const lines = capture.data.trim().split('\n');
  assert.equal(lines.length, 1, 'one page is one record');
  assert.equal(JSON.parse(lines[0]).kind, 'page');
});

test('captures are buffered, not dropped, while the app is closed', async () => {
  // No secret and no app: the record must survive for a later flush.
  const world = makeWorld({ fetchImpl: async () => fakeResponse(PAGE) });

  world.win.fetch(BOOKMARKS_URL('AAA111'));
  await new Promise((r) => setTimeout(r, 1400));

  assert.equal(world.requests.length, 0, 'nothing should be sent without a secret');
  assert.equal(queued(world.values).length, 1, 'and it must still be queued');
});
