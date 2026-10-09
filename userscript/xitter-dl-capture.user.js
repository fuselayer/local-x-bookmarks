// ==UserScript==
// @name         xitter-dl capture
// @namespace    https://github.com/example/xitter-dl
// @version      0.1.0
// @description  Files away the bookmarks X already sent your browser. No API, no extra requests.
// @author       xitter-dl
// @match        https://x.com/*
// @match        https://twitter.com/*
// @run-at       document-start
// @grant        GM_xmlhttpRequest
// @grant        GM_setValue
// @grant        GM_getValue
// @grant        unsafeWindow
// @connect      127.0.0.1
// @connect      localhost
// ==/UserScript==

/*
 * WHAT THIS SCRIPT DOES, AND WHAT IT REFUSES TO DO
 * ================================================
 *
 * It wraps `fetch` and `XMLHttpRequest` before X's own bundle loads, and reads
 * the responses the page was *already* going to receive. It originates no
 * requests to X. Not one. Your scrolling triggers the pagination fetches it
 * always would have; this script reads the answers on the way past.
 *
 * That distinction is the whole design, so it is worth being precise about:
 * passive capture adds ZERO additional requests, which is not the same claim as
 * zero risk. The residual risk is only whatever your own browsing already
 * carried.
 *
 * It therefore will not, and must not ever:
 *   - construct or replay a GraphQL request, or use a doc_id for anything
 *   - touch cookies, auth_token, ct0, or any credential
 *   - click, scroll, like, follow, or synthesise input on your behalf
 *   - read or write the DOM of the timeline
 *
 * The only network calls it makes are to 127.0.0.1, to hand captured bytes to
 * the desktop app.
 *
 * TWO NON-NEGOTIABLES
 * -------------------
 * Always `response.clone()` before reading, and never let a parse error escape.
 * A capture script that breaks the bookmarks page is worse than no script at
 * all. Every entry point below is wrapped, and the fallback is always "do
 * nothing and let the page work".
 */

(function () {
  'use strict';

  // ── configuration ──────────────────────────────────────────────────────────

  const CFG = {
    scriptVersion: '0.1.0',
    wireVersion: 1,

    // Matched on the operation NAME, never on the rotating doc_id hash. This
    // regex is the single highest-leverage line in the file: X rotates hashes
    // every 2-4 weeks, and a matcher built on them breaks every time.
    opPattern: /^(Bookmarks|BookmarkSearch|BookmarkFolderTimeline|BookmarkFoldersSlice)$/,

    portFirst: 8737,
    portLast: 8757,

    // Records per POST. A page is one record, so this is "pages".
    batchMax: 25,

    // How long to wait after a capture before flushing, so a fast scroll
    // coalesces into one request instead of one per page.
    flushDelayMs: 1200,

    // Refuse absurd bodies rather than hashing them. A bookmarks page is a few
    // hundred kB; this is a sanity bound, not a real limit.
    maxBodyChars: 12 * 1024 * 1024,

    // In-memory only. The app dedupes by content hash anyway, so this exists to
    // save bandwidth, not to guarantee correctness.
    dedupeMemory: 200,

    uiAnchorId: 'xdl-capture-pill',

    // Set this to false to load the script without patching anything the page
    // owns. It exists because "x.com will not load" has two candidate causes
    // that look identical from the outside — the pill UI, and the network
    // hooks — and this separates them in one reload. With it false you should
    // still see the pill; if x.com loads here and not with it true, the hooks
    // are the problem and the UI is exonerated.
    hookNetwork: true,
  };

  const W = typeof unsafeWindow !== 'undefined' ? unsafeWindow : window;

  /**
   * Hand a function to page code in a form page code can call.
   *
   * A userscript with any `@grant` runs in its own compartment. Assigning a
   * function from that compartment onto `window.fetch` works in Chrome, but on
   * Firefox the page's Xray vision sees a foreign function and rejects the
   * call — which takes x.com down with it, because every request it makes goes
   * through the wrapper we installed.
   *
   * `exportFunction` is Firefox's supported way to clone a function into the
   * target compartment. Chrome has no equivalent and needs none, so this is a
   * no-op there. Failing to export is not fatal: it degrades to the previous
   * behaviour rather than refusing to start.
   */
  function intoPage(fn) {
    try {
      return typeof exportFunction === 'function' ? exportFunction(fn, W) : fn;
    } catch (_) {
      return fn;
    }
  }

  // ── storage (GM_* with a graceful fallback) ────────────────────────────────
  //
  // Wrapped because some managers expose GM_getValue as sync and some as a
  // promise, and because a manager that has them disabled should degrade to
  // "captures nothing, breaks nothing" rather than throwing.

  const store = {
    get(key, fallback) {
      try {
        const v = GM_getValue(key, fallback);
        return v === undefined ? fallback : v;
      } catch (_) {
        return fallback;
      }
    },
    set(key, value) {
      try {
        GM_setValue(key, value);
      } catch (_) {
        /* not fatal: this session just will not survive a reload */
      }
    },
  };

  // ── logging ────────────────────────────────────────────────────────────────
  //
  // Off unless the user turns it on, because a console full of noise on x.com
  // is indistinguishable from the script being broken.

  const log = {
    on: false,
    info(...a) {
      if (this.on) console.info('[xitter-dl]', ...a);
    },
    warn(...a) {
      if (this.on) console.warn('[xitter-dl]', ...a);
    },
  };

  /**
   * Report something that means the script is not working, whether or not
   * logging is on. Deduplicated by key, because the pill attaches from a poll
   * and a repeating failure would otherwise flood the console.
   *
   * The gated `log` above is right for capture chatter and wrong for this: a
   * boot failure that only prints when you already suspect a boot failure is
   * not a diagnostic, it is a secret.
   */
  const loudly = (() => {
    const seen = new Set();
    return function (key, ...rest) {
      if (seen.has(key)) return;
      seen.add(key);
      try {
        console.error('[xitter-dl]', key, ...rest);
      } catch (_) {}
    };
  })();

  // ── the capture queue ──────────────────────────────────────────────────────
  //
  // Persisted, so closing the app (or the browser) mid-scroll loses nothing.
  // Records are NDJSON lines, which is exactly the wire format, so the queue is
  // appended to and shipped without a re-encode.

  const queue = {
    lines: store.get('queue', []),
    seen: new Set(store.get('recentHashes', [])),
    seenOrder: store.get('recentHashes', []),

    push(line) {
      this.lines.push(line);
      // Bound the persisted queue. A user who scrolls 5000 bookmarks with the
      // app closed should not build a 200 MB GM value; the cap is generous and
      // the overflow is surfaced rather than silently dropped.
      if (this.lines.length > 4000) this.lines.splice(0, this.lines.length - 4000);
      store.set('queue', this.lines);
    },

    take(n) {
      return this.lines.slice(0, n);
    },

    drop(n) {
      this.lines.splice(0, n);
      store.set('queue', this.lines);
    },

    size() {
      return this.lines.length;
    },

    /** True the first time this body is seen; false for a repeat. */
    firstSight(hash) {
      if (this.seen.has(hash)) return false;
      this.seen.add(hash);
      this.seenOrder.push(hash);
      while (this.seenOrder.length > CFG.dedupeMemory) {
        this.seen.delete(this.seenOrder.shift());
      }
      store.set('recentHashes', this.seenOrder);
      return true;
    },
  };

  // ── session counters, for the pill ─────────────────────────────────────────

  const session = {
    pages: 0,
    posts: 0,
    refused: 0,
  };

  // ── URL and body inspection (pure) ─────────────────────────────────────────

  function urlOf(input) {
    try {
      if (typeof input === 'string') return input;
      if (input instanceof URL) return input.href;
      if (input && typeof input.url === 'string') return input.url;
    } catch (_) {
      /* fall through */
    }
    return '';
  }

  /**
   * The operation name from a GraphQL URL: the last path segment, minus query.
   * e.g. /i/api/graphql/<rotating-hash>/Bookmarks?variables=… -> "Bookmarks"
   */
  function operationName(url) {
    if (!url || url.indexOf('/i/api/graphql/') === -1) return null;
    const path = url.split('?')[0].split('#')[0];
    const seg = path.split('/').pop();
    return seg || null;
  }

  function isBookmarkOperation(url) {
    const op = operationName(url);
    return op !== null && CFG.opPattern.test(op);
  }

  /**
   * FNV-1a over the body, combined with its length.
   *
   * Not a security hash — this only needs to notice "same page fetched twice",
   * and the app does its own content addressing with SHA-256.
   */
  function bodyHash(text) {
    let h = 0x811c9dc5;
    for (let i = 0; i < text.length; i++) {
      h ^= text.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    return text.length.toString(16) + ':' + (h >>> 0).toString(16);
  }

  /**
   * Walk the payload for `instructions` arrays and count the posts in them.
   *
   * Deliberately shallow in what it extracts. The script does NOT re-implement
   * the envelope parser: the app's Rust parser is the tested one, and shipping
   * X's bytes unchanged means a parser fix applies retroactively to everything
   * already captured. All this needs is enough for a progress counter and the
   * bottom cursor.
   */
  function peek(parsed, budget) {
    const out = { posts: 0, bottomCursor: null, sawTimeline: false };
    const walk = (node, depth) => {
      if (depth > 24 || node === null || typeof node !== 'object') return;
      if (Array.isArray(node)) {
        for (let i = 0; i < node.length; i++) walk(node[i], depth + 1);
        return;
      }
      if (Array.isArray(node.instructions)) {
        for (const inst of node.instructions) {
          if (!inst || inst.type !== 'TimelineAddEntries' || !Array.isArray(inst.entries)) continue;
          out.sawTimeline = true;
          for (const entry of inst.entries) {
            const content = entry && entry.content;
            if (!content) continue;
            if (content.entryType === 'TimelineTimelineItem') {
              const r =
                content.itemContent &&
                content.itemContent.tweet_results &&
                content.itemContent.tweet_results.result;
              // TweetWithVisibilityResults wraps the real tweet one level down.
              const t = r && r.__typename === 'TweetWithVisibilityResults' ? r.tweet : r;
              if (t && t.rest_id) out.posts++;
            } else if (content.entryType === 'TimelineTimelineCursor') {
              // Capture the BOTTOM cursor only. It is the pagination handle and
              // the honest progress signal; the top cursor is noise.
              if (content.cursorType === 'Bottom') out.bottomCursor = content.value || null;
            }
            // TimelineTimelineModule (conversation modules) are skipped, not
            // fatal — the app's parser handles them properly.
          }
        }
      }
      for (const k in node) {
        if (k === 'instructions') continue;
        walk(node[k], depth + 1);
      }
    };
    try {
      walk(parsed, 0);
    } catch (_) {
      /* a shape we did not expect is not a reason to fail the capture */
    }
    return out;
  }

  // ── capture: the hot path ──────────────────────────────────────────────────

  function onBody(url, text, via) {
    try {
      if (typeof text !== 'string' || text.length === 0) return;
      if (text.length > CFG.maxBodyChars) {
        log.warn('body too large, skipped', text.length);
        return;
      }

      const hash = bodyHash(text);
      if (!queue.firstSight(hash)) {
        log.info('repeat page, ignored', hash);
        return;
      }

      let parsed;
      try {
        parsed = JSON.parse(text);
      } catch (_) {
        // A partial or non-JSON body is normal (aborted navigation, error
        // page). Never surfaced, never fatal.
        return;
      }

      const info = peek(parsed, 0);
      if (!info.sawTimeline) {
        log.info('no timeline instructions, ignored');
        return;
      }

      const op = operationName(url) || 'Bookmarks';
      const line = JSON.stringify({
        v: CFG.wireVersion,
        kind: 'page',
        captured_at: Math.floor(Date.now() / 1000),
        op: op,
        via: via,
        raw: parsed,
      });

      queue.push(line);
      session.pages++;
      session.posts += info.posts;
      render();

      log.info('captured', op, info.posts, 'posts, cursor', info.bottomCursor);
      scheduleFlush();
    } catch (e) {
      // The non-negotiable: a capture bug must never reach the page.
      log.warn('capture failed', e);
    }
  }

  // ── interception ───────────────────────────────────────────────────────────

  function hookFetch() {
    try {
      const original = W.fetch;
      if (typeof original !== 'function') return;

      // `intoPage` matters more here than anywhere else: if the page cannot
      // call this, x.com cannot make a single request.
      W.fetch = intoPage(function (input, init) {
        const promise = original.apply(this, arguments);
        try {
          const url = urlOf(input);
          if (isBookmarkOperation(url)) {
            promise
              .then(function (response) {
                try {
                  if (!response || !response.ok) return;
                  // clone() FIRST, so the page's own consumer still gets a
                  // readable body. Reading the original would break x.com.
                  const copy = response.clone();
                  copy
                    .text()
                    .then(function (t) {
                      onBody(url, t, 'fetch');
                    })
                    .catch(function () {});
                } catch (_) {}
              })
              .catch(function () {});
          }
        } catch (_) {}
        return promise;
      });
    } catch (e) {
      log.warn('could not hook fetch', e);
    }
  }

  function hookXhr() {
    try {
      const proto = W.XMLHttpRequest && W.XMLHttpRequest.prototype;
      if (!proto) return;

      const originalOpen = proto.open;
      const originalSend = proto.send;

      proto.open = intoPage(function (method, url) {
        try {
          this.__xdlUrl = url;
        } catch (_) {}
        return originalOpen.apply(this, arguments);
      });

      proto.send = intoPage(function () {
        try {
          const self = this;
          self.addEventListener('load', function () {
            try {
              const url = self.__xdlUrl || '';
              if (!isBookmarkOperation(url)) return;
              // responseText throws for arraybuffer/blob response types.
              const type = self.responseType;
              if (type && type !== 'text' && type !== 'json') return;
              const text =
                type === 'json' ? JSON.stringify(self.response) : self.responseText;
              onBody(url, text, 'xhr');
            } catch (_) {}
          });
        } catch (_) {}
        return originalSend.apply(this, arguments);
      });
    } catch (e) {
      log.warn('could not hook XMLHttpRequest', e);
    }
  }

  // ── transport: loopback only ───────────────────────────────────────────────

  function gmRequest(opts) {
    return new Promise(function (resolve, reject) {
      try {
        GM_xmlhttpRequest({
          method: opts.method,
          url: opts.url,
          data: opts.data,
          headers: opts.headers || {},
          timeout: opts.timeout || 15000,
          onload: resolve,
          onerror: function () {
            reject(new Error('loopback request failed'));
          },
          ontimeout: function () {
            reject(new Error('loopback request timed out'));
          },
        });
      } catch (e) {
        reject(e);
      }
    });
  }

  function randomHex(bytes) {
    const a = new Uint8Array(bytes);
    try {
      (W.crypto || window.crypto).getRandomValues(a);
    } catch (_) {
      for (let i = 0; i < a.length; i++) a[i] = Math.floor(Math.random() * 256);
    }
    return Array.prototype.map.call(a, (b) => b.toString(16).padStart(2, '0')).join('');
  }

  function portCandidates() {
    const cached = store.get('port', null);
    const all = [];
    for (let p = CFG.portFirst; p <= CFG.portLast; p++) all.push(p);
    if (cached && all.indexOf(cached) !== -1) {
      all.splice(all.indexOf(cached), 1);
      all.unshift(cached);
    }
    return all;
  }

  /**
   * Find the app and prove it is the app.
   *
   * The app answers `/v1/hello` with the `probe` value we just invented. That
   * echo is the whole point: without it we would have to send the install
   * secret to every port in the range to discover which one is listening, and
   * whichever unrelated process happened to be squatting on the wrong port
   * would receive it. The app discloses nothing here that we did not supply.
   */
  async function findPort() {
    for (const port of portCandidates()) {
      try {
        const probe = randomHex(8);
        const r = await gmRequest({
          method: 'GET',
          url: 'http://127.0.0.1:' + port + '/v1/hello?probe=' + probe,
          timeout: 2500,
        });
        if (r.status !== 200) continue;
        const j = JSON.parse(r.responseText);
        if (j && j.app === 'xitter-dl' && j.probe === probe) {
          if (store.get('port', null) !== port) store.set('port', port);
          return port;
        }
      } catch (_) {
        /* next port */
      }
    }
    return null;
  }

  let flushTimer = null;
  let flushing = false;

  function scheduleFlush() {
    if (flushTimer) return;
    flushTimer = setTimeout(function () {
      flushTimer = null;
      flush();
    }, CFG.flushDelayMs);
  }

  async function flush() {
    if (flushing || queue.size() === 0) return;
    const secret = store.get('secret', null);
    if (!secret) {
      // Buffered until the user pairs. Nothing is lost.
      return;
    }

    flushing = true;
    try {
      const port = await findPort();
      if (!port) {
        setStatus('app-not-running');
        return;
      }

      while (queue.size() > 0) {
        const batch = queue.take(CFG.batchMax);
        const body = batch.join('\n') + '\n';
        let r;
        try {
          r = await gmRequest({
            method: 'POST',
            url: 'http://127.0.0.1:' + port + '/v1/capture',
            data: body,
            headers: {
              Authorization: 'Bearer ' + secret,
              'Content-Type': 'application/x-ndjson',
            },
          });
        } catch (e) {
          setStatus('app-not-running');
          return; // keep the batch queued
        }

        if (r.status === 200) {
          queue.drop(batch.length);
          let report = {};
          try {
            report = JSON.parse(r.responseText);
          } catch (_) {}
          setStatus('ok', report);
        } else if (r.status === 401) {
          // The secret was revoked, or belongs to a previous install.
          store.set('secret', null);
          setStatus('needs-pairing');
          render();
          return;
        } else {
          setStatus('error', { error: 'HTTP ' + r.status });
          return; // keep the batch queued and retry later
        }
      }
    } finally {
      flushing = false;
    }
  }

  // ── pairing ────────────────────────────────────────────────────────────────

  async function redeem(code) {
    const port = await findPort();
    if (!port) throw new Error('The xitter-dl app is not running.');

    const r = await gmRequest({
      method: 'POST',
      url: 'http://127.0.0.1:' + port + '/v1/pair',
      data: JSON.stringify({ code: code, label: 'userscript ' + CFG.scriptVersion }),
      headers: { 'Content-Type': 'application/json' },
    });

    let j = {};
    try {
      j = JSON.parse(r.responseText);
    } catch (_) {}

    if (r.status !== 200 || !j.secret) {
      throw new Error(j.error || 'That code was not accepted.');
    }

    store.set('secret', j.secret);
    if (j.port) store.set('port', j.port);
    setStatus('ok');
    render();
    flush();
    return j;
  }

  // ── the pill ───────────────────────────────────────────────────────────────
  //
  // Small, bottom-right, and inert unless clicked. Styled in a shadow root so
  // X's CSS reset cannot reach it and ours cannot leak out.

  let shadow = null;
  let pillEl = null;
  let panelEl = null;
  let statusText = 'starting';
  let statusKind = 'idle';

  function setStatus(kind, detail) {
    statusKind = kind;
    statusText =
      kind === 'ok'
        ? 'connected'
        : kind === 'needs-pairing'
        ? 'not paired'
        : kind === 'app-not-running'
        ? 'app not running'
        : kind === 'error'
        ? (detail && detail.error) || 'error'
        : kind;
    render();
  }

  // Body specifically, never `documentElement`.
  //
  // At `@run-at document-start` the parser has not created `<body>` yet, and
  // falling back to `documentElement` meant the pill got appended directly to
  // `<html>` — outside the body box, in a position the HTML parser never
  // produces and X's own bundle has no reason to preserve. Waiting for `body`
  // costs nothing and puts the pill where every other element on the page
  // lives.
  function host() {
    try {
      return (W.document && W.document.body) || null;
    } catch (_) {
      return null;
    }
  }

  function ensureUi() {
    if (pillEl && pillEl.isConnected) return true;
    const parent = host();
    if (!parent) return false;

    try {
      const wrap = W.document.createElement('div');
      wrap.id = CFG.uiAnchorId;
      // The anchor itself is zero-size; all visuals live inside the shadow root.
      wrap.style.cssText = 'all:initial;position:fixed;z-index:2147483647;';
      shadow = wrap.attachShadow({ mode: 'open' });

      const style = W.document.createElement('style');
      style.textContent = `
        :host { all: initial; }
        * { box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
        .pill {
          position: fixed; right: 16px; bottom: 16px;
          display: flex; align-items: center; gap: 8px;
          height: 34px; padding: 0 12px; border-radius: 17px;
          background: rgba(15,20,25,.92); color: #e7e9ea;
          border: 1px solid rgba(255,255,255,.14);
          font-size: 12px; font-weight: 600; letter-spacing: .01em;
          cursor: pointer; user-select: none;
        }
        .pill:hover { background: rgba(15,20,25,1); }
        .dot { width: 7px; height: 7px; border-radius: 50%; background: #71767b; flex: none; }
        .dot.ok { background: #00ba7c; }
        .dot.warn { background: #ffd400; }
        .dot.bad { background: #f4212e; }
        .panel {
          position: fixed; right: 16px; bottom: 58px; width: 300px;
          background: #000; color: #e7e9ea;
          border: 1px solid rgba(255,255,255,.14); border-radius: 14px;
          padding: 14px; font-size: 13px; line-height: 1.45;
        }
        .panel[hidden] { display: none; }
        h4 { margin: 0 0 6px; font-size: 13px; font-weight: 700; }
        p { margin: 0 0 10px; color: #71767b; font-size: 12px; }
        .row { display: flex; gap: 6px; }
        input {
          flex: 1; min-width: 0; height: 34px; padding: 0 10px;
          background: #16181c; color: #e7e9ea;
          border: 1px solid rgba(255,255,255,.14); border-radius: 8px;
          font-size: 14px; letter-spacing: .12em; text-transform: uppercase;
        }
        input:focus { outline: none; border-color: #1d9bf0; }
        button {
          height: 34px; padding: 0 12px; border-radius: 8px; border: 0;
          background: #eff3f4; color: #0f1419; font-weight: 700; font-size: 13px;
          cursor: pointer;
        }
        button:disabled { opacity: .5; cursor: default; }
        .stat { color: #71767b; font-size: 12px; margin-top: 10px; }
        .err { color: #f4212e; font-size: 12px; margin-top: 8px; }
        .okline { color: #00ba7c; font-size: 12px; margin-top: 8px; }
      `;

      pillEl = W.document.createElement('div');
      pillEl.className = 'pill';
      pillEl.addEventListener('click', () => {
        if (panelEl) panelEl.hidden = !panelEl.hidden;
        if (panelEl && !panelEl.hidden) refreshPairingHint();
      });

      panelEl = W.document.createElement('div');
      panelEl.className = 'panel';
      panelEl.hidden = true;

      shadow.appendChild(style);
      shadow.appendChild(pillEl);
      shadow.appendChild(panelEl);
      parent.appendChild(wrap);
      render();
      return true;
    } catch (e) {
      loudly('could not build the pill', e);
      return false;
    }
  }

  function refreshPairingHint() {
    if (!panelEl) return;
    const paired = !!store.get('secret', null);
    panelEl.innerHTML = '';

    const h = W.document.createElement('h4');
    h.textContent = paired ? 'Paired' : 'Pair with xitter-dl';
    panelEl.appendChild(h);

    const p = W.document.createElement('p');
    p.textContent = paired
      ? 'Captures are being filed into your local library.'
      : 'Open the app, click Pair, and type the code it shows. You only do this once.';
    panelEl.appendChild(p);

    if (!paired) {
      const row = W.document.createElement('div');
      row.className = 'row';
      const input = W.document.createElement('input');
      input.placeholder = 'ABCD-2345';
      input.maxLength = 12;
      input.autocomplete = 'off';
      input.spellcheck = false;
      const button = W.document.createElement('button');
      button.textContent = 'Pair';
      const msg = W.document.createElement('div');

      const submit = async () => {
        button.disabled = true;
        msg.className = '';
        msg.textContent = 'Checking\u2026';
        try {
          await redeem(input.value);
          refreshPairingHint();
        } catch (e) {
          msg.className = 'err';
          msg.textContent = e.message || String(e);
        } finally {
          button.disabled = false;
        }
      };

      button.addEventListener('click', submit);
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') submit();
      });

      row.appendChild(input);
      row.appendChild(button);
      panelEl.appendChild(row);
      panelEl.appendChild(msg);
      setTimeout(() => input.focus(), 0);
    } else {
      const un = W.document.createElement('button');
      un.textContent = 'Unpair this browser';
      un.addEventListener('click', () => {
        store.set('secret', null);
        setStatus('needs-pairing');
        refreshPairingHint();
      });
      panelEl.appendChild(un);
    }

    const stat = W.document.createElement('div');
    stat.className = 'stat';
    stat.textContent =
      session.pages + ' page(s), ' + session.posts + ' post(s) this session; ' +
      queue.size() + ' waiting to send.';
    panelEl.appendChild(stat);
  }

  function render() {
    if (!ensureUi()) return;
    try {
      const dotClass =
        statusKind === 'ok' ? 'ok' : statusKind === 'needs-pairing' || statusKind === 'app-not-running'
          ? 'warn'
          : statusKind === 'error'
          ? 'bad'
          : '';
      pillEl.innerHTML = '';
      const dot = W.document.createElement('span');
      dot.className = 'dot ' + dotClass;
      const label = W.document.createElement('span');
      label.textContent = statusText + (queue.size() ? ' \u00b7 ' + queue.size() + ' queued' : '');
      pillEl.appendChild(dot);
      pillEl.appendChild(label);
      if (panelEl && !panelEl.hidden) refreshPairingHint();
    } catch (_) {}
  }

  // ── start ──────────────────────────────────────────────────────────────────

  function boot() {
    try {
      if (CFG.hookNetwork) {
        hookFetch();
        hookXhr();
      }
      log.info('hooks installed');

      // Reflect stored state before anything is captured.
      if (store.get('secret', null)) {
        setStatus('app-not-running');
      } else {
        setStatus('needs-pairing');
      }

      // The pill needs a DOM, and `document-start` runs before there is one.
      // Two things can be missing and both are normal: `<body>` (always, at
      // document-start) and `documentElement` itself (sometimes, when the
      // parser has not created anything yet). So observe when there is
      // something to observe, poll for when there is not, and stop on the
      // first success.
      const attach = () => ensureUi();
      if (!attach()) {
        let obs = null;
        let timer = null;
        const stop = () => {
          try {
            if (obs) obs.disconnect();
          } catch (_) {}
          if (timer !== null) clearInterval(timer);
        };
        const tryAttach = () => {
          if (attach()) stop();
        };

        if (W.document.documentElement) {
          obs = new MutationObserver(tryAttach);
          try {
            obs.observe(W.document.documentElement, { childList: true, subtree: true });
          } catch (_) {
            obs = null;
          }
        }
        W.document.addEventListener('DOMContentLoaded', tryAttach, { once: true });
        timer = setInterval(tryAttach, 250);
      }

      // Anything left over from a previous session goes now, if the app is up.
      if (queue.size() > 0) scheduleFlush();

      // A pairing code is short-lived; if the user pairs while we are idle this
      // catches it on the next tick.
      setInterval(() => {
        if (queue.size() > 0) flush();
      }, 20000);
    } catch (e) {
      loudly('boot failed', e);
    }
  }

  boot();
})();
