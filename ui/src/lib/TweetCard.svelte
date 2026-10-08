<script lang="ts">
  /**
   * A link-preview card.
   *
   * Built entirely from the payload's own `card` binding values — no `t.co`
   * unfurling, no metadata fetch, no request to the destination. That is the
   * whole point: resolving a short link or scraping OpenGraph tags would mean
   * the app originating network requests, which is exactly the line the
   * design draws (PRD §7.7).
   *
   * X renders `summary_large_image` as a big image on top of a text block, and
   * everything else as a thumbnail beside it. Both are here.
   */
  import type { Card } from './types';

  interface Props {
    card: Card;
    onOpenLink?: (url: string) => void;
  }

  let { card, onOpenLink }: Props = $props();

  const isLarge = $derived(
    card.name === 'summary_large_image' || card.name === 'player' || card.name === 'photo',
  );

  function open(e: MouseEvent) {
    e.preventDefault();
    if (card.url) onOpenLink?.(card.url);
  }
</script>

<div class="card" class:large={isLarge} role="link" tabindex="0" onclick={open} onkeydown={(e) => e.key === 'Enter' && open(e as unknown as MouseEvent)}>
  {#if card.imageUrl}
    <div class="thumb" style={isLarge && card.imageWidth && card.imageHeight ? `aspect-ratio: ${card.imageWidth} / ${card.imageHeight};` : ''}>
      <span>preview image · not archived yet</span>
    </div>
  {/if}

  <div class="body">
    {#if card.domain}<span class="domain">{card.domain}</span>{/if}
    {#if card.title}<span class="title">{card.title}</span>{/if}
    {#if card.description}<span class="desc">{card.description}</span>{/if}
  </div>
</div>

<style>
  .card {
    display: flex;
    flex-direction: row;
    gap: 12px;
    margin-top: 12px;
    border: 1px solid var(--border);
    border-radius: 16px;
    overflow: hidden;
    cursor: pointer;
    background: var(--bg);
  }

  .card.large {
    flex-direction: column;
    gap: 0;
  }

  .card:hover {
    background: var(--bg-hover);
  }

  .thumb {
    flex-shrink: 0;
    width: 120px;
    height: 120px;
    aspect-ratio: 1 / 1;
    background: var(--bg-elevated);
    display: grid;
    place-items: center;
    text-align: center;
    padding: 8px;
  }

  .card.large .thumb {
    width: 100%;
    height: auto;
    border-bottom: 1px solid var(--border);
  }

  .thumb span {
    font-size: 11px;
    line-height: 14px;
    color: var(--text-secondary);
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 12px;
    min-width: 0;
  }

  .card.large .body {
    padding-top: 12px;
  }

  .domain,
  .desc {
    font-size: 13px;
    line-height: 16px;
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    font-size: 15px;
    line-height: 20px;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
