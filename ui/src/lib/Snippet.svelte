<script lang="ts">
  /**
   * A search snippet with its matches highlighted.
   *
   * The snippet arrives from Rust's FTS5 `snippet()` as a single string with
   * matches wrapped in `U+0001` / `U+0002`. Those are control characters
   * precisely so that this component can split on them and build real DOM
   * nodes — rather than the alternative, which is to have Rust emit `<mark>`
   * tags and have the frontend `innerHTML` them. That alternative turns any
   * tweet containing markup into script execution.
   *
   * So: no `{@html}` anywhere in this app, and no HTML parsing near user data.
   */
  import { SNIPPET_CLOSE, SNIPPET_OPEN } from './snippet';

  interface Props {
    snippet: string;
  }

  let { snippet }: Props = $props();

  type Part = { text: string; match: boolean };

  const parts = $derived.by((): Part[] => {
    const out: Part[] = [];
    let rest = snippet;
    while (rest.length > 0) {
      const open = rest.indexOf(SNIPPET_OPEN);
      if (open < 0) {
        out.push({ text: rest, match: false });
        break;
      }
      if (open > 0) out.push({ text: rest.slice(0, open), match: false });

      const close = rest.indexOf(SNIPPET_CLOSE, open + 1);
      if (close < 0) {
        // Unterminated marker — render the remainder plainly rather than
        // swallowing it.
        out.push({ text: rest.slice(open + 1), match: false });
        break;
      }
      out.push({ text: rest.slice(open + 1, close), match: true });
      rest = rest.slice(close + 1);
    }
    return out;
  });
</script>

<span class="snippet">
  {#each parts as part, i (i)}{#if part.match}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}
</span>

<style>
  .snippet {
    display: block;
    padding: 10px 16px 0;
    font-size: 13px;
    line-height: 17px;
    color: var(--text-secondary);
  }

  mark {
    background: rgba(29, 155, 240, 0.18);
    color: var(--accent);
    border-radius: 2px;
    padding: 0 1px;
  }
</style>
