/**
 * Search state for the paged body viewer (PLAN.md Phase 5, ADR 0003 follow-ups).
 *
 * Searches run in Rust in either direction. Typing starts a new search after a short
 * debounce; the backend cancels the previous scan of the same body, and any result
 * from an older search (cancelled or not) is ignored here by generation.
 */

import { createSignal } from "solid-js";
import { api, type SearchDirection } from "../ipc/api";
import type { BodyHit } from "../ipc/bindings";

export const SEARCH_DEBOUNCE_MS = 300;

export interface BodySearchOptions {
  bodyId: number;
  /** Body size in bytes (backward search from the end starts here). */
  byteLen: number;
  onHit: (hit: BodyHit | undefined) => void;
  debounceMs?: number;
}

export function createBodySearch(options: BodySearchOptions) {
  const [query, setQueryRaw] = createSignal("");
  const [ignoreCase, setIgnoreCaseRaw] = createSignal(true);
  const [hit, setHit] = createSignal<BodyHit>();
  const [message, setMessage] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let generation = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const show = (h: BodyHit | undefined) => {
    setHit(h);
    options.onHit(h);
  };

  /** Runs one search; returns the hit, or `undefined` if superseded or not found. */
  const run = async (
    from: number,
    direction: SearchDirection,
    wrapped = false,
  ): Promise<BodyHit | undefined> => {
    const q = query();
    if (!q) return undefined;
    const mine = ++generation;
    setBusy(true);
    setMessage("Searching…");
    try {
      const out = await api.searchBody(options.bodyId, q, from, ignoreCase(), direction);
      if (mine !== generation || out.cancelled) return undefined;
      if (out.hit) {
        show(out.hit);
        setMessage(`Line ${out.hit.row + 1}${wrapped ? " (wrapped)" : ""}`);
        return out.hit;
      }
      // Wrap around once, like editors do.
      const current = hit();
      if (!wrapped && current) {
        return run(direction === "forward" ? 0 : options.byteLen, direction, true);
      }
      show(undefined);
      setMessage("No matches");
      return undefined;
    } catch (e) {
      if (mine === generation) setMessage(`Search failed: ${e instanceof Error ? e.message : e}`);
      return undefined;
    } finally {
      if (mine === generation) setBusy(false);
    }
  };

  const restart = () => {
    clearTimeout(timer);
    // Invalidate any search in flight; the backend cancels it when the next one starts.
    generation++;
    setBusy(false);
    show(undefined);
    setMessage("");
    if (!query()) return;
    timer = setTimeout(() => void run(0, "forward"), options.debounceMs ?? SEARCH_DEBOUNCE_MS);
  };

  return {
    query,
    ignoreCase,
    hit,
    message,
    busy,
    setQuery(q: string) {
      setQueryRaw(q);
      restart();
    },
    setIgnoreCase(v: boolean) {
      setIgnoreCaseRaw(v);
      restart();
    },
    /** Next match after the current one (or from the start). */
    next() {
      clearTimeout(timer);
      const current = hit();
      return run(current ? current.offset + 1 : 0, "forward");
    },
    /** Previous match before the current one (or from the end). */
    previous() {
      clearTimeout(timer);
      const current = hit();
      return run(current ? current.offset : options.byteLen, "backward");
    },
    dispose() {
      clearTimeout(timer);
      generation++;
    },
  };
}

export type BodySearchStore = ReturnType<typeof createBodySearch>;
