import { createEffect, createResource, createSignal, on, onCleanup, Show } from "solid-js";
import { bodyUrl } from "../../ipc/scheme";
import styles from "./preview.module.css";

/** Bytes per Range request; the scheme refuses whole bodies over 32 MB. */
const CHUNK = 8 * 1024 * 1024;

/** Fetches a body's bytes from the `ironcall` scheme in Range chunks. */
export async function fetchBodyBytes(bodyId: number, size: number): Promise<Uint8Array> {
  const out = new Uint8Array(size);
  for (let start = 0; start < size; start += CHUNK) {
    const end = Math.min(size, start + CHUNK) - 1;
    const res = await fetch(bodyUrl(bodyId), { headers: { Range: `bytes=${start}-${end}` } });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    out.set(new Uint8Array(await res.arrayBuffer()), start);
  }
  return out;
}

// pdf.js is large: load it (and its locally bundled worker) only when a PDF is shown.
async function loadPdfjs() {
  const [pdfjs, worker] = await Promise.all([
    import("pdfjs-dist"),
    import("pdfjs-dist/build/pdf.worker.min.mjs?url"),
  ]);
  pdfjs.GlobalWorkerOptions.workerSrc = worker.default;
  return pdfjs;
}

/** PDF preview with page navigation and zoom (PLAN.md Phase 5). */
export function PdfView(props: { bodyId: number; size: number }) {
  const [page, setPage] = createSignal(1);
  const [zoom, setZoom] = createSignal(1);
  let canvas!: HTMLCanvasElement;
  let task: { destroy(): Promise<void> } | undefined;

  const [doc] = createResource(
    () => ({ id: props.bodyId, size: props.size }),
    async ({ id, size }) => {
      const pdfjs = await loadPdfjs();
      const data = await fetchBodyBytes(id, size);
      const loading = pdfjs.getDocument({ data });
      task = loading;
      return loading.promise;
    },
  );
  onCleanup(() => void task?.destroy());

  createEffect(
    on([doc, page, zoom], async ([d, p, z]) => {
      if (!d) return;
      const pdfPage = await d.getPage(p);
      const viewport = pdfPage.getViewport({ scale: z * (window.devicePixelRatio || 1) });
      canvas.width = viewport.width;
      canvas.height = viewport.height;
      canvas.style.width = `${viewport.width / (window.devicePixelRatio || 1)}px`;
      await pdfPage.render({ canvas, viewport }).promise;
    }),
  );

  const pages = () => doc()?.numPages ?? 0;
  return (
    <div class={styles.preview}>
      <div class={styles.toolbar}>
        <button
          type="button"
          onClick={() => setPage((p) => Math.max(1, p - 1))}
          disabled={page() <= 1}
          aria-label="Previous page"
        >
          ‹
        </button>
        <span class={styles.muted}>
          Page {page()} of {pages() || "…"}
        </span>
        <button
          type="button"
          onClick={() => setPage((p) => Math.min(pages(), p + 1))}
          disabled={page() >= pages()}
          aria-label="Next page"
        >
          ›
        </button>
        <span class={styles.spacer} />
        <button
          type="button"
          onClick={() => setZoom((z) => Math.max(0.25, z / 1.25))}
          aria-label="Zoom out"
        >
          −
        </button>
        <span class={styles.muted}>{Math.round(zoom() * 100)}%</span>
        <button
          type="button"
          onClick={() => setZoom((z) => Math.min(4, z * 1.25))}
          aria-label="Zoom in"
        >
          +
        </button>
      </div>
      <Show
        when={!doc.error}
        fallback={<p class={styles.muted}>Couldn't open the PDF: {String(doc.error)}</p>}
      >
        <Show when={doc()} fallback={<p class={styles.muted}>Loading PDF…</p>}>
          <div class={styles.canvasHost}>
            <canvas ref={canvas} aria-label={`PDF page ${page()}`} />
          </div>
        </Show>
      </Show>
    </div>
  );
}
