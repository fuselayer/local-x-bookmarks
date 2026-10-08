<script lang="ts">
  import Icon from './Icon.svelte';

  interface Props {
    /** Legacy verified — the pre-2023 checkmark. */
    verified: boolean;
    /** X Premium blue check. */
    blue: boolean;
    size?: number;
  }

  let { verified, blue, size = 18.75 }: Props = $props();

  const title = $derived(
    blue ? 'Verified account' : verified ? 'Verified account (legacy)' : '',
  );
</script>

{#if verified || blue}
  <span class="badge" {title} aria-label={title}>
    <Icon name="verified" {size} filled />
  </span>
{/if}

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    color: var(--accent);
    /* X sets the badge tight against the display name with a 2px gap. */
    margin-left: 2px;
    vertical-align: middle;
  }
</style>
