//! MCP servers the user configures: the settings model, the paste-JSON import, the secret values, the names the model
//! sees, and the approval rule. The config modules are pure logic. `manager` is the runtime: it owns the live rmcp
//! clients on a dedicated thread and implements `ExternalToolHost` (core/external_tools). Nothing here touches Tauri.
//!
//! Config lives in `settings.json` (`Settings::mcp`) with names only. Env and header values live in the secret store
//! (`secrets.rs`), so a copy of the settings file never holds them.

pub mod approval;
pub mod client;
pub mod config;
pub mod expand;
pub mod launch;
pub mod logs;
pub mod login_path;
pub mod manager;
pub mod names;
pub mod parse;
pub mod result;
pub mod runtime;
pub mod secrets;
pub mod service;
pub mod tools;
