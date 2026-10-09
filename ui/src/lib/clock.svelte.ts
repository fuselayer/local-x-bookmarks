/**
 * A shared clock, so relative timestamps keep ticking.
 *
 * `formatShortTimestamp` renders `3h` from the tweet's timestamp and "now".
 * If "now" is read at paint time, the label is right once and then freezes:
 * Svelte has no reason to re-render a row that has not changed, so `3h` stays
 * `3h` indefinitely. Reading a rune here instead makes every subscriber
 * re-render when the minute turns over, which is the whole point of a relative
 * label.
 *
 * One interval for the whole app rather than one per row: a list of five
 * hundred rows is exactly the situation where a per-component timer becomes a
 * problem, and there is nothing per-row to compute anyway.
 *
 * Thirty seconds is chosen against the labels' own granularity — the finest
 * one is whole minutes, so a half-minute tick can never show a stale value for
 * longer than it takes the reader to notice.
 */

let now = $state(Date.now());
let started = false;

/** The current time. Read it during render to subscribe to the clock. */
export function clockNow(): number {
  if (!started) {
    started = true;
    setInterval(() => {
      now = Date.now();
    }, 30_000);
  }
  return now;
}
