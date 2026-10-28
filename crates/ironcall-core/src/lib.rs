//! Core domain logic for IronCall: model types, variable resolution, redaction,
//! assertions and the collection runner.
//!
//! This crate must never depend on Tauri, so a future `ironcall-cli` can reuse it.

pub mod assertions;
pub mod auth;
pub mod checks;
pub mod content;
pub mod extract;
pub mod id;
pub mod logging;
pub mod model;
pub mod paths;
pub mod perms;
pub mod recent;
pub mod redact;
pub mod response;
pub mod runner;
pub mod runtime;
pub mod url;
pub mod vars;
pub mod wire;

pub use id::ItemId;
