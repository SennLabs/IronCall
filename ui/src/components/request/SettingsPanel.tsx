import { Show } from "solid-js";
import type { HttpVersion } from "../../ipc/bindings";
import type { RequestStore } from "../../stores/request";
import styles from "./panels.module.css";

/** Defaults shown when a setting isn't set on the request (PLAN.md §4.1). */
export const SETTING_DEFAULTS = {
  timeoutMs: 30000,
  followRedirects: true,
  maxRedirects: 10,
  verifyTls: true,
  httpVersion: "auto" as HttpVersion,
  freshConnection: false,
  encodeUrl: true,
  bypassProxy: false,
};

export function SettingsPanel(props: { request: RequestStore }) {
  const s = () => props.request.draft().settings ?? {};
  const get = <K extends keyof typeof SETTING_DEFAULTS>(k: K) =>
    (s()[k] ?? SETTING_DEFAULTS[k]) as (typeof SETTING_DEFAULTS)[K];
  const set = props.request.setSettings;

  return (
    <div class={styles.panel}>
      <div class={styles.form}>
        <label for="set-timeout">Timeout (ms)</label>
        <input
          id="set-timeout"
          type="number"
          min="0"
          step="1000"
          value={get("timeoutMs")}
          onChange={(e) => {
            const v = e.currentTarget.valueAsNumber;
            set({ timeoutMs: Number.isFinite(v) && v >= 0 ? Math.round(v) : null });
          }}
        />

        <label for="set-follow">Follow redirects</label>
        <input
          id="set-follow"
          type="checkbox"
          checked={get("followRedirects")}
          onChange={(e) => set({ followRedirects: e.currentTarget.checked })}
        />

        <label for="set-max">Max redirects</label>
        <input
          id="set-max"
          type="number"
          min="0"
          max="100"
          value={get("maxRedirects")}
          disabled={!get("followRedirects")}
          onChange={(e) => {
            const v = e.currentTarget.valueAsNumber;
            set({ maxRedirects: Number.isFinite(v) ? Math.max(0, Math.round(v)) : null });
          }}
        />

        <label for="set-verify">Verify TLS certificates</label>
        <span>
          <input
            id="set-verify"
            type="checkbox"
            checked={get("verifyTls")}
            onChange={(e) => set({ verifyTls: e.currentTarget.checked })}
          />{" "}
          <Show when={!get("verifyTls")}>
            <span class={`${styles.badge} ${styles.warning}`} role="alert">
              Verification off: connections are not secure
            </span>
          </Show>
        </span>

        <label for="set-http">HTTP version</label>
        <select
          id="set-http"
          value={get("httpVersion")}
          onChange={(e) => set({ httpVersion: e.currentTarget.value as HttpVersion })}
        >
          <option value="auto">Auto (negotiate)</option>
          <option value="http1">HTTP/1.1 only</option>
          <option value="http2">HTTP/2 only</option>
        </select>

        <label for="set-fresh">Fresh connection</label>
        <span>
          <input
            id="set-fresh"
            type="checkbox"
            checked={get("freshConnection")}
            onChange={(e) => set({ freshConnection: e.currentTarget.checked })}
          />{" "}
          <span class={styles.hint}>Don't reuse a pooled connection (measures DNS/TLS)</span>
        </span>

        <label for="set-encode">Encode URL</label>
        <span>
          <input
            id="set-encode"
            type="checkbox"
            checked={get("encodeUrl")}
            onChange={(e) => set({ encodeUrl: e.currentTarget.checked })}
          />{" "}
          <span class={styles.hint}>Off sends the URL exactly as typed</span>
        </span>

        <label for="set-proxy">Bypass proxy</label>
        <span>
          <input
            id="set-proxy"
            type="checkbox"
            checked={get("bypassProxy")}
            onChange={(e) => set({ bypassProxy: e.currentTarget.checked })}
          />{" "}
          <span class={styles.hint}>Connect directly even if a proxy is configured</span>
        </span>
      </div>
    </div>
  );
}
