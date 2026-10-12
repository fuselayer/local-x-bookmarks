# xitter-dl

A local-first library for your X bookmarks. A companion userscript reads the
bookmarks X already sends your own browser, hands them to a desktop app over
loopback, and they land in a single SQLite file you can search offline in
milliseconds.

No API, no OAuth, no scraping service, no cloud, no account, no recurring cost.

---

## Status

| Piece | State |
|---|---|
| `crates/core` | Schema, X payload parsing, importers, FTS5 search — 151 tests |
| `crates/core/src/bridge` | Loopback receiver and pairing — 57 of those, over real sockets |
| `crates/cli` | `xdl import / search / show / stats / bridge` |
| `crates/app` | Tauri v2 shell: window, IPC commands, receiver lifecycle |
| `ui` | Svelte 5 frontend, three X themes, capture and pairing panel |
| `userscript` | Capture script — 36 JS tests |
| Continuous capture | Not implemented |
| Media | Not implemented — see [Media](#media-is-not-implemented) |

Rust: `165` tests. JavaScript: `36`.

---

## Building and running

Dependencies are fetched once; after that everything is offline.

```powershell
cargo fetch
cd ui; pnpm install; cd ..
```

Run the app:

```powershell
cd ui; pnpm build; cd ..
cargo build --release -p xitter-dl-app --features custom-protocol
.\target\release\xitter-dl.exe
```

Or dogfood the pipeline from a terminal, against the same library the app uses:

```powershell
cargo run -p xitter-dl-cli -- import .\fixtures\tweet-simple.json
cargo run -p xitter-dl-cli -- search "localfirst"
cargo run -p xitter-dl-cli -- show 1843712994563928064
cargo run -p xitter-dl-cli -- stats
```

### Debug builds need the dev server; release builds do not

Tauri picks the window URL at compile time. A **debug** build loads
`http://localhost:5273`; a **release** build with the `custom-protocol` feature
loads the frontend embedded from `ui/dist`.

So the debug binary on its own shows `ERR_CONNECTION_REFUSED`. Either:

```powershell
cd ui; pnpm tauri dev          # development, with hot reload
```

or build the release binary as above. Only the release path applies the real
CSP from `crates/app/tauri.conf.json`, so run it before trusting the CSP.

### Where the library lives

One SQLite file holds everything:

| | |
|---|---|
| Default | `%APPDATA%\xitter-dl\library.sqlite` |
| Override | `XITTER_DL_DB` |

The app and the CLI both resolve it through `xdl_core::default_library_path`, so
they cannot disagree about which file holds your bookmarks. `bridge.json` sits
beside it, which is why pairing carries between the two.

Keep two libraries, and never point the app at the wrong one:

- **Yours** — the default path. Launch the app normally and this is what opens.
  Real bookmarks, kept indefinitely; this is the file to back up.
- **Throwaway** — `.scratch\library.sqlite`, reached by setting `XITTER_DL_DB`
  for one command. Fixtures, import tests and smoke runs go here, so a
  half-parsed payload never lands in the real thing.

```powershell
$env:XITTER_DL_DB = '.scratch\library.sqlite'   # this shell only
cargo run -p xitter-dl-cli -- stats
```

`cargo test` cannot touch the real library: no test resolves the default path,
and they work in `std::env::temp_dir()` instead.

---

## Capturing your bookmarks

The userscript files away bookmarks X **already sent** your browser. It
originates no requests, holds no X credential, and never touches the timeline
DOM.

1. Start the app and click **Connect browser**. It shows a short code that
   rotates every two minutes.
2. Install `userscript/xitter-dl-capture.user.js` in Tampermonkey or
   Violentmonkey. Install it from the raw GitHub URL rather than pasting the
   file: Tampermonkey reads `@updateURL` from the copy it already has, so a
   pasted script has no URL to poll and will never update itself. Installed from
   the URL, it keeps itself current whenever `@version` moves.
3. Open `https://x.com/i/bookmarks`. Click the pill in the bottom-right corner
   and type the code.

After that it works across restarts. You re-pair only if you revoke, reinstall
the script, clear its storage, or want to add a second browser — the app's
**Add another browser** issues a fresh code without disturbing the browsers
that already work.

Imported bookmarks appear in the app as they arrive.

### The pill is on every page; capture is not

The pill is attached on every x.com route. It is where capture status and pairing
state are visible, and a pill that exists on only one route cannot tell you why
it is missing on another. Capture is gated to `/i/bookmarks`: on any other route
the script patches nothing and writes nothing to storage. What still runs is the
Tampermonkey menu registration, the pill, and a poll that reads
`location.pathname`.

Gating the pill as well was an overcorrection, and it is what made the pill
disappear. It widened the route check's blast radius from "the hooks" to "the
entire script", so a single answer of *no* — for any reason — produced no pill,
no capture, and nothing in the console. That is indistinguishable from a script
that was never installed, which is exactly how it got misread.

The route check still decides what is **captured**. It no longer decides what is
**visible**.

What the gate was originally for: boot ran everywhere and installed a
`subtree: true` `MutationObserver` over the document, disconnected only once the
pill had been built. A pill that could not be built left that observer firing on
every DOM change for the life of the tab — which is what "any page that is not
the bookmarks page fails to load" turned out to be. The observer is gone;
waiting for `<body>` is a bounded poll that always stops.

Hooks are armed on arrival at the timeline, removed on leaving it, and armed
again on return. Arming used to be one-time while disarming was not, so capture
died at the first navigation away and never came back.

Four rules fall out of all that, and every one of them was learned from the
console:

- **Nothing structured is handed to the userscript manager.** Values crossing
  that boundary are primitives; the queue and the dedupe memory are stored as
  JSON strings. Passing an array produced
  `Permission denied to access property "clonable"` on Firefox, which took the
  whole boot down with it.
- **No event-listener options object.** Firefox reads `capture` off that
  dictionary in the *page's* compartment and refuses one built in the script's:
  `Permission denied to access property "capture"`.
- **A failure is announced.** The script prints `[xitter-dl] <version> loaded`
  and every route change through `console.log`, whether or not debug logging is
  on, and anything that stops it working goes to `console.error` with the same
  prefix. Silence from a capture script is otherwise indistinguishable from a
  capture script that is working.
- **`console.log`, never `console.info`.** Firefox files `info` behind its own
  filter chip, so a beacon sent there is invisible to anyone with the console
  filtered to "Logs" — which is everyone. It was invisible here for two rounds
  of debugging.

The pill's panel repeats the last of these in the UI: it names the route the
script is reading and whether it counts as one it captures on, so a route check
that disagrees with the address bar can be seen without opening the console at
all.

On Firefox the hooks are installed only when `exportFunction` is available to
carry them across compartments; without it the script declines to hook and says
so, rather than handing page code a function it cannot call.

Two switches are reachable from the Tampermonkey menu, so they work even if the
page is broken: network hooks on/off, and debug logging on/off.

### Importing a single tweet without the script

The app's **Import** button takes a pasted payload, and `xdl import` takes a
file, a directory, NDJSON, or a HAR:

1. Open `x.com/i/bookmarks`.
2. DevTools → Network → filter `graphql` → find a `Bookmarks` request.
3. Right-click → Copy → Copy response.
4. Paste into the app's Import box.

Nothing is sent anywhere. The payload is parsed and stored locally.

### Running the userscript tests

```powershell
node userscript/test/capture.test.mjs
node userscript/test/pill.test.mjs
```

`node --test userscript/test/` also works on current Node. The harnesses build
a stubbed browser, evaluate the **real** `.user.js` in it, and drive it through
the fetch hook, so they fail if the hooks stop installing, if `response.clone()`
stops being called, or if the wire format drifts from the Rust parser.

---

## How the two sides find each other

The app binds `127.0.0.1:8737`, or the next free port up to `8757`. The port is
not persisted.

`GET /v1/hello` answers an unauthenticated **pure echo** — it returns the random
`probe` value the caller invented, and nothing else. The script probes the port
range and sends its install secret only to a port that echoed its own nonce.

Without that echo, the script would have to present its secret to every port in
the range to discover which one is the app, handing it to whatever unrelated
process happened to be squatting on the wrong one.

Pairing is a one-time exchange: a short code, valid for two minutes, redeemed
for a 256-bit secret. The secret authenticates every capture afterwards and
never crosses the IPC boundary into the webview.

`xdl bridge` runs the same receiver in the foreground and prints the code as it
rotates — useful for diagnosing a script that will not connect, without a GUI:

```powershell
# terminal 1
cargo run -p xitter-dl-cli -- --db .scratch\e2e.sqlite bridge

# terminal 2 — pairs, captures a generated page, checks every refusal
node scripts/smoke-bridge.mjs A4KY-P8N4
```

---

## Layout

```
crates/core/     Schema, X payload parsing, import, search, the loopback bridge.
crates/cli/      `xdl` — the whole pipeline without a GUI.
crates/app/      Tauri v2 shell: window, typed IPC commands, receiver lifecycle.
ui/              Svelte 5 frontend.
userscript/      The capture script, plus its Node test harnesses.
fixtures/        Test payloads shared by the Rust tests, JS tests and the UI.
scripts/         Build helpers, fixture generator, bridge smoke test.
```

`crates/core` has no Tauri dependency, which is what makes the CLI, the test
suite and the importers usable without a GUI.

---

## Design decisions worth knowing

**The Chirp typeface is not in this repo.** X renders in Chirp, and matching it
is most of the difference between "looks like X" and "looks like X in the wrong
font". Chirp is also not redistributable — it is a commercial typeface from
[Grilli Type](https://www.grillitype.com) licensed to X, and its own metadata
says `All rights reserved` with no license field. The `chirp-font` repositories
on GitHub are unauthorised redistributions: not malware, but not licensed
either, and committing one would put all-rights-reserved software in a public
repo.

So the `@font-face` rules in `ui/src/styles/x.css` are committed and the files
are not (`ui/public/fonts/` is gitignored). Without them the stack falls through
to a system font — no error, no layout shift. Drop legitimate `.woff2` files in
and you get the real thing with no code change. For redistribution, pick a face
you are allowed to ship; the fallback list ends at Inter, the closest
freely-licensed relative.

**The script ships X's raw envelopes.** It does not build a `BookmarkRecord` in
JavaScript. It sends the envelope exactly as X sent it and lets the Rust parser
— the one with the tests — interpret it. Two consequences: the parser can be
fixed retroactively against bytes already captured, and the script never becomes
a second, untested implementation of the same logic. It parses only enough to
count posts for its pill and read the bottom cursor.

**The install secret is a file, not the OS keyring.** It is written to
`bridge.json` beside the library, mode `0600` on Unix, inside `%APPDATA%` on
Windows. The secret authenticates a loopback-only endpoint that exists only
while the app is open. A keyring would add a dependency tree and a failure mode
(a locked or unavailable credential store) for a marginal gain, given an
attacker who can read that file already has the user's session.

**Snippet delimiters are control characters (`U+0001`/`U+0002`), not `<mark>`.**
The frontend turns match positions into DOM nodes. Emitting markup from Rust and
`innerHTML`-ing it would turn any tweet containing markup into script execution.
Control characters cannot appear in tweet text or be typed into a search box, so
the frontend splits on them and builds elements. There is no `{@html}` anywhere
in the app. The CLI opts into printable delimiters, since a terminal cannot show
`U+0001`.

**Conversation modules are captured, but carry no `sortIndex`.** A
`TimelineTimelineModule` orders its tweets as a thread, not as bookmarks, so
letting that ordering into the bookmark ordering would scramble the library.
Skipping them would discard real posts the user can see.

**`sortIndex` is read from the timeline entry, not from `legacy`.** It is a
sibling of `content`. Reading `legacy.sort_index` yields `null` on every record
forever, and presents as "bookmark ordering isn't available" rather than as a
bug. There is a test named after this.

**Entity indices are verified, not trusted.** X's `[start, end]` indices may
count UTF-16 code units or Unicode code points. Get it wrong and every link in a
tweet containing an emoji shifts by one character. `TweetText.svelte` checks
every slice against the entity's own literal text, retries with code-point
indices, and falls back to plain text — an unstyled link is cosmetic, a
misplaced one is corruption.

**The bridge decodes chunked bodies.** The first version refused
`Transfer-Encoding` outright, on the assumption that the only client is our own
script. Node's `http.request` switches to chunked the moment `Content-Length` is
omitted and browsers streaming a body do the same, so that produced a 400
nothing on the client side could explain. The genuine smuggling risk is a
request declaring its length **two** ways, and that is what gets refused.
`Expect: 100-continue` is answered for the same reason — .NET sends it by
default for POSTs and will otherwise wait out its own timeout.

**Reads open their own SQLite connection.** A single mutex-guarded `Library`
would serialise every search behind every import and defeat the WAL pragma.
Writes take a mutex; reads do not.

**The userscript is plain JavaScript with no build step.** Greasy Fork requires
readable source rather than minified output, and a reviewer should be reading
the exact bytes that run.

**It requests `unsafeWindow`.** Patching `window.fetch` from a sandboxed
manager script is unreliable without reaching the page's real window, and a hook
that silently does not install looks exactly like "X stopped sending bookmarks".
The grant is used for one thing: assigning the two hooks.

**`tauri-specta` is not in use yet.** The TypeScript types in
`ui/src/lib/types.ts` are hand-written mirrors of `crates/core/src/model.rs`. A
Rust model change must be mirrored there by hand.

**`rusqlite` has no `fts5` feature.** FTS5 comes from the `bundled` feature,
whose build script passes `-DSQLITE_ENABLE_FTS5` to the C compiler. The
`trigram` tokenizer needs SQLite ≥ 3.34, and the bundle is far newer.

---

## Media is not implemented

No media is downloaded yet, at any size, and frames render from the payload's
own dimensions with the real alt text rather than a broken image. Avatars are
monograms for the same reason.

When it lands, the intended policy has a size ceiling: media lives on
`pbs.twimg.com`, and a bookmarks feed contains multi-hundred-megabyte videos
that should never be pulled down implicitly.

- Default ceiling of a few MB per item. Anything larger is catalogued — size,
  dimensions, duration, URL — but not fetched.
- A skipped item renders as a named placeholder (`video · 412 MB · not
  downloaded`), never a broken image.
- Each skipped item can be fetched individually.
- A default-off toggle allows automatic large downloads, with a hard cap.

The ceiling is checked before any request, so a skipped item costs zero bytes
and never touches the politeness budget.

---

## What this will never do

No X API. No OAuth. No headless browser, Playwright, Puppeteer or Selenium. No
`auth_token` or `ct0` anywhere near the Rust code. No backend-issued GraphQL. No
third-party scraping API, proxies or residential IPs. No cloud sync, telemetry
or phone-home.

The app holds no X credential and links no HTTP client. It could not generate X
traffic if it wanted to, and that is enforced at the dependency boundary rather
than by convention.
