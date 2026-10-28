import { createSignal, For, Show } from "solid-js";
import type { EnvChoice, ItemResult, Outcome, RunItem } from "../../ipc/api";
import type { AppStore } from "../../stores/app";
import { closeDialog } from "../../stores/dialogs";
import { formatMs } from "../common/format";
import { Modal } from "../common/Modal";
import { CheckResults, checkSummary } from "../tests/CheckResults";
import styles from "./runner.module.css";

const OUTCOME_LABEL: Record<Outcome, string> = {
  passed: "Passed",
  failed: "Failed",
  error: "Error",
  cancelled: "Cancelled",
};

const ACTIVE = "active";
const NONE = "none";

function envValue(e: EnvChoice) {
  return e.kind === "id" ? e.id : e.kind === "none" ? NONE : ACTIVE;
}

/**
 * Collection runner (PLAN.md Phase 7): options, live progress, a results table with
 * per-request detail and drill-down into history, and "Copy results as Markdown".
 */
export function RunnerDialog(props: { app: AppStore }) {
  const runner = () => props.app.runner;
  const vars = () => props.app.vars;
  const [open, setOpen] = createSignal<string>();
  const multi = () => runner().options().iterations > 1;
  const key = (r: { iteration: number; index: number }) => `${r.iteration}.${r.index}`;
  const num = (r: { iteration: number; index: number }) =>
    multi() ? `${r.iteration}.${r.index + 1}` : `${r.index + 1}`;

  const done = () => runner().results().length;
  const percent = () => (runner().total() ? Math.round((done() / runner().total()) * 100) : 0);
  /** Rows: finished results, then (while running or before a run) the requests still to go. */
  const pending = (): (RunItem & { index: number })[] => {
    if (runner().report() || runner().results().length > 0) return [];
    return runner()
      .items()
      .map((item, index) => ({ ...item, index }));
  };

  const openInHistory = (id: string) => {
    closeDialog();
    props.app.history.setOpen(true);
    void props.app.history.select(id);
  };

  const summary = () => {
    const r = runner().report();
    if (!r) return null;
    const tail = r.cancelled
      ? " (cancelled)"
      : r.stoppedEarly
        ? " (stopped at the first failure)"
        : "";
    return `${r.passed} passed, ${r.failed} failed, ${r.errors} errors in ${formatMs(r.totalMs)}${tail}`;
  };

  return (
    <Modal
      title={`Run: ${runner().title()}`}
      wide
      onClose={closeDialog}
      footer={
        <>
          <Show when={runner().report()}>
            <button type="button" onClick={() => void runner().copyMarkdown()}>
              Copy results as Markdown
            </button>
          </Show>
          <span style={{ flex: 1 }} />
          <Show
            when={runner().running()}
            fallback={
              <button
                type="button"
                class="primary"
                disabled={runner().items().length === 0}
                onClick={() => void runner().start()}
              >
                {runner().report() ? "Run again" : "Run"}
              </button>
            }
          >
            <button type="button" onClick={() => void runner().cancel()}>
              Cancel run
            </button>
          </Show>
          <button type="button" onClick={closeDialog}>
            Close
          </button>
        </>
      }
    >
      <div class={styles.body}>
        <fieldset class={styles.options} disabled={runner().running()} aria-label="Run options">
          <label>
            Environment
            <select
              aria-label="Run environment"
              onChange={(e) => {
                const v = e.currentTarget.value;
                runner().setEnvironment(
                  v === ACTIVE
                    ? { kind: "active" }
                    : v === NONE
                      ? { kind: "none" }
                      : { kind: "id", id: v },
                );
              }}
            >
              <option value={ACTIVE} selected={envValue(runner().environment()) === ACTIVE}>
                Active ({vars().activeEnvironment()?.name ?? "none"})
              </option>
              <option value={NONE} selected={envValue(runner().environment()) === NONE}>
                No environment
              </option>
              <For each={vars().environments()}>
                {(env) => (
                  <option
                    value={env.id}
                    disabled={!!env.error}
                    selected={envValue(runner().environment()) === env.id}
                  >
                    {env.name}
                  </option>
                )}
              </For>
            </select>
          </label>
          <label>
            Iterations
            <input
              class={styles.number}
              type="number"
              min="1"
              max="1000"
              value={runner().options().iterations}
              onChange={(e) =>
                runner().setOptions({
                  iterations: Math.max(1, Math.floor(+e.currentTarget.value || 1)),
                })
              }
            />
          </label>
          <label>
            Delay (ms)
            <input
              class={styles.number}
              type="number"
              min="0"
              step="100"
              value={runner().options().delayMs}
              onChange={(e) =>
                runner().setOptions({
                  delayMs: Math.max(0, Math.floor(+e.currentTarget.value || 0)),
                })
              }
            />
          </label>
          <label class={styles.inline}>
            <input
              type="checkbox"
              checked={runner().options().stopOnFailure}
              onChange={(e) => runner().setOptions({ stopOnFailure: e.currentTarget.checked })}
            />
            Stop on first failure
          </label>
          <label
            class={styles.inline}
            title="Each iteration's requests run at once; values extracted in one request may not reach the others"
          >
            <input
              type="checkbox"
              checked={runner().options().parallel}
              onChange={(e) => runner().setOptions({ parallel: e.currentTarget.checked })}
            />
            Run in parallel
          </label>
        </fieldset>

        <Show
          when={runner().items().length > 0}
          fallback={<p class={styles.summary}>There are no requests to run here.</p>}
        >
          <Show when={runner().running()}>
            <div
              class={styles.progress}
              role="progressbar"
              aria-label="Run progress"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent()}
            >
              <div class={styles.progressBar} style={{ width: `${percent()}%` }} />
            </div>
            <p class={styles.summary} role="status">
              Running {Math.min(done() + 1, runner().total())} of {runner().total()}…
            </p>
          </Show>
          <Show when={summary()}>
            {(s) => (
              <p class={styles.summary} role="status" aria-label="Run summary">
                {s()}
              </p>
            )}
          </Show>
          <Show when={!runner().running() && !runner().report()}>
            <p class={styles.summary}>
              {runner().items().length} request{runner().items().length === 1 ? "" : "s"}
              {multi() ? ` × ${runner().options().iterations} iterations` : ""}, in order.
            </p>
          </Show>

          <table class={styles.table} aria-label="Run results">
            <thead>
              <tr>
                <th class={styles.num}>#</th>
                <th>Request</th>
                <th>Status</th>
                <th>Time</th>
                <th>Assertions</th>
                <th>Result</th>
              </tr>
            </thead>
            <tbody>
              <For each={runner().results()}>
                {(r) => (
                  <ResultRow
                    result={r}
                    num={num(r)}
                    open={open() === key(r)}
                    onToggle={() => setOpen(open() === key(r) ? undefined : key(r))}
                    onHistory={openInHistory}
                  />
                )}
              </For>
              <For each={pending()}>
                {(item) => (
                  <tr aria-label={item.name}>
                    <td class={styles.num}>{item.index + 1}</td>
                    <td>
                      <Show when={item.folder}>
                        <span class={styles.folder}>{item.folder} / </span>
                      </Show>
                      {item.name}
                    </td>
                    <td />
                    <td />
                    <td />
                    <td class={styles.pending}>
                      {runner().current()?.index === item.index ? "Running…" : "Pending"}
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </div>
    </Modal>
  );
}

function ResultRow(props: {
  result: ItemResult;
  num: string;
  open: boolean;
  onToggle: () => void;
  onHistory: (id: string) => void;
}) {
  const r = () => props.result;
  const checks = () => checkSummary(r().assertions, r().extracted);
  const hasDetail = () => r().assertions.length > 0 || r().extracted.length > 0 || !!r().error;
  return (
    <>
      <tr aria-label={`${r().item.name} ${OUTCOME_LABEL[r().outcome]}`}>
        <td class={styles.num}>{props.num}</td>
        <td>
          <Show when={r().item.folder}>
            <span class={styles.folder}>{r().item.folder} / </span>
          </Show>
          {r().item.name}
        </td>
        <td>{r().status ?? "–"}</td>
        <td>{r().durationMs !== null ? formatMs(r().durationMs) : "–"}</td>
        <td>{r().assertions.length > 0 ? `${checks().passed}/${checks().total}` : "–"}</td>
        <td>
          <span class={styles[r().outcome]}>{OUTCOME_LABEL[r().outcome]}</span>{" "}
          <Show when={hasDetail() || r().historyId}>
            <button
              type="button"
              class="ghost"
              aria-expanded={props.open}
              aria-label={`Details of ${r().item.name}`}
              onClick={props.onToggle}
            >
              {props.open ? "Hide" : "Details"}
            </button>
          </Show>
        </td>
      </tr>
      <Show when={props.open}>
        <tr class={styles.detailRow}>
          <td colSpan={6}>
            <Show when={r().error}>
              <p class={styles.errorText}>{r().error}</p>
            </Show>
            <CheckResults assertions={r().assertions} extracted={r().extracted} />
            <Show when={r().historyId}>
              {(id) => (
                <div class={styles.detailActions}>
                  <button type="button" onClick={() => props.onHistory(id())}>
                    Open in history
                  </button>
                </div>
              )}
            </Show>
          </td>
        </tr>
      </Show>
    </>
  );
}
