//! The read-only file tools (`read_file`, `list_dir`, `glob`, `grep`): the model reads this machine's files without going
//! through run_command. Like `skill`, they are ungated: nothing runs and nothing changes. They read only inside the
//! session's project folder (`scope.rs`), never the sensitive paths, and only while the local shell is on top.
//!
//! `backend.rs` is the seam a remote backend would plug into (phase 2, not built). `tool.rs` holds the schemas, the
//! argument checks and the dispatch; `local.rs` is the local backend.

pub mod backend;
mod local;
mod scope;
mod tool;

pub use scope::Scope;
pub use tool::{Place, TOOL_NAMES, is_tool, run, schemas};
