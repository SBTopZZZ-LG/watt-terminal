//! The observer: the agent's view of the session.
//!
//! The PTY pump never waits for this module. It pushes bytes into `ObsQueue`, which is byte-bounded.
//! When the queue would overflow, the queued bytes are dropped and a `Resync` marker takes their place.
//! The observer then rebuilds its parser, reports `Desynced`, and trusts events again at the next
//! prompt. Terminal output is unaffected: the renderer receives every byte separately.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU16;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use vt100::capture::{basic_formatted_to_plain, BasicFormattedCaptureState, RowContents};
use vt100::{Callbacks, Parser};

use crate::core::foreground::{self, Foreground, ForegroundKind};
use crate::core::record::{Cwd, OutputBuilder, RawCapture, Record, RecordStatus, RecordSummary, Source};
use crate::core::scanner::{Event, Found, Scanner};

/// Bytes the observer may hold before it desyncs. Matches the design in plan.md.
pub const OBSERVER_QUEUE_CAP: usize = 4 << 20;
/// No integration event within this long after spawn means the shell is not observable (`Raw`).
pub const INTEGRATION_TIMEOUT: Duration = Duration::from_secs(5);
/// A submitted line that never reaches C is released after this long, so the gate cannot stick. At a nested level it
/// becomes `Unresponsive` instead of `Prompt`, and only that level's C or A ends it.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Shell levels the session may hold at once, the local shell included. A push beyond this is refused and counted.
pub const MAX_DEPTH: usize = 8;
/// A level expected by `expect_level` must say Hello within this long. Otherwise its nonce is retired.
pub const PENDING_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest label kept for a nested level.
pub const LABEL_CAP: usize = 1000;
/// The key of the local shell's level.
pub const LOCAL_KEY: u64 = 0;
/// The command text of a nested record whose text the shell did not report (bash does not record every command).
pub const UNAVAILABLE_COMMAND: &str = "(command text unavailable)";

// ---------- queue ----------

pub enum ObsMsg {
    Bytes(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    /// Queued bytes were dropped before this point. Rebuild the parser.
    Resync,
    Eof,
}

#[derive(Default)]
struct QueueState {
    q: VecDeque<ObsMsg>,
    queued_bytes: usize,
    overflow_events: u64,
    dropped_bytes: u64,
}

pub struct ObsQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

impl ObsQueue {
    pub fn new() -> Self {
        ObsQueue { state: Mutex::new(QueueState::default()), cv: Condvar::new() }
    }

    /// Never blocks. On overflow, drops queued bytes and leaves one `Resync` marker. Resize and
    /// Resync messages are kept, so the parser's size stays right.
    pub fn push_bytes(&self, bytes: Vec<u8>) {
        let mut s = self.lock();
        if s.queued_bytes + bytes.len() <= OBSERVER_QUEUE_CAP {
            s.queued_bytes += bytes.len();
            s.q.push_back(ObsMsg::Bytes(bytes));
        } else {
            s.overflow_events += 1;
            s.dropped_bytes += (s.queued_bytes + bytes.len()) as u64;
            s.q.retain(|m| matches!(m, ObsMsg::Resize { .. } | ObsMsg::Resync));
            s.queued_bytes = 0;
            if !matches!(s.q.back(), Some(ObsMsg::Resync)) {
                s.q.push_back(ObsMsg::Resync);
            }
        }
        drop(s);
        self.cv.notify_one();
    }

    /// Resize and Eof: small, never dropped.
    pub fn push_control(&self, msg: ObsMsg) {
        self.lock().q.push_back(msg);
        self.cv.notify_one();
    }

    pub fn pop(&self) -> ObsMsg {
        let mut s = self.lock();
        loop {
            if let Some(msg) = s.q.pop_front() {
                if let ObsMsg::Bytes(b) = &msg {
                    s.queued_bytes -= b.len();
                }
                return msg;
            }
            s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Like `pop`, but gives up after `timeout` so the observer can check time-based transitions.
    pub fn pop_timeout(&self, timeout: Duration) -> Option<ObsMsg> {
        let deadline = Instant::now() + timeout;
        let mut s = self.lock();
        loop {
            if let Some(msg) = s.q.pop_front() {
                if let ObsMsg::Bytes(b) = &msg {
                    s.queued_bytes -= b.len();
                }
                return Some(msg);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            s = self.cv.wait_timeout(s, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// (overflow_events, dropped_bytes)
    pub fn overflow_stats(&self) -> (u64, u64) {
        let s = self.lock();
        (s.overflow_events, s.dropped_bytes)
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for ObsQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------- status types ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputState {
    /// Waiting for the integration's first prompt.
    Booting,
    /// At an idle prompt. The only state where submit is accepted.
    Prompt,
    /// A line was written; waiting for its C (or the timeout).
    Submitted,
    /// A command is running (C seen, D not yet, or D seen and the next prompt not yet drawn).
    Running,
    /// zsh is waiting for more of a multi-line command (PS2).
    Continuation,
    /// A nested shell did not answer a submitted line within SUBMIT_TIMEOUT. Only its C or A ends it. Never reported
    /// for the local shell.
    Unresponsive,
    /// The alternate screen is active (vim, less, ...).
    Alt,
    /// No integration within INTEGRATION_TIMEOUT. The agent can read the screen only.
    Raw,
    /// The PTY reached EOF.
    Exited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    Booting,
    Live,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverStatus {
    Ok,
    /// Bytes were dropped; events are ignored until the next prompt.
    Desynced,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScreenSnapshot {
    pub rows: Vec<String>,
    /// (row, col), zero-based.
    pub cursor: (u16, u16),
    pub alternate_screen: bool,
    /// (rows, cols)
    pub size: (u16, u16),
}

/// What a `wait` needs to know, read under one lock.
#[derive(Debug, Clone, Copy)]
pub struct Activity {
    pub state: InputState,
    /// Bumped whenever the visible screen changes (see `Core::note_screen`).
    pub screen_gen: u64,
    /// Id of the newest finished record, 0 when no command has finished yet.
    pub last_record_id: u64,
}

/// What `run_command`'s wait reads to decide an early return, all under one lock (see `Core::yield_signals`).
#[derive(Debug, Clone)]
pub struct YieldSignals {
    /// The top level's open record is the one the call started (false while a nested shell is on top of it, or when the
    /// command has not started or has finished).
    pub on_top: bool,
    /// The open command is a login: a remote shell, a nested shell, su, or a container shell (`foreground::is_login`).
    pub login: bool,
    /// The parser's bracketed-paste mode (vt100's `Screen::bracketed_paste`). Shells with line editing turn it on at their
    /// prompt and off while a command runs.
    pub bracketed_paste: bool,
    /// Output has arrived since the command started (its C event).
    pub output_since_start: bool,
    /// How long the visible screen has been unchanged. `Duration::MAX` when it has never changed.
    pub quiet_for: Duration,
    /// The last non-blank visible row, trailing spaces trimmed. None on a blank screen, or when the call is not on top.
    pub last_row: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelKind {
    /// The shell the app started.
    Local,
    /// A shell that the app integrated inside another program (ssh, a nested shell).
    Remote,
}

/// One shell level, for the UI and the status. `state` is the top level's state for the top one; the levels below it
/// are parked, so they are Running.
#[derive(Debug, Clone, Serialize)]
pub struct LevelInfo {
    pub key: u64,
    pub kind: LevelKind,
    pub label: String,
    pub sh: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    pub state: InputState,
}

/// Why `Core::begin_injection` (and so `Session::integrate_shell`) did not start. The texts are for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectRefusal {
    /// No command is running at the top level (an idle prompt, a shell that is booting, or an exited session).
    NotRunning,
    /// A full-screen program is open. Close it first.
    AltScreen,
    /// The running program is not a shell, ssh, su or a container exec, so there is no shell to integrate.
    NoShellProgram,
    /// The depth cap is reached: another level would pass MAX_DEPTH.
    TooDeep,
    /// Another integration is in flight, or a level is already waiting for its Hello.
    Busy,
}

impl std::fmt::Display for InjectRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InjectRefusal::NotRunning => write!(f, "no command is running in this shell, so there is nothing to integrate"),
            InjectRefusal::AltScreen => write!(f, "a full-screen program is open; close it first"),
            InjectRefusal::NoShellProgram => {
                write!(f, "the running program is not a shell, ssh, su or a container exec, so there is no shell to integrate")
            }
            InjectRefusal::TooDeep => write!(f, "the shell is nested too deeply to integrate another level"),
            InjectRefusal::Busy => write!(f, "another shell integration is already in progress"),
        }
    }
}

/// Why `send_keys` may not type now (see `Core::keys_target`). The texts are for the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysRefusal {
    /// The session is closed: nothing can be written.
    Closed,
    /// The observer lost output and has not seen a prompt since.
    Desynced,
    /// The shell on top is not the one the call was approved for (another shell started, or the shell exited).
    Location,
    /// The shell is at its prompt or has just been submitted: no command is running.
    NothingRunning,
    /// The top level is in a state that takes no keys (booting, a continuation, unresponsive, no integration, exited).
    NotRunning(InputState),
    /// No command is open at the top level.
    NoCommand,
    /// The command open at the top level was not started by the agent: it is the user's.
    NotAgents,
    /// The command open at the top level is not the one the call names (`wanted`), or the call is bound to another one.
    WrongCommand { wanted: Option<u64>, open: u64 },
    /// The command has finished: its D has been seen.
    Finished,
}

impl std::fmt::Display for KeysRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeysRefusal::Closed => write!(f, "the session is closed"),
            KeysRefusal::Desynced => write!(f, "the shell output was lost; nothing was sent: use get_screen, then decide again"),
            KeysRefusal::Location => write!(
                f,
                "the shell you were working in has changed (it exited or another shell started); nothing was sent: check the \
                 screen and decide again"
            ),
            KeysRefusal::NothingRunning => write!(
                f,
                "nothing is running; use run_command to start a command at an idle prompt"
            ),
            KeysRefusal::NotRunning(state) => write!(
                f,
                "the session is {state:?}, so keys cannot be sent to a command now: use get_screen and wait, or ask the user"
            ),
            KeysRefusal::NoCommand => write!(
                f,
                "no command is open at the top of the shell: use get_screen to see what is there, and run_command only at an \
                 idle prompt"
            ),
            KeysRefusal::NotAgents => write!(
                f,
                "the command in the shell was not started by you, so keys cannot be sent to it: use get_screen and ask the user \
                 to act"
            ),
            KeysRefusal::WrongCommand { wanted: Some(id), open } => write!(
                f,
                "command id {id} is not the one running; the running command is id {open}. Use that id, or leave out id."
            ),
            KeysRefusal::WrongCommand { wanted: None, open } => write!(
                f,
                "the command you sent keys to has changed; the running command is id {open}. Use get_screen and decide again."
            ),
            KeysRefusal::Finished => write!(
                f,
                "that command has finished, so keys cannot be sent to it: use get_command for its output, or run_command for a \
                 new command"
            ),
        }
    }
}

/// Session changes for listeners (the UI). Delivered in order, outside the core lock.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    State {
        state: InputState,
        /// The foreground program hint, while a command runs (see core/foreground.rs). Left out otherwise.
        #[serde(skip_serializing_if = "Option::is_none")]
        foreground: Option<Foreground>,
    },
    Integration { status: IntegrationStatus },
    Observer { status: ObserverStatus },
    Cwd { cwd: Option<Cwd> },
    RecordStarted { id: u64, command: String, source: Source },
    RecordFinished { record: RecordSummary },
    /// The shell levels changed (a push, a pop, a pending level). `nonces` are every nonce now accepted, the local one
    /// included, so the frontend can check OSC events against them. The same exposure as the session nonce.
    Levels { levels: Vec<LevelInfo>, nonces: Vec<String> },
}

pub trait EventSink: Send + Sync {
    fn event(&self, ev: SessionEvent);
}

/// The last values reported, so state and status events fire only on change.
#[derive(Default)]
struct Reported {
    state: Option<InputState>,
    foreground: Option<Foreground>,
    integration: Option<IntegrationStatus>,
    observer: Option<ObserverStatus>,
    cwd: Option<Cwd>,
}

// ---------- vt100 callbacks ----------

/// How many scrolled-off normal-screen rows are kept for `get_screen` with `lines`. The oldest go first.
pub const HISTORY_ROWS: usize = 1000;
/// The most text `get_screen` with `lines` returns, in bytes. The newest lines win.
pub const SCREEN_HISTORY_CAP: usize = 12 << 10;

/// One scrolled-off row, kept as vt100 writes it. It is rendered to plain text only when it is read (`plain`).
struct HistoryRow {
    /// The row's formatted capture (SGR codes included). May start with the newline vt100 writes before a row that follows
    /// a hard line end; `start` skips it.
    raw: String,
    start: usize,
    /// True when the row soft-wrapped into the next one. Set when the next row arrives. The newest row has no next row yet:
    /// `Collector::newest_wrapped` reads it instead.
    wrapped: bool,
}

impl HistoryRow {
    fn plain(&self) -> String {
        basic_formatted_to_plain(&self.raw[self.start..]).collect()
    }
}

/// Owned by the vt100 parser. Counts rows that scroll off the normal screen, keeps the newest of them (the history ring, as
/// raw captures), and renders the open record's output as it goes (`row_text`, separately from the ring).
pub struct Collector {
    /// Normal-screen rows scrolled off since the parser was created. Row N of the scrollback has absolute index N.
    scrolled_total: u64,
    /// The newest scrolled-off rows, oldest first, at most `ring_capacity`. Ring row i has absolute index
    /// `scrolled_total - ring.len() + i`.
    ring: VecDeque<HistoryRow>,
    /// `HISTORY_ROWS`. Zero turns the ring off (the benchmark measures that).
    ring_capacity: usize,
    /// True while an agent's command is open at any level (set by `Core` before each parser write). Rows scrolled off are
    /// kept in the ring only then, so a user's own commands cost nothing. When it turns off the ring stays as it is.
    recording: bool,
    /// One capture state for every ring row, in order. vt100 writes a newline before a row when the row before it ended a
    /// line, so each row says whether the one before it wrapped. Its pending newline says whether the newest row wrapped.
    capture: BasicFormattedCaptureState,
    open: Option<OpenRecord>,
}

impl Default for Collector {
    fn default() -> Self {
        Collector {
            scrolled_total: 0,
            ring: VecDeque::new(),
            ring_capacity: HISTORY_ROWS,
            recording: false,
            capture: BasicFormattedCaptureState::new(),
            open: None,
        }
    }
}

impl Collector {
    /// Turns the ring's recording on or off. Turning it on starts a fresh ring: the rows kept by an earlier agent command
    /// are dropped, so one command's history never shows another's. Turning it off keeps what was captured.
    fn set_recording(&mut self, on: bool) {
        if on && !self.recording {
            self.ring.clear();
            self.capture = BasicFormattedCaptureState::new();
        }
        self.recording = on;
    }

    /// Whether the newest ring row soft-wrapped into the next row. Read from the capture state (its Debug output is the only
    /// public view of it, as in `row_text`). False when the ring is empty.
    fn newest_wrapped(&self) -> bool {
        !format!("{:?}", self.capture).contains("newline_pending: true")
    }
}

impl Callbacks for Collector {
    fn on_scroll(&mut self, contents: RowContents<'_>, alternate_screen: bool) {
        if alternate_screen {
            return;
        }
        let index = self.scrolled_total;
        self.scrolled_total += 1;
        let in_open_record = self.open.as_ref().is_some_and(|open| index >= open.start_abs && !open.builder.is_full());
        if in_open_record && let Some(open) = self.open.as_mut() {
            let (text, wrapped) = row_text(&contents);
            open.builder.push_row(&text, wrapped);
        }
        if self.recording && self.ring_capacity > 0 {
            // The oldest row's buffer is reused, so a full ring costs no allocation per row.
            let mut raw = if self.ring.len() == self.ring_capacity {
                self.ring.pop_front().map(|row| row.raw).unwrap_or_default()
            } else {
                String::new()
            };
            raw.clear();
            let _ = contents.write_formatted_basic(&mut raw, &mut self.capture);
            let start = usize::from(raw.starts_with('\n'));
            if let Some(previous) = self.ring.back_mut() {
                previous.wrapped = start == 0;
            }
            self.ring.push_back(HistoryRow { raw, start, wrapped: false });
        }
    }
}

/// Plain text of one scrolled row, and whether it soft-wrapped into the next row.
fn row_text(contents: &RowContents<'_>) -> (String, bool) {
    let mut raw = String::new();
    let mut state = BasicFormattedCaptureState::new();
    let _ = contents.write_formatted_basic(&mut raw, &mut state);
    let text: String = basic_formatted_to_plain(&raw).collect();
    // vt100 0.18.1 keeps the wrap flag in a private `newline_pending` field (set to !wrapped). Its
    // derived Debug output is the only public view of it. The unit test `wrap_flag_is_readable`
    // fails if a vt100 upgrade changes this.
    let wrapped = !format!("{state:?}").contains("newline_pending: true");
    (text, wrapped)
}

struct OpenRecord {
    record: Record,
    builder: OutputBuilder,
    raw: RawCapture,
    /// Absolute index of the row where the command started (scrolled rows + cursor row at C).
    start_abs: u64,
    used_alt: bool,
    approximate: bool,
    /// When C started the command. The age in the notes is read from it.
    started: Instant,
}

/// One of the agent's own commands that is still open: its record id, the call that started it, its command text, how
/// long it has run, and whether it is on the top level (false: it is parked under a nested shell that is on top).
#[derive(Debug, Clone)]
pub struct OpenAgentRecord {
    pub id: u64,
    pub call_id: String,
    pub command: String,
    pub age: Duration,
    pub on_top: bool,
}

/// The rows `get_screen` returns with `lines` (see `Core::screen_lines`).
#[derive(Debug, Clone)]
pub struct ScreenLines {
    /// Logical lines, oldest first: soft-wrapped rows are joined.
    pub rows: Vec<String>,
    /// True when older lines were left out to stay under `SCREEN_HISTORY_CAP`.
    pub truncated: bool,
    /// True on the alternate screen, which has no history: `rows` are the visible rows.
    pub no_history: bool,
}

// ---------- observer core ----------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lifecycle {
    Booting,
    Prompt,
    Submitted { since: Instant },
    Running,
    /// zsh is waiting for more of a command. `since` is when this stretch began (PS2 redraws keep it).
    Continuation { since: Instant },
}

/// How many abandoned agent calls are remembered. Older ones are forgotten first.
pub const ABANDONED_CAP: usize = 64;

/// What the agent's `run_command` can learn about its own call, read under one lock.
#[derive(Debug, Clone)]
pub enum CallStatus {
    /// The command finished. Carries its record.
    Finished(Box<Record>),
    /// The line never ran: the shell went back to the prompt (Ctrl-C) or the user finished the line themselves.
    Abandoned,
    /// The shell has been waiting for the rest of the line (PS2) for at least the requested time.
    WaitingForInput,
    /// Nothing is known yet. Carries the shell state now, and the id of the call's record when the shell has started it
    /// (the record is open). Both are read under the same lock.
    Pending { state: InputState, record_id: Option<u64> },
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN)
}

fn new_parser(size: (u16, u16)) -> Parser<Collector> {
    Parser::new_with_callbacks(nz(size.0), nz(size.1), 0, Collector::default())
}

/// A hash of what is visible: the size, the rows, the cursor and whether the alternate screen is up.
fn screen_fingerprint(screen: &vt100::Screen) -> u64 {
    let mut hasher = DefaultHasher::new();
    let (rows, cols) = screen.size();
    (rows.get(), cols.get()).hash(&mut hasher);
    for row in screen.rows(0, cols.get()) {
        row.hash(&mut hasher);
    }
    screen.cursor_position().hash(&mut hasher);
    screen.alternate_screen().hash(&mut hasher);
    hasher.finish()
}

/// The text and source of an E event at a nested level. bash does not record every command (ignoreboth, ignoredups, or
/// history off), so its E can be empty, and an E can differ from the line the agent sent. A pending agent line is then the
/// truth: its text and source win. Otherwise the E stands, as it does at level 0.
fn nested_command_line(text: String, pending: Option<(Source, Option<String>)>) -> (String, Source) {
    match pending {
        Some((source @ Source::Agent { .. }, Some(agent_text))) if agent_text != text => (agent_text, source),
        Some((source, Some(pending_text))) if pending_text == text => (text, source),
        Some((source, None)) => (text, source),
        _ => (text, Source::Unknown),
    }
}

/// What a level keeps while another level is on top. The top level's own state lives in the Core's fields (lifecycle,
/// pending, last_e, and the parser's capture target), so the existing code reads the top unchanged. Put back on pop.
/// A level is only parked while it is Running (a push needs that), so its lifecycle is not stored: on pop it is Running.
struct Parked {
    pending: Option<(Source, Option<String>)>,
    last_e: Option<(String, Source)>,
    open: Option<OpenRecord>,
}

impl Parked {
    /// The state of a level that has nothing open and nothing pending.
    fn idle() -> Self {
        Parked { pending: None, last_e: None, open: None }
    }
}

/// One shell level in the stack. The stack is never empty: index 0 is the local shell, and the last entry is the top.
struct Level {
    key: u64,
    kind: LevelKind,
    label: String,
    sh: Option<String>,
    host: Option<String>,
    user: Option<String>,
    /// The cwd path from 633;Cwd. Nested levels only: level 0 reports its cwd through OSC 7.
    cwd: Option<String>,
    /// Set while the level is not on top.
    parked: Option<Parked>,
}

impl Level {
    fn local() -> Self {
        Level {
            key: LOCAL_KEY,
            kind: LevelKind::Local,
            label: "local".into(),
            sh: None,
            host: None,
            user: None,
            cwd: None,
            parked: None,
        }
    }
}

/// A level that `expect_level` registered, waiting for its Hello.
struct PendingLevel {
    key: u64,
    label: String,
    since: Instant,
}

/// An injection in flight: the loader was typed with `token`, and `sh` is set once its Ready carried that token.
struct Injection {
    token: String,
    sh: Option<String>,
}

pub struct Core {
    parser: Parser<Collector>,
    /// Hash of the visible screen at the last `note_screen`. Compared on each call.
    screen_hash: u64,
    /// Counts visible-screen changes. Waits compare it between polls.
    screen_gen: u64,
    /// When the screen last changed, and when any output last arrived. `integration_starting` reads them.
    screen_changed_at: Option<Instant>,
    output_at: Option<Instant>,
    /// Set when an agent's record closes, and cleared at the next command start. While it is set the history ring keeps
    /// recording: the rows still on screen when the command ends scroll off during the prompt that follows, and they are
    /// part of the command's output.
    settling_history: bool,
    size: (u16, u16),
    scanner: Scanner,
    /// The shell levels, outermost (local) first. Never empty; the last one is the top.
    stack: Vec<Level>,
    /// The next level key to hand out. Keys are never reused.
    next_key: u64,
    /// The level expected by `expect_level`, if any.
    pending_level: Option<PendingLevel>,
    /// Pushes refused (not Running, depth cap) and nested events refused for want of a pending level.
    refused: u64,
    /// The injection in flight, if any (see `begin_injection`).
    injection: Option<Injection>,
    /// Ready events whose token was not the one in flight (or none was in flight). Not forged: they are only counted.
    stray_ready: u64,
    /// The top level's lifecycle.
    lifecycle: Lifecycle,
    spawned_at: Instant,
    /// The local shell's pid, from its Hello.
    shell_pid: Option<u32>,
    desynced: bool,
    eof: bool,
    /// Set by a submit or a user CR. Matched against the next E event to attribute the command. A None text matches
    /// any command: that is a line the user typed in the terminal, whose text Rust never sees.
    pending: Option<(Source, Option<String>)>,
    /// E seen, waiting for its C. The top level's.
    last_e: Option<(String, Source)>,
    /// The user has typed text that may be on the prompt line. Set by `note_user_bytes`, which counts typing at the
    /// prompt and type-ahead typed while a command runs (zsh reads that from the tty at the next prompt). Cleared by
    /// Ctrl-C at the prompt (not while a command runs), by a CR that submits the line (`begin_submit`), and by C. It
    /// survives the transition from a command to the prompt. While it is set the agent's run_command is refused, so it
    /// cannot land on the user's text.
    user_typed: bool,
    /// Agent calls whose line never ran (see `CallStatus::Abandoned`). Bounded by `ABANDONED_CAP`.
    abandoned: VecDeque<String>,
    cwd: Option<Cwd>,
    records: VecDeque<Record>,
    next_id: u64,
    /// Events raised since the last `take_events`.
    events: Vec<SessionEvent>,
    reported: Reported,
}

/// Keeps at most this many completed records in memory. The oldest are dropped first.
pub const MAX_RECORDS: usize = 1000;
/// How long the screen must be still before a shell counts as started for an integration (see `integration_starting`).
pub const STARTING_QUIET: Duration = Duration::from_millis(300);

impl Core {
    pub fn new(nonce: &str, size: (u16, u16)) -> Self {
        let parser = new_parser(size);
        Core {
            screen_hash: screen_fingerprint(parser.screen()),
            screen_gen: 0,
            screen_changed_at: None,
            output_at: None,
            settling_history: false,
            parser,
            size,
            scanner: Scanner::new(nonce),
            stack: vec![Level::local()],
            next_key: LOCAL_KEY + 1,
            pending_level: None,
            refused: 0,
            injection: None,
            stray_ready: 0,
            lifecycle: Lifecycle::Booting,
            spawned_at: Instant::now(),
            shell_pid: None,
            desynced: false,
            eof: false,
            pending: None,
            last_e: None,
            user_typed: false,
            abandoned: VecDeque::new(),
            cwd: None,
            records: VecDeque::new(),
            next_id: 1,
            events: Vec::new(),
            reported: Reported::default(),
        }
    }

    /// Returns the events raised since the last call, including state and status changes. With
    /// `deliver` false (no listener yet) the queued record events are dropped and nothing is reported,
    /// so a listener attached later still gets the current state.
    pub fn take_events(&mut self, deliver: bool) -> Vec<SessionEvent> {
        self.prune_pending();
        if !deliver {
            self.events.clear();
            return Vec::new();
        }
        let mut out = std::mem::take(&mut self.events);
        // A change of the foreground hint is reported as a state event too: it is the same moment in the same event.
        let state = self.lifecycle_view();
        let foreground = self.foreground();
        if self.reported.state != Some(state) || self.reported.foreground != foreground {
            self.reported.state = Some(state);
            self.reported.foreground = foreground.clone();
            out.push(SessionEvent::State { state, foreground });
        }
        let integration = self.integration_status();
        if self.reported.integration != Some(integration) {
            self.reported.integration = Some(integration);
            out.push(SessionEvent::Integration { status: integration });
        }
        let observer = self.observer_status();
        if self.reported.observer != Some(observer) {
            self.reported.observer = Some(observer);
            out.push(SessionEvent::Observer { status: observer });
        }
        let cwd = self.cwd();
        if self.reported.cwd != cwd {
            self.reported.cwd = cwd.clone();
            out.push(SessionEvent::Cwd { cwd });
        }
        out
    }

    pub fn apply(&mut self, msg: ObsMsg) {
        match msg {
            ObsMsg::Bytes(bytes) => self.feed(&bytes),
            ObsMsg::Resize { rows, cols } => self.resize(rows, cols),
            ObsMsg::Resync => self.resync(),
            ObsMsg::Eof => {
                // The shell is gone, so the nested levels and a command still open will never get their D. They end
                // here, incomplete. Popping first hands the local record back, so it is closed too.
                self.truncate_to(LOCAL_KEY);
                self.close_open(RecordStatus::Incomplete);
                self.eof = true;
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let found: Vec<Found> = self.scanner.feed(bytes);
        let mut pos = 0;
        for f in found {
            // Feed the parser up to the event, so the screen matches the event's position in the stream.
            self.process_segment(&bytes[pos..f.end]);
            pos = f.end;
            self.route(f.level, f.event);
        }
        self.process_segment(&bytes[pos..]);
    }

    /// Sends one authenticated event to the level that sent it. An event from a level below the top first pops the
    /// levels above it. Hello and 633;Cwd need the level, so they are handled here. The rest go to `handle`, which works
    /// on the top level.
    fn route(&mut self, level: Option<u64>, event: Event) {
        // While desynced, only a prompt is trusted: it is the next point where the observer knows the state.
        if self.desynced && event != Event::PromptStart {
            return;
        }
        self.prune_pending();
        // Ready is untagged, so its token is what ties it to the injection in flight. It counts at any depth, because the
        // loader can be typed into a nested shell as well as the local one.
        if let Event::Ready { token, sh } = &event {
            match self.injection.as_mut() {
                Some(inj) if inj.token == *token => inj.sh = Some(sh.clone()),
                _ => self.stray_ready += 1,
            }
            return;
        }
        let Some(key) = level else {
            // Untagged OSC 7 is the local shell's cwd only. A nested shell does not change it.
            if self.top_is_local() {
                self.handle(event);
            }
            return;
        };
        if self.pending_level.as_ref().is_some_and(|p| p.key == key) {
            // Only the Hello of the expected level means anything. Its nonce was accepted by the scanner, so it is authentic.
            if let Event::Hello { sh, user, host, .. } = event {
                self.push_level(sh, user, host);
            }
            return;
        }
        if !self.stack.iter().any(|l| l.key == key) {
            return;
        }
        if key != self.top_key() {
            self.truncate_to(key);
        }
        match event {
            Event::Hello { pid, sh, user, host, .. } => {
                if key == LOCAL_KEY {
                    self.shell_pid = Some(pid);
                }
                if let Some(level) = self.stack.last_mut() {
                    level.sh = sh;
                    level.user = user;
                    level.host = host;
                }
                self.levels_changed();
            }
            Event::LevelCwd { path } => {
                // The local shell's cwd comes from OSC 7 only.
                if let Some(level) = self.stack.last_mut().filter(|l| l.key != LOCAL_KEY) {
                    level.cwd = Some(path);
                }
            }
            other => self.handle(other),
        }
    }

    /// Pushes the pending level when its Hello arrives. Refused (and counted) unless the top is Running and the depth cap
    /// allows it. A refused Hello leaves the pending level in place until it expires.
    fn push_level(&mut self, sh: Option<String>, user: Option<String>, host: Option<String>) {
        if self.lifecycle != Lifecycle::Running || self.stack.len() >= MAX_DEPTH {
            self.refused += 1;
            return;
        }
        let Some(pending) = self.pending_level.take() else {
            return;
        };
        // The parent's record keeps its slot and its state. From here on the parser captures into the child's record
        // only, so no output is attributed to the parent while the child is on top.
        let parked = Parked {
            pending: self.pending.take(),
            last_e: self.last_e.take(),
            open: self.parser.callbacks_mut().open.take(),
        };
        if let Some(parent) = self.stack.last_mut() {
            parent.parked = Some(parked);
        }
        self.lifecycle = Lifecycle::Booting;
        self.user_typed = false;
        self.stack.push(Level {
            key: pending.key,
            kind: LevelKind::Remote,
            label: pending.label,
            sh,
            host,
            user,
            cwd: None,
            parked: None,
        });
        self.levels_changed();
    }

    /// Pops every level above `key`. Each popped level's open record closes Incomplete, and its nonce is retired, so its
    /// late events count as stale. Level `key` resumes Running (never Prompt) with its record open again. That record
    /// is marked approximate, since the output of the time the child was on top was not captured.
    fn truncate_to(&mut self, key: u64) {
        let mut changed = false;
        while self.top_key() != key && self.stack.len() > 1 {
            self.close_open(RecordStatus::Incomplete);
            let Some(popped) = self.stack.pop() else {
                break;
            };
            self.scanner.retire_nonce(popped.key);
            let parked = self.stack.last_mut().and_then(|parent| parent.parked.take()).unwrap_or_else(Parked::idle);
            let mut open = parked.open;
            if let Some(open) = open.as_mut() {
                open.approximate = true;
            }
            self.lifecycle = Lifecycle::Running;
            self.pending = parked.pending;
            self.last_e = parked.last_e;
            self.user_typed = false;
            self.parser.callbacks_mut().open = open;
            changed = true;
        }
        if changed {
            self.levels_changed();
        }
    }

    /// Registers a level the app expects to start. Its shell must send a Hello that carries `nonce`, and that Hello pushes
    /// the level. One at a time: a new call replaces the pending level, and the replaced nonce is retired. Returns the key.
    pub fn expect_level(&mut self, nonce: &str, label: &str) -> u64 {
        self.prune_pending();
        if let Some(old) = self.pending_level.take() {
            self.scanner.retire_nonce(old.key);
        }
        let key = self.next_key;
        self.next_key += 1;
        self.scanner.add_nonce(key, nonce);
        let label: String = label.chars().take(LABEL_CAP).collect();
        self.pending_level = Some(PendingLevel { key, label, since: Instant::now() });
        self.levels_changed();
        key
    }

    /// A pending level that waited too long is dropped and its nonce retired.
    fn prune_pending(&mut self) {
        if self.pending_level.as_ref().is_some_and(|p| p.since.elapsed() > PENDING_TIMEOUT) {
            if let Some(pending) = self.pending_level.take() {
                self.scanner.retire_nonce(pending.key);
            }
            self.levels_changed();
        }
    }

    fn levels_changed(&mut self) {
        let levels = self.levels();
        let nonces = self.scanner.accepted_nonces();
        self.events.push(SessionEvent::Levels { levels, nonces });
    }

    /// Starts an injection with `token`: the loader is about to be typed. Only while the top level is Running a program that
    /// holds a shell (ssh, a shell, su, a container exec), with no level pending, no injection in flight and depth to spare.
    pub fn begin_injection(&mut self, token: &str) -> Result<(), InjectRefusal> {
        if let Some(refusal) = self.injection_refusal() {
            return Err(refusal);
        }
        self.injection = Some(Injection { token: token.to_string(), sh: None });
        Ok(())
    }

    /// True while the program in the foreground is still starting, so the Ctrl-C an integration types first could stop it
    /// (a `zsh -f -i` that has not drawn its prompt is killed by SIGINT). That is so when no output has arrived since its
    /// command started, or when the screen changed less than `STARTING_QUIET` ago. Writes nothing.
    pub fn integration_starting(&self, now: Instant) -> bool {
        let Some(open) = self.parser.callbacks().open.as_ref() else {
            return false;
        };
        let no_output_since_start = self.output_at.is_none_or(|t| t < open.started);
        let screen_moving = self.screen_changed_at.is_some_and(|t| now.saturating_duration_since(t) < STARTING_QUIET);
        no_output_since_start || screen_moving
    }

    /// Why `begin_injection` would refuse now, or None. The same checks, changing nothing: the agent asks this before it
    /// asks the user.
    pub fn injection_refusal(&self) -> Option<InjectRefusal> {
        if self.injection.is_some() || self.pending_level.is_some() {
            return Some(InjectRefusal::Busy);
        }
        if self.depth() >= MAX_DEPTH {
            return Some(InjectRefusal::TooDeep);
        }
        match self.lifecycle_view() {
            InputState::Running => {}
            InputState::Alt => return Some(InjectRefusal::AltScreen),
            _ => return Some(InjectRefusal::NotRunning),
        }
        let holds_a_shell = self.foreground().is_some_and(|fg| {
            matches!(fg.kind, ForegroundKind::Ssh | ForegroundKind::Shell | ForegroundKind::Su | ForegroundKind::ContainerExec)
        });
        if !holds_a_shell {
            return Some(InjectRefusal::NoShellProgram);
        }
        None
    }

    /// The shell name (`zsh` or `bash`) once the loader's Ready arrived for the injection in flight.
    pub fn injection_ready(&self) -> Option<String> {
        self.injection.as_ref().and_then(|inj| inj.sh.clone())
    }

    /// Ends the injection in flight, whatever its outcome.
    pub fn end_injection(&mut self) {
        self.injection = None;
    }

    /// Ready events that did not match the injection in flight, counted and otherwise ignored.
    pub fn stray_ready_events(&self) -> u64 {
        self.stray_ready
    }

    /// Drops the level that `expect_level` registered (its nonce is retired, so a late Hello counts as stale).
    pub fn cancel_pending_level(&mut self) {
        if let Some(pending) = self.pending_level.take() {
            self.scanner.retire_nonce(pending.key);
            self.levels_changed();
        }
    }

    /// The shell name of the top level (`zsh` or `bash`), from its Hello. None until the Hello.
    pub fn top_sh(&self) -> Option<&str> {
        self.stack.last().and_then(|l| l.sh.as_deref())
    }

    /// The key of the level on top, the one that takes events and submits. 0 is the local shell.
    pub fn top_key(&self) -> u64 {
        self.stack.last().map_or(LOCAL_KEY, |l| l.key)
    }

    /// Every nonce the scanner accepts now (the local one included). The UI checks OSC events against them.
    pub fn accepted_nonces(&self) -> Vec<String> {
        self.scanner.accepted_nonces()
    }

    fn top_is_local(&self) -> bool {
        self.stack.len() <= 1
    }

    fn process_segment(&mut self, seg: &[u8]) {
        if seg.is_empty() {
            return;
        }
        self.output_at = Some(Instant::now());
        self.sync_history();
        let open = self.parser.callbacks_mut().open.as_mut();
        if let Some(open) = open {
            open.raw.push(seg);
        }
        self.parser.process(seg);
        let alt = self.parser.screen().alternate_screen();
        if let Some(open) = self.parser.callbacks_mut().open.as_mut() {
            open.used_alt |= alt;
        }
    }

    fn handle(&mut self, event: Event) {
        // While desynced, only a prompt is trusted: it is the next point where the observer knows the state.
        if self.desynced && event != Event::PromptStart {
            return;
        }
        match event {
            // Hello and 633;Cwd are handled in `route`, which knows the level they came from.
            Event::Hello { .. } | Event::LevelCwd { .. } => {}
            // Ready is for the injection state machine (B2b). Nothing consumes it yet.
            Event::Ready { .. } => {}
            Event::PromptStart => {
                self.desynced = false;
                self.close_open(RecordStatus::Incomplete);
                self.last_e = None;
                // At a nested level a submit that timed out is Unresponsive: its own A is what ends that.
                let unresponsive = !self.top_is_local()
                    && matches!(self.lifecycle, Lifecycle::Submitted { since } if since.elapsed() > SUBMIT_TIMEOUT);
                match self.lifecycle {
                    // A redraw of the prompt right after a submit must not open the gate before C.
                    Lifecycle::Submitted { .. } if !unresponsive => {}
                    Lifecycle::Booting if self.top_is_local() && self.shell_pid.is_none() => {}
                    _ => {
                        // An agent line still waiting for its C, and zsh is back at the prompt: the line was cancelled.
                        if let Some((Source::Agent { call_id }, _)) = &self.pending {
                            let call_id = call_id.clone();
                            self.mark_abandoned(call_id);
                        }
                        // The flag is kept here, whether this is a transition or a redraw. Bytes typed while a command
                        // ran are still in the tty, and zsh draws them on this new line (type-ahead).
                        self.lifecycle = Lifecycle::Prompt;
                        self.pending = None;
                    }
                }
            }
            Event::Continuation => {
                // Submitted too: a multi-line line the user typed closes the gate with its first CR, and zsh then
                // asks for more (PS2). Without this the gate would reopen while zsh is still inside the command.
                if matches!(self.lifecycle, Lifecycle::Prompt | Lifecycle::Submitted { .. }) {
                    self.lifecycle = Lifecycle::Continuation { since: Instant::now() };
                }
            }
            Event::CommandLine(text) => {
                let pending = self.pending.take();
                let entry = if self.top_is_local() {
                    let source = match pending {
                        Some((source, None)) => source,
                        Some((source, Some(pending_text))) if pending_text == text => source,
                        _ => Source::Unknown,
                    };
                    (text, source)
                } else {
                    nested_command_line(text, pending)
                };
                self.last_e = Some(entry);
            }
            Event::CommandStart => {
                // C while a record is open: the previous command ended without a D we could trust.
                self.close_open(RecordStatus::Incomplete);
                self.settling_history = false;
                let (command, source) = self.command_for_c();
                self.pending = None;
                // The line the user typed has run (or is running): the prompt is no longer holding their text.
                self.user_typed = false;
                self.start_record(command, source);
                self.lifecycle = Lifecycle::Running;
            }
            Event::CommandEnd { exit, pipestatus } => {
                let open = self.parser.callbacks_mut().open.take();
                if let Some(open) = open {
                    self.finish_record(open, Some(exit), pipestatus, RecordStatus::Complete);
                }
            }
            Event::ScrollRegion => {
                if let Some(open) = self.parser.callbacks_mut().open.as_mut() {
                    open.approximate = true;
                }
            }
            Event::Cwd { host, path } => {
                // Reached only when the local shell is on top (see `route`).
                let cwd = Cwd { host, path };
                if self.cwd.as_ref() != Some(&cwd) {
                    self.cwd = Some(cwd);
                }
            }
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let size = (rows.max(1), cols.max(1));
        if let Some(open) = self.parser.callbacks_mut().open.as_mut() {
            // vt100 may push rows through on_scroll or reflow the cursor during a resize; the output is then approximate.
            open.approximate = true;
        }
        self.sync_history();
        self.parser.set_size(nz(size.0), nz(size.1));
        self.size = size;
    }

    /// Bytes were dropped. Forget the open record (incomplete), start a fresh parser, and wait for a prompt.
    fn resync(&mut self) {
        self.close_open(RecordStatus::Incomplete);
        self.parser = new_parser(self.size);
        self.settling_history = false;
        self.scanner.reset();
        self.desynced = true;
        self.last_e = None;
        self.pending = None;
    }

    /// The command text and source for the record that C starts. Level 0 takes E as it always did. A nested level takes
    /// the E it has (already corrected by `nested_command_line`), or else the pending agent line, or else a placeholder.
    fn command_for_c(&mut self) -> (String, Source) {
        if self.top_is_local() {
            return self.last_e.take().unwrap_or_else(|| (String::new(), Source::Unknown));
        }
        match self.last_e.take() {
            Some((text, source)) if !text.is_empty() => (text, source),
            _ => match self.pending.take() {
                Some((source @ Source::Agent { .. }, Some(text))) => (text, source),
                _ => (UNAVAILABLE_COMMAND.to_string(), Source::User),
            },
        }
    }

    fn start_record(&mut self, command: String, source: Source) {
        let alt = self.parser.screen().alternate_screen();
        let (crow, _) = self.parser.screen().cursor_position();
        let id = self.next_id;
        self.next_id += 1;
        // A record made while a nested level is on top belongs to that level: its cwd is the level's, and its location is
        // the label the level was registered with. Level 0 keeps the local cwd and shell pid.
        let (cwd, shell_pid, level, location) = match self.stack.last() {
            Some(top) if top.key != LOCAL_KEY => (
                top.cwd.clone().map(|path| Cwd { host: top.host.clone().unwrap_or_default(), path }),
                None,
                top.key,
                Some(top.label.clone()),
            ),
            _ => (self.cwd.clone(), self.shell_pid, LOCAL_KEY, None),
        };
        self.events.push(SessionEvent::RecordStarted { id, command: command.clone(), source: source.clone() });
        let cb = self.parser.callbacks_mut();
        let start_abs = cb.scrolled_total + u64::from(crow);
        cb.open = Some(OpenRecord {
            record: Record {
                id,
                command,
                source,
                shell_pid,
                cwd,
                level,
                location,
                exit_code: None,
                pipestatus: Vec::new(),
                status: RecordStatus::Incomplete,
                used_alt_screen: alt,
                output: String::new(),
                output_truncated: false,
                raw_head: Vec::new(),
                raw_tail: Vec::new(),
                raw_truncated: false,
            },
            builder: OutputBuilder::default(),
            raw: RawCapture::default(),
            start_abs,
            used_alt: alt,
            approximate: false,
            started: Instant::now(),
        });
    }

    fn close_open(&mut self, status: RecordStatus) {
        let open = self.parser.callbacks_mut().open.take();
        if let Some(open) = open {
            self.finish_record(open, None, Vec::new(), status);
        }
    }

    /// Adds the rows still on screen (from the command's first row to the cursor) and stores the record.
    fn finish_record(&mut self, open: OpenRecord, exit: Option<i32>, pipestatus: Vec<i32>, status: RecordStatus) {
        let OpenRecord { mut record, mut builder, raw, start_abs, used_alt, approximate, .. } = open;
        if matches!(record.source, Source::Agent { .. }) {
            self.settling_history = true;
        }
        let (screen, cb) = self.parser.screen_and_callbacks_mut();
        if !screen.alternate_screen() && !used_alt {
            let (crow, _) = screen.cursor_position();
            let cols = screen.size().1.get();
            let first = start_abs.saturating_sub(cb.scrolled_total) as usize;
            for (r, text) in screen.rows(0, cols).enumerate().skip(first) {
                if r > crow as usize {
                    break;
                }
                builder.push_row(&text, screen.row_wrapped(r as u16));
            }
        }
        let (output, truncated) = builder.finish();
        let (raw_head, raw_tail, raw_truncated) = raw.into_parts();
        record.output = output;
        record.output_truncated = truncated;
        record.exit_code = exit;
        record.pipestatus = pipestatus;
        record.used_alt_screen = used_alt;
        record.status = match status {
            RecordStatus::Incomplete => RecordStatus::Incomplete,
            _ if approximate => RecordStatus::Approximate,
            _ => status,
        };
        record.raw_head = raw_head;
        record.raw_tail = raw_tail;
        record.raw_truncated = raw_truncated;
        self.events.push(SessionEvent::RecordFinished { record: RecordSummary::from(&record) });
        self.records.push_back(record);
        while self.records.len() > MAX_RECORDS {
            self.records.pop_front();
        }
    }

    // ----- queries -----

    pub fn lifecycle_view(&self) -> InputState {
        if self.eof {
            return InputState::Exited;
        }
        if self.integration_status() == IntegrationStatus::Raw {
            return InputState::Raw;
        }
        if self.parser.screen().alternate_screen() {
            return InputState::Alt;
        }
        match &self.lifecycle {
            Lifecycle::Booting => InputState::Booting,
            Lifecycle::Prompt => InputState::Prompt,
            Lifecycle::Submitted { since } if since.elapsed() > SUBMIT_TIMEOUT => {
                if self.top_is_local() {
                    InputState::Prompt
                } else {
                    InputState::Unresponsive
                }
            }
            Lifecycle::Submitted { .. } => InputState::Submitted,
            Lifecycle::Running => InputState::Running,
            Lifecycle::Continuation { .. } => InputState::Continuation,
        }
    }

    /// Every level, outermost first, with the top level's state. The levels below the top are parked, so Running.
    pub fn levels(&self) -> Vec<LevelInfo> {
        let top = self.stack.len().saturating_sub(1);
        self.stack
            .iter()
            .enumerate()
            .map(|(i, l)| LevelInfo {
                key: l.key,
                kind: l.kind,
                label: l.label.clone(),
                sh: l.sh.clone(),
                host: l.host.clone(),
                user: l.user.clone(),
                state: if i == top { self.lifecycle_view() } else { InputState::Running },
            })
            .collect()
    }

    /// How many levels the session holds, the local one included. 1 means no nested shell.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// The label of the nested level on top, the command that started it (for example `ssh ubuntu@lidia`). None on the
    /// local shell.
    pub fn location(&self) -> Option<String> {
        if self.top_is_local() {
            return None;
        }
        self.stack.last().map(|l| l.label.clone())
    }

    /// Pushes refused (not Running, or the depth cap) since the session started.
    pub fn refused_levels(&self) -> u64 {
        self.refused
    }

    /// Tagged events from popped levels, counted apart from forged ones.
    pub fn stale_events(&self) -> u64 {
        self.scanner.stale_events()
    }

    /// Remembers that an agent call's line never ran. Bounded: the oldest entry goes first.
    fn mark_abandoned(&mut self, call_id: String) {
        if self.abandoned.contains(&call_id) {
            return;
        }
        self.abandoned.push_back(call_id);
        while self.abandoned.len() > ABANDONED_CAP {
            self.abandoned.pop_front();
        }
    }

    /// The status of one agent call, all read in this one call (so, under one lock). `input_wait` is how long zsh
    /// must have been waiting for more input before this reports `WaitingForInput`.
    pub fn call_status(&self, call_id: &str, input_wait: Duration) -> CallStatus {
        let finished = self.records.iter().find(|r| matches!(&r.source, Source::Agent { call_id: c } if c == call_id));
        if let Some(record) = finished {
            return CallStatus::Finished(Box::new(record.clone()));
        }
        if self.abandoned.iter().any(|c| c == call_id) {
            return CallStatus::Abandoned;
        }
        if let (Lifecycle::Continuation { since }, Some((Source::Agent { call_id: c }, _))) =
            (&self.lifecycle, &self.pending)
            && c == call_id
            && since.elapsed() >= input_wait
        {
            return CallStatus::WaitingForInput;
        }
        let record_id = self.open_records().find(|(open, _)| matches!(&open.record.source, Source::Agent { call_id: c } if c == call_id)).map(|(open, _)| open.record.id);
        CallStatus::Pending { state: self.lifecycle_view(), record_id }
    }

    /// Every open record, the top level's first and then the parked levels', outermost first. The flag is true for the top
    /// level's record. A parked record is the command that started a nested shell (for example ssh), still running.
    fn open_records(&self) -> impl Iterator<Item = (&OpenRecord, bool)> + '_ {
        let top = self.parser.callbacks().open.as_ref().map(|open| (open, true));
        let parked = self.stack.iter().filter_map(|level| level.parked.as_ref()?.open.as_ref()).map(|open| (open, false));
        top.into_iter().chain(parked)
    }

    /// The agent's own open commands (see `OpenAgentRecord`).
    pub fn open_agent_records(&self) -> Vec<OpenAgentRecord> {
        let now = Instant::now();
        self.open_records()
            .filter_map(|(open, on_top)| match &open.record.source {
                Source::Agent { call_id } => Some(OpenAgentRecord {
                    id: open.record.id,
                    call_id: call_id.clone(),
                    command: open.record.command.clone(),
                    age: now.saturating_duration_since(open.started),
                    on_top,
                }),
                _ => None,
            })
            .collect()
    }

    /// Tells the history ring whether it records: an agent's command is open at any level, or one has just closed and the
    /// prompt after it is still being drawn (`settling_history`). A command the user runs costs nothing. Called before each
    /// write to the parser, so the events that open or close a record take effect for the bytes after them.
    fn sync_history(&mut self) {
        let on = self.settling_history
            || self.open_records().any(|(open, _)| matches!(open.record.source, Source::Agent { .. }));
        self.parser.callbacks_mut().set_recording(on);
    }

    /// True while a record with this id is open (any source, any level).
    pub fn record_is_open(&self, id: u64) -> bool {
        self.open_records().any(|(open, _)| open.record.id == id)
    }

    /// True once the record with this id has finished (and it is still kept).
    pub fn has_finished_record(&self, id: u64) -> bool {
        self.records.iter().any(|r| r.id == id)
    }

    /// The summary of a finished record, by id.
    pub fn record_summary(&self, id: u64) -> Option<RecordSummary> {
        self.records.iter().find(|r| r.id == id).map(RecordSummary::from)
    }

    /// `get_screen` with `lines`: the last `n` logical lines, from the history ring and the visible rows. A soft-wrapped
    /// row continues the line before it. Trailing blank rows below the cursor are not output and are left out. On the
    /// alternate screen there is no history, so the visible rows are returned and `no_history` is set. The newest lines
    /// are kept when the text is over `SCREEN_HISTORY_CAP`.
    pub fn screen_lines(&self, n: usize) -> ScreenLines {
        let screen = self.parser.screen();
        if screen.alternate_screen() {
            return ScreenLines { rows: self.screen_snapshot().rows, truncated: false, no_history: true };
        }
        let (rows_visible, cols) = screen.size();
        let cursor_row = usize::from(screen.cursor_position().0);
        let collector = self.parser.callbacks();
        let newest = collector.ring.len().saturating_sub(1);
        let newest_wrapped = collector.newest_wrapped();
        let mut physical: Vec<(String, bool)> = collector
            .ring
            .iter()
            .enumerate()
            .map(|(i, row)| (row.plain(), if i == newest { newest_wrapped } else { row.wrapped }))
            .collect();
        let visible_start = physical.len();
        for (r, text) in screen.rows(0, cols.get()).enumerate().take(usize::from(rows_visible.get())) {
            physical.push((text, screen.row_wrapped(r as u16)));
        }
        while physical.len() > visible_start + cursor_row + 1 {
            match physical.last() {
                Some((text, false)) if text.trim().is_empty() => {
                    physical.pop();
                }
                _ => break,
            }
        }
        let mut logical: Vec<String> = Vec::new();
        let mut continues = false;
        for (text, wrapped) in physical {
            let piece = if wrapped { text } else { text.trim_end().to_string() };
            match logical.last_mut() {
                Some(last) if continues => last.push_str(&piece),
                _ => logical.push(piece),
            }
            continues = wrapped;
        }
        let kept = &logical[logical.len().saturating_sub(n)..];
        let mut used = 0;
        let mut first = kept.len();
        for (i, line) in kept.iter().enumerate().rev() {
            if used + line.len() + 1 > SCREEN_HISTORY_CAP {
                break;
            }
            used += line.len() + 1;
            first = i;
        }
        let mut rows: Vec<String> = kept[first..].to_vec();
        let mut truncated = first > 0;
        if rows.is_empty() && let Some(last) = kept.last() {
            // The newest line alone is over the cap: its end is kept, cut at a character boundary.
            let mut cut = last.len() - SCREEN_HISTORY_CAP.min(last.len());
            while !last.is_char_boundary(cut) {
                cut += 1;
            }
            rows.push(last[cut..].to_string());
            truncated = true;
        }
        ScreenLines { rows, truncated, no_history: false }
    }

    // TODO: `exec zsh` and nested shells are undetected. They have no WATT_NONCE, so they emit no events:
    // this stays Live and the lifecycle stays Running until the session exits. Needs an "integration lost" status.
    /// The foreground program hint: only while the shell is Running (not Alt, not Raw) with a command open, and the
    /// command text is not blank. Derived from the open record each time, so it cannot go stale.
    pub fn foreground(&self) -> Option<Foreground> {
        if self.lifecycle_view() != InputState::Running {
            return None;
        }
        let open = self.parser.callbacks().open.as_ref()?;
        let fg = foreground::classify(&open.record.command);
        (!fg.command.is_empty()).then_some(fg)
    }

    /// True while the foreground hint names a program that takes the typed keys itself: anything but `other`. An ordinary
    /// command (`sleep`, `make`) reads the tty too, but its type-ahead still lands on the prompt line, so it does not count.
    fn foreground_holds_the_keys(&self) -> bool {
        self.foreground().is_some_and(|fg| fg.kind != ForegroundKind::Other)
    }

    pub fn integration_status(&self) -> IntegrationStatus {
        // A nested level exists only after the local shell was Running, so it has been live since then.
        if self.stack.len() > 1 || self.lifecycle != Lifecycle::Booting {
            IntegrationStatus::Live
        } else if self.spawned_at.elapsed() > INTEGRATION_TIMEOUT {
            IntegrationStatus::Raw
        } else {
            IntegrationStatus::Booting
        }
    }

    pub fn observer_status(&self) -> ObserverStatus {
        if self.desynced {
            ObserverStatus::Desynced
        } else {
            ObserverStatus::Ok
        }
    }

    pub fn desynced(&self) -> bool {
        self.desynced
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    /// What the early return of `run_command` reads for one agent call, under the caller's lock (see `YieldSignals`). `now` is
    /// the time of the poll, so the quiet time is measured to it. Writes nothing.
    pub fn yield_signals(&self, call_id: &str, now: Instant) -> YieldSignals {
        let screen = self.parser.screen();
        let own = self
            .parser
            .callbacks()
            .open
            .as_ref()
            .filter(|open| matches!(&open.record.source, Source::Agent { call_id: c } if c == call_id));
        let last_row = own.and_then(|_| {
            let (_, cols) = screen.size();
            let rows: Vec<String> = screen.rows(0, cols.get()).collect();
            rows.iter().rev().find(|r| !r.trim().is_empty()).map(|r| r.trim_end().to_string())
        });
        YieldSignals {
            on_top: own.is_some(),
            login: own.is_some_and(|open| foreground::is_login(&open.record.command)),
            bracketed_paste: screen.bracketed_paste(),
            output_since_start: own.is_some_and(|open| self.output_at.is_some_and(|t| t >= open.started)),
            quiet_for: self.screen_changed_at.map_or(Duration::MAX, |t| now.saturating_duration_since(t)),
            last_row,
        }
    }

    /// True while the command in the foreground is a login (see `foreground::is_login`), read from the full command text of
    /// the open record. False unless the shell is Running with a command open.
    pub fn foreground_is_login(&self) -> bool {
        self.lifecycle_view() == InputState::Running
            && self.parser.callbacks().open.as_ref().is_some_and(|open| foreground::is_login(&open.record.command))
    }

    /// True while the application cursor mode is on (vt100's `Screen::application_cursor`): arrows and Home/End then take the
    /// `ESC O` form. Read under the caller's lock, right before the keys are encoded.
    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    /// Whether `send_keys` may type into the command on top now, and the id of that command. Writes nothing. The writer
    /// calls it under the same lock as the write, so the answer holds for the bytes it sends. Only a command the agent
    /// started and that is running (Running or Alt) takes keys. `record` is the id the call was approved for (or the one
    /// the model named), and `level` the shell key it was approved for. None means no check of that kind.
    pub fn keys_target(&self, record: Option<u64>, level: Option<u64>) -> Result<u64, KeysRefusal> {
        if self.desynced {
            return Err(KeysRefusal::Desynced);
        }
        if level.is_some_and(|key| key != self.top_key()) {
            return Err(KeysRefusal::Location);
        }
        let state = self.lifecycle_view();
        if matches!(state, InputState::Prompt | InputState::Submitted) {
            return Err(KeysRefusal::NothingRunning);
        }
        if !matches!(state, InputState::Running | InputState::Alt) {
            return Err(KeysRefusal::NotRunning(state));
        }
        let Some(open) = self.parser.callbacks().open.as_ref() else {
            return Err(KeysRefusal::NoCommand);
        };
        if !matches!(open.record.source, Source::Agent { .. }) {
            return Err(KeysRefusal::NotAgents);
        }
        let open_id = open.record.id;
        // A command's D closes its record, so an open record has not had its D. The check is kept anyway: a finished record
        // must never take keys, whatever closed it.
        if self.records.iter().any(|r| r.id == open_id) {
            return Err(KeysRefusal::Finished);
        }
        if let Some(wanted) = record.filter(|&id| id != open_id) {
            return Err(KeysRefusal::WrongCommand { wanted: Some(wanted), open: open_id });
        }
        Ok(open_id)
    }

    /// Called by the writer under the same lock as the check. Moves to Submitted and records the pending attribution.
    /// The line leaves the prompt, so the user's text is no longer there either (an empty line never gets a C).
    pub fn begin_submit(&mut self, pending: Option<(Source, Option<String>)>) {
        self.lifecycle = Lifecycle::Submitted { since: Instant::now() };
        self.pending = pending;
        self.user_typed = false;
    }

    /// Records the user's bytes for the typed-text check. Counted in Prompt, Submitted, Continuation and Running. While a
    /// command runs, the bytes stay in the tty and zsh draws them on the next prompt line (type-ahead), so they count.
    /// Not counted in Alt, Raw, Booting or Exited: a full-screen program or a shell without integration reads those keys.
    /// Accepted false positives: a program that reads stdin, an ssh session.
    ///
    /// Ctrl-C at the prompt discards the line, so it clears the flag. Ctrl-C while a command runs does not clear it.
    /// Probed with zsh on macOS: `zqx` typed during `sleep 3`, then Ctrl-C, left the new prompt empty, so the tty did
    /// discard the type-ahead. That depends on the foreground program keeping ISIG on and not setting NOFLSH, and a
    /// program that never reads input may leave the bytes behind. Not clearing costs one extra Ctrl-C at the prompt.
    ///
    /// Ctrl-L clears the screen and redraws the same line, so it changes nothing. A CR does not set the flag: a CR at the
    /// prompt submits the line (`begin_submit`). Any other byte, arrow keys included (history recall puts text on the
    /// line), sets it.
    ///
    /// While a program that holds the keys runs in the foreground (ssh, a nested shell, su, a container exec, a
    /// multiplexer: see `foreground_holds_the_keys`), the bytes belong to it and never reach the prompt line, so they do
    /// not count. Nothing clears the flag when that program exits, so the bytes must not set it in the first place.
    pub fn note_user_bytes(&mut self, bytes: &[u8]) {
        let state = self.lifecycle_view();
        // Unresponsive counts like Submitted: a submitted line is still in the tty and may reach the prompt.
        if !matches!(
            state,
            InputState::Prompt | InputState::Submitted | InputState::Unresponsive | InputState::Continuation | InputState::Running
        ) {
            return;
        }
        if state == InputState::Running && self.foreground_holds_the_keys() {
            return;
        }
        let running = state == InputState::Running;
        for &b in bytes {
            match b {
                0x03 if !running => self.user_typed = false,
                0x03 | b'\r' | 0x0c => {}
                _ => self.user_typed = true,
            }
        }
    }

    /// True while the user has text on the prompt line (see the field). run_command is refused meanwhile.
    pub fn user_typed_since_prompt(&self) -> bool {
        self.user_typed
    }

    /// A CR the user typed while zsh waits for more of a command. Attributes the command it completes to the user,
    /// without changing the lifecycle.
    pub fn note_user_line(&mut self) {
        // An agent line still waiting for its C is completed by the user's text: the agent's own line never runs.
        if let Some((Source::Agent { call_id }, _)) = &self.pending {
            let call_id = call_id.clone();
            self.mark_abandoned(call_id);
        }
        self.pending = Some((Source::User, None));
    }

    pub fn forged_events(&self) -> u64 {
        self.scanner.forged_events()
    }

    /// The shell's directory: the top level's, from its 633;Cwd when a nested shell is on top, else the local shell's (OSC 7).
    /// None on a nested level that has not reported a directory yet.
    pub fn cwd(&self) -> Option<Cwd> {
        match self.stack.last() {
            Some(top) if top.key != LOCAL_KEY => {
                top.cwd.as_ref().map(|path| Cwd { host: top.host.clone().unwrap_or_default(), path: path.clone() })
            }
            _ => self.cwd.clone(),
        }
    }

    pub fn shell_pid(&self) -> Option<u32> {
        self.shell_pid
    }

    pub fn screen_snapshot(&self) -> ScreenSnapshot {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        ScreenSnapshot {
            rows: screen.rows(0, cols.get()).map(|r| r.trim_end().to_string()).collect(),
            cursor: screen.cursor_position(),
            alternate_screen: screen.alternate_screen(),
            size: (rows.get(), cols.get()),
        }
    }

    pub fn records(&self) -> Vec<RecordSummary> {
        self.records.iter().map(RecordSummary::from).collect()
    }

    pub fn get_record(&self, id: u64) -> Option<Record> {
        self.records.iter().find(|r| r.id == id).cloned()
    }

    /// Bumps the screen generation when the visible screen differs from the last call. The observer calls it after
    /// every message and every tick, under the same lock as the apply, so a change is never seen half-applied.
    pub fn note_screen(&mut self) {
        let hash = screen_fingerprint(self.parser.screen());
        if hash != self.screen_hash {
            self.screen_hash = hash;
            self.screen_gen += 1;
            self.screen_changed_at = Some(Instant::now());
        }
    }

    pub fn activity(&self) -> Activity {
        Activity {
            state: self.lifecycle_view(),
            screen_gen: self.screen_gen,
            last_record_id: self.records.back().map_or(0, |r| r.id),
        }
    }
}

/// The core behind a session's locks. The writer, the observer thread, and the API all lock it.
pub struct Shared {
    pub core: Mutex<Core>,
    /// Held while events are taken and delivered, so listeners see them in the order they were raised.
    dispatch: Mutex<()>,
    sink: Mutex<Option<Arc<dyn EventSink>>>,
}

impl Shared {
    pub fn new(core: Core) -> Self {
        Shared { core: Mutex::new(core), dispatch: Mutex::new(()), sink: Mutex::new(None) }
    }

    pub fn lock(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Installs the listener and delivers the current state to it.
    pub fn set_sink(&self, sink: Arc<dyn EventSink>) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
        self.flush();
    }

    /// Delivers pending events. Never called with the core lock held.
    pub fn flush(&self) {
        let _order = self.dispatch.lock().unwrap_or_else(|e| e.into_inner());
        let sink = self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let events = self.lock().take_events(sink.is_some());
        if let Some(sink) = sink {
            for ev in events {
                sink.event(ev);
            }
        }
    }
}

/// How often the observer wakes without bytes, so time-based transitions (Booting to Raw, a submit timing out) are reported.
pub const OBSERVER_TICK: Duration = Duration::from_millis(100);

/// Runs until the queue delivers Eof.
pub fn run_observer(queue: Arc<ObsQueue>, shared: Arc<Shared>) {
    loop {
        let msg = queue.pop_timeout(OBSERVER_TICK);
        let eof = matches!(msg, Some(ObsMsg::Eof));
        {
            let mut core = shared.lock();
            if let Some(msg) = msg {
                core.apply(msg);
            }
            // On every message, not only on a tick: a long burst of output never ticks, and it changes the screen.
            core.note_screen();
        }
        shared.flush();
        if eof {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::foreground::ForegroundKind;

    struct Probe(Vec<(String, bool)>);

    impl Callbacks for Probe {
        fn on_scroll(&mut self, contents: RowContents<'_>, _alternate_screen: bool) {
            self.0.push(row_text(&contents));
        }
    }

    /// Guards the Debug-based wrap readout in `row_text`: a soft-wrapped row must read as wrapped, a hard line end must not.
    #[test]
    fn wrap_flag_is_readable() {
        let mut p = Parser::new_with_callbacks(nz(3), nz(10), 0, Probe(Vec::new()));
        // 29 chars wrap over three 10-column rows, then a hard newline, then a short line.
        p.process(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\nbbb\r\nccc\r\nddd\r\n");
        let rows = &p.callbacks().0;
        assert!(rows.contains(&("aaaaaaaaaa".to_string(), true)), "wrapped row not reported as wrapped: {rows:?}");
        assert!(rows.iter().any(|(t, w)| t.starts_with("aaaaaaaaa") && !w), "hard line end not reported: {rows:?}");
    }

    fn core() -> Core {
        Core::new("test-nonce", (24, 80))
    }

    /// Which lifecycles count typed bytes: the prompt-editing ones and Running (type-ahead). Alt, Raw, Booting and
    /// Exited do not.
    #[test]
    fn typed_bytes_count_only_where_zsh_holds_the_prompt_line() {
        let typed = |setup: &dyn Fn(&mut Core)| {
            let mut c = core();
            setup(&mut c);
            c.note_user_bytes(b"x");
            c.user_typed_since_prompt()
        };
        assert!(typed(&|c| c.lifecycle = Lifecycle::Prompt));
        assert!(typed(&|c| c.lifecycle = Lifecycle::Submitted { since: Instant::now() }));
        assert!(typed(&|c| c.lifecycle = Lifecycle::Continuation { since: Instant::now() }));
        assert!(typed(&|c| c.lifecycle = Lifecycle::Running));
        assert!(!typed(&|c| c.lifecycle = Lifecycle::Booting));
        assert!(!typed(&|c| {
            c.lifecycle = Lifecycle::Prompt;
            c.parser.process(b"\x1b[?1049h");
        }));
        assert!(!typed(&|c| c.spawned_at = Instant::now() - Duration::from_secs(10)), "Raw");
        assert!(!typed(&|c| c.apply(ObsMsg::Eof)), "Exited");
    }

    #[test]
    fn the_raw_and_alt_setups_are_the_states_they_claim() {
        let mut raw = core();
        raw.spawned_at = Instant::now() - Duration::from_secs(10);
        assert_eq!(raw.lifecycle_view(), InputState::Raw);

        let mut alt = core();
        alt.lifecycle = Lifecycle::Prompt;
        alt.parser.process(b"\x1b[?1049h");
        assert_eq!(alt.lifecycle_view(), InputState::Alt);

        let mut exited = core();
        exited.apply(ObsMsg::Eof);
        assert_eq!(exited.lifecycle_view(), InputState::Exited);
    }

    /// Type-ahead typed while a command runs survives the transition into Prompt: zsh draws it there.
    #[test]
    fn type_ahead_survives_the_transition_to_the_prompt() {
        let mut c = core();
        c.lifecycle = Lifecycle::Running;
        c.note_user_bytes(b"zqx");
        c.handle(Event::PromptStart);
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
        assert!(c.user_typed_since_prompt(), "the flag was cleared by the transition into Prompt");
    }

    /// Ctrl-C at the prompt discards the line. Ctrl-C while a command runs does not clear type-ahead (see note_user_bytes).
    #[test]
    fn ctrl_c_clears_at_the_prompt_but_not_while_a_command_runs() {
        let mut prompt = core();
        prompt.lifecycle = Lifecycle::Prompt;
        prompt.note_user_bytes(b"x");
        prompt.note_user_bytes(&[0x03]);
        assert!(!prompt.user_typed_since_prompt());

        let mut running = core();
        running.lifecycle = Lifecycle::Running;
        running.note_user_bytes(b"x");
        running.note_user_bytes(&[0x03]);
        assert!(running.user_typed_since_prompt());
        running.handle(Event::PromptStart);
        running.note_user_bytes(&[0x03]);
        assert!(!running.user_typed_since_prompt(), "Ctrl-C at the new prompt did not clear");
    }

    /// Bytes in one write are processed in order: text typed before a Ctrl-C at the prompt is cleared by it.
    #[test]
    fn a_write_is_processed_in_order() {
        let mut c = core();
        c.lifecycle = Lifecycle::Prompt;
        c.note_user_bytes(b"x\x03");
        assert!(!c.user_typed_since_prompt());
        c.note_user_bytes(b"\x03x");
        assert!(c.user_typed_since_prompt());
    }

    /// A CR does not set the flag (a CR at the prompt submits the line), nor does Ctrl-L (it redraws the same line).
    #[test]
    fn cr_and_ctrl_l_do_not_set_the_flag() {
        for state in [Lifecycle::Prompt, Lifecycle::Running] {
            let mut c = core();
            c.lifecycle = state;
            c.note_user_bytes(b"\r");
            c.note_user_bytes(&[0x0c]);
            assert!(!c.user_typed_since_prompt());
        }
    }

    #[test]
    fn a_submit_and_command_start_clear_the_flag() {
        let mut submitted = core();
        submitted.lifecycle = Lifecycle::Prompt;
        submitted.note_user_bytes(b"x");
        submitted.begin_submit(None);
        assert!(!submitted.user_typed_since_prompt(), "begin_submit did not clear");

        let mut started = core();
        started.lifecycle = Lifecycle::Prompt;
        started.note_user_bytes(b"x");
        started.handle(Event::CommandStart);
        assert!(!started.user_typed_since_prompt(), "C did not clear");
    }

    const WAIT: Duration = Duration::from_secs(2);

    fn agent(call_id: &str) -> Source {
        Source::Agent { call_id: call_id.to_string() }
    }

    /// An agent line submitted at the prompt, then zsh asks for more (PS2).
    fn agent_in_continuation(c: &mut Core, call_id: &str) {
        c.lifecycle = Lifecycle::Submitted { since: Instant::now() };
        c.begin_submit(Some((agent(call_id), Some("echo `x".into()))));
        c.handle(Event::Continuation);
    }

    /// An agent command submitted at the prompt and started (its C seen), as `run_command` leaves it while it runs. Output
    /// after this is what the history ring records.
    fn agent_command_open(c: &mut Core, call_id: &str, command: &str) {
        c.lifecycle = Lifecycle::Prompt;
        c.begin_submit(Some((agent(call_id), Some(command.into()))));
        c.handle(Event::CommandLine(command.into()));
        c.handle(Event::CommandStart);
    }

    /// The lines `from..=to` as `prefix-N`, each ending in a line feed.
    fn numbered_lines(prefix: &str, from: usize, to: usize) -> Vec<u8> {
        (from..=to).map(|i| format!("{prefix}-{i}\r\n")).collect::<String>().into_bytes()
    }

    fn status(c: &Core, call_id: &str) -> String {
        match c.call_status(call_id, WAIT) {
            CallStatus::Finished(_) => "finished".into(),
            CallStatus::Abandoned => "abandoned".into(),
            CallStatus::WaitingForInput => "waiting".into(),
            CallStatus::Pending { state, .. } => format!("pending {state:?}"),
        }
    }

    #[test]
    fn agent_line_cancelled_at_the_continuation_prompt_is_abandoned() {
        let mut c = core();
        agent_in_continuation(&mut c, "run-1");
        c.handle(Event::PromptStart);
        assert_eq!(status(&c, "run-1"), "abandoned");
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
    }

    #[test]
    fn agent_line_completed_by_the_user_is_abandoned() {
        let mut c = core();
        agent_in_continuation(&mut c, "run-2");
        c.note_user_line();
        assert_eq!(status(&c, "run-2"), "abandoned");
    }

    /// A normal agent command: C closes the attribution, and it is never marked abandoned.
    #[test]
    fn a_normal_agent_command_is_not_abandoned_by_its_c() {
        let mut c = core();
        c.lifecycle = Lifecycle::Prompt;
        c.begin_submit(Some((agent("run-3"), Some("echo ok".into()))));
        c.handle(Event::CommandLine("echo ok".into()));
        c.handle(Event::CommandStart);
        assert_eq!(status(&c, "run-3"), "pending Running");
        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        assert_eq!(status(&c, "run-3"), "finished");
    }

    #[test]
    fn waiting_for_input_needs_the_full_wait_in_continuation() {
        let mut c = core();
        agent_in_continuation(&mut c, "run-4");
        assert_eq!(status(&c, "run-4"), "pending Continuation", "reported before the wait");

        c.lifecycle = Lifecycle::Continuation { since: Instant::now() - Duration::from_secs(3) };
        assert_eq!(status(&c, "run-4"), "waiting");
        assert_eq!(
            match c.call_status("run-4", Duration::from_secs(10)) {
                CallStatus::Pending { state, .. } => format!("{state:?}"),
                other => format!("{other:?}"),
            },
            "Continuation",
            "a longer wait must not report yet"
        );
    }

    /// Only a call that is still the pending agent line counts as waiting: someone else's line does not.
    #[test]
    fn waiting_is_only_for_the_calls_own_line() {
        let mut c = core();
        agent_in_continuation(&mut c, "run-5");
        c.lifecycle = Lifecycle::Continuation { since: Instant::now() - Duration::from_secs(3) };
        assert_eq!(status(&c, "other-call"), "pending Continuation");
    }

    #[test]
    fn the_abandoned_set_is_bounded_and_forgets_the_oldest() {
        let mut c = core();
        for i in 0..(ABANDONED_CAP + 10) {
            c.mark_abandoned(format!("run-{i}"));
        }
        assert_eq!(c.abandoned.len(), ABANDONED_CAP);
        assert_eq!(status(&c, "run-0"), "pending Booting", "the oldest entry was not forgotten");
        assert_eq!(status(&c, &format!("run-{}", ABANDONED_CAP + 9)), "abandoned");
        c.mark_abandoned(format!("run-{}", ABANDONED_CAP + 9));
        assert_eq!(c.abandoned.len(), ABANDONED_CAP, "a repeat mark grew the set");
    }

    /// The screen generation moves on a visible change (text, cursor, alternate screen, size) and not otherwise.
    #[test]
    fn screen_gen_moves_only_when_the_visible_screen_changes() {
        let mut c = core();
        let start = c.activity().screen_gen;
        c.note_screen();
        assert_eq!(c.activity().screen_gen, start, "an unchanged screen bumped the generation");

        c.parser.process(b"hello");
        c.note_screen();
        let text = c.activity().screen_gen;
        assert_eq!(text, start + 1, "new text did not bump the generation");
        c.note_screen();
        assert_eq!(c.activity().screen_gen, text, "a repeated call with no change bumped the generation");

        c.parser.process(b"\x1b[3;1H");
        c.note_screen();
        assert_eq!(c.activity().screen_gen, text + 1, "a cursor move is a visible change");

        c.parser.process(b"\x1b[?1049h");
        c.note_screen();
        assert_eq!(c.activity().screen_gen, text + 2, "entering the alternate screen did not bump the generation");

        c.resize(10, 40);
        c.note_screen();
        assert_eq!(c.activity().screen_gen, text + 3, "a resize did not bump the generation");
    }

    /// Opens a record for `command` the way the shell does: E, then C, in the Running lifecycle.
    fn running(command: &str) -> Core {
        let mut c = core();
        c.lifecycle = Lifecycle::Prompt;
        c.handle(Event::CommandLine(command.to_string()));
        c.handle(Event::CommandStart);
        c
    }

    /// The foreground hint comes from the open record while the shell is Running, and from nothing else.
    #[test]
    fn foreground_is_the_open_records_command_only_while_running() {
        let c = running("ssh ubuntu@lidia");
        assert_eq!(c.lifecycle_view(), InputState::Running);
        let fg = c.foreground().expect("a hint while the ssh record is open");
        assert_eq!(fg.kind, ForegroundKind::Ssh);
        assert_eq!(fg.command, "ssh ubuntu@lidia");

        let mut prompt = core();
        prompt.lifecycle = Lifecycle::Prompt;
        assert_eq!(prompt.foreground(), None, "no hint at the prompt");

        let mut ended = running("ssh ubuntu@lidia");
        ended.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        assert_eq!(ended.foreground(), None, "no hint once D closed the record, even before the next prompt");

        let mut submitted = core();
        submitted.lifecycle = Lifecycle::Submitted { since: Instant::now() };
        assert_eq!(submitted.foreground(), None, "no hint while the line is only submitted");
    }

    /// The hint is left out in the alternate screen (a full-screen program has its own state) and when the command
    /// text is blank (nothing says what runs).
    #[test]
    fn no_hint_in_the_alternate_screen_or_for_a_blank_command() {
        let mut alt = running("ssh ubuntu@lidia");
        alt.parser.process(b"\x1b[?1049h");
        assert_eq!(alt.lifecycle_view(), InputState::Alt);
        assert_eq!(alt.foreground(), None);

        let blank = running("   ");
        assert_eq!(blank.lifecycle_view(), InputState::Running);
        assert_eq!(blank.foreground(), None);
    }

    /// The hint is on the wire as an optional `foreground` field of the state event: present while a command runs, and
    /// left out at a prompt. A change of the hint alone is reported as a state event.
    #[test]
    fn the_state_event_carries_the_foreground_hint_only_when_there_is_one() {
        let mut c = core();
        c.lifecycle = Lifecycle::Prompt;
        let first = c.take_events(true);
        let wire = |evs: &[SessionEvent]| -> Vec<serde_json::Value> {
            evs.iter().filter(|e| matches!(e, SessionEvent::State { .. })).map(|e| serde_json::to_value(e).unwrap()).collect()
        };
        assert_eq!(wire(&first), vec![serde_json::json!({"type": "state", "state": "prompt"})]);

        c.handle(Event::CommandLine("ssh ubuntu@lidia".into()));
        c.handle(Event::CommandStart);
        let running = c.take_events(true);
        assert_eq!(
            wire(&running),
            vec![serde_json::json!({
                "type": "state",
                "state": "running",
                "foreground": {"kind": "ssh", "command": "ssh ubuntu@lidia"},
            })]
        );

        // Same lifecycle, same hint: nothing new is reported.
        assert!(wire(&c.take_events(true)).is_empty());

        // Back at the prompt: the state changes and the hint goes away from the wire.
        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        c.handle(Event::PromptStart);
        assert_eq!(wire(&c.take_events(true)), vec![serde_json::json!({"type": "state", "state": "prompt"})]);
    }

    /// Keys typed while ssh, a nested shell, su, a container exec or a multiplexer runs belong to that program: they do not
    /// set the flag, so the prompt after it is clean. Ctrl-C too: it does not clear anything that was not set.
    #[test]
    fn keys_typed_into_a_program_that_holds_the_keys_do_not_set_the_flag() {
        for command in ["ssh ubuntu@lidia", "zsh -f -i", "su -", "docker exec -it app sh", "tmux attach"] {
            let mut c = running(command);
            assert_eq!(c.lifecycle_view(), InputState::Running, "{command}");
            c.note_user_bytes(b"echo hi\r");
            c.note_user_bytes(b"exit\r");
            c.note_user_bytes(&[0x03]);
            assert!(!c.user_typed_since_prompt(), "keys typed into `{command}` set the flag");
            c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
            c.handle(Event::PromptStart);
            assert_eq!(c.lifecycle_view(), InputState::Prompt, "{command}");
            assert!(!c.user_typed_since_prompt(), "`{command}` left the flag behind at the prompt");
            c.note_user_bytes(b"x");
            assert!(c.user_typed_since_prompt(), "the prompt after `{command}` does not count typed text");
        }
    }

    /// An ordinary command keeps the type-ahead rule: its keys land on the prompt line, so they set the flag.
    #[test]
    fn keys_typed_during_an_ordinary_command_still_set_the_flag() {
        let mut c = running("sleep 2");
        assert_eq!(c.foreground().map(|f| f.kind), Some(ForegroundKind::Other));
        c.note_user_bytes(b"zqx");
        assert!(c.user_typed_since_prompt());
    }

    /// The exemption is only while the program runs. Typed text at the prompt, and an alt-screen program that the hint does
    /// not name, follow the rules they always had.
    #[test]
    fn the_exemption_ends_with_the_program_and_never_applies_at_the_prompt() {
        let mut prompt = core();
        prompt.lifecycle = Lifecycle::Prompt;
        prompt.note_user_bytes(b"x");
        assert!(prompt.user_typed_since_prompt(), "typed text at the prompt no longer counts");

        let mut alt = running("ssh ubuntu@lidia");
        alt.parser.process(b"\x1b[?1049h");
        assert_eq!(alt.lifecycle_view(), InputState::Alt);
        alt.note_user_bytes(b"x");
        assert!(!alt.user_typed_since_prompt(), "alt screen rule changed");
    }

    // ---------- nested levels ----------

    const LOCAL: &str = "test-nonce";
    const CHILD: &str = "child-nonce";

    fn osc(body: &str) -> Vec<u8> {
        format!("\x1b]{body}\x1b\\").into_bytes()
    }

    /// A tagged integration event: `code` (e.g. `133;A`), the nonce in field 2, then `rest` (empty or starting with `;`).
    fn tagged(code: &str, nonce: &str, rest: &str) -> Vec<u8> {
        osc(&format!("{code};n={nonce}{rest}"))
    }

    /// The local shell runs `ssh ubuntu@lidia`, and the remote shell has said Hello with CHILD: it is pushed, Booting.
    fn nested() -> Core {
        let mut c = running("ssh ubuntu@lidia");
        assert_eq!(c.expect_level(CHILD, "ssh ubuntu@lidia"), 1);
        c.feed(&tagged("633;Hello", CHILD, ";v=1;sh=bash;user=ubuntu;host=lidia;pid=77"));
        c
    }

    /// The remote shell at its first prompt.
    fn nested_at_prompt() -> Core {
        let mut c = nested();
        c.feed(&tagged("133;A", CHILD, ""));
        c
    }

    /// The remote shell runs `command` (E, then C).
    fn remote_running(c: &mut Core, command: &str) {
        c.feed(&tagged("633;E", CHILD, &format!(";{command}")));
        c.feed(&tagged("133;C", CHILD, ""));
    }

    /// (level count, nonces) of each Levels event in `evs`.
    fn levels_events(evs: &[SessionEvent]) -> Vec<(usize, Vec<String>)> {
        evs.iter()
            .filter_map(|e| match e {
                SessionEvent::Levels { levels, nonces } => Some((levels.len(), nonces.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_hello_with_the_expected_nonce_pushes_a_level_that_boots() {
        let c = nested();
        assert_eq!(c.depth(), 2);
        assert_eq!(c.lifecycle_view(), InputState::Booting);
        let levels = c.levels();
        assert_eq!(levels.iter().map(|l| l.key).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(levels[0].kind, LevelKind::Local);
        assert_eq!(levels[1].kind, LevelKind::Remote);
        assert_eq!(levels[1].label, "ssh ubuntu@lidia");
        assert_eq!(levels[1].host.as_deref(), Some("lidia"));
        assert_eq!(levels[1].user.as_deref(), Some("ubuntu"));
        assert_eq!(levels[1].sh.as_deref(), Some("bash"));
        assert_eq!(levels[1].state, InputState::Booting);
        assert_eq!(levels[0].state, InputState::Running, "the parked local shell is Running");
        assert_eq!(c.location().as_deref(), Some("ssh ubuntu@lidia"));
        assert_eq!(c.integration_status(), IntegrationStatus::Live);
        assert_eq!(c.shell_pid(), None, "a nested Hello does not change the local shell's pid");
        assert_eq!(core().location(), None);
    }

    #[test]
    fn a_nested_level_goes_booting_prompt_submitted_running_prompt() {
        let mut c = nested_at_prompt();
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
        c.begin_submit(Some((agent("run-n"), Some("ls".into()))));
        assert_eq!(c.lifecycle_view(), InputState::Submitted);
        c.feed(&tagged("633;E", CHILD, ";ls"));
        c.feed(&tagged("133;C", CHILD, ""));
        assert_eq!(c.lifecycle_view(), InputState::Running);
        assert_eq!(status(&c, "run-n"), "pending Running");
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        c.feed(&tagged("133;A", CHILD, ""));
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
        assert_eq!(status(&c, "run-n"), "finished");
        assert_eq!(c.depth(), 2, "the remote level is still the top");
    }

    /// The local shell reports while the remote one is on top: the remote shell has gone, so its level is popped. Its
    /// open record closes Incomplete, the local record resumes as Running, and the local A then makes it Prompt.
    #[test]
    fn an_event_from_a_level_below_the_top_pops_the_levels_above_it() {
        let mut c = nested_at_prompt();
        remote_running(&mut c, "sleep 9");
        c.feed(&tagged("133;D", LOCAL, ";exit=0;ps=0"));
        assert_eq!(c.depth(), 1);
        assert_eq!(c.lifecycle_view(), InputState::Running, "the local shell resumes Running, never Prompt");
        let recs = c.records();
        assert_eq!(recs.len(), 2, "{recs:?}");
        assert_eq!((recs[0].command.as_str(), recs[0].level, recs[0].status), ("sleep 9", 1, RecordStatus::Incomplete));
        assert_eq!(recs[1].command, "ssh ubuntu@lidia");
        assert_eq!(recs[1].level, 0);
        assert_eq!(recs[1].status, RecordStatus::Approximate, "its output while the remote shell was on top is missing");
        c.feed(&tagged("133;A", LOCAL, ""));
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
    }

    /// Once popped, the remote shell's nonce is retired: its late events count as stale, not forged.
    #[test]
    fn late_events_from_a_popped_level_are_stale_not_forged() {
        let mut c = nested_at_prompt();
        c.feed(&tagged("133;D", LOCAL, ";exit=0;ps=0"));
        assert_eq!(c.depth(), 1);
        c.feed(&tagged("133;A", CHILD, ""));
        c.feed(&tagged("633;E", CHILD, ";ls"));
        assert_eq!(c.stale_events(), 2);
        assert_eq!(c.forged_events(), 0);
        assert_eq!(c.lifecycle_view(), InputState::Running, "a stale event changes nothing");
    }

    #[test]
    fn a_push_is_refused_while_the_local_shell_is_not_running_and_the_pending_level_waits() {
        let mut c = running("ssh ubuntu@lidia");
        c.lifecycle = Lifecycle::Prompt;
        c.expect_level(CHILD, "ssh ubuntu@lidia");
        c.feed(&tagged("633;Hello", CHILD, ";v=1;pid=3"));
        assert_eq!(c.depth(), 1);
        assert_eq!(c.refused_levels(), 1);
        c.lifecycle = Lifecycle::Running;
        c.feed(&tagged("633;Hello", CHILD, ";v=1;pid=3"));
        assert_eq!(c.depth(), 2, "the pending level still takes its Hello once the local shell is Running");
    }

    #[test]
    fn a_hello_with_a_nonce_nobody_expected_is_forged_and_pushes_nothing() {
        let mut c = running("ssh ubuntu@lidia");
        c.expect_level(CHILD, "ssh ubuntu@lidia");
        c.feed(&tagged("633;Hello", "some-other-nonce", ";v=1;pid=3"));
        assert_eq!(c.depth(), 1);
        assert_eq!(c.forged_events(), 1);
        assert_eq!(c.refused_levels(), 0);
    }

    #[test]
    fn the_depth_cap_refuses_the_push_that_would_pass_it() {
        let mut c = running("ssh h");
        for i in 1..MAX_DEPTH {
            let nonce = format!("nested-{i}");
            c.expect_level(&nonce, "ssh h");
            c.feed(&tagged("633;Hello", &nonce, ";v=1;pid=1"));
            assert_eq!(c.depth(), i + 1);
            c.feed(&tagged("133;A", &nonce, ""));
            c.feed(&tagged("633;E", &nonce, ";sleep 1"));
            c.feed(&tagged("133;C", &nonce, ""));
        }
        assert_eq!(c.depth(), MAX_DEPTH);
        c.expect_level("one-too-many", "ssh h");
        c.feed(&tagged("633;Hello", "one-too-many", ";v=1;pid=1"));
        assert_eq!(c.depth(), MAX_DEPTH);
        assert_eq!(c.refused_levels(), 1);
    }

    #[test]
    fn a_pending_level_that_waits_too_long_is_dropped_and_its_nonce_is_stale() {
        let mut c = running("ssh h");
        c.expect_level(CHILD, "ssh h");
        c.pending_level.as_mut().expect("a pending level").since = Instant::now() - PENDING_TIMEOUT - Duration::from_secs(1);
        c.take_events(true);
        c.feed(&tagged("633;Hello", CHILD, ";v=1;pid=1"));
        assert_eq!(c.depth(), 1);
        assert_eq!((c.stale_events(), c.forged_events()), (1, 0));
    }

    #[test]
    fn a_new_expect_replaces_the_pending_level_and_retires_its_nonce() {
        let mut c = running("ssh h");
        c.expect_level("first", "ssh a");
        let second = c.expect_level("second", "ssh b");
        c.feed(&tagged("633;Hello", "first", ";v=1;pid=1"));
        assert_eq!(c.depth(), 1);
        assert_eq!(c.stale_events(), 1);
        c.feed(&tagged("633;Hello", "second", ";v=1;pid=2"));
        assert_eq!(c.depth(), 2);
        assert_eq!(c.levels()[1].key, second);
    }

    /// The Levels event carries the level count and every accepted nonce, so the frontend can check OSC against them.
    #[test]
    fn push_and_pop_emit_the_levels_and_the_nonces_to_accept() {
        let mut c = running("ssh h");
        c.take_events(true);
        c.expect_level(CHILD, "ssh h");
        let announced = levels_events(&c.take_events(true));
        assert_eq!(announced.len(), 1);
        assert_eq!(announced[0].0, 1, "only the local level exists until the Hello");
        assert!(announced[0].1.contains(&CHILD.to_string()), "the pending nonce is announced before its shell says Hello");

        c.feed(&tagged("633;Hello", CHILD, ";v=1;pid=1"));
        let pushed = levels_events(&c.take_events(true));
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].0, 2);
        assert!(pushed[0].1.contains(&LOCAL.to_string()) && pushed[0].1.contains(&CHILD.to_string()));

        c.feed(&tagged("133;A", LOCAL, ""));
        let popped = levels_events(&c.take_events(true));
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].0, 1);
        assert!(!popped[0].1.contains(&CHILD.to_string()), "a popped nonce is no longer accepted");
    }

    /// `user_typed` is reset by a push and by a pop, so text typed under the parent is not carried into the child.
    #[test]
    fn typed_text_is_reset_on_push_and_on_pop() {
        let mut c = running("sleep 30");
        c.note_user_bytes(b"zqx");
        assert!(c.user_typed_since_prompt());
        c.expect_level(CHILD, "ssh h");
        c.feed(&tagged("633;Hello", CHILD, ";v=1;pid=5"));
        assert_eq!(c.depth(), 2);
        assert!(!c.user_typed_since_prompt(), "a push reset the flag");
        c.feed(&tagged("133;A", CHILD, ""));
        c.note_user_bytes(b"abc");
        assert!(c.user_typed_since_prompt());
        c.feed(&tagged("133;A", LOCAL, ""));
        assert_eq!(c.depth(), 1);
        assert!(!c.user_typed_since_prompt(), "a pop reset the flag");
    }

    /// One capture target: while the remote level is on top, its output goes to its own record and not the parked one.
    #[test]
    fn output_while_a_nested_level_is_on_top_is_not_attributed_to_the_parent() {
        let mut c = nested_at_prompt();
        remote_running(&mut c, "seq 40");
        let mut out = Vec::new();
        for i in 0..40 {
            out.extend(format!("remote-{i}\r\n").into_bytes());
        }
        c.feed(&out);
        let parked = c.stack[0].parked.as_ref().and_then(|p| p.open.as_ref()).expect("the parent's record is parked");
        assert!(!format!("{:?}", parked.builder).contains("remote-"), "the parent's record took the child's output");
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        let child = c.get_record(2).expect("the remote record");
        assert_eq!(child.level, 1);
        assert!(child.output.contains("remote-0"), "{:?}", child.output);
    }

    /// The foreground hint is the top level's: the parked ssh hint is not reported while the remote shell is on top.
    #[test]
    fn foreground_answers_for_the_top_level_only() {
        let mut c = nested();
        assert_eq!(c.foreground(), None, "the remote shell is Booting");
        c.feed(&tagged("133;A", CHILD, ""));
        assert_eq!(c.foreground(), None, "the remote shell is at its prompt");
        remote_running(&mut c, "ls -la");
        let fg = c.foreground().expect("the remote command is the hint");
        assert_eq!((fg.kind, fg.command.as_str()), (ForegroundKind::Other, "ls -la"));
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        c.feed(&tagged("133;A", CHILD, ""));
        remote_running(&mut c, "ssh deeper");
        assert_eq!(c.foreground().map(|f| f.kind), Some(ForegroundKind::Ssh));
    }

    /// Submitted at a nested level that is not answering becomes Unresponsive. Its own C or A ends that.
    #[test]
    fn a_submit_that_times_out_at_a_nested_level_is_unresponsive_until_its_own_a_or_c() {
        let mut c = nested_at_prompt();
        c.begin_submit(None);
        c.lifecycle = Lifecycle::Submitted { since: Instant::now() - Duration::from_secs(5) };
        assert_eq!(c.lifecycle_view(), InputState::Unresponsive);
        c.note_user_bytes(b"x");
        assert!(c.user_typed_since_prompt(), "typed text counts while the line may still be in the tty");
        c.feed(&tagged("133;A", CHILD, ""));
        assert_eq!(c.lifecycle_view(), InputState::Prompt, "its own A recovers it");

        c.begin_submit(None);
        c.lifecycle = Lifecycle::Submitted { since: Instant::now() - Duration::from_secs(5) };
        c.feed(&tagged("133;C", CHILD, ""));
        assert_eq!(c.lifecycle_view(), InputState::Running, "its own C recovers it too");
    }

    #[test]
    fn the_local_shell_still_reverts_a_timed_out_submit_to_prompt() {
        let mut c = core();
        c.lifecycle = Lifecycle::Submitted { since: Instant::now() - Duration::from_secs(5) };
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
    }

    #[test]
    fn the_alternate_screen_overrides_a_nested_level() {
        let mut c = nested_at_prompt();
        c.feed(b"\x1b[?1049h");
        assert_eq!(c.lifecycle_view(), InputState::Alt);
        c.note_user_bytes(b"x");
        assert!(!c.user_typed_since_prompt());
    }

    /// After a desync only a prompt is trusted: the next A from a live level, here the local one below the remote, resumes.
    #[test]
    fn after_a_desync_the_next_prompt_from_any_live_level_is_trusted() {
        let mut c = nested_at_prompt();
        remote_running(&mut c, "sleep 9");
        c.apply(ObsMsg::Resync);
        assert!(c.desynced());
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        assert_eq!(c.depth(), 2, "a non-prompt event is ignored while desynced");
        c.feed(&tagged("133;A", LOCAL, ""));
        assert!(!c.desynced());
        assert_eq!(c.depth(), 1);
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
    }

    #[test]
    fn after_a_desync_the_next_prompt_of_the_top_level_is_trusted() {
        let mut c = nested_at_prompt();
        c.apply(ObsMsg::Resync);
        c.feed(&tagged("133;A", CHILD, ""));
        assert!(!c.desynced());
        assert_eq!(c.depth(), 2);
        assert_eq!(c.lifecycle_view(), InputState::Prompt);
    }

    /// The end of the session closes the remote record and the local one, both Incomplete.
    #[test]
    fn the_end_of_the_session_closes_every_open_record() {
        let mut c = nested_at_prompt();
        remote_running(&mut c, "sleep 9");
        c.apply(ObsMsg::Eof);
        assert_eq!(c.lifecycle_view(), InputState::Exited);
        assert_eq!(c.depth(), 1);
        let recs = c.records();
        assert_eq!(recs.len(), 2, "{recs:?}");
        assert!(recs.iter().all(|r| r.status == RecordStatus::Incomplete), "{recs:?}");
    }

    /// A remote level reports its own cwd (tagged 633;Cwd), records take it, and untagged OSC 7 is ignored while it is on top.
    #[test]
    fn a_nested_level_reports_its_own_cwd_and_the_local_osc7_is_ignored_on_top() {
        let mut c = nested_at_prompt();
        // Before the level's own 633;Cwd there is no directory for it: the local OSC 7 is not used for it.
        c.feed(&osc("7;file://mac.local/tmp/local"));
        assert_eq!(c.cwd(), None, "a nested level with no directory yet does not show the local one");
        c.feed(&tagged("633;Cwd", CHILD, ";/srv/app"));
        c.feed(&osc("7;file://lidia/tmp/elsewhere"));
        assert_eq!(c.cwd(), Some(Cwd { host: "lidia".into(), path: "/srv/app".into() }), "the top level's cwd, not OSC 7");
        remote_running(&mut c, "ls");
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        let rec = c.get_record(2).expect("the remote record");
        assert_eq!(rec.cwd, Some(Cwd { host: "lidia".into(), path: "/srv/app".into() }));
        assert_eq!(rec.location.as_deref(), Some("ssh ubuntu@lidia"));
    }

    /// Output on the local shell's own cwd still works with no nested level, as before.
    #[test]
    fn the_local_cwd_is_untouched_without_a_nested_level() {
        let mut c = core();
        c.feed(&osc("7;file://mac.local/tmp/x"));
        assert_eq!(c.cwd(), Some(Cwd { host: "mac.local".into(), path: "/tmp/x".into() }));
    }

    // ---------- injection (B2b) ----------

    const TOKEN: &str = "TOK123";

    fn ready(token: &str, sh: &str) -> Vec<u8> {
        osc(&format!("633;Ready;t={token};sh={sh}"))
    }

    #[test]
    fn injection_is_refused_unless_a_shell_program_runs_at_a_running_top() {
        let mut prompt = core();
        prompt.lifecycle = Lifecycle::Prompt;
        assert_eq!(prompt.begin_injection(TOKEN), Err(InjectRefusal::NotRunning));

        let mut alt = running("ssh ubuntu@lidia");
        alt.parser.process(b"\x1b[?1049h");
        assert_eq!(alt.begin_injection(TOKEN), Err(InjectRefusal::AltScreen));

        let mut sleep = running("sleep 5");
        assert_eq!(sleep.begin_injection(TOKEN), Err(InjectRefusal::NoShellProgram));
        let mut multiplexer = running("tmux attach");
        assert_eq!(multiplexer.begin_injection(TOKEN), Err(InjectRefusal::NoShellProgram));

        for command in ["ssh ubuntu@lidia", "zsh -f -i", "su -", "docker exec -it app sh"] {
            let mut ok = running(command);
            assert_eq!(ok.begin_injection(TOKEN), Ok(()), "{command}");
            ok.end_injection();
        }
    }

    #[test]
    fn injection_is_busy_while_one_is_in_flight_or_a_level_is_pending() {
        let mut c = running("zsh -f -i");
        assert_eq!(c.begin_injection(TOKEN), Ok(()));
        assert_eq!(c.begin_injection("other"), Err(InjectRefusal::Busy));
        c.end_injection();

        c.expect_level(CHILD, "zsh -f -i");
        assert_eq!(c.begin_injection(TOKEN), Err(InjectRefusal::Busy), "a pending level is in the way");
        c.cancel_pending_level();
        assert_eq!(c.begin_injection(TOKEN), Ok(()));
    }

    #[test]
    fn injection_is_too_deep_at_the_depth_cap() {
        let mut deep = running("ssh h");
        for i in 1..MAX_DEPTH {
            let nonce = format!("nested-{i}");
            deep.expect_level(&nonce, "ssh h");
            deep.feed(&tagged("633;Hello", &nonce, ";v=1;pid=1"));
            deep.feed(&tagged("133;A", &nonce, ""));
            deep.feed(&tagged("633;E", &nonce, ";sleep 1"));
            deep.feed(&tagged("133;C", &nonce, ""));
        }
        assert_eq!(deep.depth(), MAX_DEPTH);
        assert_eq!(deep.begin_injection(TOKEN), Err(InjectRefusal::TooDeep));
    }

    /// Ready is untagged and matched by its token. It sets the shell only for the injection in flight, at any depth.
    #[test]
    fn a_ready_with_the_token_in_flight_sets_the_shell_at_any_depth() {
        let mut c = running("zsh -f -i");
        c.begin_injection(TOKEN).expect("injection");
        c.feed(&ready("WRONG", "zsh"));
        assert_eq!(c.injection_ready(), None);
        assert_eq!(c.stray_ready_events(), 1);
        c.feed(&ready(TOKEN, "bash"));
        assert_eq!(c.injection_ready().as_deref(), Some("bash"));
        assert_eq!(c.stray_ready_events(), 1);
        c.end_injection();
        c.feed(&ready(TOKEN, "zsh"));
        assert_eq!(c.injection_ready(), None, "no injection in flight");
        assert_eq!(c.stray_ready_events(), 2);

        let mut n = nested_at_prompt();
        remote_running(&mut n, "bash --norc");
        assert_eq!(n.begin_injection(TOKEN), Ok(()));
        n.feed(&ready(TOKEN, "zsh"));
        assert_eq!(n.injection_ready().as_deref(), Some("zsh"), "the loader was typed into the remote shell");
    }

    #[test]
    fn cancel_pending_level_retires_its_nonce_and_announces_it() {
        let mut c = running("zsh -f -i");
        c.expect_level(CHILD, "zsh -f -i");
        c.take_events(true);
        c.cancel_pending_level();
        let announced = levels_events(&c.take_events(true));
        assert_eq!(announced.len(), 1);
        assert!(!announced[0].1.contains(&CHILD.to_string()), "the cancelled nonce is still accepted");
        c.feed(&tagged("633;Hello", CHILD, ";v=1;pid=1"));
        assert_eq!(c.depth(), 1);
        assert_eq!((c.stale_events(), c.forged_events()), (1, 0));
    }

    #[test]
    fn the_top_shell_name_is_the_top_levels_hello() {
        assert_eq!(nested().top_sh(), Some("bash"));
        assert_eq!(core().top_sh(), None);
    }

    /// bash records nothing for a repeated command, so its E is empty. The pending agent line names the record and its source.
    #[test]
    fn a_nested_command_with_an_empty_e_takes_the_agent_line() {
        let mut c = nested_at_prompt();
        c.begin_submit(Some((agent("run-e1"), Some("echo same".into()))));
        c.feed(&tagged("633;E", CHILD, ";"));
        c.feed(&tagged("133;C", CHILD, ""));
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        let rec = c.get_record(2).expect("the remote record");
        assert_eq!(rec.command, "echo same");
        assert!(matches!(&rec.source, Source::Agent { call_id } if call_id == "run-e1"), "{:?}", rec.source);
        assert_eq!(rec.level, 1);
        assert_eq!(rec.status, RecordStatus::Complete);
    }

    #[test]
    fn a_nested_e_that_differs_from_the_agent_line_gives_the_agent_line() {
        let mut c = nested_at_prompt();
        c.begin_submit(Some((agent("run-e2"), Some("ls".into()))));
        c.feed(&tagged("633;E", CHILD, ";ls -la"));
        c.feed(&tagged("133;C", CHILD, ""));
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        let rec = c.get_record(2).expect("the remote record");
        assert_eq!(rec.command, "ls");
        assert!(matches!(&rec.source, Source::Agent { call_id } if call_id == "run-e2"));
    }

    /// No E event at all, but the agent line is pending: the record still takes the agent line.
    #[test]
    fn a_nested_command_with_no_e_event_takes_the_pending_agent_line() {
        let mut c = nested_at_prompt();
        c.begin_submit(Some((agent("run-e3"), Some("pwd".into()))));
        c.feed(&tagged("133;C", CHILD, ""));
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        let rec = c.get_record(2).expect("the remote record");
        assert_eq!(rec.command, "pwd");
        assert!(matches!(&rec.source, Source::Agent { call_id } if call_id == "run-e3"));
    }

    /// Nothing says what ran: the placeholder, with the user as the source.
    #[test]
    fn a_nested_command_with_no_text_and_no_agent_line_is_a_placeholder() {
        let mut c = nested_at_prompt();
        c.feed(&tagged("633;E", CHILD, ";"));
        c.feed(&tagged("133;C", CHILD, ""));
        c.feed(&tagged("133;D", CHILD, ";exit=0;ps=0"));
        let rec = c.get_record(2).expect("the remote record");
        assert_eq!(rec.command, UNAVAILABLE_COMMAND);
        assert!(matches!(rec.source, Source::User));
    }

    /// Level 0 is unchanged: an empty E stays an empty command text, whatever the pending line was.
    #[test]
    fn level_zero_keeps_an_empty_e_as_an_empty_command() {
        let mut c = core();
        c.begin_submit(Some((agent("run-e4"), Some("echo same".into()))));
        c.handle(Event::CommandLine(String::new()));
        c.handle(Event::CommandStart);
        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        let rec = c.get_record(1).expect("the local record");
        assert_eq!(rec.command, "");
        assert!(matches!(rec.source, Source::Unknown));
    }

    #[test]
    fn accepted_nonces_follow_the_levels() {
        // The local nonce is always accepted. A pending level's nonce is accepted, and stays once it is on top. A parent event
        // pops the child, and its nonce is no longer accepted.
        let local = "test-nonce".to_string();
        let mut c = running("ssh ubuntu@lidia");
        assert_eq!(c.accepted_nonces(), vec![local.clone()]);
        c.expect_level(CHILD, "ssh ubuntu@lidia");
        assert_eq!(c.accepted_nonces(), vec![local.clone(), CHILD.to_string()]);
        c.feed(&tagged("633;Hello", CHILD, ";v=1;sh=bash;user=ubuntu;host=lidia;pid=77"));
        assert_eq!(c.top_key(), 1);
        assert_eq!(c.accepted_nonces(), vec![local.clone(), CHILD.to_string()]);
        c.feed(&tagged("133;A", &local, ""));
        assert_eq!(c.top_key(), 0);
        assert_eq!(c.accepted_nonces(), vec![local]);
    }

    /// Lines that scrolled off during an agent command are kept (newest 1000). `screen_lines` reads them with the visible
    /// rows, in order.
    #[test]
    fn the_history_ring_keeps_the_newest_scrolled_rows() {
        let mut c = Core::new("test-nonce", (5, 20));
        agent_command_open(&mut c, "run-h", "seq 1 301");
        let text: String = (1..=300).map(|i| format!("{i}\r\n")).collect::<String>() + "301";
        c.apply(ObsMsg::Bytes(text.into_bytes()));
        let three = c.screen_lines(3);
        assert_eq!(three.rows, ["299", "300", "301"]);
        assert!(!three.truncated && !three.no_history);
        let all = c.screen_lines(1000);
        assert_eq!(all.rows.len(), 301, "296 scrolled rows and the 5 visible ones");
        assert_eq!(all.rows[0], "1");

        let mut long = Core::new("test-nonce", (5, 20));
        agent_command_open(&mut long, "run-long", "seq 1 1500");
        let text: String = (1..=1499).map(|i| format!("{i}\r\n")).collect::<String>() + "1500";
        long.apply(ObsMsg::Bytes(text.into_bytes()));
        let rows = long.screen_lines(1000).rows;
        assert_eq!(rows.len(), 1000, "the ring holds 1000 rows");
        assert_eq!((rows[0].as_str(), rows[999].as_str()), ("501", "1500"));
    }

    /// A taller resize leaves the scrolled-off rows in the history. (vt100 also inserts blank rows at the top of a taller
    /// screen; that is unchanged from before the history existed, and is not asserted here.)
    #[test]
    fn a_taller_resize_keeps_the_history() {
        let mut c = Core::new("test-nonce", (4, 20));
        agent_command_open(&mut c, "run-r", "seq 1 10");
        c.apply(ObsMsg::Bytes("1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10".as_bytes().to_vec()));
        assert_eq!(c.screen_snapshot().rows, ["7", "8", "9", "10"]);
        c.resize(8, 20);
        let lines = c.screen_lines(100).rows;
        assert_eq!(lines.first().map(String::as_str), Some("1"), "{lines:?}");
        assert!(lines.iter().any(|r| r == "6") && lines.last().map(String::as_str) == Some("10"), "{lines:?}");
    }

    /// A soft-wrapped row continues the line before it, so the logical line is whole.
    #[test]
    fn screen_lines_joins_soft_wraps_into_one_logical_line() {
        let mut c = Core::new("test-nonce", (3, 10));
        agent_command_open(&mut c, "run-w", "wrap");
        let text = format!("{}\r\nxyz", "a".repeat(29));
        c.apply(ObsMsg::Bytes(text.into_bytes()));
        let rows = c.screen_lines(10).rows;
        assert!(rows.contains(&"a".repeat(29)), "{rows:?}");
        assert_eq!(rows.last().map(String::as_str), Some("xyz"));
    }

    /// The alternate screen has no history: the visible rows come back, and the flag says so.
    #[test]
    fn the_alternate_screen_gives_the_visible_rows_and_no_history() {
        let mut c = Core::new("test-nonce", (4, 20));
        c.apply(ObsMsg::Bytes(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n\x1b[?1049halt-row".to_vec()));
        let lines = c.screen_lines(10);
        assert!(lines.no_history, "{lines:?}");
        assert!(!lines.truncated);
        assert_eq!(lines.rows.iter().filter(|r| !r.is_empty()).collect::<Vec<_>>(), [&"alt-row".to_string()]);
    }

    /// Over the byte cap the oldest lines go, and the answer says so.
    #[test]
    fn screen_lines_stay_under_the_cap_and_keep_the_newest_lines() {
        let mut c = Core::new("test-nonce", (5, 120));
        agent_command_open(&mut c, "run-c", "cap");
        let text: String = (0..250).map(|i| format!("{i:03}{}\r\n", "x".repeat(100))).collect::<String>() + "last";
        c.apply(ObsMsg::Bytes(text.into_bytes()));
        let lines = c.screen_lines(200);
        assert!(lines.truncated, "{} rows", lines.rows.len());
        let bytes: usize = lines.rows.iter().map(|r| r.len() + 1).sum();
        assert!(bytes <= SCREEN_HISTORY_CAP, "{bytes} bytes");
        assert_eq!(lines.rows.last().map(String::as_str), Some("last"));
    }

    /// A command the user runs records nothing: its scrolled rows are not kept. The agent's command records while it is
    /// open. When it ends its history stays as captured and a later user command adds nothing. The next agent command
    /// starts a fresh ring.
    #[test]
    fn the_ring_records_only_while_an_agent_record_is_open() {
        let mut c = Core::new("test-nonce", (5, 20));
        c.lifecycle = Lifecycle::Prompt;
        c.handle(Event::CommandStart);
        assert!(c.open_agent_records().is_empty(), "a user command is open, not an agent one");
        c.apply(ObsMsg::Bytes(numbered_lines("user", 1, 50)));
        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        let user_only = c.screen_lines(1000).rows;
        assert!(!user_only.iter().any(|r| r == "user-1" || r == "user-20"), "a user command kept history: {user_only:?}");

        agent_command_open(&mut c, "run-a", "make");
        c.apply(ObsMsg::Bytes(numbered_lines("agent", 1, 50)));
        let during = c.screen_lines(1000).rows;
        assert!(during.contains(&"agent-1".to_string()), "{during:?}");
        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        // The shell draws its prompt, then the user starts a command: the ring records nothing of that command.
        c.handle(Event::PromptStart);
        c.handle(Event::CommandStart);
        c.apply(ObsMsg::Bytes(numbered_lines("later", 1, 50)));
        let after = c.screen_lines(1000).rows;
        assert!(after.contains(&"agent-1".to_string()), "the captured history is kept: {after:?}");
        assert!(!after.iter().any(|r| r == "later-1"), "the ring kept recording for a user command: {after:?}");
        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });

        agent_command_open(&mut c, "run-b", "ls");
        c.apply(ObsMsg::Bytes(numbered_lines("second", 1, 3)));
        let fresh = c.screen_lines(1000).rows;
        assert!(!fresh.iter().any(|r| r == "agent-1"), "the next agent record starts a fresh ring: {fresh:?}");
    }

    /// The rows still on screen when an agent command ends scroll off while the prompt is drawn. They are part of the
    /// command's output, so the history keeps them.
    #[test]
    fn the_rows_still_on_screen_when_an_agent_command_ends_are_kept() {
        let mut c = Core::new("test-nonce", (5, 20));
        agent_command_open(&mut c, "run-t", "seq 1 5");
        c.apply(ObsMsg::Bytes(numbered_lines("out", 1, 5)));
        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        c.handle(Event::PromptStart);
        c.apply(ObsMsg::Bytes(b"\r\n\r\n\r\n$ ".to_vec()));
        let rows = c.screen_lines(100).rows;
        for n in 1..=4 {
            assert!(rows.contains(&format!("out-{n}")), "out-{n} scrolled off during the prompt and was lost: {rows:?}");
        }
    }

    /// The history of an agent command that yielded (still running) is there after more output, and by its record id.
    #[test]
    fn history_from_a_yielded_command_is_readable_after_more_output() {
        let mut c = Core::new("test-nonce", (5, 20));
        agent_command_open(&mut c, "run-y", "seq 1 400");
        c.apply(ObsMsg::Bytes(numbered_lines("early", 1, 300)));
        // run_command returns here, while the command runs. Its record id is what wait and get_command use.
        let id = match c.call_status("run-y", WAIT) {
            CallStatus::Pending { record_id: Some(id), .. } => id,
            other => panic!("the yielded command has no record id: {other:?}"),
        };
        c.apply(ObsMsg::Bytes(numbered_lines("late", 1, 60)));
        assert!(c.record_is_open(id), "the command is still open after more output");
        let rows = c.screen_lines(400).rows;
        assert!(rows.contains(&"early-1".to_string()), "the first line of the yielded output is gone: {rows:?}");
        assert!(rows.contains(&"early-300".to_string()) && rows.contains(&"late-60".to_string()), "{rows:?}");
    }

    /// The agent's command still records while a nested shell is on top of it (the record is parked, and still open).
    #[test]
    fn the_ring_keeps_recording_while_an_agent_record_is_parked() {
        let mut c = core();
        agent_command_open(&mut c, "run-p", "ssh h");
        c.expect_level(CHILD, "ssh h");
        c.feed(&tagged("633;Hello", CHILD, ";v=1;sh=bash;user=u;host=h;pid=77"));
        assert_eq!(c.top_key(), 1, "the nested shell is on top");
        assert_eq!(c.open_agent_records().len(), 1, "the agent record is parked, still open");
        c.apply(ObsMsg::Bytes(numbered_lines("nested", 1, 60)));
        let rows = c.screen_lines(100).rows;
        assert!(rows.contains(&"nested-1".to_string()), "{rows:?}");
    }

    /// A shell is still starting until output has come since its command began, and then the screen has been still for
    /// STARTING_QUIET. Until then an integration would not type its Ctrl-C.
    #[test]
    fn a_shell_is_starting_until_output_has_come_and_the_screen_is_still() {
        let mut c = core();
        c.lifecycle = Lifecycle::Prompt;
        c.handle(Event::CommandStart);
        assert!(c.integration_starting(Instant::now()), "no output since its command began");
        c.apply(ObsMsg::Bytes(b"host% ".to_vec()));
        c.note_screen(); // the observer loop does this after every message
        assert!(c.integration_starting(Instant::now()), "the screen changed just now");
        std::thread::sleep(STARTING_QUIET + Duration::from_millis(50));
        assert!(!c.integration_starting(Instant::now()), "output came and the screen has been still");
    }

    /// The early-return signals of an agent's command: on top, a login, bracketed paste from the parser's mode, output since
    /// its start, the quiet time from the last screen change, and the last non-blank row. Another call sees none of it.
    #[test]
    fn yield_signals_read_the_agents_record_and_the_shell_state() {
        let mut c = core();
        agent_command_open(&mut c, "run-s", "zsh -f -i");
        let sig = c.yield_signals("run-s", Instant::now());
        assert!(sig.on_top && sig.login, "{sig:?}");
        assert!(!sig.bracketed_paste && !sig.output_since_start, "{sig:?}");
        assert_eq!(sig.quiet_for, Duration::MAX, "the screen has never changed");
        assert_eq!(sig.last_row, None, "a blank screen has no row");
        assert!(!c.yield_signals("someone-else", Instant::now()).on_top, "another call's record is not this one");
        assert_eq!(c.yield_signals("someone-else", Instant::now()).last_row, None);

        c.apply(ObsMsg::Bytes(b"\x1b[?2004h% ".to_vec()));
        c.note_screen();
        let now = Instant::now();
        let sig = c.yield_signals("run-s", now);
        assert!(sig.bracketed_paste, "vt100's mode is on after the shell turned it on");
        assert!(sig.output_since_start, "output came after the command started");
        assert_eq!(sig.last_row.as_deref(), Some("%"), "the last row, trailing blanks trimmed");
        assert!(sig.quiet_for < Duration::from_secs(1), "{:?}", sig.quiet_for);
        let later = c.yield_signals("run-s", now + Duration::from_millis(600));
        assert!(later.quiet_for >= Duration::from_millis(590), "the quiet time is measured to the poll: {:?}", later.quiet_for);
    }

    /// `send_keys` may type into the agent's own command on top, while it runs. Each other case is its own refusal.
    #[test]
    fn keys_target_takes_only_the_agents_running_command_on_top() {
        let mut idle = core();
        idle.lifecycle = Lifecycle::Prompt;
        assert_eq!(idle.keys_target(None, None), Err(KeysRefusal::NothingRunning), "at a prompt");

        let mut c = core();
        agent_command_open(&mut c, "run-k", "cat");
        let id = c.open_agent_records()[0].id;
        assert_eq!(c.keys_target(None, None), Ok(id));
        assert_eq!(c.keys_target(Some(id), Some(LOCAL_KEY)), Ok(id), "the named id and the approved shell match");
        assert_eq!(c.keys_target(Some(id + 1), None), Err(KeysRefusal::WrongCommand { wanted: Some(id + 1), open: id }));
        assert_eq!(c.keys_target(None, Some(LOCAL_KEY + 5)), Err(KeysRefusal::Location), "another shell is on top");

        let mut user = core();
        user.lifecycle = Lifecycle::Prompt;
        user.handle(Event::CommandLine("cat".into()));
        user.handle(Event::CommandStart);
        assert_eq!(user.keys_target(None, None), Err(KeysRefusal::NotAgents), "the user's own command");

        c.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        assert_eq!(c.keys_target(None, None), Err(KeysRefusal::NoCommand), "after its D there is no open command");
        assert!(c.records.iter().any(|r| r.id == id), "the finished record is kept");
    }

    /// The application cursor mode is read from the screen: DECCKM on turns it on.
    #[test]
    fn the_application_cursor_mode_follows_the_screen() {
        let mut c = core();
        assert!(!c.application_cursor());
        c.apply(ObsMsg::Bytes(b"\x1b[?1h".to_vec()));
        assert!(c.application_cursor());
        c.apply(ObsMsg::Bytes(b"\x1b[?1l".to_vec()));
        assert!(!c.application_cursor());
    }

    /// Output that came before the command's C does not count as output since its start.
    #[test]
    fn output_before_the_command_start_is_not_output_since_start() {
        let mut c = core();
        c.apply(ObsMsg::Bytes(b"banner\r\n".to_vec()));
        c.note_screen();
        agent_command_open(&mut c, "run-t", "sleep 9");
        assert!(!c.yield_signals("run-t", Instant::now()).output_since_start);
        c.apply(ObsMsg::Bytes(b"tick".to_vec()));
        assert!(c.yield_signals("run-t", Instant::now()).output_since_start);
    }

    /// The login test of the foreground: a login command with nothing open is not one, and a command that is not a login is
    /// not one even while it runs.
    #[test]
    fn foreground_is_login_follows_the_open_record_only_while_running() {
        assert!(running("zsh -f -i").foreground_is_login());
        assert!(running("ssh ubuntu@lidia").foreground_is_login());
        assert!(!running("ssh ubuntu@lidia make").foreground_is_login(), "ssh with a remote command");
        assert!(!running("sleep 30").foreground_is_login());
        let mut prompt = core();
        prompt.lifecycle = Lifecycle::Prompt;
        assert!(!prompt.foreground_is_login(), "no command open");
        let mut ended = running("zsh -f -i");
        ended.handle(Event::CommandEnd { exit: 0, pipestatus: vec![0] });
        assert!(!ended.foreground_is_login(), "closed by D");
    }

    /// The agent's open command is found while it runs, through the alternate screen, and while a nested shell is on top of
    /// it (parked). Its call status carries the record id under the same lock.
    #[test]
    fn open_agent_records_follow_the_command_into_alt_and_a_nested_shell() {
        let mut c = core();
        c.lifecycle = Lifecycle::Prompt;
        c.begin_submit(Some((agent("run-9"), Some("ssh h".into()))));
        c.handle(Event::CommandLine("ssh h".into()));
        c.handle(Event::CommandStart);
        let open = c.open_agent_records();
        assert_eq!(open.len(), 1, "{open:?}");
        assert_eq!((open[0].id, open[0].call_id.as_str(), open[0].on_top), (1, "run-9", true));
        assert!(matches!(c.call_status("run-9", WAIT), CallStatus::Pending { state: InputState::Running, record_id: Some(1) }));

        c.feed(b"\x1b[?1049h");
        assert_eq!(c.lifecycle_view(), InputState::Alt);
        assert_eq!(c.open_agent_records().len(), 1, "the record survives the alternate screen");
        c.feed(b"\x1b[?1049l");

        c.expect_level(CHILD, "ssh h");
        c.feed(&tagged("633;Hello", CHILD, ";v=1;sh=bash;user=u;host=h;pid=77"));
        assert_eq!(c.top_key(), 1);
        let parked = c.open_agent_records();
        assert_eq!(parked.len(), 1, "{parked:?}");
        assert!(!parked[0].on_top, "the record is parked under the nested shell");
        assert!(c.record_is_open(1));
        assert!(!c.has_finished_record(1));
    }

    /// Throughput of the observer path (`Core::apply`) on ~50 MB of `seq` output, in four settings:
    /// ring off (no ring, no agent record); ring gated off (the ring is on but no agent record is open, a user's command
    /// flood: it should match ring off); ring on (an agent's command open, so every scrolled row is recorded); and agent
    /// record with the ring off, which shows the open record's own rendering apart from the ring's.
    /// Run it with `cargo test --lib bench_history_ring_cost -- --ignored --nocapture`. Prints MB/s for each setting.
    #[test]
    #[ignore]
    fn bench_history_ring_cost() {
        use std::io::Write;
        let mut data: Vec<u8> = Vec::with_capacity(50 << 20);
        let mut n = 1u64;
        while data.len() < 50 << 20 {
            write!(data, "{n}\r\n").expect("in-memory write");
            n += 1;
        }
        let run = |ring: usize, agent: bool| -> f64 {
            let mut c = Core::new("bench-nonce", (30, 120));
            c.parser.callbacks_mut().ring_capacity = ring;
            if agent {
                agent_command_open(&mut c, "bench", "seq 1 50000000");
            } else {
                c.lifecycle = Lifecycle::Prompt;
                c.handle(Event::CommandStart);
            }
            let start = Instant::now();
            for chunk in data.chunks(64 << 10) {
                c.apply(ObsMsg::Bytes(chunk.to_vec()));
            }
            let secs = start.elapsed().as_secs_f64();
            (data.len() as f64 / (1 << 20) as f64) / secs
        };
        for round in 1..=2 {
            let off = run(0, false);
            let gated_off = run(HISTORY_ROWS, false);
            let on = run(HISTORY_ROWS, true);
            let record_only = run(0, true);
            println!(
                "round {round}: ring off {off:.1} MB/s; ring gated off (user flood) {gated_off:.1} MB/s ({:+.1}% vs off); \
                 ring on (agent flood) {on:.1} MB/s ({:+.1}% vs off); agent record with ring off {record_only:.1} MB/s",
                (gated_off - off) / off * 100.0,
                (on - off) / off * 100.0,
            );
        }
    }
}
