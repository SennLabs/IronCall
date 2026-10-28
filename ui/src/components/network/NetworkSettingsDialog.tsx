import { createResource, createSignal, For, type JSX, Show } from "solid-js";
import { api, type CaCertInfo, type ProxyMode, type ProxySettings } from "../../ipc/api";
import { closeDialog, promptSecret } from "../../stores/dialogs";
import { attempt, notify } from "../../stores/notify";
import { Modal } from "../common/Modal";
import styles from "./network.module.css";

/** Proxy settings and trusted CA certificates (PLAN.md Phase 4). */
export function NetworkSettingsDialog(props: { hasWorkspace: boolean }) {
  const [settings, { mutate }] = createResource(() => api.networkSettingsGet());
  const [workspaceCas] = createResource(
    () => props.hasWorkspace,
    () => api.workspaceCaCertsGet(),
  );

  // Edits, falling back to the loaded values.
  const [proxyEdit, setProxyEdit] = createSignal<Partial<ProxySettings>>({});
  const [globalCas, setGlobalCas] = createSignal<CaCertInfo[]>();
  const [wsCas, setWsCas] = createSignal<CaCertInfo[]>();
  const [newHost, setNewHost] = createSignal("");

  const proxy = (): ProxySettings | undefined => {
    const s = settings();
    return s ? { ...s.proxy, ...proxyEdit() } : undefined;
  };
  const patch = (p: Partial<ProxySettings>) => setProxyEdit({ ...proxyEdit(), ...p });
  const globals = () => globalCas() ?? settings()?.caCerts ?? [];
  const workspace = () => wsCas() ?? workspaceCas() ?? [];

  const addHost = () => {
    const host = newHost().trim();
    const p = proxy();
    if (!host || !p || p.noProxy.includes(host)) return;
    patch({ noProxy: [...p.noProxy, host] });
    setNewHost("");
  };

  const pickCa = async (add: (info: CaCertInfo) => void) => {
    const info = await attempt(() => api.pickCaCert(), "Couldn't open the file picker");
    if (info) add(info);
  };

  const setPassword = async () => {
    const value = await promptSecret("Proxy password", "Password", "Store");
    if (value === null) return;
    const ok = await attempt(async () => {
      await api.proxyPasswordSet(value);
      return true;
    }, "Couldn't store the proxy password");
    const s = settings();
    if (ok && s) mutate({ ...s, proxy: { ...s.proxy, hasPassword: true } });
  };

  const clearPassword = async () => {
    const ok = await attempt(async () => {
      await api.proxyPasswordSet(null);
      return true;
    }, "Couldn't clear the proxy password");
    const s = settings();
    if (ok && s) mutate({ ...s, proxy: { ...s.proxy, hasPassword: false } });
  };

  const save = async () => {
    const p = proxy();
    if (!p) return;
    const saved = await attempt(async () => {
      const next = await api.networkSettingsSet(
        p,
        globals().map((c) => c.path),
      );
      if (props.hasWorkspace && wsCas()) {
        await api.workspaceCaCertsSet(workspace().map((c) => c.path));
      }
      return next;
    }, "Couldn't save the network settings");
    if (saved) {
      notify("Network settings saved");
      closeDialog();
    }
  };

  return (
    <Modal
      title="Network settings"
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
            disabled={!settings() || (proxy()?.mode === "manual" && !proxy()?.url?.trim())}
            onClick={() => void save()}
          >
            Save
          </button>
        </>
      }
    >
      <Show when={settings.error}>
        <p class={styles.error}>Couldn't load the settings: {String(settings.error)}</p>
      </Show>
      <Show when={proxy()} fallback={<p>Loading…</p>}>
        {(p) => (
          <div class={styles.section}>
            <h3 class={styles.heading}>Proxy</h3>
            <div class={styles.form}>
              <label for="proxy-mode">Mode</label>
              <select
                id="proxy-mode"
                value={p().mode}
                onChange={(e) => patch({ mode: e.currentTarget.value as ProxyMode })}
              >
                <option value="none">No proxy</option>
                <option value="environment">From environment (HTTP_PROXY / HTTPS_PROXY)</option>
                <option value="manual">Manual</option>
              </select>

              <Show when={p().mode === "environment"}>
                <span>Environment</span>
                <span class={styles.mono}>
                  {settings()?.environmentProxy ?? (
                    <span class={styles.muted}>No proxy variables set</span>
                  )}
                </span>
              </Show>

              <Show when={p().mode === "manual"}>
                <label for="proxy-url">Proxy URL</label>
                <input
                  id="proxy-url"
                  placeholder="http://proxy.example.com:3128"
                  value={p().url ?? ""}
                  aria-invalid={!p().url?.trim()}
                  onInput={(e) => patch({ url: e.currentTarget.value || null })}
                />
                <label for="proxy-user">Username</label>
                <input
                  id="proxy-user"
                  autocomplete="off"
                  value={p().username ?? ""}
                  onInput={(e) => patch({ username: e.currentTarget.value || null })}
                />
                <span>Password</span>
                <span class={styles.actions}>
                  <Show when={p().hasPassword} fallback={<span class={styles.muted}>Not set</span>}>
                    <span
                      class={styles.badge}
                      title="A proxy password is stored in the OS keychain"
                    >
                      stored
                    </span>
                  </Show>
                  <button type="button" onClick={() => void setPassword()}>
                    {p().hasPassword ? "Replace…" : "Set…"}
                  </button>
                  <Show when={p().hasPassword}>
                    <button type="button" class="ghost" onClick={() => void clearPassword()}>
                      Clear
                    </button>
                  </Show>
                </span>
              </Show>
            </div>
            <Show when={p().mode === "manual"}>
              <p class={styles.muted}>
                HTTPS requests go through a CONNECT tunnel. The password is kept in the OS keychain.
              </p>
            </Show>

            <Show when={p().mode !== "none"}>
              <h3 class={styles.heading}>Bypass the proxy for</h3>
              <ul class={styles.list} aria-label="No-proxy hosts">
                <For each={p().noProxy}>
                  {(host) => (
                    <li class={styles.listItem}>
                      <span class={`${styles.grow} ${styles.mono}`}>{host}</span>
                      <button
                        type="button"
                        class="ghost"
                        aria-label={`Remove ${host}`}
                        onClick={() => patch({ noProxy: p().noProxy.filter((h) => h !== host) })}
                      >
                        ✕
                      </button>
                    </li>
                  )}
                </For>
              </ul>
              <form
                class={styles.actions}
                onSubmit={(e) => {
                  e.preventDefault();
                  addHost();
                }}
              >
                <input
                  placeholder="example.com, .corp, 10.0.0.0/8, localhost"
                  aria-label="New no-proxy host"
                  value={newHost()}
                  onInput={(e) => setNewHost(e.currentTarget.value)}
                />
                <button type="submit" disabled={!newHost().trim()}>
                  Add
                </button>
              </form>
              <Show when={p().mode === "environment"}>
                <p class={styles.muted}>Added to the environment's NO_PROXY.</p>
              </Show>
            </Show>

            <h3 class={styles.heading}>Trusted CA certificates (all workspaces)</h3>
            <CaList
              label="Global CA certificates"
              certs={globals()}
              onRemove={(path) => setGlobalCas(globals().filter((c) => c.path !== path))}
              onAdd={() =>
                void pickCa((info) =>
                  setGlobalCas([...globals().filter((c) => c.path !== info.path), info]),
                )
              }
            />

            <Show when={props.hasWorkspace}>
              <h3 class={styles.heading}>Trusted CA certificates (this workspace)</h3>
              <Show when={workspaceCas.error}>
                <p class={styles.error}>{String(workspaceCas.error)}</p>
              </Show>
              <CaList
                label="Workspace CA certificates"
                certs={workspace()}
                onRemove={(path) => setWsCas(workspace().filter((c) => c.path !== path))}
                onAdd={() =>
                  void pickCa((info) =>
                    setWsCas([...workspace().filter((c) => c.path !== info.path), info]),
                  )
                }
                note={
                  <>
                    Kept in <code>.ironcall/local.yaml</code> (paths are specific to this machine).
                  </>
                }
              />
            </Show>
            <p class={styles.muted}>
              Certificates are trusted in addition to the operating system's store.
            </p>
          </div>
        )}
      </Show>
    </Modal>
  );
}

function CaList(props: {
  label: string;
  certs: readonly CaCertInfo[];
  onAdd: () => void;
  onRemove: (path: string) => void;
  note?: JSX.Element;
}) {
  return (
    <>
      <ul class={styles.list} aria-label={props.label}>
        <For each={props.certs} fallback={<li class={styles.muted}>None</li>}>
          {(c) => (
            <li class={styles.listItem}>
              <span class={styles.grow}>
                <strong>{c.subject ?? c.path.split(/[\\/]/).pop()}</strong>
                <br />
                <span class={`${styles.mono} ${styles.muted}`}>{c.path}</span>
                <Show when={c.error}>
                  <br />
                  <span class={styles.error}>{c.error}</span>
                </Show>
                <Show when={!c.error}>
                  <br />
                  <span class={styles.muted}>
                    {c.count} certificate{c.count === 1 ? "" : "s"}
                    {c.notAfter ? ` · expires ${c.notAfter.slice(0, 10)}` : ""}
                  </span>
                </Show>
              </span>
              <button
                type="button"
                class="ghost"
                aria-label={`Remove ${c.subject ?? c.path}`}
                onClick={() => props.onRemove(c.path)}
              >
                ✕
              </button>
            </li>
          )}
        </For>
      </ul>
      <span class={styles.actions}>
        <button type="button" onClick={props.onAdd}>
          Add certificate…
        </button>
        <Show when={props.note}>
          <span class={styles.muted}>{props.note}</span>
        </Show>
      </span>
    </>
  );
}
