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
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;

use crate::core::foreground::Foreground;
use crate::core::inject::{self, END_MARKER};
use crate::core::observer::{
    run_observer, Activity, CallStatus, Core, EventSink, InjectRefusal, InputState, IntegrationStatus, LevelInfo,
    ObsMsg, ObsQueue, ObserverStatus, OpenAgentRecord, ScreenLines, ScreenSnapshot, Shared,
};
use crate::core::pty::{self, Gate, OutputSink};
use crate::core::record::{Cwd, Record, RecordSummary, Source};
use crate::core::shim;

/// Ctrl-C is sent before the loader. The loader is typed once the output has been quiet this long (or the cap passes).
const QUIET_FOR: Duration = Duration::from_millis(300);
const QUIET_CAP: Duration = Duration::from_secs(2);
/// How long the loader has to say Ready, and how long the new level has to say Hello after its blob.
const READY_WAIT: Duration = Duration::from_secs(5);
const PUSH_WAIT: Duration = Duration::from_secs(5);
const INJECT_POLL: Duration = Duration::from_millis(20);
/// The line the loader prints (alone on its row) when it refuses the shell.
const UNSUPPORTED_LINE: &str = "Unsupported shell";
/// The outcome text when the shell in the foreground is still starting. Ctrl-C would stop it, so nothing is typed.
pub const STARTING_HINT: &str = "the shell is still starting, so Ctrl-C could stop it: nothing was typed. Wait a few seconds \
     (get_screen shows what it is doing), then call request_integration again";

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
    /// The top level is bash, and the text has a newline. bash's multi-line history is joined or split, so the
    /// observer cannot attribute it safely. Nothing was written.
    MultiLineAtBash,
    /// The shell on top is not the one the command was approved for (it exited, or another shell started). Nothing was
    /// written.
    LocationChanged,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::NotAtPrompt(s) => write!(f, "session is not at a prompt (state: {s:?})"),
            SubmitError::UserTyped => write!(f, "{}", user_text_refusal()),
            SubmitError::ObserverDesynced => write!(f, "observer is desynced; wait for the next prompt"),
            SubmitError::Closed => write!(f, "session is closed"),
            SubmitError::MultiLineAtBash => write!(
                f,
                "this shell is bash and cannot take multi-line commands safely; send one line, or join with ;"
            ),
            SubmitError::LocationChanged => write!(
                f,
                "the shell you were working in has changed (it exited or another shell started); the command was not run, check the screen and decide again"
            ),
        }
    }
}

/// What `Session::integrate_shell` reports. Tagged for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum IntegrateOutcome {
    /// The shell is integrated as a new level, and it is on top.
    Integrated { key: u64, label: String, sh: String, user: Option<String>, host: Option<String> },
    /// The loader refused the shell (bash older than 4.4, posix mode, or promptvars off). Nothing was installed.
    UnsupportedShell,
    /// The loader did not say Ready in time. Ctrl-C was sent.
    Timeout,
    /// The blob went in, but the level did not appear. `hint` is the shell's last message, or the last screen line.
    /// Ctrl-C was sent and the pending level dropped.
    Failed { hint: String },
    /// Not started. See the reason.
    Refused { reason: InjectRefusal },
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct SessionStats {
    pub forged_events: u64,
    /// Tagged events from popped shell levels (not forged, just late).
    pub stale_events: u64,
    pub overflow_events: u64,
    pub dropped_bytes: u64,
}

/// Input to the writer thread.
enum WriteCmd {
    /// Raw keystrokes or bytes from the user. A CR at the prompt starts a submitted line, so the gate closes until C
    /// (or the submit timeout, for an empty line).
    User(Vec<u8>),
    /// `expected_level` is the level key the command was approved for. None means the top level whatever it is.
    Submit { source: Source, text: String, expected_level: Option<u64>, reply: Sender<Result<(), SubmitError>> },
    /// Bytes for the injection (Ctrl-C, the loader, the blob lines). Written as they are: no submit gate, no typed-text
    /// bookkeeping. Only `integrate_shell` sends these.
    Inject(Vec<u8>),
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
        self.submit_bound(source, text, None)
    }

    /// Like `submit`, but refused with `LocationChanged` unless the top level is `expected_level` when the writer takes the
    /// line. The agent binds an approved command to the shell it was carded for, so a shell that changed in between
    /// gets nothing.
    pub fn submit_bound(&self, source: Source, text: &str, expected_level: Option<u64>) -> Result<(), SubmitError> {
        let (reply, got) = mpsc::channel();
        self.wtx
            .send(WriteCmd::Submit { source, text: text.to_string(), expected_level, reply })
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

    /// The foreground program hint while a command runs: what it is, and that it has no shell integration when it is a
    /// remote login or a nested shell. See core/foreground.rs. None at a prompt and in the other states.
    pub fn foreground(&self) -> Option<Foreground> {
        self.shared.lock().foreground()
    }

    /// Every shell level, the local one first. Only the status reads it, to show the nested levels.
    pub fn levels(&self) -> Vec<LevelInfo> {
        self.shared.lock().levels()
    }

    /// The key of the level on top (0 is the local shell).
    pub fn top_key(&self) -> u64 {
        self.shared.lock().top_key()
    }

    /// The label of the nested level on top (the command that started it). None on the local shell.
    pub fn location(&self) -> Option<String> {
        self.shared.lock().location()
    }

    /// Every nonce the observer accepts now (the local shell's and the live levels'). The UI checks OSC events against them.
    pub fn nonces(&self) -> Vec<String> {
        self.shared.lock().accepted_nonces()
    }

    /// Why `integrate_shell` would refuse right now, or None. Writes nothing.
    pub fn integration_refusal(&self) -> Option<InjectRefusal> {
        self.shared.lock().injection_refusal()
    }

    /// True while the program in the foreground is still starting, so `integrate_shell` would not type yet (see
    /// `Core::integration_starting`). Writes nothing.
    pub fn integration_starting(&self) -> bool {
        self.shared.lock().integration_starting(Instant::now())
    }

    /// Integrates the shell that runs in the foreground (ssh, a nested shell, su, a container exec) as a new level. It
    /// types Ctrl-C and the loader, waits for the shell's Ready, then writes the blob with a fresh nonce and waits for the
    /// new level's Hello. Blocking: call it from a blocking thread. One at a time; a second call is refused as Busy.
    /// The core lock is never held while writing or sleeping.
    pub fn integrate_shell(&self) -> IntegrateOutcome {
        let token = match inject::new_token() {
            Ok(token) => token,
            Err(e) => return IntegrateOutcome::Failed { hint: format!("no random bytes for the handshake: {e}") },
        };
        let label = {
            let mut core = self.shared.lock();
            // Checked after the refusals, so a shell that is not there at all still gets its own reason.
            if core.injection_refusal().is_none() && core.integration_starting(Instant::now()) {
                return IntegrateOutcome::Failed { hint: STARTING_HINT.into() };
            }
            if let Err(reason) = core.begin_injection(&token) {
                return IntegrateOutcome::Refused { reason };
            }
            core.foreground().map(|fg| fg.command).unwrap_or_default()
        };
        let outcome = self.run_injection(&token, &label);
        // Only reached once begin_injection succeeded, so the record here is this call's own.
        self.shared.lock().end_injection();
        outcome
    }

    /// The handshake after `begin_injection`. Every early return leaves the shell as it was, or sends Ctrl-C.
    fn run_injection(&self, token: &str, label: &str) -> IntegrateOutcome {
        // Ctrl-C clears whatever the shell has on its line, then the loader is typed once the output settles.
        self.inject(vec![0x03]);
        self.wait_quiet();
        self.inject(format!("{}\r", inject::loader_line(token)).into_bytes());

        let loader = self.poll_until(READY_WAIT, |s| {
            let ready = s.shared.lock().injection_ready();
            if let Some(sh) = ready {
                return Some(Ok(sh));
            }
            s.shows_line(UNSUPPORTED_LINE).then_some(Err(()))
        });
        let sh = match loader {
            Some(Ok(sh)) => sh,
            // The loader printed its refusal and exited. Nothing was written, so no Ctrl-C.
            Some(Err(())) => return IntegrateOutcome::UnsupportedShell,
            None => {
                self.inject(vec![0x03]);
                return IntegrateOutcome::Timeout;
            }
        };

        let nonce = match inject::new_token() {
            Ok(nonce) => nonce,
            Err(e) => return self.abort(format!("no random bytes for the nonce: {e}")),
        };
        let lines = match inject::blob_lines(&nonce, &sh) {
            Ok(lines) => lines,
            Err(e) => return self.abort(e.to_string()),
        };
        let key = self.shared.lock().expect_level(&nonce, label);
        // The Levels event carries the new nonce. It must reach the listener before any blob byte goes out.
        self.shared.flush();
        for line in &lines {
            self.inject(format!("{line}\r").into_bytes());
        }
        self.inject(format!("{END_MARKER}\r").into_bytes());

        let pushed = self.poll_until(PUSH_WAIT, |s| (s.shared.lock().top_key() == key).then_some(())).is_some();
        if !pushed {
            return self.abort(self.last_message());
        }
        let level = self.shared.lock().levels().into_iter().find(|l| l.key == key);
        match level {
            Some(level) => IntegrateOutcome::Integrated {
                key,
                label: level.label,
                sh,
                user: level.user,
                host: level.host,
            },
            None => self.abort(self.last_message()),
        }
    }

    /// Drops the pending level and sends Ctrl-C, for a handshake that did not finish. Returns the Failed outcome.
    fn abort(&self, hint: String) -> IntegrateOutcome {
        {
            let mut core = self.shared.lock();
            core.cancel_pending_level();
        }
        self.shared.flush();
        self.inject(vec![0x03]);
        IntegrateOutcome::Failed { hint }
    }

    /// Queues bytes for the writer (no submit gate, no typed-text bookkeeping).
    fn inject(&self, bytes: Vec<u8>) {
        let _ = self.wtx.send(WriteCmd::Inject(bytes));
    }

    /// Returns when the screen has not changed for `QUIET_FOR`, or after `QUIET_CAP`.
    fn wait_quiet(&self) {
        let start = Instant::now();
        let mut seen = self.activity().screen_gen;
        let mut changed_at = start;
        loop {
            thread::sleep(INJECT_POLL);
            let now_gen = self.activity().screen_gen;
            if now_gen != seen {
                seen = now_gen;
                changed_at = Instant::now();
            }
            if changed_at.elapsed() >= QUIET_FOR || start.elapsed() >= QUIET_CAP {
                return;
            }
        }
    }

    /// Calls `f` every `INJECT_POLL` until it returns Some, or `within` passes.
    fn poll_until<T>(&self, within: Duration, mut f: impl FnMut(&Session) -> Option<T>) -> Option<T> {
        let end = Instant::now() + within;
        loop {
            if let Some(v) = f(self) {
                return Some(v);
            }
            if Instant::now() >= end {
                return None;
            }
            thread::sleep(INJECT_POLL);
        }
    }

    /// True when a screen row is exactly `text` (trimmed). The loader's own echo holds the same words inside a longer line,
    /// so a substring match would fire on the echo.
    fn shows_line(&self, text: &str) -> bool {
        self.screen_snapshot().rows.iter().any(|row| row.trim() == text)
    }

    /// The shell's last message for a failed handshake: the last row that starts with `agentterm:`, else the last non-blank row.
    fn last_message(&self) -> String {
        let rows = self.screen_snapshot().rows;
        let tagged = rows.iter().rev().find(|r| r.contains("agentterm:"));
        let any = rows.iter().rev().find(|r| !r.trim().is_empty());
        tagged.or(any).map(|r| r.trim().to_string()).unwrap_or_default()
    }

    pub fn observer_status(&self) -> ObserverStatus {
        self.shared.lock().observer_status()
    }

    pub fn stats(&self) -> SessionStats {
        let (forged_events, stale_events) = {
            let core = self.shared.lock();
            (core.forged_events(), core.stale_events())
        };
        let (overflow_events, dropped_bytes) = self.queue.overflow_stats();
        SessionStats { forged_events, stale_events, overflow_events, dropped_bytes }
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

    /// The summary of a finished record, by id.
    pub fn record_summary(&self, id: u64) -> Option<RecordSummary> {
        self.shared.lock().record_summary(id)
    }

    /// True once the record with this id has finished (and it is still kept).
    pub fn has_finished_record(&self, id: u64) -> bool {
        self.shared.lock().has_finished_record(id)
    }

    /// True while a record with this id is open (any source, any level).
    pub fn record_is_open(&self, id: u64) -> bool {
        self.shared.lock().record_is_open(id)
    }

    /// The agent's own open commands, the top level's and the parked ones (see `OpenAgentRecord`).
    pub fn open_agent_records(&self) -> Vec<OpenAgentRecord> {
        self.shared.lock().open_agent_records()
    }

    /// The last `n` logical lines of the screen and its history (see `Core::screen_lines`).
    pub fn screen_lines(&self, n: usize) -> ScreenLines {
        self.shared.lock().screen_lines(n)
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
            WriteCmd::Submit { source, text, expected_level, reply } => {
                let prepared = {
                    let mut core = shared.lock();
                    if core.desynced() {
                        Err(SubmitError::ObserverDesynced)
                    } else if expected_level.is_some_and(|key| key != core.top_key()) {
                        Err(SubmitError::LocationChanged)
                    } else {
                        match core.lifecycle_view() {
                            // The user's text is on the line: refuse, and write nothing.
                            InputState::Prompt if core.user_typed_since_prompt() => Err(SubmitError::UserTyped),
                            InputState::Prompt if text.contains('\n') && core.top_sh() == Some("bash") => {
                                Err(SubmitError::MultiLineAtBash)
                            }
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
            WriteCmd::Inject(bytes) => {
                // The injection's own bytes: no gate, no typed-text bookkeeping (see `integrate_shell`).
                let _ = write_all(&mut writer, &bytes);
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
