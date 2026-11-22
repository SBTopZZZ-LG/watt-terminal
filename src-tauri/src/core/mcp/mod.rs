//! MCP servers the user configures: the settings model, the paste-JSON import, the secret values, the names the model
//! sees, and the approval rule. Pure logic. Nothing here starts a process, opens a connection or talks to rmcp; the
//! client that does is a later stage, behind `ExternalToolHost` (core/external_tools).
//!
//! Config lives in `settings.json` (`Settings::mcp`) with names only. Env and header values live in the secret store
//! (`secrets.rs`), so a copy of the settings file never holds them.

pub mod approval;
pub mod config;
pub mod expand;
pub mod launch;
pub mod names;
pub mod parse;
pub mod secrets;
