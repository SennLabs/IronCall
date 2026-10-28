/** State of the current send: progress, response or error, plus the recent-sends list. */

import { type Accessor, createSignal, onCleanup } from "solid-js";
import { api, newSendId, type RequestDraft, SendFailure, sendError } from "../ipc/api";
import type { RecentSend, ResponseSummary, SendError, SendStage } from "../ipc/bindings";

export type SendState =
  | { status: "idle" }
  | {
      status: "sending";
      sendId: string;
      stage: SendStage;
      /** Response bytes received. */
      bytes: number;
      /** Request body bytes sent, and the total if known (file and multipart bodies). */
      uploaded: number;
      uploadTotal: number | null;
    }
  | { status: "done"; response: ResponseSummary }
  | { status: "error"; error: SendError };

export interface SendStore {
  state: Accessor<SendState>;
  recent: Accessor<RecentSend[]>;
  isSending: () => boolean;
  send(request: RequestDraft): Promise<void>;
  cancel(): Promise<void>;
  refreshRecent(): Promise<void>;
}

function toSendError(e: unknown): SendError {
  if (e instanceof SendFailure) return e.error;
  return sendError("internal", e instanceof Error ? e.message : String(e));
}

export function createSendStore(): SendStore {
  const [state, setState] = createSignal<SendState>({ status: "idle" });
  const [recent, setRecent] = createSignal<RecentSend[]>([]);

  let unlisten: (() => void) | undefined;
  let disposed = false;
  void api
    .onSendProgress((p) => {
      const s = state();
      if (s.status === "sending" && s.sendId === p.sendId) {
        setState({
          ...s,
          stage: p.stage,
          bytes: p.bytes,
          uploaded: p.uploaded,
          uploadTotal: p.uploadTotal ?? s.uploadTotal,
        });
      }
    })
    .then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
  onCleanup(() => {
    disposed = true;
    unlisten?.();
  });

  const refreshRecent = async () => {
    try {
      setRecent(await api.recentSends());
    } catch {
      // Not critical; keep the old list.
    }
  };

  const releaseBody = () => {
    const s = state();
    const bodyId = s.status === "done" ? s.response.body.bodyId : null;
    if (bodyId !== null) void api.closeBody(bodyId).catch(() => {});
  };

  return {
    state,
    recent,
    isSending: () => state().status === "sending",
    async send(request) {
      if (state().status === "sending") return;
      releaseBody();
      const sendId = newSendId();
      setState({
        status: "sending",
        sendId,
        stage: "connecting",
        bytes: 0,
        uploaded: 0,
        uploadTotal: null,
      });
      try {
        const response = await api.sendRequest(sendId, request);
        setState({ status: "done", response });
      } catch (e) {
        setState({ status: "error", error: toSendError(e) });
      }
      await refreshRecent();
    },
    async cancel() {
      const s = state();
      if (s.status === "sending") await api.cancelSend(s.sendId);
    },
    refreshRecent,
  };
}
