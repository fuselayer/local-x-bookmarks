<script lang="ts">
  /**
   * Renders tweet text the way X does: plain text with links, mentions and
   * hashtags styled inline at positions X supplies.
   *
   * ── The index-units problem, and how this stays correct without guessing ──
   *
   * X's entity `indices` are `[start, end]`. Whether they count UTF-16 code
   * units (what JavaScript's `String.slice` uses) or Unicode code points (what
   * the public API documents) decides where every link lands in any tweet
   * containing an emoji. Get it wrong and each astral character shifts the
   * styling by one — links appear mid-word, hashtags lose a letter.
   *
   * Rather than pick one and hope, every slice is *verified*: the slice must
   * equal the entity's own literal text (`https://t.co/abc`, `@handle`,
   * `#tag`). If the UTF-16 reading fails, this retries with code-point
   * indices. If both fail, the entity renders as plain text — an unstyled link
   * is a cosmetic loss; a link styled one character off is corruption.
   */
  import type { Entities, UrlEntity, MentionEntity, TagEntity } from './types';

  interface Props {
    text: string;
    entities: Entities;
    /** Media present, used to drop the trailing media `t.co` X used to append. */
    hasMedia?: boolean;
    /** Timeline rows render at 15px, the detail view at 17px. */
    size?: 'timeline' | 'detail';
    /**
     * Called instead of navigating. Inside the app a plain `<a href>` would
     * send the *webview* to an X-owned host, which is exactly what the
     * architecture forbids — so clicks are always handed outward to the OS
     * browser (see `openExternal` in api.ts).
     */
    onOpenLink?: (url: string) => void;
  }

  let {
    text,
    entities,
    hasMedia = false,
    size = 'timeline',
    onOpenLink,
  }: Props = $props();

  type Segment =
    | { kind: 'text'; text: string }
    | { kind: 'link'; text: string; href: string; title: string };

  /**
   * Convert a code-point index into a UTF-16 offset.
   *
   * Walks the string counting code points rather than code units, so the
   * result is the index `String.slice` should use for the same logical
   * position.
   */
  function codePointToUtf16(s: string, cpIndex: number): number {
    let cp = 0;
    let u16 = 0;
    while (u16 < s.length && cp < cpIndex) {
      const code = s.codePointAt(u16);
      if (code === undefined) break;
      u16 += code > 0xffff ? 2 : 1;
      cp += 1;
    }
    return u16;
  }

  /**
   * Resolve an entity's `[start, end)` to offsets that actually bracket
   * `expected`, or `null` if no interpretation works.
   */
  function resolve(
    s: string,
    start: number,
    end: number,
    expected: string,
  ): [number, number] | null {
    if (start < 0 || end > s.length || start >= end) return null;

    // Interpretation 1: indices are already UTF-16 offsets.
    if (s.slice(start, end) === expected) return [start, end];

    // Interpretation 2: indices are code points.
    const a = codePointToUtf16(s, start);
    const b = codePointToUtf16(s, end);
    if (a < b && s.slice(a, b) === expected) return [a, b];

    return null;
  }

  /**
   * X historically appended a `t.co` link to the text of any tweet with
   * attached media, and does not render it. Payloads still carry it. Only the
   * ones pointing back at the tweet's own media are dropped, so a genuine
   * link that happens to sit at the end survives.
   */
  function isMediaSelfLink(url: UrlEntity): boolean {
    return /^https?:\/\/(twitter|x)\.com\/[^/]+\/status\/\d+\/(photo|video)\/\d+/.test(
      url.expanded,
    );
  }

  const segments = $derived.by((): Segment[] => {
    const found: { start: number; end: number; seg: Segment }[] = [];

    for (const u of entities.urls) {
      if (hasMedia && isMediaSelfLink(u)) continue;
      const span = resolve(text, u.start, u.end, u.url);
      if (!span) continue;
      found.push({
        start: span[0],
        end: span[1],
        seg: {
          kind: 'link',
          // X displays `display_url`, which is the readable truncated form.
          text: u.display || u.url,
          href: u.expanded,
          title: u.expanded,
        },
      });
    }

    for (const m of entities.mentions) {
      const span = resolve(text, m.start, m.end, `@${m.handle}`);
      if (!span) continue;
      found.push({
        start: span[0],
        end: span[1],
        seg: {
          kind: 'link',
          text: `@${m.handle}`,
          href: `https://x.com/${m.handle}`,
          title: m.name ? `${m.name} (@${m.handle})` : `@${m.handle}`,
        },
      });
    }

    for (const h of entities.hashtags) {
      const span = resolve(text, h.start, h.end, `#${h.text}`);
      if (!span) continue;
      found.push({
        start: span[0],
        end: span[1],
        seg: {
          kind: 'link',
          text: `#${h.text}`,
          href: `https://x.com/hashtag/${encodeURIComponent(h.text)}`,
          title: `#${h.text}`,
        },
      });
    }

    for (const s of entities.symbols) {
      const span = resolve(text, s.start, s.end, `$${s.text}`);
      if (!span) continue;
      found.push({
        start: span[0],
        end: span[1],
        seg: {
          kind: 'link',
          text: `$${s.text}`,
          href: `https://x.com/search?q=%24${encodeURIComponent(s.text)}`,
          title: `$${s.text}`,
        },
      });
    }

    // Sort by position, then drop anything overlapping an earlier entity.
    // X never emits overlapping entities; a malformed one must not produce
    // duplicated text.
    found.sort((a, b) => a.start - b.start || a.end - b.end);

    const out: Segment[] = [];
    let cursor = 0;
    for (const f of found) {
      if (f.start < cursor) continue;
      if (f.start > cursor) out.push({ kind: 'text', text: text.slice(cursor, f.start) });
      out.push(f.seg);
      cursor = f.end;
    }
    if (cursor < text.length) out.push({ kind: 'text', text: text.slice(cursor) });
    return out;
  });

  // Exposed so App can route clicks to the OS browser instead of navigating
  // the webview — see the note on `openExternal` in api.ts.
  function handleClick(event: MouseEvent, href: string) {
    // Inside the app, a plain <a href> would navigate the webview to an
    // X-owned host, which is precisely what the architecture forbids. So the
    // click is always intercepted and handed outward.
    event.preventDefault();
    onOpenLink?.(href);
  }
</script>

<p class="tweet-text {size}">
  {#each segments as seg, i (i)}
    {#if seg.kind === 'text'}<span>{seg.text}</span
      >{:else}<a
        href={seg.href}
        title={seg.title}
        onclick={(e) => handleClick(e, seg.href)}
        onkeydown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') handleClick(e as unknown as MouseEvent, seg.href);
        }}>{seg.text}</a
      >{/if}
  {/each}
</p>

<style>
  .tweet-text {
    margin: 0;
    /* X preserves newlines and wraps long URLs without breaking layout. */
    white-space: pre-wrap;
    overflow-wrap: break-word;
    color: var(--text-primary);
  }

  /* Timeline rows: 15px/20px. Detail view: 17px/24px. Both are X's values. */
  .tweet-text.timeline {
    font-size: 15px;
    line-height: 20px;
  }

  .tweet-text.detail {
    font-size: 17px;
    line-height: 24px;
  }

  a {
    color: var(--accent);
    text-decoration: none;
    cursor: pointer;
  }

  a:hover {
    text-decoration: underline;
  }
</style>
