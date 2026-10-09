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
function boot({ firefox = false, exported = false, stored = {}, path = '/i/bookmarks' } = {}) {
  const dom = makeDom();
  const values = new Map(Object.entries(stored));
  const messages = [];

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
      observe() {}
      disconnect() {}
    },
    fetch: pageFetch,
    XMLHttpRequest: function () {},
  };
  win.XMLHttpRequest.prototype = { open: xhrOpen, send: xhrSend };

  const targets = [];
  const ctx = vm.createContext({
    unsafeWindow: win,
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
    setInterval: () => 0,
    clearInterval: () => {},
    URL,
  });

  vm.runInContext(SCRIPT, ctx);

  const anchor = dom.body.childNodes.find((n) => n.id === 'xdl-capture-pill');
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

test('booting is silent on both failure channels', () => {
  const { warnings, errors } = boot();
  assert.deepEqual(warnings, [], `unexpected warnings: ${warnings.join(' | ')}`);
  assert.deepEqual(errors, [], `unexpected errors: ${errors.join(' | ')}`);
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
  for (const path of ['/', '/home', '/someone/status/1234567890123456789', '/i/history']) {
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
  for (const path of ['/i/bookmarks', '/i/bookmarks/folder/123']) {
    const { win, originals } = boot({ path });
    assert.notEqual(win.fetch, originals.fetch, `fetch should be hooked on ${path}`);
  }
});
