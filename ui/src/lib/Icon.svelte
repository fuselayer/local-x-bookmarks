<script lang="ts">
  import { ICONS, type IconName } from './icons';

  interface Props {
    name: IconName;
    /** X renders action-bar icons at 18.75px. */
    size?: number;
    /** Verified is the only filled glyph in the set. */
    filled?: boolean;
    className?: string;
  }

  let { name, size = 18.75, filled = false, className = '' }: Props = $props();
</script>

<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  aria-hidden="true"
  focusable="false"
  class={className}
  fill={filled ? 'currentColor' : 'none'}
  stroke={filled ? 'none' : 'currentColor'}
  stroke-width={filled ? 0 : 1.4}
  stroke-linejoin="round"
>
  <path d={ICONS[name]} />
</svg>

<style>
  svg {
    display: block;
    flex-shrink: 0;
  }
</style>
