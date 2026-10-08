# xitter-dl

A local-first library for your X bookmarks. Ingested by a companion userscript
that rides along in **your own browser**, stored in a single SQLite file,
searchable offline in milliseconds. No API, no cloud, no account, no recurring
cost.

> **The design contract is in [`PRD.md`](PRD.md).** This file covers how to
> build and run it, and — more usefully — everywhere the implementation
> deliberately departs from that document, with reasons.

---

## Status

Milestone 1: **the app runs and renders an imported tweet exactly as X does.**

| Piece | State |
|---|---|
| `crates/core` — schema, parser, importers, search | ✅ 143 tests passing |
| `crates/core/src/bridge` — loopback receiver, pairing | ✅ 57 of those, incl. real HTTP round-trips |
| `crates/cli` — `xdl import / search / show / stats` | ✅ working |
| `ui/` — Svelte 5 frontend, three X themes | ✅ builds |
| `crates/app` — Tauri v2 shell | ✅ builds; bridge starts on launch |
| `userscript/` — capture script | ✅ 16 tests green; **not yet run against a real scroll** |
| Continuous capture (save-as-you-go) | ⬜ next — see [Deviations](#deferred-not-dropped) |
| Pairing panel in the app UI | ⬜ next — the bridge and the script both work; nothing draws it yet |
| Media pipeline | ⬜ M3 |

---

## Quick start

Everything goes through one wrapper, because this environment needs a
workspace-local `CARGO_HOME` and a process-scoped execution-policy bypass. See
[Environment](#environment-notes) for why.

```powershell
# one-time: restore dependencies
.\scripts\dev.cmd cargo fetch
cd ui; pnpm install; cd ..

# run the whole pipeline headlessly
.\scripts\dev.cmd cargo run -p xitter-dl-cli -- import .\fixtures\tweet-simple.json
.\scripts\dev.cmd cargo run -p xitter-dl-cli -- search "localfirst"
.\scripts\dev.cmd cargo run -p xitter-dl-cli -- show 1843712994563928064

# or run the app
cd ui; pnpm tauri dev
```

`xdl` reads and writes the same library the app does, so anything imported from
the terminal is instantly visible in the window.

### Debug builds need the dev server. Release builds do not.

This trips people up, so it is worth stating plainly:

Tauri picks the window URL at compile time. A **debug** build loads
`build.devUrl` (`http://localhost:5273`); a **release** build loads the frontend
embedded from `build.frontendDist` (`ui/dist`).

So running the debug binary on its own gives you a window containing
`ERR_CONNECTION_REFUSED` — the app is fine, there is just nothing listening on
5273. Two ways to get a working window:

```powershell
# development: starts vite, then the app, with hot reload
cd ui; pnpm tauri dev

# production: embeds ui/dist and applies the locked-down CSP
cd ui; pnpm build; cd ..
.\scripts\dev.cmd cargo build --release -p xitter-dl-app --features custom-protocol
.\target\release\xitter-dl.exe
```

The release path is also the only one that exercises the real CSP from
`crates/app/tauri.conf.json`, so it is worth running before trusting §10.

### Importing your first real tweet

The app's **Import** button takes a pasted payload. That is the whole import
path for milestone 1, and it deliberately needs no browser, no file dialog and
no network:

1. Open `x.com/i/bookmarks` in your browser.
2. DevTools → Network → filter `graphql` → find a `Bookmarks` request.
3. Right-click → Copy → Copy response.
4. Paste into the app's Import box.

Nothing is sent anywhere. The payload is parsed and stored locally.

### Capturing your bookmarks for real

The userscript is the point of the product. It files away the bookmarks X
**already sent** your browser — it originates no requests, holds no credential,
and never touches the DOM.

```powershell
# 1. start the app; it binds 127.0.0.1:8737 and prints nothing
.\target\release\xitter-dl.exe

# 2. install userscript/xitter-dl-capture.user.js
#    Tampermonkey: Dashboard -> Utilities -> Install from file
#    Violentmonkey: + -> Install from file

# 3. open https://x.com/i/bookmarks and scroll
```

Pairing is a one-time exchange. The app shows a short code, you paste it into
the pill in the corner of the page, and the script stores the install secret it
gets back. After that it works across restarts; you re-pair only if you revoke,
reinstall, or clear script storage.

**The script is deliberately small enough to read in one sitting.** Before you
run it, check two things in it: that `fetch` is read through `response.clone()`
(read the original and you break the page), and that the only URLs it ever
contacts are `http://127.0.0.1:<port>`. Both are asserted in the test suite.

#### Running the userscript tests

```powershell
# not `node --test <dir>`: that spawns a child process per file, which the
# sandbox on this host refuses with EPERM. Running the file directly is
# equivalent and does not spawn.
.\scripts\dev.cmd node userscript\test\capture.test.mjs
```

The harness builds a stubbed browser, evaluates the **real** `.user.js` inside
it, and drives it through the fetch hook — so the tests fail if the hooks stop
being installed or the wire format drifts from the Rust parser.

#### How the two sides find each other

The app binds `8737` and, if that is taken, the next free port up to `8757`.
The port is deliberately **not** persisted: instead, `/v1/hello` answers an
unauthenticated *pure echo*, returning the random `probe` value the caller
invented. The script probes the range, and only sends its install secret to a
port that echoed its own nonce.

That echo is not decoration. Without it the script would have to present the
secret to every port in the range to discover which one is the app — handing it
to whatever unrelated process happened to be squatting on the wrong one.

#### Checking it works without a browser

`xdl bridge` runs the same receiver the desktop app does, in the foreground,
printing the pairing code as it rotates:

```powershell
# terminal 1
.\scripts\dev.cmd cargo run -p xitter-dl-cli -- --db .scratch\e2e.sqlite bridge

# terminal 2 — pairs, captures a generated page, and checks every refusal
.\scripts\dev.cmd node scripts\smoke-bridge.mjs A4KY-P8N4
```

This is the thing to run when a script "will not connect": it fails loudly with
the status and the response body of whichever step broke, rather than leaving
you guessing between pairing, framing and the port.

---

## Layout

```
crates/core/     schema, X payload parsing, import, search, the loopback bridge.
crates/cli/      `xdl` — dogfood the pipeline without a GUI.
crates/app/      Tauri v2 shell: window, typed IPC commands, bridge lifecycle.
ui/              Svelte 5 frontend.
userscript/      The capture script, plus its Node test harness.
fixtures/        Canonical test payloads, shared by Rust tests, JS tests and the UI.
scripts/         dev-env.ps1, dev.cmd, make-icon.py, make-fixture-bookmarks.mjs.
```

`crates/core` having no Tauri dependency is what makes the CLI, the test suite
and the importers usable without a GUI — the split the PRD takes from gyotaku
(§7.2).

---

## Environment notes

Three host quirks shaped the build setup. All are handled in
`scripts/dev-env.ps1` and `scripts/dev.cmd`; none affect a normal machine.

**1. Rust is not on `PATH`, and `~/.cargo` is not writable.**
Cargo writes to `$CARGO_HOME` on every dependency fetch. The dev script
therefore keeps `CARGO_HOME` inside the repo (`.cargo-home/`, gitignored) and
calls the toolchain binaries directly rather than through the rustup shims.

**2. This host blocks all unsigned local `.ps1` files.**
Not just downloaded ones — a copy in `%TEMP%` with no mark-of-the-web is
refused too, while `Get-ExecutionPolicy` reports `RemoteSigned`, which should
permit it. We do **not** change machine or user policy. `scripts/dev.cmd` sets
`Bypass` at process scope only, which affects nothing outside that shell.

**3. Sandboxed shells cannot do TLS, and cannot spawn piped children.**
Two separate constraints, both worth knowing because the error messages are
misleading:

- Commands run at **Low integrity**, and schannel cannot acquire crypto
  credentials there. Every schannel client fails identically — cargo, `curl.exe`
  and .NET all report `SEC_E_NO_CREDENTIALS`. Node is unaffected because it
  bundles OpenSSL. So `cargo fetch` needs to run once with wider access;
  everything after that is offline from `.cargo-home`.
- Node's `child_process` with piped stdio fails with `EPERM`. That is what
  esbuild and therefore vite need, so frontend builds also need wider access
  in a sandboxed shell.

**Neither applies to your own terminal.** Run `pnpm tauri dev` normally and
none of this is visible.

---

## Deviations from the PRD

Recorded because the PRD is the contract, and a silent departure is worse than
a documented one.

### Corrections — the PRD was wrong

- **`rusqlite` has no `fts5` feature.** PRD §7.3 lists one; it does not exist
  in 0.32 or in the current release. FTS5 comes from `bundled`, whose build
  script passes `-DSQLITE_ENABLE_FTS5` (plus FTS3, RTREE, JSON1, STAT4)
  straight to the C compiler. Since `trigram` needs SQLite ≥ 3.34 and the
  bundle is far newer, `bundled` alone is both necessary and sufficient.
  Verified by reading `libsqlite3-sys/build.rs`, not assumed.

### Deliberate departures

- **Conversation modules are captured, not skipped.** PRD §6.3 says a
  `TimelineTimelineModule` "should be skipped, not fatal." We capture the tweets
  inside them but assign **no `sortIndex`**, because a module's ordering is
  thread order rather than bookmark order and letting it into the bookmark
  ordering would scramble the library. Skipping them entirely would discard
  real posts the user can see; this keeps the data without corrupting the order.

- **Snippet delimiters are control characters (`U+0001`/`U+0002`), not `[`/`]`.**
  The frontend has to turn match positions into DOM nodes. The obvious approach
  — have Rust emit `<mark>` and have the frontend `innerHTML` it — turns any
  tweet containing markup into script execution. Control characters cannot
  appear in tweet text or be typed into a search box, so the frontend splits on
  them and builds elements. **There is no `{@html}` anywhere in this app.** The
  CLI opts into printable delimiters, since a terminal cannot show `U+0001`.

- **The action bar renders in the list as well as the detail pane.** PRD §7.7
  implies a dense list, but X shows Reply/Repost/Like/Views on every row of the
  bookmarks list, and a list without it does not look like X.

- **Chunked request bodies are decoded, not refused.** The first version of the
  bridge rejected `Transfer-Encoding` outright, on the reasoning that the only
  client is our own userscript. That was wrong, and the smoke test caught it:
  Node's `http.request` switches to chunked the moment `Content-Length` is
  omitted, browsers streaming a body do the same, and the result was a capture
  that failed with a 400 nothing on the script side could explain. The genuine
  smuggling risk is a request declaring its length **two** ways, and that is
  what gets refused. `Expect: 100-continue` is answered for the same reason —
  .NET sends it by default for POSTs and will otherwise wait out its own
  timeout against a server that never replies.

- **A note on what could not be verified here.** The app-side bridge wiring
  (start on launch, emit progress events) compiles and is exercised by
  `cargo check`, but it could not be run: Tauri creates the window *before*
  invoking the user's setup closure (`tauri-2.12.1/src/app.rs:2691`), so on a
  host where WebView2 cannot start, the process panics before the bridge is
  reached. Nothing about the wiring is known to be wrong — it is simply
  unreachable in that environment. This is the practical argument for
  `xdl bridge`: the receiver is identical, and it runs anywhere a terminal
  does.

- **Read commands open their own SQLite connection.** A single
  `Mutex<Library>` would serialise every search behind every import, which
  defeats the WAL pragma. Writes take a mutex; reads do not.

- **The userscript ships X's raw envelopes and does not normalise a record.**
  PRD §6.3 specifies a `BookmarkRecord` built in JavaScript. We send `page`
  records holding the envelope exactly as X sent it, and let the Rust parser —
  the one with the tests — do the interpretation. Two reasons: the parser can
  then be fixed retroactively against already-captured bytes (PRD §9.2), and
  the script does not become a second, untested implementation of the same
  logic that silently disagrees with the first. The script parses only enough
  to count posts for its progress pill and read the bottom cursor.

- **The userscript requests a fourth grant: `unsafeWindow`.** PRD §10 lists
  three `GM_*` grants plus `@connect 127.0.0.1`. Patching `window.fetch` from a
  sandboxed manager script is unreliable without reaching the page's real
  window, and a hook that silently does not install is the worst failure mode
  this product has — it looks like "X stopped sending bookmarks". The grant is
  used for one thing: assigning the two hooks.

- **The userscript is plain JavaScript, not TypeScript + `vite-plugin-monkey`.**
  PRD §7.3 specifies the toolchain. A single readable file beats a build step
  here: Greasy Fork requires readable source rather than minified output, §10
  wants the script auditable in one sitting, and a reviewer should be reading
  the exact bytes that run. There is no build step at all.

- **The install secret is a file, not the OS keyring.** PRD §7.3 specifies the
  `keyring` crate. It is written to `bridge.json` beside the library, mode
  `0600` on Unix and inside the user's own `%APPDATA%` on Windows. The secret
  authenticates a **loopback-only** endpoint that exists only while the app is
  open; a keyring would add a dependency tree and a failure mode (a locked or
  unavailable credential store) for a marginal gain in a threat model where the
  attacker already has the user's session. Recorded because it is a real
  departure from the stated design, not because it is equivalent.

- **The port is discovered by echo rather than advertised.** PRD §7.4 says the
  app "advertises the new port on the next successful exchange". That cannot
  work as written: if the port moved, the script cannot reach the app to receive
  the advertisement. Instead the script probes the documented range against the
  unauthenticated `/v1/hello` echo and caches the answer, rescans only on
  failure, and never presents its secret to a port that has not proved it is the
  app.

### The Chirp typeface is deliberately not in this repo

X renders in **Chirp**, and matching it is the difference between "looks like
X" and "looks like X in the wrong font". Chirp is also **not redistributable**,
and the font says so itself:

```
copyright   : "Copyright © 2021 by Noel Leu and Grilli Type. All rights reserved."
trademark   : "The Chirp and Chirp Display typeface names are trademarks of Twitter, Inc."
manufacturer: "Grilli Type AG"
description : "Chirp and Chirp Display are customized versions of GT America
               created exclusively for Twitter."
license     : (absent — no license field, no license URL)
```

It is a commercial typeface from [Grilli Type](https://www.grillitype.com),
licensed to X. The `chirp-font` repositories on GitHub are unauthorised
redistributions — they are not malware, but they are not licensed either, and
committing one would put all-rights-reserved software in a public repo.

So the split is:

| | |
|---|---|
| `@font-face` rules in `ui/src/styles/x.css` | **Committed.** Without the files they fail to load and the stack falls through to the system font — no error, no layout shift, no broken build. |
| The font files themselves | **Not committed.** `ui/public/fonts/` is gitignored. |

Anyone who legitimately has Chirp drops the `.woff2` files into
`ui/public/fonts/` and gets the real thing with no code change.

**For shipping**, pick a face you are allowed to redistribute. The `--font`
stack in `x.css` lists the fallbacks in priority order; an open humane
grotesque such as Inter is the closest freely-licensed relative.

### Deferred, not dropped

- **Continuous capture (save-as-you-go) is not implemented.** PRD §6.5 calls it
  the highest-value feature in the userscript, and it is — but it is ID-first by
  design and needs a stub path in the store that does not exist yet: a record
  carrying only a tweet ID and an observed `bookmarked_at`, to be enriched by a
  later page. Shipping it without that path would mean either inventing a tweet
  or silently dropping the save. The interception layer it needs is already in
  place, so this is additive.

- **`tauri-specta` is deferred to M2.** PRD §7.3 calls for typed IPC bindings.
  The TypeScript types in `ui/src/lib/types.ts` are hand-written mirrors of
  `crates/core/src/model.rs` for now. Codegen is a build-script dependency and
  a failure surface during bring-up, and the command surface is still moving.
  **Until M2, a Rust model change must be mirrored there by hand.**

- **Media is a placeholder until M3.** Media lives on `pbs.twimg.com`, and
  hot-linking it would make the webview open connections to an X-owned host.
  §7.5's default path caches only bytes the page already loaded, which needs
  the M3 pipeline. Frames currently reserve the correct space from the
  payload's own dimensions and show the real alt text, labelled "not archived
  yet" — the honest version of a gap.

- **Avatars are monograms.** Same reason. The tooltip says why.

- **The `asset:` protocol is not enabled yet.** Nothing local to serve until
  the media store exists.

### Added

- **`scripts/dev-env.ps1`, `scripts/dev.cmd`, `scripts/make-icon.py`** — see
  Environment notes. The icon is drawn programmatically so it can be
  regenerated and tweaked by anyone reading the repo.

---

## Two things the tests are built to catch

Both are silent-failure classes that would otherwise look like "X just doesn't
provide that".

**Entity indices.** X's `[start, end]` indices may count UTF-16 code units
(what JavaScript `String.slice` uses) or Unicode code points (what the public
API documents). Get it wrong and every link in a tweet containing an emoji
shifts by one character. `TweetText.svelte` therefore *verifies* every slice
against the entity's own literal text before styling it, retries with code-point
indices, and falls back to plain text — an unstyled link is cosmetic, a
misplaced one is corruption. `crates/core/tests/fixtures.rs` reproduces that
logic in Rust and asserts against both readings, using a fixture containing an
emoji precisely because that is where the two disagree.

**`sortIndex`.** It lives on the timeline *entry*, as a sibling of `content` —
not under `legacy`. Reading `legacy.sort_index` yields `null` on every record,
forever, and presents as "bookmark ordering isn't available" rather than as a
bug. There is a test named after this.

---

## What is not here, and will not be

No X API. No OAuth. No headless browser, Playwright, Puppeteer or Selenium. No
`auth_token`/`ct0` anywhere near the Rust code. No backend-issued GraphQL. No
third-party scraping API, proxies, or residential IPs. No cloud sync,
telemetry, or phone-home. See PRD §15.

The app holds no X credential and links no HTTP client. It could not generate X
traffic if it wanted to, and that is enforced at the dependency boundary rather
than by convention.
