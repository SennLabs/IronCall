import { createResource, createSignal, createUniqueId, For, Show } from "solid-js";
import { api, type TreeNode } from "../../ipc/api";
import { type QuickOpenEntry, rankEntries } from "../../model/quickOpen";
import type { AppStore } from "../../stores/app";
import { closeDialog } from "../../stores/dialogs";
import { MethodBadge } from "../common/MethodBadge";
import { Modal } from "../common/Modal";
import styles from "./quickopen.module.css";

function treeEntries(root: TreeNode): QuickOpenEntry[] {
  const out: QuickOpenEntry[] = [];
  const walk = (n: TreeNode, crumbs: string[]) => {
    for (const c of n.children) {
      if (c.kind === "request") {
        out.push({
          id: c.id,
          name: c.name,
          where: crumbs.join(" / "),
          method: c.method ?? "GET",
          url: c.url ?? "",
          scratch: false,
        });
      } else if (c.kind === "folder") {
        walk(c, [...crumbs, c.name]);
      }
    }
  };
  walk(root, []);
  return out;
}

/** Ctrl+P: find a request (workspace or scratch) by name, folder, method or URL. */
export function QuickOpen(props: { app: AppStore }) {
  const id = createUniqueId();
  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal(0);
  const [scratch] = createResource(async () => {
    const list = await api.scratchList().catch(() => []);
    return list.map(
      (r): QuickOpenEntry => ({
        id: r.id,
        name: r.name,
        where: "Scratch",
        method: r.method,
        url: r.url,
        scratch: true,
      }),
    );
  });
  const entries = () => {
    const tree = props.app.workspace.info()?.tree;
    return [...(tree ? treeEntries(tree) : []), ...(scratch() ?? [])];
  };
  const results = () => rankEntries(entries(), query());

  const choose = async (entry: QuickOpenEntry | undefined) => {
    if (!entry) return;
    closeDialog();
    if (entry.scratch) {
      const draft = (await api.scratchList().catch(() => [])).find((r) => r.id === entry.id);
      if (draft) props.app.tabs.openScratch(draft);
    } else {
      await props.app.openRequest(entry.id, true);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const n = results().length;
    if (e.key === "ArrowDown" && n > 0) {
      e.preventDefault();
      setSelected((selected() + 1) % n);
    } else if (e.key === "ArrowUp" && n > 0) {
      e.preventDefault();
      setSelected((selected() - 1 + n) % n);
    } else if (e.key === "Enter") {
      e.preventDefault();
      void choose(results()[selected()]);
    }
  };

  return (
    <Modal title="Go to request" onClose={closeDialog}>
      <div class={styles.body}>
        <input
          class={styles.search}
          type="search"
          role="combobox"
          aria-label="Search requests"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-activedescendant={results().length ? `${id}-${selected()}` : undefined}
          placeholder="Name, folder, method or URL"
          autofocus
          value={query()}
          onInput={(e) => {
            setQuery(e.currentTarget.value);
            setSelected(0);
          }}
          onKeyDown={onKeyDown}
        />
        <Show
          when={results().length > 0}
          fallback={<p class={styles.empty}>No matching requests.</p>}
        >
          <div id={`${id}-list`} role="listbox" aria-label="Requests" class={styles.list}>
            <For each={results()}>
              {(entry, i) => (
                <div
                  id={`${id}-${i()}`}
                  role="option"
                  aria-selected={i() === selected()}
                  tabIndex={-1}
                  class={styles.option}
                  onMouseMove={() => setSelected(i())}
                  onClick={() => void choose(entry)}
                  onKeyDown={(e) => e.key === "Enter" && void choose(entry)}
                >
                  <MethodBadge method={entry.method} />
                  <span class={styles.name}>{entry.name}</span>
                  <span class={styles.where}>
                    {[entry.where, entry.url].filter(Boolean).join(" · ")}
                  </span>
                </div>
              )}
            </For>
          </div>
        </Show>
      </div>
    </Modal>
  );
}
