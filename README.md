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
| `crates/core` — schema, parser, importers, search | ✅ 89 tests passing |
| `crates/cli` — `xdl import / search / show / stats` | ✅ working |
| `ui/` — Svelte 5 frontend, three X themes | ✅ builds |
| `crates/app` — Tauri v2 shell | ✅ builds |
| Capture userscript | ⬜ next |
| Loopback receiver + pairing | ⬜ next |
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

---

## Layout

```
crates/core/     schema, X payload parsing, import, search. No Tauri dependency.
crates/cli/      `xdl` — dogfood the pipeline without a GUI.
crates/app/      Tauri v2 shell: window, typed IPC commands.
ui/              Svelte 5 frontend.
fixtures/        Canonical test payloads, shared by Rust tests and the UI.
scripts/         dev-env.ps1, dev.cmd, make-icon.py.
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

- **Read commands open their own SQLite connection.** A single
  `Mutex<Library>` would serialise every search behind every import, which
  defeats the WAL pragma. Writes take a mutex; reads do not.

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

### Deferred, not dropped- **`tauri-specta` is deferred to M2.** PRD §7.3 calls for typed IPC bindings.
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
