//! Persistence of agent conversations and UI state as plain JSON files. Nothing here depends on Tauri: every function
//! takes its directories and the current time explicitly, so the tests run against real temp directories.
//!
//! Layout (the directories are chosen by the caller):
//! - `<data_dir>/conversations/<id>.json`: one file per conversation (`conversations.rs`).
//! - `<data_dir>/conversations/index.json`: metadata of every conversation, for listing. It is a cache: when it is
//!   missing, corrupt or disagrees with the files, `list` rebuilds it by reading the conversation files.
//! - `<config_dir>/ui-state.json`: small UI preferences and the last conversation used (`ui_state.rs`).
//!
//! Every write goes through `write_private_atomic` (core/settings): a temp file in the same directory, then rename.
//! Files are 0600 and directories 0700. Conversation messages are the model-facing history, redacted before they
//! are written (see `conversations::redact_messages`). The UI transcript is not stored.

pub mod conversations;
pub mod turn;
pub mod ui_state;
pub mod workspace;

use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use self::conversations::ConversationStore;
use self::ui_state::{UiStateStore, ui_state_path};

/// The two stores. `new` only records the paths: directories are created by the first write.
#[derive(Debug)]
pub struct Persist {
    pub conversations: ConversationStore,
    pub ui: UiStateStore,
}

impl Persist {
    /// Conversations live under `data_dir/conversations`, the UI state in `config_dir/ui-state.json`.
    pub fn new(config_dir: &Path, data_dir: &Path) -> Self {
        Persist {
            conversations: ConversationStore::new(data_dir.join("conversations")),
            ui: UiStateStore::new(ui_state_path(config_dir)),
        }
    }
}

/// Why a persistence operation failed. Messages never include stored message content or secret values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistError {
    /// The request is not acceptable: a bad id, title or patch value. Nothing was written.
    Invalid(String),
    /// The conversation does not exist.
    NotFound(String),
    /// A stored file cannot be read or used. The file is left as it was.
    Corrupt(String),
    /// Reading or writing failed.
    Io(String),
}

impl fmt::Display for PersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PersistError::Invalid(msg) => write!(f, "invalid request: {msg}"),
            PersistError::NotFound(msg) => write!(f, "not found: {msg}"),
            PersistError::Corrupt(msg) => write!(f, "stored data is unusable: {msg}"),
            PersistError::Io(msg) => write!(f, "storage error: {msg}"),
        }
    }
}

impl std::error::Error for PersistError {}

/// Seconds since the Unix epoch. For the callers that run the app; the tests pass their own times.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
