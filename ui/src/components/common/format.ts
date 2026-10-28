/** Display formatting helpers. */

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export function formatMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || Number.isNaN(ms)) return "–";
  if (ms < 1) return `${ms.toFixed(2)} ms`;
  if (ms < 1000) return `${ms < 10 ? ms.toFixed(1) : Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

/** Status colour class: 1xx/2xx/3xx/4xx/5xx. */
export function statusClass(status: number): "info" | "success" | "redirect" | "client" | "server" {
  if (status < 200) return "info";
  if (status < 300) return "success";
  if (status < 400) return "redirect";
  if (status < 500) return "client";
  return "server";
}
