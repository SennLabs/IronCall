/**
 * Debounced `preview_request`, for the auto-header rows and the resolved URL. Re-runs
 * when `version` changes (environment switch, saved variables). Secrets stay masked.
 */

import { type Accessor, createEffect, createSignal, on, onCleanup } from "solid-js";
import { api, type RequestDraft, SendFailure, sendError } from "../ipc/api";
import type { RequestPreview, SendError } from "../ipc/bindings";

export const PREVIEW_DEBOUNCE_MS = 250;

export function createPreview(
  draft: Accessor<RequestDraft>,
  delay = PREVIEW_DEBOUNCE_MS,
  version: Accessor<number> = () => 0,
) {
  const [preview, setPreview] = createSignal<RequestPreview>();
  const [error, setError] = createSignal<SendError>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;

  createEffect(
    on([draft, version], ([d]) => {
      clearTimeout(timer);
      const mine = ++generation;
      timer = setTimeout(async () => {
        try {
          const p = await api.previewRequest(d, false);
          if (mine !== generation) return;
          setPreview(p);
          setError(undefined);
        } catch (e) {
          if (mine !== generation) return;
          setError(e instanceof SendFailure ? e.error : sendError("internal", String(e)));
        }
      }, delay);
    }),
  );
  onCleanup(() => clearTimeout(timer));

  return { preview, error };
}
