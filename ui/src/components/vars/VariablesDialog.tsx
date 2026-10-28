import {
  api,
  type CollectionDraft,
  type FolderDraft,
  type SecretScope,
  type VarsDraft,
} from "../../ipc/api";
import { closeDialog, type VariablesTarget } from "../../stores/dialogs";
import type { VarsStore } from "../../stores/vars";
import { createVarsEditor, type VarsSource } from "../../stores/varsEditor";
import { Modal } from "../common/Modal";
import { VarsEditorView } from "./VarsEditorView";

function sourceFor(target: VariablesTarget): {
  source: VarsSource;
  scope: SecretScope;
  title: string;
} {
  if (target.kind === "global") {
    return {
      title: "Global variables",
      scope: { kind: "global" },
      source: {
        load: () => api.globalsLoad(),
        save: (vars, hash) => api.globalsSave(vars, hash),
      },
    };
  }
  if (target.kind === "collection") {
    let file: CollectionDraft | undefined;
    return {
      title: "Collection variables",
      scope: { kind: "collection" },
      source: {
        async load() {
          const c = await api.collectionLoad();
          file = c.collection;
          return { vars: (c.collection.vars as VarsDraft | undefined) ?? {}, hash: c.hash };
        },
        save: (vars, hash) => {
          if (!file) throw new Error("not loaded");
          return api.collectionSave({ ...file, vars }, hash);
        },
      },
    };
  }
  let file: FolderDraft | undefined;
  return {
    title: `Folder variables · ${target.name}`,
    scope: { kind: "folder", id: target.id },
    source: {
      async load() {
        const f = await api.folderLoad(target.id);
        file = f.folder;
        return { vars: (f.folder.vars as VarsDraft | undefined) ?? {}, hash: f.hash };
      },
      save: (vars, hash) => {
        if (!file) throw new Error("not loaded");
        return api.folderSave({ ...file, vars }, hash);
      },
    },
  };
}

/** Variables of the globals (app-wide), the collection (`ironcall.yaml`) or a folder. */
export function VariablesDialog(props: { vars: VarsStore; target: VariablesTarget }) {
  const { source, scope, title } = sourceFor(props.target);
  const editor = createVarsEditor(source, scope, props.vars);
  return (
    <Modal title={title} wide onClose={closeDialog}>
      <VarsEditorView editor={editor} label={title} allowSecrets />
    </Modal>
  );
}
