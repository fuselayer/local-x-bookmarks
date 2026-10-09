/**
 * Number and date formatting, matching how X displays them.
 *
 * X's compact-number rules, as observed on the web client:
 *
 * | Value      | X shows  |
 * |------------|----------|
 * | 0–9,999    | `1,234`  |
 * | 10,000+    | `12.3K`  |
 * | 1,000,000+ | `1.2M`   |
 * | 1e9+       | `1.2B`   |
 *
 * The threshold at 10,000 rather than 1,000 is the detail that gives it away:
 * X shows `9,999` but `10K`, not `9.9K`. Reproducing that matters, because
 * `9.9K` is what a generic formatter produces and it reads wrong immediately.
 */

/** Format a count the way X formats likes, reposts and views. */
export function formatCount(n: number | null | undefined): string {
  if (n === null || n === undefined) return '';
  if (!Number.isFinite(n)) return '';
  if (n < 0) return '0';

  if (n < 10_000) return n.toLocaleString('en-US');

  const units: [number, string][] = [
    [1_000_000_000, 'B'],
    [1_000_000, 'M'],
    [1_000, 'K'],
  ];

  for (const [size, suffix] of units) {
    if (n >= size) {
      const scaled = n / size;
      // One decimal place, but drop a trailing `.0` — X shows `12K`, not
      // `12.0K`.
      const text = scaled >= 100 ? Math.floor(scaled).toString() : scaled.toFixed(1);
      return `${text.replace(/\.0$/, '')}${suffix}`;
    }
  }

  return n.toString();
}

/**
 * The timestamp line X shows beside a handle.
 *
 * In a timeline row X uses a short form (`2h`, `Oct 10`); in the detail view it
 * uses the full `2:19 PM · Oct 10, 2018`. We mirror X in rendering times in the
 * viewer's local zone, which is what `toLocaleString` does by default.
 */
export function formatDetailTimestamp(unixSeconds: number | null): string {
  if (!unixSeconds) return '';
  const d = new Date(unixSeconds * 1000);
  const time = d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  const date = d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
  return `${time} · ${date}`;
}

/**
 * The short form used in timeline rows.
 *
 * `nowMs` is a parameter rather than a call to `Date.now()` on purpose. A
 * relative label only re-renders when something reactive changes, so reading
 * the clock inside the function produced a value that was correct when painted
 * and then silently froze — `3h` stayed `3h` until an unrelated update
 * happened to repaint the row. Taking `now` from a subscribed clock instead
 * makes "3h" become "4h" when it should.
 */
export function formatShortTimestamp(unixSeconds: number | null, nowMs = Date.now()): string {
  if (!unixSeconds) return '';
  const d = new Date(unixSeconds * 1000);
  const ageMs = nowMs - d.getTime();
  const minutes = ageMs / 60_000;

  if (minutes < 1) return 'now';
  if (minutes < 60) return `${Math.floor(minutes)}m`;

  const hours = minutes / 60;
  if (hours < 24) return `${Math.floor(hours)}h`;

  const days = hours / 24;
  if (days < 7) return `${Math.floor(days)}d`;

  // Past a week X shows the date, and adds the year only when it is not the
  // current one.
  const sameYear = d.getFullYear() === new Date(nowMs).getFullYear();
  return d.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
  });
}

/** `saved (observed) 3m ago` — the honest label for a continuous-capture time. */
export function formatObservedSave(unixSeconds: number | null): string | null {
  if (!unixSeconds) return null;
  return `saved (observed) ${formatShortTimestamp(unixSeconds)}`;
}
