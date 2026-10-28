/** The small "Imported from cURL; ignored: …" notice shown on a tab after a paste. */

import { createSignal } from "solid-js";

export interface ImportNotice {
  tabKey: string;
  text: string;
}

const [importNotice, setImportNotice] = createSignal<ImportNotice>();

export { importNotice };

export function showImportNotice(tabKey: string, text: string) {
  setImportNotice({ tabKey, text });
}

export function dismissImportNotice() {
  setImportNotice(undefined);
}
