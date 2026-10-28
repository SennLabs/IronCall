import { createSignal } from "solid-js";

export interface CopyButtonProps {
  /** Text to copy, or a function that produces it (may be async, e.g. fetching a body). */
  text: string | (() => string | Promise<string>);
  label?: string;
  class?: string;
}

/** Copies text to the clipboard and briefly confirms. */
export function CopyButton(props: CopyButtonProps) {
  const [state, setState] = createSignal<"idle" | "copied" | "failed">("idle");
  const copy = async () => {
    try {
      const text = typeof props.text === "function" ? await props.text() : props.text;
      await navigator.clipboard.writeText(text);
      setState("copied");
    } catch {
      setState("failed");
    }
    setTimeout(() => setState("idle"), 1200);
  };
  const label = () => props.label ?? "Copy";
  return (
    <button type="button" class={props.class ?? "ghost"} onClick={copy} title={label()}>
      {state() === "copied" ? "Copied" : state() === "failed" ? "Copy failed" : label()}
    </button>
  );
}
