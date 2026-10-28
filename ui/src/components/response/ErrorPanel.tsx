import { Show } from "solid-js";
import type { SendError, SendErrorKind } from "../../ipc/bindings";
import styles from "./response.module.css";
import { Timeline } from "./Timeline";

const INFO: Record<SendErrorKind, { title: string; icon: string; hint?: string }> = {
  invalidRequest: {
    title: "Invalid request",
    icon: "⚠",
    hint: "Check the method, path and query parameters, headers and body.",
  },
  invalidUrl: {
    title: "Invalid URL",
    icon: "⚠",
    hint: "URLs need a scheme and host, e.g. https://api.example.com/path.",
  },
  dns: {
    title: "Host not found",
    icon: "⌕",
    hint: "Check the host name for typos, and that you're on the right network or VPN.",
  },
  connect: {
    title: "Could not connect",
    icon: "⊘",
    hint: "The server refused or didn't answer. Check the port, that the service is running, and any firewall or proxy.",
  },
  tls: {
    title: "TLS / certificate error",
    icon: "⚠",
    hint: "Check the server's certificate. For internal or self-signed certificates, add the CA, or turn off TLS verification for this request in Settings.",
  },
  timeout: {
    title: "Timed out",
    icon: "◷",
    hint: "The server took too long. Increase the timeout in Settings or try again.",
  },
  cancelled: {
    title: "Cancelled",
    icon: "■",
    hint: "The request was cancelled before it finished.",
  },
  tooManyRedirects: {
    title: "Too many redirects",
    icon: "↻",
    hint: "The server kept redirecting. Raise the limit in Settings, or turn off following redirects to inspect them.",
  },
  protocol: {
    title: "Protocol error",
    icon: "⚠",
    hint: "The server sent a response IronCall couldn't parse.",
  },
  io: { title: "Read/write error", icon: "⚠" },
  unsupported: { title: "Not supported yet", icon: "ℹ" },
  internal: { title: "Something went wrong", icon: "⚠", hint: "This is a bug in IronCall." },
};

export function ErrorPanel(props: { error: SendError }) {
  const info = () => INFO[props.error.kind] ?? INFO.internal;
  return (
    <div
      class={styles.error}
      classList={{ [styles.errorMuted as string]: props.error.kind === "cancelled" }}
      role="alert"
      data-kind={props.error.kind}
    >
      <div class={styles.errorTitle}>
        <span aria-hidden="true">{info().icon}</span> {info().title}
      </div>
      <p class={styles.errorMessage}>{props.error.message}</p>
      <Show when={info().hint}>
        <p class={styles.hint}>{info().hint}</p>
      </Show>
      <Show when={props.error.detail}>
        <details>
          <summary>Details</summary>
          <pre class={styles.detail}>{props.error.detail}</pre>
        </details>
      </Show>
      <Show when={props.error.timeline.length > 0}>
        <details class={styles.errorTimeline} open={props.error.kind !== "cancelled"}>
          <summary>Timeline</summary>
          <div class={styles.errorTimelineBody}>
            <Timeline entries={props.error.timeline} />
          </div>
        </details>
      </Show>
    </div>
  );
}
