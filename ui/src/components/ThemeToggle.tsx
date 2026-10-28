import { cycleThemeSetting } from "../stores/settings";
import { theme } from "../theme/theme";
import styles from "./ThemeToggle.module.css";

const LABELS = { system: "Theme: System", light: "Theme: Light", dark: "Theme: Dark" } as const;

export function ThemeToggle() {
  return (
    <button type="button" class={styles.button} onClick={cycleThemeSetting} title="Switch theme">
      {LABELS[theme()]}
    </button>
  );
}
