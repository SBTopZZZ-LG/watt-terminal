//! The observer: the agent's view of the session.
//!
//! The PTY pump never waits for this module. It pushes bytes into `ObsQueue`, which is byte-bounded.
//! When the queue would overflow, the queued bytes are dropped and a `Resync` marker takes their place.
//! The observer then rebuilds its parser, reports `Desynced`, and trusts events again at the next
//! prompt. Terminal output is unaffected: the renderer receives every byte separately.

use std::collections::VecDeque;
use std::num::NonZeroU16;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use vt100::capture::{basic_formatted_to_plain, BasicFormattedCaptureState, RowContents};
use vt100::{Callbacks, Parser};

use crate::core::record::{Cwd, OutputBuilder, RawCapture, Record, RecordStatus, RecordSummary, Source};
use crate::core::scanner::{Event, Found, Scanner};

/// Bytes the observer may hold before it desyncs. Matches the design in plan.md.
pub const OBSERVER_QUEUE_CAP: usize = 4 << 20;
/// No integration event within this long after spawn means the shell is not observable (`Raw`).
pub const INTEGRATION_TIMEOUT: Duration = Duration::from_secs(5);
/// A submitted line that never reaches C is released after this long, so the gate cannot stick.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_millis(1500);

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

/// Session changes for listeners (the UI). Delivered in order, outside the core lock.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    State { state: InputState },
    Integration { status: IntegrationStatus },
    Observer { status: ObserverStatus },
    Cwd { cwd: Option<Cwd> },
    RecordStarted { id: u64, command: String, source: Source },
    RecordFinished { record: RecordSummary },
}

pub trait EventSink: Send + Sync {
    fn event(&self, ev: SessionEvent);
}

/// The last values reported, so state and status events fire only on change.
#[derive(Default)]
struct Reported {
    state: Option<InputState>,
    integration: Option<IntegrationStatus>,
    observer: Option<ObserverStatus>,
    cwd: Option<Cwd>,
}

// ---------- vt100 callbacks ----------

/// Owned by the vt100 parser. Counts rows that scroll off the normal screen, and renders the open record's output as it goes.
#[derive(Default)]
pub struct Collector {
    /// Normal-screen rows scrolled off since the parser was created. Row N of the scrollback has absolute index N.
    scrolled_total: u64,
    open: Option<OpenRecord>,
}

impl Callbacks for Collector {
    fn on_scroll(&mut self, contents: RowContents<'_>, alternate_screen: bool) {
        if alternate_screen {
            return;
        }
        let index = self.scrolled_total;
        self.scrolled_total += 1;
        if let Some(open) = self.open.as_mut()
            && index >= open.start_abs
            && !open.builder.is_full()
        {
            let (text, wrapped) = row_text(&contents);
            open.builder.push_row(&text, wrapped);
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
}

// ---------- observer core ----------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lifecycle {
    Booting,
    Prompt,
    Submitted { since: Instant },
    Running,
    Continuation,
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN)
}

fn new_parser(size: (u16, u16)) -> Parser<Collector> {
    Parser::new_with_callbacks(nz(size.0), nz(size.1), 0, Collector::default())
}

pub struct Core {
    parser: Parser<Collector>,
    size: (u16, u16),
    scanner: Scanner,
    lifecycle: Lifecycle,
    spawned_at: Instant,
    shell_pid: Option<u32>,
    desynced: bool,
    eof: bool,
    /// Set by a submit or a user CR. Matched against the next E event to attribute the command. A None text matches
    /// any command: that is a line the user typed in the terminal, whose text Rust never sees.
    pending: Option<(Source, Option<String>)>,
    /// E seen, waiting for its C.
    last_e: Option<(String, Source)>,
    /// The user has typed text that may be on the prompt line. Set by `note_user_bytes`, which counts typing at the
    /// prompt and type-ahead typed while a command runs (zsh reads that from the tty at the next prompt). Cleared by
    /// Ctrl-C at the prompt (not while a command runs), by a CR that submits the line (`begin_submit`), and by C. It
    /// survives the transition from a command to the prompt. While it is set the agent's run_command is refused, so it
    /// cannot land on the user's text.
    user_typed: bool,
    cwd: Option<Cwd>,
    records: VecDeque<Record>,
    next_id: u64,
    /// Events raised since the last `take_events`.
    events: Vec<SessionEvent>,
    reported: Reported,
}

/// Keeps at most this many completed records in memory. The oldest are dropped first.
pub const MAX_RECORDS: usize = 1000;

impl Core {
    pub fn new(nonce: &str, size: (u16, u16)) -> Self {
        Core {
            parser: new_parser(size),
            size,
            scanner: Scanner::new(nonce),
            lifecycle: Lifecycle::Booting,
            spawned_at: Instant::now(),
            shell_pid: None,
            desynced: false,
            eof: false,
            pending: None,
            last_e: None,
            user_typed: false,
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
        if !deliver {
            self.events.clear();
            return Vec::new();
        }
        let mut out = std::mem::take(&mut self.events);
        let state = self.lifecycle_view();
        if self.reported.state != Some(state) {
            self.reported.state = Some(state);
            out.push(SessionEvent::State { state });
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
        if self.reported.cwd != self.cwd {
            self.reported.cwd = self.cwd.clone();
            out.push(SessionEvent::Cwd { cwd: self.cwd.clone() });
        }
        out
    }

    pub fn apply(&mut self, msg: ObsMsg) {
        match msg {
            ObsMsg::Bytes(bytes) => self.feed(&bytes),
            ObsMsg::Resize { rows, cols } => self.resize(rows, cols),
            ObsMsg::Resync => self.resync(),
            ObsMsg::Eof => {
                // The shell is gone, so a command still open will never get its D. It ends here, incomplete.
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
            self.handle(f.event);
        }
        self.process_segment(&bytes[pos..]);
    }

    fn process_segment(&mut self, seg: &[u8]) {
        if seg.is_empty() {
            return;
        }
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
            Event::Hello { pid } => self.shell_pid = Some(pid),
            Event::PromptStart => {
                self.desynced = false;
                self.close_open(RecordStatus::Incomplete);
                self.last_e = None;
                match self.lifecycle {
                    // A redraw of the prompt right after a submit must not open the gate before C.
                    Lifecycle::Submitted { .. } => {}
                    Lifecycle::Booting if self.shell_pid.is_none() => {}
                    _ => {
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
                    self.lifecycle = Lifecycle::Continuation;
                }
            }
            Event::CommandLine(text) => {
                let source = match self.pending.take() {
                    Some((source, None)) => source,
                    Some((source, Some(pending_text))) if pending_text == text => source,
                    _ => Source::Unknown,
                };
                self.last_e = Some((text, source));
            }
            Event::CommandStart => {
                // C while a record is open: the previous command ended without a D we could trust.
                self.close_open(RecordStatus::Incomplete);
                let (command, source) = self.last_e.take().unwrap_or_else(|| (String::new(), Source::Unknown));
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
        self.parser.set_size(nz(size.0), nz(size.1));
        self.size = size;
    }

    /// Bytes were dropped. Forget the open record (incomplete), start a fresh parser, and wait for a prompt.
    fn resync(&mut self) {
        self.close_open(RecordStatus::Incomplete);
        self.parser = new_parser(self.size);
        self.scanner.reset();
        self.desynced = true;
        self.last_e = None;
        self.pending = None;
    }

    fn start_record(&mut self, command: String, source: Source) {
        let alt = self.parser.screen().alternate_screen();
        let (crow, _) = self.parser.screen().cursor_position();
        let id = self.next_id;
        self.next_id += 1;
        let cwd = self.cwd.clone();
        let shell_pid = self.shell_pid;
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
        let OpenRecord { mut record, mut builder, raw, start_abs, used_alt, approximate } = open;
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
            Lifecycle::Submitted { since } if since.elapsed() > SUBMIT_TIMEOUT => InputState::Prompt,
            Lifecycle::Submitted { .. } => InputState::Submitted,
            Lifecycle::Running => InputState::Running,
            Lifecycle::Continuation => InputState::Continuation,
        }
    }

    // TODO: `exec zsh` and nested shells are undetected. They have no AGENTTERM_NONCE, so they emit no events:
    // this stays Live and the lifecycle stays Running until the session exits. Needs an "integration lost" status.
    pub fn integration_status(&self) -> IntegrationStatus {
        if self.lifecycle != Lifecycle::Booting {
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
    pub fn note_user_bytes(&mut self, bytes: &[u8]) {
        let state = self.lifecycle_view();
        if !matches!(state, InputState::Prompt | InputState::Submitted | InputState::Continuation | InputState::Running) {
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
        self.pending = Some((Source::User, None));
    }

    pub fn forged_events(&self) -> u64 {
        self.scanner.forged_events()
    }

    pub fn cwd(&self) -> Option<Cwd> {
        self.cwd.clone()
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
        if let Some(msg) = msg {
            shared.lock().apply(msg);
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
        assert!(typed(&|c| c.lifecycle = Lifecycle::Continuation));
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
}
