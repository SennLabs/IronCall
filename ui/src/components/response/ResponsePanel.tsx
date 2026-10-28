import { Match, Show, Switch } from "solid-js";
import { uploadPercent } from "../../model/bodyFiles";
import { previewKind } from "../../model/bodyView";
import type { SendStore } from "../../stores/send";
import { formatBytes } from "../common/format";
import { Tabs } from "../common/Tabs";
import { CheckResults, checkSummary } from "../tests/CheckResults";
import { BodyPreview } from "./BodyPreview";
import { ErrorPanel } from "./ErrorPanel";
import { HeadersTable } from "./HeadersTable";
import { ResponseBody } from "./ResponseBody";
import { ResponseMeta } from "./ResponseMeta";
import styles from "./response.module.css";
import { SentRequest } from "./SentRequest";
import { Timeline } from "./Timeline";
import { TlsSummary } from "./TlsSummary";
import { Waterfall } from "./Waterfall";

const STAGE_LABEL = {
  connecting: "Connecting…",
  uploading: "Uploading…",
  waiting: "Waiting for response…",
  downloading: "Downloading…",
} as const;

type Tab = "body" | "preview" | "headers" | "tests" | "timing" | "tls" | "timeline" | "request";

/** Upload progress bar with percentage and bytes (file and multipart bodies). */
export function UploadProgress(props: { uploaded: number; total: number | null }) {
  const percent = () => uploadPercent(props.uploaded, props.total);
  return (
    <div class={styles.upload}>
      <div
        class={styles.progress}
        role="progressbar"
        aria-label="Upload progress"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent() ?? undefined}
      >
        <div class={styles.progressBar} style={{ width: `${percent() ?? 100}%` }} />
      </div>
      <p class={styles.muted}>
        {percent() !== null ? `${percent()}% · ` : ""}
        {formatBytes(props.uploaded)}
        {props.total ? ` of ${formatBytes(props.total)}` : ""} sent
      </p>
    </div>
  );
}

export function ResponsePanel(props: { sender: SendStore; onCopyCurl?: () => void }) {
  const state = () => props.sender.state();
  return (
    <section class={styles.panel} aria-label="Response">
      <Switch>
        <Match when={state().status === "idle"}>
          <div class={styles.emptyState}>
            <p>Send a request to see the response here.</p>
            <p class={styles.muted}>
              <kbd>Ctrl</kbd>+<kbd>Enter</kbd> sends · <kbd>Esc</kbd> cancels
            </p>
          </div>
        </Match>
        <Match
          when={(() => {
            const s = state();
            return s.status === "sending" ? s : undefined;
          })()}
        >
          {(s) => (
            <div class={styles.emptyState} role="status" aria-live="polite">
              <p>{STAGE_LABEL[s().stage]}</p>
              <Show when={s().stage === "uploading"}>
                <UploadProgress uploaded={s().uploaded} total={s().uploadTotal} />
              </Show>
              <p class={styles.muted}>
                {s().bytes > 0 ? `${formatBytes(s().bytes)} received` : ""}
              </p>
              <button type="button" onClick={() => void props.sender.cancel()}>
                Cancel
              </button>
            </div>
          )}
        </Match>
        <Match
          when={(() => {
            const s = state();
            return s.status === "error" ? s : undefined;
          })()}
        >
          {(s) => <ErrorPanel error={s().error} />}
        </Match>
        <Match
          when={(() => {
            const s = state();
            return s.status === "done" ? s : undefined;
          })()}
        >
          {(s) => {
            const r = () => s().response;
            const testsLabel = () => {
              const c = r().checks;
              if (!c) return "Tests";
              const sum = checkSummary(c.assertions, c.extracted);
              const count = sum.total > 0 ? ` ${sum.passed}/${sum.total}` : "";
              return `Tests${count}${sum.ok ? "" : " ✗"}`;
            };
            const preview = () =>
              r().body.bodyId !== null ? previewKind(r().body.contentType, r().body.kind) : null;
            const tabs = () =>
              [
                { key: "body" as const, label: "Body" },
                ...(preview() ? [{ key: "preview" as const, label: "Preview" }] : []),
                { key: "headers" as const, label: `Headers (${r().headers.length})` },
                ...(r().checks ? [{ key: "tests" as const, label: testsLabel() }] : []),
                { key: "timing" as const, label: "Timing" },
                ...(r().tls ? [{ key: "tls" as const, label: "TLS" }] : []),
                { key: "timeline" as const, label: "Timeline" },
                { key: "request" as const, label: "Request" },
              ] satisfies { key: Tab; label: string }[];
            return (
              <div class={styles.done}>
                <ResponseMeta response={s().response} />
                <div class={styles.tabsHost}>
                  <Tabs<Tab> label="Response" tabs={tabs()}>
                    {(active) => (
                      <Switch>
                        <Match when={active === "body"}>
                          <ResponseBody body={r().body} status={r().status} url={r().request.url} />
                        </Match>
                        <Match when={active === "preview" && preview()}>
                          {(kind) => <BodyPreview body={r().body} kind={kind()} />}
                        </Match>
                        <Match when={active === "tests" && r().checks}>
                          {(c) => (
                            <CheckResults assertions={c().assertions} extracted={c().extracted} />
                          )}
                        </Match>
                        <Match when={active === "timing"}>
                          <Waterfall response={r()} />
                        </Match>
                        <Match when={active === "tls" && r().tls}>
                          {(tls) => <TlsSummary tls={tls()} />}
                        </Match>
                        <Match when={active === "timeline"}>
                          <Timeline entries={r().timeline} />
                        </Match>
                        <Match when={active === "headers"}>
                          <div class={styles.section}>
                            <HeadersTable label="Response headers" headers={s().response.headers} />
                          </div>
                        </Match>
                        <Match when={active === "request"}>
                          <SentRequest
                            request={s().response.request}
                            onCopyCurl={props.onCopyCurl}
                          />
                        </Match>
                      </Switch>
                    )}
                  </Tabs>
                </div>
              </div>
            );
          }}
        </Match>
      </Switch>
    </section>
  );
}
