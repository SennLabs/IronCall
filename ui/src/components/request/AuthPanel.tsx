import { createResource, Show } from "solid-js";
import { api } from "../../ipc/api";
import { describeAuth, describeOrigin } from "../../model/auth";
import { openDialog } from "../../stores/dialogs";
import type { RequestStore } from "../../stores/request";
import { AuthEditor } from "../network/AuthEditor";
import netStyles from "../network/network.module.css";
import styles from "./panels.module.css";

/** The request's Auth tab: its own auth, or the inherited one (PLAN.md Phase 4). */
export function AuthPanel(props: { request: RequestStore; readOnly?: boolean }) {
  const d = () => props.request.draft();
  const inherits = () => (d().auth?.type ?? "inherit") === "inherit";
  // Refetch when the request's own auth changes; parents are re-read each time.
  const [effective] = createResource(
    () => (inherits() ? JSON.stringify([d().id, d().auth ?? null]) : null),
    () => api.authEffective(d()),
  );

  const inheritedCard = () => (
    <div class={netStyles.card} role="note" aria-label="Inherited auth">
      <Show when={effective.error}>
        <span class={netStyles.error}>
          Couldn't work out the inherited auth: {String(effective.error)}
        </span>
      </Show>
      <Show when={effective()} fallback={<span class={netStyles.muted}>Checking parents…</span>}>
        {(e) => (
          <>
            <span>
              Inherited from <strong>{describeOrigin(e().origin)}</strong>
              <Show when={e().origin.kind !== "none"}>
                : <span class={netStyles.mono}>{describeAuth(e().auth)}</span>
              </Show>
            </span>
            <Show
              when={(() => {
                const o = e().origin;
                return o.kind === "folder" || o.kind === "collection" ? o : undefined;
              })()}
            >
              {(o) => (
                <span>
                  <button
                    type="button"
                    class="ghost"
                    onClick={() => {
                      const origin = o();
                      openDialog({
                        kind: "containerSettings",
                        target:
                          origin.kind === "folder"
                            ? { kind: "folder", id: origin.id, name: origin.name }
                            : { kind: "collection" },
                      });
                    }}
                  >
                    {o().kind === "folder" ? "Open folder settings" : "Open collection settings"}
                  </button>
                </span>
              )}
            </Show>
          </>
        )}
      </Show>
    </div>
  );

  return (
    <div class={styles.panel}>
      <AuthEditor
        auth={d().auth}
        readOnly={props.readOnly}
        onChange={(auth) => props.request.setAuth(auth)}
        inherited={inheritedCard()}
      />
    </div>
  );
}
