import { createSignal, For, onCleanup, Show } from "solid-js";
import { api, type VariableContext, type VisibleVar } from "../../ipc/api";
import { secretScopeOf, sourceLabel, valuePreview } from "../../model/variables";
import type { AppStore } from "../../stores/app";
import { openDialog } from "../../stores/dialogs";
import { attempt, notify } from "../../stores/notify";
import styles from "./vars.module.css";

const NONE = "";
const MANAGE = "__manage__";

/**
 * Toolbar environment picker (PLAN.md Phase 3) plus the quick-look popover listing every
 * variable the active tab can see.
 */
export function EnvironmentSwitcher(props: { app: AppStore }) {
  const vars = () => props.app.vars;
  const [open, setOpen] = createSignal(false);
  let root!: HTMLDivElement;

  const away = (e: Event) => {
    if (!root.contains(e.target as Node)) setOpen(false);
  };
  window.addEventListener("pointerdown", away, true);
  onCleanup(() => window.removeEventListener("pointerdown", away, true));

  const label = (name: string, missing: number) =>
    missing > 0 ? `${name} (${missing} missing)` : name;

  return (
    <div ref={root} class={styles.switcher}>
      <Show when={props.app.workspace.info()}>
        {/* `selected` on each option, not `value` on the select: when the environment
            list and the active ID change together, the select's value would be applied
            before the new option exists, and the browser would fall back to the first. */}
        <select
          aria-label="Environment"
          onChange={(e) => {
            const v = e.currentTarget.value;
            if (v === MANAGE) {
              e.currentTarget.value = vars().active() ?? NONE;
              openDialog({ kind: "environments" });
              return;
            }
            void vars().setActive(v === NONE ? null : v);
          }}
        >
          <option value={NONE} selected={vars().active() === null}>
            No environment
          </option>
          <For each={vars().environments()}>
            {(env) => (
              <option value={env.id} disabled={!!env.error} selected={env.id === vars().active()}>
                {label(env.name, env.missingSecrets)}
              </option>
            )}
          </For>
          <option value={MANAGE}>Manage environments…</option>
        </select>
      </Show>
      <button
        type="button"
        class={`ghost ${styles.eye}`}
        aria-label="Variables quick look"
        aria-expanded={open()}
        title="Variables visible to this request"
        onClick={() => setOpen(!open())}
      >
        {"{{ }}"}
      </button>
      <Show when={open()}>
        <div class={styles.popover} role="dialog" aria-label="Variables">
          <QuickLook context={props.app.tabs.active()?.vars.context()} app={props.app} />
        </div>
      </Show>
    </div>
  );
}

/**
 * Every visible variable: value (secrets masked, reveal per row), source, overrides.
 * Runtime variables (from `extract`) can be saved to the active environment or cleared.
 */
export function QuickLook(props: { context: VariableContext | undefined; app?: AppStore }) {
  const [revealed, setRevealed] = createSignal<Record<string, string>>({});
  const isRuntime = (v: VisibleVar) => v.source.scope === "runtime";
  const activeEnv = () => props.app?.vars.activeEnvironment();

  const saveRuntime = async (name: string) => {
    const env = activeEnv();
    if (!env || !props.app) return;
    const ok = await attempt(
      () => api.runtimeVarSave(name).then(() => true),
      `Couldn't save ${name} to ${env.name}`,
    );
    if (!ok) return;
    notify(`Saved ${name} to ${env.name}`, "info");
    void props.app.vars.refresh();
  };
  const clearRuntime = async (name: string | null) => {
    await attempt(() => api.runtimeVarsClear(name), "Couldn't clear runtime variables");
    props.app?.vars.bump();
  };

  const reveal = async (v: VisibleVar) => {
    if (revealed()[v.name] !== undefined) {
      setRevealed(({ [v.name]: _, ...rest }) => rest);
      return;
    }
    const scope = secretScopeOf(v.source);
    if (!scope) return;
    try {
      const value = await api.secretReveal(scope, v.name);
      setRevealed((m) => ({ ...m, [v.name]: value ?? "" }));
    } catch {
      // Locked or unavailable: leave masked.
    }
  };

  return (
    <Show
      when={props.context}
      fallback={<p class={styles.muted}>Open a request to see its variables.</p>}
    >
      {(ctx) => (
        <>
          <h3>Variables</h3>
          <Show
            when={ctx().variables.length > 0}
            fallback={<p class={styles.muted}>No variables are defined for this request.</p>}
          >
            <table class={styles.lookTable}>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Value</th>
                  <th>Source</th>
                  <th aria-label="Actions" />
                </tr>
              </thead>
              <tbody>
                <For each={ctx().variables}>
                  {(v) => (
                    <tr aria-label={v.name}>
                      <td class={styles.lookName}>{v.name}</td>
                      <td class={styles.lookValue}>
                        <Show
                          when={!v.missing}
                          fallback={<span class={styles.missingTag}>missing</span>}
                        >
                          {revealed()[v.name] ?? valuePreview(v)}
                        </Show>
                      </td>
                      <td>
                        <span class={styles.sourceBadge}>{sourceLabel(v.source)}</span>
                        <Show when={v.overrides.length > 0}>
                          <div class={styles.lookOverrides}>
                            overrides {v.overrides.map(sourceLabel).join(", ")}
                          </div>
                        </Show>
                      </td>
                      <td>
                        <Show when={v.secret && !v.missing && !isRuntime(v)}>
                          <button type="button" class="ghost" onClick={() => void reveal(v)}>
                            {revealed()[v.name] !== undefined ? "Hide" : "Reveal"}
                          </button>
                        </Show>
                        <Show when={isRuntime(v) && props.app}>
                          <Show when={activeEnv()}>
                            {(env) => (
                              <button
                                type="button"
                                class="ghost"
                                title={`Save to the ${env().name} environment`}
                                aria-label={`Save ${v.name} to ${env().name}`}
                                onClick={() => void saveRuntime(v.name)}
                              >
                                Save
                              </button>
                            )}
                          </Show>
                          <button
                            type="button"
                            class="ghost"
                            aria-label={`Clear ${v.name}`}
                            onClick={() => void clearRuntime(v.name)}
                          >
                            Clear
                          </button>
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
          <Show when={props.app && ctx().variables.some(isRuntime)}>
            <p class={styles.muted}>
              Runtime variables come from extractions and last until the app closes.{" "}
              <button type="button" class="ghost" onClick={() => void clearRuntime(null)}>
                Clear all
              </button>
            </p>
          </Show>
          <Show when={ctx().unresolved.length > 0}>
            <p class={styles.errorText}>Not defined: {ctx().unresolved.join(", ")}</p>
          </Show>
        </>
      )}
    </Show>
  );
}
