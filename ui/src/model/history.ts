/**
 * Snapshot undo/redo history for one tab's request draft.
 *
 * Edits within `coalesceMs` of each other merge into one step, so typing a word is a
 * single undo. CodeMirror keeps its own history inside the body editor.
 */

export interface History<T> {
  /** Records the state after an edit. */
  record(state: T): void;
  /** Returns the previous state, or `undefined` if there is none. */
  undo(): T | undefined;
  redo(): T | undefined;
  canUndo(): boolean;
  canRedo(): boolean;
  /** Starts over from `state` (e.g. after loading or taking theirs). */
  reset(state: T): void;
  /** Ends the current coalescing burst, so the next edit is a new step. */
  breakBurst(): void;
}

export interface HistoryOptions {
  coalesceMs?: number;
  limit?: number;
  now?: () => number;
}

export function createHistory<T>(initial: T, options: HistoryOptions = {}): History<T> {
  const coalesceMs = options.coalesceMs ?? 600;
  const limit = options.limit ?? 200;
  const now = options.now ?? (() => Date.now());

  // past[last] is the current state.
  let past: T[] = [initial];
  let future: T[] = [];
  let lastEdit = Number.NEGATIVE_INFINITY;

  return {
    record(state) {
      const t = now();
      if (t - lastEdit <= coalesceMs && past.length > 1) {
        past[past.length - 1] = state;
      } else {
        past.push(state);
        if (past.length > limit) past = past.slice(past.length - limit);
      }
      future = [];
      lastEdit = t;
    },
    undo() {
      if (past.length < 2) return undefined;
      future.push(past.pop() as T);
      lastEdit = Number.NEGATIVE_INFINITY;
      return past[past.length - 1];
    },
    redo() {
      const next = future.pop();
      if (next === undefined) return undefined;
      past.push(next);
      lastEdit = Number.NEGATIVE_INFINITY;
      return next;
    },
    canUndo: () => past.length > 1,
    canRedo: () => future.length > 0,
    reset(state) {
      past = [state];
      future = [];
      lastEdit = Number.NEGATIVE_INFINITY;
    },
    breakBurst() {
      lastEdit = Number.NEGATIVE_INFINITY;
    },
  };
}
