/**
 * In-memory runtime variables and collection runs for tests and browser-only dev
 * (PLAN.md Phase 7). Runs send through the mock's own `sendRequest`, so `respond`
 * decides each response; assertions aren't evaluated here: a response passes unless its
 * `checks` (set by `respond`) say otherwise.
 */

import { type Api, SendFailure } from "./api";
import type {
  ItemResult,
  Outcome,
  ResponseChecks,
  RunItem,
  RunProgress,
  RunReport,
  RunTarget,
  RuntimeVarView,
  TreeNode,
} from "./bindings";
import { VarsFailure } from "./varsTypes";

export interface MockRunSeed {
  /** Runtime variables at start (`value` is the real value). */
  runtime?: RuntimeVarView[];
}

type RunApi = Pick<
  Api,
  | "runItems"
  | "runCollection"
  | "runCancel"
  | "runReportMarkdown"
  | "onRunProgress"
  | "runtimeVars"
  | "runtimeVarsClear"
  | "runtimeVarSave"
>;

export function createMockRun(mock: () => Api, seed: MockRunSeed = {}) {
  let runtime: RuntimeVarView[] = [...(seed.runtime ?? [])];
  const listeners = new Set<(p: RunProgress) => void>();
  const cancelled = new Set<string>();
  const reports = new Map<string, { title: string; report: RunReport }>();
  const saved: string[] = [];

  const set = (v: RuntimeVarView) => {
    runtime = [...runtime.filter((r) => r.name !== v.name), v];
  };

  /** Applies a response's successful extractions (values as the mock reports them). */
  const applyChecks = (checks: ResponseChecks | null) => {
    for (const e of checks?.extracted ?? []) {
      if (e.value !== null) set({ name: e.var, value: e.value, sensitive: e.sensitive });
    }
  };

  const collect = (node: TreeNode, crumbs: string[], out: RunItem[]) => {
    for (const child of node.children) {
      if (child.kind === "request") {
        out.push({ id: child.id, name: child.name, folder: crumbs.join(" / ") });
      } else if (child.kind === "folder") {
        collect(child, [...crumbs, child.name], out);
      }
    }
  };

  const items = async (target: RunTarget): Promise<{ title: string; list: RunItem[] }> => {
    const ws = await mock().workspaceCurrent();
    if (!ws) throw new VarsFailure("no workspace is open", false, false);
    const find = (n: TreeNode): TreeNode | undefined =>
      n.id === target.folder ? n : n.children.map(find).find(Boolean);
    const root = target.folder === null ? ws.tree : find(ws.tree);
    if (!root) throw new VarsFailure("that folder no longer exists", false, false);
    const list: RunItem[] = [];
    collect(root, [], list);
    return { title: root.name, list };
  };

  const emit = (p: RunProgress) => {
    for (const l of listeners) l(p);
  };

  const api: RunApi = {
    runItems: async (target) => (await items(target)).list,
    async runCollection(runId, target, _environment, options) {
      const { title, list } = await items(target);
      const started = Date.now();
      const results: ItemResult[] = [];
      let stopped = false;
      let n = 0;
      const halted = () => cancelled.has(runId) || stopped;
      outer: for (let iteration = 1; iteration <= Math.max(1, options.iterations); iteration++) {
        for (const [index, item] of list.entries()) {
          if (n > 0 && options.delayMs > 0)
            await new Promise((r) => setTimeout(r, options.delayMs));
          if (halted()) break outer;
          emit({ runId, progress: { type: "started", iteration, index } });
          const base = { item, iteration, index, assertions: [], extracted: [] };
          let result: ItemResult;
          try {
            const { request } = await mock().requestLoad(item.id);
            const r = await mock().sendRequest(`${runId}-${n++}`, request);
            const failed =
              !!r.checks &&
              (r.checks.assertions.some((a) => !a.passed) ||
                r.checks.extracted.some((e) => e.error));
            result = {
              ...base,
              outcome: failed ? "failed" : "passed",
              status: r.status,
              durationMs: r.timings.totalMs,
              error: null,
              assertions: r.checks?.assertions ?? [],
              extracted: r.checks?.extracted ?? [],
              historyId: r.historyId,
            };
          } catch (e) {
            const failure = e instanceof SendFailure ? e.error : null;
            const outcome: Outcome = failure?.kind === "cancelled" ? "cancelled" : "error";
            result = {
              ...base,
              outcome,
              status: null,
              durationMs: null,
              error: outcome === "error" ? (e as Error).message : null,
              historyId: failure?.historyId ?? null,
            };
          }
          if (
            options.stopOnFailure &&
            (result.outcome === "failed" || result.outcome === "error")
          ) {
            stopped = true;
          }
          results.push(result);
          emit({ runId, progress: { type: "finished", result } });
        }
      }
      const count = (o: Outcome) => results.filter((r) => r.outcome === o).length;
      const report: RunReport = {
        results,
        passed: count("passed"),
        failed: count("failed"),
        errors: count("error"),
        cancelled: cancelled.has(runId),
        stoppedEarly: stopped && !cancelled.has(runId),
        totalMs: Date.now() - started,
      };
      reports.set(runId, { title, report });
      return report;
    },
    async runCancel(runId) {
      cancelled.add(runId);
    },
    async runReportMarkdown(runId) {
      const r = reports.get(runId);
      if (!r) return null;
      const rows = r.report.results.map(
        (x) => `| ${x.index + 1} | ${x.item.name} | ${x.status ?? "–"} | ${x.outcome} |`,
      );
      return [
        `## Run: ${r.title}`,
        "",
        `**${r.report.passed} passed, ${r.report.failed} failed, ${r.report.errors} errors**`,
        "",
        "| # | Request | Status | Result |",
        "|---|---|---|---|",
        ...rows,
        "",
      ].join("\n");
    },
    async onRunProgress(cb) {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    async runtimeVars(reveal) {
      return runtime.map((v) => (v.sensitive && !reveal ? { ...v, value: "••••" } : v));
    },
    async runtimeVarsClear(name) {
      runtime = name === null ? [] : runtime.filter((v) => v.name !== name);
    },
    async runtimeVarSave(name) {
      if (!runtime.some((v) => v.name === name)) {
        throw new VarsFailure(`there's no runtime variable \`${name}\``, false, false);
      }
      saved.push(name);
    },
  };

  return {
    api,
    applyChecks,
    external: {
      /** Names passed to `runtimeVarSave`, in order. */
      saved,
      setRuntime(vars: RuntimeVarView[]) {
        runtime = [...vars];
      },
      /** The real runtime values (unmasked). */
      runtime: () => runtime,
    },
  };
}
