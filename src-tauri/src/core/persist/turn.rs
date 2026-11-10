//! The end of a turn, as the persistence layer sees it: save the conversation, keep the store bounded, and remember the
//! conversation as the last one used. Also the read that resumes a saved conversation. Nothing here depends on Tauri
//! or the agent loop: the caller passes the history snapshot, the agent's redactor and the environment.

use std::path::Path;

use serde_json::Value;

use super::conversations::{ConversationMeta, SaveRequest};
use super::ui_state::UiStatePatch;
use super::workspace::{Workspace, workspace_for};
use super::{Persist, PersistError};
use crate::core::chat::history::History;
use crate::core::chat::redact::Redactor;
use crate::core::chat::todo::{self, Todo};

/// Conversations kept on disk. After each saved turn the oldest beyond this are deleted, never the one just saved.
pub const MAX_CONVERSATIONS: usize = 500;

/// What a finished turn hands over: the conversation, its model-facing history, and where the shell was.
pub struct FinishedTurn<'a> {
    pub conversation_id: &'a str,
    /// The History snapshot (no system message). Redacted when it is saved.
    pub messages: Vec<Value>,
    /// The conversation's todo list at the end of the turn. Redacted when it is saved.
    pub todos: Vec<Todo>,
    pub model: Option<String>,
    /// The shell's working directory as `(host, path)`. None before the shell has reported one.
    pub cwd: Option<(&'a str, &'a str)>,
    /// True for a checkpoint inside the turn, false for the save at its end (see `StoredConversation::in_turn`).
    pub in_turn: bool,
}

/// This machine, as the workspace needs it: the host name the shell reports, and the home directory.
pub struct TurnEnv<'a> {
    pub local_host: &'a str,
    pub home: Option<&'a Path>,
}

/// A saved conversation, ready to become the live one.
pub struct Resumed {
    pub id: String,
    pub title: String,
    /// The stored messages, repaired (see `History::from_messages`). This is what the live conversation then holds.
    pub history: History,
    /// The stored todo list, repaired (see `todo::repair`).
    pub todos: Vec<Todo>,
    /// The file was saved at a checkpoint inside a turn, so that turn never finished (the app ended first).
    pub in_turn: bool,
}

/// Saves a finished turn. Returns `None`, and writes nothing, when the history has no user message. Otherwise it
/// saves the conversation, prunes the store (the newest `MAX_CONVERSATIONS` are kept, and this one always), and
/// records the id as the last conversation in the UI state. Every step is attempted; the first error is returned.
pub fn save_turn(
    persist: &Persist,
    turn: FinishedTurn<'_>,
    redactor: &Redactor,
    env: TurnEnv<'_>,
    now: u64,
) -> Result<Option<ConversationMeta>, PersistError> {
    save_turn_keeping(persist, turn, redactor, env, now, MAX_CONVERSATIONS)
}

fn save_turn_keeping(
    persist: &Persist,
    turn: FinishedTurn<'_>,
    redactor: &Redactor,
    env: TurnEnv<'_>,
    now: u64,
    keep: usize,
) -> Result<Option<ConversationMeta>, PersistError> {
    if !has_user_message(&turn.messages) {
        return Ok(None);
    }
    let id = turn.conversation_id.to_string();
    // A failed read of the index leaves no known workspaces, which only makes the root choice less careful.
    let known: Vec<Workspace> =
        persist.conversations.list().unwrap_or_default().into_iter().map(|m| m.workspace).collect();
    let req = SaveRequest {
        id: id.clone(),
        workspace: workspace_for(turn.cwd, env.local_host, env.home, &known),
        model: turn.model,
        messages: turn.messages,
        todos: turn.todos,
        in_turn: turn.in_turn,
    };
    let meta = persist.conversations.save(&req, redactor, now)?;
    let pruned = persist.conversations.prune(keep, Some(&id)).map(|_| ());
    let patch = UiStatePatch { last_conversation_id: Some(Some(id)), ..UiStatePatch::default() };
    let remembered = persist.ui.patch(patch).map(|_| ());
    pruned?;
    remembered?;
    Ok(Some(meta))
}

/// Reads a saved conversation for resuming. A missing, unreadable or invalid file is an error, and nothing is changed.
pub fn resume(persist: &Persist, id: &str) -> Result<Resumed, PersistError> {
    let stored = persist.conversations.load(id)?;
    Ok(Resumed {
        id: stored.id,
        title: stored.title,
        history: History::from_messages(stored.messages),
        todos: todo::repair(stored.todos),
        in_turn: stored.in_turn,
    })
}

fn has_user_message(messages: &[Value]) -> bool {
    messages.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("user"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    const SECRET: &str = "known-secret-value-123";

    fn persist(tmp: &TempDir) -> Persist {
        Persist::new(&tmp.path().join("config"), &tmp.path().join("data"))
    }

    fn id(n: u32) -> String {
        format!("ses_{n:032x}")
    }

    fn redactor() -> Redactor {
        Redactor::new(vec![SECRET.to_string()])
    }

    fn env(home: Option<&Path>) -> TurnEnv<'_> {
        TurnEnv { local_host: "this-mac", home }
    }

    fn turn<'a>(id: &'a str, messages: Vec<Value>, cwd: Option<(&'a str, &'a str)>) -> FinishedTurn<'a> {
        FinishedTurn {
            conversation_id: id,
            messages,
            todos: Vec::new(),
            model: Some("model-a".into()),
            cwd,
            in_turn: false,
        }
    }

    fn exchange(user_text: &str) -> Vec<Value> {
        vec![json!({"role": "user", "content": user_text}), json!({"role": "assistant", "content": "reply"})]
    }

    /// Every file under `dir`, concatenated. Empty when the directory does not exist.
    fn all_text(dir: &Path) -> String {
        let Ok(entries) = fs::read_dir(dir) else { return String::new() };
        let mut out = String::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.push_str(&all_text(&path));
            } else if let Ok(text) = fs::read_to_string(&path) {
                out.push_str(&text);
            }
        }
        out
    }

    #[test]
    fn a_turn_without_a_user_message_saves_nothing() {
        let tmp = TempDir::new();
        let p = persist(&tmp);
        let empty = save_turn(&p, turn(&id(1), Vec::new(), None), &redactor(), env(None), 5).unwrap();
        assert!(empty.is_none());
        let assistant_only = vec![json!({"role": "assistant", "content": "hi"})];
        let none = save_turn(&p, turn(&id(1), assistant_only, None), &redactor(), env(None), 5).unwrap();
        assert!(none.is_none());
        assert!(!tmp.path().join("data").exists(), "no conversation directory");
        assert!(!tmp.path().join("config").exists(), "no ui-state file");
    }

    #[test]
    fn a_saved_turn_is_redacted_recorded_and_remembered_as_the_last_conversation() {
        let tmp = TempDir::new();
        let p = persist(&tmp);
        let messages = vec![
            json!({"role": "user", "content": format!("use {SECRET} now")}),
            json!({"role": "assistant", "content": format!("ok {SECRET}")}),
        ];
        let meta = save_turn(&p, turn(&id(2), messages, None), &redactor(), env(None), 77).unwrap().expect("saved");
        assert_eq!(meta.id, id(2));
        assert_eq!(meta.created_at, 77);
        assert_eq!(meta.title, "use [REDACTED] now");
        assert_eq!(meta.model.as_deref(), Some("model-a"));
        assert_eq!(meta.workspace.label, "Unknown", "no cwd yet");

        let stored = p.conversations.load(&id(2)).unwrap();
        assert_eq!(stored.messages[1]["content"], "ok [REDACTED]");
        assert_eq!(p.ui.load().unwrap().last_conversation_id, Some(id(2)));
        assert!(!all_text(tmp.path()).contains(SECRET), "the secret is on disk somewhere");
    }

    #[test]
    fn the_workspace_is_the_project_root_of_the_working_directory() {
        let tmp = TempDir::new();
        let p = persist(&tmp);
        let repo = tmp.path().join("repo");
        let cwd = repo.join("src");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let cwd_text = cwd.to_string_lossy().into_owned();
        let meta = save_turn(&p, turn(&id(3), exchange("hi"), Some(("", &cwd_text))), &redactor(), env(None), 1)
            .unwrap()
            .unwrap();
        assert_eq!(meta.workspace.root, repo.to_string_lossy());
        assert_eq!(meta.workspace.label, "repo");
        assert_eq!(meta.workspace.host, "");
    }

    #[test]
    fn the_store_is_pruned_to_the_newest_and_never_drops_the_saved_conversation() {
        let tmp = TempDir::new();
        let p = persist(&tmp);
        for n in 10..13 {
            save_turn_keeping(&p, turn(&id(n), exchange("x"), None), &redactor(), env(None), u64::from(n), 2).unwrap();
        }
        // id 10 is the oldest and beyond the two newest: it is gone. The others stay.
        let mut left: Vec<String> = p.conversations.list().unwrap().into_iter().map(|m| m.id).collect();
        left.sort();
        assert_eq!(left, vec![id(11), id(12)]);
        assert_eq!(p.ui.load().unwrap().last_conversation_id, Some(id(12)));

        // A conversation saved with an older time than the newest one is kept when it is the one just saved.
        save_turn_keeping(&p, turn(&id(20), exchange("y"), None), &redactor(), env(None), 1, 1).unwrap();
        let mut left: Vec<String> = p.conversations.list().unwrap().into_iter().map(|m| m.id).collect();
        left.sort();
        assert!(left.contains(&id(20)), "the saved conversation survives its own prune: {left:?}");
    }

    #[test]
    fn resume_repairs_the_stored_messages_and_reports_missing_or_damaged_files() {
        let tmp = TempDir::new();
        let p = persist(&tmp);
        let call = json!({"id": "c1", "type": "function", "function": {"name": "run_command", "arguments": "{}"}});
        let messages = vec![
            json!({"role": "user", "content": "run it"}),
            json!({"role": "assistant", "content": null, "tool_calls": [call]}),
        ];
        save_turn(&p, turn(&id(30), messages, None), &redactor(), env(None), 1).unwrap();

        let resumed = resume(&p, &id(30)).unwrap();
        assert_eq!(resumed.id, id(30));
        assert_eq!(resumed.title, "run it");
        let snapshot = resumed.history.snapshot();
        assert_eq!(snapshot.len(), 3, "the unanswered call gets a closing tool message");
        assert_eq!(snapshot[2]["role"], "tool");
        assert_eq!(snapshot[2]["tool_call_id"], "c1");
        assert_eq!(snapshot[2]["content"], "skipped: not recorded");

        assert!(!resumed.in_turn, "a turn that finished resumes as finished");
        let id30 = id(30);
        let mid = FinishedTurn { in_turn: true, ..turn(&id30, exchange("again"), None) };
        save_turn(&p, mid, &redactor(), env(None), 2).unwrap();
        assert!(resume(&p, &id30).unwrap().in_turn, "a checkpoint resumes as interrupted");

        assert!(matches!(resume(&p, &id(31)), Err(PersistError::NotFound(_))));
        let damaged = tmp.path().join("data").join("conversations").join(format!("{}.json", id(32)));
        fs::create_dir_all(damaged.parent().unwrap()).unwrap();
        fs::write(&damaged, b"{ not a conversation").unwrap();
        assert!(matches!(resume(&p, &id(32)), Err(PersistError::Corrupt(_))));
    }
}
