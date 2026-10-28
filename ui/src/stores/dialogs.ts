/**
 * In-app modal dialogs that native dialogs can't do (text prompts, folder pickers inside
 * the workspace, compare views, description and variable editors).
 *
 * One main dialog at a time, plus one prompt that stacks above it (e.g. typing a secret
 * value from inside the environment manager).
 */

import { createSignal } from "solid-js";
import type { ItemId } from "../ipc/api";

export interface PromptRequest {
  title: string;
  label: string;
  initial: string;
  okLabel: string;
  /** Masked input, for secret values and passphrases. */
  password?: boolean;
  /** Ask twice and require both to match (new passphrases). */
  confirm?: boolean;
  resolve: (v: string | null) => void;
}

export type VariablesTarget =
  | { kind: "global" }
  | { kind: "collection" }
  | { kind: "folder"; id: ItemId; name: string };

export type ContainerTarget = { kind: "collection" } | { kind: "folder"; id: ItemId; name: string };

export type DialogRequest =
  | { kind: "saveTo"; tabKey: string }
  | { kind: "compare"; tabKey: string }
  | { kind: "description"; target: { kind: "collection" } | { kind: "folder"; id: ItemId } }
  /** Environment manager (PLAN.md Phase 3). */
  | { kind: "environments"; select?: ItemId }
  /** Variables of the globals, the collection or a folder. */
  | { kind: "variables"; target: VariablesTarget }
  /** Secret backend settings. */
  | { kind: "secrets" }
  /** Fill in missing secrets. */
  | { kind: "fillSecrets" }
  /** The resolved request as it would be sent. */
  | { kind: "preview"; tabKey: string }
  /** Auth and default headers of the collection or a folder (PLAN.md Phase 4). */
  | { kind: "containerSettings"; target: ContainerTarget }
  /** The cookie jar. */
  | { kind: "cookies" }
  /** Proxy and CA certificates. */
  | { kind: "network" }
  /** History retention and per-workspace body storage (PLAN.md Phase 5). */
  | { kind: "history" }
  /** cURL and OpenAPI import (PLAN.md Phase 6). */
  | { kind: "import"; tab?: "curl" | "openapi" }
  /** "Copy as cURL" for an open tab or a request in the tree. */
  | { kind: "copyCurl"; source: CopyCurlSource }
  /** The collection runner (PLAN.md Phase 7); what to run is in `app.runner`. */
  | { kind: "runner" }
  /** Ctrl+P: go to a request by name (PLAN.md Phase 8). */
  | { kind: "quickOpen" }
  /** App settings (PLAN.md Phase 8). */
  | { kind: "settings"; section?: "appearance" | "requests" | "privacy" | "more" | "about" };

export type CopyCurlSource = { tabKey: string } | { requestId: ItemId };

const [dialog, setDialog] = createSignal<DialogRequest>();
const [promptRequest, setPrompt] = createSignal<PromptRequest>();

export { dialog, promptRequest };

/** True while any in-app dialog or prompt is open (global shortcuts pause). */
export const anyDialog = () => !!dialog() || !!promptRequest();

export function openDialog(d: DialogRequest) {
  setDialog(d);
}

export function closeDialog() {
  setDialog(undefined);
}

function ask(request: Omit<PromptRequest, "resolve">): Promise<string | null> {
  promptRequest()?.resolve(null);
  return new Promise((resolve) => {
    setPrompt({
      ...request,
      resolve: (v) => {
        setPrompt(undefined);
        resolve(v);
      },
    });
  });
}

/** Asks for a line of text; resolves `null` if cancelled. */
export function prompt(
  title: string,
  label: string,
  initial = "",
  okLabel = "OK",
): Promise<string | null> {
  return ask({ title, label, initial, okLabel });
}

/**
 * Asks for a secret value or passphrase (masked). With `confirm`, asks twice and only
 * accepts matching entries. Resolves `null` if cancelled.
 */
export function promptSecret(
  title: string,
  label: string,
  okLabel = "OK",
  confirm = false,
): Promise<string | null> {
  return ask({ title, label, initial: "", okLabel, password: true, confirm });
}
