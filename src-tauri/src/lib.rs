//! Tauri wiring only. All logic lives in `core`; this file maps commands and channels onto it.
//!
//! Channels, per session: `on_data` carries raw PTY bytes (`InvokeResponseBody::Raw`, which the frontend
//! receives as an ArrayBuffer) and must be acknowledged with `ack`. `on_events` carries JSON objects:
//! session events (`state`, `integration`, `observer`, `cwd`, `record_started`, `record_finished`) and
//! agent events (`text`, `tool_call`, `tool_result`, `approval_request`, `done`), told apart by `type`.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use serde::Serialize;
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Manager, RunEvent, State};

// Core logic (PTY, shell integration, observer, agent) has no Tauri dependency, so `cargo test` runs it headless.
pub mod core;

use crate::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate, MockAgent};
use crate::core::observer::{EventSink, InputState, IntegrationStatus, ObserverStatus, SessionEvent};
use crate::core::pty::OutputSink;
use crate::core::record::{Cwd, Source};
use crate::core::registry::Registry;
use crate::core::session::{Session, SessionStats, SpawnConfig};
use crate::core::tools::SessionTools;

type Reg = Registry<Attached>;

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
    let mut pending = Some((on_data, on_events));
    let (att, spawned) = reg.get_or_insert_with(&session_id, || -> Result<Attached, String> {
        let (data, events) = pending.take().ok_or("channels already used")?;
        let channels = Arc::new(Channels::default());
        channels.attach(data, events);
        let cfg = SpawnConfig { rows, cols, real_zdotdir: None, histfile: dev_histfile() };
        let session = Session::spawn(&session_id, cfg, Box::new(RawSink(channels.clone()))).map_err(io_err)?;
        session.set_event_sink(Arc::new(EventsSink(channels.clone())));
        Ok(Attached { session, channels, approvals: ApprovalGate::new(), agent_busy: AtomicBool::new(false) })
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

/// Keystrokes from the terminal, as text (sent as UTF-8). Ctrl-C also cancels pending approvals.
#[tauri::command]
fn write_user(session_id: String, text: String, reg: State<'_, Reg>) -> Result<(), String> {
    let att = session(&reg, &session_id)?;
    agent::user_input(&att.session, &att.approvals, &text).map_err(io_err)
}

/// A line typed in the input bar (the frontend strips a leading `!`). Waits for the writer, so it runs off the UI thread.
#[tauri::command]
async fn submit_input(session_id: String, text: String, reg: State<'_, Reg>) -> Result<(), String> {
    let att = session(&reg, &session_id)?;
    tauri::async_runtime::spawn_blocking(move || att.session.submit(Source::User, &text).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Starts one agent turn on its own thread. Events arrive on the events channel.
#[tauri::command]
fn agent_message(session_id: String, text: String, reg: State<'_, Reg>) -> Result<(), String> {
    let att = session(&reg, &session_id)?;
    if att.agent_busy.swap(true, Ordering::SeqCst) {
        return Err("the agent is still working on the previous message".into());
    }
    let worker = att.clone();
    let spawned = thread::Builder::new().name("agent".into()).spawn(move || {
        let tools = SessionTools::new(&worker.session);
        let channels = worker.channels.clone();
        let emit = move |ev: AgentEvent| channels.send_json(&ev);
        let ctx = AgentCtx::new(&tools, &worker.approvals, &emit);
        agent::run_turn(&mut MockAgent, &text, &ctx);
        worker.agent_busy.store(false, Ordering::SeqCst);
    });
    if let Err(e) = spawned {
        att.agent_busy.store(false, Ordering::SeqCst);
        return Err(e.to_string());
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

#[derive(Serialize)]
struct Status {
    state: InputState,
    integration: IntegrationStatus,
    observer: ObserverStatus,
    cwd: Option<Cwd>,
    stats: SessionStats,
    auto_approve: bool,
    agent_busy: bool,
}

#[tauri::command]
fn get_status(session_id: String, reg: State<'_, Reg>) -> Result<Status, String> {
    let att = session(&reg, &session_id)?;
    Ok(Status {
        state: att.session.state(),
        integration: att.session.integration_status(),
        observer: att.session.observer_status(),
        cwd: att.session.cwd(),
        stats: att.session.stats(),
        auto_approve: att.approvals.auto_approve(),
        agent_busy: att.agent_busy.load(Ordering::SeqCst),
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

pub fn run() {
    let app = tauri::Builder::default()
        .manage(Reg::new())
        .invoke_handler(tauri::generate_handler![
            start_session,
            ack,
            resize,
            write_user,
            submit_input,
            agent_message,
            approve_tool,
            set_auto_approve,
            get_status,
            kill_session
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
