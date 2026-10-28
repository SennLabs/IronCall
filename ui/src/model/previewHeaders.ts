/** Labels for where a previewed header comes from (PLAN.md Phases 1 and 4). */

import type { HeaderSource, PreviewHeader } from "../ipc/bindings";

export const HEADER_SOURCE_LABEL: Record<HeaderSource, string> = {
  user: "yours",
  derived: "auto",
  engine: "engine",
  auth: "auth",
  default: "default",
};

/** Tooltip explaining a previewed header row. */
export function headerSourceTitle(h: PreviewHeader): string {
  if (!h.effective) return "Overridden by your header";
  switch (h.source) {
    case "user":
      return "Your header";
    case "derived":
      return "Set from the body type";
    case "engine":
      return "Added when the request is sent";
    case "auth":
      return `From the auth${h.origin ? ` of ${h.origin}` : ""}`;
    case "default":
      return `Default header${h.origin ? ` from ${h.origin}` : ""}`;
  }
}
