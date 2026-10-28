/**
 * One tab's view of its variables: a debounced `variables_for` of the current draft,
 * re-run when the draft or `version` (environment switch, saved variables) changes.
 * Secret values are never requested here; the quick-look reveals them one by one.
 */

import { type Accessor, createEffect, createSignal, on, onCleanup } from "solid-js";
import { api, type RequestDraft, type VariableContext } from "../ipc/api";

export const VARIABLES_DEBOUNCE_MS = 200;

export interface VariablesStore {
  context: Accessor<VariableContext | undefined>;
}

export function createVariables(
  draft: Accessor<RequestDraft>,
  version: Accessor<number> = () => 0,
  delay = VARIABLES_DEBOUNCE_MS,
): VariablesStore {
  const [context, setContext] = createSignal<VariableContext>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;

  createEffect(
    on([draft, version], ([d]) => {
      clearTimeout(timer);
      const mine = ++generation;
      timer = setTimeout(async () => {
        try {
          const ctx = await api.variablesFor(d, false);
          if (mine === generation) setContext(ctx);
        } catch {
          // Keep the previous context (e.g. no workspace yet); highlighting stays neutral.
        }
      }, delay);
    }),
  );
  onCleanup(() => clearTimeout(timer));
  return { context };
}
