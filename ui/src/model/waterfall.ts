/**
 * Timing waterfall maths (PLAN.md Phase 5): turns per-hop timings into bars on one
 * shared time scale. Phases are durations (see `ironcall-http::Timings`); they are laid
 * end to end within each hop, and hops follow each other.
 */

import type { InformationalResponse, TimingPhase, TimingsInfo } from "../ipc/bindings";

export type PhaseKey = "dns" | "connect" | "tls" | "ttfb" | "download";

export const PHASE_LABEL: Record<PhaseKey, string> = {
  dns: "DNS",
  connect: "Connect",
  tls: "TLS",
  ttfb: "Waiting (TTFB)",
  download: "Download",
};

export interface Segment {
  phase: PhaseKey;
  /** Start and width as fractions of the scale (0..1). */
  start: number;
  width: number;
  /** Milliseconds, or `null` for reused / not applicable phases. */
  ms: number | null;
  state: "measured" | "reused" | "notApplicable";
}

export interface WaterfallRow {
  label: string;
  /** Status code of the hop (redirect status, or the final status). */
  status: number;
  url: string;
  segments: Segment[];
  /** This hop's own total, in ms. */
  totalMs: number;
}

export interface Marker {
  /** Position as a fraction of the scale. */
  at: number;
  label: string;
}

export interface Waterfall {
  rows: WaterfallRow[];
  /** Length of the scale in ms (the whole send). */
  scaleMs: number;
  markers: Marker[];
  /** Gridline positions (fractions) and their labels, e.g. every 10/50/100 ms. */
  ticks: { at: number; label: string }[];
}

export interface Hop {
  status: number;
  url: string;
  timings: TimingsInfo;
}

// Durations are `number | null` in the bindings (f64 NaN serialises as null).
const n = (v: number | null | undefined) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const phaseMs = (p: TimingPhase) => (p.kind === "measured" ? n(p.ms) : 0);
const phaseState = (p: TimingPhase): Segment["state"] =>
  p.kind === "measured" ? "measured" : p.kind === "reused" ? "reused" : "notApplicable";

/** Sum of a hop's phases (its bar length); may differ slightly from `totalMs`. */
export function hopSpan(t: TimingsInfo): number {
  return phaseMs(t.dns) + phaseMs(t.connect) + phaseMs(t.tls) + n(t.ttfbMs) + n(t.downloadMs);
}

/** A "nice" tick step so the scale gets about 4–8 gridlines. */
export function tickStep(scaleMs: number): number {
  if (scaleMs <= 0) return 1;
  const raw = scaleMs / 6;
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 5, 10]) {
    if (raw <= m * magnitude) return m * magnitude;
  }
  return 10 * magnitude;
}

/**
 * Builds the waterfall for redirect hops followed by the final response. `finalTotalMs`
 * is the whole send (it includes the redirects); 1xx responses are placed relative to
 * the start of the final hop's wait.
 */
export function buildWaterfall(
  redirects: Hop[],
  final: Hop,
  finalTotalMs: number | null,
  informational: InformationalResponse[] = [],
): Waterfall {
  const hops = [...redirects, final];
  const spans = hops.map((h) => hopSpan(h.timings));
  const sum = spans.reduce((a, b) => a + b, 0);
  const scaleMs = Math.max(sum, n(finalTotalMs), 0.001);
  const frac = (ms: number) => ms / scaleMs;

  let offset = 0;
  let finalWaitStart = 0;
  const rows = hops.map((hop, i) => {
    const t = hop.timings;
    const parts: [PhaseKey, TimingPhase | number][] = [
      ["dns", t.dns],
      ["connect", t.connect],
      ["tls", t.tls],
      ["ttfb", n(t.ttfbMs)],
      ["download", n(t.downloadMs)],
    ];
    let at = offset;
    const segments = parts.map(([phase, value]): Segment => {
      const isNumber = typeof value === "number";
      const ms = isNumber ? value : phaseMs(value);
      const state = isNumber ? "measured" : phaseState(value);
      if (phase === "ttfb" && i === hops.length - 1) finalWaitStart = at;
      const seg = {
        phase,
        start: frac(at),
        width: frac(ms),
        ms: state === "measured" ? ms : null,
        state,
      };
      at += ms;
      return seg;
    });
    offset += spans[i] ?? 0;
    return {
      label: i < redirects.length ? `Redirect ${i + 1}` : "Response",
      status: hop.status,
      url: hop.url,
      segments,
      totalMs: n(t.totalMs),
    };
  });

  const markers = informational
    .filter((r) => r.atMs !== null)
    .map((r) => ({
      at: Math.min(1, frac(finalWaitStart + (r.atMs ?? 0))),
      label: `${r.status}`,
    }));

  const step = tickStep(scaleMs);
  const ticks: Waterfall["ticks"] = [];
  for (let ms = 0; ms <= scaleMs + 1e-9; ms += step) {
    ticks.push({ at: frac(ms), label: ms >= 1000 ? `${ms / 1000}s` : `${ms}ms` });
    if (ticks.length > 20) break;
  }

  return { rows, scaleMs, markers, ticks };
}
