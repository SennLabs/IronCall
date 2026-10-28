/** In-memory general settings for tests and browser-only dev (PLAN.md Phase 8). */

import type { Api } from "./api";
import type { GeneralSettings } from "./bindings";

export function defaultSettings(): GeneralSettings {
  return {
    requests: {
      timeoutMs: 30000,
      followRedirects: true,
      maxRedirects: 10,
      httpVersion: "auto",
      freshConnection: false,
    },
    appearance: { theme: "system", fontSize: 13, zoom: 100 },
    sensitiveHeaders: [],
  };
}

export function createMockSettings(seed?: Partial<GeneralSettings>) {
  let settings: GeneralSettings = { ...defaultSettings(), ...seed };
  const zooms: number[] = [];
  const errors: { message: string; stack: string | null }[] = [];
  let logsOpened = 0;
  const api: Pick<
    Api,
    | "settingsGet"
    | "settingsSet"
    | "setZoom"
    | "diagnosticInfo"
    | "openLogsFolder"
    | "logFrontendError"
  > = {
    async settingsGet() {
      return structuredClone(settings);
    },
    async settingsSet(next) {
      if (next.appearance.zoom < 50 || next.appearance.zoom > 200) {
        throw new Error("zoom must be 50–200%");
      }
      const headers: string[] = [];
      for (const h of next.sensitiveHeaders.map((x) => x.trim()).filter(Boolean)) {
        if (/\s/.test(h)) throw new Error(`\`${h}\` isn't a valid header name`);
        if (!headers.some((x) => x.toLowerCase() === h.toLowerCase())) headers.push(h);
      }
      settings = { ...structuredClone(next), sensitiveHeaders: headers };
      return structuredClone(settings);
    },
    async setZoom(scale) {
      zooms.push(scale);
    },
    async diagnosticInfo() {
      return `IronCall: ${__APP_VERSION__} (mock)\nOS: browser\n\nRecent log lines:\n(none)\n`;
    },
    async openLogsFolder() {
      logsOpened++;
    },
    async logFrontendError(message, stack) {
      errors.push({ message, stack });
    },
  };
  return {
    api,
    external: { zooms, errors, logsOpened: () => logsOpened, current: () => settings },
  };
}
