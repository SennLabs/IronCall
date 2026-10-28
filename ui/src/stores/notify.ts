/** Small transient messages ("Saved", errors from file operations). */

import { createSignal } from "solid-js";

export interface Notice {
  id: number;
  kind: "info" | "error";
  text: string;
}

const [notices, setNotices] = createSignal<Notice[]>([]);
let next = 1;

export { notices };

export function notify(
  text: string,
  kind: Notice["kind"] = "info",
  ms = kind === "error" ? 6000 : 2500,
) {
  const id = next++;
  setNotices((list) => [...list, { id, kind, text }]);
  setTimeout(() => dismiss(id), ms);
}

export function dismiss(id: number) {
  setNotices((list) => list.filter((n) => n.id !== id));
}

/** Runs an async action, reporting failures as error notices. */
export async function attempt<T>(
  action: () => Promise<T>,
  failure: string,
): Promise<T | undefined> {
  try {
    return await action();
  } catch (e) {
    notify(`${failure}: ${e instanceof Error ? e.message : String(e)}`, "error");
    return undefined;
  }
}
