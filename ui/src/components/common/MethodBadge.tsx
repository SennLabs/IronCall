import styles from "./MethodBadge.module.css";

const KNOWN = new Set(["GET", "POST", "PUT", "PATCH", "DELETE"]);

/** Short coloured method label, e.g. for the tree and tabs. */
export function MethodBadge(props: { method: string | null | undefined }) {
  const m = () => (props.method ?? "GET").toUpperCase();
  const short = () => (m() === "DELETE" ? "DEL" : m() === "OPTIONS" ? "OPT" : m().slice(0, 5));
  return (
    <span
      class={styles.badge}
      data-method={KNOWN.has(m()) ? m().toLowerCase() : "other"}
      title={m()}
    >
      {short()}
    </span>
  );
}
