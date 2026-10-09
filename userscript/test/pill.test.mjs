/**
 * Tests for the pill — the one piece of UI a user has to click.
 *
 * ## Why this file exists separately
 *
 * `capture.test.mjs` deliberately stubs a *broken* DOM (`createElement`
 * throws) to prove that capture keeps working without one. That is a good
 * property to hold, but it left the pill entirely unexercised: `buildPill`
 * threw, `boot` caught it, `log` is off by default, and so a pill that never
 * rendered was indistinguishable from one that did. Sixteen passing tests
 * said nothing at all about the thing the user actually has to find on screen.
 *
 * So this harness gives the script a DOM that behaves, and then insists the
 * pill appears. It also runs the script with logging forced on, so a failure
 * arrives with the script's own explanation attached rather than a bare
 * assertion.
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

// Logging is off in the shipped script on purpose — a noisy console on x.com
// is indistinguishable from a broken script. Flipping it here means a failure
// reports the script's own reason. Guarded, so this cannot silently no-op if
// the wording ever changes.
const SCRIPT_LOUD = SCRIPT.replace('on: false,', 'on: true,');
assert.notEqual(SCRIPT_LOUD, SCRIPT, 'could not force logging on — did `on: false` move?');

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

function boot({ script = SCRIPT_LOUD, exportFunction = null } = {}) {
  const dom = makeDom();
  const values = new Map();
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
    document: dom.doc,
    MutationObserver: class {
      observe() {}
      disconnect() {}
    },
    fetch: pageFetch,
    XMLHttpRequest: function () {},
  };
  win.XMLHttpRequest.prototype = { open: xhrOpen, send: xhrSend };

  const ctx = vm.createContext({
    unsafeWindow: win,
    GM_getValue: (k, d) => (values.has(k) ? values.get(k) : d),
    GM_setValue: (k, v) => void values.set(k, v),
    GM_xmlhttpRequest() {},
    ...(exportFunction ? { exportFunction } : {}),
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

  vm.runInContext(script, ctx);

  const anchor = dom.body.childNodes.find((n) => n.id === 'xdl-capture-pill');
  const byLevel = (l) => messages.filter(([level]) => level === l).map(([, m]) => m);
  return {
    dom,
    win,
    anchor,
    messages,
    warnings: byLevel('warn'),
    errors: byLevel('error'),
    values,
    ctx,
    originals: { fetch: pageFetch, xhrOpen, xhrSend },
  };
}

// ── the pill ─────────────────────────────────────────────────────────────────

test('the pill is built and attached to the page', () => {
  const { anchor, warnings } = boot();

  assert.ok(anchor, `no #xdl-capture-pill was appended. warnings: ${warnings.join(' | ') || 'none'}`);
  assert.ok(anchor.shadowRoot, 'the pill must live in a shadow root so x.com cannot restyle it');

  const pill = findAll(anchor, (n) => n.className === 'pill');
  assert.equal(pill.length, 1, 'exactly one .pill should exist');
});

test('the pill exists even though capture needs no DOM', () => {
  // The old harness asserted the opposite property — that the script survives a
  // broken DOM. Both must hold: no DOM must not break capture, and a working
  // DOM must actually produce the UI.
  const { dom, messages } = boot();
  assert.equal(dom.shadows.length >= 1, true, 'a shadow root should have been created');
  assert.ok(
    messages.some(([, m]) => m.includes('hooks installed')),
    'the script should have reported installing its hooks'
  );
});

test('the pill shows the unpaired state before any pairing', () => {
  const { anchor } = boot();

  const labels = findAll(anchor, (n) => n.tagName === 'SPAN' && n.textContent);
  const text = labels.map((n) => n.textContent).join(' ');

  assert.match(text, /not paired/, `expected an unpaired label, got: ${JSON.stringify(text)}`);

  const dot = findAll(anchor, (n) => n.className.startsWith('dot'));
  assert.equal(dot.length, 1, 'the status dot should be present');
  assert.match(dot[0].className, /warn/, 'unpaired should be the warning colour, not connected');
});

test('clicking the pill opens a panel with a pairing code input', () => {
  const { anchor } = boot();

  const pill = findAll(anchor, (n) => n.className === 'pill')[0];
  assert.ok(pill.listeners.click && pill.listeners.click.length, 'the pill must be clickable');

  const panel = findAll(anchor, (n) => n.className === 'panel')[0];
  assert.ok(panel, 'a panel should exist');
  assert.equal(panel.hidden, true, 'the panel starts closed');

  pill.listeners.click[0]();
  assert.equal(panel.hidden, false, 'clicking the pill should open the panel');

  const input = findAll(panel, (n) => n.tagName === 'INPUT')[0];
  assert.ok(input, 'the open panel must offer somewhere to type the code');
  assert.equal(input.placeholder, 'ABCD-2345');

  const button = findAll(panel, (n) => n.tagName === 'BUTTON')[0];
  assert.ok(button, 'the panel must offer a way to submit the code');
});

test('booting is silent on both failure channels', () => {
  const { warnings, errors } = boot();
  assert.deepEqual(warnings, [], `unexpected warnings: ${warnings.join(' | ')}`);
  assert.deepEqual(errors, [], `unexpected errors: ${errors.join(' | ')}`);
});

test('the hooks are exported into the page compartment', () => {
  // The Firefox failure mode: page code calling window.fetch() gets a function
  // from the userscript's compartment, Xray vision rejects the call, and x.com
  // cannot load because every request it makes goes through this wrapper.
  const targets = [];
  const { win, originals } = boot({
    exportFunction: (fn, target) => {
      targets.push(target);
      const clone = function (...args) {
        return fn.apply(this, args);
      };
      clone.exported = true;
      return clone;
    },
  });

  assert.ok(targets.length >= 2, 'intoPage should have exported fetch and the XHR methods');
  assert.notEqual(win.fetch, originals.fetch, 'fetch should have been hooked');
  assert.equal(win.fetch.exported, true, 'window.fetch must be the exported clone');
  assert.equal(
    win.XMLHttpRequest.prototype.open.exported,
    true,
    'XMLHttpRequest.prototype.open must be exported too'
  );
});

test('hookNetwork:false leaves everything the page owns untouched', () => {
  // The bisect tool for "x.com will not load": if the page loads here and not
  // with the hooks on, the hooks are the cause and the UI is exonerated.
  const off = SCRIPT_LOUD.replace('hookNetwork: true,', 'hookNetwork: false,');
  assert.notEqual(off, SCRIPT_LOUD, 'could not flip hookNetwork — did it move?');

  const { win, anchor, originals } = boot({ script: off });

  assert.ok(anchor, 'the pill must still render with the hooks off');
  assert.equal(win.fetch, originals.fetch, 'fetch must be left alone');
  assert.equal(
    win.XMLHttpRequest.prototype.open,
    originals.xhrOpen,
    'XMLHttpRequest.prototype.open must be left alone'
  );
  assert.equal(
    win.XMLHttpRequest.prototype.send,
    originals.xhrSend,
    'XMLHttpRequest.prototype.send must be left alone'
  );
});
