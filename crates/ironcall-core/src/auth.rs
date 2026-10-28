//! Auth inheritance (PLAN.md Phase 4).
//!
//! A request's `auth` is `inherit` by default. Inheritance walks up from the innermost
//! folder to the collection; the first non-`inherit` auth wins. `inherit` all the way up
//! (including at the collection) means no auth.

use crate::ItemId;
use crate::model::{Auth, Extra};

/// Where an effective auth comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthOrigin {
    /// Set on the request itself.
    Request,
    Folder {
        id: ItemId,
        name: String,
    },
    Collection,
    /// Nothing set anywhere: no auth.
    None,
}

impl AuthOrigin {
    /// Short label for previews, e.g. `Folder › Users`. `None` for request-level auth.
    pub fn label(&self) -> Option<String> {
        match self {
            AuthOrigin::Request | AuthOrigin::None => None,
            AuthOrigin::Folder { name, .. } => Some(format!("Folder › {name}")),
            AuthOrigin::Collection => Some("Collection".to_string()),
        }
    }
}

/// One folder's auth in a request's folder chain.
#[derive(Clone, Copy, Debug)]
pub struct FolderAuth<'a> {
    pub id: ItemId,
    pub name: &'a str,
    pub auth: &'a Auth,
}

/// The auth a request uses after inheritance. `folders` is the request's folder chain,
/// outermost first. The returned auth is never [`Auth::Inherit`].
pub fn effective_auth(request: &Auth, folders: &[FolderAuth<'_>], collection: &Auth) -> (Auth, AuthOrigin) {
    if !request.is_inherit() {
        return (request.clone(), AuthOrigin::Request);
    }
    if let Some(folder) = folders.iter().rev().find(|f| !f.auth.is_inherit()) {
        return (
            folder.auth.clone(),
            AuthOrigin::Folder {
                id: folder.id,
                name: folder.name.to_string(),
            },
        );
    }
    if !collection.is_inherit() {
        return (collection.clone(), AuthOrigin::Collection);
    }
    (Auth::None { extra: Extra::new() }, AuthOrigin::None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bearer(token: &str) -> Auth {
        Auth::Bearer {
            token: token.into(),
            extra: Extra::new(),
        }
    }

    fn none() -> Auth {
        Auth::None { extra: Extra::new() }
    }

    #[test]
    fn request_auth_wins() {
        let outer = ItemId::new();
        let folders = [FolderAuth {
            id: outer,
            name: "Outer",
            auth: &bearer("folder"),
        }];
        assert_eq!(
            effective_auth(&bearer("req"), &folders, &bearer("col")),
            (bearer("req"), AuthOrigin::Request)
        );
        // An explicit `none` on the request is not inheritance.
        assert_eq!(
            effective_auth(&none(), &folders, &bearer("col")),
            (none(), AuthOrigin::Request)
        );
    }

    #[test]
    fn innermost_non_inherit_folder_wins() {
        let (outer, inner) = (ItemId::new(), ItemId::new());
        let outer_auth = bearer("outer");
        let folders = [
            FolderAuth {
                id: outer,
                name: "Outer",
                auth: &outer_auth,
            },
            FolderAuth {
                id: inner,
                name: "Inner",
                auth: &Auth::Inherit,
            },
        ];
        assert_eq!(
            effective_auth(&Auth::Inherit, &folders, &bearer("col")),
            (
                bearer("outer"),
                AuthOrigin::Folder {
                    id: outer,
                    name: "Outer".into()
                }
            )
        );

        let inner_auth = bearer("inner");
        let folders = [
            folders[0],
            FolderAuth {
                auth: &inner_auth,
                ..folders[1]
            },
        ];
        let (auth, origin) = effective_auth(&Auth::Inherit, &folders, &bearer("col"));
        assert_eq!(auth, bearer("inner"));
        assert_eq!(origin.label().as_deref(), Some("Folder › Inner"));
    }

    #[test]
    fn falls_back_to_collection_then_none() {
        let folders = [FolderAuth {
            id: ItemId::new(),
            name: "F",
            auth: &Auth::Inherit,
        }];
        assert_eq!(
            effective_auth(&Auth::Inherit, &folders, &bearer("col")),
            (bearer("col"), AuthOrigin::Collection)
        );
        assert_eq!(
            effective_auth(&Auth::Inherit, &folders, &Auth::Inherit),
            (none(), AuthOrigin::None)
        );
        assert_eq!(
            effective_auth(&Auth::Inherit, &[], &Auth::Inherit),
            (none(), AuthOrigin::None)
        );
        // A folder with explicit `none` stops inheritance.
        let none_auth = none();
        let folders = [FolderAuth {
            id: ItemId::new(),
            name: "Public",
            auth: &none_auth,
        }];
        assert_eq!(effective_auth(&Auth::Inherit, &folders, &bearer("col")).0, none());
    }
}
