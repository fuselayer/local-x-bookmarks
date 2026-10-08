/**
 * Theme selection.
 *
 * X ships three themes and a "follow system" setting that picks between two of
 * them. We mirror that exactly, including the mapping: on a dark system X
 * selects **Lights out** (pure black), not Dim. Dim is a deliberate choice a
 * user has to make, never something they land on by accident.
 *
 * The choice persists in `localStorage` — a UI preference, not library data, so
 * it has no business in the SQLite file.
 */

export type ThemeChoice = 'system' | 'light' | 'dim' | 'lights-out';
export type ResolvedTheme = 'light' | 'dim' | 'lights-out';

export const THEME_CHOICES: { value: ThemeChoice; label: string }[] = [
  { value: 'system', label: 'Follow system' },
  { value: 'light', label: 'Default' },
  { value: 'dim', label: 'Dim' },
  { value: 'lights-out', label: 'Lights out' },
];

const STORAGE_KEY = 'xitter-dl.theme';

export function loadThemeChoice(): ThemeChoice {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored && THEME_CHOICES.some((t) => t.value === stored)) {
      return stored as ThemeChoice;
    }
  } catch {
    // localStorage can be unavailable under a strict CSP; fall through.
  }
  return 'system';
}

export function saveThemeChoice(choice: ThemeChoice): void {
  try {
    localStorage.setItem(STORAGE_KEY, choice);
  } catch {
    /* non-fatal */
  }
}

export function prefersDark(): boolean {
  return typeof matchMedia === 'function' && matchMedia('(prefers-color-scheme: dark)').matches;
}

export function resolveTheme(choice: ThemeChoice): ResolvedTheme {
  if (choice === 'system') return prefersDark() ? 'lights-out' : 'light';
  return choice;
}

/** Swap the theme class on `<html>`. */
export function applyTheme(choice: ThemeChoice): ResolvedTheme {
  const resolved = resolveTheme(choice);
  const root = document.documentElement;
  root.classList.remove('theme-light', 'theme-dim', 'theme-lights-out');
  root.classList.add(`theme-${resolved}`);
  // Tells the webview to render form controls and scrollbars to match.
  root.style.colorScheme = resolved === 'light' ? 'light' : 'dark';
  return resolved;
}
