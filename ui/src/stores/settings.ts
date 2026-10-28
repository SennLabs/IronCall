/**
 * General settings (PLAN.md Phase 8): request defaults, appearance and extra sensitive
 * headers, stored by the backend in `settings.json`. The theme is also cached in
 * localStorage (`theme.ts`) so the window doesn't flash the wrong theme on start.
 */

import { createSignal } from "solid-js";
import { type AppearanceInfo, api, type GeneralSettings } from "../ipc/api";
import { cycleTheme, setTheme, theme } from "../theme/theme";
import { attempt } from "./notify";

/** Zoom steps for Ctrl +/−, in percent. */
export const ZOOM_STEPS = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200] as const;

const [general, setGeneral] = createSignal<GeneralSettings>();

export { general };

/** Applies appearance to the document and the webview. */
export function applyAppearance(a: AppearanceInfo, root: HTMLElement = document.documentElement) {
  if (theme() !== a.theme) setTheme(a.theme);
  root.style.setProperty("--font-size", `${a.fontSize}px`);
  void api.setZoom(a.zoom / 100).catch(() => {
    // Zoom unsupported here (e.g. tests): the rest still applies.
  });
}

/** Loads settings from the backend and applies the appearance. */
export async function loadSettings() {
  const s = await attempt(() => api.settingsGet(), "Couldn't load settings");
  if (!s) return;
  stored = s;
  setGeneral(s);
  applyAppearance(s.appearance);
}

/** Saves run one after another, so quick key presses land in order. */
let queue: Promise<unknown> = Promise.resolve();
/** The last settings the backend confirmed (to fall back to if a save fails). */
let stored: GeneralSettings | undefined;
/** Number of the newest save; older saves don't overwrite newer optimistic values. */
let latest = 0;

/** Saves settings; resolves with what was stored, or undefined if it failed (notified). */
export function saveSettings(next: GeneralSettings): Promise<GeneralSettings | undefined> {
  const mine = ++latest;
  const run = queue.then(async () => {
    const saved = await attempt(() => api.settingsSet(next), "Couldn't save settings");
    if (saved) stored = saved;
    // Only the newest save decides what's shown; a failure goes back to what's stored.
    const show = mine === latest ? (saved ?? stored) : undefined;
    if (show) {
      setGeneral(show);
      applyAppearance(show.appearance);
    }
    return saved;
  });
  queue = run;
  return run;
}

/** Applies at once (so the next key press builds on it), then saves. */
async function patchAppearance(patch: Partial<AppearanceInfo>) {
  const current = general();
  if (!current) return;
  const next = { ...current, appearance: { ...current.appearance, ...patch } };
  setGeneral(next);
  applyAppearance(next.appearance);
  await saveSettings(next);
}

/** Ctrl +/−: the next zoom step up or down. */
export function zoomBy(direction: 1 | -1) {
  const zoom = general()?.appearance.zoom ?? 100;
  const steps = direction > 0 ? ZOOM_STEPS : [...ZOOM_STEPS].reverse();
  const next = steps.find((z) => (direction > 0 ? z > zoom : z < zoom));
  if (next !== undefined) void patchAppearance({ zoom: next });
}

export function resetZoom() {
  void patchAppearance({ zoom: 100 });
}

/** The toolbar toggle: system → light → dark, saved. */
export function cycleThemeSetting() {
  cycleTheme();
  void patchAppearance({ theme: theme() });
}
