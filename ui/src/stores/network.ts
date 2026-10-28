/** App-wide network UI state (PLAN.md Phase 4). */

import { createSignal } from "solid-js";

const [cookiesVersion, setCookiesVersion] = createSignal(0);

/** Bumped after every send, so an open cookie list refreshes. */
export { cookiesVersion };

export function bumpCookies() {
  setCookiesVersion((v) => v + 1);
}
