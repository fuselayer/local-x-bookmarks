/**
 * End-to-end smoke test for the capture bridge.
 *
 * Assumes `xdl bridge` is already running and holds the pairing code it printed.
 *
 *   xdl --db .scratch/e2e.sqlite bridge          # in one terminal
 *   node scripts/smoke-bridge.mjs NYJ9-MVWJ      # in another
 *
 * ## Why this exists alongside the Rust tests
 *
 * `crates/core/src/bridge/server.rs` already covers each endpoint against a
 * real socket. This covers something those cannot: the *whole* path in one
 * process pair — pair, capture a real generated page, and confirm the bytes
 * land in the database — using a stock HTTP client rather than the Rust one.
 *
 * It is also the thing to run when someone says "my script will not connect",
 * because it fails loudly with the status and body of whichever step broke.
 */

import { readFileSync } from 'node:fs';
import http from 'node:http';

const code = process.argv[2];
const PORT = Number(process.env.XDL_PORT || 8737);

if (!code) {
  console.error('usage: node scripts/smoke-bridge.mjs <PAIRING-CODE>');
  process.exit(2);
}

function request({ method, path, body, headers = {} }) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: '127.0.0.1', port: PORT, method, path, headers },
      (res) => {
        let data = '';
        res.setEncoding('utf8');
        res.on('data', (c) => (data += c));
        res.on('end', () => resolve({ status: res.statusCode, body: data }));
      }
    );
    req.on('error', reject);
    if (body !== undefined) req.write(body);
    req.end();
  });
}

function json(res) {
  try {
    return JSON.parse(res.body);
  } catch {
    return null;
  }
}

let failures = 0;
function check(label, ok, detail = '') {
  console.log(`${ok ? '  ok  ' : ' FAIL '} ${label}${detail ? '  — ' + detail : ''}`);
  if (!ok) failures++;
}

// ── 1. the unauthenticated echo ──────────────────────────────────────────────

const probe = 'a1b2c3d4e5f60718';
const hello = await request({ method: 'GET', path: `/v1/hello?probe=${probe}` });
const h = json(hello);
check('hello answers 200', hello.status === 200, `got ${hello.status}`);
check('hello identifies the app', h?.app === 'xitter-dl');
check('hello echoes our own probe', h?.probe === probe, `got ${JSON.stringify(h?.probe)}`);
check(
  'hello discloses nothing we did not send',
  !/secret|version|path|cursor/i.test(hello.body),
  hello.body
);

// ── 2. the endpoint refuses anything unauthenticated ─────────────────────────

const noAuth = await request({
  method: 'POST',
  path: '/v1/capture',
  body: '{}',
  headers: { 'Content-Type': 'application/x-ndjson' },
});
check('capture without a secret is 401', noAuth.status === 401, `got ${noAuth.status}`);

const wrongAuth = await request({
  method: 'POST',
  path: '/v1/capture',
  body: '{}',
  headers: { Authorization: 'Bearer ' + '0'.repeat(64) },
});
check('capture with a wrong secret is 401', wrongAuth.status === 401, `got ${wrongAuth.status}`);

// ── 3. pair ──────────────────────────────────────────────────────────────────

const pair = await request({
  method: 'POST',
  path: '/v1/pair',
  body: JSON.stringify({ code, label: 'smoke test' }),
  headers: { 'Content-Type': 'application/json' },
});
const p = json(pair);
check('pairing succeeds', pair.status === 200, `${pair.status} ${pair.body}`);
check('pairing returns a 256-bit secret', typeof p?.secret === 'string' && p.secret.length === 64);
check('pairing reports the port', p?.port === PORT, `got ${p?.port}`);

const secret = p?.secret;
if (!secret) {
  console.error('\ncannot continue without a secret');
  process.exit(1);
}
const auth = { Authorization: 'Bearer ' + secret, 'Content-Type': 'application/x-ndjson' };

// ── 4. capture a real generated page ─────────────────────────────────────────

const raw = JSON.parse(readFileSync(new URL('../fixtures/bookmarks-page.json', import.meta.url), 'utf8'));
const expected = JSON.parse(
  readFileSync(new URL('../fixtures/bookmarks-page.expected.json', import.meta.url), 'utf8')
);

// JSON.stringify never emits a newline, which is what makes this one NDJSON
// record rather than a few hundred unparseable fragments.
const line = JSON.stringify({
  v: 1,
  kind: 'page',
  captured_at: Math.floor(Date.now() / 1000),
  op: expected.op,
  raw,
});

const cap = await request({ method: 'POST', path: '/v1/capture', body: line + '\n', headers: auth });
const c = json(cap);
check('capture is accepted', cap.status === 200, `${cap.status} ${cap.body}`);
// Accounted for, not necessarily new: running this twice against the same
// library legitimately reports the second pass as updates.
check(
  `capture accounts for all ${expected.bookmarkCount} bookmarks`,
  (c?.new ?? 0) + (c?.updated ?? 0) === expected.bookmarkCount,
  `new=${c?.new} updated=${c?.updated} — a fresh library reports new=${expected.bookmarkCount}`
);
check('capture reports no problems', (c?.problems || []).length === 0, JSON.stringify(c?.problems));

// ── 5. replay is idempotent ──────────────────────────────────────────────────

const again = await request({ method: 'POST', path: '/v1/capture', body: line + '\n', headers: auth });
const a = json(again);
check('replaying the same page adds nothing', a?.new === 0, `got new=${a?.new}`);
check(
  'replaying the same page is an update, not a duplicate',
  a?.updated === expected.bookmarkCount,
  `got updated=${a?.updated}`
);

// ── 6. framing ───────────────────────────────────────────────────────────────
//
// Chunked is *accepted*, and this check exists because it once was not. Node's
// http.request switches to chunked the moment Content-Length is omitted, as
// does any browser streaming a body, so refusing it made captures fail with a
// 400 that nothing on the script side could explain.
//
// The genuinely ambiguous case — a request declaring its length both ways — is
// refused, and is covered exhaustively in the Rust unit tests where a raw
// socket is available.

const chunkedBody = JSON.stringify({
  v: 1,
  kind: 'page',
  captured_at: Math.floor(Date.now() / 1000),
  op: expected.op,
  raw,
});

const chunked = await request({
  method: 'POST',
  path: '/v1/capture',
  body: chunkedBody + '\n',
  headers: { ...auth, 'Transfer-Encoding': 'chunked' },
});
check('a chunked capture is accepted', chunked.status === 200, `got ${chunked.status} ${chunked.body}`);

console.log(failures === 0 ? '\nall checks passed' : `\n${failures} check(s) FAILED`);
process.exit(failures === 0 ? 0 : 1);
