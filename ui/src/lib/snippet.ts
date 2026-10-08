/**
 * The snippet delimiters, mirroring `SNIPPET_OPEN` / `SNIPPET_CLOSE` in
 * `crates/core/src/search.rs`.
 *
 * Control characters, not `[` and `]` and not HTML tags. See the comment on
 * `SNIPPET_OPEN` in the Rust source for why: they let the frontend build DOM
 * nodes instead of parsing HTML, which is the difference between rendering a
 * snippet and executing whatever a tweet happened to contain.
 */

export const SNIPPET_OPEN = '\u0001';
export const SNIPPET_CLOSE = '\u0002';
