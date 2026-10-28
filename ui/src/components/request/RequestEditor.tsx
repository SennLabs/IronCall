import { type Accessor, Match, Show, Switch } from "solid-js";
import type { RequestPreview } from "../../ipc/bindings";
import type { RequestStore } from "../../stores/request";
import { Tabs } from "../common/Tabs";
import { AuthPanel } from "./AuthPanel";
import { BodyPanel } from "./BodyPanel";
import { DocsPanel } from "./DocsPanel";
import { HeadersPanel } from "./HeadersPanel";
import { ParamsPanel } from "./ParamsPanel";
import styles from "./panels.module.css";
import { SettingsPanel } from "./SettingsPanel";
import { TestsPanel } from "./TestsPanel";
import { VarsPanel } from "./VarsPanel";

type Tab = "params" | "headers" | "auth" | "body" | "vars" | "tests" | "settings" | "docs";

export interface RequestEditorProps {
  request: RequestStore;
  preview: Accessor<RequestPreview | undefined>;
  readOnly?: boolean;
}

const count = (n: number) => (n > 0 ? ` (${n})` : "");

export function RequestEditor(props: RequestEditorProps) {
  const d = () => props.request.draft();
  const enabledCount = (rows: { enabled?: boolean }[] | undefined) =>
    (rows ?? []).filter((r) => r.enabled !== false).length;
  const tlsOff = () => d().settings?.verifyTls === false;

  return (
    <Tabs<Tab>
      label="Request"
      initial="params"
      tabs={[
        { key: "params", label: `Params${count(enabledCount(d().params))}` },
        { key: "headers", label: `Headers${count(enabledCount(d().headers))}` },
        {
          key: "auth",
          label: (d().auth?.type ?? "inherit") === "inherit" ? "Auth" : "Auth •",
        },
        { key: "body", label: d().body && d().body?.type !== "none" ? "Body •" : "Body" },
        { key: "vars", label: `Vars${count(Object.keys(d().vars ?? {}).length)}` },
        {
          key: "tests",
          label: `Tests${count((d().assert ?? []).filter((a) => a.enabled !== false).length + (d().extract ?? []).length)}`,
        },
        { key: "docs", label: d().description ? "Docs •" : "Docs" },
        {
          key: "settings",
          label: (
            <>
              Settings
              <Show when={tlsOff()}>
                {" "}
                <span class={`${styles.badge} ${styles.warning}`}>TLS off</span>
              </Show>
            </>
          ),
        },
      ]}
    >
      {(active) => (
        <Switch>
          <Match when={active === "params"}>
            <ParamsPanel request={props.request} />
          </Match>
          <Match when={active === "headers"}>
            <HeadersPanel request={props.request} preview={props.preview} />
          </Match>
          <Match when={active === "auth"}>
            <AuthPanel request={props.request} readOnly={props.readOnly} />
          </Match>
          <Match when={active === "body"}>
            <BodyPanel request={props.request} />
          </Match>
          <Match when={active === "settings"}>
            <SettingsPanel request={props.request} />
          </Match>
          <Match when={active === "vars"}>
            <VarsPanel request={props.request} readOnly={props.readOnly} />
          </Match>
          <Match when={active === "tests"}>
            <TestsPanel request={props.request} readOnly={props.readOnly} />
          </Match>
          <Match when={active === "docs"}>
            <DocsPanel request={props.request} readOnly={props.readOnly} />
          </Match>
        </Switch>
      )}
    </Tabs>
  );
}
