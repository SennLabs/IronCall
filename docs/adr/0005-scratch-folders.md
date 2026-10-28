# 0005 — Scratch folders are a layout file, not directories

- **Status:** Accepted
- **Date:** 2026-10-07
- **Phase:** post-P8 (user request)

## Context
Scratch requests (PLAN.md §4.6) were a flat list of `scratch/<id>.yaml` files in the
app data dir. Users asked to create folders and requests from a right-click menu in the
Scratch section, as they can in a workspace. Scratch requests autosave by ID every few
hundred milliseconds, and session restore, history reopen and "Save to workspace…" all
find them by ID.

## Options considered
1. **Subdirectories with a `_folder.yaml`, like workspaces.** Matches the workspace
   format. But every save and lookup must first find which directory a request is in,
   moving a request renames files, and a half-finished move can leave duplicates.
2. **A layout file next to the requests (chosen).** `scratch/folders.json` lists the
   folders (ID, name, parent, in display order) and maps request IDs to folder IDs.
   Request files never move.

## Decision
Option 2. Request storage and every existing command are unchanged. The store
normalises the layout on load and save: duplicate folder IDs, unknown parents and
cycles fall back to the top level, and placements into unknown folders or of deleted
requests are dropped. A missing or corrupt layout gives a flat list, like before.
Folder IDs are created in Rust (`scratch_folder_create`); the UI changes names,
nesting and placement by saving the whole layout (`scratch_save_layout`).

## Consequences
- Scratch folders can't be shared or committed; that's what workspaces are for.
- Deleting a folder deletes its scratch requests permanently (after asking), since
  they don't go to the OS trash today either.
- Ordering inside a folder is still most-recently-modified first; folders keep their
  creation order. Manual ordering can come later if it's wanted.
