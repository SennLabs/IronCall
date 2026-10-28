import { type JSX, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import styles from "./Modal.module.css";

export interface ModalProps {
  title: string;
  onClose: () => void;
  children: JSX.Element;
  footer?: JSX.Element;
  wide?: boolean;
}

const FOCUSABLE =
  'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

/**
 * A modal dialog: Esc and clicking the backdrop close it; focus starts inside and Tab
 * cycles within it.
 */
export function Modal(props: ModalProps) {
  let panel!: HTMLDivElement;
  /** True for the dialog on top (a prompt stacks above the main dialog). */
  const topmost = () => {
    const dialogs = document.querySelectorAll('[role="dialog"][aria-modal="true"]');
    return dialogs[dialogs.length - 1] === panel;
  };
  // Keeps Tab / Shift+Tab inside the topmost dialog.
  const trapFocus = (e: KeyboardEvent) => {
    if (!topmost()) return;
    const items = [...panel.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
      (el) => !el.closest("[hidden]"),
    );
    const first = items[0];
    const last = items[items.length - 1];
    if (!first || !last) return;
    const current = document.activeElement;
    if (e.shiftKey && (current === first || !panel.contains(current))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (current === last || !panel.contains(current))) {
      e.preventDefault();
      first.focus();
    }
  };
  onMount(() => {
    // Focus goes back to whatever opened the dialog (WCAG 2.4.3 focus order).
    const opener = document.activeElement;
    onCleanup(() => {
      const inside =
        panel.contains(document.activeElement) || document.activeElement === document.body;
      if (opener instanceof HTMLElement && opener.isConnected && inside) opener.focus();
    });
    const first =
      panel.querySelector<HTMLElement>("[autofocus]") ??
      panel.querySelector<HTMLElement>("input, textarea, button");
    first?.focus();
    const onKey = (e: KeyboardEvent) => {
      // Every open dialog listens on window: only the top one closes, so Escape in a
      // stacked prompt doesn't also discard the dialog underneath.
      if (e.key === "Escape" && topmost()) {
        e.preventDefault();
        e.stopPropagation();
        props.onClose();
      } else if (e.key === "Tab") {
        trapFocus(e);
      }
    };
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });
  return (
    <Portal>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: backdrop click closes; Esc is handled above. */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: keyboard close is the Esc handler. */}
      <div class={styles.backdrop} onClick={(e) => e.target === e.currentTarget && props.onClose()}>
        <div
          ref={panel}
          class={styles.panel}
          classList={{ [styles.wide as string]: !!props.wide }}
          role="dialog"
          aria-modal="true"
          aria-label={props.title}
        >
          <div class={styles.header}>
            <h2>{props.title}</h2>
            <button type="button" class="ghost" aria-label="Close" onClick={() => props.onClose()}>
              ✕
            </button>
          </div>
          <div class={styles.body}>{props.children}</div>
          {props.footer && <footer class={styles.footer}>{props.footer}</footer>}
        </div>
      </div>
    </Portal>
  );
}
