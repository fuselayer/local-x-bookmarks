<script lang="ts">
  /**
   * Connect a browser: show a pairing code, and report what arrives.
   *
   * ## Why this component is mounted rather than toggled
   *
   * It is rendered only while the panel is open, so mounting *is* "show the
   * code" and unmounting *is* "hide the code". That mapping is deliberate: the
   * backend invalidates the code when the panel closes, because a code that
   * stays valid while off screen is a code nobody is watching. A CSS `hidden`
   * toggle would quietly break that guarantee.
   */

  import { onMount } from 'svelte';
  import * as api from './api';

  let { onclose }: { onclose: () => void } = $props();

  let info = $state<api.PairingInfo | null>(null);
  let error = $state<string | null>(null);
  let secondsLeft = $state(0);
  let lastCapture = $state<api.CaptureReport | null>(null);
  let note = $state<string | null>(null);

  onMount(() => {
    let alive = true;
    let timer: ReturnType<typeof setInterval> | undefined;

    const apply = (next: api.PairingInfo) => {
      info = next;
      secondsLeft = next.secondsRemaining;
    };

    const rotate = async () => {
      try {
        const next = await api.pairingShow();
        if (alive) apply(next);
      } catch (e) {
        if (alive) error = String(e);
      }
    };

    void rotate();

    timer = setInterval(() => {
      if (!alive) return;
      secondsLeft -= 1;
      if (secondsLeft <= 0) void rotate();
    }, 1000);

    let off: (() => void) | undefined;
    void api
      .onBridgeEvent((event) => {
        if (!alive) return;
        if (event.type === 'paired') {
          note = event.label ? `Paired with ${event.label}.` : 'Paired.';
          void api.pairingStatus().then(apply).catch(() => {});
        } else if (event.type === 'captured') {
          lastCapture = event;
          note = null;
        } else if (event.type === 'rejected') {
          note = `A request was refused: ${event.reason}`;
        }
      })
      .then((f) => {
        if (alive) off = f;
        else f();
      })
      .catch((e) => {
        if (alive) error = String(e);
      });

    return () => {
      alive = false;
      clearInterval(timer);
      off?.();
      // Invalidate the code the moment the panel goes away.
      void api.pairingHide().catch(() => {});
    };
  });

  async function unpair() {
    try {
      info = await api.pairingRevoke();
      secondsLeft = info.secondsRemaining;
      lastCapture = null;
      note = 'Every paired browser must pair again before it can send anything.';
    } catch (e) {
      error = String(e);
    }
  }
</script>

<section class="pairing hairline">
  <header>
    <h3>Connect your browser</h3>
    <button class="x" onclick={onclose} title="Close">×</button>
  </header>

  {#if error}
    <p class="bad">{error}</p>
  {/if}

  {#if info && info.port === 0}
    <p class="bad">
      The capture receiver is not running, so no browser can connect. Something
      stopped it binding a loopback port at startup.
    </p>
  {:else if info}
    {#if info.paired}
      <p class="state ok">
        <span class="dot ok"></span>
        Paired{info.pairedLabel ? ` with ${info.pairedLabel}` : ''}.
      </p>
      <p class="hint">
        Captures from that browser are filed into this library automatically.
        Scroll <code>x.com/i/bookmarks</code> and they will appear on the left.
      </p>
      <button class="danger" onclick={unpair}>Unpair every browser</button>
    {:else}
      <p class="state">
        <span class="dot warn"></span>
        Waiting for a browser to pair.
      </p>
      <p class="hint">
        On <code>x.com/i/bookmarks</code>, click the pill in the bottom-right
        corner of the page and type this code. You only do this once per browser.
      </p>
      <div class="code">{info.formattedCode ?? '————-————'}</div>
      <p class="count">
        {#if secondsLeft > 0}
          New code in {secondsLeft}s. It rotates on its own — read the current one.
        {:else}
          Rotating…
        {/if}
      </p>
    {/if}

    <p class="addr">
      Receiver <code>{info.address}</code>
    </p>
  {:else}
    <p class="hint">Asking the backend…</p>
  {/if}

  {#if lastCapture}
    <p class="capture">
      Last batch: {lastCapture.seen} seen, {lastCapture.new} new,
      {lastCapture.updated} already held.
      {#if lastCapture.problems.length}
        <span class="bad">{lastCapture.problems.length} could not be read.</span>
      {/if}
    </p>
  {/if}

  {#if note}
    <p class="hint">{note}</p>
  {/if}
</section>

<style>
  .pairing {
    padding: 14px 18px 16px;
    background: var(--bg, transparent);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  h3 {
    margin: 0;
    font-size: 15px;
    font-weight: 700;
  }
  .x {
    margin-left: auto;
    border: 0;
    background: transparent;
    color: inherit;
    font-size: 20px;
    line-height: 1;
    cursor: pointer;
    opacity: 0.7;
  }
  .x:hover {
    opacity: 1;
  }
  p {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
  }
  .hint,
  .count,
  .addr,
  .capture {
    opacity: 0.65;
    font-size: 12px;
  }
  .state {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
    background: #71767b;
  }
  .dot.ok {
    background: #00ba7c;
  }
  .dot.warn {
    background: #ffd400;
  }
  .ok {
    color: #00ba7c;
  }
  .bad {
    color: #f4212e;
  }
  /* Large, monospace and widely tracked: this is read off the screen and typed
     into a different window, which is exactly the job letter-spacing is for. */
  .code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 30px;
    font-weight: 700;
    letter-spacing: 0.14em;
    padding: 10px 0 6px;
    user-select: all;
  }
  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 0.95em;
    opacity: 0.85;
  }
  button.danger {
    align-self: flex-start;
    border: 1px solid #f4212e;
    background: transparent;
    color: #f4212e;
    border-radius: 9999px;
    padding: 6px 14px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
  }
  button.danger:hover {
    background: rgba(244, 33, 46, 0.1);
  }
</style>
