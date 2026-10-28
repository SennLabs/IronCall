import { createSignal, type JSX } from "solid-js";
import styles from "./SplitPane.module.css";

export interface SplitPaneProps {
  first: JSX.Element;
  second: JSX.Element;
  /** "row" puts panes side by side; "column" stacks them. */
  direction?: "row" | "column";
  /** Initial size of the first pane, as a fraction (0–1). */
  initial?: number;
}

/** Two panes with a draggable (and keyboard-adjustable) divider. */
export function SplitPane(props: SplitPaneProps) {
  const [fraction, setFraction] = createSignal(props.initial ?? 0.5);
  let container!: HTMLDivElement;
  const row = () => (props.direction ?? "row") === "row";
  const clamp = (f: number) => Math.min(0.85, Math.max(0.15, f));

  const onPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const move = (ev: PointerEvent) => {
      const rect = container.getBoundingClientRect();
      setFraction(
        clamp(
          row() ? (ev.clientX - rect.left) / rect.width : (ev.clientY - rect.top) / rect.height,
        ),
      );
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const keys = row() ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
    const dir = e.key === keys[0] ? -1 : e.key === keys[1] ? 1 : 0;
    if (dir) {
      e.preventDefault();
      setFraction((f) => clamp(f + dir * 0.05));
    }
  };

  return (
    <div
      ref={container}
      class={styles.split}
      style={{ "flex-direction": row() ? "row" : "column" }}
    >
      <div class={styles.pane} style={{ "flex-basis": `${fraction() * 100}%` }}>
        {props.first}
      </div>
      {/* biome-ignore lint/a11y/useSemanticElements: an <hr> can't be focused or dragged. */}
      <div
        role="separator"
        aria-orientation={row() ? "vertical" : "horizontal"}
        aria-valuenow={Math.round(fraction() * 100)}
        tabIndex={0}
        class={styles.divider}
        classList={{ [styles.vertical as string]: row() }}
        onPointerDown={onPointerDown}
        onKeyDown={onKeyDown}
      />
      <div class={styles.pane} style={{ "flex-basis": `${(1 - fraction()) * 100}%` }}>
        {props.second}
      </div>
    </div>
  );
}
