//! Tauri wiring only. All logic lives in `core`; this file maps commands and channels onto it.
//!
//! Channels, per terminal (tab): `on_data` carries raw PTY bytes (`InvokeResponseBody::Raw`, which the frontend
//! receives as an ArrayBuffer) and must be acknowledged with `ack`. `on_events` carries JSON objects of session
//! events (`state`, `integration`, `observer`, `cwd`, `record_started`, `record_finished`).
//! Per chat: the events channel given to `attach_chat` carries agent events (`text`, `text_delta`, `thinking`,
//! `tool_call`, `tool_result`, `approval_request`, `question_request`, `integration_request`, `todo_update`,
//! `context_usage`, `agent_control`, `notice`, `done`). Events are told apart by `type`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Manager, RunEvent, State};
use tauri_plugin_window_state::StateFlags;

// Core logic (PTY, shell integration, observer, agent) has no Tauri dependency, so `cargo test` runs it headless.
pub mod core;

use crate::core::agent::{
    self, AgentCtx, AgentEvent, ApprovalGate, AutoState, MockAgent, OnDrop, Steer, SteerItem, SteerMode, TurnRules, UserMessage,
};
use crate::core::env::{env_setting, home_dir};
use crate::core::approval::{self, ApprovalPolicy, ManualPolicy, SendKeysMemory};
use crate::core::chat::agent_loop::{self, Conversation, LastUsage, LlmAgent, LlmSettings, USER_AGENT};
use crate::core::chat::commands::{self, CommandCatalog};
use crate::core::chat::instructions;
use crate::core::chat::questions;
use crate::core::chat::side::{self, SideCall};
use crate::core::chat::attachments::{AttachmentInput, ImageFile, prepare, stored_image_url};
use crate::core::chat::context::ContextUsage;
use crate::core::chat::export;
use crate::core::chat::history::History;
use crate::core::chat::redact::Redactor;
use crate::core::chat::todo::{Todo, TodoList};
use crate::core::foreground::Foreground;
use crate::core::fs_tools::Scope;
use crate::core::llm::TurnCancel;
use crate::core::observer::{EventSink, InputState, IntegrationStatus, LOCAL_KEY, LevelInfo, ObserverStatus, SessionEvent};
use crate::core::persist::conversations::{self, ConversationMeta};
use crate::core::persist::workspace::workspace_for;
use crate::core::persist::turn::{FinishedTurn, TurnEnv, resume, save_turn};
use crate::core::persist::ui_state::{UiState, UiStatePatch};
use crate::core::persist::{Persist, now_secs};
use crate::core::pty::OutputSink;
use crate::core::record::Cwd;
use crate::core::registry::Registry;
use crate::core::session::{IntegrateOutcome, Session, SessionStats, SpawnConfig};
use crate::core::settings::models::fetch_models as fetch_model_list;
use crate::core::settings::presets::{self, ProviderPreset};
use crate::core::settings::secrets::{Secrets, secrets_file_path};
use crate::core::settings::service::{
    FetchOutcome, RecoverAction, SaveProviderInput, SettingsService, SettingsStateView, SettingsView, TestResult,
    test_model,
};
use crate::core::settings::{
    AgentRequests, AgentSettings, ApprovalSettings, IntegrationSettings, LimitsSettings, ModelRef, Nested, OnDenied,
};
use crate::core::mcp::config::McpLevel;
use crate::core::mcp::service::{McpImportResult, McpServerInput, McpService, McpStatus, McpView, TurnMcp};
use crate::core::skills::{self, Places, SkillsSettings, SkillsView};
use crate::core::terminals::{ChatTerminals, Terminal};
use crate::core::tools::{AgentTools, SessionTools, TurnBudget};

/// The terminals: one shell each, keyed by tab id. A tab belongs to at most one chat (`Tab::owner`).
type Reg = Registry<Tab>;
/// The agent conversations, keyed by chat id.
type ChatReg = Registry<Chat>;
/// The terminals an agent turn took control of, each with the `terminal` its control event named (None: the only one). The
/// turn clears them all when it ends (see `Chat::end_turn`).
type Touched = Mutex<Vec<(Arc<Tab>, Option<String>)>>;

/// The first tab's id. Its chat has the same id: single-tab parity, the app's first terminal belongs to chat `main`.
const MAIN: &str = "main";

/// Counts tabs, so each has a creation order (`Tab::seq`). A chat's default terminal is its oldest.
static TAB_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_tab_seq() -> u64 {
    TAB_SEQ.fetch_add(1, Ordering::SeqCst)
}

/// Counts shell spawns, so each one gets its own shim directory name (see `next_shim_id`).
static SHIM_IDS: AtomicU64 = AtomicU64::new(0);

/// A name for the next shell's shim directory, unique in this process. A dead shell's drop removes its own directory,
/// which must never be the one a live shell is using, so no two spawns share a name (session ids are not used).
fn next_shim_id() -> String {
    SHIM_IDS.fetch_add(1, Ordering::SeqCst).to_string()
}

/// Counts chats, so each new chat gets its own id (see `next_chat_id`).
static CHAT_SEQ: AtomicU64 = AtomicU64::new(0);

/// A new chat's id: "c1", "c2", ... Process-global and never reused, so a closed chat's id is never taken over by a later
/// one. `MAIN` is never produced: it is the default chat's id.
fn next_chat_id() -> String {
    format!("c{}", CHAT_SEQ.fetch_add(1, Ordering::SeqCst) + 1)
}

/// The longest terminal title kept, in characters.
const TITLE_MAX: usize = 120;

/// A title as the frontend sends it: trimmed and capped. Empty (or blank) is None, which means no title.
fn clean_title(title: Option<String>) -> Option<String> {
    let capped: String = title?.trim().chars().take(TITLE_MAX).collect();
    let trimmed = capped.trim_end();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether a chat's conversation is the one remembered as the last conversation (in the UI state). Only chat `MAIN` does.
fn remembers_last(chat_id: &str) -> bool {
    chat_id == MAIN
}

/// The frontend's channels for one session. Swapped on re-attach, so the session keeps running.
#[derive(Default)]
struct Channels {
    data: Mutex<Option<Channel<InvokeResponseBody>>>,
    events: Mutex<Option<Channel<Value>>>,
}

impl Channels {
    fn attach(&self, data: Channel<InvokeResponseBody>, events: Channel<Value>) {
        *lock(&self.data) = Some(data);
        *lock(&self.events) = Some(events);
    }

    /// Sends a session event (see `EventsSink`). Agent events go to the chat's channel instead (`Chat::send_agent`).
    fn send_json<T: Serialize>(&self, ev: &T) {
        send_to(&self.events, ev);
    }
}

/// Writes one event to a frontend events channel, when one is attached.
fn send_to<T: Serialize>(slot: &Mutex<Option<Channel<Value>>>, ev: &T) {
    if let Ok(v) = serde_json::to_value(ev)
        && let Some(ch) = lock(slot).as_ref()
    {
        let _ = ch.send(v);
    }
}

/// Raw output to the terminal. With no window attached the bytes are dropped; the gate is then never
/// acked for them. Losing more than 512 KiB while detached is enough to set up the hang (the stale
/// unacked count plus a later burst trips the 2 MiB pause with no acks coming); `attach_or_spawn`
/// resets the gate on re-attach to clear it.
struct RawSink(Arc<Channels>);

impl OutputSink for RawSink {
    fn output(&mut self, bytes: &[u8]) {
        if let Some(ch) = lock(&self.0.data).as_ref() {
            let _ = ch.send(InvokeResponseBody::Raw(bytes.to_vec()));
        }
    }
}

struct EventsSink(Arc<Channels>);

impl EventSink for EventsSink {
    fn event(&self, ev: SessionEvent) {
        self.0.send_json(&ev);
    }
}

/// One terminal: a shell (PTY) with its frontend data and session-event channels. `owner` is the chat it belongs to.
pub struct Tab {
    /// The terminal's id (the tab id the frontend uses). The model names it in `terminal`.
    id: String,
    /// Shared with the views of agent turns (see `Terminal::view`), which may outlive a call.
    session: Arc<Session>,
    channels: Arc<Channels>,
    /// True while the agent holds this shell: a shell-acting call, or its own command on top (see `AgentEvent::AgentControl`).
    agent_control: AtomicBool,
    /// The chat this terminal belongs to, if any. Changed only by `assign_tab`.
    owner: Mutex<Option<String>>,
    /// Creation order (see `next_tab_seq`).
    seq: u64,
    /// The title shown for the terminal and given to the model, set by `set_tab_title`. None when there is none.
    title: Mutex<Option<String>>,
}

impl Tab {
    fn owner_id(&self) -> Option<String> {
        lock(&self.owner).clone()
    }
}

impl Terminal for Tab {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn title(&self) -> Option<String> {
        lock(&self.title).clone()
    }

    /// The tools over this terminal's session, on the turn's budget and cancel. The view owns the session, not the tab.
    fn view(self: Arc<Self>, budget: Arc<TurnBudget>, cancel: Option<Arc<TurnCancel>>) -> Box<dyn AgentTools> {
        let mut tools = SessionTools::shared(self.session.clone()).with_budget(budget);
        if let Some(cancel) = cancel {
            tools = tools.with_cancel(cancel);
        }
        Box::new(tools)
    }
}

/// One agent conversation (a chat): its agent state, approvals and agent-events channel. Its terminals are the tabs
/// whose `owner` is its id (see `chat_tabs`); the chat holds no list of them.
pub struct Chat {
    id: String,
    /// The frontend's agent-events channel. Swapped on re-attach, like the tab's channels.
    events: Mutex<Option<Channel<Value>>>,
    approvals: ApprovalGate,
    agent_busy: AtomicBool,
    /// The running turn's cancel token. Set when a turn starts, cleared when it ends. Ctrl-C and `cancel_agent` set it.
    turn_cancel: Mutex<Option<Arc<TurnCancel>>>,
    /// The conversation: its id (sent to the provider) and its history. `new_conversation` resets it.
    conversation: Conversation,
    /// The project instructions already announced: the workspace root path and the rules text. A turn whose
    /// loaded pair differs announces itself with an `AgentEvent::Notice`; a repeat stays quiet.
    project_rules_seen: Mutex<Option<(String, String)>>,
    /// The steer mailbox of the running turn: messages sent while it runs (see `steer_message`). Open only for an agent turn.
    steer: Steer,
    /// Set by `close_chat`. A closed chat takes no turn and no terminal, so a stale handle cannot revive it.
    closed: AtomicBool,
}

impl Chat {
    fn new(id: &str) -> io::Result<Self> {
        Ok(Chat {
            id: id.to_string(),
            events: Mutex::new(None),
            approvals: ApprovalGate::new(),
            agent_busy: AtomicBool::new(false),
            turn_cancel: Mutex::new(None),
            conversation: Conversation::new()?,
            project_rules_seen: Mutex::new(None),
            steer: Steer::default(),
            closed: AtomicBool::new(false),
        })
    }

    fn attach_events(&self, events: Channel<Value>) {
        *lock(&self.events) = Some(events);
    }

    /// Sends an agent event to the chat's channel. Session events go to the tab's channel instead (`EventsSink`).
    fn send_agent<T: Serialize>(&self, ev: &T) {
        send_to(&self.events, ev);
    }

    /// A restarted terminal starts with the approval state a new session had: approve-all off, no `send_keys` memory,
    /// and the project-rules notice comes again. The conversation itself carries over (see `restart_session`).
    fn reset_for_new_shell(&self) {
        self.approvals.set_auto_approve(false, 0);
        *self.approvals.send_keys_memory() = SendKeysMemory::default();
        *lock(&self.project_rules_seen) = None;
    }

    /// Ends a turn: clears the cancel token, the control flag of every terminal the turn took control of (telling the UI,
    /// per terminal) and the busy flag. Runs even when the turn panicked. Steers still waiting are given back first
    /// (`steer_returned`), so they arrive before the turn's `done`. `touched` is what the turn recorded (see `Touched`).
    fn end_turn(&self, touched: &Touched) {
        *lock(&self.turn_cancel) = None;
        self.return_steers();
        for (tab, terminal) in lock(touched).drain(..) {
            if tab.agent_control.swap(false, Ordering::SeqCst) {
                self.send_agent(&AgentEvent::AgentControl { active: false, terminal });
            }
        }
        self.agent_busy.store(false, Ordering::SeqCst);
    }

    /// Closes the steer mailbox and gives back what it still holds (`steer_returned`, when there is any).
    fn return_steers(&self) {
        let ids: Vec<u64> = self.steer.close_and_take().iter().map(|item| item.id).collect();
        if !ids.is_empty() {
            self.send_agent(&AgentEvent::SteerReturned { ids });
        }
    }

    /// Cancels the running turn, if any, and denies its pending approvals. Harmless when no turn is running. The steers
    /// waiting for the turn are given back: a cancel clears them.
    fn cancel_turn(&self) {
        agent::cancel_turn(&self.turn_cancel, &self.approvals);
        self.return_steers();
    }

    /// Whether the Ctrl-C typed into `tab` belongs to this chat's turn, and so cancels it. Never when the agent is not
    /// busy. Always when the agent holds the shell (a shell-acting call, or its own command on top). Otherwise
    /// only at an idle prompt of the local shell (Prompt, Continuation, Submitted with no nested levels), where
    /// Ctrl-C can only clear the line: a Ctrl-C typed into the user's own running command, the alternate screen,
    /// a nested prompt, raw mode or a dead session stays with the shell.
    fn interrupt_cancels_turn(&self, tab: &Tab) -> bool {
        Self::interrupt_cancels(
            self.agent_busy.load(Ordering::SeqCst),
            tab.agent_control.load(Ordering::SeqCst),
            tab.session.open_agent_records().iter().any(|r| r.on_top),
            tab.session.state(),
            tab.session.levels().len(),
        )
    }

    /// The decision half of `interrupt_cancels_turn`, split out so the matrix is unit-testable without a live
    /// session. `levels` is the count from `Session::levels`, which always includes the local shell: more than
    /// one means a nested shell holds the prompt.
    fn interrupt_cancels(
        agent_busy: bool,
        agent_control: bool,
        agent_on_top: bool,
        state: InputState,
        levels: usize,
    ) -> bool {
        if !agent_busy {
            return false;
        }
        if agent_control || agent_on_top {
            return true;
        }
        matches!(state, InputState::Prompt | InputState::Continuation | InputState::Submitted) && levels <= 1
    }

    /// Marks the agent busy and installs the turn's cancel token. Returns the conversation's history, todo list and id for
    /// the turn. Held under the turn-cancel lock, which `new_conversation` also takes, so a reset cannot land mid-start.
    fn begin_turn(&self, cancel: &Arc<TurnCancel>) -> Result<(Arc<Mutex<History>>, TodoList, String), String> {
        let mut slot = lock(&self.turn_cancel);
        if self.closed.load(Ordering::SeqCst) {
            return Err(format!("no chat {}", self.id));
        }
        if self.agent_busy.swap(true, Ordering::SeqCst) {
            return Err("the agent is still working on the previous message".into());
        }
        *slot = Some(cancel.clone());
        Ok((self.conversation.history(), self.conversation.todos(), self.conversation.id()))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn no_session(id: &str) -> String {
    format!("no session {id}")
}

fn tab_of(reg: &Reg, id: &str) -> Result<Arc<Tab>, String> {
    reg.get(id).ok_or_else(|| no_session(id))
}

fn chat_of(chats: &ChatReg, id: &str) -> Result<Arc<Chat>, String> {
    chats.get(id).ok_or_else(|| no_session(id))
}

/// The chat that `session_id` attaches to on start: the first tab (`MAIN`) belongs to chat `MAIN`. Other tabs have none.
fn owner_for_session(session_id: &str) -> Option<&'static str> {
    (session_id == MAIN).then_some(MAIN)
}

/// The tabs `chat_id` owns, oldest first. Generic over the tab type, so the rule is testable without a PTY.
fn owned_tabs<T>(
    tabs: Vec<Arc<T>>,
    chat_id: &str,
    owner: impl Fn(&T) -> Option<String>,
    seq: impl Fn(&T) -> u64,
) -> Vec<Arc<T>> {
    let mut mine: Vec<Arc<T>> = tabs.into_iter().filter(|t| owner(t.as_ref()).as_deref() == Some(chat_id)).collect();
    mine.sort_by_key(|t| seq(t.as_ref()));
    mine
}

/// The terminals of a chat: the tab registry filtered by owner, oldest first. A chat with none is legal.
fn chat_tabs(reg: &Reg, chat_id: &str) -> Vec<Arc<Tab>> {
    owned_tabs(reg.snapshot(), chat_id, |t| t.owner_id(), |t| t.seq)
}

/// The chat's default terminal: its oldest tab. None when the chat has no tab.
fn default_tab(reg: &Reg, chat_id: &str) -> Option<Arc<Tab>> {
    chat_tabs(reg, chat_id).into_iter().next()
}

/// A chat and its default terminal, for the chat commands. The terminal is None for a chat with no terminal, which is a
/// legal chat: each command takes what it needs from the terminal when there is one. Missing chat is "no session".
fn chat_and_default(reg: &Reg, chats: &ChatReg, chat_id: &str) -> Result<(Arc<Chat>, Option<Arc<Tab>>), String> {
    Ok((chat_of(chats, chat_id)?, default_tab(reg, chat_id)))
}

/// The approve-all level of a chat's default terminal: its shell on top. The local level when there is no terminal.
fn top_key_of(tab: Option<&Tab>) -> u64 {
    tab.map_or(LOCAL_KEY, |tab| tab.session.top_key())
}

/// The terminals of a chat for an agent turn (see `ChatTerminals::new`): the registry's tabs of the chat, read on every
/// call, so a terminal assigned or unassigned during a turn is seen by the next call.
fn chat_terminals(app: tauri::AppHandle, chat_id: String) -> impl Fn() -> Vec<Arc<dyn Terminal>> + Send + Sync + 'static {
    move || {
        let reg = app.state::<Reg>();
        chat_tabs(&reg, &chat_id).into_iter().map(|tab| tab as Arc<dyn Terminal>).collect()
    }
}

/// The chat with `id`, created when it does not exist yet.
fn ensure_chat(chats: &ChatReg, id: &str) -> Result<(), String> {
    chats.get_or_insert_with(id, || Chat::new(id)).map(|_| ()).map_err(io_err)
}

/// Whether `chat_id` may change a terminal's owner, and whether the owner changes. `current` is the owner now, `busy`
/// whether the chat has a turn running. `on` assigns the terminal to the chat, else unassigns it. Ok(true) changes the
/// owner, Ok(false) leaves it, Err refuses.
fn owner_change(current: Option<&str>, chat_id: &str, on: bool, busy: bool) -> Result<bool, String> {
    match current {
        Some(owner) if owner == chat_id => {
            if !on && busy { Err(BUSY_REFUSAL.into()) } else { Ok(!on) }
        }
        Some(_) if on => Err("that terminal belongs to another chat".into()),
        Some(_) => Ok(false),
        None => Ok(on),
    }
}

/// Assigns a terminal to a chat (`on`) or unassigns it. Refused when another chat owns it, or when a chat with a running
/// turn would lose it. The owner changes under the chat's turn-cancel lock, as the other chat-state changes do.
fn assign_tab(reg: &Reg, chats: &ChatReg, chat_id: &str, tab_id: &str, on: bool) -> Result<(), String> {
    let chat = chat_of(chats, chat_id)?;
    let tab = tab_of(reg, tab_id)?;
    let _turn = lock(&chat.turn_cancel);
    if chat.closed.load(Ordering::SeqCst) {
        return Err(no_session(chat_id));
    }
    let mut owner = lock(&tab.owner);
    if owner_change(owner.as_deref(), chat_id, on, chat.agent_busy.load(Ordering::SeqCst))? {
        *owner = on.then(|| chat_id.to_string());
    }
    Ok(())
}

/// The chats in lock order, by id. Code that locks several chats (the conversation deletes) takes them in this order,
/// so two of them cannot deadlock.
fn in_lock_order(mut chats: Vec<Arc<Chat>>) -> Vec<Arc<Chat>> {
    chats.sort_by(|a, b| a.id.cmp(&b.id));
    chats
}

fn io_err(e: io::Error) -> String {
    e.to_string()
}

const BUSY_REFUSAL: &str = "the agent is busy; wait for it to finish or cancel it first";
const NO_STORE: &str = "saved conversations are unavailable: the app folders could not be found";

/// The conversation and UI-state stores. `None` when the app folders could not be resolved: nothing is saved, and the
/// commands that use the stores return an error.
struct PersistState(Option<Arc<Persist>>);

impl PersistState {
    fn get(&self) -> Result<Arc<Persist>, String> {
        self.0.clone().ok_or_else(|| NO_STORE.to_string())
    }
}

/// The AI settings and the UI state live here. WATT_CONFIG_DIR (or the old AGENT_TERM_CONFIG_DIR) wins when it is set and
/// not empty; otherwise the Tauri app config folder is used. Nothing is created here.
fn resolve_config_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    dir_override("WATT_CONFIG_DIR", "AGENT_TERM_CONFIG_DIR").or_else(|| app.path().app_config_dir().ok())
}

/// Resolves the config and data folders. WATT_CONFIG_DIR and WATT_DATA_DIR (or the old AGENT_TERM_* names) win when
/// they are set and not empty; otherwise the Tauri app folders are used. Nothing is created here: the stores create
/// their folders on write.
fn resolve_persist(app: &tauri::AppHandle) -> Option<Persist> {
    let config = resolve_config_dir(app);
    let data = resolve_data_dir(app);
    match (config, data) {
        (Some(config), Some(data)) => Some(Persist::new(&config, &data)),
        _ => {
            eprintln!("watt: the app folders could not be resolved; conversations and UI state are not saved");
            None
        }
    }
}

/// Watt's data folder (conversations): WATT_DATA_DIR (or the old AGENT_TERM_DATA_DIR) when set, else the app data folder.
fn resolve_data_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    dir_override("WATT_DATA_DIR", "AGENT_TERM_DATA_DIR").or_else(|| app.path().app_data_dir().ok())
}

fn dir_override(name: &str, legacy: &str) -> Option<PathBuf> {
    env_setting(name, legacy).map(PathBuf::from)
}

/// The AI provider settings. `None` when the app folders could not be resolved: the settings commands then return an
/// error, and the environment settings still work.
struct SettingsState(Option<Arc<SettingsService>>);

const NO_SETTINGS: &str = "AI settings are unavailable: the app folders could not be found";

impl SettingsState {
    fn get(&self) -> Result<Arc<SettingsService>, String> {
        self.0.clone().ok_or_else(|| NO_SETTINGS.to_string())
    }
}

/// The MCP servers (core/mcp/service.rs). `None` when the app folders could not be found, as for `SettingsState`. The
/// enabled servers start in the background at launch (`setup`); otherwise the runtime starts on the first MCP command or turn.
struct McpState(Option<Arc<McpService>>);

impl McpState {
    fn get(&self) -> Result<Arc<McpService>, String> {
        self.0.clone().ok_or_else(|| NO_SETTINGS.to_string())
    }
}

/// This machine's host name, as the shell reports it in OSC 7 (`$HOST`). Read once; "" when that fails.
fn local_host() -> &'static str {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(read_local_host)
}

/// Unix: `/bin/hostname`, which is what zsh puts in `$HOST`.
#[cfg(unix)]
fn read_local_host() -> String {
    std::process::Command::new("/bin/hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Windows: `COMPUTERNAME`, the host the PowerShell integration reports (Hello `host=` and the OSC 7 host).
#[cfg(windows)]
fn read_local_host() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// What a save of the live conversation needs besides the store and the session: its id, history, todo list and model.
struct SavedState {
    conversation_id: String,
    history: Arc<Mutex<History>>,
    todos: TodoList,
    model: Option<String>,
    /// The conversation's last context usage, saved with it so a restored conversation can show its meter.
    context: LastUsage,
    /// Whether the save remembers the conversation as the last one (see `remembers_last`).
    remember: bool,
}

/// Saves the conversation: the whole history, todo list and last context usage, redacted, with the shell's directory (`cwd`,
/// None for a chat with no terminal). `in_turn` is true at a checkpoint inside a turn and false at the end of the turn. A
/// failure is logged without any content and never fails the turn.
fn save_conversation(store: &Persist, cwd: Option<Cwd>, state: &SavedState, redactor: &Redactor, in_turn: bool) {
    let cwd_pair = cwd.as_ref().map(|c| (c.host.as_str(), c.path.as_str()));
    let turn = FinishedTurn {
        conversation_id: &state.conversation_id,
        messages: lock(&state.history).snapshot(),
        todos: lock(&state.todos).clone(),
        model: state.model.clone(),
        cwd: cwd_pair,
        in_turn,
        context: lock(&state.context).clone(),
        remember: state.remember,
    };
    let home = home_dir();
    let env = TurnEnv { local_host: local_host(), home: home.as_deref() };
    if let Err(e) = save_turn(store, turn, redactor, env, now_secs()) {
        eprintln!("watt: the conversation was not saved ({e})");
    }
}

/// The checkpoint of an LLM turn (see `LlmAgent::with_checkpoint`): saves the conversation with `in_turn` set. Without
/// a store, nothing is saved. `tab` is the chat's default terminal, if it has one (its directory is saved).
fn checkpoint_saver(
    store: Option<Arc<Persist>>,
    tab: Option<Arc<Tab>>,
    state: Arc<SavedState>,
) -> impl FnMut(&Redactor) + Send + 'static {
    move |redactor| {
        if let Some(store) = store.as_ref() {
            save_conversation(store, tab.as_ref().and_then(|tab| tab.session.cwd()), &state, redactor, true);
        }
    }
}

/// Runs blocking storage work on Tauri's blocking pool, so file access never runs on the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())?
}

/// Dev only. WATT_HISTFILE (or the old AGENT_TERM_HISTFILE), when non-empty, is the HISTFILE the shell uses instead of
/// the real ~/.zsh_history, so manual runs never write there. Unset (the default) keeps the user's own history.
fn dev_histfile() -> Option<PathBuf> {
    dir_override("WATT_HISTFILE", "AGENT_TERM_HISTFILE")
}

#[derive(Serialize)]
struct StartResult {
    /// False when the session already existed: the channels were swapped and nothing was spawned.
    spawned: bool,
    /// The integration nonce. The frontend ignores OSC 133/633 payloads whose `n=` differs. Same on every attach.
    nonce: String,
}

/// Starts the session, or re-attaches to it. Rows and cols apply only when spawning; call `resize` after attaching.
#[tauri::command]
async fn start_session(
    session_id: String,
    rows: u16,
    cols: u16,
    on_data: Channel<InvokeResponseBody>,
    on_events: Channel<Value>,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
) -> Result<StartResult, String> {
    attach_or_spawn(&reg, &chats, &session_id, &next_shim_id(), rows, cols, on_data, on_events, None)
}

/// Starts a new shell when the terminal has exited: the dead tab is removed and a fresh one spawned under the same id,
/// with a new nonce. Its chat keeps its conversation, which is told about the new shell once, so the transcript the
/// frontend keeps still matches the backend. Otherwise this is start_session (idempotent).
#[tauri::command]
async fn restart_session(
    session_id: String,
    rows: u16,
    cols: u16,
    on_data: Channel<InvokeResponseBody>,
    on_events: Channel<Value>,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
) -> Result<StartResult, String> {
    // Checked and removed under the registry lock, so two restarts cannot each remove the other's new session.
    let old = reg.remove_if(&session_id, |tab| tab.session.state() == InputState::Exited);
    // The owning chat's turn may still be running: keep it cancelled (its approvals are denied and its `run_command`
    // stops waiting), then wait for it to end so the carried conversation is final. Past the bound the restart goes on
    // anyway with whatever the conversation holds.
    let owner = match old {
        Some(old) => {
            let owner = old.owner_id();
            if let Some(chat) = owner.as_deref().and_then(|id| chats.get(id)) {
                chat.cancel_turn();
                for _ in 0..200 {
                    if !chat.agent_busy.load(Ordering::SeqCst) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                // The new shell is a new session: the next turn is told so, once (see `Conversation::note_new_shell_session`).
                chat.conversation.note_new_shell_session();
                chat.reset_for_new_shell();
            }
            owner
        }
        None => None,
    };
    attach_or_spawn(&reg, &chats, &session_id, &next_shim_id(), rows, cols, on_data, on_events, owner)
}

/// Attaches to the tab `session_id`, spawning its shell first if there is none. `shim_id` names the shim directory on
/// spawn. `owner` is the chat a restarted tab keeps; a fresh spawn has none, except `MAIN`, which joins chat `MAIN`.
#[allow(clippy::too_many_arguments)]
fn attach_or_spawn(
    reg: &Reg,
    chats: &ChatReg,
    session_id: &str,
    shim_id: &str,
    rows: u16,
    cols: u16,
    on_data: Channel<InvokeResponseBody>,
    on_events: Channel<Value>,
    owner: Option<String>,
) -> Result<StartResult, String> {
    let mut pending = Some((on_data, on_events));
    let (tab, spawned) = reg.get_or_insert_with(session_id, || -> Result<Tab, String> {
        let (data, events) = pending.take().ok_or("channels already used")?;
        let channels = Arc::new(Channels::default());
        channels.attach(data, events);
        let cfg = SpawnConfig { rows, cols, real_zdotdir: None, histfile: dev_histfile() };
        let session = Session::spawn(shim_id, cfg, Box::new(RawSink(channels.clone()))).map_err(io_err)?;
        session.set_event_sink(Arc::new(EventsSink(channels.clone())));
        Ok(Tab {
            id: session_id.to_string(),
            session: Arc::new(session),
            channels,
            agent_control: AtomicBool::new(false),
            owner: Mutex::new(owner),
            seq: next_tab_seq(),
            title: Mutex::new(None),
        })
    })?;
    if let Some((data, events)) = pending.take() {
        reattach_channels(&tab.channels, data, events, || tab.session.reset_flow_control());
    }
    if let Some(chat_id) = owner_for_session(session_id) {
        ensure_chat(chats, chat_id)?;
        assign_tab(reg, chats, chat_id, session_id, true)?;
    }
    Ok(StartResult { spawned, nonce: tab.session.nonce().to_string() })
}

/// Swaps in the re-attached frontend's channels, then resets flow control so bytes the dead channel
/// never acked cannot keep the pump paused. Runs only on re-attach (`pending` is still Some);
/// initial spawn starts with a fresh gate. The reset runs after the swap, so newly rendered output
/// is acked against a clean count.
fn reattach_channels(
    channels: &Arc<Channels>,
    data: Channel<InvokeResponseBody>,
    events: Channel<Value>,
    reset_flow_control: impl FnOnce(),
) {
    channels.attach(data, events);
    reset_flow_control();
}

#[tauri::command]
fn ack(session_id: String, bytes: usize, reg: State<'_, Reg>) -> Result<(), String> {
    tab_of(&reg, &session_id)?.session.ack(bytes);
    Ok(())
}

#[tauri::command]
fn resize(session_id: String, rows: u16, cols: u16, reg: State<'_, Reg>) -> Result<(), String> {
    tab_of(&reg, &session_id)?.session.resize(rows, cols).map_err(io_err)
}

/// Keystrokes from the terminal, as text (sent as UTF-8). The terminal's Ctrl-C also cancels the running agent
/// turn and its pending approvals, but only when it belongs to the agent (see `Chat::interrupt_cancels_turn`): a Ctrl-C
/// typed into the user's own command stays with the shell. A terminal no chat owns is a plain shell.
#[tauri::command]
fn write_user(session_id: String, text: String, reg: State<'_, Reg>, chats: State<'_, ChatReg>) -> Result<(), String> {
    let tab = tab_of(&reg, &session_id)?;
    let Some(chat) = tab.owner_id().and_then(|id| chats.get(&id)) else {
        return tab.session.write_user(text.as_bytes()).map_err(io_err);
    };
    let interrupt = text.contains('\u{3}') && chat.interrupt_cancels_turn(&tab);
    if interrupt {
        chat.cancel_turn();
    }
    agent::user_input(&tab.session, &chat.approvals, &text, interrupt).map_err(io_err)
}

/// Cancels the running agent turn, if any: its pending approvals are denied and its `run_command` stops waiting.
/// Harmless when no turn is running.
#[tauri::command]
fn cancel_agent(chat_id: String, chats: State<'_, ChatReg>) -> Result<(), String> {
    chat_of(&chats, &chat_id)?.cancel_turn();
    Ok(())
}

/// The model for the next turn: the saved default model when there is one, else the environment settings
/// (core/chat/agent_loop), else `None`, which means the mock agent. Returns the settings and the model id.
/// A saved default that cannot be used is an error, not a fallback, so the user sees why.
fn choose_llm(service: Option<&SettingsService>) -> Result<Option<(LlmSettings, String)>, String> {
    if let Some(service) = service
        && let Some(active) = service.active()?
    {
        return Ok(Some((active.settings, active.model)));
    }
    Ok(agent_loop::settings_cloned().map(|env| {
        let model = env.model.clone();
        (env, model)
    }))
}

/// The approval policy for one user message, from the settings in use: the saved ones, or the last working copy's in
/// last-good mode (`SettingsService::approval_settings`). Manual when there is no settings service, when settings.json
/// cannot be used, or when a saved rule does not compile. Built per message, so its per-turn count starts at zero.
fn approval_policy(service: Option<&SettingsService>) -> Box<dyn ApprovalPolicy> {
    service
        .and_then(|s| s.approval_settings().ok())
        .and_then(|approval| approval::policy_for(&approval).ok())
        .unwrap_or_else(|| -> Box<dyn ApprovalPolicy> { Box::new(ManualPolicy) })
}

/// Whether nested shells follow the local approval rules (approval nested = same as local), in the settings in use. False
/// when the settings cannot be used.
fn follows_local(service: Option<&SettingsService>) -> bool {
    service.and_then(|s| s.approval_settings().ok()).is_some_and(|approval| approval.nested == Nested::SameAsLocal)
}

/// The rules one turn decides by, from the settings in use (see `TurnRules`). All are off when the settings cannot be used,
/// so a denied call stops the turn then.
fn turn_rules(service: Option<&SettingsService>) -> TurnRules {
    TurnRules {
        follow_local_in_nested: follows_local(service),
        integrate_without_asking: service
            .and_then(|s| s.integration_settings().ok())
            .is_some_and(|integration| integration.agent_requests == AgentRequests::Auto),
        continue_on_denied: service
            .and_then(|s| s.agent_settings().ok())
            .is_some_and(|agent| agent.on_denied == OnDenied::Continue),
    }
}

/// The per-turn caps (model calls, polling calls, commands, wait minutes) from the settings in use. The defaults when the
/// settings cannot be used.
fn turn_limits(service: Option<&SettingsService>) -> LimitsSettings {
    service.and_then(|s| s.limits_settings().ok()).unwrap_or_default()
}

/// Writes the images of a message to the attachment store, before the message is recorded. Without a store there is
/// nowhere to keep them, so the message is refused.
fn write_images(store: Option<&Persist>, conversation_id: &str, images: &[ImageFile]) -> Result<(), String> {
    if images.is_empty() {
        return Ok(());
    }
    let persist = store.ok_or_else(|| NO_STORE.to_string())?;
    let files = persist.conversations.attachments();
    for image in images {
        files
            .write(conversation_id, &image.sha256, image.ext, &image.bytes)
            .map_err(|e| format!("the attachment was not saved ({e})"))?;
    }
    Ok(())
}

/// Starts one agent turn on its own thread. Events arrive on the events channel. The model is chosen by `choose_llm`,
/// and the approval policy by `approval_policy`. Both are read from the settings before the turn starts, so an error
/// leaves the agent idle. `attachments` are checked first: a refused message changes nothing.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn agent_message(
    chat_id: String,
    text: String,
    attachments: Option<Vec<AttachmentInput>>,
    app: tauri::AppHandle,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
    settings: State<'_, SettingsState>,
) -> Result<(), String> {
    // A chat with no terminal runs its turn too. The project, the folder and the rules come from the default terminal when
    // there is one, and are none otherwise.
    let chat = chat_of(&chats, &chat_id)?;
    let tab = default_tab(&reg, &chat_id);
    // The project instructions come from the terminal's live working directory, read once before the
    // turn starts. Anything unreadable just means no rules; it never fails the turn.
    let project_root = tab.as_deref().and_then(|tab| session_project_root(&app, tab));
    // The folder the file tools read this turn (core/fs_tools), from the same live directory.
    let fs_scope = tab.as_deref().and_then(|tab| fs_scope_for(&app, tab, project_root.clone()));
    let project_rooted_rules =
        project_root.as_ref().and_then(|root| instructions::load(root).map(|rules| (root.clone(), rules)));
    let project_rules = project_rooted_rules.as_ref().map(|(_, rules)| rules.clone());
    let prepared = prepare(&attachments.unwrap_or_default())?;
    // Reading a saved model touches the Keychain, so it runs on the blocking pool.
    let service = settings.0.clone();
    let (chosen, policy, rules, limits) = blocking(move || {
        let policy = approval_policy(service.as_deref());
        let rules = turn_rules(service.as_deref());
        let limits = turn_limits(service.as_deref());
        Ok((choose_llm(service.as_deref())?, policy, rules, limits))
    })
    .await?;
    let cancel = Arc::new(TurnCancel::new());
    let (history, todos, conversation_id) = chat.begin_turn(&cancel)?;
    // Opened for an agent turn only: side commands also take the slot through `begin_turn`, and never get a mailbox.
    chat.steer.open();
    // Announced once per root and rules text: a repeat stays quiet, and no file means no notice.
    // Never stored in the history; the rules themselves ride the turn's system message only.
    if let Some((root, rules)) = project_rooted_rules.as_ref() {
        let seen = (root.display().to_string(), rules.clone());
        if lock(&chat.project_rules_seen).as_ref() != Some(&seen) {
            *lock(&chat.project_rules_seen) = Some(seen);
            chat.send_agent(&AgentEvent::Notice {
                text: format!("Using project instructions from {}/AGENTS.md", root.display()),
            });
        }
    }
    let store = persist.0.clone();
    // The MCP tools of this turn (LLM turns only). Set below when the servers are available.
    let mcp_service = app.state::<McpState>().0.clone();
    let mut mcp_turn: Option<TurnMcp> = None;
    // Only the LLM agent writes history, so only its turns are saved. The mock agent never touches the history, and it
    // ignores attachments.
    let (mut llm, saved) = match chosen {
        None => (None, None),
        Some((llm_settings, model)) => match agent_loop::shared_client() {
            Ok(client) => {
                // The images are on disk before the message is recorded, so a saved message never names a missing file.
                if let Err(e) = write_images(store.as_deref(), &conversation_id, &prepared.images) {
                    chat.end_turn(&Touched::default());
                    return Err(e);
                }
                let state = Arc::new(SavedState {
                    conversation_id: conversation_id.clone(),
                    history: history.clone(),
                    todos: todos.clone(),
                    model: Some(model),
                    context: chat.conversation.last_usage(),
                    remember: remembers_last(&chat.id),
                });
                let checkpoint = checkpoint_saver(store.clone(), tab.clone(), state.clone());
                // The servers start on the first turn that needs them. If they cannot be read, this turn runs without MCP
                // tools, and the user is told why.
                if let Some(service) = mcp_service.clone() {
                    match blocking(move || service.for_turn()).await {
                        Ok(turn) => mcp_turn = Some(turn),
                        Err(e) => {
                            chat.send_agent(&AgentEvent::Notice {
                                text: format!("MCP tools are not available this turn: {e}"),
                            });
                        }
                    }
                }
                // The skills of this turn (core/skills), read from the folders on the blocking pool. A scan never fails the turn.
                let setup = skill_setup(&app, project_root);
                let skill_index = Arc::new(blocking(move || Ok(setup.load().0)).await.unwrap_or_else(|_| skills::builtin_index()));
                let live_state = state_provider(&app, skill_index.clone());
                let mut agent = LlmAgent::new(client, llm_settings, &conversation_id, history, todos, cancel.clone())
                    // The note is taken only by a turn that calls the model: a turn that fails to start keeps it.
                    .with_resume_note(chat.conversation.take_resume_note())
                    .with_project_rules(project_rules)
                    .with_skills(skill_index)
                    .with_state(live_state)
                    .with_fs_scope(fs_scope)
                    .with_checkpoint(checkpoint)
                    .with_limits(limits)
                    .with_no_vision(chat.conversation.no_vision())
                    .with_calibration(chat.conversation.calibration())
                    .with_learned_limits(chat.conversation.learned_limits())
                    .with_last_usage(chat.conversation.last_usage())
                    .with_offered_tools(chat.conversation.offered_tools())
                    .with_extra_secrets(mcp_turn.as_ref().map_or_else(Vec::new, |turn| turn.secret_values.clone()));
                if let Some(persist) = store.as_ref() {
                    agent = agent.with_attachments(persist.conversations.attachments().clone());
                }
                (Some(agent), Some(state))
            }
            Err(e) => {
                chat.end_turn(&Touched::default());
                return Err(e);
            }
        },
    };
    let message = UserMessage { text, attachments: prepared.attachments };
    let worker = chat.clone();
    let worker_tab = tab.clone();
    let settle_cancel = cancel.clone();
    let terminals = chat_terminals(app.clone(), chat_id.clone());
    let lookup = app.clone();
    let touched: Arc<Touched> = Arc::new(Mutex::new(Vec::new()));
    let spawned = thread::Builder::new().name("agent".into()).spawn(move || {
        // Runs on panic too, so a crashed turn cannot leave the agent busy or in control of a shell. Fired below once
        // the history is saved; after that it does nothing, so it cannot end a newer turn.
        let mut end = OnDrop::new(|| worker.end_turn(&touched));
        let tools = ChatTerminals::new(terminals, limits.max_wait()).with_cancel(cancel);
        let events = worker.clone();
        let control = worker_tab.clone();
        let touch = touched.clone();
        let emit = move |ev: AgentEvent| {
            if let AgentEvent::AgentControl { active, terminal } = &ev {
                // The control is of the terminal the event names (two or more terminals), else of the default one.
                let target = match terminal {
                    Some(id) => tab_of(&lookup.state::<Reg>(), id).ok(),
                    None => control.clone(),
                };
                if let Some(target) = target {
                    target.agent_control.store(*active, Ordering::SeqCst);
                    let mut seen = lock(&touch);
                    if *active && !seen.iter().any(|(known, _)| Arc::ptr_eq(known, &target)) {
                        seen.push((target, terminal.clone()));
                    }
                }
            }
            events.send_agent(&ev);
        };
        // The turn that starts the servers waits for them to connect, so their tools are offered this turn.
        if let Some(turn) = mcp_turn.as_ref() {
            turn.settle(&settle_cancel);
        }
        let base = AgentCtx::new(&tools, &worker.approvals, &emit)
            .with_policy(policy.as_ref())
            .with_rules(rules)
            .with_steer(&worker.steer);
        let ctx = match mcp_turn.as_ref() {
            Some(turn) => base.with_external_tools(&*turn.manager, &settle_cancel),
            None => base,
        };
        match llm.as_mut() {
            Some(agent) => agent::run_reply(agent, &message, &ctx),
            None => agent::run_reply(&mut MockAgent, &message, &ctx),
        }
        // The history is final now. The end of a turn is saved as finished, whether it completed, was cancelled, or
        // failed. Then the turn is ended (frees the slot), and only then `Done` is sent: a message the user sends on the
        // back of `Done` finds the agent free. The guard above is fired here, so it does not end the turn a second time.
        if let (Some(agent), Some(store), Some(state)) = (llm.as_ref(), store.as_ref(), saved.as_ref()) {
            save_conversation(store, worker_tab.as_ref().and_then(|tab| tab.session.cwd()), state, agent.redactor(), false);
        }
        end.fire();
        emit(AgentEvent::Done);
    });
    if let Err(e) = spawned {
        *lock(&chat.turn_cancel) = None;
        chat.return_steers();
        chat.agent_busy.store(false, Ordering::SeqCst);
        return Err(e.to_string());
    }
    Ok(())
}

/// Sends a message into the running agent turn (steering): it goes in at the turn's next step, after the tool call that is
/// running (a running call is not interrupted). `mode` is "all" (the default: every waiting message in one) or "one" (the
/// first alone). Returns true when the message was accepted. False means it was not: no agent turn is running, the turn is
/// ending, or a side command holds the slot. The caller then queues the message as it does otherwise. Never starts a turn.
/// The attachments are checked and stored as `agent_message` does it.
#[tauri::command]
async fn steer_message(
    chat_id: String,
    id: u64,
    text: String,
    attachments: Option<Vec<AttachmentInput>>,
    mode: Option<String>,
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
) -> Result<bool, String> {
    let chat = chat_of(&chats, &chat_id)?;
    let mode = SteerMode::parse(mode.as_deref().unwrap_or("all"))?;
    let prepared = prepare(&attachments.unwrap_or_default())?;
    if !chat.steer.is_open() {
        return Ok(false);
    }
    // The images are on disk before the message can be taken into the history (see `agent_message`).
    write_images(persist.0.as_deref(), &chat.conversation.id(), &prepared.images)?;
    let item = SteerItem { id, text, attachments: prepared.attachments };
    Ok(chat.steer.accept(item, mode))
}

/// Takes a steered message back before the turn takes it. True when it was still waiting, so the caller restores it to the
/// composer. False when it was already taken into the turn (or was never accepted).
#[tauri::command]
fn withdraw_steer(chat_id: String, id: u64, chats: State<'_, ChatReg>) -> Result<bool, String> {
    Ok(chat_of(&chats, &chat_id)?.steer.withdraw(id))
}

/// Starts a new conversation: the history is cleared and the id rotated. Refused while the agent is working. Takes
/// the turn-cancel lock, as `begin_turn` does, so the check and the reset cannot interleave with a turn starting. The
/// remembered last conversation is then forgotten; a failure to forget it is logged, not returned.
#[tauri::command]
async fn new_conversation(
    chat_id: String,
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
) -> Result<(), String> {
    let chat = chat_of(&chats, &chat_id)?;
    {
        let _turn = lock(&chat.turn_cancel);
        if chat.agent_busy.load(Ordering::SeqCst) {
            return Err(BUSY_REFUSAL.into());
        }
        chat.conversation.reset().map_err(io_err)?;
    }
    // Without a store there is no remembered conversation to forget. The remembered one is chat `main`'s (see `load_conversation`).
    if chat.id == MAIN && let Some(store) = persist.0.clone() {
        let forgot = blocking(move || store.forget_last().map_err(|e| e.to_string())).await;
        if let Err(e) = forgot {
            eprintln!("watt: the last conversation was not forgotten ({e})");
        }
    }
    Ok(())
}

#[tauri::command]
fn approve_tool(chat_id: String, call_id: String, approve: bool, chats: State<'_, ChatReg>) -> Result<(), String> {
    if chat_of(&chats, &chat_id)?.approvals.decide(&call_id, approve) {
        Ok(())
    } else {
        Err(format!("no pending approval {call_id}"))
    }
}

/// Answers the dialog for a `request_integration` call: `accept` integrates the shell, anything else declines it.
#[tauri::command]
fn answer_integration(chat_id: String, call_id: String, accept: bool, chats: State<'_, ChatReg>) -> Result<(), String> {
    if chat_of(&chats, &chat_id)?.approvals.integrations().decide(&call_id, accept) {
        Ok(())
    } else {
        Err(format!("no pending integration request {call_id}"))
    }
}

/// Answers a waiting `ask_user` call. Invalid answers are refused with the reason, so the UI can fix them and retry;
/// an unknown call id means the question already settled (answered, timed out or cancelled).
#[tauri::command]
fn answer_questions(
    chat_id: String,
    call_id: String,
    answers: Vec<questions::AnswerInput>,
    chats: State<'_, ChatReg>,
) -> Result<(), String> {
    chat_of(&chats, &chat_id)?.approvals.questions().decide(&call_id, answers)
}

#[tauri::command]
fn set_auto_approve(chat_id: String, on: bool, reg: State<'_, Reg>, chats: State<'_, ChatReg>) -> Result<(), String> {
    // The shell on top is the level approve-all is tied to. The terminal supplies it, not the frontend.
    let (chat, tab) = chat_and_default(&reg, &chats, &chat_id)?;
    chat.approvals.set_auto_approve(on, top_key_of(tab.as_deref()));
    Ok(())
}

/// Approve-all as the shell on top sees it: `auto` is on (in whichever shell), `suspended` is on but was switched on in
/// another shell level, so it does not apply here.
#[derive(Serialize)]
struct AutoApproveState {
    auto: bool,
    suspended: bool,
}

#[tauri::command]
fn auto_approve_state(
    chat_id: String,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
    settings: State<'_, SettingsState>,
) -> Result<AutoApproveState, String> {
    let (chat, tab) = chat_and_default(&reg, &chats, &chat_id)?;
    let state = chat.approvals.auto_state_across(top_key_of(tab.as_deref()), follows_local(settings.0.as_deref()));
    Ok(AutoApproveState { auto: state != AutoState::Off, suspended: state == AutoState::Suspended })
}

/// Whether the LLM agent is configured, which model and variant, and from which saved provider. `provider` and
/// `variant` are null for the environment settings. Never the key or the URL.
#[derive(Serialize)]
struct LlmStatus {
    enabled: bool,
    model: Option<String>,
    provider: Option<String>,
    variant: Option<String>,
}

#[derive(Serialize)]
struct Status {
    state: InputState,
    /// The foreground program hint while a command runs (core/foreground.rs). Left out when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    foreground: Option<Foreground>,
    /// The nested shell levels, outermost first. The local shell is not listed. Empty when there is none.
    levels: Vec<LevelInfo>,
    /// How many shell levels the session holds, the local one included. 1 means no nested shell.
    depth: usize,
    integration: IntegrationStatus,
    observer: ObserverStatus,
    cwd: Option<Cwd>,
    stats: SessionStats,
    auto_approve: bool,
    /// Approve-all is on but was switched on in another shell level, so it does not apply to the shell on top.
    auto_approve_suspended: bool,
    /// Every nonce the observer accepts (the local shell's and the live levels'), so a reloaded window can check OSC events.
    nonces: Vec<String>,
    agent_busy: bool,
    agent_control: bool,
    llm: LlmStatus,
    /// The live conversation's id (the one the provider sees, and the file name of its saved copy).
    conversation_id: String,
    /// The state of settings.json. Null when the app folders could not be found.
    settings_state: Option<SettingsStateView>,
}

/// The environment settings as the status shows them.
fn env_llm() -> LlmStatus {
    let env = agent_loop::settings();
    LlmStatus { enabled: env.is_some(), model: env.map(|s| s.model.clone()), provider: None, variant: None }
}

/// The terminal `get_status` reads the shell fields from: the one `session_id` names (unknown is "no session"), or else
/// the chat's default. None when the chat has no terminal.
fn status_shell(reg: &Reg, chat_id: &str, session_id: Option<&str>) -> Result<Option<Arc<Tab>>, String> {
    match session_id {
        Some(id) => tab_of(reg, id).map(Some),
        None => Ok(default_tab(reg, chat_id)),
    }
}

#[tauri::command]
fn get_status(
    chat_id: String,
    session_id: Option<String>,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
    settings: State<'_, SettingsState>,
) -> Result<Status, String> {
    // Chat fields come from the chat, terminal fields from the terminal `session_id` names, else from its default terminal.
    let chat = chat_of(&chats, &chat_id)?;
    let tab = status_shell(&reg, &chat_id, session_id.as_deref())?;
    // The saved default's label reads no secret. Without a saved default the environment settings are shown. While
    // settings.json cannot be used, the agent is off and the environment settings are not shown in its place.
    let (llm, settings_state) = match settings.0.as_ref() {
        Some(service) => {
            let llm = match service.default_label() {
                Ok(Some((provider, model, variant))) => {
                    LlmStatus { enabled: true, model: Some(model), provider: Some(provider), variant }
                }
                Ok(None) => env_llm(),
                Err(_) => LlmStatus { enabled: false, model: None, provider: None, variant: None },
            };
            (llm, Some(service.settings_state()))
        }
        None => (env_llm(), None),
    };
    // A chat with no terminal has no shell: the shell fields are those of an exited, empty session with no levels.
    let shell = tab.as_deref();
    let levels = shell.map(|tab| tab.session.levels()).unwrap_or_default();
    let depth = levels.len();
    Ok(Status {
        state: shell.map_or(InputState::Exited, |tab| tab.session.state()),
        foreground: shell.and_then(|tab| tab.session.foreground()),
        levels: levels.into_iter().skip(1).collect(),
        depth,
        integration: shell.map_or(IntegrationStatus::Raw, |tab| tab.session.integration_status()),
        observer: shell.map_or(ObserverStatus::Ok, |tab| tab.session.observer_status()),
        cwd: shell.and_then(|tab| tab.session.cwd()),
        stats: shell.map_or(
            SessionStats { forged_events: 0, stale_events: 0, overflow_events: 0, dropped_bytes: 0 },
            |tab| tab.session.stats(),
        ),
        auto_approve: chat.approvals.auto_approve(),
        auto_approve_suspended: chat.approvals.auto_state_across(top_key_of(shell), follows_local(settings.0.as_deref()))
            == AutoState::Suspended,
        nonces: shell.map(|tab| tab.session.nonces()).unwrap_or_default(),
        agent_busy: chat.agent_busy.load(Ordering::SeqCst),
        agent_control: shell.is_some_and(|tab| tab.agent_control.load(Ordering::SeqCst)),
        llm,
        conversation_id: chat.conversation.id(),
        settings_state,
    })
}

/// The slash-command catalog: custom prompt templates from `<config>/commands` (global) and
/// `<workspace>/.watt/commands` (project), plus the two directories so `/help` can show where to
/// create them. The project directory comes from the session's live working directory; it is skipped
/// for shells on another machine, whose filesystem cannot be read. Missing directories are not an error.
#[tauri::command]
fn list_commands(
    session_id: String,
    app: tauri::AppHandle,
    reg: State<'_, Reg>,
) -> Result<CommandCatalog, String> {
    let global_dir = resolve_config_dir(&app).map(|dir| dir.join("commands"));
    let project_dir = tab_of(&reg, &session_id)?.session.cwd().and_then(|cwd| project_commands_dir(&app, &cwd));
    Ok(commands::catalog(project_dir.as_deref(), global_dir.as_deref()))
}

/// The workspace root for a shell working directory. None without a usable root, or when the
/// shell runs on another machine, whose filesystem cannot be read.
fn workspace_root_for_cwd(app: &tauri::AppHandle, cwd: &Cwd) -> Option<PathBuf> {
    if !is_local_cwd(cwd) {
        return None;
    }
    let home = app.path().home_dir().ok();
    let workspace =
        workspace_for(Some((cwd.host.as_str(), cwd.path.as_str())), local_host(), home.as_deref(), &[]);
    if workspace.root.is_empty() {
        return None;
    }
    Some(PathBuf::from(workspace.root))
}

/// What the skills of a session are read from (core/skills): the places folder settings resolve against, the skill settings
/// in use, and a warning when they cannot be used (the default folders are then read).
struct SkillSetup {
    places: Places,
    settings: SkillsSettings,
    warning: Option<String>,
}

impl SkillSetup {
    /// The skills and the folder layout. Reads the folders, so it runs on the blocking pool.
    fn load(&self) -> (skills::SkillIndex, skills::Layout) {
        let (mut index, layout) = skills::load(&self.places, &self.settings);
        if let Some(warning) = &self.warning {
            index.warnings.insert(0, warning.clone());
        }
        (index, layout)
    }
}

/// The skill setup of a session: the Watt config, home and guarded folders, and the session's project root for relative
/// folders. The settings are the ones in use.
fn skill_setup(app: &tauri::AppHandle, project_root: Option<PathBuf>) -> SkillSetup {
    let (settings, warning) = match app.state::<SettingsState>().get().and_then(|s| s.skills_settings()) {
        Ok(settings) => (settings, None),
        Err(e) => (SkillsSettings::default(), Some(format!("skill folder settings are not used ({e}); the default folders are read"))),
    };
    SkillSetup { places: skill_places(app, project_root), settings, warning }
}

/// The live `state` file of the built-in skill (core/skills/state): the settings as they are when the file is read, the MCP
/// servers' status, and the skills of the turn. It reads the settings and the MCP status only (no server starts, no secret
/// value is read; the settings view checks which keys are set). Called on the agent's thread, once per `state` read.
fn state_provider(app: &tauri::AppHandle, index: Arc<skills::SkillIndex>) -> skills::tool::StateProvider {
    let settings = app.state::<SettingsState>().0.clone();
    let mcp = app.state::<McpState>().0.clone();
    let config_dir = resolve_config_dir(app);
    let data_dir = resolve_data_dir(app);
    let home = home_dir();
    Arc::new(move || {
        let view = settings.as_ref().and_then(|s| s.view().ok());
        let mcp_status = mcp.as_ref().map(|m| m.status()).unwrap_or_default();
        skills::state::render(&skills::state::Input {
            settings: view.as_ref(),
            mcp: &mcp_status,
            skills: &index,
            app_version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            config_dir: config_dir.as_deref(),
            data_dir: data_dir.as_deref(),
            home: home.as_deref(),
        })
    })
}

/// The places skill folders resolve against and are refused against. Watt's config, data and secrets folders are guarded
/// like the file tools guard them (see `guarded_paths`), and so is the secrets file's folder.
fn skill_places(app: &tauri::AppHandle, project_root: Option<PathBuf>) -> Places {
    let mut guarded = guarded_paths(app);
    guarded.extend(secrets_file_path().and_then(|p| p.parent().map(Path::to_path_buf)).filter(|p| !p.as_os_str().is_empty()));
    Places { config: resolve_config_dir(app), home: home_dir(), project: project_root, guarded }
}

/// Watt's own folders and the secrets file: the paths the file tools and the skill folders never read.
fn guarded_paths(app: &tauri::AppHandle) -> Vec<PathBuf> {
    [resolve_config_dir(app), resolve_data_dir(app), secrets_file_path()].into_iter().flatten().collect()
}

/// The project root of a session: the workspace of the local shell's directory. None while a nested shell is on top, since
/// its directory is on another machine.
fn session_project_root(app: &tauri::AppHandle, tab: &Tab) -> Option<PathBuf> {
    tab.session.local_cwd().and_then(|cwd| workspace_root_for_cwd(app, &cwd))
}

/// Whether a shell working directory is on this machine. A directory reported by another host cannot be read here.
fn is_local_cwd(cwd: &Cwd) -> bool {
    cwd.host.is_empty() || cwd.host == "localhost" || cwd.host == local_host()
}

/// The folder the file tools (core/fs_tools) may read in a turn: the session's project root, else its local directory. None
/// while a nested shell is on top (its directory is on another machine), or when the local directory is not on this machine.
fn fs_scope_for(app: &tauri::AppHandle, tab: &Tab, project_root: Option<PathBuf>) -> Option<Scope> {
    let cwd = tab.session.local_cwd().filter(is_local_cwd)?;
    let local = PathBuf::from(&cwd.path);
    let root = project_root.unwrap_or_else(|| local.clone());
    // Watt's own folders and the secrets file: the file tools never read or write them, whatever the paths are.
    Some(Scope::new(root, local, guarded_paths(app)))
}

/// The skills for the settings view (core/skills): the folders in use, the skills in them and the ones switched off. The
/// project's folders are read when the session's local shell is on top. Read on the blocking pool; a missing folder is not
/// an error.
#[tauri::command]
async fn list_skills(chat_id: String, app: tauri::AppHandle, reg: State<'_, Reg>, chats: State<'_, ChatReg>) -> Result<SkillsView, String> {
    let tab = chat_and_default(&reg, &chats, &chat_id)?.1;
    blocking(move || {
        let project_root = tab.as_deref().and_then(|tab| session_project_root(&app, tab));
        let setup = skill_setup(&app, project_root);
        let (index, layout) = setup.load();
        Ok(skills::view(&index, &layout, &setup.settings))
    })
    .await
}

/// Saves the skill folders and the switched-off skills. Refused while settings.json is broken or in last-good mode, and when a
/// folder is refused: Watt's own folders, the secrets file's folder, or a credential folder (see core/skills/folders).
#[tauri::command]
async fn set_skills_settings(
    input: SkillsSettings,
    app: tauri::AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<SettingsView, String> {
    let service = settings.get()?;
    let places = skill_places(&app, None);
    blocking(move || {
        if let Some(folders) = &input.folders {
            let refused = skills::folders::refusals(&places, folders);
            if !refused.is_empty() {
                return Err(refused.join("; "));
            }
        }
        service.set_skills(input)
    })
    .await
}

/// The project's command directory for a shell working directory: `<workspace root>/.watt/commands`.
/// None without a usable root, or when the shell runs on another machine.
fn project_commands_dir(app: &tauri::AppHandle, cwd: &Cwd) -> Option<PathBuf> {
    workspace_root_for_cwd(app, cwd).map(|root| root.join(".watt").join("commands"))
}

/// Integrates the shell in the foreground (ssh, a nested shell, a container exec) as a new level. The handshake waits for
/// the shell, so it runs on the blocking pool. See `Session::integrate_shell`. Called from the UI's Integrate dialog;
/// the agent's `request_integration` tool integrates through the session instead (see `SessionTools::integrate_shell`).
#[tauri::command]
async fn integrate_shell(session_id: String, reg: State<'_, Reg>) -> Result<IntegrateOutcome, String> {
    let tab = tab_of(&reg, &session_id)?;
    blocking(move || Ok(tab.session.integrate_shell())).await
}

/// The saved conversations, most recently updated first.
#[tauri::command]
async fn list_conversations(persist: State<'_, PersistState>) -> Result<Vec<ConversationMeta>, String> {
    let store = persist.get()?;
    blocking(move || store.conversations.list().map_err(|e| e.to_string())).await
}

#[derive(Serialize)]
struct LoadedConversation {
    id: String,
    title: String,
    messages: Vec<Value>,
    /// The todo list the conversation ended with (repaired). Empty when it had none.
    todos: Vec<Todo>,
    /// The context usage the conversation ended with, or null when none was saved (or it was not sane).
    context: Option<ContextUsage>,
}

/// A saved conversation as the live one. The file is read and repaired first. Then, when the agent is idle, the live
/// conversation takes its id and history, and the id is remembered as the last conversation. Returns the repaired
/// messages, so the UI is built from exactly what the backend now holds. Refused while the agent is working.
#[tauri::command]
async fn load_conversation(
    chat_id: String,
    id: String,
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
    claims: State<'_, Claims>,
) -> Result<LoadedConversation, String> {
    let chat = chat_of(&chats, &chat_id)?;
    let store = persist.get()?;
    let read_store = store.clone();
    let read_id = id.clone();
    let resumed = blocking(move || resume(&read_store, &read_id).map_err(|e| e.to_string())).await?;
    let messages = resumed.history.snapshot();
    let title = resumed.title;
    let todos = resumed.todos;
    let in_turn = resumed.in_turn;
    let context = resumed.context;
    {
        // The claim and the restore are one step under the claims lock, so two chats cannot both take one conversation
        // (their histories would diverge under one id). Then the busy check, under the turn-cancel lock, as `begin_turn`
        // and `new_conversation` do.
        let _claim = lock(&claims.0);
        if held_by_other(&chats.snapshot(), &chat.id, &id) {
            return Err("that conversation is open in another chat".into());
        }
        let _turn = lock(&chat.turn_cancel);
        if chat.agent_busy.load(Ordering::SeqCst) {
            return Err(BUSY_REFUSAL.into());
        }
        chat.conversation.restore(id.clone(), resumed.history, todos.clone(), in_turn);
        chat.conversation.set_last_usage(context.clone());
    }
    // The remembered last conversation is one value for the app, so only chat `main` writes it.
    if chat.id == MAIN {
        let remember_id = id.clone();
        let remember = blocking(move || {
            let patch = UiStatePatch { last_conversation_id: Some(Some(remember_id)), ..UiStatePatch::default() };
            store.ui.patch(patch).map(|_| ()).map_err(|e| e.to_string())
        });
        if let Err(e) = remember.await {
            eprintln!("watt: the last conversation was not remembered ({e})");
        }
    }
    Ok(LoadedConversation { id, title, messages, todos, context })
}

/// One stored image of the session's open conversation, as a `data:` URL, for the image viewer. The conversation is the
/// session's own, never the webview's choice; the address and type are checked before any file is read.
#[tauri::command]
async fn get_attachment(chat_id: String, sha256: String, mime: String, chats: State<'_, ChatReg>, persist: State<'_, PersistState>) -> Result<String, String> {
    let chat = chat_of(&chats, &chat_id)?;
    let store = persist.get()?;
    let conversation_id = chat.conversation.id();
    blocking(move || stored_image_url(store.conversations.attachments(), &conversation_id, &sha256, &mime)).await
}

/// What a derived conversation (compact/fork) hands to `save_side`: its id, model-facing history, todo list, model
/// and context usage.
struct SideSave {
    id: String,
    messages: Vec<Value>,
    todos: Vec<Todo>,
    model: Option<String>,
    context: Option<ContextUsage>,
    /// Whether the save remembers the conversation as the last one (see `remembers_last`).
    remember: bool,
}

/// Saves a derived conversation (compact/fork) via `save_turn`, never a raw store save: redacted, pruned and
/// remembered as the last conversation like a finished turn, but marked finished (`in_turn: false`). `None` from
/// `save_turn` (no user message) is an error; the callers refuse empty histories first.
fn save_side(
    store: &Persist,
    cwd: Option<(String, String)>,
    save: SideSave,
    redactor: &Redactor,
) -> Result<ConversationMeta, String> {
    let pair = cwd.as_ref().map(|(host, path)| (host.as_str(), path.as_str()));
    let turn = FinishedTurn {
        conversation_id: &save.id,
        messages: save.messages,
        todos: save.todos,
        model: save.model,
        cwd: pair,
        in_turn: false,
        context: save.context,
        remember: save.remember,
    };
    let home = home_dir();
    let env = TurnEnv { local_host: local_host(), home: home.as_deref() };
    save_turn(store, turn, redactor, env, now_secs())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the conversation has no user message".to_string())
}

/// A side question about the live conversation: one model call outside a turn, never touching stored history or the
/// transcript. Reserves the turn slot, so a second call is refused while one runs, and Ctrl-C/`cancel_agent` aborts
/// it through the reserved turn's cancel token.
#[tauri::command]
async fn ask_btw(
    chat_id: String,
    text: String,
    chats: State<'_, ChatReg>,
    settings: State<'_, SettingsState>,
) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("Usage: /btw <question>".into());
    }
    let chat = chat_of(&chats, &chat_id)?;
    let cancel = Arc::new(TurnCancel::new());
    let (history, _, old_id) = chat.begin_turn(&cancel)?;
    // Ends the turn on every return; dropped explicitly before success.
    let end = OnDrop::new({ let chat = chat.clone(); move || chat.end_turn(&Touched::default()) });
    // The snapshot leaves the locks before any await.
    let snapshot = lock(&history).snapshot();
    // Reading a saved model touches the Keychain, so it runs on the blocking pool.
    let service = settings.0.clone();
    let (llm_settings, client) = blocking(move || {
        let chosen =
            choose_llm(service.as_deref())?.ok_or_else(|| "No model is configured. Add one in Settings.".to_string())?;
        let client = agent_loop::shared_client().cloned()?;
        Ok((chosen.0, client))
    })
    .await?;
    let mut messages = side::conversation_context(&snapshot, side::budget_chars(llm_settings.context_limit, 1200));
    messages.push(json!({"role": "user", "content": text}));
    let answer = SideCall::new(&client, &llm_settings, &old_id, 1200, cancel)
        .ask(side::BTW_SYSTEM, messages, Duration::from_secs(90))
        .await;
    drop(end);
    answer
}

/// Suggests a title for the live conversation: one model call outside a turn, never touching stored history or
/// the transcript. Reserves the turn slot, so a second call is refused while one runs, and Ctrl-C/`cancel_agent`
/// aborts it through the reserved turn's cancel token. Refused when the history has no user message yet; an empty
/// reply or an unusable title falls back to the derived first-message title instead of failing.
#[tauri::command]
async fn suggest_name(
    chat_id: String,
    chats: State<'_, ChatReg>,
    settings: State<'_, SettingsState>,
) -> Result<String, String> {
    let chat = chat_of(&chats, &chat_id)?;
    let cancel = Arc::new(TurnCancel::new());
    let (history, _, old_id) = chat.begin_turn(&cancel)?;
    // Ends the turn on every return; dropped explicitly before success.
    let end = OnDrop::new({ let chat = chat.clone(); move || chat.end_turn(&Touched::default()) });
    // The snapshot leaves the locks before any await.
    let snapshot = lock(&history).snapshot();
    let limit = lock(&chat.conversation.last_usage()).clone().and_then(|u| u.limit);
    let context = side::conversation_context(&snapshot, side::budget_chars(limit, 64));
    if context.is_empty() {
        return Err("There is nothing to name yet.".into());
    }
    // Reading a saved model touches the Keychain, so it runs on the blocking pool.
    let service = settings.0.clone();
    let (llm_settings, client) = blocking(move || {
        let chosen =
            choose_llm(service.as_deref())?.ok_or_else(|| "No model is configured. Add one in Settings.".to_string())?;
        let client = agent_loop::shared_client().cloned()?;
        Ok((chosen.0, client))
    })
    .await?;
    let mut messages = context;
    messages.push(json!({"role": "user", "content": "Name this conversation."}));
    let reply = SideCall::new(&client, &llm_settings, &old_id, 1024, cancel)
        .ask(side::TITLE_SYSTEM, messages, Duration::from_secs(90))
        .await;
    drop(end);
    let fallback = conversations::derive_title(&snapshot);
    side::title_or_fallback(reply, fallback)
}

/// Suggests a few follow-up prompts: what the user might send next, after the latest reply. One read-only model call:
/// it does not reserve the turn slot, so it never blocks a send or a turn, and it never touches the agent's busy
/// state, the turn cancel, the stored history, the transcript or the saved conversation. Its own cancel token is never
/// registered, so Ctrl-C does not reach it; the 20-second timeout bounds it. No conversation text yet gives an empty
/// list, and so does an empty reply. Errors are returned, and the frontend ignores them.
#[tauri::command]
async fn suggest_prompts(
    chat_id: String,
    chats: State<'_, ChatReg>,
    settings: State<'_, SettingsState>,
) -> Result<Vec<String>, String> {
    let chat = chat_of(&chats, &chat_id)?;
    let old_id = chat.conversation.id();
    // The snapshot leaves the lock before any await.
    let history = chat.conversation.history();
    let snapshot = lock(&history).snapshot();
    let limit = lock(&chat.conversation.last_usage()).clone().and_then(|u| u.limit);
    let mut messages = side::conversation_context(&snapshot, side::budget_chars(limit, 200));
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    // Reading a saved model touches the Keychain, so it runs on the blocking pool.
    let service = settings.0.clone();
    let (llm_settings, client) = blocking(move || {
        let chosen =
            choose_llm(service.as_deref())?.ok_or_else(|| "No model is configured. Add one in Settings.".to_string())?;
        let client = agent_loop::shared_client().cloned()?;
        Ok((chosen.0, client))
    })
    .await?;
    messages.push(json!({"role": "user", "content": "Suggest the next messages I might send."}));
    let reply = SideCall::new(&client, &llm_settings, &old_id, 200, Arc::new(TurnCancel::new()))
        .ask(side::SUGGEST_SYSTEM, messages, Duration::from_secs(20))
        .await;
    side::suggestions_or_empty(reply)
}

/// Replaces the live conversation with a summary of itself: one model call outside a turn, saved as a new conversation
/// and adopted atomically. The old conversation stays untouched in storage. Refused while busy, and when the history
/// has no user message yet.
#[tauri::command]
async fn compact_conversation(
    chat_id: String,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
    settings: State<'_, SettingsState>,
) -> Result<LoadedConversation, String> {
    // The save records the terminal's directory, so the chat's default terminal is needed too.
    let (chat, tab) = chat_and_default(&reg, &chats, &chat_id)?;
    let store = persist.get()?;
    let cancel = Arc::new(TurnCancel::new());
    let (history, todos, old_id) = chat.begin_turn(&cancel)?;
    // Ends the turn on every return; dropped explicitly before success.
    let end = OnDrop::new({ let chat = chat.clone(); move || chat.end_turn(&Touched::default()) });
    // The snapshot leaves the locks before any await.
    let snapshot = lock(&history).snapshot();
    let current_todos = lock(&todos).clone();
    if !snapshot.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("user")) {
        return Err("Nothing to compact yet.".into());
    }
    // Reading a saved model touches the Keychain and the store reads files, so both run on the blocking pool.
    let service = settings.0.clone();
    let reader = store.clone();
    let reader_id = old_id.clone();
    let (llm_settings, model, old_title, client) = blocking(move || {
        let chosen =
            choose_llm(service.as_deref())?.ok_or_else(|| "No model is configured. Add one in Settings.".to_string())?;
        let old_title = reader.conversations.load(&reader_id).ok().map(|m| m.title);
        let client = agent_loop::shared_client().cloned()?;
        Ok((chosen.0, chosen.1, old_title, client))
    })
    .await?;
    let old_title = old_title.unwrap_or_else(|| conversations::derive_title(&snapshot));
    let input = side::summarizer_input(&snapshot, side::budget_chars(llm_settings.context_limit, 1024));
    let request = format!(
        "Summarize this conversation for context compaction: goals, decisions, files/commands touched, current state, open questions.\n\n{input}"
    );
    let summary = SideCall::new(&client, &llm_settings, &old_id, 1024, cancel)
        .ask(side::SUMMARY_SYSTEM, vec![json!({"role": "user", "content": request})], Duration::from_secs(90))
        .await?;
    let new_id = agent_loop::new_conversation_id().map_err(io_err)?;
    let mut seeded = History::new();
    seeded.push_user_content(Value::String(side::compact_seed(&summary)));
    let messages = seeded.snapshot();
    // The header values resolve with the old conversation id, as the side call did.
    let redactor = llm_settings.redactor(&old_id);
    let cwd = tab.as_ref().and_then(|tab| tab.session.cwd()).map(|c| (c.host, c.path));
    let saver = store.clone();
    let saver_id = new_id.clone();
    let saver_messages = messages.clone();
    let remember = remembers_last(&chat.id);
    let saver_todos = current_todos.clone();
    let title = side::suffixed_title(&old_title, " (compacted)");
    let saved_title = blocking(move || {
        let save = SideSave {
            id: saver_id.clone(),
            messages: saver_messages,
            todos: saver_todos,
            model: Some(model),
            context: None,
            remember,
        };
        let meta = save_side(&saver, cwd, save, &redactor)?;
        // A rename failure keeps the derived title and still succeeds.
        match saver.conversations.rename(&saver_id, &title) {
            Ok(renamed) => Ok(renamed.title),
            Err(_) => Ok(meta.title),
        }
    })
    .await?;
    let out_todos = current_todos.clone();
    chat.conversation.adopt(new_id.clone(), seeded, current_todos);
    chat.conversation.set_last_usage(None);
    drop(end);
    Ok(LoadedConversation { id: new_id, title: saved_title, messages, todos: out_todos, context: None })
}

/// Forks the live conversation: the same messages, todos and context usage under a new id, with the old
/// conversation's attachment files copied. No model call, so no model needs to be configured. The old conversation
/// stays untouched. Refused while busy, and when the history has no user message yet.
#[tauri::command]
async fn fork_conversation(
    chat_id: String,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
) -> Result<LoadedConversation, String> {
    // The save records the terminal's directory, so the chat's default terminal is needed too.
    let (chat, tab) = chat_and_default(&reg, &chats, &chat_id)?;
    let store = persist.get()?;
    let cancel = Arc::new(TurnCancel::new());
    let (history, todos, old_id) = chat.begin_turn(&cancel)?;
    // Ends the turn on every return; dropped explicitly before success.
    let end = OnDrop::new({ let chat = chat.clone(); move || chat.end_turn(&Touched::default()) });
    // The snapshot leaves the locks before any await.
    let snapshot = lock(&history).snapshot();
    let current_todos = lock(&todos).clone();
    if !snapshot.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("user")) {
        return Err("Nothing to fork yet.".into());
    }
    let context = lock(&chat.conversation.last_usage()).clone();
    let reader = store.clone();
    let reader_id = old_id.clone();
    let old_meta = blocking(move || Ok(reader.conversations.load(&reader_id).ok().map(|m| (m.title, m.model)))).await?;
    let (old_title, old_model) = old_meta.unwrap_or_else(|| (conversations::derive_title(&snapshot), None));
    let new_id = agent_loop::new_conversation_id().map_err(io_err)?;
    // The attachments are copied before the save, so a saved fork never names a missing file. A missing source
    // directory is fine: a conversation without attachments forks to one without them. A copy failure aborts.
    let copier = store.clone();
    let (from, to) = (old_id.clone(), new_id.clone());
    blocking(move || copier.conversations.attachments().copy_conversation(&from, &to).map_err(io_err)).await?;
    let cwd = tab.as_ref().and_then(|tab| tab.session.cwd()).map(|c| (c.host, c.path));
    let saver = store.clone();
    let saver_id = new_id.clone();
    let saver_messages = snapshot.clone();
    let saver_todos = current_todos.clone();
    let saver_context = context.clone();
    let remember = remembers_last(&chat.id);
    let title = side::suffixed_title(&old_title, " (fork)");
    let saved_title = blocking(move || {
        // No model is configured for a fork, so nothing secret is known: the credential-shaped patterns still apply,
        // and anything already stored was scrubbed when it was first saved.
        let redactor = Redactor::new(Vec::new());
        let save = SideSave {
            id: saver_id.clone(),
            messages: saver_messages,
            todos: saver_todos,
            model: old_model,
            context: saver_context,
            remember,
        };
        let meta = save_side(&saver, cwd, save, &redactor)?;
        // A rename failure keeps the derived title and still succeeds.
        match saver.conversations.rename(&saver_id, &title) {
            Ok(renamed) => Ok(renamed.title),
            Err(_) => Ok(meta.title),
        }
    })
    .await?;
    let out_todos = current_todos.clone();
    let out_context = context.clone();
    chat.conversation.adopt(new_id.clone(), History::from_messages(snapshot.clone()), current_todos);
    chat.conversation.set_last_usage(out_context.clone());
    drop(end);
    Ok(LoadedConversation { id: new_id, title: saved_title, messages: snapshot, todos: out_todos, context: out_context })
}

/// The live conversation as Markdown for `/export`: the stored history (never the capped UI
/// transcript), so nothing is lost.
#[tauri::command]
fn export_conversation(chat_id: String, chats: State<'_, ChatReg>) -> Result<String, String> {
    let chat = chat_of(&chats, &chat_id)?;
    Ok(export::markdown(&lock(&chat.conversation.history()).snapshot()))
}

/// What a delete reports: whether a live conversation was among those deleted and was reset to a new one.
#[derive(Serialize)]
struct DeleteResult {
    deleted_live: bool,
}

/// Deletes saved conversations (`target`: one id, or `None`: all of them) and resets the live conversations that
/// the deletion covers, as one step. Refused while any agent is busy. Every session's turn-cancel lock is held from
/// the busy check to the end of the reset, as `begin_turn` and `new_conversation` do, so no turn starts in between.
/// `chats` must be in lock order (see `in_lock_order`).
/// A reset live conversation gets a new id, so a later save cannot write a deleted conversation back. The reset
/// happens even when the file deletion fails part way, for the same reason. Returns whether a live conversation was
/// reset.
fn delete_saved(chats: &[Arc<Chat>], store: &Persist, target: Option<&str>) -> Result<bool, String> {
    let _turns: Vec<_> = chats.iter().map(|chat| lock(&chat.turn_cancel)).collect();
    if chats.iter().any(|chat| chat.agent_busy.load(Ordering::SeqCst)) {
        return Err(BUSY_REFUSAL.into());
    }
    let deleted = match target {
        Some(id) => store.conversations.delete(id),
        None => store.conversations.delete_all(),
    };
    let mut reset = false;
    for chat in chats {
        if target.is_none_or(|id| chat.conversation.id() == id) {
            chat.conversation.reset().map_err(io_err)?;
            reset = true;
        }
    }
    deleted.map_err(|e| e.to_string())?;
    Ok(reset)
}

/// Deletes a saved conversation. Deleting one that does not exist is not an error. When the deleted conversation is the
/// live one, the live conversation is reset (see `delete_saved`) and the remembered last conversation is forgotten.
/// Otherwise, when it was the remembered last conversation, that is forgotten too. A failure to forget is logged, not
/// returned. Refused while the agent is working.
#[tauri::command]
async fn delete_conversation(
    id: String,
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
) -> Result<DeleteResult, String> {
    let store = persist.get()?;
    let chats = in_lock_order(chats.snapshot());
    blocking(move || {
        let deleted_live = delete_saved(&chats, &store, Some(&id))?;
        let remembered = if deleted_live {
            store.forget_last().map(|_| ())
        } else {
            store.forget_if_last(&id).map(|_| ())
        };
        if let Err(e) = remembered {
            eprintln!("watt: the last conversation was not forgotten ({e})");
        }
        Ok(DeleteResult { deleted_live })
    })
    .await
}

/// Renames a saved conversation. Returns its metadata.
#[tauri::command]
async fn rename_conversation(
    id: String,
    title: String,
    persist: State<'_, PersistState>,
) -> Result<ConversationMeta, String> {
    let store = persist.get()?;
    blocking(move || store.conversations.rename(&id, &title).map_err(|e| e.to_string())).await
}

/// Deletes every saved conversation, and forgets the remembered last one. Every live conversation is reset, since each
/// one may have been deleted (see `delete_saved`). Refused while any agent is working.
#[tauri::command]
async fn delete_all_conversations(
    chats: State<'_, ChatReg>,
    persist: State<'_, PersistState>,
) -> Result<DeleteResult, String> {
    let store = persist.get()?;
    let chats = in_lock_order(chats.snapshot());
    blocking(move || {
        let deleted_live = delete_saved(&chats, &store, None)?;
        if let Err(e) = store.forget_last() {
            eprintln!("watt: the last conversation was not forgotten ({e})");
        }
        Ok(DeleteResult { deleted_live })
    })
    .await
}

#[tauri::command]
async fn get_ui_state(persist: State<'_, PersistState>) -> Result<UiState, String> {
    let store = persist.get()?;
    blocking(move || store.ui.load().map_err(|e| e.to_string())).await
}

/// Applies a patch to the UI state and returns the state as stored.
#[tauri::command]
async fn patch_ui_state(patch: UiStatePatch, persist: State<'_, PersistState>) -> Result<UiState, String> {
    let store = persist.get()?;
    blocking(move || store.ui.patch(patch).map_err(|e| e.to_string())).await
}

/// The AI provider settings as the settings UI shows them: the settings and which secrets are stored, never their
/// values. Reading which secrets are stored touches the secret store, so this runs when the UI asks, not at startup.
#[tauri::command]
async fn get_settings(settings: State<'_, SettingsState>) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.view()).await
}

/// Recovers from a broken settings.json. "revert" keeps the broken file aside (settings.json.broken-<unix>) and
/// restores the last working copy as settings.json. "use_last_good" runs the last working copy for this session and
/// leaves settings.json exactly as it is. Both return the settings as they now are.
#[tauri::command]
async fn recover_settings(action: RecoverAction, settings: State<'_, SettingsState>) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || match action {
        RecoverAction::Revert => service.revert_to_last_good(),
        RecoverAction::UseLastGood => service.use_last_good(),
    })
    .await
}

/// Re-reads settings.json. A file that is valid again makes the AI settings usable again, without a restart.
#[tauri::command]
async fn reload_settings(settings: State<'_, SettingsState>) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.reload()).await
}

/// Adds a provider, or replaces the one with the same id. Keys and secret header values go to the secret store and are
/// never returned. The result is the settings as stored.
#[tauri::command]
async fn save_provider(input: SaveProviderInput, settings: State<'_, SettingsState>) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.save_provider(input)).await
}

/// Deletes a provider and its secrets. A default model that used it is cleared.
#[tauri::command]
async fn delete_provider(id: String, settings: State<'_, SettingsState>) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.delete_provider(&id)).await
}

/// Sets the default model, or clears it with null. The model must be listed by its provider.
#[tauri::command]
async fn set_default_model(
    model: Option<ModelRef>,
    settings: State<'_, SettingsState>,
) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.set_default_model(model)).await
}

/// Sets the global context cap in tokens, or clears it with null.
#[tauri::command]
async fn set_global_context_limit(
    limit: Option<u32>,
    settings: State<'_, SettingsState>,
) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.set_global_context_limit(limit)).await
}

/// Saves the approval rules: the mode, allow and deny lists. Refused while settings.json is broken or in last-good mode.
#[tauri::command]
async fn set_approval_settings(
    input: ApprovalSettings,
    settings: State<'_, SettingsState>,
) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.set_approval(input)).await
}

/// Saves the shell integration settings: whether the agent's integration requests ask the user. Refused while settings.json
/// is broken or in last-good mode.
#[tauri::command]
async fn set_integration_settings(
    input: IntegrationSettings,
    settings: State<'_, SettingsState>,
) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.set_integration(input)).await
}

/// Saves the agent behaviour settings: whether a denied tool call stops the turn or lets the agent carry on. Refused while
/// settings.json is broken or in last-good mode.
#[tauri::command]
async fn set_agent_settings(input: AgentSettings, settings: State<'_, SettingsState>) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.set_agent(input)).await
}

/// Saves the per-turn limits: the model-call cap (or unlimited), the polling cap, the commands cap and the wait minutes. Refused
/// while settings.json is broken or in last-good mode.
#[tauri::command]
async fn set_limits_settings(input: LimitsSettings, settings: State<'_, SettingsState>) -> Result<SettingsView, String> {
    let service = settings.get()?;
    blocking(move || service.set_limits(input)).await
}

/// What the saved approval settings would decide for `command`, without running it. Approve-all is not included: it is
/// session-only and is not saved.
#[tauri::command]
async fn test_approval(command: String, settings: State<'_, SettingsState>) -> Result<approval::Preview, String> {
    let service = settings.get()?;
    blocking(move || {
        let saved = service.approval_settings()?;
        approval::preview(&saved, &command)
    })
    .await
}

/// The built-in deny list: first words that never run without a card, whatever the rules say.
#[tauri::command]
fn approval_builtin_deny() -> Vec<&'static str> {
    approval::builtin_deny().to_vec()
}

/// The built-in provider presets the settings UI offers. Nothing is stored until the user saves one.
#[tauri::command]
fn provider_presets() -> Vec<ProviderPreset> {
    presets::presets()
}

/// Fetches a provider's model list (one GET to the provider) and folds it into the saved models. The result is the
/// settings as stored, plus what changed.
#[tauri::command]
async fn fetch_models(provider_id: String, settings: State<'_, SettingsState>) -> Result<FetchOutcome, String> {
    let service = settings.get()?;
    let plan = {
        let service = service.clone();
        let id = provider_id.clone();
        blocking(move || service.provider_for_fetch(&id)).await?
    };
    let client = agent_loop::shared_client()?;
    let fetched = fetch_model_list(client, &plan.url, &plan.api_key, &plan.headers, USER_AGENT)
        .await
        .map_err(|e| e.to_string())?;
    blocking(move || {
        let (view, summary) = service.apply_fetched(&provider_id, fetched)?;
        Ok(FetchOutcome { view, summary })
    })
    .await
}

/// Sends one short test message to a model and reports whether it answered. It is one request, with the reply capped at
/// 16 output tokens, so it costs a few tokens. The model does not have to be the default.
#[tauri::command]
async fn test_connection(
    provider_id: String,
    model_id: String,
    settings: State<'_, SettingsState>,
) -> Result<TestResult, String> {
    let service = settings.get()?;
    let plan = blocking(move || service.test_plan(&provider_id, &model_id)).await?;
    let client = agent_loop::shared_client()?;
    Ok(test_model(client, &plan).await)
}

/// The MCP servers as saved, with their live state and tools. Starts the servers on first use. Holds no secret value.
#[tauri::command]
async fn get_mcp(mcp: State<'_, McpState>) -> Result<McpView, String> {
    let service = mcp.get()?;
    blocking(move || service.view()).await
}

/// The composer's MCP view: names, switches, states and tool counts. Never starts the servers, so it is cheap to poll.
#[tauri::command]
async fn get_mcp_status(mcp: State<'_, McpState>) -> Result<McpStatus, String> {
    let service = mcp.get()?;
    blocking(move || Ok(service.status())).await
}

/// Adds the servers of a pasted `mcpServers` list. Refused as a whole, naming the servers, when a name already exists.
#[tauri::command]
async fn mcp_import(text: String, mcp: State<'_, McpState>) -> Result<McpImportResult, String> {
    let service = mcp.get()?;
    blocking(move || service.import(&text)).await
}

/// Adds a server or replaces the one with its name. Secret values are kept in the secret store (see `McpService::save_server`).
#[tauri::command]
async fn mcp_save_server(input: McpServerInput, mcp: State<'_, McpState>) -> Result<McpView, String> {
    let service = mcp.get()?;
    blocking(move || service.save_server(input)).await
}

#[tauri::command]
async fn mcp_remove_server(name: String, mcp: State<'_, McpState>) -> Result<McpView, String> {
    let service = mcp.get()?;
    blocking(move || service.remove_server(&name)).await
}

#[tauri::command]
async fn mcp_set_enabled(name: String, enabled: bool, mcp: State<'_, McpState>) -> Result<McpView, String> {
    let service = mcp.get()?;
    blocking(move || service.set_enabled(&name, enabled)).await
}

#[tauri::command]
async fn mcp_set_server_approval(name: String, level: McpLevel, mcp: State<'_, McpState>) -> Result<McpView, String> {
    let service = mcp.get()?;
    blocking(move || service.set_server_approval(&name, level)).await
}

/// Sets one tool's level, or clears its override with null (the tool then inherits the server's level).
#[tauri::command]
async fn mcp_set_tool_approval(
    name: String,
    tool: String,
    level: Option<McpLevel>,
    mcp: State<'_, McpState>,
) -> Result<McpView, String> {
    let service = mcp.get()?;
    blocking(move || service.set_tool_approval(&name, &tool, level)).await
}

#[tauri::command]
async fn mcp_restart(name: String, mcp: State<'_, McpState>) -> Result<McpView, String> {
    let service = mcp.get()?;
    blocking(move || service.restart(&name)).await
}

/// The last log lines of a server, oldest first.
#[tauri::command]
async fn mcp_logs(name: String, mcp: State<'_, McpState>) -> Result<Vec<String>, String> {
    let service = mcp.get()?;
    blocking(move || service.logs(&name)).await
}

/// The bundle identifier before the rename to Watt, and the current one (tauri.conf.json). On macOS the app's data
/// folder is named after the identifier, so the old folder is copied to the new one once (`migrate_app_folder`).
#[cfg(target_os = "macos")]
const LEGACY_IDENTIFIER: &str = "dev.agentterm.app";
#[cfg(target_os = "macos")]
const IDENTIFIER: &str = "dev.watt.terminal";

/// Copies the old app folder to the new one before anything reads it, so the window state plugin and the stores see the
/// user's files. Skipped when a config or data folder override is set (tests and development). A failure is logged and
/// startup goes on.
#[cfg(target_os = "macos")]
fn migrate_app_folder() {
    let overridden = dir_override("WATT_CONFIG_DIR", "AGENT_TERM_CONFIG_DIR").is_some()
        || dir_override("WATT_DATA_DIR", "AGENT_TERM_DATA_DIR").is_some();
    if overridden {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let support = PathBuf::from(home).join("Library").join("Application Support");
    let legacy = support.join(LEGACY_IDENTIFIER);
    let new = support.join(IDENTIFIER);
    match crate::core::migrate::migrate_legacy_dir(&legacy, &new) {
        Ok(crate::core::migrate::MigrateOutcome::Copied) => {
            eprintln!("watt: copied the app folder from {LEGACY_IDENTIFIER} to {IDENTIFIER}");
        }
        Ok(_) => {}
        Err(e) => eprintln!("watt: could not copy the app folder from {LEGACY_IDENTIFIER}: {e}"),
    }
}

/// Brings the main window forward. The frontend calls it when a notification is clicked; the window may be hidden,
/// minimized or behind other apps.
#[tauri::command]
fn focus_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or_else(|| "the main window is gone".to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.unminimize().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

/// The layout inspector's gate: the command opens DevTools only when the developer setting is on. The frontend hides the
/// menu item too, but this check is the one that counts.
fn devtools_gate(enabled: bool) -> Result<(), String> {
    if enabled { Ok(()) } else { Err("developer tools are disabled in Settings".to_string()) }
}

/// Opens WebView2 DevTools (the layout inspector) for the main window. Refused unless the developer setting is on.
#[tauri::command]
async fn open_devtools(app: tauri::AppHandle, persist: State<'_, PersistState>) -> Result<(), String> {
    let store = persist.get()?;
    let enabled = blocking(move || store.ui.load().map(|s| s.devtools).map_err(|e| e.to_string())).await?;
    devtools_gate(enabled)?;
    let window = app.get_webview_window("main").ok_or_else(|| "the main window is gone".to_string())?;
    window.open_devtools();
    Ok(())
}

pub fn run() {
    #[cfg(target_os = "macos")]
    migrate_app_folder();
    // Reads the dev LLM settings now, so a bad value is reported at launch and falls back to the mock agent.
    agent_loop::settings();
    let app = tauri::Builder::default()
        // Registered first, as the plugin requires. A second launch runs this closure in the first instance and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        // Desktop notifications (the frontend sends them and handles clicks). Registered after single-instance.
        .plugin(tauri_plugin_notification::init())
        // Native folder picker (api.ts pickFolder). The capability grants only dialog:allow-open.
        .plugin(tauri_plugin_dialog::init())
        // Saves the window's size, position and maximized state on exit and restores them at launch.
        // Visibility, decorations and fullscreen are not tracked: Tauri shows the window itself.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .build(),
        )
        .manage(Reg::new())
        .manage(ChatReg::new())
        .manage(Claims::default())
        .setup(|app| {
            let persist = resolve_persist(app.handle()).map(Arc::new);
            app.manage(PersistState(persist));
            // Loads settings.json only. The secret store is read on first use, or by the MCP warm-up below on its own thread.
            let settings = resolve_config_dir(app.handle())
                .map(|dir| Arc::new(SettingsService::new(&dir, Arc::new(Secrets::from_env(&dir)))));
            let mcp = settings.as_ref().map(|s| Arc::new(McpService::new(s.clone())));
            // The enabled MCP servers start on their own thread, so they connect before the first turn; the window does not wait.
            if let Some(mcp) = &mcp {
                let warm = mcp.clone();
                thread::spawn(move || warm.warm());
            }
            app.manage(McpState(mcp));
            app.manage(SettingsState(settings));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_session,
            restart_session,
            ack,
            resize,
            write_user,
            agent_message,
            steer_message,
            withdraw_steer,
            cancel_agent,
            new_conversation,
            ask_btw,
            suggest_name,
            suggest_prompts,
            focus_main_window,
            open_devtools,
            compact_conversation,
            fork_conversation,
            export_conversation,
            approve_tool,
            set_auto_approve,
            auto_approve_state,
            get_status,
            list_commands,
            integrate_shell,
            list_conversations,
            load_conversation,
            get_attachment,
            delete_conversation,
            rename_conversation,
            delete_all_conversations,
            get_ui_state,
            patch_ui_state,
            get_settings,
            recover_settings,
            reload_settings,
            save_provider,
            delete_provider,
            set_default_model,
            set_global_context_limit,
            set_approval_settings,
            set_integration_settings,
            set_agent_settings,
            set_limits_settings,
            set_skills_settings,
            answer_integration,
            answer_questions,
            test_approval,
            approval_builtin_deny,
            provider_presets,
            fetch_models,
            test_connection,
            get_mcp,
            get_mcp_status,
            mcp_import,
            mcp_save_server,
            mcp_remove_server,
            mcp_set_enabled,
            mcp_set_server_approval,
            mcp_set_tool_approval,
            mcp_restart,
            mcp_logs,
            list_skills,
            close_session,
            list_tabs,
            set_tab_title,
            assign_tab_cmd,
            create_chat,
            attach_chat,
            close_chat,
            list_chats
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|app, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            // Kill every shell. Session::kill sends SIGKILL, so zsh does not rewrite the user's history.
            for att in app.state::<Reg>().take_all() {
                att.session.kill();
            }
            // Stops the MCP servers, if they were started. Idempotent.
            if let Ok(mcp) = app.state::<McpState>().get() {
                mcp.shutdown();
            }
        }
    });
}

/// Serialises the check that a conversation is not open in another chat with the restore that takes it (`load_conversation`).
#[derive(Default)]
struct Claims(Mutex<()>);

/// Whether a chat other than `chat_id` holds the conversation `conversation_id`. The caller holds `Claims`.
fn held_by_other(chats: &[Arc<Chat>], chat_id: &str, conversation_id: &str) -> bool {
    chats.iter().any(|other| other.id != chat_id && other.conversation.id() == conversation_id)
}

/// A terminal as the tab list shows it (`list_tabs`).
#[derive(Serialize)]
struct TabInfo {
    id: String,
    /// The chat the terminal belongs to. None: no chat sees it.
    owner: Option<String>,
    state: InputState,
    nonce: String,
    title: Option<String>,
}

/// Every terminal, oldest first.
#[tauri::command]
fn list_tabs(reg: State<'_, Reg>) -> Vec<TabInfo> {
    let mut tabs = reg.snapshot();
    tabs.sort_by_key(|tab| tab.seq);
    tabs.iter()
        .map(|tab| TabInfo {
            id: tab.id.clone(),
            owner: tab.owner_id(),
            state: tab.session.state(),
            nonce: tab.session.nonce().to_string(),
            title: lock(&tab.title).clone(),
        })
        .collect()
}

/// Closes a terminal: it is removed and its shell killed, whatever it runs. An agent turn that held it is not cancelled:
/// its next call to the terminal errors. Allowed at any time.
#[tauri::command]
fn close_session(session_id: String, reg: State<'_, Reg>) -> Result<(), String> {
    let tab = reg.remove(&session_id).ok_or_else(|| no_session(&session_id))?;
    // A running turn may still hold the tab's session, so the shell is killed here, not when the last reference drops.
    tab.session.kill();
    Ok(())
}

/// Sets a terminal's title (trimmed, capped at `TITLE_MAX`). Empty or None clears it.
#[tauri::command]
fn set_tab_title(session_id: String, title: Option<String>, reg: State<'_, Reg>) -> Result<(), String> {
    *lock(&tab_of(&reg, &session_id)?.title) = clean_title(title);
    Ok(())
}

/// Assigns a terminal to a chat (`on`), or unassigns it. See `assign_tab`. Named apart from that fn.
#[tauri::command]
fn assign_tab_cmd(
    chat_id: String,
    session_id: String,
    on: bool,
    reg: State<'_, Reg>,
    chats: State<'_, ChatReg>,
) -> Result<(), String> {
    assign_tab(&reg, &chats, &chat_id, &session_id, on)
}

/// A new chat with no terminal and a new conversation. Returns its id.
#[tauri::command]
fn create_chat(chats: State<'_, ChatReg>) -> Result<String, String> {
    let id = next_chat_id();
    ensure_chat(&chats, &id)?;
    Ok(id)
}

/// Sets the chat's agent-events channel. Idempotent: a later call swaps the channel. Chat `MAIN` is created when missing,
/// so the frontend can attach it right after `start_session`.
#[tauri::command]
fn attach_chat(chat_id: String, on_events: Channel<Value>, chats: State<'_, ChatReg>) -> Result<(), String> {
    if chat_id == MAIN {
        ensure_chat(&chats, MAIN)?;
    }
    chat_of(&chats, &chat_id)?.attach_events(on_events);
    Ok(())
}

/// Closes a chat: refused while its turn runs. Its terminals are unassigned and the chat is removed. Closing `MAIN` is
/// allowed; `start_session` of `main` makes it again.
#[tauri::command]
fn close_chat(chat_id: String, reg: State<'_, Reg>, chats: State<'_, ChatReg>) -> Result<(), String> {
    let chat = chat_of(&chats, &chat_id)?;
    // The turn-cancel lock, as `assign_tab` takes it: no terminal is assigned to the chat while it closes.
    let _turn = lock(&chat.turn_cancel);
    if chat.agent_busy.load(Ordering::SeqCst) {
        return Err(BUSY_REFUSAL.into());
    }
    chat.closed.store(true, Ordering::SeqCst);
    for tab in chat_tabs(&reg, &chat_id) {
        *lock(&tab.owner) = None;
    }
    chats.remove(&chat_id);
    Ok(())
}

/// A chat as the chat list shows it (`list_chats`).
#[derive(Serialize)]
struct ChatInfo {
    id: String,
    /// The live conversation's id.
    conversation_id: String,
    busy: bool,
    /// The ids of the terminals the chat owns, oldest first.
    tabs: Vec<String>,
}

/// Every chat, by id.
#[tauri::command]
fn list_chats(reg: State<'_, Reg>, chats: State<'_, ChatReg>) -> Vec<ChatInfo> {
    let mut all = chats.snapshot();
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all.iter()
        .map(|chat| ChatInfo {
            id: chat.id.clone(),
            conversation_id: chat.conversation.id(),
            busy: chat.agent_busy.load(Ordering::SeqCst),
            tabs: chat_tabs(&reg, &chat.id).iter().map(|tab| tab.id.clone()).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_ids_are_distinct_per_call() {
        let ids: Vec<String> = (0..100).map(|_| next_shim_id()).collect();
        let distinct: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(distinct.len(), ids.len(), "shim ids collide: {ids:?}");
    }

    #[test]
    fn status_shell_reads_the_named_terminal_or_none() {
        let reg = Reg::new();
        assert!(matches!(status_shell(&reg, "main", None), Ok(None)), "a chat with no terminal has no shell");
        assert_eq!(status_shell(&reg, "main", Some("t9")).err().as_deref(), Some("no session t9"));
    }

    #[test]
    fn reattach_swaps_channels_and_resets_flow_control() {
        let channels = Arc::new(Channels::default());
        assert!(lock(&channels.data).is_none(), "no data channel before re-attach");
        assert!(lock(&channels.events).is_none(), "no events channel before re-attach");
        let data: Channel<InvokeResponseBody> = Channel::new(|_| Ok(()));
        let events: Channel<Value> = Channel::new(|_| Ok(()));
        let mut resets = 0;
        reattach_channels(&channels, data, events, || resets += 1);
        assert_eq!(resets, 1, "the re-attach path resets flow control");
        assert!(lock(&channels.data).is_some(), "re-attach swaps the data channel");
        assert!(lock(&channels.events).is_some(), "re-attach swaps the events channel");
    }

    #[test]
    fn reattach_resets_only_after_the_channels_are_swapped() {
        let channels = Arc::new(Channels::default());
        let data: Channel<InvokeResponseBody> = Channel::new(|_| Ok(()));
        let events: Channel<Value> = Channel::new(|_| Ok(()));
        let swapped_at_reset = Mutex::new(None);
        let probe = channels.clone();
        reattach_channels(&channels, data, events, || {
            *swapped_at_reset.lock().unwrap() = Some(lock(&probe.data).is_some());
        });
        assert_eq!(*swapped_at_reset.lock().unwrap(), Some(true), "reset runs after the channel swap");
    }

    #[test]
    fn devtools_opens_only_when_the_setting_is_on() {
        assert!(devtools_gate(true).is_ok());
        assert_eq!(devtools_gate(false).unwrap_err(), "developer tools are disabled in Settings");
    }

    /// The terminal's Ctrl-C cancels the agent's turn only when it belongs to the agent (see
    /// `Chat::interrupt_cancels_turn`). Without a busy agent it never does, whatever the shell shows.
    #[test]
    fn interrupt_never_cancels_without_a_busy_agent() {
        for state in [
            InputState::Booting,
            InputState::Prompt,
            InputState::Submitted,
            InputState::Running,
            InputState::Continuation,
            InputState::Unresponsive,
            InputState::Alt,
            InputState::Raw,
            InputState::Exited,
        ] {
            assert!(!Chat::interrupt_cancels(false, false, false, state, 1), "idle agent at {state:?}");
            assert!(!Chat::interrupt_cancels(false, true, true, state, 1), "idle agent at {state:?}");
        }
    }

    /// While the agent holds the shell (a shell-acting call, or its own command on top), the terminal's Ctrl-C
    /// is the agent's interrupt: it cancels whatever the shell shows, even a nested prompt or a user command.
    #[test]
    fn interrupt_cancels_whenever_the_agent_holds_the_shell() {
        for state in [
            InputState::Booting,
            InputState::Prompt,
            InputState::Submitted,
            InputState::Running,
            InputState::Continuation,
            InputState::Unresponsive,
            InputState::Alt,
            InputState::Raw,
            InputState::Exited,
        ] {
            assert!(Chat::interrupt_cancels(true, true, false, state, 1), "agent control at {state:?}");
            assert!(Chat::interrupt_cancels(true, true, false, state, 3), "agent control nested at {state:?}");
            assert!(Chat::interrupt_cancels(true, false, true, state, 1), "agent command on top at {state:?}");
            assert!(Chat::interrupt_cancels(true, false, true, state, 3), "agent command on top nested at {state:?}");
        }
    }

    /// Otherwise only an idle prompt of the local shell counts: there Ctrl-C can only clear the line. The user's
    /// own running command, the alternate screen, a nested prompt, raw mode and a dead session keep the Ctrl-C.
    #[test]
    fn interrupt_cancels_only_at_idle_local_prompts() {
        // (state, levels, cancels)
        for (state, levels, cancels) in [
            (InputState::Prompt, 1, true),
            (InputState::Continuation, 1, true),
            (InputState::Submitted, 1, true),
            (InputState::Prompt, 2, false),
            (InputState::Prompt, 3, false),
            (InputState::Continuation, 2, false),
            (InputState::Submitted, 2, false),
            (InputState::Running, 1, false),
            (InputState::Booting, 1, false),
            (InputState::Unresponsive, 1, false),
            (InputState::Unresponsive, 2, false),
            (InputState::Alt, 1, false),
            (InputState::Raw, 1, false),
            (InputState::Exited, 1, false),
        ] {
            assert_eq!(
                Chat::interrupt_cancels(true, false, false, state, levels),
                cancels,
                "busy agent, no control, no command on top at {state:?} with {levels} level(s)"
            );
        }
    }

    /// Stands in for a `Tab` in the owner rules, which do not need a PTY.
    struct Probe {
        owner: Option<String>,
        seq: u64,
    }

    fn probe(seq: u64, owner: Option<&str>) -> Arc<Probe> {
        Arc::new(Probe { owner: owner.map(str::to_string), seq })
    }

    #[test]
    fn a_chats_terminals_are_its_own_in_creation_order() {
        let tabs = vec![probe(3, Some("main")), probe(1, Some("other")), probe(2, Some("main")), probe(0, None)];
        let mine = owned_tabs(tabs, "main", |t| t.owner.clone(), |t| t.seq);
        assert_eq!(mine.iter().map(|t| t.seq).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn the_default_terminal_is_the_oldest_one_of_the_chat() {
        let tabs = vec![probe(7, Some("main")), probe(4, Some("main")), probe(5, Some("other"))];
        let default = owned_tabs(tabs, "main", |t| t.owner.clone(), |t| t.seq).into_iter().next();
        assert_eq!(default.map(|t| t.seq), Some(4));
    }

    #[test]
    fn a_chat_without_terminals_has_no_default() {
        let tabs = vec![probe(1, Some("other"))];
        assert!(owned_tabs(tabs, "main", |t| t.owner.clone(), |t| t.seq).into_iter().next().is_none());
    }

    #[test]
    fn owner_change_follows_the_assign_and_unassign_rules() {
        // Unowned: assign takes it, even while the chat runs a turn; unassign does nothing.
        assert_eq!(owner_change(None, "main", true, false), Ok(true));
        assert_eq!(owner_change(None, "main", true, true), Ok(true));
        assert_eq!(owner_change(None, "main", false, false), Ok(false));
        // Already ours: assign again changes nothing; unassign releases it when idle, and is refused during a turn.
        assert_eq!(owner_change(Some("main"), "main", true, false), Ok(false));
        assert_eq!(owner_change(Some("main"), "main", false, false), Ok(true));
        assert_eq!(owner_change(Some("main"), "main", false, true), Err(BUSY_REFUSAL.to_string()));
        // Another chat's: assign is refused, unassign leaves it alone.
        assert_eq!(
            owner_change(Some("other"), "main", true, false),
            Err("that terminal belongs to another chat".to_string())
        );
        assert_eq!(owner_change(Some("other"), "main", false, false), Ok(false));
    }

    #[test]
    fn only_the_first_terminal_joins_a_chat_on_start() {
        assert_eq!(owner_for_session("main"), Some("main"));
        assert_eq!(owner_for_session("other"), None);
    }

    #[test]
    fn a_chat_with_no_terminal_runs_with_no_default_and_the_local_level() {
        let reg: Reg = Registry::new();
        let chats: ChatReg = Registry::new();
        ensure_chat(&chats, "main").expect("the chat");
        let (chat, tab) = chat_and_default(&reg, &chats, "main").expect("a chat with no terminal is legal");
        assert_eq!(chat.id, "main");
        assert!(tab.is_none());
        assert_eq!(top_key_of(None), LOCAL_KEY);
        assert!(chat_and_default(&reg, &chats, "absent").is_err(), "a missing chat is still no session");
    }

    #[test]
    fn new_chat_ids_are_distinct_and_never_the_default() {
        let ids: Vec<String> = (0..100).map(|_| next_chat_id()).collect();
        let distinct: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(distinct.len(), ids.len(), "chat ids collide: {ids:?}");
        assert!(ids.iter().all(|id| id.starts_with('c') && id != MAIN), "{ids:?}");
    }

    #[test]
    fn only_the_default_chat_remembers_the_last_conversation() {
        assert!(remembers_last("main"));
        assert!(!remembers_last("c1"));
    }

    #[test]
    fn titles_are_trimmed_capped_and_blank_means_none() {
        assert_eq!(clean_title(None), None);
        assert_eq!(clean_title(Some("   ".into())), None);
        assert_eq!(clean_title(Some("  zsh  ".into())), Some("zsh".to_string()));
        let long = "x".repeat(200);
        assert_eq!(clean_title(Some(long)).map(|t| t.chars().count()), Some(TITLE_MAX));
        // The cap is in characters, so a multi-byte title is cut whole, and trailing space left by the cut is dropped.
        let cut = format!("{} {}", "é".repeat(TITLE_MAX - 1), "tail");
        assert_eq!(clean_title(Some(cut)), Some("é".repeat(TITLE_MAX - 1)));
    }

    #[test]
    fn a_conversation_is_held_by_at_most_the_chat_that_claims_it() {
        let main = Arc::new(Chat::new("main").unwrap());
        let other = Arc::new(Chat::new("c1").unwrap());
        let chats = vec![main.clone(), other.clone()];
        assert!(held_by_other(&chats, "c1", &main.conversation.id()), "main holds its conversation");
        assert!(!held_by_other(&chats, "main", &main.conversation.id()), "a chat does not block itself");
        assert!(!held_by_other(&chats, "c1", "ses_unknown"), "an unheld conversation is free");
    }

    #[test]
    fn a_closed_chat_takes_no_turn() {
        let chat = Chat::new("c9").unwrap();
        chat.closed.store(true, Ordering::SeqCst);
        assert!(chat.begin_turn(&Arc::new(TurnCancel::new())).is_err());
        assert!(!chat.agent_busy.load(Ordering::SeqCst), "a refused turn leaves the chat idle");
    }

    #[test]
    fn chats_are_locked_in_id_order() {
        let chats = vec![Arc::new(Chat::new("b").unwrap()), Arc::new(Chat::new("a").unwrap())];
        let ids: Vec<String> = in_lock_order(chats).iter().map(|chat| chat.id.clone()).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }
}
