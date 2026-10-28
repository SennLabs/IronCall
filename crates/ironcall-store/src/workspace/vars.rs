//! Helpers for variable resolution and secrets: the folder chain above a request, and
//! every declared secret in the workspace (PLAN.md §5, §6).

use ironcall_core::ItemId;
use ironcall_core::model::Vars;

use super::{NodeKind, Result, Workspace, WorkspaceError};

/// Where a declared secret lives in a workspace (globals are handled by the app).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SecretScopeRef {
    Collection,
    Environment(ItemId),
    Folder(ItemId),
}

/// One folder's variables, for building resolver layers.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderVars {
    pub id: ItemId,
    pub name: String,
    pub vars: Vars,
}

impl Workspace {
    /// Variables declared in `ironcall.yaml`.
    pub fn collection_vars(&self) -> &Vars {
        &self.collection.vars
    }

    /// Folders containing `id` (a request or folder), outermost first, with their
    /// variables. Folders without a readable `_folder.yaml` contribute no variables.
    pub fn folder_chain(&self, id: ItemId) -> Result<Vec<FolderVars>> {
        let mut chain = Vec::new();
        let mut current = self.entry(id)?.parent;
        while let Some(parent) = current {
            let entry = self.entry(parent)?;
            if entry.kind != NodeKind::Folder {
                break;
            }
            let (name, vars) = if entry.meta_error {
                let name = self.tree.find(parent).map(|n| n.name.clone()).unwrap_or_default();
                (name, Vars::default())
            } else {
                let file = self.load_folder(parent)?.file;
                (file.name, file.vars)
            };
            chain.push(FolderVars {
                id: parent,
                name,
                vars,
            });
            current = entry.parent;
        }
        chain.reverse();
        Ok(chain)
    }
}

/// Every variable declared `secret: true` in the collection, the environments and the
/// folders, as `(scope, name)`, sorted. Unreadable files are skipped.
pub fn declared_secrets(ws: &Workspace) -> Vec<(SecretScopeRef, String)> {
    fn secrets(vars: &Vars) -> impl Iterator<Item = &String> {
        vars.iter().filter(|(_, v)| v.is_secret()).map(|(name, _)| name)
    }

    let mut out: Vec<(SecretScopeRef, String)> = secrets(ws.collection_vars())
        .map(|n| (SecretScopeRef::Collection, n.clone()))
        .collect();
    for env in ws.environments() {
        if env.error.is_some() {
            continue;
        }
        if let Ok(vars) = ws.environment_vars(env.id) {
            out.extend(secrets(&vars).map(|n| (SecretScopeRef::Environment(env.id), n.clone())));
        }
    }
    let mut folders: Vec<ItemId> = ws
        .index
        .iter()
        .filter(|(_, e)| e.kind == NodeKind::Folder && e.hash.is_some() && !e.meta_error)
        .map(|(id, _)| *id)
        .collect();
    folders.sort();
    for id in folders {
        match ws.load_folder(id) {
            Ok(loaded) => {
                out.extend(secrets(&loaded.file.vars).map(|n| (SecretScopeRef::Folder(id), n.clone())))
            }
            Err(WorkspaceError::NotFound(_)) => {}
            Err(e) => tracing::debug!(error = %e, "skipping unreadable folder while listing secrets"),
        }
    }
    out.sort();
    out
}
