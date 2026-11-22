//! The file tools. The read-only ones (`read_file`, `list_dir`, `glob`, `grep`) let the model read this machine's files without
//! going through run_command. Like `skill`, they are ungated: nothing runs and nothing changes. They read only inside the
//! session's project folder (`scope.rs`), never the sensitive paths, and only while the local shell is on top.
//!
//! The write tools (`write_file`, `edit_file`, `write.rs`) change files on this machine. Each call goes through the approval
//! gate (core/agent), which shows `preview_write` on its card and runs `run_write` on approval. They never write .git,
//! secret or Watt's own files, and never outside the project folder.
//!
//! `backend.rs` is the seam a remote backend would plug into (phase 2, not built). `tool.rs` holds the schemas, the
//! argument checks and the dispatch; `local.rs` is the local backend.

pub mod backend;
mod local;
mod scope;
mod tool;
mod write;

pub use scope::Scope;
pub use tool::{
    FS_WRITES_PER_TURN, Place, TOOL_NAMES, WRITE_TOOL_NAMES, is_tool, is_write, preview_write, run, run_write, schemas,
};
