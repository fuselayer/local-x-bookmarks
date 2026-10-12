/**
 * Tests for the pill — the one piece of UI a user has to click.
 *
 * `capture.test.mjs` deliberately stubs a *broken* DOM (`createElement`
 * throws) to prove capture keeps working without one. That is a good property
 * to hold, but it left the pill entirely unexercised: `buildPill` threw, `boot`
 * caught it, logging is off by default, and so a pill that never rendered was
 * indistinguishable from one that did.
 *
 * So this harness gives the script a DOM that behaves and insists the pill
 * appears. It also covers the two ways the script can take x.com down:
 * patching page objects without `exportFunction` on Firefox, and the decision
 * to patch at all.
 *
 * Run:  node userscript/test/pill.test.mjs
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

/** A DOM small enough to read and complete enough for the pill to build. */
function makeDom() {
  const shadows = [];

  const makeElement = (tag) => {
    const el = {
      tagName: String(tag).toUpperCase(),
      childNodes: [],
      style: { cssText: '' },
      id: '',
      className: '',
      hidden: false,
      disabled: false,
      isConnected: false,
      textContent: '',
      placeholder: '',
      maxLength: 0,
      autocomplete: '',
      spellcheck: true,
      parentNode: null,
      listeners: Object.create(null),
      appendChild(child) {
        child.parentNode = el;
        child.isConnected = true;
        el.childNodes.push(child);
        return child;
      },
      addEventListener(type, fn) {
        (el.listeners[type] || (el.listeners[type] = [])).push(fn);
      },
      focus() {
        el.focused = true;
      },
      attachShadow() {
        const sr = makeElement('#shadow-root');
        sr.host = el;
        el.shadowRoot = sr;
        shadows.push(sr);
        return sr;
      },
    };
    Object.defineProperty(el, 'innerHTML', {
      get: () => '',
      set: () => {
        el.childNodes.length = 0;
      },
    });
    return el;
  };

  const documentElement = makeElement('html');
  const body = makeElement('body');
  documentElement.appendChild(body);

  return {
    doc: { documentElement, body, createElement: makeElement, addEventListener() {} },
    documentElement,
    body,
    shadows,
  };
}

/** Walk a tree (through shadow roots too) collecting matches. */
function findAll(node, pred, out = []) {
  for (const child of node.childNodes || []) {
    if (pred(child)) out.push(child);
    findAll(child, pred, out);
  }
  if (node.shadowRoot) findAll(node.shadowRoot, pred, out);
  return out;
}

const FIREFOX_UA = 'Mozilla/5.0 (Windows NT 10.0; rv:130.0) Gecko/20100101 Firefox/130.0';
const CHROME_UA = 'Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/130.0 Safari/537.36';

/**
 * @param {object}  o
 * @param {boolean} o.firefox    Which navigator.userAgent to present.
 * @param {boolean} o.exported   Whether `exportFunction` exists, as on Firefox.
 * @param {object}  o.stored     GM storage, e.g. { hookNetwork: false }.
 */
function boot({ firefox = false, exported = false, stored = {}, path = '/i/bookmarks', noBody = false } = {}) {
  const dom = makeDom();
  if (noBody) dom.doc.body = null;
  const values = new Map(Object.entries(stored));
  const messages = [];
  // Recorded so a test can prove the script installs none of them. See the
  // regression guard at the bottom of this file.
  const observers = [];
  // Intervals are recorded rather than run: `boot` starts a 20 s flush timer and
  // a 500 ms route poll, and neither should fire in a unit test. A test that
  // wants one drives it by hand.
  const intervals = [];

  // Kept as named references so a test can prove the script left them alone.
  const pageFetch = async () => ({ ok: true, clone: () => ({ text: async () => '{}' }) });
  const xhrOpen = function () {};
  const xhrSend = function () {};

  const win = {
    crypto: {
      getRandomValues(a) {
        for (let i = 0; i < a.length; i++) a[i] = (i * 37 + 11) & 255;
        return a;
      },
    },
    navigator: { userAgent: firefox ? FIREFOX_UA : CHROME_UA },
    location: { pathname: path },
    document: dom.doc,
    MutationObserver: class {
      constructor(cb) {
        this.cb = cb;
        observers.push(this);
      }
      observe(target, opts) {
        this.target = target;
        this.opts = opts;
      }
      disconnect() {
        this.disconnected = true;
      }
    },
    fetch: pageFetch,
    XMLHttpRequest: function () {},
  };
  win.XMLHttpRequest.prototype = { open: xhrOpen, send: xhrSend };

  const targets = [];
  const ctx = vm.createContext({
    unsafeWindow: win,
    // The sandbox a userscript actually runs in has `location` and `document`
    // as globals, so the harness has to as well. Leaving `location` out made
    // the load beacon throw into its own catch block and vanish — the script
    // looked silent here while being fine in Firefox, which is the opposite of
    // what a harness is for. Same object as `win.location`, so mutating the one
    // to simulate navigation moves both.
    location: win.location,
    document: dom.doc,
    GM_getValue: (k, d) => (values.has(k) ? values.get(k) : d),
    GM_setValue: (k, v) => void values.set(k, v),
    GM_xmlhttpRequest() {},
    GM_registerMenuCommand() {},
    ...(exported
      ? {
          exportFunction: (fn, target) => {
            targets.push(target);
            const clone = function (...args) {
              return fn.apply(this, args);
            };
            clone.exported = true;
            return clone;
          },
        }
      : {}),
    console: {
      info: (...a) => messages.push(['info', a.join(' ')]),
      warn: (...a) => messages.push(['warn', a.join(' ')]),
      log: (...a) => messages.push(['log', a.join(' ')]),
      error: (...a) => messages.push(['error', a.join(' ')]),
    },
    setTimeout,
    clearTimeout,
    setInterval: (fn, ms) => {
      intervals.push({ fn, ms, runs: 0, cleared: false });
      return intervals.length;
    },
    clearInterval: (id) => {
      const it = intervals[id - 1];
      if (it) it.cleared = true;
    },
    URL,
  });

  vm.runInContext(SCRIPT, ctx);

  const anchor = dom.body
    ? dom.body.childNodes.find((n) => n.id === 'xdl-capture-pill')
    : null;
  const byLevel = (l) => messages.filter(([level]) => level === l).map(([, m]) => m);
  const pillText = () =>
    findAll(anchor || dom.body, (n) => n.tagName === 'SPAN' && n.textContent)
      .map((n) => n.textContent)
      .join(' ');

  return {
    dom,
    win,
    anchor,
    pillText,
    targets,
    messages,
    observers,
    intervals,
    warnings: byLevel('warn'),
    errors: byLevel('error'),
    values,
    originals: { fetch: pageFetch, xhrOpen, xhrSend },
  };
}

// ── the pill ─────────────────────────────────────────────────────────────────

test('the pill is built and attached to the page', () => {
  const { anchor, warnings, errors } = boot();
  const why = [...warnings, ...errors].join(' | ') || 'none';

  assert.ok(anchor, `no #xdl-capture-pill was appended. reported: ${why}`);
  assert.ok(anchor.shadowRoot, 'the pill must live in a shadow root so x.com cannot restyle it');
  assert.equal(findAll(anchor, (n) => n.className === 'pill').length, 1);
});

test('the pill is attached to <body>, never <html>', () => {
  // At document-start `<body>` does not exist. Falling back to documentElement
  // injected a div into the root element mid-parse.
  const { dom } = boot();
  assert.equal(
    dom.documentElement.childNodes.includes(
      dom.body.childNodes.find((n) => n.id === 'xdl-capture-pill')
    ),
    false,
    'the pill must not be a child of <html>'
  );
});

test('the pill shows the unpaired state before any pairing', () => {
  const { anchor, pillText } = boot();
  assert.match(pillText(), /not paired/);

  const dot = findAll(anchor, (n) => n.className.startsWith('dot'));
  assert.equal(dot.length, 1);
  assert.match(dot[0].className, /warn/, 'unpaired should be the warning colour');
});

test('clicking the pill opens a panel with a pairing code input', () => {
  const { anchor } = boot();

  const pill = findAll(anchor, (n) => n.className === 'pill')[0];
  const panel = findAll(anchor, (n) => n.className === 'panel')[0];
  assert.equal(panel.hidden, true, 'the panel starts closed');

  pill.listeners.click[0]();
  assert.equal(panel.hidden, false, 'clicking the pill should open the panel');

  const input = findAll(panel, (n) => n.tagName === 'INPUT')[0];
  assert.ok(input, 'the open panel must offer somewhere to type the code');
  assert.equal(input.placeholder, 'ABCD-2345');
  assert.ok(findAll(panel, (n) => n.tagName === 'BUTTON').length, 'and a way to submit it');
});

test('the open panel reports the route the script is reading', () => {
  // "Waiting for bookmarks" while you are looking at the bookmarks timeline is
  // only meaningful next to the path the script actually saw. Without it, a
  // broken route check and a page the script simply does not capture on produce
  // the same pill, and the console is the only way to tell them apart — which
  // is exactly the position a report came from.
  const onRoute = boot({ path: '/i/bookmarks' });
  const offRoute = boot({ path: '/home' });

  const panelText = ({ anchor }) => {
    const pill = findAll(anchor, (n) => n.className === 'pill')[0];
    const panel = findAll(anchor, (n) => n.className === 'panel')[0];
    pill.listeners.click[0]();
    return findAll(panel, (n) => n.className === 'stat')
      .map((n) => n.textContent)
      .join(' ');
  };

  assert.match(panelText(onRoute), /capturing on \/i\/bookmarks/);
  assert.match(panelText(offRoute), /not capturing on \/home/);
});

test('booting is silent on both failure channels', () => {
  const { warnings, errors } = boot();
  assert.deepEqual(warnings, [], `unexpected warnings: ${warnings.join(' | ')}`);
  assert.deepEqual(errors, [], `unexpected errors: ${errors.join(' | ')}`);
});

test('the load beacon goes to console.log, so the Logs filter shows it', () => {
  // It was `console.info` and Firefox files that behind a separate "Info" chip,
  // so two rounds of debugging happened against a console filtered to "Logs"
  // that could not have shown it.
  const { messages } = boot();
  const beacon = messages.find(([, m]) => m.includes('[xitter-dl]') && m.includes('loaded'));

  assert.ok(beacon, 'the beacon should be printed on load');
  assert.equal(beacon[0], 'log', `the beacon went to ${beacon[0]}, which the Logs filter hides`);
});

// ── the hooks, and the two ways they can take x.com down ─────────────────────

test('hooks are exported into the page compartment when exportFunction exists', () => {
  const { win, originals, targets } = boot({ firefox: true, exported: true });

  assert.ok(targets.length >= 2, 'intoPage should have exported fetch and the XHR methods');
  assert.notEqual(win.fetch, originals.fetch, 'fetch should have been hooked');
  assert.equal(win.fetch.exported, true, 'window.fetch must be the exported clone');
  assert.equal(
    win.XMLHttpRequest.prototype.open.exported,
    true,
    'XMLHttpRequest.prototype.open must be exported too'
  );
});

test('Firefox without exportFunction refuses to hook, and says why', () => {
  // The failure this guards: page code calling window.fetch() gets a function
  // from the userscript's compartment, Xray vision rejects the call, and
  // x.com cannot load because every request it makes goes through the wrapper.
  // Refusing to hook is strictly better than taking the site down.
  const { win, originals, anchor, errors, pillText } = boot({ firefox: true, exported: false });

  assert.equal(win.fetch, originals.fetch, 'fetch must be left alone');
  assert.equal(win.XMLHttpRequest.prototype.open, originals.xhrOpen, 'XHR.open must be left alone');
  assert.equal(win.XMLHttpRequest.prototype.send, originals.xhrSend, 'XHR.send must be left alone');

  assert.ok(anchor, 'the pill must still render — the page is fine, capture is off');
  assert.match(pillText(), /cannot patch fetch safely/, 'the pill must say capture is off');
  assert.ok(
    errors.some((m) => m.includes('not hooking fetch')),
    'the refusal must be reported unconditionally, not only when logging is on'
  );
});

test('Chrome hooks without exportFunction, because it has no Xray vision', () => {
  const { win, originals } = boot({ firefox: false, exported: false });
  assert.notEqual(win.fetch, originals.fetch, 'fetch should be hooked on Chrome');
});

test('a stored hookNetwork:false leaves everything the page owns untouched', () => {
  // Reachable from the Tampermonkey menu, which is the point: it must work
  // when x.com is too broken to show the pill.
  const { win, anchor, originals } = boot({ stored: { hookNetwork: false } });

  assert.ok(anchor, 'the pill must still render with the hooks off');
  assert.equal(win.fetch, originals.fetch, 'fetch must be left alone');
  assert.equal(win.XMLHttpRequest.prototype.open, originals.xhrOpen, 'XHR.open must be left alone');
  assert.equal(win.XMLHttpRequest.prototype.send, originals.xhrSend, 'XHR.send must be left alone');
});

test('a stored debug flag turns the gated logger on', () => {
  const { messages } = boot({ stored: { debug: true } });
  assert.ok(
    messages.some(([, m]) => m.includes('hooks installed')),
    'debug logging should be on when the stored flag says so'
  );
});

test('no hooks are installed anywhere except the bookmarks route', () => {
  // The reported failure: x.com would not load on a status page while the
  // bookmarks timeline was fine. The hooks can only be useful on the timeline,
  // so on every other route the correct number of page objects to patch is
  // zero.
  // Synthetic paths only. A real status URL was used here while diagnosing a
  // report, which put somebody else's handle and a real tweet id into a file
  // destined for a public repository.
  for (const path of ['/', '/home', '/someone/status/1234567890123456789']) {
    const { win, originals } = boot({ path });
    assert.equal(win.fetch, originals.fetch, `fetch must be untouched on ${path}`);
    assert.equal(
      win.XMLHttpRequest.prototype.open,
      originals.xhrOpen,
      `XHR.open must be untouched on ${path}`
    );
    assert.equal(
      win.XMLHttpRequest.prototype.send,
      originals.xhrSend,
      `XHR.send must be untouched on ${path}`
    );
  }
});

test('the bookmarks route does get hooked', () => {
  // `/i/history` is not the bookmarks URL, and it fetches the bookmarks
  // timeline anyway: a session on it logged `/graphql/Bookmarks` with 120
  // entries. Capture was disabled on the one page the user was reading, which
  // is why it is here rather than in the off-route list above.
  for (const path of ['/i/bookmarks', '/i/bookmarks/folder/123', '/i/history']) {
    const { win, originals } = boot({ path });
    assert.notEqual(win.fetch, originals.fetch, `fetch should be hooked on ${path}`);
  }
});

test('leaving the route and coming back re-hooks, rather than staying disarmed', () => {
  // The bug this exists for: arming was a one-time act guarded by `started`,
  // while disarming was not. So the hooks came out on the first navigation away
  // and were never put back — capture died at the first route change and stayed
  // dead for the life of the document, with the pill still cheerfully on
  // screen. The poll is driven by hand here; `boot` only registers it.
  const { win, originals, intervals } = boot({ path: '/i/bookmarks' });
  const poll = intervals.find((i) => i.ms === 500);
  assert.ok(poll, 'the route poll should be registered');

  assert.notEqual(win.fetch, originals.fetch, 'armed on first arrival');

  win.location.pathname = '/home';
  poll.fn();
  assert.equal(win.fetch, originals.fetch, 'disarmed on leaving');

  win.location.pathname = '/i/bookmarks';
  poll.fn();
  assert.notEqual(win.fetch, originals.fetch, 're-armed on returning');
});

// ── what must NOT happen off the bookmarks route ─────────────────────────────

test('off-route, the script appends the pill and nothing else, and patches nothing', () => {
  // The reported failure was "any page that is not the bookmarks page fails to
  // load", and gating only the *hooks* did not fix it: the rest of boot still
  // ran everywhere — it read storage and installed a document-wide observer
  // that only disconnected once the pill had been built. A pill that could not
  // be built left that observer running for the life of the tab.
  //
  // Gating the pill as well was the overcorrection. It made the whole script's
  // visibility depend on one route check, so a check that answered `no` for any
  // reason produced no pill, no capture, and no message — indistinguishable
  // from a script that was never installed. The observer is what was dangerous,
  // and it is gone; one shadow-DOM node is not.
  //
  // Synthetic paths only, as above.
  for (const path of ['/', '/home', '/someone/status/1234567890123456789']) {
    const { dom, values, warnings, errors, win, originals } = boot({ path });

    assert.equal(dom.body.childNodes.length, 1, `only the pill may be appended on ${path}`);
    assert.equal(
      dom.body.childNodes[0].id,
      'xdl-capture-pill',
      `the one appended node on ${path} should be the pill anchor`
    );
    assert.equal(win.fetch, originals.fetch, `fetch must be untouched on ${path}`);
    assert.equal(values.size, 0, `nothing may be written to storage on ${path}`);

    const why = [...warnings, ...errors].join(' | ');
    assert.equal(why, '', `booting off-route must be silent, got: ${why}`);
  }
});

test('nothing structured is ever handed to the manager, off-route included', () => {
  // `Permission denied to access property "clonable"` came from passing an
  // array to GM storage: the manager reaches into the value from its own
  // compartment and the Xray wrapper refuses. Storage therefore only ever sees
  // strings, numbers, booleans or null. The end-to-end version of this, with an
  // actual capture behind it, lives in capture.test.mjs.
  for (const path of ['/i/bookmarks', '/home']) {
    const { values } = boot({ path });
    for (const [key, value] of values) {
      const kind = value === null ? 'null' : typeof value;
      assert.ok(
        value === null || ['string', 'number', 'boolean'].includes(kind),
        `${key} was stored as ${kind} on ${path}; only primitives may cross this boundary`
      );
    }
  }
});

test('no MutationObserver is installed, on any route', () => {
  // This is the page-hang itself. Waiting for `<body>` used to mean a
  // `subtree: true` observer over the document, disconnected only once the pill
  // had been built — so a pill that could not be built left it firing on every
  // DOM change x.com makes, for the life of the tab.
  //
  // The only question is whether `<body>` exists yet, and a short poll answers
  // it. Nothing here needs an observer at all, and this test is what stops one
  // coming back.
  for (const path of ['/i/bookmarks', '/i/bookmarks/folder/1', '/home', '/']) {
    const { observers } = boot({ path });
    assert.equal(observers.length, 0, `a MutationObserver was installed on ${path}`);
  }
});

test('waiting for <body> gives up instead of polling forever', () => {
  // The bounded half of the same fix. With `<body>` never appearing, the retry
  // has to stop on its own. Driven by hand through the recorded timers, because
  // the real ones would make this test wait ten seconds to prove it.
  const { intervals } = boot({ path: '/i/bookmarks', noBody: true });

  const poll = intervals.find((i) => i.ms === 250);
  assert.ok(poll, 'the script should poll for <body> while it is missing');

  for (let i = 0; i < 200 && !poll.cleared; i++) {
    poll.runs += 1;
    poll.fn();
  }

  assert.ok(poll.cleared, 'the poll must stop on its own, not run for the life of the tab');
  assert.ok(poll.runs <= 60, `the poll ran ${poll.runs} times; it is supposed to be bounded`);
});
