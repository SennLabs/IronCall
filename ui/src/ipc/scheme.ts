/**
 * URLs for IronCall's custom URI scheme (PLAN.md §3.1).
 *
 * Tauri serves custom schemes as `http://<scheme>.localhost/...` on Windows (WebView2)
 * and `<scheme>://localhost/...` on Linux and macOS (WebKit). Always build URLs here.
 */

export type Platform = "windows" | "other";

export function detectPlatform(userAgent: string = navigator.userAgent): Platform {
  return /Windows/i.test(userAgent) ? "windows" : "other";
}

export function schemeUrl(
  path: string,
  platform: Platform = detectPlatform(),
  scheme = "ironcall",
): string {
  const clean = path.replace(/^\/+/, "");
  return platform === "windows"
    ? `http://${scheme}.localhost/${clean}`
    : `${scheme}://localhost/${clean}`;
}

/** URL of a body served with its real `Content-Type` (Range supported). */
export function bodyUrl(bodyId: number, platform: Platform = detectPlatform()): string {
  return schemeUrl(`body/${bodyId}`, platform);
}

/**
 * URL of an HTML body on the isolated preview origin, which serves it with
 * `Content-Security-Policy: default-src 'none'; …`, so nothing loads from the network.
 */
export function previewUrl(bodyId: number, platform: Platform = detectPlatform()): string {
  return schemeUrl(`body/${bodyId}`, platform, "ironcall-preview");
}
