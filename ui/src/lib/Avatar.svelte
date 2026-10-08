<script lang="ts">
  /**
   * A profile picture.
   *
   * ## Why this is a monogram rather than an image
   *
   * `avatarUrl` points at `pbs.twimg.com/profile_images/...`. Loading it would
   * make the webview open a connection to an X-owned host, which the CSP in
   * PRD §10 is locked to prevent and which §7.1 forbids outright. Caching
   * avatars into the local content-addressed store is M3 work.
   *
   * Until then this shows the display-name initial, which is what X itself
   * shows when an avatar fails to load — so the fallback is one X already
   * uses, not an invention. The tooltip says why.
   */
  interface Props {
    name: string;
    handle: string;
    /** X uses 40px in rows and 48px on the focused post. */
    size?: number;
  }

  let { name, handle, size = 40 }: Props = $props();

  // X picks the colour from the handle, so it is stable per account rather
  // than changing on every render.
  const PALETTE = [
    '#1d9bf0', '#7856ff', '#ff7a00', '#f91880',
    '#00ba7c', '#e0245e', '#536471', '#ffd400',
  ];

  const initial = $derived((name.trim()[0] ?? handle.trim()[0] ?? '?').toUpperCase());
  const colour = $derived(
    PALETTE[[...handle].reduce((acc, ch) => acc + ch.codePointAt(0)!, 0) % PALETTE.length],
  );
</script>

<div
  class="avatar"
  style="width: {size}px; height: {size}px; background: {colour}; font-size: {Math.round(size * 0.42)}px;"
  title="@{handle} — avatar not archived yet"
  aria-hidden="true"
>
  {initial}
</div>

<style>
  .avatar {
    border-radius: 9999px;
    display: grid;
    place-items: center;
    color: #fff;
    font-weight: 700;
    flex-shrink: 0;
    user-select: none;
  }
</style>
