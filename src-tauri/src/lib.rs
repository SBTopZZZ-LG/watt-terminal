//! Tauri wiring only. All logic lives in `core`; this file maps commands and channels onto it.
//!
//! Channels, per session: `on_data` carries raw PTY bytes (`InvokeResponseBody::Raw`, which the frontend
//! receives as an ArrayBuffer) and must be acknowledged with `ack`. `on_events` carries JSON objects:
//! session events (`state`, `integration`, `observer`, `cwd`, `record_started`, `record_finished`) and
//! agent events (`text`, `tool_call`, `tool_result`, `approval_request`, `agent_control`, `done`), told apart by `type`.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;

use serde::Serialize;
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Manager, RunEvent, State};

// Core logic (PTY, shell integration, observer, agent) has no Tauri dependency, so `cargo test` runs it headless.
pub mod core;

use crate::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate, MockAgent, OnDrop};
use crate::core::chat::agent_loop::{self, Conversation, LlmAgent, LlmSettings, USER_AGENT};
use crate::core::chat::history::History;
use crate::core::chat::redact::Redactor;
use crate::core::llm::TurnCancel;
use crate::core::observer::{EventSink, InputState, IntegrationStatus, ObserverStatus, SessionEvent};
use crate::core::persist::conversations::ConversationMeta;
use crate::core::persist::turn::{FinishedTurn, TurnEnv, resume, save_turn};
use crate::core::persist::ui_state::{UiState, UiStatePatch};
use crate::core::persist::{Persist, now_secs};
use crate::core::pty::OutputSink;
use crate::core::record::Cwd;
use crate::core::registry::Registry;
use crate::core::session::{Session, SessionStats, SpawnConfig};
use crate::core::settings::models::fetch_models as fetch_model_list;
use crate::core::settings::presets::{self, ProviderPreset};
use crate::core::settings::secrets::Secrets;
use crate::core::settings::service::{
    FetchOutcome, SaveProviderInput, SettingsService, SettingsView, TestResult, test_model,
};
use crate::core::settings::ModelRef;
use crate::core::tools::SessionTools;

type Reg = Registry<Attached>;

/// Counts restarts, to give each restarted shell its own shim directory name.
static RESTARTS: AtomicU64 = AtomicU64::new(0);

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

    fn send_json<T: Serialize>(&self, ev: &T) {
        if let Ok(v) = serde_json::to_value(ev)
            && let Some(ch) = lock(&self.events).as_ref()
        {
            let _ = ch.send(v);
        }
    }
}

/// Raw output to the terminal. With no window attached the bytes are dropped; the gate is then never
/// acked for them, which only matters after a webview reload that loses more than the 2 MiB window.
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

/// One live session plus its per-session agent state.
pub struct Attached {
    session: Session,
    channels: Arc<Channels>,
    approvals: ApprovalGate,
    agent_busy: AtomicBool,
    /// True while the agent is executing a shell-acting call (see `AgentEvent::AgentControl`).
    agent_control: AtomicBool,
    /// The running turn's cancel token. Set when a turn starts, cleared when it ends. Ctrl-C and `cancel_agent` set it.
    turn_cancel: Mutex<Option<Arc<TurnCancel>>>,
    /// The conversation: its id (sent to the provider) and its history. `new_conversation` resets it.
    conversation: Conversation,
}

impl Attached {
    /// Ends a turn: clears the cancel token, the control flag (telling the UI) and the busy flag. Runs even when the
    /// turn panicked.
    fn end_turn(&self) {
        *lock(&self.turn_cancel) = None;
        if self.agent_control.swap(false, Ordering::SeqCst) {
            self.channels.send_json(&AgentEvent::AgentControl { active: false });
        }
        self.agent_busy.store(false, Ordering::SeqCst);
    }

    /// Cancels the running turn, if any, and denies its pending approvals. Harmless when no turn is running.
    fn cancel_turn(&self) {
        agent::cancel_turn(&self.turn_cancel, &self.approvals);
    }

    /// Marks the agent busy and installs the turn's cancel token. Returns the conversation's history and id for the
    /// turn. Held under the turn-cancel lock, which `new_conversation` also takes, so a reset cannot land mid-start.
    fn begin_turn(&self, cancel: &Arc<TurnCancel>) -> Result<(Arc<Mutex<History>>, String), String> {
        let mut slot = lock(&self.turn_cancel);
        if self.agent_busy.swap(true, Ordering::SeqCst) {
            return Err("the agent is still working on the previous message".into());
        }
        *slot = Some(cancel.clone());
        Ok((self.conversation.history(), self.conversation.id()))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn session(reg: &Reg, id: &str) -> Result<Arc<Attached>, String> {
    reg.get(id).ok_or_else(|| format!("no session {id}"))
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

/// The AI settings and the UI state live here. AGENT_TERM_CONFIG_DIR wins when it is set and not empty; otherwise the
/// Tauri app config folder is used. Nothing is created here.
fn resolve_config_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    dir_override("AGENT_TERM_CONFIG_DIR").or_else(|| app.path().app_config_dir().ok())
}

/// Resolves the config and data folders. AGENT_TERM_CONFIG_DIR and AGENT_TERM_DATA_DIR win when they are set and not
/// empty; otherwise the Tauri app folders are used. Nothing is created here: the stores create their folders on write.
fn resolve_persist(app: &tauri::AppHandle) -> Option<Persist> {
    let config = resolve_config_dir(app);
    let data = dir_override("AGENT_TERM_DATA_DIR").or_else(|| app.path().app_data_dir().ok());
    match (config, data) {
        (Some(config), Some(data)) => Some(Persist::new(&config, &data)),
        _ => {
            eprintln!("agent-term: the app folders could not be resolved; conversations and UI state are not saved");
            None
        }
    }
}

fn dir_override(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
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

/// This machine's host name, as the shell reports it in OSC 7 (`$HOST`). Read once with `hostname`; "" when that fails.
fn local_host() -> &'static str {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
        std::process::Command::new("/bin/hostname")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    })
}

/// Saves the conversation at the end of a model turn. A failure is logged without any content and never fails the turn.
fn save_finished_turn(
    store: &Persist,
    session: &Session,
    conversation_id: &str,
    history: &Mutex<History>,
    redactor: &Redactor,
    model: Option<String>,
) {
    let cwd = session.cwd();
    let cwd_pair = cwd.as_ref().map(|c| (c.host.as_str(), c.path.as_str()));
    let turn = FinishedTurn { conversation_id, messages: lock(history).snapshot(), model, cwd: cwd_pair };
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let env = TurnEnv { local_host: local_host(), home: home.as_deref() };
    if let Err(e) = save_turn(store, turn, redactor, env, now_secs()) {
        eprintln!("agent-term: the conversation was not saved ({e})");
    }
}

/// Runs blocking storage work on Tauri's blocking pool, so file access never runs on the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())?
}

/// Dev only. AGENT_TERM_HISTFILE, when non-empty, is the HISTFILE the shell uses instead of the real
/// ~/.zsh_history, so manual runs never write there. Unset (the default) keeps the user's own history.
fn dev_histfile() -> Option<PathBuf> {
    std::env::var_os("AGENT_TERM_HISTFILE").filter(|v| !v.is_empty()).map(PathBuf::from)
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
) -> Result<StartResult, String> {
    attach_or_spawn(&reg, &session_id, &session_id, rows, cols, on_data, on_events)
}

/// Starts a new shell when the session has exited: the dead session is removed and a fresh one spawned, with a new
/// nonce. Otherwise this is start_session (idempotent).
#[tauri::command]
async fn restart_session(
    session_id: String,
    rows: u16,
    cols: u16,
    on_data: Channel<InvokeResponseBody>,
    on_events: Channel<Value>,
    reg: State<'_, Reg>,
) -> Result<StartResult, String> {
    // Checked and removed under the registry lock, so two restarts cannot each remove the other's new session.
    drop(reg.remove_if(&session_id, |att| att.session.state() == InputState::Exited));
    // A new shim directory name: the dead session's drop removes its own directory, which must not be the new one.
    let shim_id = format!("{session_id}-{}", RESTARTS.fetch_add(1, Ordering::SeqCst));
    attach_or_spawn(&reg, &session_id, &shim_id, rows, cols, on_data, on_events)
}

/// Attaches to `session_id`, spawning its shell first if there is none. `shim_id` names the shim directory on spawn.
fn attach_or_spawn(
    reg: &Reg,
    session_id: &str,
    shim_id: &str,
    rows: u16,
    cols: u16,
    on_data: Channel<InvokeResponseBody>,
    on_events: Channel<Value>,
) -> Result<StartResult, String> {
    let mut pending = Some((on_data, on_events));
    let (att, spawned) = reg.get_or_insert_with(session_id, || -> Result<Attached, String> {
        let (data, events) = pending.take().ok_or("channels already used")?;
        let channels = Arc::new(Channels::default());
        channels.attach(data, events);
        let cfg = SpawnConfig { rows, cols, real_zdotdir: None, histfile: dev_histfile() };
        let session = Session::spawn(shim_id, cfg, Box::new(RawSink(channels.clone()))).map_err(io_err)?;
        session.set_event_sink(Arc::new(EventsSink(channels.clone())));
        Ok(Attached {
            session,
            channels,
            approvals: ApprovalGate::new(),
            agent_busy: AtomicBool::new(false),
            agent_control: AtomicBool::new(false),
            turn_cancel: Mutex::new(None),
            conversation: Conversation::new().map_err(io_err)?,
        })
    })?;
    if let Some((data, events)) = pending.take() {
        att.channels.attach(data, events);
    }
    Ok(StartResult { spawned, nonce: att.session.nonce().to_string() })
}

#[tauri::command]
fn ack(session_id: String, bytes: usize, reg: State<'_, Reg>) -> Result<(), String> {
    session(&reg, &session_id)?.session.ack(bytes);
    Ok(())
}

#[tauri::command]
fn resize(session_id: String, rows: u16, cols: u16, reg: State<'_, Reg>) -> Result<(), String> {
    session(&reg, &session_id)?.session.resize(rows, cols).map_err(io_err)
}

/// Keystrokes from the terminal, as text (sent as UTF-8). Ctrl-C also cancels the running agent turn and pending approvals.
#[tauri::command]
fn write_user(session_id: String, text: String, reg: State<'_, Reg>) -> Result<(), String> {
    let att = session(&reg, &session_id)?;
    if text.contains('\u{3}') {
        att.cancel_turn();
    }
    agent::user_input(&att.session, &att.approvals, &text).map_err(io_err)
}

/// Cancels the running agent turn, if any: its pending approvals are denied and its `run_command` stops waiting.
/// Harmless when no turn is running.
#[tauri::command]
fn cancel_agent(session_id: String, reg: State<'_, Reg>) -> Result<(), String> {
    session(&reg, &session_id)?.cancel_turn();
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

/// Starts one agent turn on its own thread. Events arrive on the events channel. The model is chosen by `choose_llm`.
/// The choice is made before the turn starts, so an error leaves the agent idle.
#[tauri::command]
async fn agent_message(
    session_id: String,
    text: String,
    reg: State<'_, Reg>,
    persist: State<'_, PersistState>,
    settings: State<'_, SettingsState>,
) -> Result<(), String> {
    let att = session(&reg, &session_id)?;
    // Reading a saved model touches the Keychain, so it runs on the blocking pool.
    let service = settings.0.clone();
    let chosen = blocking(move || choose_llm(service.as_deref())).await?;
    let cancel = Arc::new(TurnCancel::new());
    let (history, conversation_id) = att.begin_turn(&cancel)?;
    // Only the LLM agent writes history, so only its turns are saved. The mock agent never touches the history.
    let (mut llm, model) = match chosen {
        None => (None, None),
        Some((llm_settings, model)) => match agent_loop::shared_client() {
            Ok(client) => {
                let agent = LlmAgent::new(client, llm_settings, &conversation_id, history.clone(), cancel.clone());
                (Some(agent), Some(model))
            }
            Err(e) => {
                att.end_turn();
                return Err(e);
            }
        },
    };
    let store = persist.0.clone();
    let worker = att.clone();
    let spawned = thread::Builder::new().name("agent".into()).spawn(move || {
        // Runs on panic too, so a crashed turn cannot leave the agent busy or in control of the shell.
        let _end = OnDrop::new(|| worker.end_turn());
        let tools = SessionTools::new(&worker.session).with_cancel(cancel);
        let channels = worker.channels.clone();
        let owner = worker.clone();
        let emit = move |ev: AgentEvent| {
            if let AgentEvent::AgentControl { active } = &ev {
                owner.agent_control.store(*active, Ordering::SeqCst);
            }
            channels.send_json(&ev);
        };
        let ctx = AgentCtx::new(&tools, &worker.approvals, &emit);
        match llm.as_mut() {
            Some(agent) => agent::run_turn(agent, &text, &ctx),
            None => agent::run_turn(&mut MockAgent, &text, &ctx),
        }
        // The history is final now, and this is still before the turn is ended (the guard above).
        if let (Some(agent), Some(store)) = (llm.as_ref(), store.as_ref()) {
            save_finished_turn(store, &worker.session, &conversation_id, &history, agent.redactor(), model);
        }
    });
    if let Err(e) = spawned {
        *lock(&att.turn_cancel) = None;
        att.agent_busy.store(false, Ordering::SeqCst);
        return Err(e.to_string());
    }
    Ok(())
}

/// Starts a new conversation: the history is cleared and the id rotated. Refused while the agent is working. Takes
/// the turn-cancel lock, as `begin_turn` does, so the check and the reset cannot interleave with a turn starting. The
/// remembered last conversation is then forgotten; a failure to forget it is logged, not returned.
#[tauri::command]
async fn new_conversation(
    session_id: String,
    reg: State<'_, Reg>,
    persist: State<'_, PersistState>,
) -> Result<(), String> {
    let att = session(&reg, &session_id)?;
    {
        let _turn = lock(&att.turn_cancel);
        if att.agent_busy.load(Ordering::SeqCst) {
            return Err(BUSY_REFUSAL.into());
        }
        att.conversation.reset().map_err(io_err)?;
    }
    // Without a store there is no remembered conversation to forget.
    if let Some(store) = persist.0.clone() {
        let forgot = blocking(move || store.forget_last().map_err(|e| e.to_string())).await;
        if let Err(e) = forgot {
            eprintln!("agent-term: the last conversation was not forgotten ({e})");
        }
    }
    Ok(())
}

#[tauri::command]
fn approve_tool(session_id: String, call_id: String, approve: bool, reg: State<'_, Reg>) -> Result<(), String> {
    if session(&reg, &session_id)?.approvals.decide(&call_id, approve) {
        Ok(())
    } else {
        Err(format!("no pending approval {call_id}"))
    }
}

#[tauri::command]
fn set_auto_approve(session_id: String, on: bool, reg: State<'_, Reg>) -> Result<(), String> {
    session(&reg, &session_id)?.approvals.set_auto_approve(on);
    Ok(())
}

/// Whether the LLM agent is configured, which model, and from which saved provider. `provider` is null for the
/// environment settings. Never the key or the URL.
#[derive(Serialize)]
struct LlmStatus {
    enabled: bool,
    model: Option<String>,
    provider: Option<String>,
}

#[derive(Serialize)]
struct Status {
    state: InputState,
    integration: IntegrationStatus,
    observer: ObserverStatus,
    cwd: Option<Cwd>,
    stats: SessionStats,
    auto_approve: bool,
    agent_busy: bool,
    agent_control: bool,
    llm: LlmStatus,
    /// The live conversation's id (the one the provider sees, and the file name of its saved copy).
    conversation_id: String,
}

#[tauri::command]
fn get_status(session_id: String, reg: State<'_, Reg>, settings: State<'_, SettingsState>) -> Result<Status, String> {
    let att = session(&reg, &session_id)?;
    // The saved default's label reads no secret; without one, the environment settings are shown.
    let llm = match settings.0.as_ref().and_then(|s| s.default_label()) {
        Some((provider, model)) => LlmStatus { enabled: true, model: Some(model), provider: Some(provider) },
        None => {
            let env = agent_loop::settings();
            LlmStatus { enabled: env.is_some(), model: env.map(|s| s.model.clone()), provider: None }
        }
    };
    Ok(Status {
        state: att.session.state(),
        integration: att.session.integration_status(),
        observer: att.session.observer_status(),
        cwd: att.session.cwd(),
        stats: att.session.stats(),
        auto_approve: att.approvals.auto_approve(),
        agent_busy: att.agent_busy.load(Ordering::SeqCst),
        agent_control: att.agent_control.load(Ordering::SeqCst),
        llm,
        conversation_id: att.conversation.id(),
    })
}

/// Async, so the `pkill`/`kill` calls in `Session::kill` do not run on the UI thread.
#[tauri::command]
async fn kill_session(session_id: String, reg: State<'_, Reg>) -> Result<(), String> {
    match reg.remove(&session_id) {
        Some(att) => {
            att.session.kill();
            Ok(())
        }
        None => Err(format!("no session {session_id}")),
    }
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
}

/// A saved conversation as the live one. The file is read and repaired first. Then, when the agent is idle, the live
/// conversation takes its id and history, and the id is remembered as the last conversation. Returns the repaired
/// messages, so the UI is built from exactly what the backend now holds. Refused while the agent is working.
#[tauri::command]
async fn load_conversation(
    session_id: String,
    id: String,
    reg: State<'_, Reg>,
    persist: State<'_, PersistState>,
) -> Result<LoadedConversation, String> {
    let att = session(&reg, &session_id)?;
    let store = persist.get()?;
    let read_store = store.clone();
    let read_id = id.clone();
    let resumed = blocking(move || resume(&read_store, &read_id).map_err(|e| e.to_string())).await?;
    let messages = resumed.history.snapshot();
    let title = resumed.title;
    {
        // The busy check and the restore happen under the turn-cancel lock, as `begin_turn` and `new_conversation` do.
        let _turn = lock(&att.turn_cancel);
        if att.agent_busy.load(Ordering::SeqCst) {
            return Err(BUSY_REFUSAL.into());
        }
        att.conversation.restore(id.clone(), resumed.history);
    }
    let remember_id = id.clone();
    let remember = blocking(move || {
        let patch = UiStatePatch { last_conversation_id: Some(Some(remember_id)), ..UiStatePatch::default() };
        store.ui.patch(patch).map(|_| ()).map_err(|e| e.to_string())
    });
    if let Err(e) = remember.await {
        eprintln!("agent-term: the last conversation was not remembered ({e})");
    }
    Ok(LoadedConversation { id, title, messages })
}

/// Deletes a saved conversation. Deleting one that does not exist is not an error. When it was the remembered last
/// conversation, that is forgotten too; a failure to forget it is logged, not returned.
#[tauri::command]
async fn delete_conversation(id: String, persist: State<'_, PersistState>) -> Result<(), String> {
    let store = persist.get()?;
    blocking(move || {
        store.conversations.delete(&id).map_err(|e| e.to_string())?;
        if let Err(e) = store.forget_if_last(&id) {
            eprintln!("agent-term: the last conversation was not forgotten ({e})");
        }
        Ok(())
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

/// Deletes every saved conversation, and forgets the remembered last one. The live conversation is not touched.
#[tauri::command]
async fn delete_all_conversations(persist: State<'_, PersistState>) -> Result<(), String> {
    let store = persist.get()?;
    blocking(move || {
        store.conversations.delete_all().map_err(|e| e.to_string())?;
        if let Err(e) = store.forget_last() {
            eprintln!("agent-term: the last conversation was not forgotten ({e})");
        }
        Ok(())
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

pub fn run() {
    // Reads the dev LLM settings now, so a bad value is reported at launch and falls back to the mock agent.
    agent_loop::settings();
    let app = tauri::Builder::default()
        .manage(Reg::new())
        .setup(|app| {
            let persist = resolve_persist(app.handle()).map(Arc::new);
            app.manage(PersistState(persist));
            // Loads settings.json only. The secret store is read on first use, so startup never touches the Keychain.
            let settings = resolve_config_dir(app.handle())
                .map(|dir| Arc::new(SettingsService::new(&dir, Arc::new(Secrets::from_env(&dir)))));
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
            cancel_agent,
            new_conversation,
            approve_tool,
            set_auto_approve,
            get_status,
            kill_session,
            list_conversations,
            load_conversation,
            delete_conversation,
            rename_conversation,
            delete_all_conversations,
            get_ui_state,
            patch_ui_state,
            get_settings,
            save_provider,
            delete_provider,
            set_default_model,
            set_global_context_limit,
            provider_presets,
            fetch_models,
            test_connection
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|app, event| {
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            // Kill every shell. Session::kill sends SIGKILL, so zsh does not rewrite the user's history.
            for att in app.state::<Reg>().take_all() {
                att.session.kill();
            }
        }
    });
}
