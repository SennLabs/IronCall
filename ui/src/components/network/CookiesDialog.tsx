import { createResource, createSignal, For, Show } from "solid-js";
import { api, type CookieRow, type SameSite } from "../../ipc/api";
import { closeDialog } from "../../stores/dialogs";
import { cookiesVersion } from "../../stores/network";
import { attempt } from "../../stores/notify";
import { Modal } from "../common/Modal";
import styles from "./network.module.css";

const emptyCookie = (): CookieRow => ({
  domain: "",
  path: "/",
  name: "",
  value: "",
  expires: null,
  secure: false,
  httpOnly: false,
  sameSite: null,
  hostOnly: true,
});

const keyOf = (c: CookieRow) => `${c.domain}\u0000${c.path}\u0000${c.name}`;

/** `YYYY-MM-DDTHH:mm` for a datetime-local input, from RFC 3339 (UTC kept as local text). */
const toLocalInput = (rfc: string | null) => (rfc ? rfc.slice(0, 16) : "");
const fromLocalInput = (v: string) => (v ? `${v}:00Z` : null);

const flags = (c: CookieRow) =>
  [
    c.secure && "Secure",
    c.httpOnly && "HttpOnly",
    c.sameSite && `SameSite=${c.sameSite}`,
    c.hostOnly && "host-only",
  ]
    .filter(Boolean)
    .join(" · ");

/** The cookie jar of the open workspace (or the app jar for scratch requests). */
export function CookiesDialog() {
  const [cookies, { refetch }] = createResource(cookiesVersion, () => api.cookiesList());
  const [editing, setEditing] = createSignal<{ original: CookieRow | null; draft: CookieRow }>();
  const [filter, setFilter] = createSignal("");

  const shown = () => {
    const q = filter().trim().toLowerCase();
    const list = cookies() ?? [];
    return q ? list.filter((c) => `${c.domain} ${c.name}`.toLowerCase().includes(q)) : list;
  };

  const remove = async (c: CookieRow) => {
    await attempt(() => api.cookieDelete(c.domain, c.path, c.name), "Couldn't delete the cookie");
    await refetch();
  };

  const clearAll = async () => {
    const ok = await api.confirm("Delete every cookie in this jar?", {
      title: "Clear cookies",
      okLabel: "Clear all",
      kind: "warning",
    });
    if (!ok) return;
    await attempt(() => api.cookiesClear(), "Couldn't clear the cookies");
    await refetch();
  };

  const saveEdit = async () => {
    const e = editing();
    if (!e) return;
    const ok = await attempt(async () => {
      // Renaming a cookie (domain, path or name) replaces the old one.
      if (e.original && keyOf(e.original) !== keyOf(e.draft)) {
        await api.cookieDelete(e.original.domain, e.original.path, e.original.name);
      }
      await api.cookieSet(e.draft);
      return true;
    }, "Couldn't save the cookie");
    if (ok) {
      setEditing(undefined);
      await refetch();
    }
  };

  const patch = (p: Partial<CookieRow>) => {
    const e = editing();
    if (e) setEditing({ ...e, draft: { ...e.draft, ...p } });
  };

  return (
    <Modal
      title="Cookies"
      wide
      onClose={closeDialog}
      footer={
        <>
          <button
            type="button"
            onClick={() => void clearAll()}
            disabled={(cookies() ?? []).length === 0}
          >
            Clear all
          </button>
          <span class={styles.spacer} />
          <button type="button" onClick={closeDialog}>
            Close
          </button>
        </>
      }
    >
      <div class={styles.section}>
        <div class={styles.actions}>
          <input
            type="search"
            placeholder="Filter by domain or name"
            aria-label="Filter cookies"
            value={filter()}
            onInput={(e) => setFilter(e.currentTarget.value)}
          />
          <span class={styles.spacer} />
          <button
            type="button"
            onClick={() => setEditing({ original: null, draft: emptyCookie() })}
          >
            Add cookie
          </button>
        </div>
        <Show when={cookies.error}>
          <p class={styles.error}>Couldn't load cookies: {String(cookies.error)}</p>
        </Show>
        <Show
          when={shown().length > 0}
          fallback={<p class={styles.muted}>{cookies.loading ? "Loading…" : "No cookies."}</p>}
        >
          <table class={styles.table} aria-label="Cookies">
            <thead>
              <tr>
                <th>Domain</th>
                <th>Path</th>
                <th>Name</th>
                <th>Value</th>
                <th>Expires</th>
                <th>Flags</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              <For each={shown()}>
                {(c) => (
                  <tr aria-label={`${c.name} for ${c.domain}`}>
                    <td>{c.domain}</td>
                    <td>{c.path}</td>
                    <td>{c.name}</td>
                    <td class={`${styles.mono} ${styles.truncate}`} title={c.value}>
                      {c.value}
                    </td>
                    <td>{c.expires ? c.expires.replace("T", " ").slice(0, 16) : "Session"}</td>
                    <td class={styles.muted}>{flags(c)}</td>
                    <td>
                      <span class={styles.actions}>
                        <button
                          type="button"
                          class="ghost"
                          aria-label={`Edit ${c.name}`}
                          onClick={() => setEditing({ original: c, draft: { ...c } })}
                        >
                          Edit
                        </button>
                        <button
                          type="button"
                          class="ghost"
                          aria-label={`Delete ${c.name}`}
                          onClick={() => void remove(c)}
                        >
                          ✕
                        </button>
                      </span>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>

        <Show when={editing()}>
          {(e) => (
            <form
              class={styles.editForm}
              aria-label={e().original ? "Edit cookie" : "New cookie"}
              onSubmit={(ev) => {
                ev.preventDefault();
                void saveEdit();
              }}
            >
              <label for="ck-domain">Domain</label>
              <input
                id="ck-domain"
                value={e().draft.domain}
                onInput={(v) => patch({ domain: v.currentTarget.value })}
              />
              <label for="ck-path">Path</label>
              <input
                id="ck-path"
                value={e().draft.path}
                onInput={(v) => patch({ path: v.currentTarget.value })}
              />
              <label for="ck-name">Name</label>
              <input
                id="ck-name"
                value={e().draft.name}
                onInput={(v) => patch({ name: v.currentTarget.value })}
              />
              <label for="ck-value">Value</label>
              <input
                id="ck-value"
                value={e().draft.value}
                onInput={(v) => patch({ value: v.currentTarget.value })}
              />
              <label for="ck-expires">Expires (UTC)</label>
              <input
                id="ck-expires"
                type="datetime-local"
                value={toLocalInput(e().draft.expires)}
                onInput={(v) => patch({ expires: fromLocalInput(v.currentTarget.value) })}
              />
              <label for="ck-samesite">SameSite</label>
              <select
                id="ck-samesite"
                value={e().draft.sameSite ?? ""}
                onChange={(v) =>
                  patch({ sameSite: (v.currentTarget.value || null) as SameSite | null })
                }
              >
                <option value="">Not set</option>
                <option value="strict">Strict</option>
                <option value="lax">Lax</option>
                <option value="none">None</option>
              </select>
              <span>Flags</span>
              <span class={styles.actions}>
                <label>
                  <input
                    type="checkbox"
                    checked={e().draft.secure}
                    onChange={(v) => patch({ secure: v.currentTarget.checked })}
                  />{" "}
                  Secure
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={e().draft.httpOnly}
                    onChange={(v) => patch({ httpOnly: v.currentTarget.checked })}
                  />{" "}
                  HttpOnly
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={e().draft.hostOnly}
                    onChange={(v) => patch({ hostOnly: v.currentTarget.checked })}
                  />{" "}
                  Host only
                </label>
              </span>
              <span />
              <span class={styles.actions}>
                <button type="button" onClick={() => setEditing(undefined)}>
                  Cancel
                </button>
                <button
                  type="submit"
                  class="primary"
                  disabled={!e().draft.name.trim() || !e().draft.domain.trim()}
                >
                  Save cookie
                </button>
              </span>
            </form>
          )}
        </Show>
      </div>
    </Modal>
  );
}
