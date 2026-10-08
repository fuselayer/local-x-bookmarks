<script lang="ts">
  /**
   * The row of icons under a tweet.
   *
   * ## Read-only, and honest about it
   *
   * X's bar is Reply / Repost / Like / Views / Bookmark / Share. Four of those
   * are write actions this app does not perform and will never perform — it is
   * read-only by design and holds no X credentials (PRD §2, §4.3).
   *
   * They are still rendered, because a tweet without them does not look like a
   * tweet, and looking like a tweet is the requirement. But they get no hover
   * colour and the default cursor: the coloured hover states are precisely how
   * X signals "this is clickable", so reproducing them here would be a lie
   * told in CSS.
   *
   * The last two slots are real, and they are the two actions that can
   * actually be honoured: copying a permalink, and opening the post in the
   * user's own browser. Reusing X's own share glyph for the first keeps the
   * bar's rhythm intact.
   */
  import Icon from './Icon.svelte';
  import { formatCount } from './format';
  import { tweetUrl } from './api';
  import type { Metrics } from './types';

  interface Props {
    metrics: Metrics;
    handle: string;
    id: string;
    onOpenLink?: (url: string) => void;
  }

  let { metrics, handle, id, onOpenLink }: Props = $props();

  const permalink = $derived(tweetUrl(handle, id));

  let copied = $state(false);

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(permalink);
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      // Clipboard access can be denied; fall back to handing it outward so the
      // user still gets the link rather than a silent failure.
      onOpenLink?.(permalink);
    }
  }

  // Explains, on hover, why the icon does not respond. Silence would read as
  // a bug.
  const staticHint = 'Read-only archive — open on X to interact';
</script>

<div class="actions">
  <div class="group static" title={staticHint}>
    <span class="pill"><Icon name="reply" /></span>
    <span class="count">{formatCount(metrics.replies)}</span>
  </div>

  <div class="group static" title={staticHint}>
    <span class="pill"><Icon name="repost" /></span>
    <span class="count">{formatCount(metrics.reposts)}</span>
  </div>

  <div class="group static" title={staticHint}>
    <span class="pill"><Icon name="like" /></span>
    <span class="count">{formatCount(metrics.likes)}</span>
  </div>

  <div class="group static" title={staticHint}>
    <span class="pill"><Icon name="views" /></span>
    <span class="count">{formatCount(metrics.views)}</span>
  </div>

  <div class="spacer"></div>

  <button class="group live" onclick={copyLink} title="Copy link to this post">
    <span class="pill"><Icon name="share" /></span>
    {#if copied}<span class="count confirmed">Copied</span>{/if}
  </button>

  <button
    class="group live"
    onclick={() => onOpenLink?.(permalink)}
    title="Open on X in your browser"
  >
    <span class="pill"><Icon name="externalLink" /></span>
  </button>
</div>

<style>
  .actions {
    display: flex;
    align-items: center;
    /* X spreads the four read-only actions and pins the two working ones
       right, with a max-width that keeps the bar from stretching absurdly on
       a wide window. */
    gap: 4px;
    max-width: 425px;
    margin-top: 12px;
    margin-left: -8px;
  }

  .spacer {
    flex: 1;
  }

  .group {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-secondary);
    font-size: 13px;
    line-height: 16px;
  }

  .pill {
    display: grid;
    place-items: center;
    width: 34.75px;
    height: 34.75px;
    border-radius: 9999px;
  }

  /* Read-only actions: correct at rest, inert on hover. */
  .static {
    cursor: default;
  }

  /* The two that work get X's real hover treatment. */
  .live {
    cursor: pointer;
  }

  .live:hover {
    color: var(--accent);
  }

  .live:hover .pill {
    background: rgba(29, 155, 240, 0.1);
  }

  .live:hover .count {
    text-decoration: underline;
  }

  .count {
    min-width: 1ch;
    font-variant-numeric: tabular-nums;
  }

  .confirmed {
    color: var(--accent);
  }
</style>
