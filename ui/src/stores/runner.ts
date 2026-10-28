/**
 * The collection runner (PLAN.md Phase 7): what to run, the options, and the live and
 * final results. One run at a time; its results stay here after the dialog closes, so
 * the user can drill into history and come back.
 */

import { batch, createSignal } from "solid-js";
import {
  api,
  type EnvChoice,
  type ItemResult,
  type RunItem,
  type RunOptions,
  type RunReport,
  type RunTarget,
} from "../ipc/api";
import { attempt, notify } from "./notify";

export const DEFAULT_RUN_OPTIONS: RunOptions = {
  iterations: 1,
  delayMs: 0,
  stopOnFailure: false,
  parallel: false,
};

export interface RunnerStoreOptions {
  /** A run finished: runtime variables and history changed. */
  onFinished?: () => void;
}

let nextRun = 1;

export function createRunnerStore(options: RunnerStoreOptions = {}) {
  const [target, setTarget] = createSignal<RunTarget>({ folder: null });
  const [title, setTitle] = createSignal("");
  const [items, setItems] = createSignal<RunItem[]>([]);
  const [runOptions, setRunOptions] = createSignal<RunOptions>(DEFAULT_RUN_OPTIONS);
  const [environment, setEnvironment] = createSignal<EnvChoice>({ kind: "active" });
  const [runId, setRunId] = createSignal<string>();
  const [running, setRunning] = createSignal(false);
  const [results, setResults] = createSignal<ItemResult[]>([]);
  const [current, setCurrent] = createSignal<{ iteration: number; index: number }>();
  const [report, setReport] = createSignal<RunReport>();
  /** Root of the workspace the current target belongs to. */
  let workspaceRoot: string | null = null;

  const reset = () =>
    batch(() => {
      setResults([]);
      setReport(undefined);
      setCurrent(undefined);
      setRunId(undefined);
    });

  const store = {
    target,
    title,
    items,
    options: runOptions,
    environment,
    running,
    results,
    current,
    report,

    /** Total requests this run will send (items × iterations). */
    total: () => items().length * Math.max(1, runOptions().iterations),

    /**
     * Chooses what to run. Keeps the last results if it's the same target in the same
     * workspace (`workspace` is its root; a whole-workspace target is the same object in
     * every workspace).
     */
    async prepare(next: RunTarget, name: string, workspace: string | null = null) {
      if (running()) return;
      const same = next.folder === target().folder && title() !== "" && workspace === workspaceRoot;
      workspaceRoot = workspace;
      batch(() => {
        setTarget(next);
        setTitle(name);
      });
      if (!same) reset();
      const list = await attempt(() => api.runItems(next), "Couldn't list the requests to run");
      setItems(list ?? []);
    },

    setOptions(patch: Partial<RunOptions>) {
      setRunOptions((o) => ({ ...o, ...patch }));
    },
    setEnvironment,

    async start() {
      if (running() || items().length === 0) return;
      const id = `run-${Date.now()}-${nextRun++}`;
      batch(() => {
        reset();
        setRunId(id);
        setRunning(true);
      });
      let unlisten: (() => void) | undefined;
      try {
        // Inside the try: if listening fails, `finally` still clears "running".
        unlisten = await api.onRunProgress((p) => {
          if (p.runId !== id) return;
          if (p.progress.type === "started") {
            setCurrent({ iteration: p.progress.iteration, index: p.progress.index });
          } else {
            const r = p.progress.result;
            setResults((list) => [...list, r]);
          }
        });
        const final = await attempt(
          () => api.runCollection(id, target(), environment(), runOptions()),
          "The run couldn't start",
        );
        if (final) {
          batch(() => {
            setReport(final);
            // The report is complete and in run order (parallel runs finish out of order).
            setResults(final.results);
          });
          if (final.cancelled) notify("Run cancelled", "info");
        }
      } finally {
        unlisten?.();
        batch(() => {
          setRunning(false);
          setCurrent(undefined);
        });
        options.onFinished?.();
      }
    },

    async cancel() {
      const id = runId();
      if (id && running()) await api.runCancel(id);
    },

    /** Copies the finished run as a Markdown table. */
    async copyMarkdown() {
      const id = runId();
      if (!id) return;
      const md = await attempt(() => api.runReportMarkdown(id), "Couldn't build the report");
      if (!md) return;
      try {
        await navigator.clipboard.writeText(md);
        notify("Results copied as Markdown", "info");
      } catch {
        notify("Couldn't copy to the clipboard", "error");
      }
    },
  };
  return store;
}

export type RunnerStore = ReturnType<typeof createRunnerStore>;
