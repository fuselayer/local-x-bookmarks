/**
 * The Rust model, mirrored.
 *
 * These interfaces are the frontend half of the contract defined in
 * `crates/core/src/model.rs`. Every field name here is the camelCase form
 * serde produces from the Rust struct's snake_case field plus its
 * `#[serde(rename_all = "camelCase")]` attribute.
 *
 * If you change a Rust model type, change it here in the same commit. There is
 * no codegen yet — `tauri-specta` will replace this file at M2 (see PRD §7.3),
 * at which point drift becomes impossible rather than merely discouraged.
 */

/** Which convention an entity's `[start, end]` indices use. */
export type EntityIndexUnits = 'utf16' | 'codePoints';

export interface UrlEntity {
  /** The `t.co` link exactly as it appears in the text. */
  url: string;
  /** What X displays instead: `github.com/foo/bar`. */
  display: string;
  /**
   * The real destination, straight from the payload.
   *
   * This is why we never unfurl `t.co` ourselves — resolving it would be a
   * request to an X-owned host (PRD §7.7).
   */
  expanded: string;
  start: number;
  end: number;
}

export interface MentionEntity {
  handle: string;
  name: string | null;
  start: number;
  end: number;
}

export interface TagEntity {
  text: string;
  start: number;
  end: number;
}

export interface Entities {
  /**
   * `null` when the payload carried no indices to infer from. The renderer
   * verifies every slice regardless, so an unknown value is safe.
   */
  units: EntityIndexUnits | null;
  urls: UrlEntity[];
  mentions: MentionEntity[];
  hashtags: TagEntity[];
  symbols: TagEntity[];
  /** `[start, end]` pairs for media, used to strip t.co links out of the text. */
  mediaIndices: [number, number][];
}

export interface Author {
  id: string;
  handle: string;
  name: string;
  avatarUrl: string | null;
  /** Legacy verified — the pre-2023 checkmark. */
  verified: boolean;
  /** X Premium blue check. */
  blueVerified: boolean;
  verifiedType: string | null;
}

export type MediaKind = 'photo' | 'video' | 'gif';

export interface Media {
  kind: MediaKind;
  /** Best still image in the payload. No `?name=orig` — see PRD §7.5. */
  url: string;
  altText: string | null;
  width: number | null;
  height: number | null;
  videoUrl: string | null;
  durationMs: number | null;
}

export interface Card {
  name: string | null;
  title: string | null;
  description: string | null;
  url: string | null;
  domain: string | null;
  imageUrl: string | null;
  imageWidth: number | null;
  imageHeight: number | null;
}

export interface Metrics {
  likes: number;
  reposts: number;
  replies: number;
  quotes: number;
  bookmarks: number;
  /** `null` when the author has hidden view counts. Not the same as zero. */
  views: number | null;
}

export interface Tweet {
  id: string;
  author: Author;
  /** The text to display: `note_tweet` when present, else `full_text`. */
  text: string;
  /** The truncated `full_text`, when this is a long post. */
  truncatedText: string | null;
  isLongForm: boolean;
  entities: Entities;
  /** Unix seconds. */
  createdAt: number | null;
  lang: string | null;
  conversationId: string | null;
  inReplyToId: string | null;
  inReplyToHandle: string | null;
  metrics: Metrics;
  media: Media[];
  card: Card | null;
  quoted: Tweet | null;
  parserVersion: number;
}

/** A `Tweet` plus local, user-specific state. Flattened by serde. */
export interface PostView extends Tweet {
  /**
   * When *we* observed the save.
   *
   * `null` for anything from a backfill scroll, because X exposes no bookmark
   * timestamp anywhere in the payload (PRD §6.3). Only continuous capture can
   * fill this in. Never sort by it, and always label it "saved (observed)".
   */
  bookmarkedAt: number | null;
  /** `entries[].sortIndex` — X's own bookmark ordering key. */
  sortIndex: string | null;
  firstSeenAt: number;
  lastSeenAt: number | null;
  /**
   * Set when a later capture covered this post's range and did not find it.
   *
   * The badge reads "no longer in your bookmarks" — never "deleted", because
   * absence cannot distinguish deletion from an unbookmark, from filtering, or
   * from a partial capture.
   */
  removedAt: number | null;
  source: string;
}

export interface SearchHit {
  post: PostView;
  /** Fused RRF score. Ordering only — never display it. */
  score: number;
  /** Match context with `[` `]` around hits, when a retriever produced one. */
  snippet: string | null;
  /** Which retrievers matched: `unicode61`, `trigram`, later `vector`. */
  matched: string[];
}

export interface LibraryStats {
  posts: number;
  bookmarks: number;
  authors: number;
  media: number;
  payloads: number;
  /** Posts flagged as no longer present in a later capture. */
  removed: number;
}

export interface ImportResult {
  files: number;
  seen: number;
  new: number;
  updated: number;
  payloads: number;
  problems: string[];
  library: LibraryStats;
}
