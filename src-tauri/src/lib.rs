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
use tauri_plugin_window_state::StateFlags;

// Core logic (PTY, shell integration, observer, agent) has no Tauri dependency, so `cargo test` runs it headless.
pub mod core;

use crate::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate, AutoState, MockAgent, OnDrop, TurnRules, UserMessage};
use crate::core::env::env_setting;
use crate::core::approval::{self, ApprovalPolicy, ManualPolicy};
use crate::core::chat::agent_loop::{self, Conversation, LlmAgent, LlmSettings, USER_AGENT};
use crate::core::chat::attachments::{AttachmentInput, ImageFile, prepare};
use crate::core::chat::history::History;
use crate::core::chat::redact::Redactor;
use crate::core::chat::todo::{Todo, TodoList};
use crate::core::foreground::Foreground;
use crate::core::llm::TurnCancel;
use crate::core::observer::{EventSink, InputState, IntegrationStatus, LevelInfo, ObserverStatus, SessionEvent};
use crate::core::persist::conversations::ConversationMeta;
use crate::core::persist::turn::{FinishedTurn, TurnEnv, resume, save_turn};
use crate::core::persist::ui_state::{UiState, UiStatePatch};
use crate::core::persist::{Persist, now_secs};
use crate::core::pty::OutputSink;
use crate::core::record::Cwd;
use crate::core::registry::Registry;
use crate::core::session::{IntegrateOutcome, Session, SessionStats, SpawnConfig};
use crate::core::settings::models::fetch_models as fetch_model_list;
use crate::core::settings::presets::{self, ProviderPreset};
use crate::core::settings::secrets::Secrets;
use crate::core::settings::service::{
    FetchOutcome, RecoverAction, SaveProviderInput, SettingsService, SettingsStateView, SettingsView, TestResult,
    test_model,
};
use crate::core::settings::{AgentRequests, ApprovalSettings, IntegrationSettings, LimitsSettings, ModelRef, Nested};
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

    /// Marks the agent busy and installs the turn's cancel token. Returns the conversation's history, todo list and id for
    /// the turn. Held under the turn-cancel lock, which `new_conversation` also takes, so a reset cannot land mid-start.
    fn begin_turn(&self, cancel: &Arc<TurnCancel>) -> Result<(Arc<Mutex<History>>, TodoList, String), String> {
        let mut slot = lock(&self.turn_cancel);
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
    let data = dir_override("WATT_DATA_DIR", "AGENT_TERM_DATA_DIR").or_else(|| app.path().app_data_dir().ok());
    match (config, data) {
        (Some(config), Some(data)) => Some(Persist::new(&config, &data)),
        _ => {
            eprintln!("watt: the app folders could not be resolved; conversations and UI state are not saved");
            None
        }
    }
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

/// What a save of the live conversation needs besides the store and the session: its id, history, todo list and model.
struct SavedState {
    conversation_id: String,
    history: Arc<Mutex<History>>,
    todos: TodoList,
    model: Option<String>,
}

/// Saves the conversation: the whole history and todo list, redacted, with the shell's directory. `in_turn` is true at
/// a checkpoint inside a turn and false at the end of the turn. A failure is logged without any content and never
/// fails the turn.
fn save_conversation(store: &Persist, session: &Session, state: &SavedState, redactor: &Redactor, in_turn: bool) {
    let cwd = session.cwd();
    let cwd_pair = cwd.as_ref().map(|c| (c.host.as_str(), c.path.as_str()));
    let turn = FinishedTurn {
        conversation_id: &state.conversation_id,
        messages: lock(&state.history).snapshot(),
        todos: lock(&state.todos).clone(),
        model: state.model.clone(),
        cwd: cwd_pair,
        in_turn,
    };
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    let env = TurnEnv { local_host: local_host(), home: home.as_deref() };
    if let Err(e) = save_turn(store, turn, redactor, env, now_secs()) {
        eprintln!("watt: the conversation was not saved ({e})");
    }
}

/// The checkpoint of an LLM turn (see `LlmAgent::with_checkpoint`): saves the conversation with `in_turn` set. Without
/// a store, nothing is saved.
fn checkpoint_saver(
    store: Option<Arc<Persist>>,
    att: Arc<Attached>,
    state: Arc<SavedState>,
) -> impl FnMut(&Redactor) + Send + 'static {
    move |redactor| {
        if let Some(store) = store.as_ref() {
            save_conversation(store, &att.session, &state, redactor, true);
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

/// The rules one turn decides by, from the settings in use (see `TurnRules`). Both are off when the settings cannot be used.
fn turn_rules(service: Option<&SettingsService>) -> TurnRules {
    TurnRules {
        follow_local_in_nested: follows_local(service),
        integrate_without_asking: service
            .and_then(|s| s.integration_settings().ok())
            .is_some_and(|integration| integration.agent_requests == AgentRequests::Auto),
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
#[tauri::command]
async fn agent_message(
    session_id: String,
    text: String,
    attachments: Option<Vec<AttachmentInput>>,
    reg: State<'_, Reg>,
    persist: State<'_, PersistState>,
    settings: State<'_, SettingsState>,
) -> Result<(), String> {
    let att = session(&reg, &session_id)?;
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
    let (history, todos, conversation_id) = att.begin_turn(&cancel)?;
    let store = persist.0.clone();
    // Only the LLM agent writes history, so only its turns are saved. The mock agent never touches the history, and it
    // ignores attachments.
    let (mut llm, saved) = match chosen {
        None => (None, None),
        Some((llm_settings, model)) => match agent_loop::shared_client() {
            Ok(client) => {
                // The images are on disk before the message is recorded, so a saved message never names a missing file.
                if let Err(e) = write_images(store.as_deref(), &conversation_id, &prepared.images) {
                    att.end_turn();
                    return Err(e);
                }
                let state = Arc::new(SavedState {
                    conversation_id: conversation_id.clone(),
                    history: history.clone(),
                    todos: todos.clone(),
                    model: Some(model),
                });
                let checkpoint = checkpoint_saver(store.clone(), att.clone(), state.clone());
                let mut agent = LlmAgent::new(client, llm_settings, &conversation_id, history, todos, cancel.clone())
                    // The note is taken only by a turn that calls the model: a turn that fails to start keeps it.
                    .with_resume_note(att.conversation.take_resume_note())
                    .with_checkpoint(checkpoint)
                    .with_limits(limits)
                    .with_no_vision(att.conversation.no_vision());
                if let Some(persist) = store.as_ref() {
                    agent = agent.with_attachments(persist.conversations.attachments().clone());
                }
                (Some(agent), Some(state))
            }
            Err(e) => {
                att.end_turn();
                return Err(e);
            }
        },
    };
    let message = UserMessage { text, attachments: prepared.attachments };
    let worker = att.clone();
    let spawned = thread::Builder::new().name("agent".into()).spawn(move || {
        // Runs on panic too, so a crashed turn cannot leave the agent busy or in control of the shell.
        let _end = OnDrop::new(|| worker.end_turn());
        let tools = SessionTools::new(&worker.session).with_cancel(cancel).with_wait_budget(limits.max_wait());
        let channels = worker.channels.clone();
        let owner = worker.clone();
        let emit = move |ev: AgentEvent| {
            if let AgentEvent::AgentControl { active } = &ev {
                owner.agent_control.store(*active, Ordering::SeqCst);
            }
            channels.send_json(&ev);
        };
        let ctx = AgentCtx::new(&tools, &worker.approvals, &emit).with_policy(policy.as_ref()).with_rules(rules);
        match llm.as_mut() {
            Some(agent) => agent::run_turn(agent, &message, &ctx),
            None => agent::run_turn(&mut MockAgent, &message, &ctx),
        }
        // The history is final now, and this is still before the turn is ended (the guard above). The end of a turn
        // is saved as finished, whether it completed, was cancelled, or failed.
        if let (Some(agent), Some(store), Some(state)) = (llm.as_ref(), store.as_ref(), saved.as_ref()) {
            save_conversation(store, &worker.session, state, agent.redactor(), false);
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
            eprintln!("watt: the last conversation was not forgotten ({e})");
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

/// Answers the dialog for a `request_integration` call: `accept` integrates the shell, anything else declines it.
#[tauri::command]
fn answer_integration(session_id: String, call_id: String, accept: bool, reg: State<'_, Reg>) -> Result<(), String> {
    if session(&reg, &session_id)?.approvals.integrations().decide(&call_id, accept) {
        Ok(())
    } else {
        Err(format!("no pending integration request {call_id}"))
    }
}

#[tauri::command]
fn set_auto_approve(session_id: String, on: bool, reg: State<'_, Reg>) -> Result<(), String> {
    // The shell on top is the level approve-all is tied to. The session supplies it, not the frontend.
    let att = session(&reg, &session_id)?;
    att.approvals.set_auto_approve(on, att.session.top_key());
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
    session_id: String,
    reg: State<'_, Reg>,
    settings: State<'_, SettingsState>,
) -> Result<AutoApproveState, String> {
    let att = session(&reg, &session_id)?;
    let state = att.approvals.auto_state_across(att.session.top_key(), follows_local(settings.0.as_deref()));
    Ok(AutoApproveState { auto: state != AutoState::Off, suspended: state == AutoState::Suspended })
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
    LlmStatus { enabled: env.is_some(), model: env.map(|s| s.model.clone()), provider: None }
}

#[tauri::command]
fn get_status(session_id: String, reg: State<'_, Reg>, settings: State<'_, SettingsState>) -> Result<Status, String> {
    let att = session(&reg, &session_id)?;
    // The saved default's label reads no secret. Without a saved default the environment settings are shown. While
    // settings.json cannot be used, the agent is off and the environment settings are not shown in its place.
    let (llm, settings_state) = match settings.0.as_ref() {
        Some(service) => {
            let llm = match service.default_label() {
                Ok(Some((provider, model))) => {
                    LlmStatus { enabled: true, model: Some(model), provider: Some(provider) }
                }
                Ok(None) => env_llm(),
                Err(_) => LlmStatus { enabled: false, model: None, provider: None },
            };
            (llm, Some(service.settings_state()))
        }
        None => (env_llm(), None),
    };
    let levels = att.session.levels();
    let depth = levels.len();
    Ok(Status {
        state: att.session.state(),
        foreground: att.session.foreground(),
        levels: levels.into_iter().skip(1).collect(),
        depth,
        integration: att.session.integration_status(),
        observer: att.session.observer_status(),
        cwd: att.session.cwd(),
        stats: att.session.stats(),
        auto_approve: att.approvals.auto_approve(),
        auto_approve_suspended: att.approvals.auto_state_across(att.session.top_key(), follows_local(settings.0.as_deref()))
            == AutoState::Suspended,
        nonces: att.session.nonces(),
        agent_busy: att.agent_busy.load(Ordering::SeqCst),
        agent_control: att.agent_control.load(Ordering::SeqCst),
        llm,
        conversation_id: att.conversation.id(),
        settings_state,
    })
}

/// Integrates the shell in the foreground (ssh, a nested shell, a container exec) as a new level. The handshake waits for
/// the shell, so it runs on the blocking pool. See `Session::integrate_shell`. Nothing calls it from the UI yet (B4).
#[tauri::command]
async fn integrate_shell(session_id: String, reg: State<'_, Reg>) -> Result<IntegrateOutcome, String> {
    let att = session(&reg, &session_id)?;
    blocking(move || Ok(att.session.integrate_shell())).await
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
    /// The todo list the conversation ended with (repaired). Empty when it had none.
    todos: Vec<Todo>,
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
    let todos = resumed.todos;
    let in_turn = resumed.in_turn;
    {
        // The busy check and the restore happen under the turn-cancel lock, as `begin_turn` and `new_conversation` do.
        let _turn = lock(&att.turn_cancel);
        if att.agent_busy.load(Ordering::SeqCst) {
            return Err(BUSY_REFUSAL.into());
        }
        att.conversation.restore(id.clone(), resumed.history, todos.clone(), in_turn);
    }
    let remember_id = id.clone();
    let remember = blocking(move || {
        let patch = UiStatePatch { last_conversation_id: Some(Some(remember_id)), ..UiStatePatch::default() };
        store.ui.patch(patch).map(|_| ()).map_err(|e| e.to_string())
    });
    if let Err(e) = remember.await {
        eprintln!("watt: the last conversation was not remembered ({e})");
    }
    Ok(LoadedConversation { id, title, messages, todos })
}

/// What a delete reports: whether a live conversation was among those deleted and was reset to a new one.
#[derive(Serialize)]
struct DeleteResult {
    deleted_live: bool,
}

/// Deletes saved conversations (`target`: one id, or `None`: all of them) and resets the live conversations that
/// the deletion covers, as one step. Refused while any agent is busy. Every session's turn-cancel lock is held from
/// the busy check to the end of the reset, as `begin_turn` and `new_conversation` do, so no turn starts in between.
/// A reset live conversation gets a new id, so a later save cannot write a deleted conversation back. The reset
/// happens even when the file deletion fails part way, for the same reason. Returns whether a live conversation was
/// reset.
fn delete_saved(sessions: &[Arc<Attached>], store: &Persist, target: Option<&str>) -> Result<bool, String> {
    let _turns: Vec<_> = sessions.iter().map(|att| lock(&att.turn_cancel)).collect();
    if sessions.iter().any(|att| att.agent_busy.load(Ordering::SeqCst)) {
        return Err(BUSY_REFUSAL.into());
    }
    let deleted = match target {
        Some(id) => store.conversations.delete(id),
        None => store.conversations.delete_all(),
    };
    let mut reset = false;
    for att in sessions {
        if target.is_none_or(|id| att.conversation.id() == id) {
            att.conversation.reset().map_err(io_err)?;
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
    reg: State<'_, Reg>,
    persist: State<'_, PersistState>,
) -> Result<DeleteResult, String> {
    let store = persist.get()?;
    let sessions = reg.snapshot();
    blocking(move || {
        let deleted_live = delete_saved(&sessions, &store, Some(&id))?;
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
    reg: State<'_, Reg>,
    persist: State<'_, PersistState>,
) -> Result<DeleteResult, String> {
    let store = persist.get()?;
    let sessions = reg.snapshot();
    blocking(move || {
        let deleted_live = delete_saved(&sessions, &store, None)?;
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

pub fn run() {
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
        // Saves the window's size, position and maximized state on exit and restores them at launch.
        // Visibility, decorations and fullscreen are not tracked: Tauri shows the window itself.
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .build(),
        )
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
            auto_approve_state,
            get_status,
            integrate_shell,
            kill_session,
            list_conversations,
            load_conversation,
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
            set_limits_settings,
            answer_integration,
            test_approval,
            approval_builtin_deny,
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
