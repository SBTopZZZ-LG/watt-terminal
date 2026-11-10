//! Saved agent conversations: one JSON file per conversation, plus `index.json` with the metadata of all of them.
//!
//! A conversation file holds the model-facing history (`messages`, the chat-completions messages without the system
//! message), so a resumed conversation continues where it stopped. The messages are redacted with the agent's
//! `Redactor` before they are written (`save` does it; see `redact_messages`). The UI transcript is not stored.
//!
//! The id is the conversation id (`ses_` and 32 lowercase hex digits). It is validated before it becomes a file
//! name, so no id can point outside the directory.
//!
//! `save` does not prune. The caller decides the retention (`prune`) after a save.
//!
//! A damaged file is never overwritten in place: when a conversation is saved over one, its bytes are moved to
//! `<id>.json.corrupt-<now>` first, and the conversation is written afresh. Those aside files are kept for inspection;
//! `list` ignores them and `delete_all` removes them.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::PersistError;
use super::workspace::Workspace;
use crate::core::chat::redact::Redactor;
use crate::core::settings::write_private_atomic;

pub const CONVERSATION_VERSION: u32 = 1;
const INDEX_FILE: &str = "index.json";
const ID_PREFIX: &str = "ses_";
/// Longest title made from a message, in characters, ellipsis included.
const MAX_TITLE_CHARS: usize = 80;
/// Longest title a user can set by renaming, in characters.
const MAX_RENAME_CHARS: usize = 120;
const UNTITLED: &str = "Untitled";

/// The metadata of a conversation: what the list shows. Stored in `index.json` and derived from the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    /// Unix seconds, set on the first save.
    pub created_at: u64,
    /// Unix seconds, set on every save. A rename does not change it.
    pub updated_at: u64,
    /// The user and assistant messages (tool results and the system message are not counted).
    pub message_count: usize,
    pub workspace: Workspace,
    pub model: Option<String>,
}

/// A conversation file. `messages` are the stored History messages, redacted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredConversation {
    pub version: u32,
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub workspace: Workspace,
    pub model: Option<String>,
    pub messages: Vec<Value>,
}

impl StoredConversation {
    pub fn meta(&self) -> ConversationMeta {
        ConversationMeta {
            id: self.id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            message_count: message_count(&self.messages),
            workspace: self.workspace.clone(),
            model: self.model.clone(),
        }
    }
}

/// What the agent hands over at the end of a turn. `save` redacts the messages and fills in the rest.
#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub id: String,
    /// Stored only when the conversation is saved for the first time. A later save keeps the stored workspace.
    pub workspace: Workspace,
    pub model: Option<String>,
    /// The History snapshot: stored messages, no system message.
    pub messages: Vec<Value>,
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    conversations: Vec<ConversationMeta>,
    /// Ids of conversation files that could not be read when the index was last rebuilt. The consistency check
    /// leaves them out, so a damaged file does not force a rescan of every file on every call. Absent in older
    /// indexes, which then have none.
    #[serde(default)]
    skipped: Vec<String>,
}

/// The conversations directory. Every operation holds one lock, so a save and a rename cannot interleave.
#[derive(Debug)]
pub struct ConversationStore {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl ConversationStore {
    /// A store over `dir`. The directory is created on the first write.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), lock: Mutex::new(()) }
    }

    /// Writes the conversation and its index entry. A new id gets `created_at = now` and a title derived from the
    /// messages. An existing id keeps its `created_at`, title and workspace. A damaged existing file is moved aside
    /// first, to `<id>.json.corrupt-<now>` with its bytes unchanged, and the conversation is written afresh.
    pub fn save(&self, req: &SaveRequest, redactor: &Redactor, now: u64) -> Result<ConversationMeta, PersistError> {
        check_id(&req.id)?;
        let _guard = self.guard();
        let messages = redact_messages(&req.messages, redactor);
        let existing = match self.read_stored(&req.id) {
            Ok(existing) => existing,
            Err(PersistError::Corrupt(_)) => {
                self.set_aside(&req.id, now)?;
                None
            }
            Err(e) => return Err(e),
        };
        let stored = match existing {
            Some(old) => StoredConversation {
                version: CONVERSATION_VERSION,
                id: req.id.clone(),
                title: old.title,
                created_at: old.created_at,
                updated_at: now,
                workspace: old.workspace,
                model: req.model.clone().or(old.model),
                messages,
            },
            None => StoredConversation {
                version: CONVERSATION_VERSION,
                id: req.id.clone(),
                title: derive_title(&messages),
                created_at: now,
                updated_at: now,
                workspace: req.workspace.clone(),
                model: req.model.clone(),
                messages,
            },
        };
        self.write_file(&stored)?;
        let meta = stored.meta();
        self.update_index_locked(&[meta.id.as_str()], |metas| {
            metas.retain(|m| m.id != meta.id);
            metas.push(meta.clone());
        })?;
        Ok(meta)
    }

    /// Reads one conversation. `NotFound` when there is no file, `Corrupt` when the file is unreadable or invalid.
    pub fn load(&self, id: &str) -> Result<StoredConversation, PersistError> {
        check_id(id)?;
        let _guard = self.guard();
        self.read_stored(id)?.ok_or_else(|| PersistError::NotFound(format!("conversation {id}")))
    }

    /// Every conversation, most recently updated first. Uses the index when it agrees with the files on disk;
    /// otherwise rebuilds it from the files (skipping any that cannot be read) and rewrites it.
    pub fn list(&self) -> Result<Vec<ConversationMeta>, PersistError> {
        let _guard = self.guard();
        Ok(self.index_locked()?.conversations)
    }

    /// Deletes one conversation and its index entry. Deleting an id that does not exist is not an error.
    pub fn delete(&self, id: &str) -> Result<(), PersistError> {
        check_id(id)?;
        let _guard = self.guard();
        remove_if_exists(&self.path_for(id))?;
        self.update_index_locked(&[id], |metas| metas.retain(|m| m.id != id))
    }

    /// Sets the title. The title is trimmed and must be 1 to 120 characters with no control characters. The
    /// `updated_at` time is kept.
    pub fn rename(&self, id: &str, title: &str) -> Result<ConversationMeta, PersistError> {
        check_id(id)?;
        let title = normalize_title(title)?;
        let _guard = self.guard();
        let mut stored =
            self.read_stored(id)?.ok_or_else(|| PersistError::NotFound(format!("conversation {id}")))?;
        stored.title = title;
        self.write_file(&stored)?;
        let meta = stored.meta();
        self.update_index_locked(&[meta.id.as_str()], |metas| {
            metas.retain(|m| m.id != meta.id);
            metas.push(meta.clone());
        })?;
        Ok(meta)
    }

    /// Keeps the `keep` most recently updated conversations and deletes the rest. `protect` is never deleted and is
    /// kept in addition to the `keep` newest. Returns the ids that were deleted.
    pub fn prune(&self, keep: usize, protect: Option<&str>) -> Result<Vec<String>, PersistError> {
        if let Some(id) = protect {
            check_id(id)?;
        }
        let _guard = self.guard();
        let metas = self.index_locked()?.conversations;
        let mut removed: HashSet<String> = HashSet::new();
        for (i, meta) in metas.iter().enumerate() {
            if i < keep || Some(meta.id.as_str()) == protect {
                continue;
            }
            remove_if_exists(&self.path_for(&meta.id))?;
            removed.insert(meta.id.clone());
        }
        if !removed.is_empty() {
            let touched: Vec<&str> = removed.iter().map(String::as_str).collect();
            self.update_index_locked(&touched, |metas| metas.retain(|m| !removed.contains(&m.id)))?;
        }
        let mut removed: Vec<String> = removed.into_iter().collect();
        removed.sort();
        Ok(removed)
    }

    /// Deletes every conversation, the moved-aside damaged files, and the index. Other files are left alone.
    pub fn delete_all(&self) -> Result<(), PersistError> {
        let _guard = self.guard();
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_error(&self.dir, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| io_error(&self.dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let ours = name == INDEX_FILE
                || name.strip_suffix(".json").is_some_and(is_valid_id)
                || aside_id(&name).is_some();
            if ours && entry.file_type().is_ok_and(|t| t.is_file()) {
                remove_if_exists(&entry.path())?;
            }
        }
        Ok(())
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// The stored conversation, `None` when there is no file. A file that cannot be read or used is `Corrupt`.
    fn read_stored(&self, id: &str) -> Result<Option<StoredConversation>, PersistError> {
        let path = self.path_for(id);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            // Bytes that are not UTF-8 are a damaged file, not a failed read.
            Err(e) if e.kind() == ErrorKind::InvalidData => {
                return Err(PersistError::Corrupt(format!("{} is not valid UTF-8", path.display())));
            }
            Err(e) => return Err(io_error(&path, e)),
        };
        // The serde message can echo a value from the file, so only the error class and position are reported.
        let stored: StoredConversation = serde_json::from_str(&text).map_err(|e| {
            PersistError::Corrupt(format!(
                "{} is not a valid conversation ({:?} error at line {}, column {})",
                path.display(),
                e.classify(),
                e.line(),
                e.column()
            ))
        })?;
        if stored.version != CONVERSATION_VERSION {
            return Err(PersistError::Corrupt(format!("{} has an unsupported version", path.display())));
        }
        if stored.id != id {
            return Err(PersistError::Corrupt(format!("{} holds a different conversation id", path.display())));
        }
        Ok(Some(stored))
    }

    fn write_file(&self, stored: &StoredConversation) -> Result<(), PersistError> {
        write_json(&self.path_for(&stored.id), stored)
    }

    /// Moves a damaged file to `<id>.json.corrupt-<now>`, or `-<now>-<n>` when that name is taken, so earlier aside
    /// files are never replaced. The bytes are unchanged, and the file keeps its mode (0600 when we wrote it). Caller
    /// holds the lock.
    fn set_aside(&self, id: &str, now: u64) -> Result<(), PersistError> {
        let path = self.path_for(id);
        let mut target = self.dir.join(format!("{id}.json.corrupt-{now}"));
        let mut n = 1;
        while target.exists() {
            target = self.dir.join(format!("{id}.json.corrupt-{now}-{n}"));
            n += 1;
        }
        fs::rename(&path, &target).map_err(|e| io_error(&path, e))
    }

    /// The index, checked against the files. Caller holds the lock. The index is trusted when its listed conversations
    /// and its `skipped` ids together are exactly the `<id>.json` files on disk, and every skipped id is on disk.
    /// Otherwise it is rebuilt from the files, and the rebuilt index is written.
    fn index_locked(&self) -> Result<IndexFile, PersistError> {
        let on_disk = self.ids_on_disk()?;
        if let Some(index) = self.read_index() {
            let listed: HashSet<&str> = index.conversations.iter().map(|m| m.id.as_str()).collect();
            let skipped: HashSet<&str> = index.skipped.iter().map(String::as_str).collect();
            let agrees = listed.len() == index.conversations.len()
                && skipped.len() == index.skipped.len()
                && skipped.iter().all(|id| on_disk.contains(*id))
                && listed.iter().all(|id| on_disk.contains(*id) && !skipped.contains(*id))
                && listed.len() + skipped.len() == on_disk.len();
            if agrees {
                return Ok(index);
            }
        }
        let mut conversations = Vec::new();
        let mut skipped = Vec::new();
        for id in &on_disk {
            match self.read_stored(id) {
                Ok(Some(stored)) => conversations.push(stored.meta()),
                _ => skipped.push(id.clone()),
            }
        }
        sort_metas(&mut conversations);
        skipped.sort();
        let index = IndexFile { version: CONVERSATION_VERSION, conversations, skipped };
        self.write_index(&index)?;
        Ok(index)
    }

    /// Applies `change` to the conversations, forgets the `touched` ids as skipped, and writes the index back. Caller
    /// holds the lock.
    fn update_index_locked(
        &self,
        touched: &[&str],
        change: impl FnOnce(&mut Vec<ConversationMeta>),
    ) -> Result<(), PersistError> {
        let mut index = self.index_locked()?;
        change(&mut index.conversations);
        sort_metas(&mut index.conversations);
        index.skipped.retain(|id| !touched.contains(&id.as_str()));
        self.write_index(&index)
    }

    /// The index file, or `None` when it is missing, unreadable, or not a version this code understands.
    fn read_index(&self) -> Option<IndexFile> {
        let text = fs::read_to_string(self.dir.join(INDEX_FILE)).ok()?;
        let index: IndexFile = serde_json::from_str(&text).ok()?;
        (index.version == CONVERSATION_VERSION).then_some(index)
    }

    fn write_index(&self, index: &IndexFile) -> Result<(), PersistError> {
        write_json(&self.dir.join(INDEX_FILE), index)
    }

    /// The ids of the `<id>.json` files in the directory. Anything that is not a valid id is ignored.
    fn ids_on_disk(&self) -> Result<HashSet<String>, PersistError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(HashSet::new()),
            Err(e) => return Err(io_error(&self.dir, e)),
        };
        let mut ids = HashSet::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_error(&self.dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".json")
                && is_valid_id(stem)
                && entry.file_type().is_ok_and(|t| t.is_file())
            {
                ids.insert(stem.to_string());
            }
        }
        Ok(ids)
    }
}

/// The conversation id of an aside file, `<id>.json.corrupt-<now>` (or `-<n>`). `None` for any other name.
fn aside_id(name: &str) -> Option<&str> {
    let (stem, suffix) = name.split_once(".json.corrupt-")?;
    let digits = suffix.split('-').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    (is_valid_id(stem) && digits).then_some(stem)
}

/// True for `ses_` followed by 32 lowercase hex digits. The only shape of id that becomes a file name.
pub fn is_valid_id(id: &str) -> bool {
    id.strip_prefix(ID_PREFIX)
        .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn check_id(id: &str) -> Result<(), PersistError> {
    if is_valid_id(id) { Ok(()) } else { Err(PersistError::Invalid("the conversation id is not valid".into())) }
}

/// Trims the title and checks its length and characters. Returns the title as it will be stored.
pub fn normalize_title(raw: &str) -> Result<String, PersistError> {
    let title = raw.trim();
    let chars = title.chars().count();
    if chars == 0 {
        return Err(PersistError::Invalid("the title must not be empty".into()));
    }
    if chars > MAX_RENAME_CHARS {
        return Err(PersistError::Invalid(format!("the title must be at most {MAX_RENAME_CHARS} characters")));
    }
    if title.chars().any(char::is_control) {
        return Err(PersistError::Invalid("the title must not contain control characters".into()));
    }
    Ok(title.to_string())
}

/// The title of a conversation: the first non-empty line of the first user message that has one, whitespace
/// collapsed, at most 80 characters with a trailing "…" when cut. "Untitled" when there is no such line.
pub fn derive_title(messages: &[Value]) -> String {
    messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|m| m.get("content").and_then(Value::as_str))
        .find_map(first_line)
        .map(truncate_title)
        .unwrap_or_else(|| UNTITLED.to_string())
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(collapse_whitespace).find(|line| !line.is_empty())
}

/// Control characters count as whitespace, then runs of whitespace become one space and the ends are trimmed.
fn collapse_whitespace(line: &str) -> String {
    let spaced: String = line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_title(line: String) -> String {
    if line.chars().count() <= MAX_TITLE_CHARS {
        return line;
    }
    let kept: String = line.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

/// A copy of `messages` with the redactor applied to the text that could carry a secret: the `content` of every
/// message, and the `function.arguments` of every tool call. Ids, roles, names and structure are unchanged.
pub fn redact_messages(messages: &[Value], redactor: &Redactor) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            if let Some(obj) = message.as_object_mut() {
                if let Some(Value::String(content)) = obj.get_mut("content") {
                    *content = redactor.redact(content);
                }
                if let Some(Value::Array(calls)) = obj.get_mut("tool_calls") {
                    for call in calls.iter_mut() {
                        if let Some(Value::String(arguments)) =
                            call.get_mut("function").and_then(|f| f.get_mut("arguments"))
                        {
                            *arguments = redactor.redact(arguments);
                        }
                    }
                }
            }
            message
        })
        .collect()
}

fn message_count(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter(|m| matches!(m.get("role").and_then(Value::as_str), Some("user") | Some("assistant")))
        .count()
}

fn sort_metas(metas: &mut [ConversationMeta]) {
    metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), PersistError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|e| PersistError::Io(format!("cannot encode {}: {e}", path.display())))?;
    text.push('\n');
    write_private_atomic(path, text.as_bytes()).map_err(|e| io_error(path, e))
}

fn remove_if_exists(path: &Path) -> Result<(), PersistError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(path, e)),
    }
}

fn io_error(path: &Path, e: std::io::Error) -> PersistError {
    PersistError::Io(format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;

    use super::*;
    use crate::core::settings::test_support::TempDir;

    const SECRET: &str = "known-secret-value-123";

    fn id(n: u32) -> String {
        format!("ses_{n:032x}")
    }

    fn workspace() -> Workspace {
        Workspace { host: String::new(), root: "/work/proj".into(), label: "proj".into() }
    }

    fn redactor() -> Redactor {
        Redactor::new(vec![SECRET.to_string()])
    }

    fn request(id: &str, user_text: &str) -> SaveRequest {
        SaveRequest {
            id: id.to_string(),
            workspace: workspace(),
            model: Some("model-a".into()),
            messages: vec![
                json!({"role": "user", "content": user_text}),
                json!({"role": "assistant", "content": "reply"}),
            ],
        }
    }

    fn store(tmp: &TempDir) -> ConversationStore {
        ConversationStore::new(tmp.path().join("conversations"))
    }

    fn read_index(store: &ConversationStore) -> IndexFile {
        serde_json::from_str(&fs::read_to_string(store.dir.join(INDEX_FILE)).unwrap()).unwrap()
    }

    fn tool_call(call_id: &str, arguments: &str) -> Value {
        json!({"id": call_id, "type": "function", "function": {"name": "run_command", "arguments": arguments}})
    }

    // Ids and paths

    #[test]
    fn only_ses_plus_32_lowercase_hex_is_a_valid_id() {
        assert!(is_valid_id(&id(7)));
        assert!(is_valid_id("ses_0123456789abcdef0123456789abcdef"));
        let bad = [
            "",
            "ses_",
            "ses_0123456789abcdef0123456789abcde",
            "ses_0123456789abcdef0123456789abcdef0",
            "ses_0123456789ABCDEF0123456789abcdef",
            "ses_0123456789abcdef0123456789abcdeg",
            "SES_0123456789abcdef0123456789abcdef",
            "../ses_0123456789abcdef0123456789abcde",
            "ses_../../../../../../../etc/passwd",
            "ses_0123456789abcdef0123456789abcd/f",
            "ses_0123456789abcdef0123456789abcd\\f",
            "ses_0123456789abcdef0123456789abcd.f",
            "/ses_0123456789abcdef0123456789abcdef",
            "ses_0123456789abcdef0123456789abcd\0",
            "ses_0123456789abcdef0123456789abcdé",
        ];
        for id in bad {
            assert!(!is_valid_id(id), "{id:?} must be rejected");
        }
    }

    #[test]
    fn traversal_and_absolute_ids_are_refused_and_touch_nothing() {
        let tmp = TempDir::new();
        let outside = tmp.path().join("victim.json");
        fs::create_dir_all(tmp.path()).unwrap();
        fs::write(&outside, b"keep me").unwrap();
        let s = store(&tmp);
        let evil = [
            "../victim",
            "../../victim",
            "/etc/passwd",
            "",
            "ses_../victim",
            "ses_0123456789abcdef0123456789abcd/../../victim",
        ];
        for bad in evil {
            assert!(matches!(s.load(bad), Err(PersistError::Invalid(_))), "load {bad:?}");
            assert!(matches!(s.delete(bad), Err(PersistError::Invalid(_))), "delete {bad:?}");
            assert!(matches!(s.rename(bad, "x"), Err(PersistError::Invalid(_))), "rename {bad:?}");
            assert!(matches!(s.prune(0, Some(bad)), Err(PersistError::Invalid(_))), "prune protect {bad:?}");
            assert!(matches!(s.save(&request(bad, "hi"), &redactor(), 1), Err(PersistError::Invalid(_))), "save {bad:?}");
        }
        assert_eq!(fs::read(&outside).unwrap(), b"keep me");
    }

    // Title derivation

    #[test]
    fn title_is_the_first_non_empty_line_of_the_first_user_message() {
        let messages = vec![
            json!({"role": "system", "content": "system text"}),
            json!({"role": "user", "content": "\n   \nFix   the\tparser\nsecond line"}),
            json!({"role": "user", "content": "later"}),
        ];
        assert_eq!(derive_title(&messages), "Fix the parser");
    }

    #[test]
    fn title_skips_a_blank_user_message() {
        let messages = vec![
            json!({"role": "user", "content": "   "}),
            json!({"role": "assistant", "content": "x"}),
            json!({"role": "user", "content": "real question"}),
        ];
        assert_eq!(derive_title(&messages), "real question");
    }

    #[test]
    fn title_without_a_user_message_is_untitled() {
        assert_eq!(derive_title(&[]), "Untitled");
        assert_eq!(derive_title(&[json!({"role": "assistant", "content": "hello"})]), "Untitled");
        assert_eq!(derive_title(&[json!({"role": "user", "content": " \n\t "})]), "Untitled");
    }

    #[test]
    fn long_title_is_cut_to_80_characters_with_an_ellipsis_on_a_char_boundary() {
        let text = "é".repeat(100);
        let title = derive_title(&[json!({"role": "user", "content": text})]);
        assert_eq!(title.chars().count(), 80);
        assert!(title.ends_with('…'));
        assert_eq!(title, format!("{}…", "é".repeat(79)));

        let exact = "a".repeat(80);
        assert_eq!(derive_title(&[json!({"role": "user", "content": exact.clone()})]), exact);
        let one_more = "a".repeat(81);
        assert_eq!(derive_title(&[json!({"role": "user", "content": one_more})]), format!("{}…", "a".repeat(79)));
    }

    #[test]
    fn title_cut_does_not_leave_a_space_before_the_ellipsis() {
        let text = format!("{} tail words here", "a".repeat(78));
        let title = derive_title(&[json!({"role": "user", "content": text})]);
        assert_eq!(title, format!("{}…", "a".repeat(78)));
    }

    #[test]
    fn control_characters_in_a_title_become_spaces() {
        let title = derive_title(&[json!({"role": "user", "content": "a\u{7}b\u{0}c"})]);
        assert_eq!(title, "a b c");
    }

    #[test]
    fn rename_titles_are_trimmed_and_checked() {
        assert_eq!(normalize_title("  Plan  ").unwrap(), "Plan");
        assert!(matches!(normalize_title("   "), Err(PersistError::Invalid(_))));
        assert!(matches!(normalize_title(&"x".repeat(121)), Err(PersistError::Invalid(_))));
        assert_eq!(normalize_title(&"x".repeat(120)).unwrap().chars().count(), 120);
        assert!(matches!(normalize_title("a\nb"), Err(PersistError::Invalid(_))));
        assert!(matches!(normalize_title("a\u{1b}[31m"), Err(PersistError::Invalid(_))));
    }

    // Redaction

    #[test]
    fn redaction_covers_assistant_text_tool_arguments_and_user_text() {
        let messages = vec![
            json!({"role": "user", "content": format!("use {SECRET} please")}),
            json!({"role": "assistant", "content": format!("the key is {SECRET}"), "tool_calls": [tool_call("c1", &format!("{{\"cmd\":\"echo {SECRET}\"}}"))]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "done"}),
        ];
        let out = redact_messages(&messages, &redactor());
        let text = serde_json::to_string(&out).unwrap();
        assert!(!text.contains(SECRET), "secret survived: {text}");
        assert_eq!(out[0]["content"], "use [REDACTED] please");
        assert_eq!(out[1]["content"], "the key is [REDACTED]");
        assert_eq!(out[1]["tool_calls"][0]["function"]["arguments"], "{\"cmd\":\"echo [REDACTED]\"}");
    }

    #[test]
    fn redaction_leaves_ids_roles_names_and_structure_alone() {
        let messages = vec![
            json!({"role": "assistant", "content": null, "name": "n", "tool_calls": [tool_call("call-id-1", "{}")]}),
            json!({"role": "tool", "tool_call_id": "call-id-1", "content": "ok"}),
        ];
        let out = redact_messages(&messages, &redactor());
        assert_eq!(out, messages);
    }

    #[test]
    fn redaction_applies_the_pattern_table_too() {
        let messages = vec![json!({"role": "assistant", "content": "token sk-proj-abcdefghijklmnopqrstuvwx here"})];
        let out = redact_messages(&messages, &Redactor::new(Vec::new()));
        assert_eq!(out[0]["content"], "token [REDACTED:api_key] here");
    }

    // Save, load and file format

    #[test]
    fn save_then_load_round_trips_with_redacted_messages() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let req = SaveRequest {
            id: id(1),
            workspace: workspace(),
            model: Some("model-a".into()),
            messages: vec![
                json!({"role": "user", "content": format!("hello {SECRET}")}),
                json!({"role": "assistant", "content": "hi", "tool_calls": [tool_call("c1", "{}")]}),
                json!({"role": "tool", "tool_call_id": "c1", "content": "out"}),
                json!({"role": "assistant", "content": "bye"}),
            ],
        };
        let meta = s.save(&req, &redactor(), 1_000).unwrap();
        assert_eq!(meta.id, id(1));
        assert_eq!(meta.title, "hello [REDACTED]");
        assert_eq!(meta.created_at, 1_000);
        assert_eq!(meta.updated_at, 1_000);
        assert_eq!(meta.message_count, 3, "user and assistant messages only");
        assert_eq!(meta.workspace, workspace());
        assert_eq!(meta.model.as_deref(), Some("model-a"));

        let loaded = s.load(&id(1)).unwrap();
        assert_eq!(loaded.version, CONVERSATION_VERSION);
        assert_eq!(loaded.messages.len(), 4);
        assert_eq!(loaded.messages[0]["content"], "hello [REDACTED]");
        assert_eq!(loaded.messages[1]["tool_calls"][0]["id"], "c1");
        assert_eq!(loaded.messages[2]["content"], "out");
        assert_eq!(loaded.meta(), meta);
    }

    #[test]
    fn the_file_on_disk_never_contains_the_secret() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let req = SaveRequest {
            id: id(2),
            workspace: workspace(),
            model: None,
            messages: vec![
                json!({"role": "user", "content": SECRET}),
                json!({"role": "assistant", "content": null, "tool_calls": [tool_call("c1", &format!("{{\"k\":\"{SECRET}\"}}"))]}),
                json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}),
            ],
        };
        s.save(&req, &redactor(), 5).unwrap();
        let on_disk = fs::read_to_string(s.path_for(&id(2))).unwrap();
        assert!(!on_disk.contains(SECRET));
        assert!(!fs::read_to_string(s.dir.join(INDEX_FILE)).unwrap().contains(SECRET));
    }

    #[test]
    fn files_are_private_and_the_directory_is_0700() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(3), "hi"), &redactor(), 1).unwrap();
        let dir_mode = fs::metadata(&s.dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        for path in [s.path_for(&id(3)), s.dir.join(INDEX_FILE)] {
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "{}", path.display());
        }
    }

    #[test]
    fn resaving_keeps_created_at_title_and_workspace_and_moves_updated_at() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(4), "first question"), &redactor(), 100).unwrap();
        let other_ws = Workspace { host: "box".into(), root: "/elsewhere".into(), label: "elsewhere".into() };
        let again = SaveRequest {
            id: id(4),
            workspace: other_ws,
            model: None,
            messages: vec![json!({"role": "user", "content": "different now"})],
        };
        let meta = s.save(&again, &redactor(), 250).unwrap();
        assert_eq!(meta.created_at, 100);
        assert_eq!(meta.updated_at, 250);
        assert_eq!(meta.title, "first question");
        assert_eq!(meta.workspace, workspace());
        assert_eq!(meta.model.as_deref(), Some("model-a"), "a save without a model keeps the stored one");
        assert_eq!(meta.message_count, 1);
    }

    #[test]
    fn save_over_a_damaged_file_moves_it_aside_and_writes_a_fresh_one() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        fs::create_dir_all(&s.dir).unwrap();
        let damaged: &[u8] = b"{ not json";
        fs::write(s.path_for(&id(5)), damaged).unwrap();

        let meta = s.save(&request(&id(5), "fresh start"), &redactor(), 42).unwrap();
        assert_eq!(meta.created_at, 42, "a fresh file gets the save time");
        assert_eq!(meta.title, "fresh start", "the title comes from the new messages");
        assert_eq!(fs::read(s.dir.join(format!("{}.json.corrupt-42", id(5)))).unwrap(), damaged);
        let loaded = s.load(&id(5)).unwrap();
        assert_eq!(loaded.messages[0]["content"], "fresh start");
        assert_eq!(s.list().unwrap().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec![id(5).as_str()]);
    }

    #[test]
    fn a_second_damaged_file_in_the_same_second_gets_its_own_aside_name() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(15), "one"), &redactor(), 1).unwrap();
        fs::write(s.path_for(&id(15)), b"first damage").unwrap();
        s.save(&request(&id(15), "two"), &redactor(), 42).unwrap();
        fs::write(s.path_for(&id(15)), b"second damage").unwrap();
        s.save(&request(&id(15), "three"), &redactor(), 42).unwrap();

        assert_eq!(fs::read(s.dir.join(format!("{}.json.corrupt-42", id(15)))).unwrap(), b"first damage");
        assert_eq!(fs::read(s.dir.join(format!("{}.json.corrupt-42-1", id(15)))).unwrap(), b"second damage");
        assert_eq!(s.load(&id(15)).unwrap().title, "three");
    }

    #[test]
    fn load_of_a_damaged_file_changes_nothing_and_its_error_leaks_nothing() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        fs::create_dir_all(&s.dir).unwrap();
        let damaged = format!(r#"{{"version": "sk-proj-abcdefghijklmnopqrstuvwx", "note": "{SECRET}""#);
        fs::write(s.path_for(&id(16)), &damaged).unwrap();

        let err = s.load(&id(16)).unwrap_err().to_string();
        assert!(!err.contains("sk-proj") && !err.contains(SECRET), "{err}");
        assert_eq!(fs::read(s.path_for(&id(16))).unwrap(), damaged.as_bytes(), "load changes nothing");
        assert_eq!(fs::read_dir(&s.dir).unwrap().count(), 1, "load creates no aside file");
    }

    #[test]
    fn list_ignores_aside_files() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(13), "fine"), &redactor(), 3).unwrap();
        s.save(&request(&id(14), "was damaged"), &redactor(), 4).unwrap();
        fs::write(s.path_for(&id(14)), b"damage").unwrap();
        s.save(&request(&id(14), "repaired"), &redactor(), 5).unwrap();
        assert!(s.dir.join(format!("{}.json.corrupt-5", id(14))).exists());

        let listed: Vec<String> = s.list().unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(listed, vec![id(14), id(13)]);
        let index = read_index(&s);
        assert_eq!(index.conversations.len(), 2);
        assert!(index.skipped.is_empty());
    }

    #[test]
    fn load_reports_missing_damaged_and_mismatched_files() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        assert!(matches!(s.load(&id(6)), Err(PersistError::NotFound(_))));

        fs::create_dir_all(&s.dir).unwrap();
        fs::write(s.path_for(&id(6)), b"[1, 2").unwrap();
        assert!(matches!(s.load(&id(6)), Err(PersistError::Corrupt(_))));
        fs::write(s.path_for(&id(6)), [0xff, 0xfe, 0x00]).unwrap();
        assert!(matches!(s.load(&id(6)), Err(PersistError::Corrupt(_))), "bytes that are not UTF-8");

        s.save(&request(&id(7), "hi"), &redactor(), 1).unwrap();
        fs::copy(s.path_for(&id(7)), s.path_for(&id(8))).unwrap();
        assert!(matches!(s.load(&id(8)), Err(PersistError::Corrupt(msg)) if msg.contains("different conversation id")));

        let mut value: Value = serde_json::from_str(&fs::read_to_string(s.path_for(&id(7))).unwrap()).unwrap();
        value["version"] = json!(2);
        fs::write(s.path_for(&id(7)), value.to_string()).unwrap();
        assert!(matches!(s.load(&id(7)), Err(PersistError::Corrupt(msg)) if msg.contains("version")));
    }

    #[test]
    fn a_corrupt_error_does_not_echo_file_content() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        fs::create_dir_all(&s.dir).unwrap();
        fs::write(s.path_for(&id(9)), br#"{"version":"sk-proj-abcdefghijklmnopqrstuvwx","id":1}"#).unwrap();
        let err = s.load(&id(9)).unwrap_err().to_string();
        assert!(!err.contains("sk-proj"), "{err}");
    }

    // Listing and the index

    #[test]
    fn list_is_most_recently_updated_first_and_writes_the_index() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(10), "old"), &redactor(), 10).unwrap();
        s.save(&request(&id(11), "newest"), &redactor(), 30).unwrap();
        s.save(&request(&id(12), "middle"), &redactor(), 20).unwrap();
        let listed: Vec<String> = s.list().unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(listed, vec![id(11), id(12), id(10)]);
        assert_eq!(read_index(&s).conversations.len(), 3);
    }

    #[test]
    fn list_of_a_missing_directory_is_empty() {
        let tmp = TempDir::new();
        assert!(store(&tmp).list().unwrap().is_empty());
    }

    #[test]
    fn list_rebuilds_a_missing_index_and_skips_unreadable_files() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(20), "kept"), &redactor(), 5).unwrap();
        s.save(&request(&id(21), "also kept"), &redactor(), 6).unwrap();
        fs::write(s.path_for(&id(22)), b"garbage").unwrap();
        fs::remove_file(s.dir.join(INDEX_FILE)).unwrap();

        let listed = s.list().unwrap();
        assert_eq!(listed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec![id(21), id(20)]);
        let index = read_index(&s);
        assert_eq!(index.conversations.len(), 2, "the rewritten index lists only readable conversations");
        assert_eq!(index.skipped, vec![id(22)], "the unreadable file is recorded as skipped");
    }

    #[test]
    fn a_damaged_file_does_not_make_every_call_rewrite_the_index() {
        use std::os::unix::fs::MetadataExt;
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(40), "ok"), &redactor(), 5).unwrap();
        fs::write(s.path_for(&id(41)), b"garbage").unwrap();
        s.list().unwrap();
        let index_path = s.dir.join(INDEX_FILE);
        let before = fs::metadata(&index_path).unwrap().ino();

        // A rewrite is an atomic rename of a new file, so its inode changes. An unchanged inode means no rewrite.
        assert_eq!(s.list().unwrap().len(), 1);
        assert_eq!(s.list().unwrap().len(), 1);
        assert_eq!(fs::metadata(&index_path).unwrap().ino(), before);

        // A save of another conversation still writes the index, and keeps the skipped id.
        s.save(&request(&id(42), "also ok"), &redactor(), 6).unwrap();
        assert_eq!(read_index(&s).skipped, vec![id(41)]);
        assert_eq!(s.list().unwrap().len(), 2);
    }

    #[test]
    fn saving_over_a_damaged_file_clears_its_skipped_entry() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(50), "ok"), &redactor(), 5).unwrap();
        fs::write(s.path_for(&id(51)), b"garbage").unwrap();
        s.list().unwrap();
        assert_eq!(read_index(&s).skipped, vec![id(51)]);

        s.save(&request(&id(51), "repaired"), &redactor(), 6).unwrap();
        let index = read_index(&s);
        assert!(index.skipped.is_empty());
        assert_eq!(index.conversations.len(), 2);
    }

    #[test]
    fn deleting_a_damaged_file_clears_its_skipped_entry() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(52), "ok"), &redactor(), 5).unwrap();
        fs::write(s.path_for(&id(53)), b"garbage").unwrap();
        s.list().unwrap();
        s.delete(&id(53)).unwrap();
        assert!(read_index(&s).skipped.is_empty());
        assert_eq!(s.list().unwrap().len(), 1);
    }

    #[test]
    fn a_skipped_file_removed_by_hand_is_forgotten_by_the_next_list() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(54), "ok"), &redactor(), 5).unwrap();
        fs::write(s.path_for(&id(55)), b"garbage").unwrap();
        s.list().unwrap();
        fs::remove_file(s.path_for(&id(55))).unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        assert!(read_index(&s).skipped.is_empty());
    }

    #[test]
    fn an_index_without_the_skipped_field_is_read_as_having_none() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(56), "ok"), &redactor(), 5).unwrap();
        let path = s.dir.join(INDEX_FILE);
        let mut value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("skipped");
        fs::write(&path, value.to_string()).unwrap();
        let listed: Vec<String> = s.list().unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(listed, vec![id(56)]);
    }

    #[test]
    fn list_rebuilds_a_corrupt_index() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(23), "one"), &redactor(), 5).unwrap();
        fs::write(s.dir.join(INDEX_FILE), b"{ broken").unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        assert_eq!(read_index(&s).conversations.len(), 1);
    }

    #[test]
    fn list_rebuilds_an_index_that_disagrees_with_the_files() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(24), "a"), &redactor(), 5).unwrap();
        s.save(&request(&id(25), "b"), &redactor(), 6).unwrap();

        // A file removed behind the index's back, and a copy added under another name behind its back. The copy
        // says it is conversation 25, so under the name 26 it is damaged and is skipped.
        fs::remove_file(s.path_for(&id(24))).unwrap();
        fs::copy(s.path_for(&id(25)), s.path_for(&id(26))).unwrap();
        let listed: Vec<String> = s.list().unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(listed, vec![id(25)]);
        assert_eq!(read_index(&s).conversations.len(), 1);
    }

    #[test]
    fn an_index_entry_for_a_missing_file_does_not_survive_a_list() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(27), "a"), &redactor(), 5).unwrap();
        fs::remove_file(s.path_for(&id(27))).unwrap();
        assert!(s.list().unwrap().is_empty());
        assert!(read_index(&s).conversations.is_empty());
    }

    #[test]
    fn index_version_it_does_not_know_is_rebuilt() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(28), "a"), &redactor(), 5).unwrap();
        fs::write(s.dir.join(INDEX_FILE), br#"{"version": 9, "conversations": []}"#).unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
    }

    // Delete, rename, prune, delete_all

    #[test]
    fn delete_removes_the_file_and_the_index_entry() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(30), "a"), &redactor(), 5).unwrap();
        s.save(&request(&id(31), "b"), &redactor(), 6).unwrap();
        s.delete(&id(30)).unwrap();
        assert!(!s.path_for(&id(30)).exists());
        assert_eq!(s.list().unwrap().iter().map(|m| m.id.clone()).collect::<Vec<_>>(), vec![id(31)]);
        assert_eq!(read_index(&s).conversations.len(), 1);
    }

    #[test]
    fn deleting_a_missing_conversation_is_not_an_error() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(32), "a"), &redactor(), 5).unwrap();
        assert!(s.delete(&id(33)).is_ok());
        assert_eq!(s.list().unwrap().len(), 1);
    }

    #[test]
    fn rename_changes_the_title_in_the_file_and_the_index_and_keeps_updated_at() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(34), "a"), &redactor(), 5).unwrap();
        let meta = s.rename(&id(34), "  My plan  ").unwrap();
        assert_eq!(meta.title, "My plan");
        assert_eq!(meta.updated_at, 5);
        assert_eq!(s.load(&id(34)).unwrap().title, "My plan");
        assert_eq!(s.list().unwrap()[0].title, "My plan");
        assert_eq!(read_index(&s).conversations[0].title, "My plan");
    }

    #[test]
    fn rename_rejects_bad_titles_and_missing_conversations() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(35), "a"), &redactor(), 5).unwrap();
        assert!(matches!(s.rename(&id(35), ""), Err(PersistError::Invalid(_))));
        assert!(matches!(s.rename(&id(35), "bad\ntitle"), Err(PersistError::Invalid(_))));
        assert_eq!(s.load(&id(35)).unwrap().title, "a", "a refused rename changes nothing");
        assert!(matches!(s.rename(&id(36), "fine"), Err(PersistError::NotFound(_))));
    }

    #[test]
    fn prune_keeps_the_newest_and_the_protected_conversation() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        for n in 0..4 {
            s.save(&request(&id(40 + n), "x"), &redactor(), 10 + u64::from(n)).unwrap();
        }
        // id 40 is the oldest. Protecting it keeps it in addition to the two newest.
        let removed = s.prune(2, Some(&id(40))).unwrap();
        assert_eq!(removed, vec![id(41)]);
        let mut left: Vec<String> = s.list().unwrap().into_iter().map(|m| m.id).collect();
        left.sort();
        assert_eq!(left, vec![id(40), id(42), id(43)]);
        assert!(!s.path_for(&id(41)).exists());
        assert!(s.path_for(&id(40)).exists());
    }

    #[test]
    fn prune_to_zero_without_protection_removes_everything() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(50), "x"), &redactor(), 1).unwrap();
        s.save(&request(&id(51), "x"), &redactor(), 2).unwrap();
        let mut removed = s.prune(0, None).unwrap();
        removed.sort();
        assert_eq!(removed, vec![id(50), id(51)]);
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn prune_with_nothing_to_remove_changes_nothing() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(52), "x"), &redactor(), 1).unwrap();
        assert!(s.prune(500, Some(&id(52))).unwrap().is_empty());
        assert_eq!(s.list().unwrap().len(), 1);
    }

    #[test]
    fn delete_all_removes_conversations_and_the_index_but_not_other_files() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        s.save(&request(&id(60), "x"), &redactor(), 1).unwrap();
        s.save(&request(&id(61), "x"), &redactor(), 2).unwrap();
        fs::write(s.dir.join("notes.txt"), b"not ours").unwrap();
        fs::write(s.dir.join("notes.json.corrupt-1"), b"not ours either").unwrap();
        fs::write(s.path_for(&id(62)), b"damaged").unwrap();
        s.save(&request(&id(62), "x"), &redactor(), 3).unwrap();
        assert!(s.dir.join(format!("{}.json.corrupt-3", id(62))).exists());
        s.delete_all().unwrap();
        assert!(!s.path_for(&id(60)).exists());
        assert!(!s.path_for(&id(61)).exists());
        assert!(!s.path_for(&id(62)).exists());
        assert!(!s.dir.join(format!("{}.json.corrupt-3", id(62))).exists(), "aside files are removed");
        assert!(!s.dir.join(INDEX_FILE).exists());
        assert_eq!(fs::read(s.dir.join("notes.txt")).unwrap(), b"not ours");
        assert_eq!(fs::read(s.dir.join("notes.json.corrupt-1")).unwrap(), b"not ours either");
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn delete_all_on_a_missing_directory_is_ok() {
        let tmp = TempDir::new();
        assert!(store(&tmp).delete_all().is_ok());
    }

    #[test]
    fn the_title_of_a_new_conversation_comes_from_the_redacted_text() {
        let tmp = TempDir::new();
        let s = store(&tmp);
        let req = SaveRequest {
            id: id(70),
            workspace: workspace(),
            model: None,
            messages: vec![json!({"role": "user", "content": SECRET})],
        };
        let meta = s.save(&req, &redactor(), 1).unwrap();
        assert_eq!(meta.title, "[REDACTED]");
    }
}
