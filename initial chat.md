# I want to build a tauri v2 app (rust backend) that allows me to look at and search my twitter bookma

## [1] USER

*10/8/2026, 12:50:12 AM*

I want to build a tauri v2 app (rust backend) that allows me to look at and search my twitter bookmarks. it'd be cool to have a built in way to ingest (that DOESN'T RUN AFOUL OF X'S ANTI-BOT PRACTICES, DOESN'T get the user rate-limited). 

I want you to design a PRD for that. Latest and greatest tech and techniques. Lots of people are building amazing rust GUI apps (mostly with Tauri v2, to my knowledge) and showing them off on twitter. e.g. apps like this https://github.com/xevrion/gyotaku

---

## [2A] ASSISTANT (claude-opus-4-6-search)

*10/8/2026, 12:50:12 AM*

I'll research a few key things to make sure this PRD uses current approaches—especially around X's bookmarks access and the latest Tauri v2 ecosystem.<sup>[[1](https://docs.x.com/x-api/fundamentals/rate-limits)]</sup><sup>[[2](https://docs.x.com/fundamentals/authentication/guides/v2-authentication-mapping)]</sup><sup>[[3](https://gist.github.com/igorbrigadir/de2a6cf16dfcdd4506816ef9a89aaa18)]</sup><sup>[[4](https://www.npmjs.com/package/twitter-api-v2)]</sup><sup>[[5](https://www.blotato.com/blog/twitter-api-pricing)]</sup><sup>[[6](https://devcommunity.x.com/t/rate-limited-on-posting-tweets/235709)]</sup><sup>[[7](https://docs.x.com/x-api/posts/bookmarks/quickstart/bookmarks-lookup)]</sup><sup>[[8](https://9meters.com/entertainment/social-media/x-api-rate-limits-formerly-twitter)]</sup><sup>[[9](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)]</sup><sup>[[10](https://github.com/xdevplatform/twitter-api-java-sdk)]</sup><sup>[[11](https://github.com/xevrion/gyotaku/releases/tag/v0.1.0)]</sup><sup>[[12](https://github.com/Chiroyce1/gyotaku)]</sup><sup>[[13](https://github.com/xevrion/gyotaku/releases/tag/v0.1.4)]</sup><sup>[[14](https://github.com/xevrion/gyotaku/releases/tag/v0.1.2)]</sup><sup>[[15](https://github.com/xevrion/gyotaku/releases/tag/v0.1.1)]</sup><sup>[[16](https://github.com/xevrion)]</sup><sup>[[17](https://github.com/xevrion/gyotaku)]</sup><sup>[[18](https://trendshift.io/repositories/282457)]</sup><sup>[[19](https://x.com/xevrion_the1/status/2107047524662157818)]</sup>Let me check one more thing about the official data archive and the compliance angle for ingestion.<sup>[[20](https://github.com/lhl/tweetxvault/)]</sup><sup>[[21](https://github.com/prinsss/twitter-web-exporter)]</sup><sup>[[22](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)]</sup><sup>[[23](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup><sup>[[24](https://takeoutday.org/guides/how-to-export-twitter-data)]</sup><sup>[[25](https://www.marqly.com/blog/export-twitter-x-bookmarks)]</sup><sup>[[26](https://stashr.me/blog/download-twitter-archive)]</sup><sup>[[27](https://bookmarksave.com/blog/how-to-export-twitter-bookmarks-pdf-csv-markdown)]</sup><sup>[[28](https://www.archivlyx.com/blog/how-to-export-twitter-bookmarks-in-2025-step-by-step-guide)]</sup># PRD: "BookmarkVault" — A Local-First X (Twitter) Bookmark Browser & Search App

**Stack:** Tauri v2 (Rust backend) · WebView-based UI · SQLite + FTS5 · local-first

---

## 1. Background & Problem

X gives you no good way to live with your own bookmarks. This is well-documented across the ecosystem:

- <sup>[[23](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>There is no download button. X/Twitter does not offer any way to export bookmarks — not through the app, not through Settings, not even through the official data archive download that gives you tweets, DMs, and account history. Bookmarks are simply excluded.
- The official API path exists but is capped: <sup>[[21](https://github.com/prinsss/twitter-web-exporter)]</sup>the official API has an 800-item limit when accessing bookmarks, whereas scraping the web feed can export all bookmarks until restricted by the web app itself.
- <sup>[[25](https://www.marqly.com/blog/export-twitter-x-bookmarks)]</sup>There's a practical ceiling of roughly 800–1,000 visible bookmarks — X doesn't document an official cap, but the bookmarks page stops loading older items around that point, and the API paginates out at a similar number.

So the product need is: **a fast, offline, searchable personal library of your bookmarks, with an ingestion mechanism that is gentle enough to never trip anti-bot heuristics or get you rate-limited.**

The reference app you cited (gyotaku) is a good north star for *vibe and quality*: <sup>[[17](https://github.com/xevrion/gyotaku)]</sup>it's native on Linux, macOS and Windows, fully offline, and fast on any hardware, reading the text in content and making it searchable. Its reported headline numbers set the bar for "feels instant": <sup>[[17](https://github.com/xevrion/gyotaku)]</sup>search latency on 5,000+ items of 1–10 ms per keystroke, ~120 ms window summon time, 144 fps with a GPU, and ~37 MB idle memory. We want that feel for bookmarks.

---

## 2. Goals & Non-Goals

### Goals
1. **Ingest bookmarks without getting the user flagged or rate-limited** (the hard constraint — see §5).
2. Store everything **locally** (SQLite), work **fully offline** after ingest.
3. **Instant search** — sub-10ms keystroke latency, full-text + filters, optional semantic search.
4. Rich browsing: media thumbnails, author, date, thread context, tags/folders.
5. Beautiful, fast, native-feeling UI worth showing off on X.
6. Incremental re-sync that captures *new* bookmarks cheaply.

### Non-Goals (v1)
- Writing/managing bookmarks back to X (add/remove) — read-only first.
- Multi-account / team sync.
- Cloud backend. (Local-first; optional encrypted export only.)
- Mobile build (Tauri v2 supports it, but defer).

---

## 3. Core Compliance Principle (read this before architecture)

The entire design hinges on one idea: **move at human speed, prefer official channels, and never automate aggressive scrolling or parallel requests.**

X rate limits are per-endpoint and surface via response headers. <sup>[[1](https://docs.x.com/x-api/fundamentals/rate-limits)]</sup>Rate limits control the number of requests you can make to each endpoint; exceeding limits results in a 429 error until the window resets. Critically for our backoff logic: <sup>[[9](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)]</sup>every X API response carries three headers — x-rate-limit-limit, x-rate-limit-remaining, and x-rate-limit-reset — that tell you exactly where you stand.

For the official bookmarks endpoint specifically: <sup>[[1](https://docs.x.com/x-api/fundamentals/rate-limits)]</sup>GET /2/users/:id/bookmarks is limited to 180 requests per 15 minutes per user. It requires <sup>[[7](https://docs.x.com/x-api/posts/bookmarks/quickstart/bookmarks-lookup)]</sup>a User Access Token with the bookmark.read scope via OAuth 2.0 PKCE.

A real caution worth designing around: even when headers *say* you have budget, X enforces hidden limits. <sup>[[6](https://devcommunity.x.com/t/rate-limited-on-posting-tweets/235709)]</sup>Developers report the real rate limits differ from what's advertised on the website and in the API response header; in one case, roughly one request every ~35 seconds was what reliably worked. **Our ingestion engine must therefore treat published limits as a ceiling, not a target, and self-pace well below them.**

---

## 4. High-Level Architecture

<details>
<summary><strong>💻 Code Block — 16 lines</strong></summary>

```
┌─────────────────────────────────────────────────────────┐
│  Tauri v2 App                                            │
│                                                         │
│  ┌───────────────┐        ┌──────────────────────────┐ │
│  │  UI (WebView) │◄──IPC─►│  Rust Core (backend)      │ │
│  │  Svelte/React │        │                           │ │
│  │  + virtualized│        │  • Ingestion Engine       │ │
│  │    list       │        │  • Rate Governor          │ │
│  └───────────────┘        │  • SQLite (FTS5) store    │ │
│                           │  • Search service         │ │
│  ┌───────────────┐        │  • Media cache            │ │
│  │ Capture Window│───────►│  • Secrets (keyring)      │ │
│  │ (hidden WRY   │ events │                           │ │
│  │  webview→x.com)│        └──────────────────────────┘ │
│  └───────────────┘                                     │
└─────────────────────────────────────────────────────────┘
```

</details>

### Recommended tech choices
- **Shell:** Tauri v2 (stable, cross-platform, Rust backend, capabilities/permissions ACL for security).
- **Frontend:** Svelte 5 or React 19 + a virtualized list (TanStack Virtual) for 10k+ rows. Keep DOM nodes minimal for gyotaku-class smoothness.
- **DB:** SQLite via `tauri-plugin-sql` (or direct `rusqlite`/`sqlx` in Rust core for more control). Use **FTS5** for full-text search.
- **Async runtime:** Tokio.
- **HTTP (API mode):** `reqwest` in Rust core (not `tauri-plugin-http` from JS) so the Rate Governor owns all network calls.
- **Secrets:** `keyring` crate (OS keychain) or `tauri-plugin-stronghold` for the OAuth refresh token. Never store tokens in plaintext/localStorage.
- **Semantic search (optional):** `fastembed-rs` (ONNX, local) or `candle` for embeddings; store vectors in `sqlite-vec`.
- **Media cache:** download thumbnails lazily to app data dir, keyed by URL hash.

---

## 5. Ingestion Engine — the heart of the product

Offer **three ingestion modes**, in increasing order of coverage and decreasing order of "officialness." Let the user choose; default to the safest.

### Mode A — Official API (safest, default, capped ~800)
- OAuth 2.0 PKCE flow in a Tauri webview; store refresh token in OS keychain.
- Call `GET /2/users/:id/bookmarks` with pagination, requesting rich fields (author expansion, media, metrics, created_at), mirroring the official quickstart pattern of <sup>[[7](https://docs.x.com/x-api/posts/bookmarks/quickstart/bookmarks-lookup)]</sup>retrieving bookmarked posts with author info using tweet fields, expansions, and user fields.
- **Rate Governor** (see §6) paces requests far below 180/15min.
- **Caveat to surface in-app:** the API cap. As confirmed widely, <sup>[[28](https://www.archivlyx.com/blog/how-to-export-twitter-bookmarks-in-2025-step-by-step-guide)]</sup>the full data archive does not include bookmarked tweets, and if your account is locked, suspended, or deleted you'll lose saved tweets permanently unless backed up by a third-party solution. Also note the API is now metered: <sup>[[5](https://www.blotato.com/blog/twitter-api-pricing)]</sup>rate limits are separate from cost — even on pay-per-use you hit per-endpoint 15-minute and 24-hour limits, spending more credits doesn't lift them, and you'll see HTTP 429s before hitting your cap.

### Mode B — Passive Capture (recommended for completeness, human-paced)
This is the clever bit and the most anti-bot-friendly for full coverage. Instead of scripting X, we **ride along with the user's own browsing**:

- Open a **real WRY webview** pointed at `x.com/i/bookmarks` where the user logs in normally (their session, their cookies, their device fingerprint — nothing spoofed).
- Inject a small content script that **passively intercepts the GraphQL JSON responses the web app itself loads** as the user scrolls — the same technique proven by existing userscripts. As one popular open-source tool documents, <sup>[[21](https://github.com/prinsss/twitter-web-exporter)]</sup>the script does not rely on the official API and so doesn't have the same rate limit, though the web app has its own limit; on the contrary it can export data not available from the official API.
- **Key compliance stance:** we do **not** auto-scroll aggressively or fire synthetic requests. The app captures only what the browser naturally fetches. Optional "gentle auto-scroll" is off by default and, if enabled, scrolls at human cadence with randomized pauses and stops on any rate-limit signal.
- Browser-extension equivalents demonstrate the UX we're emulating: <sup>[[22](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)]</sup>the tool reads the bookmarks feed X loads for the user and scrolls it to pull in more — no API key, so no API cap, and every tweet it loads is captured.

This mode blends in because it *is* a normal logged-in browsing session; there's no bot signature to detect.

### Mode C — Archive/File Import (zero network risk)
- Import from any exporter's JSON/CSV (twitter-web-exporter ZIP, etc.) for users who already have data.
- Note: X's own archive won't help here — <sup>[[26](https://stashr.me/blog/download-twitter-archive)]</sup>the official data archive includes your likes but not your bookmarks; it's a gap that's existed for years and as of 2026 nothing has changed. So Mode C is about interop with the third-party ecosystem, not X's archive.

### "Capture-first" continuous mode
Beyond bulk import, support the **save-as-you-go** pattern that sidesteps the cap for *future* bookmarks entirely, mirroring: <sup>[[22](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)]</sup>tools that don't use the 800-bookmark API instead read the same bookmarks feed the web app shows and save each new bookmark the moment it's tapped, so new saves stay in the library. In our app this means: when the Capture window is open and the user bookmarks something, we catch that GraphQL mutation/response and persist it immediately.

---

## 6. The Rate Governor (shared safety layer)

A single Rust module that **all** network-touching modes route through. Design requirements:

1. **Token-bucket pacing** set *conservatively* — e.g., target ≤ 30% of published per-window limits by default. Given real-world reports of ~35s/request working where headers claimed more budget, expose a "Cautious / Normal / Fast" pacing profile, default **Cautious**.
2. **Header-driven adaptation:** parse `x-rate-limit-remaining` / `x-rate-limit-reset` after every call; when remaining drops below a threshold, sleep until `reset`.
3. **429 handling with exponential backoff + jitter:** on a 429, honor `x-rate-limit-reset`, add jitter, and *lower* the pacing profile automatically for the rest of the session.
4. **No concurrency to X:** strictly serial requests, randomized human-like inter-request delays.
5. **Session awareness (Mode B):** detect X's in-app "rate limit exceeded" / challenge screens via the webview and immediately pause capture, surfacing a friendly "let's take a break" state.
6. **Resumable, idempotent sync:** persist a cursor + dedupe by tweet ID so an interrupted run resumes without re-fetching.
7. **Respect robots/ToS posture:** default to Official API; make the user explicitly opt into capture modes with a clear explanation.

---

## 7. Data Model (SQLite)

<details>
<summary><strong>💻 Code Block (sql) — 33 lines</strong></summary>

```sql
-- core
tweets(
  id TEXT PRIMARY KEY,          -- tweet id (snowflake)
  author_id TEXT,
  author_handle TEXT,
  author_name TEXT,
  text TEXT,
  created_at INTEGER,           -- unix
  lang TEXT,
  conversation_id TEXT,
  in_reply_to_id TEXT,
  public_metrics JSON,          -- likes/retweets/etc
  raw_json JSON,                -- full captured payload (future-proofing)
  bookmarked_at INTEGER,        -- when WE first saw it bookmarked
  source TEXT                   -- 'api' | 'capture' | 'import'
);

media(tweet_id, url, type, local_path, width, height, alt_text);
authors(id, handle, name, avatar_url, verified);
folders(id, name);               -- mirrors X bookmark folders if available via API
tweet_folders(tweet_id, folder_id);
tags(id, name);                  -- user's own tags (local-only)
tweet_tags(tweet_id, tag_id);

-- search
CREATE VIRTUAL TABLE tweets_fts USING fts5(
  text, author_handle, author_name,
  content='tweets', content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);

-- optional semantic
tweet_vectors(tweet_id, embedding BLOB);  -- via sqlite-vec
```

</details>

Note X exposes **bookmark folders** via API too — <sup>[[1](https://docs.x.com/x-api/fundamentals/rate-limits)]</sup>GET /2/users/:id/bookmarks/folders is limited to 50/15min — so we can mirror folder structure in Mode A (also governed/paced).

---

## 8. Search & Browse UX

- **Instant FTS:** query FTS5 on every keystroke, debounced ~30ms, highlight matches (BM25 ranking). Target gyotaku's bar: single-digit-ms latency on thousands of rows.
- **Filters:** author, date range, has-media, has-link, folder, tag, source.
- **Semantic/"meaning" search (opt-in):** local embeddings + vector KNN, blended with FTS (hybrid search). Fully offline — no data leaves the machine. (This is the differentiator vs. CSV dumps, echoing the "library you can actually search, not a dead CSV" positioning seen across the space.)
- **Virtualized masonry/list** with lazy media thumbnails.
- **Detail view:** full tweet, thread context (via `conversation_id`), link to open on X.
- **Keyboard-first:** global hotkey to summon, `/` to focus search, j/k nav, Enter to open.
- **Tags & notes:** local-only annotations the user controls.

---

## 9. Security & Privacy

- **Local-first, offline by default.** Match the ecosystem's privacy promise: <sup>[[21](https://github.com/prinsss/twitter-web-exporter)]</sup>everything processed locally in the browser/app, with no data sent to the cloud.
- OAuth refresh token in **OS keychain** (`keyring`) or Stronghold; access token in memory only.
- Tauri v2 **capabilities/permissions ACL:** grant the UI only the specific IPC commands it needs; keep all network + DB access in the Rust core.
- No telemetry by default; if ever added, opt-in and anonymous.
- Optional **encrypted export** (age/`rage`) for backups.

---

## 10. Milestones

| Phase | Deliverable |
|------|-------------|
| **M0 — Skeleton** | Tauri v2 app, SQLite schema, Mode C file import, FTS search, virtualized browse UI. Ships something useful day 1. |
| **M1 — Official API** | OAuth 2.0 PKCE, Mode A ingest with Rate Governor, folder mirroring, incremental sync. |
| **M2 — Passive Capture** | WRY webview + GraphQL interception, capture-first continuous save, challenge/limit detection. |
| **M3 — Semantic** | Local embeddings, hybrid search, tags/notes, polish + theming. |
| **M4 — Delight** | Global hotkey, command palette, media cache, export, perf pass to hit gyotaku-class latency. |

---

## 11. Key Risks & Mitigations

| Risk | Mitigation |
|---|---|
| **Getting rate-limited / flagged** | Rate Governor defaults to "Cautious," serial requests, header-driven backoff, no synthetic scrolling, prefer real user session in Mode B. |
| **API 800 cap** | Offer Mode B/C for full history; make capture-first the long-term capture path. |
| **X changes GraphQL schema** | Store `raw_json`; keep capture parsing in a versioned, swappable adapter; degrade gracefully. |
| **API now metered/paid** | Make Mode A optional; Modes B/C need no paid API. Surface cost expectations clearly. |
| **ToS ambiguity** | Default to official API; require explicit informed opt-in for capture; read-only; never resell/redistribute data; it's the user's own data on their own machine. |

---

## 12. Why this will demo well on X

It's the gyotaku formula applied to a pain everyone on X feels: **native, offline, instant search over something you already own but can't actually use.** The hook writes itself — *"X won't let you export or search your own bookmarks. So I built a local app that does, in Rust + Tauri, fully offline, in milliseconds."* That maps directly to the proven narrative style that made the reference project take off: <sup>[[19](https://x.com/xevrion_the1/status/2107047524662157818)]</sup>a simple "ctrl+F for my X" pitch — type any word, it shows up in milliseconds, fully offline, open source.

---

**One decision to make before building:** do you want **Mode A (official API, safe, capped)** as the flagship and capture as "advanced," or lead with **Mode B (passive capture)** for full-history coverage and treat the API as a convenience? I'd recommend shipping M0 + Mode C immediately (zero risk, instant value), then Mode A, then Mode B — but if your personal library is >800 bookmarks, prioritize Mode B sooner. Want me to spec the Rate Governor or the GraphQL capture adapter in detail next?

<details>
<summary><strong>📚 Sources (28)</strong></summary>

1. [X API Rate Limits - X](https://docs.x.com/x-api/fundamentals/rate-limits)
2. [X API v2 authentication mapping - X](https://docs.x.com/fundamentals/authentication/guides/v2-authentication-mapping)
3. [WIP: Twitter Undocumented Endpoints for Bookmark](https://gist.github.com/igorbrigadir/de2a6cf16dfcdd4506816ef9a89aaa18)
4. [Npmjs](https://www.npmjs.com/package/twitter-api-v2)
5. [X (Twitter) API Pricing: Complete Guide for 2026 - Blotato](https://www.blotato.com/blog/twitter-api-pricing)
6. [Devcommunity](https://devcommunity.x.com/t/rate-limited-on-posting-tweets/235709)
7. [Bookmarks Lookup - X](https://docs.x.com/x-api/posts/bookmarks/quickstart/bookmarks-lookup)
8. [X API Rate Limits (Formerly Twitter) - 9meters](https://9meters.com/entertainment/social-media/x-api-rate-limits-formerly-twitter)
9. [X (Twitter) API Alternative - Sorsa API](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)
10. [GitHub - xdevplatform/twitter-api-java-sdk: A Java SDK for the Twitter API](https://github.com/xdevplatform/twitter-api-java-sdk)
11. [Release gyotaku v0.1.0 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.0)
12. [GitHub - Chiroyce1/gyotaku: search every screenshot you've ever taken by the text inside it. native linux, offline, PaddleOCR + sqlite + gpui](https://github.com/Chiroyce1/gyotaku)
13. [Release gyotaku v0.1.4 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.4)
14. [Release gyotaku v0.1.2 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.2)
15. [Release gyotaku v0.1.1 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.1)
16. [xevrion - Overview](https://github.com/xevrion)
17. [GitHub - xevrion/gyotaku: search every screenshot you've ever taken by the text inside it. native linux, offline, PaddleOCR + sqlite + gpui](https://github.com/xevrion/gyotaku)
18. [xevrion/gyotaku — GitHub trending stats & insights](https://trendshift.io/repositories/282457)
19. [xevrion (@xevrion_the1) on X](https://x.com/xevrion_the1/status/2107047524662157818)
20. [GitHub - lhl/tweetxvault: Archive you Twitter/X likes and bookmarks](https://github.com/lhl/tweetxvault/)
21. [GitHub - prinsss/twitter-web-exporter: Export tweets, bookmarks, lists and much more from Twitter(X) web app. (推文/书签/收藏/列表导出工具)](https://github.com/prinsss/twitter-web-exporter)
22. [X (Twitter) Bookmarks Exporter, Free in 2026 | ContextBolt](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)
23. [How to Export X (Twitter) Bookmarks to CSV Free (2026)](https://contextbolt.com/blog/export-twitter-bookmarks/)
24. [Export Twitter/X Data: Tweets, DMs, Likes & Bookmarks (2026)](https://takeoutday.org/guides/how-to-export-twitter-data)
25. [How to Export X (Twitter) Bookmarks (2026, Every Method)](https://www.marqly.com/blog/export-twitter-x-bookmarks)
26. [How to download your X (Twitter) archive (and what's actually inside it) · Stashr](https://stashr.me/blog/download-twitter-archive)
27. [How to Export Twitter Bookmarks to PDF, CSV & Markdown](https://bookmarksave.com/blog/how-to-export-twitter-bookmarks-pdf-csv-markdown)
28. [How to Export Twitter Bookmarks in 2025: Step-by-Step Guide](https://www.archivlyx.com/blog/how-to-export-twitter-bookmarks-in-2025-step-by-step-guide)

</details>

---

## [2B] ASSISTANT (claude-fable-5-search)

*10/8/2026, 12:50:12 AM*

I'll pull a few current references first—the gyotaku repo, the current state of X's bookmarks API, and the latest Tauri v2 ecosystem—so the PRD reflects what's actually true in late 2026.<sup>[[1](https://github.com/xevrion)]</sup><sup>[[2](https://github.com/xevrion/gyotaku/releases/tag/v0.1.0)]</sup><sup>[[3](https://github.com/xevrion/gyotaku/releases/tag/v0.1.4)]</sup><sup>[[4](https://github.com/xevrion/gyotaku/releases/tag/v0.1.2)]</sup><sup>[[5](https://github.com/xevrion/gyotaku/releases/tag/v0.1.1)]</sup><sup>[[6](https://trendshift.io/repositories/282457)]</sup><sup>[[7](https://github.com/xevrion/gyotaku/releases/tag/models-v1)]</sup><sup>[[8](https://github.com/xevrion/gyotaku)]</sup><sup>[[9](https://github.com/Chiroyce1/gyotaku)]</sup><sup>[[10](https://x.com/xevrion_the1/status/2107047524662157818)]</sup><sup>[[11](https://devcommunity.x.com/t/can-i-access-twitter-bookmarks-for-development-purpose-for-free/221728)]</sup><sup>[[12](https://docs.x.com/x-api/posts/bookmarks/introduction)]</sup><sup>[[13](https://postproxy.dev/blog/x-api-pricing-2026/)]</sup><sup>[[14](https://www.upload-post.com/x-api-pricing/)]</sup><sup>[[15](https://www.outstand.so/blog/x-api-pricing)]</sup><sup>[[16](https://zernio.com/blog/twitter-api-pricing)]</sup><sup>[[17](https://www.xpoz.ai/blog/guides/understanding-twitter-api-pricing-tiers-and-alternatives/)]</sup><sup>[[18](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)]</sup><sup>[[19](https://api.sorsa.io/blog/twitter-api-pricing-2026)]</sup><sup>[[20](https://en.wikipedia.org/wiki/Tauri_(software_framework))]</sup><sup>[[21](https://v2.tauri.app/blog/)]</sup><sup>[[22](https://v2.tauri.app/blog/tauri-2.12/)]</sup><sup>[[23](https://tauri.app/release/core/)]</sup><sup>[[24](https://v2.tauri.app/release/)]</sup><sup>[[25](https://v2.tauri.app/blog/tauri-2-0-0-beta/)]</sup><sup>[[26](https://tauri.app/release/tauri/)]</sup><sup>[[27](https://dev.to/ottoaria/tauri-in-2026-build-cross-platform-desktop-apps-with-web-technologies-better-than-electron-11mo)]</sup><sup>[[28](https://v2.tauri.app/release/tauri/)]</sup><sup>[[29](https://tech-insider.org/tauri-tutorial-cross-platform-rust-app-2026/)]</sup><sup>[[30](https://github.com/sytelus/xarchive)]</sup><sup>[[31](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup><sup>[[32](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)]</sup><sup>[[33](https://en.wikipedia.org/wiki/Xmarks_Sync)]</sup><sup>[[34](https://github.com/prinsss/twitter-web-exporter)]</sup><sup>[[35](https://stashr.me/blog/download-twitter-archive)]</sup><sup>[[36](https://www.marqly.com/blog/export-twitter-x-bookmarks)]</sup><sup>[[37](https://bookmarksave.com/blog/how-to-export-twitter-bookmarks-pdf-csv-markdown)]</sup><sup>[[38](https://www.tweetarchivist.com/how-to-download-twitter-archive)]</sup><sup>[[39](https://docs.x.com/x-api/users/get-bookmarks)]</sup><sup>[[40](https://docs.x.com/changelog)]</sup><sup>[[41](https://devcommunity.x.com/t/x-api-pricing-update-owned-reads-now-0-001-other-changes-effective-april-20-2026/263025)]</sup><sup>[[42](https://docs.x.com/x-api/getting-started/pricing)]</sup><sup>[[43](https://www.blotato.com/blog/twitter-api-pricing)]</sup><sup>[[44](https://developer.x.com/en/docs/x-api/tweets/bookmarks/introduction)]</sup><sup>[[45](https://bundle.social/blog/x-api-pricing-2026-costs-limits)]</sup>Quick calibration before the PRD, because three things changed in 2026 that reshape the whole ingestion story:

1. **The "safe" path is now cheap and official.** The X API is pay-per-use, and as of April 20, 2026 there's "Owned Reads" pricing at $0.001 per resource for endpoints like `GET /2/users/{id}/bookmarks` when you're reading your own data through your own developer app. <sup>[[42](https://docs.x.com/x-api/getting-started/pricing)]</sup>Owned Reads are requests made by your own developer app for your own data (posts, bookmarks, followers, lists, and more). These endpoints are priced at $0.001 per resource (1,000 resources for $1). The following endpoints qualify for Owned Read pricing when {id} matches the authenticated user and that user is the owner of the developer app. That means a 5,000-bookmark backfill costs ~$5, once. The free tier is gone though: <sup>[[18](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)]</sup>there is no general free X API tier for new developers in 2026. The standalone free tier was discontinued when pay-per-use launched in February 2026, so new accounts must buy credits before making any call.
2. **The official archive is useless for this.** <sup>[[35](https://stashr.me/blog/download-twitter-archive)]</sup>The official X data archive includes your likes, but it does not include your bookmarks. It's a gap that's existed for years, developers have asked X to close it repeatedly, and as of 2026 nothing has changed.
3. **gyotaku isn't Tauri.** It's <sup>[[8](https://github.com/xevrion/gyotaku)]</sup>native linux, offline, PaddleOCR + sqlite + gpui — gpui is Zed's UI framework. Tauri is still the right call for *this* app (tweets are rich text + media + links, which a webview renders well), but worth knowing what you're benchmarking against. Its perf table is a good bar to aim at: <sup>[[8](https://github.com/xevrion/gyotaku)]</sup>Search latency, 5,000+ screenshots: 1 to 10 ms per keystroke; Window summon time: ~120 ms; Memory while idle in the background: 37 MB.

Stack-wise: <sup>[[22](https://v2.tauri.app/blog/tauri-2.12/)]</sup>Tauri 2.12 is here! This is the biggest update so far in the 2.x releases. (released Sep 26, 2026). A 3.0 alpha exists but stay on 2.x.

---

# PRD: **Marginalia** — a local-first X bookmarks library

*(working name; pick anything)*

## 1. One-liner

A fast, offline, keyboard-first desktop app that turns your X bookmarks into a searchable personal library — ingested through channels X explicitly sanctions, so you never get rate-limited, flagged, or locked out.

## 2. Goals / Non-goals

**Goals**
- Ingest 100% of what X will give you, via the official API, with cost shown *before* you spend.
- Sub-10ms keyword search and sub-100ms semantic search over 10k+ bookmarks, fully offline.
- Bookmarks survive X: cached text, media, and author snapshots persist even if the post is deleted.
- Zero cloud. No telemetry. Your DB is a single SQLite file you can back up.

**Non-goals (v1)**
- Not a Twitter client. No posting, liking, replying, timeline.
- No headless browser, no cookie-jar scraping, no GraphQL spoofing, no third-party scraping APIs. Ever. This is a product principle, not a phase-2 item.
- No multi-account (v1), no sync between devices (v1), no mobile (Tauri supports it, but later).

## 3. Ingestion design (the core of the product)

### 3.1 Principles ("don't get the user banned")
1. **Only speak to X the way X documents.** OAuth 2.0 PKCE + the v2 REST API. Honest `User-Agent` identifying the app.
2. **The app never generates synthetic browser traffic.** No automation of x.com, no reuse of browser cookies/`ct0`, no reverse-engineered GraphQL endpoints.
3. **Rate-limit headers are law.** Read `x-rate-limit-remaining` / `x-rate-limit-reset` on every response; stop *before* zero; on 429, sleep until reset — never retry-storm. The per-user limit is generous anyway: <sup>[[18](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)]</sup>/2/users/:id/bookmarks GET: per user 180 (per 15 min), at 100 results/page, so even a huge library is a handful of requests.
4. **Cost transparency.** Show estimated credit spend before every sync, show actual spend after.
5. **BYO developer app.** The user creates their own X developer app and pastes the Client ID. This is what unlocks Owned Reads pricing — <sup>[[15](https://www.outstand.so/blog/x-api-pricing)]</sup>only when {id} is the authenticated user and that user owns the developer app. A multi-tenant product whose customers connect their own accounts does not qualify; its reads stay at $0.005. It also means no shared quotas and no app-level key to leak.

### 3.2 Channel A — Official API sync (primary)

**Auth**
- OAuth 2.0 Authorization Code + PKCE, confidential-client-free (public client). Scopes: `bookmark.read tweet.read users.read offline.access`.
- Callback: loopback `http://127.0.0.1:<random-port>/callback` served by a tiny `axum`/`hyper` listener spun up only during auth. (Alternative: `tauri-plugin-deep-link` custom scheme — loopback is more reliable across OSes.)
- Tokens in the OS keychain via the `keyring` crate (never in SQLite, never in config). Refresh token rotation handled in Rust.

**Fetch**
- `GET /2/users/:id/bookmarks?max_results=100` with `tweet.fields=created_at,public_metrics,entities,referenced_tweets,note_tweet,attachments,lang,conversation_id,author_id`, `expansions=author_id,attachments.media_keys,referenced_tweets.id,referenced_tweets.id.author_id`, `media.fields=url,preview_image_url,type,variants,alt_text,width,height`, `user.fields=name,username,profile_image_url,verified`.
- Also sync folders: <sup>[[12](https://docs.x.com/x-api/posts/bookmarks/introduction)]</sup>GET /2/users/:id/bookmarks/folders — Get bookmark folders; GET /2/users/:id/bookmarks/folders/:folder_id — Get Posts in a folder.
- **Full backfill:** walk `pagination_token` to the end. Persist the cursor after every page so a crash resumes, not restarts.
- **Incremental sync:** fetch newest pages until you hit *N consecutive already-known IDs* (N=20), with a 2-page overlap to catch re-bookmarks. Typical daily sync = 1–2 requests.
- **Removals:** bookmarks absent from a full walk get `removed_at` set — never deleted locally (the whole point is outliving X).

**Cost model (shown in UI)**
- Backfill: `count × $0.001`. Incremental: ~`new × $0.001`. Note: <sup>[[14](https://www.upload-post.com/x-api-pricing/)]</sup>repeated reads of the same resource within a UTC day are billed once.
- ⚠️ Open question to verify in M1: whether `expansions` (users/media in `includes`) are billed separately as User Reads at $0.010. Test with a 1-page call and inspect the console ledger; if yes, default to minimal expansions and hydrate authors lazily/dedup'd.
- ⚠️ Known billing bug reported on the dev forum: <sup>[[41](https://devcommunity.x.com/t/x-api-pricing-update-owned-reads-now-0-001-other-changes-effective-april-20-2026/263025)]</sup>Owned Reads $0.001 rate not applied to bookmarks endpoint — billed at $0.005 instead. Still fine (5k bookmarks = $25 worst-case), but surface the actual rate from the first call rather than hardcoding $0.001.
- ⚠️ Historical ~800-item ceiling: <sup>[[34](https://github.com/prinsss/twitter-web-exporter)]</sup>the script can export data that is not available from the official API. For example, the official API has a 800 limit when accessing the bookmarks. Treat this as unverified for 2026 — measure in M1. If it holds, Channel B is how you get the long tail.

**Rate limiter (Rust)**
- Token-bucket per endpoint keyed on the user, seeded from response headers. Single in-process `tokio::sync::Semaphore(1)` for X calls — no parallel pagination, there's no need.
- Backoff: respect `reset`; on 5xx use exponential + full jitter, max 3 attempts, then surface to UI.

### 3.3 Channel B — File import (no network, zero risk)
Import JSON from the user's own browser-side exports. These tools run *in the user's real session* as part of them browsing — the app just parses files:
- `twitter-web-exporter` (userscript) JSON — <sup>[[34](https://github.com/prinsss/twitter-web-exporter)]</sup>the script will automatically capture on the following pages: User profile page (tweets, replies, media, likes), Bookmark page
- `xarchive` (Chrome extension) — <sup>[[30](https://github.com/sytelus/xarchive)]</sup>collects bookmarks through X.com's internal GraphQL API and maintains a cumulative archive per account. It follows the pages X returns without an application-imposed bookmark cap. Includes <sup>[[30](https://github.com/sytelus/xarchive)]</sup>folder assignments -- includes X Premium bookmark folders.
- Generic: CSV with a `url` column; HAR files (parse `Bookmarks` GraphQL responses out of a HAR captured from DevTools).

Define one normalized `ImportRecord` and write an adapter per format. Merge by tweet ID; API data wins on conflicts, but keep the richest media set.

### 3.4 Channel C — Companion extension (v2, optional)
A WebExtension that **passively** observes `Bookmarks` GraphQL responses while the user scrolls their own bookmarks page, and hands them to the app over `localhost` (Tauri can host a loopback endpoint with a per-session token). It injects nothing, sends nothing, and scrolls nothing. Explicitly deferred — ship A+B first and see if the 800 ceiling actually bites.

### 3.5 Media caching (be a polite CDN citizen)
- Lazy: thumbnails fetched on first viewport appearance; originals on open or via "Archive media" bulk action.
- Concurrency 3, 150ms jitter between requests, honest UA, honor 429/403 with a circuit breaker (pause media for 10 min).
- Stored under app data dir as `media/<sha256>.<ext>`; DB holds the hash. Content-addressed so re-syncs don't re-download.

## 4. Search & library features

| Feature | v1 | Notes |
|---|---|---|
| Keyword search | ✅ | SQLite FTS5, `trigram` tokenizer for substring/typo tolerance + `unicode61` table for ranked phrase matches. Debounced per keystroke, target <10ms. |
| Semantic search | ✅ | `fastembed-rs` (ONNX, local) with a small model (e.g. `bge-small-en-v1.5` or `snowflake-arctic-embed-xs`), vectors in `sqlite-vec`. Embed on ingest in a background task; UI shows "indexing 1,234/5,000". |
| Hybrid ranking | ✅ | Reciprocal Rank Fusion of FTS + vector results. Toggle per-query (`/kw`, `/sem` prefixes). |
| Filters | ✅ | author, date range, has:media / has:link / has:video, folder, lang, min likes, "deleted on X". |
| Folders | ✅ | X folders synced read-only; local **tags** and **notes** are yours. |
| Threads & quotes | ✅ | Render quoted post inline (from `referenced_tweets` expansion). Full-thread fetch = paid non-owned reads → explicit "Fetch thread ($0.0x)" button, never automatic. |
| Link previews | ✅ | Unfurl t.co → final URL once at ingest (HEAD request, 1 hop limit, same politeness rules). Index the final domain. |
| Dead-link detection | v1.1 | On incremental sync, bookmarks missing from the API get flagged; UI shows the cached copy with a "deleted on X" badge. |
| Export | ✅ | Markdown (one file per bookmark, Obsidian-friendly frontmatter), JSON (full), CSV. |
| Command palette | ✅ | `Cmd/Ctrl+K`: search, jump to author, filter, sync, export. The entire app should be usable with no mouse. |
| Global hotkey summon | v1.1 | `tauri-plugin-global-shortcut`, gyotaku-style quick window. |
| Random / "resurface" | v1.1 | Show N random old bookmarks on launch. Cheap, surprisingly loved. |

## 5. Architecture

<details>
<summary><strong>💻 Code Block — 12 lines</strong></summary>

```
┌───────────────────── Tauri 2.12 shell ─────────────────────┐
│  Frontend (webview)            │  Rust core (src-tauri)     │
│  Svelte 5 + TS + Tailwind v4   │  tauri commands (specta)   │
│  virtualised list              │  ├─ auth: oauth2 + keyring │
│  command palette               │  ├─ sync: reqwest + limiter│
│  tauri-specta typed bindings   │  ├─ import: adapters       │
│                                │  ├─ store: rusqlite+FTS5   │
│                                │  │          sqlite-vec      │
│                                │  ├─ embed: fastembed (ONNX)│
│                                │  ├─ media: cache + breaker │
│                                │  └─ events → progress UI   │
└────────────────────────────────┴────────────────────────────┘
```

</details>

**Rust crates:** `tauri 2.12`, `tauri-specta` (typed IPC), `reqwest` (rustls), `oauth2`, `keyring`, `rusqlite` (bundled, with `fts5`) + `sqlite-vec`, `fastembed`, `tokio`, `serde`, `thiserror`, `tracing`. Plugins: `opener`, `dialog`, `fs` (scoped to app data), `global-shortcut`, `updater`, `window-state`, `single-instance`.

**Frontend:** Svelte 5 (runes) or SolidJS — both are small and handle 10k-row virtualised lists without ceremony. React 19 is fine if you're faster in it. Keep *all* business logic in Rust; the frontend is a dumb renderer over typed commands and events.

**Workspace layout** (mirrors gyotaku's CLI+app split, which is a good pattern):
<details>
<summary><strong>💻 Code Block — 3 lines</strong></summary>

```
crates/core   – everything above the IPC line; no Tauri dependency
crates/cli    – `marginalia sync | search "foo" | export md` (great for dogfooding & CI)
crates/app    – Tauri shell
```

</details>

### 5.1 Data model (sketch)

<details>
<summary><strong>💻 Code Block (sql) — 13 lines</strong></summary>

```sql
users(id PK, username, name, avatar_url, avatar_hash, fetched_at)
posts(id PK, author_id FK, text, note_text, created_at, lang,
      conversation_id, like_count, repost_count, reply_count,
      raw_json, first_seen_at, last_seen_at, removed_at, source) -- 'api'|'import'
bookmarks(post_id PK FK, bookmarked_at, sort_index)              -- user-specific state
folders(id PK, name); folder_posts(folder_id, post_id)
media(key PK, post_id FK, type, url, preview_url, alt_text, w, h, blob_hash)
links(post_id, tco, expanded, final_url, domain, title)
refs(post_id, kind, ref_post_id)                                -- quoted/replied
tags(id, name); post_tags(post_id, tag_id); notes(post_id, md, updated_at)
sync_runs(id, started_at, finished_at, kind, pages, resources, est_cost, actual_rate, cursor)
posts_fts (FTS5: text, note_text, author_username, link_titles, alt_text; tokenize=trigram)
post_vecs (sqlite-vec: post_id, embedding float[384])
```

</details>

Keep `raw_json` always — it's cheap and it's your escape hatch when you want a field you didn't model.

## 6. Security & privacy
- Tauri capabilities: deny-by-default; the webview gets only the commands it needs, `fs` scoped to app data, no `shell`.
- CSP locked to `self` + `asset:`; media served through Tauri's asset protocol, not `pbs.twimg.com` directly (so the webview makes no network calls at all).
- Tokens: keychain only. Client ID in config (it's public by design for PKCE).
- No analytics, no crash reporting by default. Optional updater via `tauri-plugin-updater` with signed releases.
- Legal footer in onboarding: you're reading your own data via your own app under X's developer terms; file imports are your own files.

## 7. UX

**Onboarding (≤ 3 minutes)**
1. "Create a free X developer app" → step-by-step with screenshots → paste Client ID. (Explain *why*: cheaper reads, your keys.)
2. "Add credits" → explain pay-per-use, show the live estimate: "You have ~N bookmarks → about $X." (Note the new-account credits: <sup>[[44](https://developer.x.com/en/docs/x-api/tweets/bookmarks/introduction)]</sup>new pay-per-use accounts can earn up to $70 in free X API credits: $20 for saving a first payment card and a match of the first auto-recharge, up to $50.)
3. OAuth in system browser → back to app → sync starts with a progress event stream.
4. Or: "Skip, import a file instead."

**Main window:** three-pane — filters | results (virtualised, dense) | detail (rendered post, media, quoted post, notes, tags, "Open on X"). Search bar always focused on open.

**Empty states and errors should be specific:** "X is rate limiting; resuming at 14:32" beats "Sync failed."

## 8. Performance targets (gyotaku-calibrated)

| Metric | Target |
|---|---|
| Keyword search, 10k bookmarks | ≤ 10 ms per keystroke |
| Semantic search, 10k | ≤ 80 ms |
| Cold launch to interactive | ≤ 400 ms |
| Idle RSS | ≤ 60 MB (webview tax) |
| Full backfill, 5k bookmarks | ≤ 60 s wall clock, ≤ 50 API requests |
| Embedding throughput (CPU) | ≥ 50 posts/s |
| Bundle size | ≤ 15 MB without ONNX model; model downloaded on first run, SHA-256 pinned (exactly what gyotaku does: <sup>[[7](https://github.com/xevrion/gyotaku/releases/tag/models-v1)]</sup>the app checks both files against SHA-256 hashes pinned in its source.) |

## 9. Milestones

- **M0 (1 wk) — Spike.** OAuth loopback flow; one bookmarks page; inspect billing ledger; measure the 800-cap question. *Decide channel priorities from data.*
- **M1 (2 wk) — Core.** `crates/core`: schema, API sync w/ limiter + resumable cursors, FTS5 search, CLI. Dogfood from the terminal.
- **M2 (2 wk) — App.** Tauri shell, onboarding, list/detail, command palette, media cache, Markdown export.
- **M3 (1 wk) — Semantic.** fastembed + sqlite-vec + RRF. Background indexer with progress.
- **M4 (1 wk) — Importers.** twitter-web-exporter, xarchive, HAR, CSV.
- **M5 — Ship.** Signed builds (macOS notarised, Windows signed), updater, GitHub releases, the obligatory launch tweet with a 10-second search demo.

## 10. Risks & open questions

| Risk | Mitigation |
|---|---|
| Expansions billed as separate User Reads | Measure in M0; lazy author hydration |
| Owned Reads not applied to bookmarks (forum report) | Display actual rate from first sync; still cheap |
| API caps bookmarks at ~800 | Channel B importers; Channel C extension later |
| X changes pricing again (it's done so 4× in 18 months) | Rates fetched/estimated at runtime, not hardcoded; cost always shown pre-sync |
| BYO dev app is too much onboarding friction | Best-in-class guided setup; file-import path as the no-friction alternative |
| ONNX model download fails/offline | Keyword search works without it; semantic is additive |

## 11. Success metrics (local, opt-in, or just your own)
- Time from install → first search < 5 min.
- Search-to-open rate (did they find the thing?).
- Zero "my account got flagged" reports — because the design makes it structurally impossible.

---

If you want, the next useful artifact is M0 as actual code: the OAuth loopback flow + a single paginated fetch in `crates/core`, with the billing/800-cap measurements logged — that answers the two open questions that decide how much Channel B/C matters.

<details>
<summary><strong>📚 Sources (45)</strong></summary>

1. [xevrion - Overview](https://github.com/xevrion)
2. [Release gyotaku v0.1.0 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.0)
3. [Release gyotaku v0.1.4 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.4)
4. [Release gyotaku v0.1.2 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.2)
5. [Release gyotaku v0.1.1 · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/v0.1.1)
6. [xevrion/gyotaku — GitHub trending stats & insights](https://trendshift.io/repositories/282457)
7. [Release OCR models · xevrion/gyotaku](https://github.com/xevrion/gyotaku/releases/tag/models-v1)
8. [GitHub - xevrion/gyotaku: search every screenshot you've ever taken by the text inside it. native linux, offline, PaddleOCR + sqlite + gpui](https://github.com/xevrion/gyotaku)
9. [GitHub - Chiroyce1/gyotaku: search every screenshot you've ever taken by the text inside it. native linux, offline, PaddleOCR + sqlite + gpui](https://github.com/Chiroyce1/gyotaku)
10. [xevrion (@xevrion_the1) on X](https://x.com/xevrion_the1/status/2107047524662157818)
11. [Devcommunity](https://devcommunity.x.com/t/can-i-access-twitter-bookmarks-for-development-purpose-for-free/221728)
12. [Bookmarks - X](https://docs.x.com/x-api/posts/bookmarks/introduction)
13. [X (Twitter) API Pricing in 2026: All Tiers | Postproxy](https://postproxy.dev/blog/x-api-pricing-2026/)
14. [X API Pricing 2026: What Every Call Costs (Calculator)](https://www.upload-post.com/x-api-pricing/)
15. [X API Pricing 2026: Pay-Per-Use Rates, Limits, Costs](https://www.outstand.so/blog/x-api-pricing)
16. [X (Twitter) API Pricing Explained](https://zernio.com/blog/twitter-api-pricing)
17. [Twitter API Pricing 2026: Pay-Per-Use Costs & Alternatives | Xpoz Blog](https://www.xpoz.ai/blog/guides/understanding-twitter-api-pricing-tiers-and-alternatives/)
18. [X (Twitter) API Alternative - Sorsa API](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)
19. [X (Twitter) API Alternative - Sorsa API](https://api.sorsa.io/blog/twitter-api-pricing-2026)
20. [Tauri (software framework) - Wikipedia](https://en.wikipedia.org/wiki/Tauri_(software_framework))
21. [Blog](https://v2.tauri.app/blog/)
22. [Announcing Tauri 2.12](https://v2.tauri.app/blog/tauri-2.12/)
23. [Tauri Core Releases](https://tauri.app/release/core/)
24. [Tauri Ecosystem Releases](https://v2.tauri.app/release/)
25. [Announcing the Tauri v2 Beta Release](https://v2.tauri.app/blog/tauri-2-0-0-beta/)
26. [tauri](https://tauri.app/release/tauri/)
27. [Tauri in 2026: Build Cross-Platform Desktop Apps with Web Technologies (Better Than Electron)](https://dev.to/ottoaria/tauri-in-2026-build-cross-platform-desktop-apps-with-web-technologies-better-than-electron-11mo)
28. [tauri](https://v2.tauri.app/release/tauri/)
29. [Tech-insider](https://tech-insider.org/tauri-tutorial-cross-platform-rust-app-2026/)
30. [GitHub - sytelus/xarchive: Export your entire X.com (Twitter) bookmark collection — unlimited bookmarks, with folder assignments. Chrome extension, zero dependencies.](https://github.com/sytelus/xarchive)
31. [How to Export X (Twitter) Bookmarks to CSV Free (2026)](https://contextbolt.com/blog/export-twitter-bookmarks/)
32. [X (Twitter) Bookmarks Exporter, Free in 2026 | ContextBolt](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)
33. [Xmarks Sync - Wikipedia](https://en.wikipedia.org/wiki/Xmarks_Sync)
34. [GitHub - prinsss/twitter-web-exporter: Export tweets, bookmarks, lists and much more from Twitter(X) web app. (推文/书签/收藏/列表导出工具)](https://github.com/prinsss/twitter-web-exporter)
35. [How to download your X (Twitter) archive (and what's actually inside it) · Stashr](https://stashr.me/blog/download-twitter-archive)
36. [How to Export X (Twitter) Bookmarks (2026, Every Method)](https://www.marqly.com/blog/export-twitter-x-bookmarks)
37. [How to Export Twitter Bookmarks to PDF, CSV & Markdown](https://bookmarksave.com/blog/how-to-export-twitter-bookmarks-pdf-csv-markdown)
38. [Tweet Archivist](https://www.tweetarchivist.com/how-to-download-twitter-archive)
39. [Get Users Bookmarks - X](https://docs.x.com/x-api/users/get-bookmarks)
40. [X API changelog and release notes - X](https://docs.x.com/changelog)
41. [Devcommunity](https://devcommunity.x.com/t/x-api-pricing-update-owned-reads-now-0-001-other-changes-effective-april-20-2026/263025)
42. [X API pay-per-usage pricing and credits - X](https://docs.x.com/x-api/getting-started/pricing)
43. [X (Twitter) API Pricing: Complete Guide for 2026 - Blotato](https://www.blotato.com/blog/twitter-api-pricing)
44. [X Developer Platform - X](https://developer.x.com/en/docs/x-api/tweets/bookmarks/introduction)
45. [X API Pricing 2026: Costs, Limits, Hidden Charges](https://bundle.social/blog/x-api-pricing-2026-costs-limits)

</details>

---

## [3] USER

*10/8/2026, 1:00:16 AM*

we don't want to ingest using the x api. we want to go through and export bookmarks using tampermonkey script or some kind of browser automation. but carefully so that we don't run afoul of X

---

## [4A] ASSISTANT (claude-opus-4-6-search)

*10/8/2026, 1:00:16 AM*

Let me research the current state of browser-based bookmark export tools and anti-detection best practices to make this precise.<sup>[[1](https://greasyfork.org/en/scripts/513337-export-twitter-bookmarks)]</sup><sup>[[2](https://greasyfork.org/en/scripts/by-site/twitter.com?q=Tampermonkey&sort=created)]</sup><sup>[[3](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup><sup>[[4](https://greasyfork.org/en/scripts/492218-twitter-web-exporter)]</sup><sup>[[5](https://github.com/prinsss/twitter-web-exporter)]</sup><sup>[[6](https://github.com/kmccleary3301/scrollmark)]</sup><sup>[[7](https://gist.github.com/fdebijl/ecf9937a215a8e100d6c4f6ea5d0812f)]</sup><sup>[[8](https://github.com/olokkm/twitter-position-saver)]</sup><sup>[[9](https://en.wikipedia.org/wiki/List_of_augmented_browsing_software)]</sup><sup>[[10](https://greasyfork.org/en/scripts/513337-export-twitter-bookmarks/code)]</sup><sup>[[11](https://github.com/lhl/tweetxvault/)]</sup><sup>[[12](https://github.com/sytelus/xarchive)]</sup><sup>[[13](https://github.com/topics/twitter-bookmarks?o=desc&s=updated)]</sup><sup>[[14](https://github.com/tfriedel/tweethoarder)]</sup><sup>[[15](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)]</sup><sup>[[16](https://github.com/destefanis/twitter-bookmarks-grid)]</sup><sup>[[17](https://deepwiki.com/sahil-lalani/bookmark-export)]</sup><sup>[[18](https://divyajyotiuk.hashnode.dev/exporting-your-twitter-bookmarks-in-markdown-file)]</sup><sup>[[19](https://scrapfly.io/blog/posts/how-to-scrape-twitter)]</sup><sup>[[20](https://github.com/topics/anti-bot-detection?o=asc&s=updated)]</sup><sup>[[21](https://github.com/devlewicki/XActions)]</sup><sup>[[22](https://brightdata.com/blog/web-data/best-twitter-scrapers)]</sup><sup>[[23](https://alterlab.io/blog/how-to-scrape-twitter-x-complete-guide-for-2026)]</sup><sup>[[24](https://socialnexis.com/guides/twitter-automation-safe-2026)]</sup><sup>[[25](https://en.wikipedia.org/wiki/Bots_on_X)]</sup><sup>[[26](https://www.unfollr.com/blog/twitter-automation-rules)]</sup><sup>[[27](https://api.sorsa.io/blog/twitter-this-request-looks-like-automated)]</sup><sup>[[28](https://medium.com/@datajournal/web-scraping-intercepting-xhr-requests-38dc244c6f4e)]</sup><sup>[[29](https://github.com/connection-sphere/intercept)]</sup><sup>[[30](https://proxidize.com/blog/twitter-scraper/)]</sup><sup>[[31](https://pixelscan.net/blog/scraping-twitter-guide/)]</sup><sup>[[32](https://deepwiki.com/prinsss/twitter-web-exporter)]</sup><sup>[[33](https://deepwiki.com/xiaoyihao001018/xhs-web-exporter)]</sup><sup>[[34](https://github.com/eighteentang/eighteen-x-assistant/issues/5)]</sup><sup>[[35](https://deepwiki.com/h-koichiro/twitter-web-exporter)]</sup><sup>[[36](https://github.com/mackuba/bad_pigeon)]</sup>---

# PRD: **BookmarkVault** — Local-First X Bookmark Browser & Search Engine

**Stack:** Tauri v2 (Rust backend) · Svelte 5 or React 19 frontend · SQLite + FTS5 · Userscript-based ingestion

---

## 1. Problem

<sup>[[3](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>X/Twitter has no built-in export for bookmarks — your data archive doesn't include them. <sup>[[3](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>The X API returns only about the 800 most recent bookmarks, and most Chrome extensions hit this same wall because they rely on the same underlying API calls. Meanwhile, <sup>[[26](https://www.unfollr.com/blog/twitter-automation-rules)]</sup>X's enforcement against automated behavior has gotten dramatically stricter in 2026, with mass ban waves hitting accounts that use unauthorized tools.

So the challenge is: **export your entire bookmark library without using the paid API and without getting flagged**, then make it locally searchable, fast, and beautiful.

---

## 2. Goals & Non-Goals

### Goals
1. **Ingest all bookmarks** (not capped at 800) via a browser-native approach that is indistinguishable from human browsing.
2. Store everything **locally** (SQLite + FTS5), work **fully offline** after ingest.
3. **Instant full-text search** — sub-10ms per keystroke, filters, optional semantic search.
4. Rich browsing: media thumbnails, author info, thread context, user-defined tags/folders.
5. Native-feeling, beautiful UI worth showing off.
6. Incremental re-capture: pick up *new* bookmarks cheaply.

### Non-Goals (v1)
- Using the official X API at all.
- Writing back to X (add/remove bookmarks remotely).
- Cloud sync. Local-first only; optional encrypted file export.
- Mobile build (defer).

---

## 3. The Threat Model: How Not to Get Flagged

This is the critical design constraint and everything flows from it.

### What X detects

<sup>[[22](https://brightdata.com/blog/web-data/best-twitter-scrapers)]</sup>Twitter's detection stack includes Cloudflare WAF, TLS fingerprinting, behavioral analysis, and IP reputation scoring. <sup>[[23](https://alterlab.io/blog/how-to-scrape-twitter-x-complete-guide-for-2026)]</sup>X migrated from in-house bot detection to Cloudflare's Turnstile challenge on login walls and rate-limited endpoints. Turnstile performs browser fingerprinting, behavioral analysis, and passive challenges invisible to legitimate users but fatal to naive scrapers. Unlike a traditional CAPTCHA, Turnstile doesn't present a puzzle — it silently fails bot sessions before they reach content.

<sup>[[26](https://www.unfollr.com/blog/twitter-automation-rules)]</sup>Actions that were tolerated in 2024 are now grounds for suspension: follow/unfollow ratios that suggest churning, engagement patterns that are too regular (same times, same intervals), accounts that primarily interact with one network of accounts, rapid actions that exceed human typing/clicking speed.

<sup>[[27](https://api.sorsa.io/blog/twitter-this-request-looks-like-automated)]</sup>Browser automation frameworks like Selenium, Puppeteer, and Playwright leave detectable fingerprints.

### Why our approach is safe

The core insight — validated by multiple successful open-source projects — is: **don't send any requests yourself. Passively intercept the responses X's own web app already fetches.**

<sup>[[5](https://github.com/prinsss/twitter-web-exporter)]</sup>The script itself does not send any request to Twitter API. It installs a network interceptor to capture the response of GraphQL requests initiated by the Twitter web app. The script then parses the response and extracts data from it.

<sup>[[32](https://deepwiki.com/prinsss/twitter-web-exporter)]</sup>The exporter uses a passive capture model. It does not actively crawl Twitter's servers; instead, it waits for the user to navigate the site. As the Twitter web app requests data to render the UI, the exporter's ExtensionManager intercepts the response. A hook is placed on `XMLHttpRequest.prototype.open` to monitor URLs matching Twitter's GraphQL endpoints.

This means:
- **The user's real browser** with their real session, cookies, IP, and fingerprint.
- **Zero extra network requests.** The script only reads responses the browser was going to receive anyway.
- **Scrolling is done by the user** (or, optionally, by gentle auto-scroll at human cadence — see §5).
- <sup>[[24](https://socialnexis.com/guides/twitter-automation-safe-2026)]</sup>Browser-based automation continues to work. X's anti-bot signals haven't fundamentally changed; if a real session is doing real-shaped actions, the platform doesn't have a clean way to distinguish it from a human.

---

## 4. Ingestion Architecture: Two Modes

### Mode A — Companion Userscript (Primary, Recommended)

Ship a **Tampermonkey/Violentmonkey userscript** alongside the Tauri app. This is the ingestion engine.

**How it works technically:**

<sup>[[35](https://deepwiki.com/h-koichiro/twitter-web-exporter)]</sup>The UserScript captures Twitter data by intercepting GraphQL API responses made by the Twitter web application itself. The system operates entirely client-side within the browser environment, storing captured data in IndexedDB and providing export capabilities to JSON, CSV, HTML formats.

The proven architecture, as documented by the `twitter-web-exporter` project:

<sup>[[32](https://deepwiki.com/prinsss/twitter-web-exporter)]</sup>A hook is placed on `XMLHttpRequest.prototype.open` to monitor URLs matching Twitter's GraphQL endpoints. Specialized modules parse the complex nested JSON response into flat, typed entities. Data is saved to IndexedDB using Dexie. Users can filter captured data in the table view and invoke export pipelines to generate files.

**Our userscript's job is simpler than twitter-web-exporter's** — it only needs to:

1. **Hook `XMLHttpRequest.prototype.open`** and **`fetch`** to intercept responses whose URLs match the `Bookmarks` GraphQL endpoint pattern.
2. **Parse the nested GraphQL JSON** to extract tweet objects (text, author, media URLs, created_at, metrics, conversation_id).
3. **Dedupe by tweet ID** in a lightweight in-page `Map`.
4. **Write to a JSON file on each batch** (or use `postMessage` / a localhost WebSocket to stream directly to the running Tauri app — see Handoff below).
5. Show a **minimal floating badge** with a count of captured bookmarks.

The user's workflow:
1. Install userscript via Tampermonkey.
2. Open `x.com/i/bookmarks` in their normal browser.
3. Scroll through their bookmarks at their own pace.
4. <sup>[[3](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>twitter-web-exporter bypasses the API cap. It reads directly from the web page as you scroll, capturing everything the browser renders. If you have 3,000 bookmarks and the patience to scroll through them, it will capture all 3,000.
5. When done, export JSON → import into BookmarkVault. Or, if the Tauri app is running, data streams in live.

**Gentle auto-scroll (opt-in, off by default):**

If the user enables it, the script scrolls the page automatically but at **human cadence**:
- Scroll by a random viewport-fraction (0.6–0.9×) every 2–5 seconds (randomized).
- Pause for 5–15 seconds every 20–40 scrolls (simulating "reading").
- <sup>[[21](https://github.com/devlewicki/XActions)]</sup>Built-in 1–3s delays, human-like scrolling, auto-pause on rate limits.
- **Stop immediately** if the page shows any rate-limit UI, challenge, or error state (detected via DOM observation on known selectors).
- <sup>[[27](https://api.sorsa.io/blog/twitter-this-request-looks-like-automated)]</sup>X stops likes, replies, follows, and DMs when it reads your activity as bot-like, usually after acting too fast. Most people fix it by stopping all activity and waiting 15 to 60 minutes. Our auto-scroll does **no actions** (no likes, no follows, no clicks) — it *only* scrolls, which is a read-only operation and the lowest-risk activity possible.

**Post-read limits to be aware of:**

<sup>[[27](https://api.sorsa.io/blog/twitter-this-request-looks-like-automated)]</sup>X's reported daily limits as of June 2026 include post reads (timeline scrolling): 1,000/day for free accounts, 10,000/day for X Premium. This sets a natural ceiling on how many bookmarks can be ingested per session. The userscript should track scroll-triggered network requests and warn the user as they approach ~800 read-events, suggesting they stop and resume tomorrow.

### Mode B — File Import (Zero Risk, Fallback)

For users who already have exports from any tool:
- Import JSON/CSV from `twitter-web-exporter`, Scrollmark, xarchive, or any compatible format.
- <sup>[[6](https://github.com/kmccleary3301/scrollmark)]</sup>Scrollmark runs as a userscript on x.com and observes the same GraphQL/API responses that the X web app loads while you browse, parses useful structures out of those responses, stores them locally in IndexedDB, and gives you a fast explorer for search, review, export, and sharing. Its exports are directly importable.
- Parse `raw_json` fields opportunistically to future-proof against schema drift.

### Handoff: Userscript → Tauri App

Two options (implement both, prefer #1):

1. **Localhost WebSocket bridge.** The Tauri app runs a tiny WebSocket server on `127.0.0.1:PORT`. The userscript connects and streams captured tweet objects in real-time. The Tauri Rust backend receives, validates, dedupes, and inserts into SQLite. **This is the magic UX: you scroll in your browser and watch your BookmarkVault library grow in real-time in the Tauri window.**

2. **File-based.** The userscript writes/appends to a JSON file. The Tauri app watches a designated import directory (via `notify` crate / `fs::watch`) and auto-ingests new files.

---

## 5. Pacing & Safety Layer (Even Though We're Passive)

Even in passive-intercept mode, the *user's scrolling* triggers real GraphQL fetches. We need to pace that.

<details>
<summary><strong>💻 Code Block — 14 lines</strong></summary>

```
┌─────────────────────────────────────────────────┐
│           Safety Governor (userscript)           │
│                                                  │
│  • Count GraphQL bookmark responses received     │
│  • Track time window (15-min rolling)            │
│  • If count approaches read-limit threshold:     │
│    → Pause auto-scroll                           │
│    → Show "Take a break" badge                   │
│    → Log timestamp for resume                    │
│  • Detect DOM rate-limit / challenge screens     │
│    → Immediately halt, notify user               │
│  • Randomize ALL timing (never periodic)         │
│  • No synthetic XHR/fetch calls — ever           │
└─────────────────────────────────────────────────┘
```

</details>

<sup>[[26](https://www.unfollr.com/blog/twitter-automation-rules)]</sup>X flags engagement patterns that are too regular (same times, same intervals). So even scroll timing must have jitter. The auto-scroll uses a random distribution, not fixed intervals.

---

## 6. System Architecture

<details>
<summary><strong>💻 Code Block — 38 lines</strong></summary>

```
┌──────────────────────────────────────────────────────────────────┐
│                                                                  │
│  User's Browser (Chrome/Firefox/etc.)                            │
│  ┌────────────────────────────────────┐                          │
│  │  x.com/i/bookmarks                 │                          │
│  │  + BookmarkVault Userscript        │                          │
│  │    (Tampermonkey/Violentmonkey)    │                          │
│  │                                    │                          │
│  │  ┌─────────────┐  ┌────────────┐  │                          │
│  │  │ XHR/fetch   │  │ Safety     │  │                          │
│  │  │ interceptor │  │ Governor   │  │                          │
│  │  └──────┬──────┘  └────────────┘  │                          │
│  │         │ parsed tweet JSON       │                          │
│  │         ▼                         │                          │
│  │  ┌─────────────┐                  │                          │
│  │  │ WebSocket   │──── ws://127.0.0.1:17728 ──┐               │
│  │  │ or file     │                  │          │               │
│  │  │ export      │                  │          │               │
│  │  └─────────────┘                  │          │               │
│  └────────────────────────────────────┘          │               │
│                                                  ▼               │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │  BookmarkVault (Tauri v2)                                  │   │
│  │                                                            │   │
│  │  Rust Backend                      Frontend (WebView)      │   │
│  │  ┌──────────────────┐             ┌─────────────────────┐  │   │
│  │  │ WS server        │◄───IPC────►│ Svelte 5 / React 19 │  │   │
│  │  │ Ingest pipeline  │             │                     │  │   │
│  │  │   • validate     │             │ • Virtualized list  │  │   │
│  │  │   • dedupe       │             │ • Search bar (FTS5) │  │   │
│  │  │   • normalize    │             │ • Filter sidebar    │  │   │
│  │  │ SQLite + FTS5    │             │ • Detail panel      │  │   │
│  │  │ Media cache      │             │ • Tag/folder mgmt   │  │   │
│  │  │ Embed pipeline   │             │ • Import wizard     │  │   │
│  │  │ (optional)       │             │ • Live ingest view  │  │   │
│  │  └──────────────────┘             └─────────────────────┘  │   │
│  └───────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

</details>

### Tech Choices

| Layer | Choice | Why |
|---|---|---|
| Shell | Tauri v2 (stable) | Rust backend, tiny binary, cross-platform, security ACL |
| Frontend | Svelte 5 (or React 19) | Reactive, tiny bundle; Svelte's compiler = fewer DOM nodes |
| Virtualization | TanStack Virtual | 10k+ rows at 60fps |
| DB | `rusqlite` in Rust core | Direct control over FTS5, WAL mode, pragma tuning |
| FTS | SQLite FTS5 | Sub-ms full-text queries, BM25 ranking, `highlight()` |
| Async | Tokio | WS server + file watcher + media downloads |
| Media cache | `reqwest` → app data dir | Lazy thumbnail download, keyed by URL hash |
| Secrets | `keyring` crate | If any tokens needed (WS auth, etc.) |
| Semantic (opt) | `fastembed-rs` (ONNX) + `sqlite-vec` | Local embeddings, zero cloud dependency |
| Userscript | TypeScript, bundled via Vite + `vite-plugin-monkey` | Same toolchain as `twitter-web-exporter` |

---

## 7. Userscript: Technical Spec

### XHR/Fetch Interception

The core technique, proven by the ecosystem:

<details>
<summary><strong>💻 Code Block (typescript) — 31 lines</strong></summary>

```typescript
// Monkey-patch XMLHttpRequest.prototype.open
const originalOpen = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function(method: string, url: string, ...args: any[]) {
  this._bvUrl = url;
  return originalOpen.apply(this, [method, url, ...args]);
};

const originalSend = XMLHttpRequest.prototype.send;
XMLHttpRequest.prototype.send = function(...args: any[]) {
  this.addEventListener('load', function() {
    if (this._bvUrl?.includes('/graphql/') && this._bvUrl?.includes('Bookmarks')) {
      try {
        const data = JSON.parse(this.responseText);
        extractAndForward(data);
      } catch(e) { /* swallow parse errors */ }
    }
  });
  return originalSend.apply(this, args);
};

// Also patch fetch() for completeness
const originalFetch = window.fetch;
window.fetch = async function(input: RequestInfo, init?: RequestInit) {
  const response = await originalFetch.apply(this, [input, init]);
  const url = typeof input === 'string' ? input : input.url;
  if (url.includes('/graphql/') && url.includes('Bookmarks')) {
    const clone = response.clone();
    clone.json().then(data => extractAndForward(data)).catch(() => {});
  }
  return response;
};
```

</details>

### GraphQL Response Parsing

X's bookmark responses nest tweets inside a timeline instruction structure. The parser must:
1. Walk `data.bookmark_timeline_v2.timeline.instructions[]`
2. Find entries of type `TimelineAddEntries`
3. For each entry, extract `tweet_results.result` objects
4. Normalize into flat `BookmarkTweet` records:

<details>
<summary><strong>💻 Code Block (typescript) — 19 lines</strong></summary>

```typescript
interface BookmarkTweet {
  id: string;                    // tweet snowflake ID
  text: string;                  // full_text
  author_id: string;
  author_handle: string;
  author_name: string;
  author_avatar_url: string;
  created_at: string;            // ISO 8601
  lang: string;
  conversation_id: string;
  in_reply_to_id: string | null;
  media: { url: string; type: string; width: number; height: number; alt: string }[];
  metrics: { likes: number; retweets: number; replies: number; views: number };
  urls: { display: string; expanded: string }[];
  card: object | null;           // link preview
  folder_id: string | null;      // if captured from folder view
  captured_at: string;           // ISO 8601, when we intercepted it
  raw_json: object;              // full original for future-proofing
}
```

</details>

### Endpoint Matching

<sup>[[19](https://scrapfly.io/blog/posts/how-to-scrape-twitter)]</sup>X.com's API hides behind three rotating guards: guest tokens, doc_ids, rate limits. <sup>[[19](https://scrapfly.io/blog/posts/how-to-scrape-twitter)]</sup>Every 2–4 weeks, X.com adjusts guest tokens, doc_ids, rate limits, or detection patterns.

The GraphQL query IDs (doc_ids) in the URL change periodically. Our intercept doesn't care — we match on the **operation name** (`Bookmarks`, `BookmarkFolders`) in the URL path, not the query hash. This makes us resilient to ID rotation.

### WebSocket Handoff

<details>
<summary><strong>💻 Code Block (typescript) — 18 lines</strong></summary>

```typescript
// In userscript
const ws = new WebSocket('ws://127.0.0.1:17728/ingest');

function extractAndForward(graphqlResponse: any) {
  const tweets = parseBookmarkResponse(graphqlResponse);
  for (const tweet of tweets) {
    if (!seen.has(tweet.id)) {
      seen.add(tweet.id);
      counter++;
      updateBadge(counter);
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'bookmark', data: tweet }));
      } else {
        buffer.push(tweet);  // buffer for file export fallback
      }
    }
  }
}
```

</details>

On the Tauri/Rust side: a `tokio-tungstenite` WebSocket server on `127.0.0.1:17728` receives each tweet, validates the schema, and inserts into SQLite inside a transaction.

---

## 8. Data Model (SQLite)

<details>
<summary><strong>💻 Code Block (sql) — 77 lines</strong></summary>

```sql
CREATE TABLE tweets (
  id              TEXT PRIMARY KEY,   -- snowflake
  text            TEXT NOT NULL,
  author_id       TEXT NOT NULL,
  author_handle   TEXT NOT NULL,
  author_name     TEXT NOT NULL,
  author_avatar   TEXT,
  created_at      INTEGER NOT NULL,   -- unix timestamp
  lang            TEXT,
  conversation_id TEXT,
  in_reply_to_id  TEXT,
  metrics         TEXT,               -- JSON: {likes, retweets, replies, views}
  urls            TEXT,               -- JSON array
  card            TEXT,               -- JSON, nullable
  folder_id       TEXT,
  captured_at     INTEGER NOT NULL,
  source          TEXT NOT NULL,       -- 'userscript' | 'ws_live' | 'file_import'
  raw_json        TEXT                -- full GraphQL payload
);

CREATE TABLE media (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  tweet_id    TEXT NOT NULL REFERENCES tweets(id),
  url         TEXT NOT NULL,
  type        TEXT NOT NULL,           -- 'photo' | 'video' | 'gif'
  local_path  TEXT,                    -- after download
  width       INTEGER,
  height      INTEGER,
  alt_text    TEXT
);

CREATE TABLE authors (
  id          TEXT PRIMARY KEY,
  handle      TEXT NOT NULL,
  name        TEXT NOT NULL,
  avatar_url  TEXT,
  verified    INTEGER DEFAULT 0,
  updated_at  INTEGER
);

CREATE TABLE folders (
  id    TEXT PRIMARY KEY,
  name  TEXT NOT NULL
);

CREATE TABLE tags (
  id    INTEGER PRIMARY KEY AUTOINCREMENT,
  name  TEXT NOT NULL UNIQUE
);

CREATE TABLE tweet_tags (
  tweet_id TEXT REFERENCES tweets(id),
  tag_id   INTEGER REFERENCES tags(id),
  PRIMARY KEY (tweet_id, tag_id)
);

-- Full-text search
CREATE VIRTUAL TABLE tweets_fts USING fts5(
  text,
  author_handle,
  author_name,
  content = 'tweets',
  content_rowid = 'rowid',
  tokenize = 'unicode61 remove_diacritics 2'
);

-- Triggers to keep FTS in sync
CREATE TRIGGER tweets_ai AFTER INSERT ON tweets BEGIN
  INSERT INTO tweets_fts(rowid, text, author_handle, author_name)
  VALUES (new.rowid, new.text, new.author_handle, new.author_name);
END;

-- Optional: semantic vector index
-- CREATE VIRTUAL TABLE tweet_vectors USING vec0(
--   tweet_id TEXT PRIMARY KEY,
--   embedding FLOAT[384]
-- );
```

</details>

---

## 9. Search & Browse UX

### Instant Search
- Query FTS5 on every keystroke, debounced ~30ms.
- `SELECT highlight(tweets_fts, 0, '<mark>', '</mark>') ... WHERE tweets_fts MATCH ? ORDER BY bm25(tweets_fts)` — sub-ms on thousands of rows.
- Results rendered in a **virtualized list** (TanStack Virtual) so the DOM stays lean even with 10k+ results.

### Filters
- **Author** (autocomplete from `authors` table)
- **Date range** (calendar picker)
- **Has media** / **Has link** / **Has thread**
- **Folder** (mirroring X's bookmark folders if captured)
- **Tag** (user-created)
- **Source** (userscript vs. import)

### Semantic Search (opt-in, M3)
- Run `fastembed-rs` with a small model (e.g., `all-MiniLM-L6-v2`, ~23MB ONNX) to embed tweet text.
- Store vectors in `sqlite-vec`.
- Hybrid search: FTS5 BM25 score + vector cosine similarity, blended with a tunable weight.
- Fully local — no data leaves the machine.

### Detail View
- Full tweet text with media rendered.
- Thread context (other tweets with same `conversation_id`).
- User-added tags and notes.
- "Open on X" link.
- Copy/share as markdown.

### Keyboard-First
- Global hotkey to summon window (configurable, e.g., `Cmd+Shift+B`).
- `/` to focus search.
- `j`/`k` to navigate results.
- `Enter` to open detail, `Esc` to close.
- `t` to add tag, `o` to open on X.

---

## 10. Live Ingest View (the wow feature)

When the WebSocket bridge is active, the Tauri app shows a **"Live Capture"** panel:
- New bookmarks appear in real-time as the user scrolls in their browser.
- Each new tweet slides in with a subtle animation.
- Running count: "247 bookmarks captured this session."
- Status badge: 🟢 Connected / 🟡 Buffering / 🔴 Disconnected.
- Progress estimate: "~1,200 remaining" (based on scroll position vs. observed pagination cursors).

This is the demo moment — split-screen your browser and BookmarkVault, scroll through your bookmarks, and watch the library build itself.

---

## 11. Continuous Capture (Save-as-you-go)

Beyond bulk export, support capturing **new bookmarks as they happen**:

<sup>[[3](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>ContextBolt imports your visible bookmarks and copies each new save as you make it, no separate export step.

When the userscript is installed and the user is browsing X normally (not just on the bookmarks page), we can also intercept the **bookmark mutation response** — the GraphQL response X returns when the user taps the bookmark icon on any tweet. This means:
- Every new bookmark is captured the moment it's saved.
- No need to revisit the bookmarks page.
- The WebSocket streams it to BookmarkVault instantly.
- Over time, this eliminates the need for bulk scrolling entirely.

---

## 12. Security & Privacy

- **Local-first, offline by default.** <sup>[[5](https://github.com/prinsss/twitter-web-exporter)]</sup>Your data never leaves your computer.
- No OAuth tokens, no API keys, no accounts to create. The userscript runs in the user's own authenticated browser session.
- The WebSocket server binds to `127.0.0.1` only — not accessible from the network.
- Tauri v2 **capabilities ACL**: grant the frontend only the IPC commands it needs. All DB access, file I/O, and network stays in Rust core.
- No telemetry. No analytics. No phone-home.
- Optional **encrypted export** (`age`/`rage` crate) for backups.
- The userscript is open-source and auditable. <sup>[[3](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>Open source — you can inspect the code and verify it does not send data anywhere.

---

## 13. Resilience & Schema Drift

<sup>[[19](https://scrapfly.io/blog/posts/how-to-scrape-twitter)]</sup>Every 2–4 weeks, X.com adjusts guest tokens, doc_ids, rate limits, or detection patterns. <sup>[[16](https://github.com/destefanis/twitter-bookmarks-grid)]</sup>Twitter's GraphQL query IDs can change when they update their web app. If the sync fails, you may need to update the query IDs.

Mitigations:
1. **Match on operation name, not query ID.** URL interception uses a regex like `/\/graphql\/\w+\/Bookmarks/` — the hash changes but the name doesn't (historically stable).
2. **Store `raw_json`.** Even if the parser breaks, raw data is preserved. A future parser update can re-process stored raw payloads.
3. **Versioned parser adapter.** The tweet-extraction logic is a single swappable module with version tags. When X changes the response shape, ship a userscript update that adds a new parser version while keeping old ones for reprocessing.
4. **Fallback: file import.** If the userscript breaks, the user can always use any other export tool and import the resulting JSON/CSV.

---

## 14. Milestones

| Phase | Scope | What ships |
|---|---|---|
| **M0 — Skeleton** | 2–3 weeks | Tauri v2 app shell. SQLite schema. File import (JSON/CSV). FTS5 search. Virtualized browse list. Keyboard nav. *Immediately useful for anyone with an existing export.* |
| **M1 — Userscript + Live Bridge** | 2–3 weeks | Companion userscript (intercept + parse + badge). WebSocket bridge. Live ingest view. Deduplication. Auto-scroll (off by default). |
| **M2 — Polish & Delight** | 2 weeks | Media thumbnail cache. Detail view with thread context. Tags, folders, notes. Global hotkey. Theme (dark/light). Command palette. |
| **M3 — Semantic Search** | 1–2 weeks | Local embeddings (`fastembed-rs`). Vector storage (`sqlite-vec`). Hybrid FTS + vector search. |
| **M4 — Continuous Capture** | 1 week | Bookmark-mutation interception. Background capture while browsing normally. Incremental sync indicator. |

---

## 15. Key Risks & Mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| **User scrolls too fast, hits read limit** | Medium | <sup>[[27](https://api.sorsa.io/blog/twitter-this-request-looks-like-automated)]</sup>X has daily post-read limits: 1,000/day for free accounts, 10,000/day for Premium. Safety Governor tracks request count, warns at ~80%, auto-pauses auto-scroll. Suggest spreading bulk export across 2–3 sessions. |
| **X changes GraphQL response schema** | Medium | Store `raw_json`; versioned parser adapter; operation-name matching (not query-ID). Community can PR parser updates. |
| **X adds CSP blocking WebSocket to localhost** | Low | Fall back to file-based export (JSON download → Tauri file watcher). |
| **User gets "automated request" warning** | Low | <sup>[[5](https://github.com/prinsss/twitter-web-exporter)]</sup>The export process is not automated. It relies on human interaction to trigger the data fetching process of the Twitter web app. The script itself does not send any request to Twitter API. We generate zero extra traffic. Risk only arises from overly fast manual scrolling, mitigated by the Safety Governor. |
| **Tampermonkey/Violentmonkey ecosystem changes** | Low | Userscript is a single `.user.js` file; trivially portable to any script manager or even a bookmarklet. |
| **Tauri v2 breaking changes** | Low | Tauri v2 is stable; pin versions. |

---

## 16. Why This Demos Well

The hook is the **split-screen moment**: browser on the left showing `x.com/i/bookmarks`, BookmarkVault on the right, and as you lazily scroll your bookmarks, each tweet pops into the app in real-time through the WebSocket bridge. Then you close the browser, go offline, type a search query, and results appear in single-digit milliseconds across thousands of bookmarks.

No API key. No paid tier. No cloud. No bot risk. Just your own data, finally searchable.

---

**Decision point before building:** Should the userscript be a standalone `.user.js` published on Greasy Fork (maximum reach, works without the Tauri app for pure export), or bundled exclusively with BookmarkVault? I'd recommend **both** — publish the userscript standalone for reach and community goodwill, then have BookmarkVault's onboarding one-click install it with the WebSocket bridge pre-configured. Want me to spec the userscript or the Rust ingest pipeline in detail next?

<details>
<summary><strong>📚 Sources (36)</strong></summary>

1. [Export Twitter Bookmarks](https://greasyfork.org/en/scripts/513337-export-twitter-bookmarks)
2. [Greasyfork](https://greasyfork.org/en/scripts/by-site/twitter.com?q=Tampermonkey&sort=created)
3. [How to Export X (Twitter) Bookmarks to CSV Free (2026)](https://contextbolt.com/blog/export-twitter-bookmarks/)
4. [Greasyfork](https://greasyfork.org/en/scripts/492218-twitter-web-exporter)
5. [GitHub - prinsss/twitter-web-exporter: Export tweets, bookmarks, lists and much more from Twitter(X) web app. (推文/书签/收藏/列表导出工具)](https://github.com/prinsss/twitter-web-exporter)
6. [GitHub - kmccleary3301/scrollmark: Scrollmark: local-first X/Twitter research archive, search, bookmark capture, and portable bundle export.](https://github.com/kmccleary3301/scrollmark)
7. [Twitter Like Exporter](https://gist.github.com/fdebijl/ecf9937a215a8e100d6c4f6ea5d0812f)
8. [GitHub - olokkm/twitter-position-saver: Tampermonkey script that bookmarks your Twitter/X timeline position and scrolls back to it – never lose your place again](https://github.com/olokkm/twitter-position-saver)
9. [List of augmented browsing software - Wikipedia](https://en.wikipedia.org/wiki/List_of_augmented_browsing_software)
10. [Greasyfork](https://greasyfork.org/en/scripts/513337-export-twitter-bookmarks/code)
11. [GitHub - lhl/tweetxvault: Archive you Twitter/X likes and bookmarks](https://github.com/lhl/tweetxvault/)
12. [GitHub - sytelus/xarchive: Export your entire X.com (Twitter) bookmark collection — unlimited bookmarks, with folder assignments. Chrome extension, zero dependencies.](https://github.com/sytelus/xarchive)
13. [Build software better, together](https://github.com/topics/twitter-bookmarks?o=desc&s=updated)
14. [GitHub - tfriedel/tweethoarder: dumps twitter likes & bookmarks](https://github.com/tfriedel/tweethoarder)
15. [X (Twitter) Bookmarks Exporter, Free in 2026 | ContextBolt](https://contextbolt.com/bookmarks/export-twitter-bookmarks/)
16. [GitHub - destefanis/twitter-bookmarks-grid: An infinite, pannable masonry grid for browsing your Twitter/X bookmarks visually](https://github.com/destefanis/twitter-bookmarks-grid)
17. [Deepwiki](https://deepwiki.com/sahil-lalani/bookmark-export)
18. [Divyajyotiuk](https://divyajyotiuk.hashnode.dev/exporting-your-twitter-bookmarks-in-markdown-file)
19. [How to Scrape Twitter (X.com) Data in 2026](https://scrapfly.io/blog/posts/how-to-scrape-twitter)
20. [Build software better, together](https://github.com/topics/anti-bot-detection?o=asc&s=updated)
21. [GitHub - devlewicki/XActions: ⚡ The Complete X/Twitter Automation Toolkit — Scrapers, MCP server for AI agents (Claude/GPT), CLI, browser scripts. No API fees. Open source. Unfollow people who don't follow back. Monitor real-time analytics. Auto follow, like, comment, scrape, without API. Follow Bot. Like bot. Grow your account automatically.](https://github.com/devlewicki/XActions)
22. [The 8 Best Twitter Scrapers in 2026: Ranked and Tested](https://brightdata.com/blog/web-data/best-twitter-scrapers)
23. [Scrape Twitter/X in 2026: Selectors, API & Code Examples](https://alterlab.io/blog/how-to-scrape-twitter-x-complete-guide-for-2026)
24. [X (Twitter) Automation Rules and Rate Limits in 2026 · SocialNexis](https://socialnexis.com/guides/twitter-automation-safe-2026)
25. [Bots on X - Wikipedia](https://en.wikipedia.org/wiki/Bots_on_X)
26. [Twitter/X Automation Rules in 2026: What's Allowed](https://www.unfollr.com/blog/twitter-automation-rules)
27. [X (Twitter) API Alternative - Sorsa API](https://api.sorsa.io/blog/twitter-this-request-looks-like-automated)
28. [Medium](https://medium.com/@datajournal/web-scraping-intercepting-xhr-requests-38dc244c6f4e)
29. [GitHub - connection-sphere/intercept: Intercept AJAX / XHR calls to extract information / scraping.](https://github.com/connection-sphere/intercept)
30. [How to Scrape Twitter/X for Free with Python in 2026](https://proxidize.com/blog/twitter-scraper/)
31. [Scraping Twitter with Python in 2026: Guide how to start | Pixelscan](https://pixelscan.net/blog/scraping-twitter-guide/)
32. [Deepwiki](https://deepwiki.com/prinsss/twitter-web-exporter)
33. [Deepwiki](https://deepwiki.com/xiaoyihao001018/xhs-web-exporter)
34. [技术前提 · 逆向 X 私有接口（hook fetch/XHR → GraphQL） · Issue #5 · eighteentang/eighteen-x-assistant](https://github.com/eighteentang/eighteen-x-assistant/issues/5)
35. [Deepwiki](https://deepwiki.com/h-koichiro/twitter-web-exporter)
36. [GitHub - mackuba/bad_pigeon: A tool for extracting tweet data from GraphQL requests made by the Twitter website 🐦](https://github.com/mackuba/bad_pigeon)

</details>

---

## [4B] ASSISTANT (claude-fable-5-search)

*10/8/2026, 1:00:16 AM*

Good call to interrogate this, because the risk profile here isn't binary — it's a spectrum, and where you sit on it is *the* design decision. Here's the revised PRD with the API ripped out and ingestion rebuilt around the browser.

First, the threat model that drives everything:

- X's enforcement is behavioral, not just quota-based. <sup>[[15](https://www.tendx.app/blog/x-twitter-limits-2026)]</sup>X doesn't publish numbers for likes, bookmarks, or search queries. These run through automated detection systems that flag "unusual activity" rather than hitting you with a fixed number.
- What actually trips it: <sup>[[17](https://voidmob.com/blog/how-to-scrape-twitter-x-2026)]</sup>X locks accounts that look automated: logins from datacenter or VPN IPs, several accounts on one IP, VoIP or missing phone numbers, location jumps between sessions, headless browsers, and sudden high volume from a new account, and on the browser side, <sup>[[17](https://voidmob.com/blog/how-to-scrape-twitter-x-2026)]</sup>headless Chromium, a fresh profile every run and a timezone that disagrees with the IP all read as automation, because detection systems cross-check fingerprint and network signals.
- Volume matters even in a real browser: <sup>[[16](https://www.getxapi.com/twitter-api-rate-limits)]</sup>reading posts / scrolling caps around ~1,000/day for unverified accounts, with 15–30 min rolling windows, and common triggers include <sup>[[16](https://www.getxapi.com/twitter-api-rate-limits)]</sup>refreshing the timeline too often, scrolling through long search results quickly, visiting many profiles in succession.

The conclusion falls out immediately: **the safe design is passive capture in the user's real browser session — intercept what X already sent, generate zero synthetic requests.** The prior art proves this works and is socially accepted: twitter-web-exporter has operated this way for years. <sup>[[1](https://github.com/prinsss/twitter-web-exporter)]</sup>Basically, the script "sees" what you see — <sup>[[5](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>it reads from the session as you scroll, capturing everything the browser renders. If you have 3,000 bookmarks and the patience to scroll through them, it will capture all 3,000. Nobody gets banned for scrolling their own bookmarks.

So: **don't drive the browser. Ride along in it.**

---

# PRD v2: **Marginalia** — ingestion via companion capture, no API

## 1. One-liner

A local-first Tauri v2 desktop app for searching your X bookmarks, fed by a companion userscript that passively records your bookmarks *as you scroll them yourself* — no API keys, no automation fingerprint, no synthetic traffic.

## 2. The ingestion ladder (explicit risk tiers)

The product offers three tiers. Tier 1 and 2 ship in v1. Tier 3 is opt-in, throttled, and clearly labeled. There is no Tier 4.

| Tier | What it does | Extra requests to X | Risk |
|---|---|---|---|
| **1. Passive capture** | Userscript hooks `fetch`/XHR, records `Bookmarks` GraphQL responses the page *already* made while the user scrolls normally | **Zero** | ~Zero — indistinguishable from browsing, because it *is* browsing |
| **2. File import** | Parse exports from twitter-web-exporter / similar tools | Zero | Zero |
| **3. Assisted scroll** (opt-in) | Userscript scrolls *for* you in your real, visible browser tab, at human pace, with hard budgets | Only the pagination requests normal scrolling would trigger | Low, non-zero — mitigations below |
| ~~4. Headless/CDP automation~~ | Playwright/Puppeteer/CDP, cookie reuse in the Rust backend | — | **Never.** Hard product principle. |

Why Tier 4 is banned outright: headless/fresh-profile automation is precisely the fingerprint X's detection cross-checks, and the DIY-automation route's documented failure mode is <sup>[[17](https://voidmob.com/blog/how-to-scrape-twitter-x-2026)]</sup>"account locked". Also architectural: the moment your Rust backend holds the user's `auth_token`/`ct0` cookies and speaks GraphQL directly, you've built a scraper with your user's account as ammunition. The userscript boundary keeps all X traffic inside the user's genuine browser, genuine session, genuine fingerprint, genuine IP.

## 3. Component 1: the companion userscript (`marginalia-capture.user.js`)

Shipped for Tampermonkey/Violentmonkey (and later as a thin WebExtension wrapping the same core — same model as twitter-web-exporter, which <sup>[[1](https://github.com/prinsss/twitter-web-exporter)]</sup>installs via Tampermonkey or Violentmonkey with one click).

### 3.1 Passive capture (default mode)

- **Interception, not DOM scraping.** Wrap `window.fetch` and `XMLHttpRequest` before X's bundle loads (`@run-at document-start`); match response URLs against `/i/api/graphql/*/Bookmarks*` and bookmark-folder operations; clone and parse the JSON. This gets you the *full* tweet objects — note_tweet (long posts), media variants with bitrates, quoted tweet payloads, card/link data, exact `sort_index` — everything the DOM-scraping console scripts lose. (Those scripts read `article[data-testid="tweet"]` nodes and get only visible text; strictly worse.)
- **Never replay, never originate.** The script makes no requests to x.com. It can't trip rate limits because it doesn't spend them.
- **Capture everything useful while we're there:** the same hook can record folder-timeline responses when the user opens a bookmark folder, so folders sync for Premium users.
- **UI:** a tiny floating pill on `/i/bookmarks`: "Marginalia: capturing — 412 bookmarks seen · Send to app." Progress count is the core feedback loop (twitter-web-exporter validated this pattern: <sup>[[5](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>the script captures bookmarks as you scroll, and the panel shows a running count).
- **Resumable by design:** the app knows which IDs it has; the pill shows "368 new / 44 already synced," so a user with 5k bookmarks can do it over several casual sessions rather than one marathon scroll.

### 3.2 Assisted scroll (opt-in, "Careful Mode" is the only mode)

For users who don't want to flick a trackpad 400 times. Crucial distinction from automation: this runs **in the user's visible, foreground, real browser tab**, in their real session — the only thing synthesized is scroll position. Mitigations, all non-negotiable defaults:

- **Human-shaped pacing:** scroll increments sampled from a distribution (300–900px), inter-scroll delay 1.5–4s with jitter, occasional longer "reading pauses" (5–15s every 8–20 scrolls). Never machine-speed — <sup>[[17](https://voidmob.com/blog/how-to-scrape-twitter-x-2026)]</sup>volume and pace matter: scrolling at machine speed trips rate limits and then reviews.
- **Hard session budget:** default stop after ~600 newly captured bookmarks or 20 minutes, whichever first — deliberately far inside the <sup>[[16](https://www.getxapi.com/twitter-api-rate-limits)]</sup>~1,000/day read cap for unverified accounts. Resume tomorrow; the resumable sync makes this painless.
- **Tripwires:** on any 429, any `KeepAlive`/error overlay, or X's "rate limit exceeded" toast → stop instantly, back off for the session, tell the user plainly. Never auto-retry.
- **Attention guard:** pause when the tab loses visibility (`visibilitychange`) — both polite and realistic.
- **Cooldown ledger:** the app tracks captured-per-day and refuses to start an assisted session if the user's recent volume is high.
- **No parallelism, ever.** One tab, one session, one account.

### 3.3 The pagination wall (set expectations honestly)

Known constraint to design around: <sup>[[13](https://contextbolt.com/blog/twitter-bookmark-limit/)]</sup>the web app practically stops surfacing at roughly 800 to 1,000 saves — older bookmarks remain on the account but stop appearing as you scroll (reports vary; twitter-web-exporter advertises <sup>[[2](https://greasyfork.org/en/scripts/492218-twitter-web-exporter)]</sup>export without the max 800 limit, so the wall may be inconsistent or folder-dependent). Product responses:

- **Measure, don't assume** — M0 spike includes scrolling a real >1k-bookmark account and logging where pagination cursors die.
- **Folder workaround:** folders paginate independently; users who've foldered old bookmarks can recover them per-folder.
- **"Unbookmark-as-you-archive" (v1.1, opt-in):** once a bookmark is safely stored locally, offer to remove it on X — this drains the stack so older items surface. Uses the page's own UI affordance via simulated click at human pace within assisted-scroll budgets; destructive, so triple-confirmed and off by default.
- Above all: **this is why the app exists.** Once captured, bookmarks live in your local DB forever; the wall only limits first backfill of ancient items.

### 3.4 Transport: userscript → app

Two paths, both shipping:

1. **Live handoff (primary):** Tauri hosts `http://127.0.0.1:<port>` (loopback only). Pairing: app displays a short-lived token; user pastes it into the pill once; script then POSTs captured batches with the token. CORS pinned, token rotated per pairing, port randomized. Nothing listens unless the app is open; graceful fallback to path 2 if it isn't.
2. **File drop:** "Export captured" button downloads NDJSON; drag the file onto the app. Zero-config, works offline, doubles as backup.

Store **raw GraphQL JSON** per tweet alongside normalized columns — X's response shapes drift, and raw payloads let you re-normalize later without re-capturing.

## 4. Component 2: file importers (Tier 2)

Adapters normalizing to one `ImportRecord`:

- **twitter-web-exporter** JSON/CSV — it <sup>[[1](https://github.com/prinsss/twitter-web-exporter)]</sup>automatically captures on the bookmark page and <sup>[[1](https://github.com/prinsss/twitter-web-exporter)]</sup>exports JSON, CSV and HTML; many target users already have these files.
- **Other extension exports** (e.g. rxliuli's Twitter Exporter, which handles <sup>[[6](https://addons.mozilla.org/en-US/firefox/addon/twitter-exporter/)]</sup>bookmarks — all or by folder) — JSON/CSV best-effort.
- **HAR files** from DevTools (power-user path: scroll with Network tab recording, save HAR, import — passive capture with zero installs).
- **Generic CSV** with a `url`/`tweet_id` column (text-only stubs, enrichable later via re-capture).

Merge policy: by tweet ID; richest record wins per field; never discard media references.

## 5. What doesn't change from the previous PRD (condensed)

- **Stack:** Tauri 2.12, Rust core crate with no Tauri dependency + CLI + app shell; Svelte 5 frontend; `tauri-specta` typed IPC; rusqlite (FTS5 trigram) + `sqlite-vec` + `fastembed` ONNX for hybrid keyword/semantic search with RRF; content-addressed media cache; everything local, no telemetry.
- **Media caching** politeness still applies — it's the one place the *app itself* touches Twitter's CDN (`pbs.twimg.com`). Concurrency ≤3, jittered, circuit breaker on 429/403. Alternative worth testing in M0: have the *userscript* fetch media blobs in-session and ship them over the loopback with the JSON — then the app makes literally zero network requests to anything X-owned.
- **Search/library features, data model, perf targets** (≤10ms keyword search at 10k, ≤400ms cold start) — unchanged.
- "Deleted on X" detection changes meaning without an API: instead of diffing against fetches, mark a bookmark stale only if a later capture session covers its sort-range and it's absent.

## 6. Security specifics for the new surface

- Loopback endpoint: random high port, loopback bind only, bearer token from pairing, request body size caps, rejects anything not matching the capture schema.
- The userscript stores nothing sensitive (no cookies, no tokens from X — it only ever sees response *bodies* the page already received) and sends data only to `127.0.0.1`.
- Userscript is open source and tiny (~500 lines) — auditable in one sitting; this matters for trust, exactly as it does for prior art (<sup>[[5](https://contextbolt.com/blog/export-twitter-bookmarks/)]</sup>open source — you can inspect the code and verify it does not send data anywhere).
- App-side: same deny-by-default Tauri capabilities, scoped FS, CSP with media via asset protocol.

## 7. Milestones (revised)

- **M0 (1 wk):** Userscript spike — intercept Bookmarks GraphQL, log parsed tweets; test the >1k pagination wall on a real account; test in-session media fetch. *Go/no-go data for §3.3 and media strategy.*
- **M1 (2 wk):** Core crate — schema, NDJSON/file importers, FTS5, CLI (`marginalia import bookmarks.ndjson && marginalia search "rust gui"`). Dogfood immediately.
- **M2 (2 wk):** Tauri shell — pairing flow, loopback receiver, live capture UX ("scroll your bookmarks, watch them land in the app" is the demo), list/detail, command palette.
- **M3 (1 wk):** Semantic search + RRF.
- **M4 (1 wk):** Assisted scroll with full mitigation suite + cooldown ledger; twitter-web-exporter/HAR importers.
- **M5:** Signed builds, updater, launch. The launch video writes itself: split screen, bookmarks page scrolling on the left, search results materializing in the app on the right.

## 8. Risks

| Risk | Mitigation |
|---|---|
| X renames/reshapes the `Bookmarks` GraphQL operation (their shapes drift — <sup>[[12](https://scrapfly.io/blog/posts/how-to-scrape-twitter)]</sup>guest tokens, doc_ids, rate limits rotate; doc_ids don't affect us since we never construct requests, but response shape changes do) | Raw JSON storage + tolerant parser + userscript auto-update channel; match operation by name pattern, not doc_id |
| ToS gray zone: even passive capture is "collection" under a strict reading of X's terms | Honest positioning: personal archival of your own data, local-only, mirrors long-standing tolerated tools; no cloud component for X to target; user holds the data |
| Pagination wall blocks deep backfill | Folder capture, optional unbookmark-drain, honest UI messaging |
| Assisted scroll still spooks detection for some account (new account, VPN, shared IP — <sup>[[17](https://voidmob.com/blog/how-to-scrape-twitter-x-2026)]</sup>network signals compound) | Conservative defaults, cooldown ledger, pre-flight warning for risk factors ("on a VPN? consider passive mode"), passive mode always available |
| Userscript manager friction | WebExtension packaging in v1.1; HAR import as the zero-install path |

---

The one thing I'd push back on from your framing: "Tampermonkey script **or** browser automation" — treat those as opposite ends of the risk spectrum, not alternatives. The userscript *is* the product's moat: it inherits the user's full legitimacy (fingerprint, session, IP, human pacing) for free, which is exactly the stuff automation frameworks spend enormous effort faking badly. Want me to draft the M0 userscript — the `fetch` hook + GraphQL matcher + NDJSON exporter — as actual code next?

<details>
<summary><strong>📚 Sources (18)</strong></summary>

1. [GitHub - prinsss/twitter-web-exporter: Export tweets, bookmarks, lists and much more from Twitter(X) web app. (推文/书签/收藏/列表导出工具)](https://github.com/prinsss/twitter-web-exporter)
2. [Twitter Web Exporter](https://greasyfork.org/en/scripts/492218-twitter-web-exporter)
3. [Releases · prinsss/twitter-web-exporter](https://github.com/prinsss/twitter-web-exporter/releases)
4. [Ole Lehmann (@itsolelehmann) on X](https://x.com/itsolelehmann/status/2061911202830401564)
5. [How to Export X (Twitter) Bookmarks to CSV Free (2026)](https://contextbolt.com/blog/export-twitter-bookmarks/)
6. [Client Challenge](https://addons.mozilla.org/en-US/firefox/addon/twitter-exporter/)
7. [GitHub - nornagon/twitter-bookmark-archiver: Download your Twitter bookmarks and associated media](https://github.com/nornagon/twitter-bookmark-archiver)
8. [Export Tweets From Any Account - Chrome Web Store](https://chromewebstore.google.com/detail/export-tweet-from-any-acc/opbkmlokpjccgjmffhpndbjahhkbnhon)
9. [Bookmark Exporter — XActions](https://xactions.app/scripts/bookmark-exporter)
10. [GitHub - vladkens/twscrape: Python library and CLI for X/Twitter scraping with multi-account rotation and built-in rate-limit handling.](https://github.com/vladkens/twscrape)
11. [X (Twitter) Scraper - Tweets, Threads & Engagement Rate · Apify](https://apify.com/techforce.global/twitter-scraper)
12. [How to Scrape Twitter (X.com) Data in 2026](https://scrapfly.io/blog/posts/how-to-scrape-twitter)
13. [Twitter Bookmark Limit: How Many Can You Save? (2026)](https://contextbolt.com/blog/twitter-bookmark-limit/)
14. [Saverything](https://saverything.com/en/blog/twitter-bookmarks-limit/)
15. [X (Twitter) Limits in 2026: The Complete Rate Limit & Feature Reference | tendX blog](https://www.tendx.app/blog/x-twitter-limits-2026)
16. [X & Twitter API Rate Limits: 429s, Headers and Every Cap](https://www.getxapi.com/twitter-api-rate-limits)
17. [How to Scrape Twitter (X) in 2026: API, Tools, and Grok Bot](https://voidmob.com/blog/how-to-scrape-twitter-x-2026)
18. [X (Twitter) API Alternative - Sorsa API](https://api.sorsa.io/blog/twitter-api-rate-limits-2026)

</details>

---

