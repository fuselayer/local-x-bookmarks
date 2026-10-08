<script lang="ts">
  import type { LibraryStats, PostView, SearchHit } from './lib/types';
  import Tweet from './lib/Tweet.svelte';
  import Icon from './lib/Icon.svelte';
  import Snippet from './lib/Snippet.svelte';
  import * as api from './lib/api';
  import {
    applyTheme,
    loadThemeChoice,
    resolveTheme,
    saveThemeChoice,
    THEME_CHOICES,
    type ThemeChoice,
  } from './lib/theme';
  import { formatCount } from './lib/format';

  let posts = $state<PostView[]>([]);
  let hits = $state<SearchHit[] | null>(null);
  let selectedId = $state<string | null>(null);
  let query = $state('');
  let libraryStats = $state<LibraryStats | null>(null);
  let themeChoice = $state<ThemeChoice>(loadThemeChoice());
  let error = $state<string | null>(null);
  let loading = $state(true);

  // Paste-to-import. The quickest route to "one tweet in the library" and it
  // needs no file dialog, no clipboard and no browser involvement.
  let showImport = $state(false);
  let importText = $state('');
  let importBusy = $state(false);
  let importNote = $state<string | null>(null);

  const list = $derived(hits ? hits.map((h) => h.post) : posts);
  const selected = $derived(list.find((p) => p.id === selectedId) ?? list[0] ?? null);
  const snippetFor = $derived((id: string) => hits?.find((h) => h.post.id === id)?.snippet ?? null);

  // ── theme ────────────────────────────────────────────────────────────────
  $effect(() => {
    applyTheme(themeChoice);
    saveThemeChoice(themeChoice);
  });

  // Follow the OS live while "system" is selected, rather than only on boot.
  $effect(() => {
    if (themeChoice !== 'system' || typeof matchMedia !== 'function') return;
    const mq = matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => applyTheme('system');
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  });

  // ── data ─────────────────────────────────────────────────────────────────
  async function refresh() {
    loading = true;
    error = null;
    try {
      if (api.isTauri()) {
        posts = await api.listBookmarks(500, 0);
        libraryStats = await api.stats();
      } else {
        error =
          'Not running inside the app shell. Start it with `pnpm tauri dev` — ' +
          'the frontend has no built-in mock data on purpose, so that the ' +
          'Rust model stays the single source of truth.';
      }
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void refresh();
  });

  // Debounce: search runs on every keystroke in Rust, but there is no reason
  // to cross the IPC boundary mid-word more than necessary.
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const q = query.trim();
    clearTimeout(searchTimer);
    if (!q) {
      hits = null;
      return;
    }
    searchTimer = setTimeout(async () => {
      try {
        hits = await api.search(q, 200);
      } catch (e) {
        error = String(e);
      }
    }, 60);
    return () => clearTimeout(searchTimer);
  });

  async function doImport() {
    if (!importText.trim()) return;
    importBusy = true;
    importNote = null;
    try {
      const result = await api.importJson(importText);
      importNote =
        result.new > 0
          ? `Imported ${result.new} new post${result.new === 1 ? '' : 's'}.`
          : result.seen > 0
            ? 'Already in your library.'
            : 'No tweets found in that payload.';
      if (result.new > 0) {
        importText = '';
        await refresh();
      }
    } catch (e) {
      importNote = `Could not import: ${e}`;
    } finally {
      importBusy = false;
    }
  }

  function openLink(url: string) {
    void api.openExternal(url).catch((e) => (error = String(e)));
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      if (showImport) showImport = false;
      else if (query) query = '';
    }
    // "/" focuses search, exactly as it does on X.
    if (event.key === '/' && document.activeElement?.tagName !== 'INPUT' &&
        document.activeElement?.tagName !== 'TEXTAREA') {
      event.preventDefault();
      document.getElementById('xdl-search')?.focus();
    }
    if (event.key === 'j' || event.key === 'k') {
      const idx = list.findIndex((p) => p.id === selected?.id);
      if (idx < 0) return;
      const next = event.key === 'j' ? Math.min(idx + 1, list.length - 1) : Math.max(idx - 1, 0);
      selectedId = list[next]?.id ?? null;
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="shell">
  <header class="topbar hairline">
    <div class="brand">
      <span class="mark">xdl</span>
      <span class="wordmark">xitter-dl</span>
    </div>

    <div class="searchwrap">
      <Icon name="search" size={16} />
      <input
        id="xdl-search"
        class="search"
        type="search"
        placeholder="Search your bookmarks"
        bind:value={query}
        autocomplete="off"
        spellcheck="false"
      />
      {#if query}<button class="clear" onclick={() => (query = '')} title="Clear">×</button>{/if}
    </div>

    <div class="tools">
      <select class="theme" bind:value={themeChoice} title="Theme">
        {#each THEME_CHOICES as t (t.value)}
          <option value={t.value}>{t.label}</option>
        {/each}
      </select>
      <button class="primary" onclick={() => (showImport = !showImport)}>Import</button>
    </div>
  </header>

  {#if showImport}
    <section class="import hairline">
      <p class="hint">
        Paste the JSON of a captured GraphQL response — a whole bookmarks page,
        a <code>TweetDetail</code>, or a single <code>tweet_results.result</code>.
        Nothing is sent anywhere; this is parsed and stored locally.
      </p>
      <textarea
        bind:value={importText}
        placeholder={'{"data":{"tweetResult":{"result":{ … }}}}'}
        spellcheck="false"
      ></textarea>
      <div class="import-actions">
        <button class="primary" onclick={doImport} disabled={importBusy || !importText.trim()}>
          {importBusy ? 'Importing…' : 'Import'}
        </button>
        <button onclick={() => { showImport = false; importNote = null; }}>Cancel</button>
        {#if importNote}<span class="note">{importNote}</span>{/if}
      </div>
    </section>
  {/if}

  {#if error}
    <div class="banner error">{error}</div>
  {/if}

  <main class="panes">
    <section class="listpane scroll">
      <div class="listhead hairline">
        {#if query}
          <span>{list.length} result{list.length === 1 ? '' : 's'} for “{query}”</span>
        {:else if libraryStats}
          <span>
            {formatCount(libraryStats.bookmarks)} bookmark{libraryStats.bookmarks === 1 ? '' : 's'}
            {#if libraryStats.removed > 0}
              · <span class="muted">{libraryStats.removed} no longer present</span>
            {/if}
          </span>
        {:else}
          <span>Library</span>
        {/if}
      </div>

      {#if loading}
        <p class="empty">Loading…</p>
      {:else if list.length === 0}
        <div class="empty">
          {#if query}
            <p>No matches for “{query}”.</p>
          {:else}
            <p>No bookmarks yet.</p>
            <p class="muted">
              Click <strong>Import</strong> and paste a captured payload to add
              one. The capture userscript is the next milestone.
            </p>
          {/if}
        </div>
      {:else}
        {#each list as post (post.id)}
          <!--
            A div, not a button. A row contains links and the action bar's own
            buttons, and interactive elements cannot nest inside a <button> —
            the parser flattens them and the links stop working.
          -->
          <div
            class="row"
            class:active={selected?.id === post.id}
            role="button"
            tabindex="0"
            onclick={() => (selectedId = post.id)}
            onkeydown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                selectedId = post.id;
              }
            }}
          >
            {#if snippetFor(post.id)}
              <Snippet snippet={snippetFor(post.id)!} />
            {/if}
            <Tweet post={post} variant="timeline" {openLink} />
          </div>
        {/each}
      {/if}
    </section>

    <section class="detailpane scroll">
      {#if selected}
        <Tweet post={selected} variant="detail" {openLink} />
      {:else}
        <div class="empty">
          <p class="muted">Select a bookmark to read it.</p>
        </div>
      {/if}
    </section>
  </main>
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg);
    color: var(--text-primary);
  }

  .topbar {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 8px 16px;
    flex-shrink: 0;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .mark {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: var(--accent);
    color: #fff;
    font-size: 12px;
    font-weight: 800;
    letter-spacing: 0.02em;
  }

  .wordmark {
    font-weight: 700;
    font-size: 15px;
  }

  .searchwrap {
    flex: 1;
    max-width: 420px;
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--bg-elevated);
    border: 1px solid transparent;
    border-radius: 9999px;
    padding: 0 12px;
    height: 36px;
    color: var(--text-secondary);
  }

  .searchwrap:focus-within {
    border-color: var(--accent);
    background: var(--bg);
    color: var(--accent);
  }

  .search {
    flex: 1;
    background: none;
    border: none;
    outline: none;
    color: var(--text-primary);
    font-size: 15px;
    height: 100%;
  }

  .search::placeholder {
    color: var(--text-secondary);
  }

  .clear {
    color: var(--text-secondary);
    font-size: 18px;
    line-height: 1;
  }

  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-left: auto;
  }

  .theme {
    background: var(--bg-elevated);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: 9999px;
    padding: 6px 10px;
    font-size: 13px;
  }

  .primary {
    background: var(--accent);
    color: #fff;
    font-weight: 700;
    font-size: 14px;
    border-radius: 9999px;
    padding: 7px 16px;
  }

  .primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .primary:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .import {
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    flex-shrink: 0;
  }

  .hint {
    margin: 0;
    font-size: 13px;
    color: var(--text-secondary);
  }

  code {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--bg-elevated);
    padding: 1px 4px;
    border-radius: 4px;
  }

  textarea {
    width: 100%;
    min-height: 110px;
    resize: vertical;
    background: var(--bg-elevated);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 10px 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 18px;
    outline: none;
  }

  textarea:focus {
    border-color: var(--accent);
  }

  .import-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .note {
    font-size: 13px;
    color: var(--text-secondary);
  }

  .banner {
    padding: 8px 16px;
    font-size: 13px;
    flex-shrink: 0;
  }

  .banner.error {
    background: rgba(244, 33, 46, 0.1);
    color: #f4212e;
  }

  .panes {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(320px, 1fr) minmax(400px, 1.15fr);
    min-height: 0;
  }

  .listpane {
    border-right: 1px solid var(--border);
    min-height: 0;
  }

  .detailpane {
    min-height: 0;
  }

  .listhead {
    position: sticky;
    top: 0;
    z-index: 1;
    background: color-mix(in srgb, var(--bg) 85%, transparent);
    backdrop-filter: blur(12px);
    padding: 10px 16px;
    font-size: 13px;
    color: var(--text-secondary);
  }

  .row {
    display: block;
    width: 100%;
    text-align: left;
    padding: 0;
    color: inherit;
    cursor: pointer;
  }

  .row.active {
    background: var(--bg-hover);
  }

  .row :global(.tweet) {
    border-bottom: 1px solid var(--border);
  }

  .empty {
    padding: 24px 16px;
    color: var(--text-primary);
    font-size: 15px;
  }

  .empty p {
    margin: 0 0 8px;
  }

  .muted {
    color: var(--text-secondary);
    font-size: 14px;
  }
</style>
