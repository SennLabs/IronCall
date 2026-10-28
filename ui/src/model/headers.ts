/** Suggestions for the headers table (PLAN.md Phase 1 "header autocomplete"). */

export const COMMON_HEADERS = [
  "Accept",
  "Accept-Charset",
  "Accept-Encoding",
  "Accept-Language",
  "Authorization",
  "Cache-Control",
  "Connection",
  "Content-Disposition",
  "Content-Encoding",
  "Content-Language",
  "Content-Length",
  "Content-Type",
  "Cookie",
  "DNT",
  "Forwarded",
  "From",
  "Host",
  "If-Match",
  "If-Modified-Since",
  "If-None-Match",
  "If-Range",
  "If-Unmodified-Since",
  "Origin",
  "Pragma",
  "Prefer",
  "Proxy-Authorization",
  "Range",
  "Referer",
  "TE",
  "Upgrade",
  "User-Agent",
  "Via",
  "X-Api-Key",
  "X-Correlation-Id",
  "X-Forwarded-For",
  "X-Forwarded-Host",
  "X-Forwarded-Proto",
  "X-Request-Id",
  "X-Requested-With",
] as const;

export const MIME_TYPES = [
  "application/json",
  "application/xml",
  "application/x-www-form-urlencoded",
  "multipart/form-data",
  "text/plain",
  "text/html",
  "text/xml",
  "text/csv",
  "application/octet-stream",
  "application/pdf",
  "application/graphql",
  "application/ld+json",
  "application/problem+json",
  "application/vnd.api+json",
  "image/png",
  "image/jpeg",
  "*/*",
] as const;

const VALUE_SUGGESTIONS: Record<string, readonly string[]> = {
  accept: MIME_TYPES,
  "content-type": MIME_TYPES,
  "accept-encoding": ["gzip, deflate, br, zstd", "gzip", "identity"],
  "cache-control": ["no-cache", "no-store", "max-age=0"],
  connection: ["keep-alive", "close"],
  authorization: ["Bearer ", "Basic "],
  prefer: ["return=representation", "return=minimal", "respond-async"],
};

/** Value suggestions for a header name (case-insensitive). */
export function valueSuggestions(name: string): readonly string[] {
  return VALUE_SUGGESTIONS[name.trim().toLowerCase()] ?? [];
}
