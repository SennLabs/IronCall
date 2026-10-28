import { createSignal } from "solid-js";

export type ThemePreference = "system" | "light" | "dark";

const STORAGE_KEY = "ironcall.theme";
const PREFERENCES: readonly ThemePreference[] = ["system", "light", "dark"];

function loadPreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored && (PREFERENCES as readonly string[]).includes(stored)) {
      return stored as ThemePreference;
    }
  } catch {
    // Storage unavailable: fall back to following the OS.
  }
  return "system";
}

/** Applies a theme preference to the document. "system" follows the OS via CSS media query. */
export function applyTheme(
  preference: ThemePreference,
  root: HTMLElement = document.documentElement,
) {
  if (preference === "system") {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = preference;
  }
}

const [theme, setThemeSignal] = createSignal<ThemePreference>(loadPreference());

export { theme };

export function setTheme(preference: ThemePreference) {
  setThemeSignal(preference);
  applyTheme(preference);
  try {
    localStorage.setItem(STORAGE_KEY, preference);
  } catch {
    // Not persisted; the preference still applies for this session.
  }
}

/** Cycles system → light → dark → system. */
export function cycleTheme() {
  const next = PREFERENCES[(PREFERENCES.indexOf(theme()) + 1) % PREFERENCES.length] ?? "system";
  setTheme(next);
}

export function initTheme() {
  applyTheme(theme());
}
