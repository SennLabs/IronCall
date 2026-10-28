//! Workspace storage for IronCall: YAML files, file watching, history (SQLite)
//! and the response body store (PLAN.md §4).

pub mod atomic;
pub mod body;
pub mod globals;
pub mod history;
pub mod path_guard;
pub mod sample;
pub mod scratch;
pub mod session;
pub mod settings;
pub mod slug;
pub mod spool;
pub mod watch;
pub mod workspace;
pub mod yaml;

pub use path_guard::{PathGuard, PathGuardError};
