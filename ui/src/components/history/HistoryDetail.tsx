import { createResource, Match, onCleanup, Show, Switch } from "solid-js";
import { api, type HistoryEntryDraft } from "../../ipc/api";
import type { AppStore } from "../../stores/app";
import { formatBytes, formatMs, statusClass } from "../common/format";
import { Tabs } from "../common/Tabs";
import { HeadersTable } from "../response/HeadersTable";
import { ResponseBody } from "../response/ResponseBody";
import { SentRequest } from "../response/SentRequest";
import { TimingBreakdown } from "../response/TimingBreakdown";
import styles from "./history.module.css";

type Tab = "response" | "headers" | "request";

/** One history entry: what was sent, what came back, and actions. */
export function HistoryDetail(props: { app: AppStore; entry: HistoryEntryDraft }) {
  const e = () => props.entry;
  const [body] = createResource(
    () => (e().summary.hasBody ? e().summary.id : null),
    (id) => api.historyOpenBody(id),
  );
  onCleanup(() => {
    const id = body()?.bodyId;
    if (id !== undefined && id !== null) void api.closeBody(id).catch(() => {});
  });

  return (
    <section class={styles.detail} aria-label="History entry">
      <div class={styles.detailHeader}>
        <div class={styles.detailTitle}>
          <strong>{e().summary.method}</strong>{" "}
          <code title={e().summary.url}>{e().summary.url}</code>
        </div>
        <div class={styles.actions}>
          <button
            type="button"
            class="primary"
            onClick={() => void props.app.openFromHistory(e(), false)}
          >
            Open in new tab
          </button>
          <button type="button" onClick={() => void props.app.openFromHistory(e(), true)}>
            Resend
          </button>
          <button
            type="button"
            class="ghost"
            onClick={() => void props.app.history.remove(e().summary.id)}
          >
            Delete
          </button>
        </div>
        <p class={styles.muted}>
          {new Date(e().summary.at).toLocaleString()}
          <Show when={e().summary.workspaceName}> · {e().summary.workspaceName}</Show> ·{" "}
          {e().summary.requestName}
        </p>
      </div>
      <Show
        when={e().response}
        fallback={
          <div class={styles.failed} role="alert">
            <strong>{e().summary.error ?? "Failed"}</strong>
            <p>{e().errorMessage}</p>
          </div>
        }
      >
        {(r) => (
          <div class={styles.meta}>
            <span class={styles.status} data-class={statusClass(r().status)}>
              {r().status} {r().statusText}
            </span>
            <span>{formatMs(r().timings.totalMs)}</span>
            <span>{formatBytes(r().size)}</span>
            <span class={styles.muted}>{r().httpVersion}</span>
            <Show when={r().bodyTruncated}>
              <span class={styles.muted}>(body truncated in history)</span>
            </Show>
          </div>
        )}
      </Show>
      <div class={styles.detailTabs}>
        <Tabs<Tab>
          label="History entry"
          tabs={[
            { key: "response", label: "Body" },
            { key: "headers", label: "Headers" },
            { key: "request", label: "Request" },
          ]}
        >
          {(active) => (
            <Switch>
              <Match when={active === "response"}>
                <Show
                  when={e().summary.hasBody}
                  fallback={<p class={styles.empty}>No body was stored for this entry.</p>}
                >
                  <Show when={body()} fallback={<p class={styles.empty}>Loading…</p>}>
                    {(b) => (
                      <ResponseBody
                        body={b()}
                        status={e().response?.status ?? 0}
                        url={e().summary.url}
                      />
                    )}
                  </Show>
                </Show>
              </Match>
              <Match when={active === "headers"}>
                <div class={styles.pad}>
                  <Show when={e().response}>
                    {(r) => (
                      <>
                        <HeadersTable label="Response headers" headers={r().headers} />
                        <TimingBreakdown timings={r().timings} />
                      </>
                    )}
                  </Show>
                </div>
              </Match>
              <Match when={active === "request"}>
                <Show
                  when={e().sent}
                  fallback={<p class={styles.empty}>The request wasn't sent.</p>}
                >
                  {(sent) => <SentRequest request={sent()} />}
                </Show>
              </Match>
            </Switch>
          )}
        </Tabs>
      </div>
    </section>
  );
}
