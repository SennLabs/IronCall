import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";
import type { GeneralSettings, HttpVersion, ThemeChoice } from "../../ipc/api";
import { copyDiagnostics, openLogs } from "../../stores/diagnostics";
import { closeDialog, openDialog } from "../../stores/dialogs";
import { general, loadSettings, saveSettings, ZOOM_STEPS } from "../../stores/settings";
import { Modal } from "../common/Modal";
import styles from "./settings.module.css";

export type SettingsSection = "appearance" | "requests" | "privacy" | "more" | "about";

const SECTIONS: { key: SettingsSection; label: string }[] = [
  { key: "appearance", label: "Appearance" },
  { key: "requests", label: "Requests" },
  { key: "privacy", label: "Privacy" },
  { key: "more", label: "Network & storage" },
  { key: "about", label: "About & diagnostics" },
];

const APP_VERSION = __APP_VERSION__;

const BUILT_IN_SENSITIVE = "Authorization, Proxy-Authorization, Cookie, Set-Cookie, X-Api-Key";

/** App settings (PLAN.md Phase 8). Proxy, CAs, cookies, history and secret storage keep their own dialogs, linked from here. */
export function SettingsDialog(props: { section?: SettingsSection }) {
  const [section, setSection] = createSignal<SettingsSection>(props.section ?? "appearance");
  const current = general();
  const [draft, setDraft] = createSignal<GeneralSettings | undefined>(
    current && structuredClone(current),
  );
  const [headersText, setHeadersText] = createSignal(current?.sensitiveHeaders.join("\n") ?? "");
  // Opened before settings loaded (right after start) or after a failed load: load now,
  // and fill the form when they arrive.
  if (!current) void loadSettings();
  createEffect(() => {
    const loaded = general();
    if (loaded && !draft()) {
      setDraft(structuredClone(loaded));
      setHeadersText(loaded.sensitiveHeaders.join("\n"));
    }
  });
  const [saving, setSaving] = createSignal(false);

  const patch = <K extends keyof GeneralSettings>(key: K, value: Partial<GeneralSettings[K]>) =>
    setDraft((d) => d && { ...d, [key]: { ...(d[key] as object), ...value } });

  const save = async () => {
    const d = draft();
    if (!d) return;
    setSaving(true);
    const saved = await saveSettings({
      ...d,
      sensitiveHeaders: headersText()
        .split(/[\n,]/)
        .map((h) => h.trim())
        .filter(Boolean),
    });
    setSaving(false);
    if (saved) closeDialog();
  };

  return (
    <Modal
      title="Settings"
      wide
      onClose={closeDialog}
      footer={
        <>
          <button type="button" onClick={closeDialog}>
            Cancel
          </button>
          <button
            type="button"
            class="primary"
            disabled={!draft() || saving()}
            onClick={() => void save()}
          >
            Save
          </button>
        </>
      }
    >
      <Show when={draft()} fallback={<p class={styles.hint}>Settings couldn't be loaded.</p>}>
        {(d) => (
          <div class={styles.layout}>
            <nav class={styles.nav} aria-label="Settings sections">
              <For each={SECTIONS}>
                {(s) => (
                  <button
                    type="button"
                    class="ghost"
                    aria-current={section() === s.key ? "page" : undefined}
                    onClick={() => setSection(s.key)}
                  >
                    {s.label}
                  </button>
                )}
              </For>
            </nav>
            <Switch>
              <Match when={section() === "appearance"}>
                <section class={styles.section} aria-label="Appearance">
                  <label class={styles.field}>
                    <span>Theme</span>
                    <select
                      aria-label="Theme"
                      onChange={(e) =>
                        patch("appearance", { theme: e.currentTarget.value as ThemeChoice })
                      }
                    >
                      <For each={["system", "light", "dark"] as const}>
                        {(t) => (
                          <option value={t} selected={d().appearance.theme === t}>
                            {t === "system"
                              ? "Follow the system"
                              : t === "light"
                                ? "Light"
                                : "Dark"}
                          </option>
                        )}
                      </For>
                    </select>
                  </label>
                  <label class={styles.field}>
                    <span>Font size (px)</span>
                    <input
                      class={styles.number}
                      type="number"
                      min="10"
                      max="24"
                      aria-label="Font size"
                      value={d().appearance.fontSize}
                      onChange={(e) =>
                        patch("appearance", { fontSize: Math.round(+e.currentTarget.value) })
                      }
                    />
                  </label>
                  <label class={styles.field}>
                    <span>Zoom</span>
                    <select
                      aria-label="Zoom"
                      onChange={(e) => patch("appearance", { zoom: +e.currentTarget.value })}
                    >
                      <For each={ZOOM_STEPS}>
                        {(z) => (
                          <option value={z} selected={d().appearance.zoom === z}>
                            {z}%
                          </option>
                        )}
                      </For>
                    </select>
                  </label>
                  <p class={styles.hint}>
                    <kbd>Ctrl</kbd>+<kbd>+</kbd> and <kbd>Ctrl</kbd>+<kbd>−</kbd> zoom,{" "}
                    <kbd>Ctrl</kbd>+<kbd>0</kbd> resets.
                  </p>
                </section>
              </Match>
              <Match when={section() === "requests"}>
                <section class={styles.section} aria-label="Requests">
                  <p class={styles.hint}>
                    Used by requests that don't set these in their own Settings tab.
                  </p>
                  <label class={styles.field}>
                    <span>Timeout (ms, 0 for none)</span>
                    <input
                      class={styles.number}
                      type="number"
                      min="0"
                      step="1000"
                      aria-label="Default timeout"
                      value={d().requests.timeoutMs}
                      onChange={(e) =>
                        patch("requests", {
                          timeoutMs: Math.max(0, Math.round(+e.currentTarget.value || 0)),
                        })
                      }
                    />
                  </label>
                  <label class={styles.inline}>
                    <input
                      type="checkbox"
                      checked={d().requests.followRedirects}
                      onChange={(e) =>
                        patch("requests", { followRedirects: e.currentTarget.checked })
                      }
                    />
                    Follow redirects
                  </label>
                  <label class={styles.field}>
                    <span>Maximum redirects</span>
                    <input
                      class={styles.number}
                      type="number"
                      min="0"
                      max="50"
                      aria-label="Maximum redirects"
                      disabled={!d().requests.followRedirects}
                      value={d().requests.maxRedirects}
                      onChange={(e) =>
                        patch("requests", {
                          maxRedirects: Math.max(0, Math.round(+e.currentTarget.value || 0)),
                        })
                      }
                    />
                  </label>
                  <label class={styles.field}>
                    <span>HTTP version</span>
                    <select
                      aria-label="HTTP version"
                      onChange={(e) =>
                        patch("requests", { httpVersion: e.currentTarget.value as HttpVersion })
                      }
                    >
                      <option value="auto" selected={d().requests.httpVersion === "auto"}>
                        Auto (HTTP/2 when the server offers it)
                      </option>
                      <option value="http1" selected={d().requests.httpVersion === "http1"}>
                        HTTP/1.1 only
                      </option>
                      <option value="http2" selected={d().requests.httpVersion === "http2"}>
                        HTTP/2 only
                      </option>
                    </select>
                  </label>
                  <label class={styles.inline}>
                    <input
                      type="checkbox"
                      checked={d().requests.freshConnection}
                      onChange={(e) =>
                        patch("requests", { freshConnection: e.currentTarget.checked })
                      }
                    />
                    Always use a new connection (measure cold timings)
                  </label>
                  <p class={styles.hint}>
                    TLS verification can only be turned off per request, so it's never off by
                    accident.
                  </p>
                </section>
              </Match>
              <Match when={section() === "privacy"}>
                <section class={styles.section} aria-label="Privacy">
                  <label class={styles.field}>
                    <span>Extra sensitive headers (one per line)</span>
                    <textarea
                      class={styles.headers}
                      aria-label="Extra sensitive headers"
                      placeholder="X-Session-Token"
                      value={headersText()}
                      onInput={(e) => setHeadersText(e.currentTarget.value)}
                    />
                  </label>
                  <p class={styles.hint}>
                    Always sensitive: {BUILT_IN_SENSITIVE}. Values of sensitive headers are masked
                    in history, logs, the timeline and masked cURL exports.
                  </p>
                  <div class={styles.links}>
                    <button type="button" onClick={() => openDialog({ kind: "history" })}>
                      History retention…
                    </button>
                    <button type="button" onClick={() => openDialog({ kind: "secrets" })}>
                      Secret storage…
                    </button>
                  </div>
                </section>
              </Match>
              <Match when={section() === "more"}>
                <section class={styles.section} aria-label="Network and storage">
                  <p class={styles.hint}>
                    These have their own dialogs. Unsaved changes here are discarded.
                  </p>
                  <div class={styles.links}>
                    <button
                      type="button"
                      onClick={() => openDialog({ kind: "variables", target: { kind: "global" } })}
                    >
                      Global variables…
                    </button>
                    <button type="button" onClick={() => openDialog({ kind: "network" })}>
                      Proxy & certificates…
                    </button>
                    <button type="button" onClick={() => openDialog({ kind: "cookies" })}>
                      Cookies…
                    </button>
                    <button type="button" onClick={() => openDialog({ kind: "history" })}>
                      History retention…
                    </button>
                    <button type="button" onClick={() => openDialog({ kind: "secrets" })}>
                      Secret storage…
                    </button>
                  </div>
                </section>
              </Match>
              <Match when={section() === "about"}>
                <section class={styles.section} aria-label="About and diagnostics">
                  <h3>IronCall {APP_VERSION}</h3>
                  <p class={styles.hint}>
                    When reporting a problem, include the diagnostic info: version, platform, a
                    settings summary and recent log lines. Secrets and your home folder path are
                    left out.
                  </p>
                  <div class={styles.links}>
                    <button type="button" onClick={() => void copyDiagnostics()}>
                      Copy diagnostic info
                    </button>
                    <button type="button" onClick={openLogs}>
                      Open logs folder
                    </button>
                  </div>
                </section>
              </Match>
            </Switch>
          </div>
        )}
      </Show>
    </Modal>
  );
}
