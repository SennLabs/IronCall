/**
 * Editing one scope's variables (an environment, the globals, the collection or a
 * folder) with secrets kept out of the file (PLAN.md §6):
 *
 * - Turning "secret" on keeps the typed value only in memory (`pendingSecret`); on
 *   save the file is written first (declaring `secret: true`, no value), then the value
 *   goes to the secret store with `secret_set`.
 * - Turning it off asks first, fetches the stored value into the row, and clears the
 *   stored secret after the file is saved.
 * - Renamed or deleted secrets have their stored values moved or cleared after saving.
 * - Saves send the base hash; a file changed on disk becomes a `conflict` to reload.
 */

import { batch, createSignal } from "solid-js";
import { api, type SecretScope, type VarsDraft } from "../ipc/api";
import { VarsFailure } from "../ipc/varsTypes";
import { WorkspaceFailure } from "../ipc/workspaceTypes";
import { rowsToVars, type VarRow, varsToRows } from "../model/variables";
import { promptSecret } from "./dialogs";
import { attempt, notify } from "./notify";
import type { VarsStore } from "./vars";

export interface VarsSource {
  load(): Promise<{ vars: VarsDraft; hash: string | null }>;
  /** Saves the variables; resolves the new hash. Throws on conflict. */
  save(vars: VarsDraft, baseHash: string | null): Promise<string>;
}

export type VarsEditor = ReturnType<typeof createVarsEditor>;

const isConflict = (e: unknown) =>
  (e instanceof VarsFailure || e instanceof WorkspaceFailure) && e.conflict;

export function createVarsEditor(source: VarsSource, scope: SecretScope, vars?: VarsStore) {
  const [rows, setRows] = createSignal<VarRow[]>([]);
  const [loadedRows, setLoadedRows] = createSignal<VarRow[]>([]);
  const [hash, setHash] = createSignal<string | null>(null);
  const [loaded, setLoaded] = createSignal(false);
  const [conflict, setConflict] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  /** Revealed secret values by row name (cleared on reload). */
  const [revealed, setRevealed] = createSignal<Record<string, string>>({});

  const guard = <T>(action: () => Promise<T>, failure: string) =>
    vars ? vars.withUnlock(action, failure) : attempt(action, failure);

  const changedSecrets = () => {
    vars?.bump();
    void vars?.refresh();
  };

  // `secretsMissing` only covers the active environment; summaries cover every one.
  const missingNames = () =>
    new Set([
      ...(vars?.missing() ?? [])
        .filter((m) => JSON.stringify(m.scope) === JSON.stringify(scope))
        .map((m) => m.name),
      ...(scope.kind === "environment"
        ? (vars?.environments().find((e) => e.id === scope.id)?.missingSecretNames ?? [])
        : []),
    ]);

  const reload = async () => {
    const data = await attempt(() => source.load(), "Couldn't load the variables");
    if (!data) return;
    const r = varsToRows(data.vars);
    batch(() => {
      setRows(r);
      setLoadedRows(r);
      setHash(data.hash);
      setConflict(false);
      setRevealed({});
      setLoaded(true);
    });
  };

  const patch = (i: number, p: Partial<VarRow>) =>
    setRows((list) => list.map((r, j) => (j === i ? { ...r, ...p } : r)));

  /** True if this row is a secret that exists in the store under its loaded name. */
  const storedSecret = (r: VarRow) => !!r.originalSecret && r.originalName === r.name.trim();

  const editor = {
    rows,
    setRows,
    hash,
    loaded,
    conflict,
    busy,
    revealed,
    reload,

    dirty: () =>
      JSON.stringify(rowsToVars(rows())) !== JSON.stringify(rowsToVars(loadedRows())) ||
      rows().some((r) => r.pendingSecret !== undefined),

    secretState(r: VarRow) {
      const shown = revealed()[r.name];
      return {
        shown,
        pending: r.pendingSecret !== undefined,
        missing: !storedSecret(r) || missingNames().has(r.name),
      };
    },

    async toggleSecret(i: number, on: boolean) {
      const r = rows()[i];
      if (!r || r.secret === on) return;
      if (on) {
        patch(i, {
          secret: true,
          pendingSecret: r.value === "" ? undefined : r.value,
          value: "",
        });
        return;
      }
      const ok = await api.confirm(
        `Store "${r.name}" in the file as plain text? Anyone with the file (or the git repository) can read it.`,
        { title: "Stop treating as secret", okLabel: "Store as plain text", kind: "warning" },
      );
      if (!ok) return;
      let value = r.pendingSecret ?? "";
      if (r.pendingSecret === undefined && storedSecret(r)) {
        value =
          (await guard(
            () => api.secretReveal(scope, r.originalName ?? r.name),
            "Couldn't read the secret",
          )) ?? "";
      }
      patch(i, { secret: false, value, pendingSecret: undefined });
    },

    async setSecret(i: number) {
      const r = rows()[i];
      if (!r) return;
      const value = await promptSecret(
        `Secret value · ${r.name || "variable"}`,
        "Value (stored in the secret store, never in the file)",
        "Set",
      );
      if (value === null) return;
      if (storedSecret(r) && !editor.dirty()) {
        // Already declared on disk: store it now.
        const ok = await guard(
          // Map the void result to `true`: `guard` resolves undefined on failure.
          () => api.secretSet(scope, r.name, value).then(() => true),
          "Couldn't store the secret",
        );
        if (ok) {
          setRevealed(({ [r.name]: _, ...rest }) => rest);
          changedSecrets();
        }
        return;
      }
      patch(i, { pendingSecret: value });
    },

    async clearSecret(i: number) {
      const r = rows()[i];
      if (!r) return;
      if (r.pendingSecret !== undefined) {
        patch(i, { pendingSecret: undefined });
        return;
      }
      if (!storedSecret(r)) return;
      const ok = await guard(
        () => api.secretClear(scope, r.name).then(() => true),
        "Couldn't clear the secret",
      );
      if (ok) changedSecrets();
    },

    async reveal(i: number) {
      const r = rows()[i];
      if (!r) return;
      if (revealed()[r.name] !== undefined) {
        setRevealed(({ [r.name]: _, ...rest }) => rest);
        return;
      }
      const value = await guard(() => api.secretReveal(scope, r.name), "Couldn't read the secret");
      if (value === undefined) return;
      setRevealed((m) => ({ ...m, [r.name]: value ?? "" }));
    },

    /** Saves the file, then applies secret changes. Resolves true on success. */
    async save(): Promise<boolean> {
      if (busy()) return false;
      setBusy(true);
      try {
        const current = rows();
        const fileVars = rowsToVars(current);
        let newHash: string;
        try {
          newHash = await source.save(fileVars, hash());
        } catch (e) {
          if (isConflict(e)) {
            setConflict(true);
            return false;
          }
          notify(
            `Couldn't save the variables: ${e instanceof Error ? e.message : String(e)}`,
            "error",
          );
          return false;
        }
        setHash(newHash);

        let secretsChanged = false;
        const keep = new Set(current.map((r) => r.originalName).filter(Boolean));
        for (const r of current) {
          const name = r.name.trim();
          if (!name) continue;
          if (r.secret && r.pendingSecret !== undefined) {
            await guard(
              () => api.secretSet(scope, name, r.pendingSecret ?? ""),
              `Couldn't store ${name}`,
            );
            secretsChanged = true;
          } else if (r.secret && r.originalSecret && r.originalName && r.originalName !== name) {
            // Renamed secret: move the stored value.
            const old = r.originalName;
            const value = await guard(() => api.secretReveal(scope, old), `Couldn't move ${old}`);
            if (value != null)
              await guard(() => api.secretSet(scope, name, value), `Couldn't move ${old}`);
            await guard(() => api.secretClear(scope, old), `Couldn't clear ${old}`);
            secretsChanged = true;
          }
          if (!r.secret && r.originalSecret && r.originalName) {
            const old = r.originalName;
            await guard(() => api.secretClear(scope, old), `Couldn't clear ${old}`);
            secretsChanged = true;
          }
        }
        // Deleted secret rows: clear their stored values.
        for (const r of loadedRows()) {
          if (r.originalSecret && r.originalName && !keep.has(r.originalName)) {
            const old = r.originalName;
            await guard(() => api.secretClear(scope, old), `Couldn't clear ${old}`);
            secretsChanged = true;
          }
        }
        await reload();
        if (secretsChanged) changedSecrets();
        else vars?.bump();
        return true;
      } finally {
        setBusy(false);
      }
    },
  };
  return editor;
}
