<script lang="ts">
  import { ICONS, type IconName } from './icons';

  interface Props {
    name: IconName;
    /** X renders action-bar icons at 18.75px. */
    size?: number;
    className?: string;
  }

  let { name, size = 18.75, className = '' }: Props = $props();
</script>

<!--
  Filled, never stroked.

  X's glyphs are solid shapes: the path data in `icons.ts` is the outline *of a
  filled icon*, and the interior detail (the hole in the reply bubble, the
  counters in the share tray) is drawn as additional subpaths that only read
  correctly under a fill rule. Stroking them instead traces every one of those
  contours as a 1.4px line, which turns each icon into a thin wireframe of
  itself — recognisably in the right place, and obviously wrong.

  There is no `filled` prop any more because there is nothing to toggle: every
  icon in the set is a filled glyph, including the magnifier and the tick.
-->
<svg
  viewBox="0 0 24 24"
  width={size}
  height={size}
  aria-hidden="true"
  focusable="false"
  class={className}
  fill="currentColor"
  stroke="none"
>
  <path d={ICONS[name]} />
</svg>

<style>
  svg {
    display: block;
    flex-shrink: 0;
  }
</style>
