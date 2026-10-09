<script lang="ts">
  /**
   * One tweet, rendered as X renders it.
   *
   * Two variants, because X has two and they are not the same layout:
   *
   * - `timeline` — a row in a list. Text at 15px/20px.
   * - `detail` — the focused post on a permalink page. Text at **17px/24px**,
   *   which is the single most visible difference between X's list and its
   *   detail view, and the thing most clones get wrong.
   *
   * Quoting is rendered inline rather than recursively: X shows exactly one
   * level of a quoted post, and a nested <Tweet> would need a circular import
   * to express something X itself does not do.
   */
  import type { PostView } from './types';
  import Avatar from './Avatar.svelte';
  import Verified from './Verified.svelte';
  import TweetText from './TweetText.svelte';
  import TweetMedia from './TweetMedia.svelte';
  import TweetCard from './TweetCard.svelte';
  import ActionBar from './ActionBar.svelte';
  import { formatDetailTimestamp, formatShortTimestamp, formatObservedSave } from './format';
  import { clockNow } from './clock.svelte';

  interface Props {
    post: PostView;
    variant?: 'timeline' | 'detail';
    onOpenLink?: (url: string) => void;
  }

  let { post, variant = 'timeline', onOpenLink }: Props = $props();

  const t = $derived(post);
  const savedLabel = $derived(formatObservedSave(post.bookmarkedAt));
</script>

<article class="tweet {variant}" class:removed={post.removedAt !== null}>
  <div class="gutter">
    <Avatar name={t.author.name} handle={t.author.handle} size={40} />
  </div>

  <div class="content">
    {#if t.inReplyToHandle}
      <p class="replying-to">
        Replying to <a href="https://x.com/{t.inReplyToHandle}" onclick={(e) => { e.preventDefault(); onOpenLink?.(`https://x.com/${t.inReplyToHandle}`); }}>@{t.inReplyToHandle}</a>
      </p>
    {/if}

    <header class="byline">
      <a
        class="name"
        href="https://x.com/{t.author.handle}"
        onclick={(e) => { e.preventDefault(); onOpenLink?.(`https://x.com/${t.author.handle}`); }}
      >{t.author.name}</a>
      <Verified verified={t.author.verified} blue={t.author.blueVerified} />
      <span class="handle">@{t.author.handle}</span>
      {#if t.createdAt}
        <span class="handle dot-sep"></span>
        {#if variant === 'detail'}
          <time class="handle" datetime={new Date(t.createdAt * 1000).toISOString()}>
            {formatDetailTimestamp(t.createdAt)}
          </time>
        {:else}
          <time class="handle" datetime={new Date(t.createdAt * 1000).toISOString()}>
            {formatShortTimestamp(t.createdAt, clockNow())}
          </time>
        {/if}
      {/if}
    </header>

    <TweetText
      text={t.text}
      entities={t.entities}
      hasMedia={t.media.length > 0}
      size={variant === 'detail' ? 'detail' : 'timeline'}
      {onOpenLink}
    />

    {#if t.media.length > 0}
      <TweetMedia media={t.media} />
    {/if}

    {#if t.card}
      <TweetCard card={t.card} {onOpenLink} />
    {/if}

    {#if t.quoted}
      <!-- X's quoted-post card: bordered, rounded, smaller text. -->
      <div class="quoted">
        <header class="byline small">
          <Avatar name={t.quoted.author.name} handle={t.quoted.author.handle} size={20} />
          <span class="name">{t.quoted.author.name}</span>
          <Verified
            verified={t.quoted.author.verified}
            blue={t.quoted.author.blueVerified}
            size={15}
          />
          <span class="handle">@{t.quoted.author.handle}</span>
          {#if t.quoted.createdAt}
            <span class="handle dot-sep"></span>
            <span class="handle">{formatShortTimestamp(t.quoted.createdAt)}</span>
          {/if}
        </header>
        <TweetText
          text={t.quoted.text}
          entities={t.quoted.entities}
          hasMedia={t.quoted.media.length > 0}
          size="timeline"
          {onOpenLink}
        />
        {#if t.quoted.media.length > 0}
          <TweetMedia media={t.quoted.media} />
        {/if}
      </div>
    {/if}

    <!--
      Two honest labels, both about what we do NOT know.

      "no longer in your bookmarks" is deliberately not "deleted on X":
      absence from a later capture is equally consistent with an unbookmark, a
      filtered timeline, or a partial capture, and we cannot tell them apart
      (PRD §7.6).

      "saved (observed)" marks a real save time, and is present only for posts
      captured live. Backfilled posts have no save time at all, because X does
      not expose one anywhere in the payload (PRD §6.3).
    -->
    {#if post.removedAt !== null}
      <p class="note removed-note">no longer in your bookmarks</p>
    {:else if savedLabel}
      <p class="note">{savedLabel}</p>
    {/if}

    <!--
      The action bar appears in BOTH variants, because it does on X: a row in
      the bookmarks list carries the same Reply/Repost/Like/Views row as the
      focused post. Omitting it in the list would make the list look like a
      different product.
    -->
    <ActionBar metrics={t.metrics} handle={t.author.handle} id={t.id} {onOpenLink} />
  </div>
</article>

<style>
  .tweet {
    display: flex;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    /* X sets the cursor to pointer on a whole row in a list, because the row
       is a link. In the detail pane it is already open, so it is not. */
    cursor: default;
  }

  .tweet.timeline:hover {
    background: var(--bg-hover);
  }

  .tweet.removed {
    opacity: 0.6;
  }

  .gutter {
    flex-shrink: 0;
  }

  .content {
    flex: 1;
    min-width: 0;
  }

  .byline {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 15px;
    line-height: 20px;
    min-width: 0;
  }

  .byline.small {
    font-size: 14px;
    line-height: 18px;
  }

  .name {
    font-weight: 700;
    color: var(--text-primary);
    text-decoration: none;
    white-space: nowrap;
  }

  .name:hover {
    text-decoration: underline;
  }

  .handle {
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .replying-to {
    margin: 0 0 4px;
    font-size: 15px;
    line-height: 20px;
    color: var(--text-secondary);
  }

  .replying-to a {
    color: var(--accent);
  }

  .quoted {
    margin-top: 12px;
    border: 1px solid var(--border);
    border-radius: 16px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .quoted .byline {
    margin-bottom: 2px;
  }

  .note {
    margin: 8px 0 0;
    font-size: 13px;
    line-height: 16px;
    color: var(--text-secondary);
  }

  .removed-note {
    font-style: italic;
  }
</style>
