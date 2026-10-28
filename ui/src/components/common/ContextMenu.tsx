import { For, onCleanup, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import styles from "./ContextMenu.module.css";

export interface MenuItem {
  label: string;
  action: () => void;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
}

/** `null` entries render as separators. */
export type MenuEntries = (MenuItem | null)[];

export interface ContextMenuProps {
  x: number;
  y: number;
  items: MenuEntries;
  label: string;
  onClose: () => void;
}

/** A keyboard-navigable context menu at a screen position. */
export function ContextMenu(props: ContextMenuProps) {
  let menu!: HTMLDivElement;
  const buttons = () => [...menu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];

  onMount(() => {
    // Keep the menu on screen.
    const rect = menu.getBoundingClientRect();
    if (rect.right > window.innerWidth) menu.style.left = `${Math.max(0, props.x - rect.width)}px`;
    if (rect.bottom > window.innerHeight)
      menu.style.top = `${Math.max(0, props.y - rect.height)}px`;
    buttons()[0]?.focus();
    const away = (e: Event) => {
      if (!menu.contains(e.target as Node)) props.onClose();
    };
    window.addEventListener("pointerdown", away, true);
    window.addEventListener("blur", props.onClose);
    onCleanup(() => {
      window.removeEventListener("pointerdown", away, true);
      window.removeEventListener("blur", props.onClose);
    });
  });

  const onKeyDown = (e: KeyboardEvent) => {
    const list = buttons();
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      list[(i + step + list.length) % list.length]?.focus();
    } else if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      // Don't let Escape reach the global shortcuts (cancel send, close History).
      e.stopPropagation();
      props.onClose();
    }
  };

  return (
    <Portal>
      <div
        ref={menu}
        class={styles.menu}
        role="menu"
        aria-label={props.label}
        style={{ left: `${props.x}px`, top: `${props.y}px` }}
        onKeyDown={onKeyDown}
      >
        <For each={props.items}>
          {(item) => (
            <Show when={item} fallback={<hr class={styles.separator} />}>
              {(it) => (
                <button
                  type="button"
                  role="menuitem"
                  class={styles.item}
                  classList={{ [styles.danger as string]: !!it().danger }}
                  disabled={it().disabled}
                  onClick={() => {
                    // Run the action before closing: closing unmounts the menu, and
                    // actions often read state captured by it.
                    const action = it().action;
                    action();
                    props.onClose();
                  }}
                >
                  <span>{it().label}</span>
                  <Show when={it().shortcut}>
                    <kbd class={styles.shortcut}>{it().shortcut}</kbd>
                  </Show>
                </button>
              )}
            </Show>
          )}
        </For>
      </div>
    </Portal>
  );
}
