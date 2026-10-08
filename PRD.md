# PRD: **xitter-dl** — a local-first X bookmarks library, with zero-cost companion capture

**Status:** Synthesized design. Merges the strongest parts of four prior drafts (two written before the "no X API" constraint, two after) into one buildable spec.
**Hard constraint:** **$0 marginal cost.** The X API is out — no paid tier, no pay-per-use credits, no third-party scraping API, no proxies. Ingestion is browser-side only.
**Stack:** Tauri v2.x, minor pinned (3.0 alphas are real and current — `v3.0.0-alpha.4` — but stay on 2.x) · Svelte 5 frontend · SQLite + FTS5 · companion userscript · fully offline after ingest.

---

## 0. What this document is

Four drafts exist. Two predate the no-API decision and are now partially obsolete; two postdate it. This consolidates them:

| From | Kept | Dropped |
|---|---|---|
| **Draft #1** (opus, API era) | PostgreSQL-grade data model shape, FTS5 + `sqlite-vec` + RRF search stack, `raw_json` future-proofing, media cache politeness, perf targets, security posture, "why this demos well" | Official API ingestion, OAuth/PKCE, Rate Governor against API endpoints, cost model, BYO developer app |
| **Draft #2** (fable, API era) | The `crates/core` + `crates/cli` + `crates/app` workspace split, typed IPC via `tauri-specta`, gyotaku-calibrated performance table, CLI-for-dogfooding, SHA-256-pinned model download, "removals are flagged, never deleted" | The entire Channel A API design, Owned Reads pricing, billing-ledger open questions |
| **Draft #3** (opus, no-API) | Userscript architecture in full, XHR/fetch interception, GraphQL parsing, transport, complete SQLite DDL, live ingest UX, continuous capture, schema-drift recovery, risk table | Nothing structural — it is the engineering backbone |
| **Draft #4** (fable, no-API) | The threat model, the risk-tier ladder, assisted-scroll mitigation suite, pagination-wall honesty, loopback pairing design, in-session media fetch idea, operation-name matching | — |

**How to read it:** decisions carry an inline note when they rest on something unmeasured — `[Unverified — measure in M0]`. §14 lists the open questions and, separately, the ones that have since been **closed**; do not relitigate a closed one without new evidence. Where this document states a number X does not publish, it says so rather than inventing a source.

---

## 1. The problem

X gives you no way to live with your own bookmarks.

- **There is no export.** Not in the app, not in Settings, and — the cruelty of it — not in the official data archive either. The archive includes your likes. It excludes your bookmarks. This gap has existed for years, developers have asked X to close it repeatedly, and as of 2026 nothing has changed.
- **Bookmarks are write-only memory.** You saved it because it mattered. You will never find it again.
- **The paid path is gone by constraint, and was capped anyway.** The X API's bookmarks endpoint has historically capped around 800 items, and the free tier no longer exists for new developers.
- **The web app itself has a practical ceiling.** The bookmarks page stops surfacing older items somewhere around 800–1,000 saves. `[Unverified — measure in M0]` Reports vary and the wall may be inconsistent or folder-dependent.

**The product thesis:** a fast, offline, keyboard-first library of your bookmarks — ingested through a channel that originates **zero additional X requests**, because passive capture only ever files away responses X already sent.

**The reference north star** is [`xevrion/gyotaku`](https://github.com/xevrion/gyotaku) — search every screenshot you've ever taken, native, offline, SQLite-backed. Worth knowing: it is *not* Tauri, it's Rust + gpui (Zed's UI framework). Tauri is still the right call here because tweets are rich text, media, embeds and links — exactly what a webview renders well.

One honest caveat about benchmarking against it: **gyotaku is gpui, we are a webview.** Its numbers are a direction of travel, not a commitment. Search latency is genuinely portable — a Rust + SQLite FTS5 query costs the same either way — but window summon and idle memory are dominated by the webview runtime, not by our code, and will land wherever WebView2 / WKWebView / WebKitGTK put them.

| gyotaku's published numbers | Our stance |
|---|---|
| Search: 1–10 ms per keystroke over 5,000+ items | **Carried over as a real target:** ≤ 10 ms over 10k bookmarks. This one we will hit. |
| Window summon: ~120 ms | **Not carried over.** Set from an M0 baseline, per platform. Webview cold start is not a number we get to choose. |
| Idle memory: 37 MB | **Not carried over.** Same: baseline WebView2 / WKWebView / WebKitGTK in M0, then set a target that's honest for each. |

---

## 2. Goals & non-goals

### Goals

1. **Ingest bookmarks without getting flagged or rate-limited.** Passive mode originates zero additional X requests; assisted scroll is automation, and §5 labels its risk honestly rather than claiming it away.
2. **No paid API, no recurring cost, no account to create.** Except the X account you already have.
3. Store everything **locally** in a single SQLite file. Work **fully offline** after ingest.
4. **Instant search** — sub-10 ms keystroke latency, full-text + structured filters, optional semantic search.
5. **Bookmarks outlive X.** Cached text, media and author snapshots persist even if the post is deleted or the account is gone.
6. Rich browsing: media, author, date, thread context, quoted posts, folders, user-owned tags and notes.
7. A native-feeling, beautiful UI worth showing off on X.

### Non-goals (v1)

- **Any X API usage.** No official API, no OAuth, no API keys, no paid tier. This is a product principle, not a phase-2 item.
- **Headless/CDP browser automation.** No Playwright, Puppeteer, Selenium, no cookie reuse (`auth_token`/`ct0`) in the Rust backend. Hard product principle — see §4.
- **Writing to X.** No posting, liking, replying, bookmarking remotely. Read-only.
- **Cloud anything.** No sync service, no telemetry, no crash reporting, no accounts.
- Multi-account, multi-device, mobile. Defer all three.

---

## 3. The one insight the whole design rests on

> **Don't drive the browser. Ride along in it.**

The ecosystem's proven tools all work the same way, and it's the single most important prior art in this document. [`prinsss/twitter-web-exporter`](https://github.com/prinsss/twitter-web-exporter) states it plainly: *the script itself does not send any request to the Twitter API. It installs a network interceptor to capture the response of GraphQL requests initiated by the Twitter web app.* It has operated this way for years, in public, on Greasy Fork, without mass bans.

That model inherits four things for free — the exact four things automation frameworks burn enormous effort faking badly:

1. The user's **real browser** and real fingerprint.
2. The user's **real session** and cookies.
3. The user's **real IP** and network reputation.
4. The user's **real pacing**, because a human is doing the scrolling.

Consequently: **in passive mode the app is not a scraper. It is a library that happens to receive data.** The Rust backend never holds an X credential, never opens a socket to an X-owned host, and could not generate X traffic if it wanted to. That property is the moat, and it is worth protecting architecturally — see §7.4.

**Say it precisely, though.** "Ride along" is a claim about *provenance of requests*, not a claim of zero risk: passive capture originates zero additional X requests, and the residual risk is only whatever the user's own scrolling would have carried anyway. Assisted scroll (§6.4) is automation — real, if small, additional risk — and it is opt-in precisely because of that. Whenever this document describes passive mode, it should say *zero additional requests*, never *zero risk*.

---

## 4. Threat model

Everything downstream flows from this section.

### 4.1 What X actually detects

- **Cloudflare WAF + Turnstile.** X migrated from in-house bot detection to Cloudflare's Turnstile on login walls and rate-limited endpoints. Unlike a puzzle CAPTCHA, Turnstile runs browser fingerprinting and behavioral analysis silently and fails bot sessions *before* they reach content. Naive scrapers don't get blocked, they get nothing.
- **TLS and browser fingerprinting.** Automation frameworks leave detectable fingerprints. Headless Chromium, a fresh profile every run, and a timezone that disagrees with the IP all read as automation, because detection cross-checks fingerprint against network signals.
- **Behavioral analysis, not just quotas.** X does not publish numbers for likes, bookmarks or search. These run through detection that flags "unusual activity" rather than enforcing a fixed count. What trips it: regular timing (same intervals, same times of day), clicks exceeding human speed, visiting many profiles in succession, refreshing too often, scrolling fast through long result sets.
- **Network signals that compound.** Datacenter or VPN IPs, several accounts behind one IP, VoIP or missing phone numbers, location jumps between sessions, sudden high volume from a new account. These stack — a VPN plus a new account plus high volume is a different risk profile than any one alone.
- **No published read cap to design against.** The widely-repeated "~1,000 reads/day for unverified accounts" figure traces back to the July 2023 emergency rate limits, not to current documented policy. **Do not treat it as a spec.** Design conservative budgets as a product choice (§6.4) and react to what the platform actually does — 429s and challenge screens — rather than to a number X isn't currently publishing.

### 4.2 Why passive capture is safe

The detection stack above is built to catch *requests that shouldn't exist*. Passive capture originates none. The user's scrolling triggers the pagination fetches it always would have; our script reads the response bodies after the fact and files them locally. There is no bot signature to detect because there is no bot.

The blunt version: **nobody gets banned for scrolling their own bookmarks.**

Scope note: that argument covers **passive capture only**. Assisted scroll synthesizes scroll position, which is a (small, bounded, opt-in) automation signal — see §5 and §6.4.

### 4.3 The line we do not cross

The moment the Rust backend holds the user's `auth_token`/`ct0` cookies and speaks GraphQL directly, we have built a scraper that uses the user's account as ammunition — and we have taken on the full detection surface of §4.1. The userscript boundary keeps every byte of X traffic inside the user's genuine browser, genuine session, genuine fingerprint, genuine IP. **This boundary is architectural, not stylistic.** See §4.5.

---

## 5. The ingestion ladder (explicit risk tiers)

Three tiers. Tiers 1 and 2 ship in v1. Tier 3 is opt-in, budgeted, and clearly labeled. **There is no Tier 4.**

| Tier | What it does | Extra requests to X | Risk |
|---|---|---|---|
| **1. Passive capture** *(default)* | Userscript hooks `fetch`/XHR, records `Bookmarks` GraphQL responses the page **already** made while the user scrolls normally | **Zero** | Lowest available — the request pattern is the user's own browsing |
| **2. File import** | Parse exports from `twitter-web-exporter`, `xarchive`, HAR dumps, or generic CSV | Zero | Zero — no X interaction at all |
| **3. Assisted scroll** *(opt-in)* | Userscript scrolls *for* the user, in their real visible tab, at human pace, inside a hard budget | Only the pagination requests normal scrolling would have triggered | Non-zero. It is automation, however gentle, and it is opt-in for that reason. Full mitigation suite in §6.4 |
| ~~**4. Headless / CDP automation**~~ | Playwright/Puppeteer/CDP, cookie reuse, backend-issued GraphQL | — | **Never. Hard product principle.** |

The ladder exists so users can make an informed choice, and so Tier 1 can always be the honest default. **Tier 1 alone is a complete product** — Tier 3 is a convenience for people who don't want to flick a trackpad 400 times, not a requirement.

---

## 6. Component 1 — the companion userscript

`xitter-dl-capture.user.js`, shipped for Tampermonkey/Violentmonkey. Published standalone on Greasy Fork for reach **and** bundled with the app for one-click install with the bridge pre-configured. Both, not either — standalone publishing is what built the trust that makes the prior art acceptable.

### 6.1 Interception, not DOM scraping

Wrap `window.fetch` and `XMLHttpRequest` **before X's bundle loads** (`@run-at document-start`). Match response URLs against the GraphQL operation name, clone and parse the JSON.

Two properties this buys us that DOM scraping cannot:

- **The full tweet objects.** `note_tweet` (long-form posts), media variants with bitrates, quoted-tweet payloads, card/link data, expanded URLs, exact `sortIndex`. Console scripts that read `article[data-testid="tweet"]` nodes get only visible text and lose all of it.
- **Independence from the rendering layer.** No brittle selectors, no scroll-triggered layout coupling.

**Interception is the record path only.** It is not how anything leaves the page — see §7.4 for why every handoff goes through `GM_xmlhttpRequest`.

The canonical hook shape (`isBookmarkOperation` is defined in §6.2, `handlePayload` in §6.3):

```typescript
// @run-at document-start — patch before X's bundle executes
const originalOpen = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function (method: string, url: string, ...rest: any[]) {
  (this as any).__bvUrl = url;
  return originalOpen.apply(this, [method, url, ...rest] as any);
};

const originalSend = XMLHttpRequest.prototype.send;
XMLHttpRequest.prototype.send = function (...args: any[]) {
  this.addEventListener('load', function () {
    try {
      const url: string = (this as any).__bvUrl ?? '';
      if (isBookmarkOperation(url)) handlePayload(JSON.parse(this.responseText));
    } catch { /* never break the page */ }
  });
  return originalSend.apply(this, args as any);
};

// fetch() is what X actually uses today — patch it too, and clone before reading
const originalFetch = window.fetch;
window.fetch = async function (input: RequestInfo | URL, init?: RequestInit) {
  const response = await originalFetch.apply(this, [input, init] as any);
  try {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (isBookmarkOperation(url)) {
      // clone() so the page's own consumer still gets a readable body
      response.clone().json().then(handlePayload).catch(() => {});
    }
  } catch { /* never break the page */ }
  return response;
};
```

**Two non-negotiables in this code:** always `response.clone()` before reading, and never let a parse error escape. A capture script that breaks the bookmarks page is worse than no capture script.

### 6.2 Matching on operation name, never on query ID

X rotates GraphQL `doc_id` hashes, guest tokens, rate limits and detection patterns on a **2–4 week cadence**. Hardcoding a query ID means a broken exporter every few weeks.

We never construct requests, so `doc_id` rotation is irrelevant to us — but we do have to *recognize* responses. Match on the operation name in the URL path instead of the hash:

```typescript
// e.g. /i/api/graphql/<rotating-hash>/Bookmarks
const BOOKMARK_OPS = /Bookmarks|BookmarkFolders|BookmarkSearch|CreateBookmark|DeleteBookmark/;
const isBookmarkOperation = (url: string) =>
  url.includes('/i/api/graphql/') && BOOKMARK_OPS.test(url);
```

Keep the regex tolerant and update it in one place. This is the single highest-leverage resilience decision in the userscript.

### 6.3 Parsing X's timeline envelope

Bookmark responses arrive nested inside a timeline instruction structure. `[Unverified — validate against live payloads in M0]` Shape as last observed:

```
data.bookmark_timeline_v2.timeline.instructions[]
  └─ type: "TimelineAddEntries"
       entries[]
         ├─ sortIndex                                       ← ENTRY level, sibling to content
         ├─ content.entryType === "TimelineTimelineItem"
         │    └─ itemContent.tweet_results.result
         │         ├─ __typename: "Tweet"                      → use directly
         │         └─ __typename: "TweetWithVisibilityResults" → use .tweet
         └─ content.entryType === "TimelineTimelineCursor"
              └─ content.cursorType === "Bottom" → content.value is the next-page cursor
```

**`sortIndex` lives on the entry, not on the tweet.** It is a sibling of `content`, not a field under `legacy`. Reading `legacy.sort_index` yields `null` on every single record, silently and forever — the kind of bug that looks like "bookmark ordering just isn't available" rather than like a mistake. Read it at `entries[].sortIndex`.

The parser must:

1. Walk `instructions[]`, keep only `TimelineAddEntries`.
2. Extract `tweet_results.result`, unwrapping `TweetWithVisibilityResults` → `.tweet`.
3. Pull `note_tweet.note_tweet_results.result.text` when present — that's the real text of a long post, not the truncated `full_text`.
4. Read `legacy.extended_entities.media` (fall back to `entities.media`) for photos, video variants and alt text.
5. Read `entries[].sortIndex` (entry level) as the bookmark ordering key.
6. **Skip top cursors, capture bottom cursors** — the bottom cursor is the pagination handle and the honest progress signal.
7. Tolerate unknowns everywhere. A `TimelineTimelineModule` (conversation module) should be skipped, not fatal.

**There is no bookmark timestamp anywhere in the payload.** X does not expose when a bookmark was saved. `bookmarks.bookmarked_at` is therefore **permanently null for anything captured from a backfill scroll** — no field in the GraphQL envelope carries it, and no amount of parsing will conjure it. It is populated only by continuous capture (§6.5), where *we* observe the save happen and stamp it ourselves. Keep the column nullable and label it in the UI as "saved (observed)" so it is never mistaken for authoritative — and never sort the library by it, because it will be null for most of the oldest and most valuable rows.

Normalize into a flat record. Note that `folder_id` is deliberately **not** on the record: a bookmark can sit in multiple folders, so folder membership is a join table many-to-many (§7.6), not a column.

```typescript
interface BookmarkRecord {
  id: string;                 // snowflake
  text: string;               // note_tweet text if present, else full_text
  author: { id: string; handle: string; name: string; avatar_url: string };
  created_at: string;         // ISO 8601
  lang: string | null;
  conversation_id: string | null;
  in_reply_to_id: string | null;
  media: { url: string; type: 'photo' | 'video' | 'gif'; width: number; height: number; alt: string }[];
  metrics: { likes: number; retweets: number; replies: number; views: number };
  urls: { display: string; expanded: string }[];   // expanded_url from the payload — never unfurl t.co ourselves
  card: object | null;        // link preview, straight from the payload
  folder_ids: string[];       // folders this capture pass observed it in; [] from the main bookmarks timeline
  sort_index: string | null;  // entries[].sortIndex, entry level
  captured_at: string;        // ISO 8601, when we intercepted it
  source_version: number;     // which parser version produced this normalization
  raw_tweet: object;          // the tweet_results.result subtree ONLY — not the page envelope
}
```

**Two storage rules follow from the shape of the data**, both enforced in §7.6:

- `raw_tweet` holds the **per-tweet `tweet_results.result` subtree only.** The full GraphQL page envelope is stored **once per captured page** in `capture_payloads`, with posts referencing it. Duplicating a 200-tweet envelope into 200 post rows wastes an order of magnitude of disk for zero benefit.
- `bookmarked_at` is nullable and usually null. Document it as continuous-capture-only.

### 6.4 Assisted scroll — "Careful Mode" is the only mode

For users who don't want to scroll 400 times by hand. The crucial distinction from automation: this runs **in the user's visible, foreground, real browser tab**, in their real session. The only thing synthesized is scroll position — no clicks, no likes, no follows, no synthetic XHR.

All of the following are non-negotiable defaults:

- **Human-shaped pacing.** Scroll increments sampled from a distribution (300–900 px), inter-scroll delay 1.5–4 s with jitter, occasional longer "reading pauses" (5–15 s every 8–20 scrolls). Never periodic, never machine-speed. Volume *and pace* matter — machine-speed scrolling trips limits and then human review.
- **Hard session budget.** Stop after **~600 newly captured bookmarks or 20 minutes**, whichever comes first. Resume later; resumable sync makes this painless. **This is a product choice, not compliance with a published cap** — X is not currently publishing a read limit we could cite (see §4.1). The budget exists because a bounded session is easy to reason about and easy to stop; it is deliberately modest, and if the platform never pushes back it costs the user nothing but an extra sitting.
- **Tripwires, and they are the *only* signal we trust.** On any 429, any `KeepAlive`/error overlay, or X's "rate limit exceeded" toast → **stop instantly**, back off for the session, tell the user plainly. Never auto-retry. Since there is no published number to pace against, observed platform behavior *is* the policy — and one 429 should permanently lower the pacing profile for that account.
- **Attention guard.** Pause on `visibilitychange` when the tab loses focus. Polite and realistic.
- **Cooldown ledger.** The app tracks captured-per-day and refuses to start an assisted session if recent volume is high.
- **No parallelism, ever.** One tab, one session, one account.
- **Pre-flight risk warning.** If the user is on a VPN, a shared IP, or a new account, say so before they start and recommend passive mode.

### 6.5 Continuous capture (save-as-you-go) — ID-first by design

When the user bookmarks something anywhere on X, we want it in the library immediately. The naive implementation watches the `CreateBookmark` response for the tweet — **and gets nothing**, because `CreateBookmark` returns a status stub. The response confirms success; it does not contain the post.

**The tweet ID is in the request variables, not the response.** So the design is ID-first, not response-first:

1. **Read the request body.** Intercept the `CreateBookmark` *request*, parse its `variables`, and take the `tweet_id`. This is the one place we read something other than a response body, and it costs nothing — the request is already in flight.
2. **Try to enrich from ambient context.** The user almost always bookmarked from a page that was already rendering that post. The `TweetDetail` response, or the timeline response, that we intercepted moments earlier for that ID gives us the full record with zero extra work. Cache intercepted tweets by ID for a short window so this hit rate stays high.
3. **Otherwise queue for the next backfill pass.** An ID with no ambient record becomes a stub row — ID, `bookmarked_at` (which we *can* stamp here, since we observed the save), source, nothing else — and gets enriched on the next visit to the bookmarks page, or by any later capture that happens to include it.
4. **A stub is a valid state.** The UI should render it as "saved 3 minutes ago — details pending" rather than hiding it. Showing the user their save landed is worth more than showing them a complete record immediately.

Why this matters beyond niceness:

- Over months, continuous capture **eliminates the need for bulk scrolling entirely** for everything new — the backfill problem stops growing while you work through it.
- It is the **only** source of real `bookmarked_at` values (see §6.3). Everything from a bulk scroll has a null save time; everything from here has a true one.
- It fires only when the app is running and paired; otherwise the ID is buffered to `GM_setValue` storage and flushed on the next handoff.

This is the highest-value feature in the userscript and it is nearly free once the interception layer exists — but only if it is built ID-first from the start. Retrofitting it after building a response-parser is a rewrite.

### 6.6 The pagination wall — design around it honestly

`[Unverified — measure in M0]` The web app practically stops surfacing at roughly 800–1,000 saves. Older bookmarks remain on the account but stop appearing as you scroll. Reports vary, and `twitter-web-exporter` advertises export "without the max 800 limit," so the wall may be inconsistent or folder-dependent.

Four product responses:

1. **Measure, don't assume.** The M0 spike is a real >1k-bookmark account, logging exactly where pagination cursors die.
2. **Folder workaround.** Bookmark folders paginate independently. Users with foldered old bookmarks can recover them per-folder. This is the primary sanctioned workaround.
3. **Continuous capture moves the wall.** Once save-as-you-go is running, the wall stops mattering for anything new.
4. **Eventually the wall stops mattering at all.** Once captured, bookmarks live in the local DB forever. The wall only limits the *first* backfill of ancient items.

Surface this plainly in the UI: *"X stops showing bookmarks older than about 1,000. You've captured everything X will show you (1,043 of an unknown total)."* Honesty here costs nothing and buys enormous trust.

---

## 7. Component 2 — the app

### 7.1 Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  User's real browser (their profile, their session, their IP)     │
│  ┌────────────────────────────────────────────┐                  │
│  │  x.com/i/bookmarks + xitter-dl script      │                  │
│  │  ┌──────────────┐  ┌────────────────────┐  │                  │
│  │  │ fetch/XHR    │  │ Safety Governor    │  │                  │
│  │  │ interceptor  │  │ • budget ledger    │  │                  │
│  │  └──────┬───────┘  │ • tripwires        │  │                  │
│  │         │ records  │ • cooldown ledger  │  │                  │
│  │         ▼          └────────────────────┘  │                  │
│  │  ┌──────────────────────────────┐          │                  │
│  │  │ NDJSON buffer                │          │                  │
│  │  │ GM_xmlhttpRequest POST ──────┼─ http://127.0.0.1:<port> ──┐ │
│  │  │ (extension context, not page)│  bearer install secret     │ │
│  │  └──────────────────────────────┘          │                │ │
│  └────────────────────────────────────────────┘                │ │
│                                                                ▼ │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │  xitter-dl (Tauri v2)                                      │   │
│  │  Rust core                    Frontend (webview)           │   │
│  │  ├─ loopback receiver         ├─ virtualized list          │   │
│  │  ├─ validate → dedupe → store ├─ search bar (FTS5)         │   │
│  │  ├─ rusqlite + FTS5 + vec     ├─ filter sidebar            │   │
│  │  ├─ fastembed (ONNX, local)   ├─ detail pane               │   │
│  │  ├─ media store (content-addr)├─ live capture panel        │   │
│  │  └─ events → progress UI      └─ command palette           │   │
│  └───────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
   One-way by construction: the app binds loopback and receives.
   It never dials out to any X-owned host, and never holds a credential.
```

### 7.2 Workspace layout

Mirrors gyotaku's CLI+app split — a pattern that pays for itself immediately, because you can dogfood the whole pipeline from a terminal before the GUI exists.

```
crates/core   – schema, importers, parsers, search, embeddings. No Tauri dependency.
crates/cli    – `xdl import bookmarks.ndjson && xdl search "rust gui"` — dogfooding + CI
crates/app    – Tauri shell
```

`crates/core` having no Tauri dependency is what makes the CLI, the test suite, and the importers trivially testable.

### 7.3 Tech choices

| Layer | Choice | Why |
|---|---|---|
| Shell | Tauri v2.x (pin the minor; 3.0 alphas are real and current — `v3.0.0-alpha.4` as of Oct 1 — but stay on 2.x) | Rust backend, ~10 MB binaries, real capabilities/permissions ACL |
| Frontend | Svelte 5 (runes) or SolidJS; React 19 if you're faster in it | Small, compiles to few DOM nodes, handles 10k-row virtualized lists without ceremony |
| Virtualization | TanStack Virtual | 10k+ rows at 60 fps |
| IPC | `tauri-specta` | Typed command bindings — frontend stays a dumb renderer |
| DB | `rusqlite` (bundled, `fts5` feature) + `sqlite-vec` | Direct control over pragmas, WAL, FTS5 tokenizers |
| Async | Tokio | Loopback server, media pipeline, background embedder |
| Embeddings | `fastembed-rs` (ONNX, local) | `bge-small-en-v1.5` or `all-MiniLM-L6-v2`, 384 dims, zero cloud |
| Userscript | TypeScript, bundled with `vite-plugin-monkey` | Same toolchain as the prior art |
| Secrets | `keyring` crate | The persistent install secret (and nothing else) — **never an X credential** |
| Userscript HTTP | `GM_xmlhttpRequest` with `@connect 127.0.0.1` | Extension context bypasses page CSP, CORS, mixed-content and LNA — see §7.4 |

**All business logic lives in Rust.** The frontend renders typed commands and events; it owns no state machine, no sync logic, no parsing.

### 7.4 Transport — userscript → app

**The page-context `fetch()` route is dead on arrival. Do not design around it.**

Two independent blocks, either one fatal:

- **x.com's CSP `connect-src` blocks it today.** Not a future risk to hedge against — a present one. A `fetch()` from page context to `http://127.0.0.1:<port>` is refused by the page's own policy before it leaves.
- **Chromium's Local Network Access (LNA) gates it.** Requests from a public origin to a local or loopback address now require explicit user consent, and Chrome 145 adds a dedicated `loopback-network` permission for exactly this. LNA superseded the old Private Network Access preflight model, which had leaned on impractical server-side CORS preflights against local devices — so the old CORS-pinning advice is obsolete on both counts.

(Ironically, this restriction exists to stop malicious pages from port-scanning and attacking local services — which is precisely the threat model our loopback endpoint has to defend against. See §10.)

**The correct route: `GM_xmlhttpRequest`.**

```
// ==UserScript==
// @grant        GM_xmlhttpRequest
// @grant        GM_setValue
// @grant        GM_getValue
// @connect      127.0.0.1
// ==/UserScript==
```

`GM_xmlhttpRequest` runs in the **extension context**, not the page context. It therefore bypasses page CSP, CORS, mixed-content restrictions, and LNA in one move. `@connect 127.0.0.1` is the required grant; without it the manager blocks the request.

Consequences for the design:

- **Delete the `ws://127.0.0.1` option entirely.** A page-context `WebSocket` hits the same CSP and LNA walls, and a script-context WebSocket has no advantage over a plain HTTP POST here. One transport, not two, not "pick in M0."
- **Delete the CORS-pinning line.** CORS is not in play from the extension context, and pinning an origin would be cargo-culting a protection that does not apply.
- **Authentication is the bearer secret alone.** No origin check, no cookie, nothing ambient — an unauthenticated request to the loopback port is rejected outright.

**Pairing: exchange once, persist forever.**

The obvious design — random port plus a single-use token every launch — forces the user to re-pair constantly. That is a bad product, and a secret the user has to keep re-pasting is a secret they will eventually stop reading.

Instead:

1. The app shows a **short-lived pairing code** (rotated on a timer, shown only while the app is running and the pairing panel is open).
2. The user pastes it into the userscript pill **once**.
3. The script exchanges it for a **persistent install secret** — stored via `GM_setValue` on the script side, in the OS keyring via the `keyring` crate on the app side.
4. The port is **persisted** too, with a documented fallback range: the app prefers its stored port, and if it can't bind it, it tries the next free one and advertises the new port on the next successful exchange. The script re-discovers rather than the user re-pairing.

So: one pairing, then it just works across restarts. Re-pairing happens when the user reinstalls, clears script storage, or explicitly revokes — not every time they open the app.

Remaining hardening on the endpoint: loopback bind only, nothing listens unless the app is open, request body size caps, strict schema validation, and rejection of anything without a valid install secret.

**File drop remains the fallback**, not an alternative — it works with zero transport at all when the app isn't running.

**Format: NDJSON**, one record per line, append-only, stream-friendly. Never a single giant JSON array — it can't be appended to or partially recovered. The page envelope is stored once per captured page, not per post (§6.3, §7.6):

```json
{"v":1,"kind":"page","captured_at":"2026-10-08T01:12:44Z","op":"Bookmarks","payload_id":"sha256:9f2c…","raw":{…full GraphQL envelope, once…}}
{"v":1,"kind":"bookmark","captured_at":"2026-10-08T01:12:44Z","payload_id":"sha256:9f2c…","record":{…normalized…},"raw_tweet":{…tweet_results.result subtree…}}
```

### 7.5 Media — what "zero additional requests" actually means

This is where most implementations accidentally leak, **and** where the PRD's own slogan was overreaching. Thumbnails live on `pbs.twimg.com`. The obvious implementation has the Rust backend downloading them, which turns the app into something that talks to an X-owned host on a schedule from a desktop IP. That part is clearly wrong.

But an earlier draft of this section claimed the in-session userscript fetch was free because "the page is already displaying those exact images." **That claim is only true for the exact bytes the page loaded.** Reaching for `?name=orig`, or any variant the page never rendered, is a new request to `twimg`, originated at the script's initiative. It is not "riding along." So the claim gets scoped to what it can actually support:

**Default (v1): capture only what the page already loaded.**

- The userscript collects the **exact blob URLs the page rendered** and fetches those. In practice these are browser cache hits — same URL, same session, moments after the page requested it. Genuinely zero additional requests, and honest to say so.
- If a cache miss turns into a real network fetch, count it in the Safety Governor's budget like any other request. Do not pretend it didn't happen.
- Consequence: some posts will have a thumbnail and no original. That is an acceptable v1 outcome, and the UI says so rather than showing a broken image.

**Opt-in: "Archive originals."** An explicit bulk action that fetches higher-resolution variants the page never rendered. This is **new requests to an X-owned host**, and it is labeled as such in the UI. It keeps the full politeness rules from the old fallback path, because now it genuinely needs them:

- Lazy and budgeted: it runs on an empty queue, only while the app is open, only when the user starts it.
- Concurrency ≤ 3, 150 ms jittered delay between requests, honest `User-Agent`.
- Circuit breaker: on any 429/403, pause all media fetching for 10 minutes and report it.
- Never automatic, never a prerequisite for search working.

**Either way, storage is content-addressed** as `media/<sha256>.<ext>` so re-syncs never re-download, and files are served to the webview through Tauri's `asset:` protocol — so the *webview* makes no network calls regardless of which path produced the bytes.

### 7.6 Data model

```sql
PRAGMA journal_mode = WAL;

-- ── content ────────────────────────────────────────────────────────────
CREATE TABLE authors (
  id            TEXT PRIMARY KEY,
  handle        TEXT NOT NULL,
  name          TEXT NOT NULL,
  avatar_url    TEXT,
  avatar_hash   TEXT,                 -- content address of cached avatar
  verified      INTEGER DEFAULT 0,
  fetched_at    INTEGER
);

CREATE TABLE posts (
  id              TEXT PRIMARY KEY,   -- snowflake
  author_id       TEXT REFERENCES authors(id),
  text            TEXT NOT NULL,      -- note_tweet text when present
  note_text       TEXT,
  created_at      INTEGER,            -- unix
  lang            TEXT,
  conversation_id TEXT,
  in_reply_to_id  TEXT,
  like_count      INTEGER, repost_count INTEGER, reply_count INTEGER, view_count INTEGER,
  raw_tweet       TEXT,               -- tweet_results.result subtree ONLY, never the page envelope
  first_seen_at   INTEGER NOT NULL,
  last_seen_at    INTEGER,
  removed_at      INTEGER,            -- flagged, never deleted; UI says "no longer in your bookmarks"
  source          TEXT NOT NULL       -- 'capture' | 'file_import'
);

-- The full GraphQL page envelope, stored ONCE per captured page.
-- Posts reference it; it is never duplicated into post rows.
CREATE TABLE capture_payloads (
  id            TEXT PRIMARY KEY,     -- 'sha256:<hex>' of the raw body
  op            TEXT NOT NULL,        -- 'Bookmarks' | 'BookmarkFolders' | 'TweetDetail' | …
  captured_at   INTEGER NOT NULL,
  cursor_after  TEXT,                 -- bottom cursor this page ended on, if any
  bytes         INTEGER,
  raw           TEXT NOT NULL         -- the whole envelope, exactly once
);

CREATE TABLE post_payloads (           -- which page(s) a post was seen on
  post_id     TEXT REFERENCES posts(id),
  payload_id  TEXT REFERENCES capture_payloads(id),
  PRIMARY KEY (post_id, payload_id)
);

-- ── bookmark state (user-specific) ─────────────────────────────────────
CREATE TABLE bookmarks (
  post_id       TEXT PRIMARY KEY REFERENCES posts(id),
  bookmarked_at INTEGER,              -- NULL for backfill capture. Continuous capture only.
  sort_index    TEXT,                 -- entries[].sortIndex, entry level
  observed_at   INTEGER               -- when our capture first saw the bookmark
);

-- Many-to-many: a bookmark can live in several folders.
CREATE TABLE folders (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, last_synced_at INTEGER
);
CREATE TABLE folder_posts (
  folder_id TEXT NOT NULL REFERENCES folders(id),
  post_id   TEXT NOT NULL REFERENCES posts(id),
  PRIMARY KEY (folder_id, post_id)
);
CREATE INDEX folder_posts_by_post ON folder_posts(post_id);

-- ── media & links ──────────────────────────────────────────────────────
CREATE TABLE media (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  post_id     TEXT NOT NULL REFERENCES posts(id),
  type        TEXT NOT NULL,          -- 'photo' | 'video' | 'gif'
  url         TEXT NOT NULL,
  preview_url TEXT,
  alt_text    TEXT,
  width       INTEGER, height INTEGER,
  blob_hash   TEXT,                   -- sha256 once cached locally
  bytes       INTEGER
);

CREATE TABLE links (
  post_id   TEXT REFERENCES posts(id),
  tco       TEXT, expanded TEXT, final_url TEXT, domain TEXT, title TEXT
);

CREATE TABLE refs (
  post_id      TEXT REFERENCES posts(id),
  kind         TEXT NOT NULL,         -- 'quoted' | 'replied_to'
  ref_post_id  TEXT
);

-- ── user-owned annotations ─────────────────────────────────────────────
CREATE TABLE tags      (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE);
CREATE TABLE post_tags (post_id TEXT REFERENCES posts(id), tag_id INTEGER REFERENCES tags(id),
                        PRIMARY KEY (post_id, tag_id));
CREATE TABLE notes     (post_id TEXT PRIMARY KEY REFERENCES posts(id), md TEXT, updated_at INTEGER);

-- ── sync/ingest bookkeeping ────────────────────────────────────────────
CREATE TABLE capture_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at INTEGER, finished_at INTEGER,
  kind TEXT,                          -- 'passive' | 'assisted' | 'import' | 'live'
  records_seen INTEGER, records_new INTEGER,
  scrolls INTEGER, minutes REAL,
  last_cursor TEXT, stopped_reason TEXT
);

-- ── search ─────────────────────────────────────────────────────────────
-- External-content FTS5 over a flattened search_docs table.
--
-- Why not contentless (content='')? Two hard blockers:
--   • highlight()/snippet() need the stored text, and contentless tables
--     don't have any — we'd lose match highlighting entirely.
--   • contentless tables reject UPDATE/DELETE without contentless_delete=1,
--     which breaks us precisely when note_tweet text arrives later and a
--     stub post gets enriched. Last-write-wins on a stub is a normal
--     operation here, not an edge case.
--
-- Why not "just duplicate the text in two plain FTS tables"? Also fine.
-- At 10k posts the duplicated text is a few MB. Do NOT spend design time
-- optimizing this; external-content is chosen because it keeps one source
-- of truth for triggers and rebuilds, not because of disk.
CREATE TABLE search_docs (
  post_id       TEXT PRIMARY KEY REFERENCES posts(id),
  text          TEXT NOT NULL DEFAULT '',
  note_text     TEXT NOT NULL DEFAULT '',
  author_handle TEXT NOT NULL DEFAULT '',
  author_name   TEXT NOT NULL DEFAULT '',
  alt_text      TEXT NOT NULL DEFAULT '',
  link_titles   TEXT NOT NULL DEFAULT ''
);

CREATE VIRTUAL TABLE posts_fts_sub USING fts5(
  text, note_text, author_handle, author_name, alt_text, link_titles,
  content='search_docs', content_rowid='rowid',
  tokenize='trigram'
);
CREATE VIRTUAL TABLE posts_fts_uni USING fts5(
  text, note_text, author_handle, author_name, alt_text, link_titles,
  content='search_docs', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);

-- Keep both indexes in sync with one set of triggers.
CREATE TRIGGER search_docs_ai AFTER INSERT ON search_docs BEGIN
  INSERT INTO posts_fts_sub(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
  INSERT INTO posts_fts_uni(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
END;

CREATE TRIGGER search_docs_ad AFTER DELETE ON search_docs BEGIN
  INSERT INTO posts_fts_sub(posts_fts_sub, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
  INSERT INTO posts_fts_uni(posts_fts_uni, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
END;

CREATE TRIGGER search_docs_au AFTER UPDATE ON search_docs BEGIN
  INSERT INTO posts_fts_sub(posts_fts_sub, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
  INSERT INTO posts_fts_sub(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
  INSERT INTO posts_fts_uni(posts_fts_uni, rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES ('delete', old.rowid, old.text, old.note_text, old.author_handle, old.author_name, old.alt_text, old.link_titles);
  INSERT INTO posts_fts_uni(rowid, text, note_text, author_handle, author_name, alt_text, link_titles)
    VALUES (new.rowid, new.text, new.note_text, new.author_handle, new.author_name, new.alt_text, new.link_titles);
END;

-- ── semantic (optional, M4) ────────────────────────────────────────────
-- CREATE VIRTUAL TABLE post_vecs USING vec0(post_id TEXT PRIMARY KEY, embedding FLOAT[384]);
```

Four model decisions that matter:

- **`raw_tweet` per post; the page envelope once in `capture_payloads`.** The envelope is large and shared by every post on the page. Storing it per post duplicates an order of magnitude of bytes for zero benefit; storing it once keeps full replayability *and* lets you re-derive every post from the page it came from.
- **`removed_at`, never `DELETE`.** The whole point is outliving X. But see the wording note below — we flag, and the badge says "no longer in your bookmarks," because that is all we actually know.
- **Two FTS tables, fused with RRF.** `trigram` gives **substring matching and CJK** (it is not typo tolerance — see §7.7); `unicode61` gives proper BM25 term relevance. They are two different retrievers over the same documents, so they get fused the same way FTS and vectors do: Reciprocal Rank Fusion, one code path, not an ad-hoc score blend.
- **`search_docs` is a flattened projection, not a source of truth.** Posts own the data; the projection exists so one set of triggers can maintain both indexes.

**Disappearance detection — and why we don't say "deleted."**

Without an API there is no way to ask X whether a post still exists, so this is necessarily weaker than an API-based diff would be. The mechanism: mark a bookmark absent only if a later capture session **covered its `sort_index` range and it was not present**. Conservative by design — better to leave a vanished post unflagged than to flag one that's fine.

But the deeper problem is semantic, not mechanical. **Absence from a later capture does not mean the post was deleted.** It could equally mean: the user unbookmarked it, X filtered it out of the timeline for some reason, the capture session was partial and the sort range overlap was optimistic, or the post is still there and simply wasn't served. We cannot distinguish these, and we should not pretend to.

So the flag exists, and the badge reads **"no longer in your bookmarks."** It never reads "deleted on X." That phrasing is the difference between reporting an observation and inventing a fact.

### 7.7 Search & browse UX

| Feature | v1 | Notes |
|---|---|---|
| Keyword search | ✅ | Dual FTS5 (`trigram` + `unicode61`), debounced ~30 ms, `highlight()` + `bm25()` ranking, both retrievers fused with RRF. Target ≤ 10 ms/keystroke at 10k. |
| Substring & CJK matching | ✅ | The `trigram` retriever's actual job — matches inside words and handles scripts `unicode61` tokenizes poorly. **Not** typo tolerance. |
| Typo tolerance | ✅ (M4) | Comes from the embedding index, not from FTS. Fuzzy lexical matching is a thing we get as a side effect of semantic search, not something to bolt onto trigram. |
| Filters | ✅ | author, date range, `has:media` / `has:link` / `has:video`, folder, tag, lang, min likes, source, "no longer in your bookmarks" |
| Semantic search | ✅ (M4) | `fastembed-rs` local ONNX + `sqlite-vec` KNN. Background embedder with progress UI ("indexing 1,234 / 5,000"). |
| Hybrid ranking | ✅ (M4) | Reciprocal Rank Fusion across all retrievers — `unicode61` BM25, `trigram`, and vectors. One fusion path, not three ad-hoc blends. Per-query toggle (`/kw`, `/sem`). |
| Folders | ✅ | X folders synced read-only, many-to-many; local **tags** and **notes** are yours |
| Threads & quotes | ✅ | Render quoted post inline from `refs`. Full-thread fetch would need network — out of scope, so show what was captured and link out. |
| Link previews | ✅ | Built from the payload's own `expanded_url` and `card` data. **No `t.co` unfurling** — see below. |
| Export | ✅ | Markdown (one file per bookmark, Obsidian-friendly frontmatter), JSON, CSV |
| Command palette | ✅ | `Cmd/Ctrl+K`: search, jump to author, filter, sync, export. Usable with no mouse. |
| Live capture panel | ✅ | The wow feature — see §7.8 |
| Global hotkey summon | v1.1 | `tauri-plugin-global-shortcut`, gyotaku-style quick window |
| Random "resurface" | v1.1 | N random old bookmarks on launch. Cheap, disproportionately loved. |

**No `t.co` unfurling.** An earlier draft had the app resolving shortened links with a HEAD request at ingest, "≤ 1 hop, same politeness rules." That was wrong on its own terms: **`t.co` is an X-owned host**, so a HEAD request to it is an app-originated request to X — exactly the class of traffic this architecture exists to avoid, and a direct contradiction of the §7.1 claim that the app never dials out. It buys us a final URL and a title.

The GraphQL payload already carries what we need: `entities.urls[].expanded_url` on the post, plus `card` data with the preview title, description and destination when a card is present. Parse those. Index the expanded domain, not the shortener. If a link has neither, show the `t.co` and move on — the payload is the authority, and we never spend a request to improve on it.

**Layout:** three panes — filters | results (virtualized, dense) | detail (rendered post, media, quoted post, notes, tags, "Open on X"). Search bar focused on open. `/` focuses search, `j`/`k` navigate, `Enter` opens, `Esc` closes, `t` tags, `o` opens on X.

**Error states must be specific.** "X is rate limiting; resuming at 14:32" beats "Sync failed." "X stops showing bookmarks older than ~1,000" beats "0 new results."

### 7.8 Live capture view — the demo moment

When the bridge is active, a **Live Capture** panel shows:

- New bookmarks sliding in as the user scrolls in their browser, with a subtle animation.
- Running count: *"247 bookmarks captured this session."*
- Resumability status: *"368 new / 44 already synced."*
- Connection badge: 🟢 connected / 🟡 buffering / 🔴 disconnected.
- Session budget remaining, in plain language: *"~350 more before we suggest a break."*
- Progress: *"~1,200 remaining"* estimated from scroll position vs. observed cursors.

This is the launch video: browser on the left scrolling bookmarks, app on the right building itself, then the browser closes, you go offline, and you type a query that returns in single-digit milliseconds. No API key. No paid tier. No cloud. No bot traffic — in passive mode there is no bot to detect, because the only scrolling happening is the user's own.

---

## 8. Safety Governor

One module, shared by the userscript and the app. It is the reason this product is safe to ship.

```
┌─────────────────────────────────────────────────────────┐
│                  Safety Governor                         │
│                                                          │
│  • Count GraphQL bookmark responses received             │
│  • Track 15-min rolling + per-day windows                │
│  • Approaching the session budget → pause, "Take a       │
│    break" badge, log resume timestamp                    │
│  • DOM tripwires: rate-limit toast, KeepAlive/error      │
│    overlay, challenge screen → halt immediately, notify  │
│  • Randomize ALL timing. Never periodic.                 │
│  • Cooldown ledger: refuse to start when recent volume   │
│    is high                                               │
│  • No synthetic XHR/fetch. Ever.                         │
│  • No parallelism. One tab, one session, one account.    │
└─────────────────────────────────────────────────────────┘
```

Design notes:

- **Budgets are ceilings, not targets** — and they are **our** numbers, not X's. Default stop at ~600 new captures or 20 minutes. X is not currently publishing a read cap to pace against (§4.1), so the budget is a product decision: a bounded session is easy to reason about, easy to stop, and cheap to resume. If the platform never pushes back, the cost of the budget is one extra sitting.
- **Observed platform behavior is the policy.** With no published number to cite, the governor's real inputs are 429s, `KeepAlive`/error overlays and challenge screens. Any of them halts the session, lowers the pacing profile permanently for that account, and is never auto-retried.
- **429 handling:** honor the reset, back off for the session, lower the pacing profile, *never* retry. One 429 should visibly change the app's behavior — if it doesn't, the governor isn't doing its job.
- **Idempotent and resumable.** Persist the cursor and dedupe by post ID, so an interrupted run resumes instead of restarting. This is what makes small sessions acceptable — you're not losing progress, so you don't need to rush.
- **Media fetches count too.** Default in-session media capture is cache-hit-shaped and effectively free; the opt-in "Archive originals" path issues real requests and is budgeted and circuit-broken like any other traffic (§7.5). Don't let the media pipeline become an unaccounted exemption.
- **Refusals are a feature.** The app should be *proud* of declining to start a session. That's the demo of good citizenship.

---

## 9. Resilience & schema drift

`[Confirmed by source]` X adjusts guest tokens, `doc_id`s, rate limits and detection patterns every 2–4 weeks. Response shapes drift on their own schedule. Four layers of defense:

1. **Match on operation name, not query ID.** The hash rotates; `Bookmarks` has historically been stable. We never construct requests, so `doc_id` rotation cannot break us.
2. **Store the raw payloads — both levels.** The full GraphQL envelope once per captured page in `capture_payloads`, and the per-tweet `tweet_results.result` subtree in `posts.raw_tweet` (§7.6). Even if the parser breaks completely, the data is preserved and a parser update re-derives everything retroactively without re-capturing anything.
3. **Versioned parser adapters.** The tweet-extraction logic is a single swappable module with version tags. When X changes shape, ship an adapter that adds a new parser version while keeping old ones for reprocessing stored payloads. The userscript auto-update channel pushes the fix.
4. **Fallback: file import.** If the userscript breaks entirely, any other exporter's JSON/CSV/HAR still imports. The user is never dead in the water.

Community PRs are a genuine part of this strategy — the prior art is open source and survives partly because users can fix it.

---

## 10. Security & privacy

- **Local-first, offline by default.** Data never leaves the machine. No cloud component for anyone to target.
- **No X credentials, anywhere.** No OAuth tokens, no API keys, no `auth_token`/`ct0` in the app. The userscript only ever sees response *bodies* the page already received.
- **Optional updater via `tauri-plugin-updater` with signed releases** — the one network call the app makes to a non-X host, and it's opt-in, checksum-verified, and never carries library contents.
- **Loopback endpoint, hardened as a local service.** The Chrome LNA restriction (§7.4) exists because pages attacking localhost services is a real threat class — so this endpoint is treated as hostile territory: `127.0.0.1` bind only, random-or-persisted high port, **the bearer install secret is the only authentication** (no origin check, no ambient cookie), request body size caps, strict schema validation, and a refusal to listen at all unless the app is open. An unauthenticated request gets nothing but a rejection.
- **No CORS configuration, deliberately.** Requests arrive from the userscript's extension context, where CORS does not apply. Pinning an origin would be defending against a mechanism that isn't in play while implying it is.
- **Tauri v2 capabilities: deny-by-default.** Frontend gets only the IPC commands it needs. `fs` scoped to app data. No `shell`. All DB, file I/O and network stay in Rust.
- **CSP locked** to `self` + `asset:`. Media served through the asset protocol from the local content-addressed store, never hot-linked from `pbs.twimg.com`.
- **No telemetry, no analytics, no crash reporting.** If ever added, opt-in and anonymous. Not in v1.
- **The userscript is open source and small** — auditable in one sitting. This matters for trust exactly as much as it did for the prior art; people install it *because* they can read it. It requests three grants (`GM_xmlhttpRequest`, `GM_setValue`, `GM_getValue`) and one connect permission (`127.0.0.1`), and nothing else.
- **Optional encrypted export** (`age`/`rage`) for backups.
- **Onboarding legal footer, in plain language:** you are reading your own data, in your own browser, on your own machine. File imports are your own files. Nothing is uploaded, and nothing is resold or redistributed.

---

## 11. Performance targets

**Split into two categories, because only one of them is ours to promise.**

### Committed — these we control and will hit

| Metric | Target |
|---|---|
| Keyword search, 10k bookmarks | ≤ 10 ms per keystroke |
| Semantic search, 10k | ≤ 80 ms |
| Scrolling 10k-row results | 60 fps, DOM node count flat |
| Embedding throughput (CPU) | ≥ 50 posts/s |
| Bundle size | ≤ 15 MB without the ONNX model |
| Passive capture overhead on the x.com page | Not perceptible to the user |

Search latency is carried over from gyotaku with confidence because it is a Rust + SQLite FTS5 query — the same work in the same runtime, independent of the UI toolkit. The webview never sees the query until it's already answered.

### Baseline-first — set from measurement, not aspiration

| Metric | Status |
|---|---|
| Cold launch → interactive | **Unset.** Runs on WebView2 / WKWebView / WebKitGTK, not on our code. Baseline in M0 on all three, then publish a per-platform target. |
| Idle RSS | **Unset.** Same reason — the webview runtime dominates. gyotaku's 37 MB is a gpui number and is not addressable from a webview app. Baseline, then target. |

An earlier draft carried gyotaku's ~120 ms summon and 37 MB idle straight across as our targets. **That was aspirational, not engineering** — those numbers are properties of gpui, and publishing them before measuring would set a commitment we cannot keep for reasons no amount of optimization on our side would change. Measure in M0, publish after.

**ONNX model download:** fetched on first run, **SHA-256 verified against hashes pinned in source** — exactly what gyotaku does. Keyword search works fine without it; semantic is purely additive and must degrade gracefully when offline.

---

## 12. Milestones

| Phase | Scope | Ships |
|---|---|---|
| **M0 — Spike (1 wk)** | Userscript: `fetch`/XHR hook, operation matcher, `Bookmarks` parser, `GM_xmlhttpRequest` handoff with persisted pairing, NDJSON exporter, count badge. Run it on a real >1k-bookmark account. Also: baseline cold launch and idle RSS on WebView2 / WKWebView / WebKitGTK. | **Go/no-go data on the four real unknowns:** where pagination cursors die; whether default in-session media capture covers the media the UI needs; whether `@run-at document-start` reliably beats X's bundle across managers and browsers; and the webview performance baselines that set §11's unset targets. *Nothing else is worth building before this answers itself.* |
| **M1 — Core (2 wk)** | `crates/core`: schema, NDJSON importer, external-content FTS5 (both tokenizers + triggers), RRF fusion, search, dedupe, `capture_payloads` / `raw_tweet` storage. `crates/cli`: `xdl import … && xdl search …` | A working local library, dogfooded from the terminal. Immediately useful to anyone with an existing export. |
| **M2 — App (2 wk)** | Tauri shell, pairing + persistent secret flow, loopback receiver, live capture panel, virtualized list/detail, command palette, keyboard nav, "Open on X" | The split-screen demo runs. **This is the shippable v1.** |
| **M3 — Polish (2 wk)** | Media pipeline (default cache-hit capture + content-addressed store), optional "Archive originals" action, tags, notes, folder joins, filters, Markdown/JSON/CSV export, theming, disappearance badges | Feel and finish. |
| **M4 — Semantic (1 wk)** | `fastembed-rs` + `sqlite-vec` + RRF across all three retrievers, background indexer with progress, `/sem` prefix | The differentiator, and the only source of typo tolerance: "a library you can actually search, not a dead CSV." |
| **M5 — Careful Mode + importers (1 wk)** | Assisted scroll with the full mitigation suite, cooldown ledger, pre-flight risk warnings; `twitter-web-exporter` / `xarchive` / HAR / CSV importers | Comfort features for large libraries. |
| **M6 — Ship** | Signed builds (macOS notarized, Windows signed), updater, GitHub releases, Greasy Fork userscript publish, launch tweet with a 10-second search demo | Done. |

M0 is deliberately first and deliberately small. It de-risks the entire product in a week, and its answers change the milestone ordering — if the pagination wall is soft, folders and assisted scroll drop in priority; if default media capture covers what the UI needs, the "Archive originals" subsystem drops out of M3 entirely; if the webview baselines come back badly, that's a UI-framework conversation worth having before two weeks of polish rather than after.

**Q7 from earlier drafts is closed and costs M0 nothing:** X's data archive excludes bookmarks. That is settled, and it goes straight into onboarding copy as a definitive statement.

---

## 13. Risks & mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| **Pagination wall blocks deep backfill** | High | Measure in M0. Folder capture (folders paginate independently, many-to-many), continuous capture for everything new, honest UI messaging. Wall only affects first backfill. |
| **Schema/detection drift** (`doc_id`, response shape, regex) | Medium | Operation-name matching (not `doc_id`), page-envelope + per-tweet payload storage, versioned parsers, tolerant regex, userscript auto-update channel || **Assisted scroll spooks detection** for some account (new account, VPN, shared IP — network signals compound) | Medium | Conservative defaults (600/20 min), cooldown ledger, pre-flight warnings for risk factors, passive mode always available and always default. §5 labels the residual risk rather than denying it. |
| **Default media capture misses media the UI wants** | Medium | Cache-hit-shaped capture is the default and will have gaps by design. Gaps render as text-only cards, honestly labeled. The opt-in "Archive originals" action covers the rest, budgeted and circuit-broken (§7.5). |
| **Platform behavior changes with no published policy to track** | Medium | No read cap is asserted anywhere in the design, so there is nothing to be wrong about. The governor reacts to observed 429s / challenge screens and lowers pacing permanently per account. |
| **Userscript manager friction** | Low | WebExtension packaging in v1.1; HAR import as the zero-install path |
| **Loopback endpoint abused by a page in the same browser** | Low | Bearer install secret is the sole authentication, loopback bind only, body caps, schema validation, no ambient-cookie trust, and the LNA/CSP restrictions mean a hostile page can't reach it from page context at all |
| **ToS gray zone** — even passive capture is arguably "collection" under a strict reading of X's terms | Low | Honest positioning: personal archival of your own data, local-only, mirrors long-standing tolerated tools, no cloud component to target, user holds the data |
| **User scrolls too fast manually** | Low | Safety Governor counts requests and warns at ~80% of budget; app suggests splitting across 2–3 sessions |
| **ONNX model download fails / offline** | Low | Keyword search works without it; semantic is additive |
| **Community can't fix a broken parser** | Low | Open source, one-file userscript, operation-name matching means fixes are usually a few lines |

---

## 14. Open questions — answer in M0

These decide the architecture. Don't guess at them.

1. **Where exactly do pagination cursors die** on a >1k-bookmark account? Consistent, or folder-dependent? This decides how much folders and assisted scroll matter.
2. **Does default cache-hit-shaped media capture cover** what the UI needs for photos, videos and GIF thumbnails? If yes, "Archive originals" stays a niche power feature and an entire politeness subsystem never needs writing. If no, size the gap.
3. **Does the userscript `@run-at document-start` interception reliably beat X's bundle** in Chrome, Firefox and Safari-family browsers, and do Tampermonkey / Violentmonkey / Userscripts behave consistently with `unsafeWindow` / `wrappedJSObject` and `@connect 127.0.0.1`?
4. **What are the cold-launch and idle-RSS baselines** on WebView2, WKWebView and WebKitGTK? These set §11's unset targets — and a bad result is worth surfacing before M2, not after.
5. **Does the `TweetDetail` / timeline ambient-response hit rate** make ID-first continuous capture enrich most saves immediately, or do a large fraction land as stubs waiting for the next backfill pass?

**Closed since the last revision — do not relitigate:**

- ~~Which transport: loopback HTTP or `ws://`?~~ → `GM_xmlhttpRequest` with `@connect 127.0.0.1`. Page-context `fetch` and `WebSocket` are both blocked by CSP and Chromium LNA. Decided (§7.4).
- ~~Is the `CreateBookmark` response rich enough for a full record?~~ → No. It returns a status stub; the ID lives in the request variables. This is a design input, and §6.5 is built around it.
- ~~What is the real read cap?~~ → Not a well-founded question. The ~1,000/day figure is a 2023 emergency limit, not current policy. Design around observed behavior instead (§4.1).
- ~~Does X's data archive include bookmarks?~~ → No. Settled. Goes in onboarding copy as fact.

---

## 15. What we are explicitly not building

Written down so it doesn't creep back in:

- ❌ Any X API usage, free or paid — including "just for one-time backfill"
- ❌ Headless/CDP automation, Playwright/Puppeteer/Selenium
- ❌ Cookie extraction or reuse (`auth_token`/`ct0`) in the Rust backend
- ❌ Backend-issued GraphQL requests, even with the userscript's `doc_id`
- ❌ Third-party scraping APIs, proxies, or residential IP services
- ❌ Aggressive or unbounded auto-scroll
- ❌ Unbookmark-as-you-archive draining (destructive; touches account state)
- ❌ Any cloud sync, telemetry, or phone-home
- ❌ Any feature whose first step is "authenticate with X"

---

## 16. Success metrics

Local-only, opt-in, or just your own dogfooding:

- **Time from install → first search < 5 minutes.** One pairing, then it works across restarts — no re-pairing ritual.
- **Search-to-open rate** — did they find the thing they were looking for?
- **Captured-per-session** trending toward 100% of what X will show, with zero rate-limit events observed.
- **Zero "my account got flagged" reports** in passive mode — because passive mode originates no additional requests, not because we got lucky.
- **The one that matters:** a bookmark surfaced from 2023, found in under a second, offline, that X would never have shown you again.

---

## Appendix A — Why this demos well

The gyotaku formula applied to a pain everyone on X feels: *native, offline, instant search over something you already own but can't actually use.*

The hook writes itself: **"X won't let you export or search your own bookmarks. So I built a local app that does — in Rust + Tauri, fully offline, in milliseconds, for free."**

The launch video is a split screen: bookmarks scrolling on the left, the library materializing on the right, then the browser closes, the network disconnects, and a query returns in single-digit milliseconds across thousands of items.

No API key. No paid tier. No cloud. No bot traffic. Just your own data, finally searchable.

*(Say "no bot traffic," not "no bot risk." Passive mode originates zero additional requests — that is a precise, defensible claim. Assisted scroll is automation and carries real, if bounded, risk, which is exactly why it is opt-in and budgeted. A launch claim that overreaches is a launch claim that gets taken apart in the replies.)*

---

## Appendix B — Explicitly rejected alternatives, with reasons

| Considered | Source draft | Rejected because |
|---|---|---|
| Official X API ingestion (OAuth PKCE, Owned Reads) | Drafts #1, #2 | Violates the $0 constraint. Also capped ~800, and the free tier no longer exists for new developers. |
| Rate Governor against API endpoints | Draft #1 | No API endpoints left to govern. Superseded by the Safety Governor (§8), which budgets *user scrolling* rather than our requests. |
| WKWebView pointed at `x.com/i/bookmarks` inside the Tauri app | Draft #1 (Mode B) | Embedding the login flow means the app owns a session with X. It puts X traffic inside our process — the exact thing §4.3 forbids. A real browser stays a real browser. |
| Companion WebExtension "in v1.1" | Drafts #1, #3 | Not rejected so much as deferred: the userscript proves the concept in M0 with zero packaging overhead. The extension wraps the same core later. |
| DOM scraping of `article[data-testid="tweet"]` | — | Loses `note_tweet`, media variants, quoted payloads, cards and `sort_index`. Strictly worse than interception for identical risk. |
| Unbookmark-as-you-archive | Draft #4 | Destructive, mutates account state, and the one thing that turns a read-only tool into something that acts on the account. |
| Thread fetching from the network | Drafts #1, #2 | Requires non-owned reads. Show captured context and link out instead. |
