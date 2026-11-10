//! A shell session: one `zsh -l` on a PTY, with its reader, observer and writer threads.
//!
//! Input goes through one writer thread. The submit gate (Prompt -> Submitted) is checked and set
//! under the core lock, then the bytes are written by that same thread, so two submits can never
//! interleave. Output and observation never wait on input.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;

use crate::core::observer::{
    run_observer, Activity, CallStatus, Core, EventSink, InputState, IntegrationStatus, ObsMsg, ObsQueue,
    ObserverStatus, ScreenSnapshot, Shared,
};
use crate::core::pty::{self, Gate, OutputSink};
use crate::core::record::{Cwd, Record, RecordSummary, Source};
use crate::core::shim;

#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub rows: u16,
    pub cols: u16,
    /// Directory holding the user's startup files. None: the app's own ZDOTDIR if set, else $HOME.
    /// Tests use this to keep the user's history file out of reach (zsh appends every command to HISTFILE).
    pub real_zdotdir: Option<PathBuf>,
    /// Dev only: a file to use as HISTFILE instead of the real one (the Tauri layer sets this from
    /// AGENT_TERM_HISTFILE). None keeps the user's own history file.
    pub histfile: Option<PathBuf>,
}

/// Stable code at the start of the error `run_command` returns when the user has typed text on the prompt. The
/// MockAgent matches on it, so keep it unchanged.
pub const USER_TEXT_CODE: &str = "prompt_has_user_text";

/// The refusal text for a run_command at a prompt the user has typed on. Nothing is written to the PTY.
pub fn user_text_refusal() -> String {
    format!("{USER_TEXT_CODE}: the prompt has text typed by the user; ask them to clear it (Ctrl-C) and try again")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    /// The session is not at an idle prompt. Carries the state it is in.
    NotAtPrompt(InputState),
    /// The user has typed text on the prompt line (see `Core::note_user_bytes`). Nothing was written.
    UserTyped,
    /// The observer lost bytes and has not seen a prompt since.
    ObserverDesynced,
    /// The session has no writer any more (the shell exited or the session was closed).
    Closed,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::NotAtPrompt(s) => write!(f, "session is not at a prompt (state: {s:?})"),
            SubmitError::UserTyped => write!(f, "{}", user_text_refusal()),
            SubmitError::ObserverDesynced => write!(f, "observer is desynced; wait for the next prompt"),
            SubmitError::Closed => write!(f, "session is closed"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SessionStats {
    pub forged_events: u64,
    pub overflow_events: u64,
    pub dropped_bytes: u64,
}

/// Input to the writer thread.
enum WriteCmd {
    /// Raw keystrokes or bytes from the user. A CR at the prompt starts a submitted line, so the gate closes until C
    /// (or the submit timeout, for an empty line).
    User(Vec<u8>),
    Submit { source: Source, text: String, reply: Sender<Result<(), SubmitError>> },
}

pub struct Session {
    shared: Arc<Shared>,
    queue: Arc<ObsQueue>,
    gate: Arc<Gate>,
    wtx: Sender<WriteCmd>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    shell_pid: Option<u32>,
    shim_dir: PathBuf,
    /// The integration nonce the shell was given. Never logged; the frontend uses it only to filter OSC payloads.
    nonce: String,
}

impl Session {
    /// Starts `zsh -l` with the integration shim. `id` names the shim directory; keep it short.
    pub fn spawn(id: &str, cfg: SpawnConfig, sink: Box<dyn OutputSink>) -> io::Result<Session> {
        let nonce = random_nonce()?;
        let shim_dir = shim::dir_for(id);
        shim::write_to(&shim_dir)?;

        let pair = native_pty_system().openpty(PtySize { rows: cfg.rows, cols: cfg.cols, pixel_width: 0, pixel_height: 0 }).map_err(to_io)?;
        let mut cmd = CommandBuilder::new("zsh");
        cmd.arg("-l");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("AGENTTERM_NONCE", &nonce);
        cmd.env("ZDOTDIR", &shim_dir);
        // The user's real ZDOTDIR. Empty means $HOME.
        let real = match &cfg.real_zdotdir {
            Some(dir) => dir.clone().into_os_string(),
            None => std::env::var_os("ZDOTDIR").unwrap_or_default(),
        };
        cmd.env("AGENTTERM_REAL_ZDOTDIR", real);
        if let Some(histfile) = &cfg.histfile {
            cmd.env("AGENTTERM_HISTFILE", histfile);
        }
        let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
        // The parent must drop the slave, or EOF is never seen on the master.
        drop(pair.slave);
        let shell_pid = child.process_id();
        let reader = pair.master.try_clone_reader().map_err(to_io)?;
        let writer = pair.master.take_writer().map_err(to_io)?;

        let shared = Arc::new(Shared::new(Core::new(&nonce, (cfg.rows, cfg.cols))));
        let queue = Arc::new(ObsQueue::new());
        let gate = Arc::new(Gate::new());

        // Reader, pump, observer and writer threads exit on their own once the PTY reaches EOF and their channels close.
        pty::start_threads(reader, gate.clone(), sink, queue.clone())?;
        {
            let (q, s) = (queue.clone(), shared.clone());
            thread::Builder::new().name("observer".into()).spawn(move || run_observer(q, s))?;
        }
        let (wtx, wrx) = mpsc::channel::<WriteCmd>();
        {
            let s = shared.clone();
            thread::Builder::new().name("pty-writer".into()).spawn(move || run_writer(wrx, writer, s))?;
        }

        Ok(Session {
            shared,
            queue,
            gate,
            wtx,
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            shell_pid,
            shim_dir,
            nonce,
        })
    }

    /// The integration nonce. Stable for the life of the session.
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Writes `text` plus CR at an idle prompt, atomically: the prompt check and the Submitted transition
    /// happen in the writer thread, under the same lock, right before the bytes go out.
    pub fn submit(&self, source: Source, text: &str) -> Result<(), SubmitError> {
        let (reply, got) = mpsc::channel();
        self.wtx
            .send(WriteCmd::Submit { source, text: text.to_string(), reply })
            .map_err(|_| SubmitError::Closed)?;
        got.recv().map_err(|_| SubmitError::Closed)?
    }

    /// True while the user has typed text on the prompt line that is still there. A pre-check only: `submit` checks the
    /// same flag again under the writer's lock, right before it writes.
    pub fn user_typed_since_prompt(&self) -> bool {
        self.shared.lock().user_typed_since_prompt()
    }

    /// Raw bytes from the user (keystrokes). Only the frontend decides when these are allowed.
    pub fn write_user(&self, bytes: &[u8]) -> io::Result<()> {
        self.wtx
            .send(WriteCmd::User(bytes.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "session closed"))
    }

    /// Resizes the PTY and tells the observer, in order with the bytes already queued.
    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        self.master
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(to_io)?;
        self.queue.push_control(ObsMsg::Resize { rows, cols });
        Ok(())
    }

    /// Called by the renderer after it has rendered `bytes` of output.
    pub fn ack(&self, bytes: usize) {
        self.gate.ack(bytes);
    }

    /// Installs the listener for state, status, cwd and record events. Replaces any earlier one.
    pub fn set_event_sink(&self, sink: Arc<dyn EventSink>) {
        self.shared.set_sink(sink);
    }

    pub fn state(&self) -> InputState {
        self.shared.lock().lifecycle_view()
    }

    pub fn integration_status(&self) -> IntegrationStatus {
        self.shared.lock().integration_status()
    }

    pub fn observer_status(&self) -> ObserverStatus {
        self.shared.lock().observer_status()
    }

    pub fn stats(&self) -> SessionStats {
        let forged_events = self.shared.lock().forged_events();
        let (overflow_events, dropped_bytes) = self.queue.overflow_stats();
        SessionStats { forged_events, overflow_events, dropped_bytes }
    }

    pub fn records(&self) -> Vec<RecordSummary> {
        self.shared.lock().records()
    }

    /// Where an agent call stands, from one lock read. Use this, not `records` and `state` separately: they lock
    /// apart, so a poll built from them can see the two halves of a change at different times.
    pub fn call_status(&self, call_id: &str, input_wait: Duration) -> CallStatus {
        self.shared.lock().call_status(call_id, input_wait)
    }

    /// The shell state, the screen generation and the newest finished record, from one lock read. A wait polls this.
    pub fn activity(&self) -> Activity {
        self.shared.lock().activity()
    }

    pub fn get_record(&self, id: u64) -> Option<Record> {
        self.shared.lock().get_record(id)
    }

    pub fn screen_snapshot(&self) -> ScreenSnapshot {
        self.shared.lock().screen_snapshot()
    }

    pub fn cwd(&self) -> Option<Cwd> {
        self.shared.lock().cwd()
    }

    /// The shell's pid, from the spawn. Used for teardown.
    pub fn shell_pid(&self) -> Option<u32> {
        self.shell_pid
    }

    /// The per-session directory holding the shim files (removed on drop).
    pub fn shim_dir(&self) -> &std::path::Path {
        &self.shim_dir
    }

    /// SIGKILLs the shell and its direct children. SIGKILL (not SIGHUP) so zsh does not rewrite the user's history on exit.
    pub fn kill(&self) {
        if let Some(pid) = self.shell_pid {
            kill_hard(pid);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.gate.close();
        self.kill();
        if let Ok(mut child) = self.child.lock() {
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.shim_dir);
    }
}

fn kill_hard(pid: u32) {
    let _ = Command::new("pkill").args(["-KILL", "-P", &pid.to_string()]).status();
    let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
}

/// Serializes writes to the PTY. Gate check and Submitted transition happen under the core lock, then the bytes go out.
fn run_writer(rx: Receiver<WriteCmd>, mut writer: Box<dyn Write + Send>, shared: Arc<Shared>) {
    for cmd in rx {
        match cmd {
            WriteCmd::User(bytes) => {
                // The lock is released before the write: a blocked PTY must not hold up the observer.
                let submitted = {
                    let mut core = shared.lock();
                    core.note_user_bytes(&bytes);
                    if bytes.contains(&b'\r') && !core.desynced() {
                        match core.lifecycle_view() {
                            // The first CR at an idle prompt closes the gate, so the agent cannot inject into the same
                            // line. C (or the submit timeout, for an empty line) reopens it.
                            InputState::Prompt => {
                                core.begin_submit(Some((Source::User, None)));
                                true
                            }
                            // A CR completing a multi-line command the user is typing: attribute it to the user.
                            InputState::Continuation => {
                                core.note_user_line();
                                false
                            }
                            _ => false,
                        }
                    } else {
                        false
                    }
                };
                if submitted {
                    shared.flush();
                }
                let _ = write_all(&mut writer, &bytes);
            }
            WriteCmd::Submit { source, text, reply } => {
                let prepared = {
                    let mut core = shared.lock();
                    if core.desynced() {
                        Err(SubmitError::ObserverDesynced)
                    } else {
                        match core.lifecycle_view() {
                            // The user's text is on the line: refuse, and write nothing.
                            InputState::Prompt if core.user_typed_since_prompt() => Err(SubmitError::UserTyped),
                            InputState::Prompt => {
                                let bytes = encode_submit(&text, core.bracketed_paste());
                                core.begin_submit(Some((source, Some(text))));
                                Ok(bytes)
                            }
                            other => Err(SubmitError::NotAtPrompt(other)),
                        }
                    }
                };
                if prepared.is_ok() {
                    shared.flush();
                }
                let result = match prepared {
                    Ok(bytes) => write_all(&mut writer, &bytes).map_err(|_| SubmitError::Closed),
                    Err(e) => Err(e),
                };
                let _ = reply.send(result);
            }
        }
    }
}

fn write_all(w: &mut dyn Write, bytes: &[u8]) -> io::Result<()> {
    w.write_all(bytes)?;
    w.flush()
}

/// Bracketed paste when the shell enabled it (multi-line text then stays one line edit), plain text otherwise.
/// ESC is removed from the text so it cannot close the paste early or send other escape sequences.
fn encode_submit(text: &str, bracketed: bool) -> Vec<u8> {
    let clean = text.replace('\x1b', "");
    if bracketed {
        format!("\x1b[200~{clean}\x1b[201~\r").into_bytes()
    } else {
        format!("{clean}\r").into_bytes()
    }
}

fn random_nonce() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn to_io(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
