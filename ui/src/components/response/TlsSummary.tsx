import { For, Show } from "solid-js";
import type { TlsInfo } from "../../ipc/bindings";
import { expiryStatus, expiryText } from "../../model/tls";
import styles from "./tls.module.css";

const date = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : "unknown");

/** TLS details of the final connection (PLAN.md Phase 5). */
export function TlsSummary(props: { tls: TlsInfo; now?: Date }) {
  const cert = () => props.tls.certificate;
  const expiry = () => {
    const c = cert();
    return c ? expiryStatus(c, props.now) : undefined;
  };
  return (
    <div class={styles.tls}>
      <Show when={!props.tls.verified}>
        <p class={styles.danger} role="alert">
          ⚠ Certificate verification was turned off for this request. The connection is encrypted,
          but the server's identity was not checked.
        </p>
      </Show>
      <Show
        when={
          expiry()?.level === "warning" ||
          expiry()?.level === "expired" ||
          expiry()?.level === "notYetValid"
        }
      >
        <p
          class={expiry()?.level === "warning" ? styles.warning : styles.danger}
          role="alert"
          data-testid="tls-expiry"
        >
          ⚠ The server's certificate{" "}
          {expiryText(expiry() as NonNullable<ReturnType<typeof expiry>>)}.
        </p>
      </Show>
      <dl class={styles.list}>
        <dt>Protocol</dt>
        <dd>{props.tls.version}</dd>
        <dt>Cipher</dt>
        <dd>
          <code>{props.tls.cipher}</code>
        </dd>
        <dt>ALPN</dt>
        <dd>{props.tls.alpn ?? "none"}</dd>
        <dt>Verified</dt>
        <dd>{props.tls.verified ? "yes" : "no (verification off)"}</dd>
      </dl>
      <Show when={cert()} fallback={<p class={styles.muted}>No certificate details.</p>}>
        {(c) => (
          <>
            <h4 class={styles.heading}>Server certificate</h4>
            <dl class={styles.list}>
              <dt>Subject</dt>
              <dd>
                <code>{c().subject}</code>
              </dd>
              <dt>Issuer</dt>
              <dd>
                <code>{c().issuer}</code>
              </dd>
              <dt>Valid from</dt>
              <dd>{date(c().notBefore)}</dd>
              <dt>Valid until</dt>
              <dd>
                {date(c().notAfter)}
                <Show when={expiry()}>
                  {(e) => <span class={styles.muted}> ({expiryText(e())})</span>}
                </Show>
              </dd>
              <dt>Names</dt>
              <dd>
                <Show when={c().sans.length > 0} fallback="none">
                  <ul class={styles.sans}>
                    <For each={c().sans}>{(san) => <li>{san}</li>}</For>
                  </ul>
                </Show>
              </dd>
              <dt>Chain</dt>
              <dd>
                {c().chainLength} certificate{c().chainLength === 1 ? "" : "s"} sent by the server
              </dd>
            </dl>
          </>
        )}
      </Show>
    </div>
  );
}
