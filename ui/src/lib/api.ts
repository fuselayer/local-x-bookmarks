/**
 * The bridge to Rust.
 *
 * Everything the UI knows comes through here; the frontend owns no state
 * machine and parses nothing (PRD §7.3, "all business logic lives in Rust").
 *
 * ## Why there is no mock fallback
 *
 * It is tempting to return fake data when `invoke` is unavailable so the UI
 * can be developed in a plain browser. We deliberately do not: a mock is a
 * second source of truth that drifts, and the thing most likely to drift is
 * exactly the shape this file exists to pin down. Run `pnpm tauri dev`
 * instead — it gives hot reload for the frontend anyway.
 */

import { invoke } from '@tauri-apps/api/core';
import type { ImportResult, LibraryStats, PostView, SearchHit } from './types';

/** True when running inside the Tauri shell rather than a plain browser. */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export async function listBookmarks(limit = 100, offset = 0): Promise<PostView[]> {
  return invoke<PostView[]>('list_bookmarks', { limit, offset });
}

export async function getPost(id: string): Promise<PostView | null> {
  return invoke<PostView | null>('get_post', { id });
}

export async function search(query: string, limit = 50): Promise<SearchHit[]> {
  return invoke<SearchHit[]>('search', { query, limit });
}

export async function stats(): Promise<LibraryStats> {
  return invoke<LibraryStats>('stats');
}

export async function importPath(path: string): Promise<ImportResult> {
  return invoke<ImportResult>('import_path', { path });
}

/**
 * Import a payload the user pasted.
 *
 * This is the fastest route to "one tweet in the library" and needs no
 * clipboard, no file dialog and no browser involvement.
 */
export async function importJson(text: string): Promise<ImportResult> {
  return invoke<ImportResult>('import_json', { text });
}

/**
 * Hand a URL to the operating system's default browser.
 *
 * This is not a loophole in the "the app never dials out" rule — it is the
 * rule being obeyed. The app itself opens no connection: it asks the OS to
 * open a URL, and the *user's own browser* makes the request, with the user's
 * own session and IP, at the moment they click. That is the same provenance
 * as the userscript side of the design.
 *
 * What we must never do is let the webview navigate to x.com directly, since
 * that request would come from inside this process. Which is why every link
 * in the UI routes through here rather than using a bare `href`.
 */
export async function openExternal(url: string): Promise<void> {
  await invoke('open_external', { url });
}

/** The canonical permalink for a post. Pure string building — no request. */
export function tweetUrl(handle: string, id: string): string {
  return `https://x.com/${handle}/status/${id}`;
}
