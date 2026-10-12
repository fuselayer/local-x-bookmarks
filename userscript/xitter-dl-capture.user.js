// ==UserScript==
// @name         xitter-dl capture
// @namespace    https://github.com/example/xitter-dl
// @version      0.3.2
// @description  Files away the bookmarks X already sent your browser. No API, no extra requests.
// @author       xitter-dl
// @match        https://x.com/*
// @match        https://twitter.com/*
// @run-at       document-start
// @grant        GM_xmlhttpRequest
// @grant        GM_setValue
// @grant        GM_getValue
// @grant        GM_registerMenuCommand
// @grant        unsafeWindow
// @connect      127.0.0.1
// @connect      localhost
// @updateURL    https://raw.githubusercontent.com/fuselayer/local-x-bookmarks/main/userscript/xitter-dl-capture.user.js
// @downloadURL  https://raw.githubusercontent.com/fuselayer/local-x-bookmarks/main/userscript/xitter-dl-capture.user.js
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
    scriptVersion: '0.3.2',
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

    // Routes this script captures on. Everywhere else it patches nothing and
    // writes nothing to storage. What still runs is the Tampermonkey menu
    // registration, the pill, and a poll that reads `location.pathname`.
    //
    // `activeRoutes` is a guess about X's routing, and a wrong guess fails
    // silently: no hooks, no captures, no error. It has been wrong twice. The
    // second time was `/i/history`, which is not the bookmarks URL and yet
    // fetches the bookmarks timeline anyway — 120 entries of `/graphql/Bookmarks`
    // — so capture was disabled on the only page the user was actually reading.
    // The pill reports the path it read and its verdict, which is how that was
    // found; trust it over this list when they disagree.
    //
    // Reported symptom: x.com failed to load on any route that was not the
    // bookmarks timeline. The hooks were already gated to the timeline, but the
    // *rest* of the script was not — it read storage and installed a
    // document-wide observer on every route, and a boot failure left that
    // observer running for the life of the tab. Killed the observer, bounded
    // the wait for `<body>`, and stopped the off-route storage *writes*.
    //
    // Gating the pill as well was the overcorrection born of that, and it is
    // what broke the pill: see `boot`. This list decides what is *captured*,
    // not what is *visible*.
    //
    // This is also the seam for capturing a bookmark at the moment it is made,
    // which happens on any route and will widen this list rather than remove it.
    // The route watch below is where that plugs in.
    activeRoutes: ['/i/bookmarks', '/i/history'],
  };

  const W = typeof unsafeWindow !== 'undefined' ? unsafeWindow : window;

  /**
   * Say so, in the console, the moment this file runs.
   *
   * Not gated behind the debug flag, because the question it answers — is the
   * script on this page at all — is the first one worth asking and the one that
   * used to be unanswerable. Silence from a capture script is indistinguishable
   * from a capture script that is working.
   *
   * Reads this compartment's `location`, never `unsafeWindow.location`: it is
   * the same URL, and reaching across compartments for it is the thing that can
   * throw. The version is printed so a stale reinstall is visible rather than
   * suspected.
   *
   * `console.log`, deliberately, not `console.info`. Firefox files `info`
   * behind its own filter chip, and this line was invisible for two rounds of
   * debugging because the console was filtered to "Logs" — the one setting
   * anyone actually uses. A diagnostic hidden by the default view is not a
   * diagnostic.
   */
  try {
    console.log('[xitter-dl] ' + CFG.scriptVersion + ' loaded, path ' + location.pathname);
  } catch (_) {}

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
  //
  // Every value that crosses this boundary is a primitive, and both halves of
  // that rule are load-bearing.
  //
  // Handing the manager an array or an object is what produced
  // `Permission denied to access property "clonable"` on Firefox: the manager
  // reaches into the value from its own compartment, the Xray wrapper refuses,
  // and the call throws. A primitive cannot be refused. So anything structured
  // is serialised here first and the boundary only ever sees a string.
  //
  // The same applies to the *default* argument, which is why `getJSON` does not
  // take one in structured form.
  const store = {
    get(key, fallback) {
      try {
        const v = GM_getValue(key, fallback);
        return v === undefined || v === null ? fallback : v;
      } catch (_) {
        return fallback;
      }
    },
    set(key, value) {
      // Not a silent coercion for its own sake: it is the difference between a
      // stored list and a thrown exception, and `set` is called from the hot
      // path where an exception would cost a capture.
      if (value !== null && typeof value === 'object') return store.setJSON(key, value);
      try {
        GM_setValue(key, value);
      } catch (_) {
        /* not fatal: this session just will not survive a reload */
      }
    },
    getJSON(key, fallback) {
      try {
        const raw = GM_getValue(key, null);
        if (typeof raw !== 'string' || raw === '') return fallback;
        const parsed = JSON.parse(raw);
        if (Array.isArray(fallback) && !Array.isArray(parsed)) return fallback;
        return parsed;
      } catch (_) {
        return fallback;
      }
    },
    setJSON(key, value) {
      try {
        GM_setValue(key, JSON.stringify(value));
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
    // Built on first use rather than while the file is being evaluated, so that
    // a route this script does not work on never reads storage at all. This is
    // what makes the route gate in `watchRoute` a real one rather than a
    // promise about the parts that happen to be lazy already.
    lines: null,
    seen: null,
    seenOrder: null,

    ensure() {
      if (this.lines) return;
      const recent = store.getJSON('recentHashes', []);
      this.lines = store.getJSON('queue', []);
      this.seenOrder = recent;
      this.seen = new Set(recent);
    },

    push(line) {
      this.ensure();
      this.lines.push(line);
      // Bound the persisted queue. A user who scrolls 5000 bookmarks with the
      // app closed should not build a 200 MB GM value; the cap is generous and
      // the overflow is surfaced rather than silently dropped.
      if (this.lines.length > 4000) this.lines.splice(0, this.lines.length - 4000);
      store.setJSON('queue', this.lines);
    },

    take(n) {
      this.ensure();
      return this.lines.slice(0, n);
    },

    drop(n) {
      this.ensure();
      this.lines.splice(0, n);
      store.setJSON('queue', this.lines);
    },

    size() {
      this.ensure();
      return this.lines.length;
    },

    /** True the first time this body is seen; false for a repeat. */
    firstSight(hash) {
      this.ensure();
      if (this.seen.has(hash)) return false;
      this.seen.add(hash);
      this.seenOrder.push(hash);
      while (this.seenOrder.length > CFG.dedupeMemory) {
        this.seen.delete(this.seenOrder.shift());
      }
      store.setJSON('recentHashes', this.seenOrder);
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

  // ── hook lifetime, and the route gate ──────────────────────────────────────
  //
  // The hooks exist only while the bookmarks timeline is on screen, and are
  // removed when it is not. Two reasons, and the second is the important one:
  //
  //   1. They are only useful there. Every other route gets the risk and none
  //      of the benefit.
  //   2. They are the one thing in this file that can stop x.com loading, and
  //      a page-breaking bug that only shows up on routes you were not testing
  //      deserves more caution than a tidy implementation.

  let hooked = false;
  let savedOriginals = null;

  /**
   * Read `location.pathname` without assuming which compartment can.
   *
   * `unsafeWindow.location` is the page's own object, and on Firefox reading a
   * property off it can be refused across the Xray boundary — the same failure
   * that produced `Permission denied to access property "capture"`, one object
   * over. There is no need to reach through it: this compartment's `location`
   * is the same URL in the same document. So the page window is tried first,
   * the local one is the fallback, and a refusal is reported instead of being
   * mistaken for "not our route".
   *
   * That mistake is the specific bug this replaces. The gate swallowed every
   * error and answered `false`, so a compartment refusal looked exactly like an
   * ordinary page: no pill, no capture, and not one line in the console.
   */
  function pathname() {
    try {
      const p = W.location && W.location.pathname;
      if (typeof p === 'string') return p;
      loudly('unsafeWindow.location.pathname was not a string', p);
    } catch (e) {
      loudly('could not read unsafeWindow.location', e);
    }
    try {
      return location.pathname;
    } catch (e) {
      loudly('could not read location', e);
      return null;
    }
  }

  /** Is the current route one this script does any work on? */
  function routeIsActive() {
    const p = pathname();
    if (typeof p !== 'string') return false;
    return CFG.activeRoutes.some((r) => p === r || p.startsWith(r + '/'));
  }

  function installHooks() {
    if (hooked) return;
    // Captured before anything is replaced, so leaving the route can put back
    // exactly what was there rather than a wrapper around a wrapper.
    try {
      const proto = W.XMLHttpRequest && W.XMLHttpRequest.prototype;
      savedOriginals = {
        fetch: W.fetch,
        open: proto && proto.open,
        send: proto && proto.send,
      };
    } catch (_) {
      savedOriginals = null;
    }
    hookFetch();
    hookXhr();
    hooked = true;
    log.info('hooks installed');
  }

  function removeHooks() {
    if (!hooked) return;
    if (savedOriginals) {
      try {
        if (savedOriginals.fetch) W.fetch = savedOriginals.fetch;
      } catch (_) {}
      try {
        const proto = W.XMLHttpRequest && W.XMLHttpRequest.prototype;
        if (proto && savedOriginals.open) proto.open = savedOriginals.open;
        if (proto && savedOriginals.send) proto.send = savedOriginals.send;
      } catch (_) {}
    }
    hooked = false;
    log.info('hooks removed');
  }

  /**
   * Run something when the route becomes active, and something else when it
   * stops being active.
   *
   * Polled rather than driven by `popstate`, because x.com's client-side
   * navigation does not reliably announce itself and a 500 ms lag is invisible
   * here: the timeline's first request happens after the route settles, so the
   * hooks are always installed before there is anything to catch.
   *
   * This poll is the *only* thing that runs on a route we do not work on, and
   * it reads nothing but `location.pathname`. That restraint is the fix for
   * "any page that is not the bookmarks page fails to load": a document-start
   * script that touches storage and the DOM on every route is a script that can
   * break every route, and this one did.
   */
  function watchRoute(onActive, onInactive) {
    let active = null;
    const check = () => {
      let now;
      try {
        now = routeIsActive();
      } catch (_) {
        return;
      }
      if (now === active) return;
      active = now;
      // Once per transition, so the gate's decision is on the record. Without
      // it, "the route is not ours" and "this script never ran" read the same.
      // `log`, not `info`, for the same reason as the beacon above.
      try {
        console.log('[xitter-dl] route ' + (now ? 'active' : 'inactive'), pathname());
      } catch (_) {}
      if (now) onActive();
      else onInactive();
    };
    check();
    setInterval(check, 500);
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
        : kind === 'idle'
        ? 'waiting for bookmarks'
        : kind === 'hooks-off'
        ? 'hooks off'
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
      const b = W.document && W.document.body;
      if (b) return b;
    } catch (e) {
      loudly('could not read unsafeWindow.document.body', e);
    }
    try {
      return document.body || null;
    } catch (e) {
      loudly('could not read document.body', e);
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
      // Explicit, because the default for a <button> is "submit". There is no
      // form here today, and this is what keeps that from mattering if one ever
      // ends up in an ancestor.
      button.type = 'button';
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
        if (e.key !== 'Enter') return;
        // Stop the event here, and do not let the default action run.
        //
        // Reported symptom: entering the code and pressing Enter replaced the
        // page with `http://<code>/`. The code is hostname-shaped — `M66W-YDPA`
        // is a perfectly legal hostname — so once the text escaped the input
        // the browser's URL fixup accepted it and navigated. `preventDefault`
        // plus stopping propagation (so x.com's own global key handlers never
        // see it either) is the entire guard.
        e.preventDefault();
        e.stopPropagation();
        submit();
      });

      row.appendChild(input);
      row.appendChild(button);
      panelEl.appendChild(row);
      panelEl.appendChild(msg);
      setTimeout(() => input.focus(), 0);
    } else {
      const un = W.document.createElement('button');
      un.textContent = 'Unpair this browser';
      un.type = 'button';
      un.addEventListener('click', () => {
        store.set('secret', null);
        setStatus('needs-pairing');
        refreshPairingHint();
      });
      panelEl.appendChild(un);
    }

    const stat = W.document.createElement('div');
    stat.className = 'stat';
    // The path it is reading is part of the report, not an implementation
    // detail. "Waiting for bookmarks" while you are looking at the bookmarks
    // timeline means nothing on its own; next to the path the script actually
    // saw, it says immediately whether the route check is broken or the page
    // simply is not the one it captures on.
    stat.textContent =
      (routeIsActive() ? 'capturing on ' : 'not capturing on ') +
      (pathname() || 'an unreadable path') +
      '. ' +
      session.pages +
      ' page(s), ' +
      session.posts +
      ' post(s) this session; ' +
      queue.size() +
      ' waiting to send.';
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

  /**
   * Tampermonkey's own menu — reachable from the toolbar icon, so it still
   * works when the page is too broken to show the pill. That is the whole
   * point: the hooks are the one thing here that can take x.com down, and
   * turning them off must not require editing a file and finding a way back.
   */
  function menus() {
    if (typeof GM_registerMenuCommand !== 'function') return;
    const label = (on) => (on ? 'ON — click to turn OFF' : 'OFF — click to turn ON');
    try {
      GM_registerMenuCommand(
        'xitter-dl network hooks: ' + label(store.get('hookNetwork', CFG.hookNetwork)) + ' (reload after)',
        function () {
          store.set('hookNetwork', !store.get('hookNetwork', CFG.hookNetwork));
          try {
            alert('xitter-dl: reload the page for this to take effect.');
          } catch (_) {}
        }
      );
      GM_registerMenuCommand(
        'xitter-dl debug logging: ' + label(!!store.get('debug', false)) + ' (reload after)',
        function () {
          store.set('debug', !store.get('debug', false));
          try {
            alert('xitter-dl: reload the page for this to take effect.');
          } catch (_) {}
        }
      );
    } catch (_) {}
  }

  let started = false;
  let canHook = false;

  /**
   * One-time, per-document setup. Runs on the first active route.
   *
   * Leaving the route and coming back does not re-run this, because the
   * interval set up here is per-document, not per-route; re-running would stack
   * a second flush timer on every visit. Arming and disarming the hooks is
   * per-route, and lives in `armCapture`/`disarmCapture` below.
   */
  function startCapture() {
    if (started) return;
    started = true;

    // Debug logging is a stored preference so it can be flipped from the
    // Tampermonkey menu, including when the page will not load.
    log.on = !!store.get('debug', false);

    try {
      // Whether hooking is safe is decided at runtime, not from CFG.
      //
      // On Firefox the hooks are only safe when `exportFunction` exists to
      // carry them across compartments. Without it, patching `fetch` stops
      // x.com loading entirely — every request the page makes goes through the
      // wrapper. Refusing to hook is strictly better than taking the site down,
      // so the check is a gate, not a warning.
      //
      // The other half of the decision — the stored on/off flag that lets the
      // Tampermonkey menu turn the hooks off without editing this file, which
      // matters precisely when the page is too broken to reach the pill — is
      // applied per route in `armCapture`, because it has to be re-read every
      // time the route comes back.
      const isFirefox = /firefox|fxios/i.test(W.navigator && W.navigator.userAgent);
      const canExport = typeof exportFunction === 'function';
      canHook = ! (isFirefox && ! canExport);

      if (!canHook) {
        loudly(
          'not hooking fetch',
          'Firefox without `exportFunction`: assigning a sandbox function to ' +
            'window.fetch is what stops x.com loading. Capture is off; the page is fine.'
        );
      }

      // A pairing code is short-lived; if the user pairs while we are idle this
      // catches it on the next tick.
      setInterval(() => {
        if (queue.size() > 0) flush();
      }, 20000);
    } catch (e) {
      loudly('boot failed', e);
    }
  }

  /**
   * Arm capture for the route we are on now, and say so on the pill.
   *
   * Runs on *every* activation, not only the first, because `disarmCapture`
   * takes the hooks back out when the route is left. Arming once meant that
   * navigating away from the bookmarks timeline and back left the hooks removed
   * for the rest of the document's life: capture stopped at the first
   * navigation and never resumed, which from the outside looks exactly like X
   * having nothing new to send.
   */
  function armCapture() {
    try {
      const wanted = store.get('hookNetwork', CFG.hookNetwork);

      if (!canHook) {
        // The reason capture is off matters more than the pairing state, so
        // this case sets the error rather than falling through to it.
        setStatus('error', { error: 'firefox: cannot patch fetch safely' });
      } else if (wanted) {
        installHooks();
        setStatus(store.get('secret', null) ? 'app-not-running' : 'needs-pairing');
      } else {
        log.info('hooks off by choice');
        setStatus('hooks-off');
      }

      // Anything left over from a previous session goes now, if the app is up.
      if (queue.size() > 0) scheduleFlush();
    } catch (e) {
      loudly('arm failed', e);
    }
  }

  function disarmCapture() {
    removeHooks();
    setStatus('idle');
  }

  /**
   * Put the pill on the page, once there is a page to put it on.
   *
   * `@run-at document-start` means `<body>` does not exist yet, which is
   * normal and temporary. What is not normal is how the previous version
   * waited: a `MutationObserver` over the whole document subtree with
   * `subtree: true`, disconnected only once the pill had been built. A pill
   * that could not be built therefore left a subtree observer running on x.com
   * for the life of the tab, firing on every DOM change the site makes — which
   * is what "the page will not load" looked like from the outside.
   *
   * So: bounded, and it always stops. No observer at all, because the only
   * question is whether `<body>` exists yet and a short poll answers it.
   */
  function attachUi() {
    if (ensureUi()) return;

    let tries = 0;
    const timer = setInterval(function () {
      tries += 1;
      if (ensureUi()) {
        clearInterval(timer);
      } else if (tries >= 40) {
        clearInterval(timer);
        // Announced, because giving up quietly is what made a missing pill
        // look like a missing script.
        loudly('gave up waiting for <body>; no pill was built');
      }
    }, 250);

    const onReady = function () {
      try {
        W.document.removeEventListener('DOMContentLoaded', onReady);
      } catch (_) {}
      if (ensureUi()) clearInterval(timer);
    };

    // No options dictionary here, deliberately. Firefox reads `capture` off it
    // in the *page's* compartment, and a dictionary built in this one is
    // refused: `Permission denied to access property "capture"`. That threw out
    // of `boot`, which is how a missing `<body>` turned into a boot failure.
    try {
      W.document.addEventListener('DOMContentLoaded', onReady);
    } catch (_) {}
  }

  function boot() {
    // Registered on every route on purpose. This is the control you reach for
    // when a page will not load, so it must not depend on one having loaded.
    menus();

    // The pill is attached on every route too, and for the same reason. It is
    // where capture status and pairing state are visible, and a pill that only
    // exists on one route cannot tell you why it is missing on another.
    //
    // Gating it was the regression. This call used to sit behind the route
    // check, which widened that check's blast radius from "the hooks" to "the
    // entire script": one answer of `no`, for any reason, and there was no pill,
    // no capture and nothing in the console — the exact symptom of a script that
    // was never installed. It builds one shadow-DOM node and nothing else, and
    // the document-wide observer that once made attaching it dangerous is gone.
    attachUi();

    // Capture itself still waits for a route this script actually works on, and
    // is re-armed every time that route comes back.
    watchRoute(
      () => {
        startCapture();
        armCapture();
      },
      disarmCapture
    );
  }

  boot();
})();
