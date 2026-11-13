//! Core logic for Watt: PTY session, shell integration, observer and command records.
//!
//! Nothing in this module depends on Tauri, so `cargo test` runs against a real PTY without a window.
//! The Tauri layer in `lib.rs` only wires commands and channels to these types.

pub mod agent;
pub mod approval;
pub mod chat;
pub mod env;
pub mod foreground;
pub mod inject;
pub mod keys;
pub mod llm;
pub mod migrate;
pub mod observer;
pub mod persist;
pub mod pty;
pub mod record;
pub mod registry;
pub mod scanner;
pub mod session;
pub mod settings;
pub mod shim;
pub mod term_reply;
pub mod tools;
