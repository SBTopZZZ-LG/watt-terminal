//! Core logic for agent-term: PTY session, shell integration, observer and command records.
//!
//! Nothing in this module depends on Tauri, so `cargo test` runs against a real PTY without a window.
//! The Tauri layer in `lib.rs` only wires commands and channels to these types.

pub mod agent;
pub mod llm;
pub mod observer;
pub mod pty;
pub mod record;
pub mod registry;
pub mod scanner;
pub mod session;
pub mod shim;
pub mod tools;
