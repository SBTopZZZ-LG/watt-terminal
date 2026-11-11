//! The agent's tools: plain Rust methods over a session, plus JSON schemas so a model can call them later.
//!
//! Read tools are always available. `run_command` is the only tool that writes to the shell, and it is
//! refused unless the session is at an idle prompt with shell integration live. `send_keys` is a stub.
//! Approval is not checked here; `agent::AgentCtx::call_tool` asks for it before calling these methods.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::core::foreground::{Foreground, ForegroundKind};
use crate::core::llm::TurnCancel;
use crate::core::observer::{CallStatus, InjectRefusal, InputState, IntegrationStatus, LevelKind, ObserverStatus, OpenAgentRecord};
use crate::core::record::{Cwd, Record, RecordSummary, Source};
use crate::core::session::{user_text_refusal, IntegrateOutcome, Session};

/// The wait `run_command` gives a command when the model gives no `wait_seconds`. A command still running at its end is
/// returned as `RunOutcome::Running`, and the shell keeps it.
pub const DEFAULT_RUN_WAIT_SECONDS: u64 = 5;
pub const DEFAULT_RUN_WAIT: Duration = Duration::from_secs(DEFAULT_RUN_WAIT_SECONDS);
/// The most logical lines `get_screen` returns with `lines`.
pub const MAX_SCREEN_LINES: usize = 200;
/// The refusal for `request_integration` while the shell in the foreground is still starting (see `Core::integration_starting`).
const STARTING_REFUSAL: &str = "cannot integrate the shell yet: it is still starting (it has printed nothing since its command began, or its \
     screen is still changing). Wait a few seconds, or use get_screen to see what it shows, then call request_integration again.";
/// The last non-blank screen rows a running `run_command` result carries.
const SCREEN_TAIL_ROWS: usize = 20;
/// How long zsh must sit at a continuation prompt (PS2) after a `run_command` line before the call returns
/// `NotRun { WaitingForInput }`. Short enough to report promptly, long enough that a multi-line command passing
/// through PS2 for a moment is not cut off.
pub const WAITING_FOR_INPUT_AFTER: Duration = Duration::from_secs(2);
/// The longest one `wait` call may last, and the length when the model gives no `seconds`.
pub const MAX_WAIT_SECONDS: u64 = 120;
pub const DEFAULT_WAIT_SECONDS: u64 = 10;
/// The waiting one turn may do in total. Past it, `wait` returns an error without waiting.
pub const MAX_WAIT_PER_TURN: Duration = Duration::from_secs(30 * 60);
/// After a screen change, `wait` returns once the screen has been quiet for `WAIT_QUIET`, or `WAIT_BURST` after the
/// first change, whichever comes first. So the snapshot is not a frame that is still being drawn.
const WAIT_QUIET: Duration = Duration::from_millis(300);
const WAIT_BURST: Duration = Duration::from_millis(1500);
const RUN_POLL: Duration = Duration::from_millis(25);
/// After a command's D, the shell's prompt (A) can take a while: the user's precmd hooks run first. `run_command` waits up
/// to this long for the prompt before it returns, so a back-to-back call is not refused in that gap.
const PROMPT_SETTLE: Duration = Duration::from_secs(3);
/// The refusal for a `run_command` while the nested shell has not answered a submitted line (see `InputState::Unresponsive`).
pub const UNRESPONSIVE_REFUSAL: &str = "cannot run a command now: the shell is not answering (it may be busy, or the connection dropped); use get_screen, or ask the user";
/// Added to a nested `run_command` result that may be waiting for a password or other input. The agent must not type it.
pub const INPUT_WAIT_NOTE: &str = "The shell appears to be waiting for a password or other input. Do not type it. Ask the user to handle it, then use get_screen.";
/// The next step, when the foreground is a shell or program without integration. Used by the refusal, the screen and the note.
pub const INTEGRATE_HINT: &str = "Call request_integration to ask for it.";
/// The refusal for `request_integration` while the shell waits for a password or passphrase: the loader must not be typed there.
pub const INTEGRATE_PASSWORD_REFUSAL: &str = "cannot integrate the shell now: it appears to be waiting for a password or other input. Do not type it. Ask the user to handle it, then call request_integration again.";
const DEFAULT_LIST: usize = 10;
const DEFAULT_LIMIT: usize = 4096;
const MAX_LIMIT: usize = 64 << 10;

#[derive(Debug, Clone, Serialize)]
pub struct ScreenView {
    pub rows: Vec<String>,
    /// (row, col), zero-based.
    pub cursor: (u16, u16),
    pub alternate_screen: bool,
    /// (rows, cols)
    pub size: (u16, u16),
    pub state: InputState,
    pub observer_status: ObserverStatus,
    pub integration_status: IntegrationStatus,
    /// The foreground program hint, while a command runs (core/foreground.rs). Left out when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground: Option<Foreground>,
    /// The label of the nested shell on top (the command that started it). Left out on the local shell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Set only by `get_screen` with `lines`: `rows` are then logical lines (see `HistoryView`). Left out otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryView>,
}

/// What `get_screen` with `lines` says about the rows it returned.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryView {
    /// How many logical lines `rows` has.
    pub lines: usize,
    /// True when older lines were left out to stay under the byte cap.
    pub truncated: bool,
    /// True on the alternate screen, which has no history: `rows` are the visible rows.
    pub no_history: bool,
}

/// The shell the agent acts in: the top level of the session. `key` is the level's key (0 is the local shell), which an
/// approval is bound to: a command approved for one key is refused if another shell is on top when it is submitted. A
/// nested level has a `location` (the command that started it) and the user, host and shell its Hello gave. The local
/// shell has none of those.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopLevel {
    pub key: u64,
    pub location: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
    pub sh: Option<String>,
}

impl TopLevel {
    /// True when the top shell is a nested level, not the local machine's shell.
    pub fn is_nested(&self) -> bool {
        self.location.is_some()
    }
}

/// One page of a command's rendered output. `offset` and `limit` are byte counts, moved to character boundaries.
#[derive(Debug, Clone, Serialize)]
pub struct CommandView {
    #[serde(flatten)]
    pub record: RecordSummary,
    pub cwd: Option<Cwd>,
    pub output: String,
    pub offset: usize,
    pub total_bytes: usize,
    /// True when output continues after this page.
    pub truncated: bool,
}

/// Why a submitted line never ran, when the call returns without a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotRunReason {
    /// The line was cancelled (Ctrl-C back to the prompt) or the user finished it themselves. It will not run.
    Abandoned,
    /// zsh has been waiting for the rest of the line for `WAITING_FOR_INPUT_AFTER`. Nothing was sent to cancel it.
    WaitingForInput,
    /// The turn was cancelled while the call waited. Nothing is sent to the shell to stop it: a line that was already
    /// sent keeps running, so the command may still be running; the user can Ctrl-C it in the terminal.
    Cancelled,
    /// The turn was cancelled before the line was sent. Nothing was written to the shell, so nothing is running.
    CancelledBeforeSend,
}

/// Why a `run_command` call returned while its command was still going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum YieldReason {
    /// The wait ran out while the command had not finished.
    Timeout,
}

// `Finished` carries a whole Record. Boxing it would change the public RunOutcome type, so it stays inline.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    /// The command finished within the wait.
    Finished { record: Record },
    /// The command had not finished when the wait ended. The shell keeps running it; nothing is sent to stop it.
    /// `record_id` is the command's id (the handle for `wait` and `get_command`), or null when the shell has not started
    /// the command yet. `state` is the shell state at that moment: only a command that has started is still running.
    /// `screen_tail` is the last non-blank rows of the screen, and `hint` says what to do next.
    Running {
        reason: YieldReason,
        record_id: Option<u64>,
        call_id: String,
        state: InputState,
        /// The foreground program hint (see `ScreenView::foreground`). Left out when there is none.
        #[serde(skip_serializing_if = "Option::is_none")]
        foreground: Option<Foreground>,
        waited_ms: u64,
        screen_tail: Vec<String>,
        hint: String,
    },
    /// The line did not run, or is stuck waiting for input. The shell is not touched; the caller decides what to do.
    NotRun { reason: NotRunReason },
}

/// Why a `wait` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    /// The shell was at an idle prompt when the wait began, so there was nothing to wait for.
    Idle,
    /// The session exited, when the wait began or while it ran.
    SessionExited,
    /// A command finished while the wait ran. `record` carries it.
    CommandFinished,
    /// The visible screen changed (not reported with `ignore_updates`).
    ScreenChanged,
    /// The time ran out and nothing above happened.
    Timeout,
    /// The turn was cancelled.
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaitView {
    pub reason: WaitReason,
    pub waited_ms: u64,
    pub state: InputState,
    pub screen: ScreenView,
    /// The command that finished, for `CommandFinished`. Serialized like `RunOutcome::Finished`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

/// What the agent may do. Methods return plain data; `call` takes and returns JSON.
pub trait AgentTools {
    /// The last `n` finished commands, oldest first.
    fn list_commands(&self, n: usize) -> Vec<RecordSummary>;
    fn get_command(&self, id: u64, offset: usize, limit: usize) -> Result<CommandView, String>;
    fn get_screen(&self) -> ScreenView;
    /// The screen with its last `lines` logical lines of history (see `HistoryView`). Defaults to the visible screen, so a
    /// fake needs no change.
    fn get_screen_lines(&self, _lines: usize) -> ScreenView {
        self.get_screen()
    }
    /// The agent's own commands still open (see `OpenAgentRecord`). Read per model call. Empty by default.
    fn open_agent_records(&self) -> Vec<OpenAgentRecord> {
        Vec::new()
    }
    /// The summary of a finished command, by id. None when there is no such finished command. Defaults to None.
    fn finished_summary(&self, _id: u64) -> Option<RecordSummary> {
        None
    }
    fn get_cwd(&self) -> Option<Cwd>;
    /// The foreground program hint, while a command runs (core/foreground.rs). A hint only, never a safety decision.
    /// Defaults to None, so a fake without a shell needs no change.
    fn foreground(&self) -> Option<Foreground> {
        None
    }
    /// The shell on top (see `TopLevel`). Defaults to the local shell, so a fake without a shell needs no change.
    fn top_level(&self) -> TopLevel {
        TopLevel::default()
    }
    /// Runs `command` in the shell on top, and waits up to `wait` for it to finish. A command still running at the end of the
    /// wait is returned as `RunOutcome::Running`; it is not stopped. `expected_level` is the key the call was approved for:
    /// when another shell is on top by the time it is submitted, nothing is written. None means no check (tests and fakes).
    fn run_command(&self, command: &str, wait: Duration, expected_level: Option<u64>) -> Result<RunOutcome, String>;
    /// Waits up to `seconds` (already clamped to 1..=MAX_WAIT_SECONDS) for the shell to do something worth reporting. See
    /// `WaitReason`. With `record`, the wait is keyed to that command: it ends when that command finishes. Counts against
    /// the turn's wait budget, not the command limit.
    fn wait(&self, seconds: u64, ignore_updates: bool, record: Option<u64>) -> Result<WaitView, String>;
    /// Whether `run_command` would already be refused for text the user typed on the prompt. Checked before the approval
    /// card, so the user is not asked to approve a command that will be refused. Writes nothing.
    fn run_command_precheck(&self) -> Result<(), String> {
        Ok(())
    }
    /// Checks that `request_integration` may start now, and returns the label of the shell it would integrate (the command
    /// that holds the foreground). The error is the text the model reads. Writes nothing. A fake without a shell refuses.
    fn integration_precheck(&self) -> Result<String, String> {
        Err("shell integration is not available here".into())
    }
    /// Integrates the shell in the foreground (see `Session::integrate_shell`). Blocking. A fake without a shell fails.
    fn integrate_shell(&self) -> IntegrateOutcome {
        IntegrateOutcome::Failed { hint: "shell integration is not available here".into() }
    }
    fn send_keys(&self, bytes: &[u8]) -> Result<(), String>;

    /// Calls a tool by schema name. Optional arguments take their defaults. Does not ask for approval. `expected_level` is
    /// passed to `run_command` (see there).
    fn call(&self, name: &str, args: &Value, expected_level: Option<u64>) -> Result<Value, String> {
        match name {
            "list_commands" => to_json(self.list_commands(usize_arg(args, "n", DEFAULT_LIST)?)),
            "get_command" => {
                let id = u64_arg(args, "id")?;
                let offset = usize_arg(args, "offset", 0)?;
                let limit = usize_arg(args, "limit", DEFAULT_LIMIT)?.min(MAX_LIMIT);
                to_json(self.get_command(id, offset, limit)?)
            }
            "get_screen" => match lines_arg(args)? {
                None => to_json(self.get_screen()),
                Some((lines, note)) => {
                    let mut value = to_json(self.get_screen_lines(lines))?;
                    if let Some(note) = note {
                        value["note"] = json!(note);
                    }
                    Ok(value)
                }
            },
            "get_cwd" => to_json(self.get_cwd()),
            "wait" => {
                let (seconds, note) = wait_seconds_arg(args);
                let ignore_updates = bool_arg(args, "ignore_updates", false)?;
                let record = opt_u64_arg(args, "id")?;
                let mut value = to_json(self.wait(seconds, ignore_updates, record)?)?;
                if let Some(note) = note {
                    value["note"] = json!(note);
                }
                Ok(value)
            }
            "run_command" => {
                let command = str_arg(args, "command")?;
                let (seconds, clamp_note) = seconds_arg(args, "wait_seconds", DEFAULT_RUN_WAIT_SECONDS);
                let outcome = self.run_command(command, Duration::from_secs(seconds), expected_level)?;
                // In a nested shell, a call that is still running or sits at a password prompt gets the do-not-type note.
                let running = matches!(outcome, RunOutcome::Running { .. });
                let mut value = to_json(outcome)?;
                let mut notes: Vec<String> = clamp_note.into_iter().collect();
                if self.top_level().is_nested() && (running || screen_waits_for_input(&self.get_screen().rows)) {
                    notes.push(INPUT_WAIT_NOTE.to_string());
                }
                if !notes.is_empty() {
                    value["note"] = json!(notes.join(" "));
                }
                Ok(value)
            }
            "send_keys" => {
                let bytes: Vec<u8> = serde_json::from_value(args.get("bytes").cloned().unwrap_or(Value::Null))
                    .map_err(|_| "argument `bytes` must be an array of integers 0-255".to_string())?;
                self.send_keys(&bytes)?;
                Ok(json!({"ok": true}))
            }
            other => Err(format!("unknown tool `{other}`")),
        }
    }
}

/// The tools over one live session. The agent makes one per turn, so the wait budget below is per turn.
pub struct SessionTools<'a> {
    session: &'a Session,
    /// The continuation wait `run_command` uses. `WAITING_FOR_INPUT_AFTER` unless a test overrides it.
    input_wait: Duration,
    /// When set, `run_command` checks it while it waits and before it sends anything.
    cancel: Option<Arc<TurnCancel>>,
    /// The most `wait` may spend through this value. `MAX_WAIT_PER_TURN` unless a test overrides it.
    wait_budget: Duration,
    /// Time spent in `wait` through this value so far.
    wait_used: Cell<Duration>,
}

impl<'a> SessionTools<'a> {
    pub fn new(session: &'a Session) -> Self {
        SessionTools::with_input_wait(session, WAITING_FOR_INPUT_AFTER)
    }

    /// Like `new`, with a different continuation wait. For tests.
    pub fn with_input_wait(session: &'a Session, input_wait: Duration) -> Self {
        SessionTools {
            session,
            input_wait,
            cancel: None,
            wait_budget: MAX_WAIT_PER_TURN,
            wait_used: Cell::new(Duration::ZERO),
        }
    }

    /// Like `new`, with a different per-turn wait budget. For tests.
    pub fn with_wait_budget(self, budget: Duration) -> Self {
        SessionTools { wait_budget: budget, ..self }
    }

    /// Lets the turn's cancel stop `run_command`'s wait. A cancelled call returns `NotRun { Cancelled }` within one poll.
    /// It never sends a command that has not been sent yet, and it does not stop one that was: nothing is written to the
    /// shell for a cancel.
    pub fn with_cancel(self, cancel: Arc<TurnCancel>) -> Self {
        SessionTools { cancel: Some(cancel), ..self }
    }

    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.is_cancelled())
    }
}

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

impl AgentTools for SessionTools<'_> {
    fn list_commands(&self, n: usize) -> Vec<RecordSummary> {
        let all = self.session.records();
        all[all.len().saturating_sub(n)..].to_vec()
    }

    fn get_command(&self, id: u64, offset: usize, limit: usize) -> Result<CommandView, String> {
        let r = self.session.get_record(id).ok_or_else(|| format!("no finished command with id {id}"))?;
        let total = r.output.len();
        let start = char_floor(&r.output, offset.min(total));
        let mut end = char_floor(&r.output, start.saturating_add(limit).min(total));
        if end == start && start < total {
            // A limit smaller than one character still makes progress, so paging cannot get stuck.
            end = start + r.output[start..].chars().next().map_or(0, char::len_utf8);
        }
        Ok(CommandView {
            record: RecordSummary::from(&r),
            cwd: r.cwd.clone(),
            output: r.output[start..end].to_string(),
            offset: start,
            total_bytes: total,
            truncated: end < total,
        })
    }

    fn get_screen(&self) -> ScreenView {
        let snap = self.session.screen_snapshot();
        ScreenView {
            rows: snap.rows,
            cursor: snap.cursor,
            alternate_screen: snap.alternate_screen,
            size: snap.size,
            state: self.session.state(),
            observer_status: self.session.observer_status(),
            integration_status: self.session.integration_status(),
            foreground: self.session.foreground(),
            location: self.session.location(),
            history: None,
        }
    }

    fn get_screen_lines(&self, lines: usize) -> ScreenView {
        let history = self.session.screen_lines(lines);
        let mut screen = self.get_screen();
        screen.history = Some(HistoryView {
            lines: history.rows.len(),
            truncated: history.truncated,
            no_history: history.no_history,
        });
        screen.rows = history.rows;
        screen
    }

    fn open_agent_records(&self) -> Vec<OpenAgentRecord> {
        self.session.open_agent_records()
    }

    fn finished_summary(&self, id: u64) -> Option<RecordSummary> {
        self.session.record_summary(id)
    }

    fn get_cwd(&self) -> Option<Cwd> {
        self.session.cwd()
    }

    fn foreground(&self) -> Option<Foreground> {
        self.session.foreground()
    }

    fn top_level(&self) -> TopLevel {
        let levels = self.session.levels();
        let Some(top) = levels.last() else {
            return TopLevel::default();
        };
        let nested = top.kind == LevelKind::Remote;
        TopLevel {
            key: top.key,
            location: nested.then(|| top.label.clone()),
            user: top.user.clone(),
            host: top.host.clone(),
            sh: top.sh.clone(),
        }
    }

    fn run_command(&self, command: &str, wait: Duration, expected_level: Option<u64>) -> Result<RunOutcome, String> {
        if command.trim().is_empty() {
            return Err("command is empty".into());
        }
        // A CR or ESC inside the text would submit or escape early, so only plain newlines and tabs are allowed.
        if let Some(c) = command.chars().find(|c| c.is_control() && *c != '\n' && *c != '\t') {
            return Err(format!("command contains the control character {c:?}, which cannot be sent"));
        }
        if let Some(refusal) = self.not_at_prompt() {
            return Err(refusal);
        }
        if self.cancelled() {
            return Ok(RunOutcome::NotRun { reason: NotRunReason::CancelledBeforeSend });
        }
        let call_id = format!("run-{}", NEXT_RUN.fetch_add(1, Ordering::Relaxed));
        // submit() re-checks the prompt, and the user's text, atomically, so a racing user submit or keystroke wins or
        // loses cleanly.
        self.session
            .submit_bound(Source::Agent { call_id: call_id.clone() }, command, expected_level)
            .map_err(|e| e.to_string())?;

        let started = Instant::now();
        let deadline = started + wait;
        loop {
            // One lock read per poll: the outcome, the shell state and the record id come from the same moment.
            match self.session.call_status(&call_id, self.input_wait) {
                CallStatus::Finished(record) => {
                    self.settle_at_prompt();
                    return Ok(RunOutcome::Finished { record: *record });
                }
                CallStatus::Abandoned => return Ok(RunOutcome::NotRun { reason: NotRunReason::Abandoned }),
                CallStatus::WaitingForInput => return Ok(RunOutcome::NotRun { reason: NotRunReason::WaitingForInput }),
                CallStatus::Pending { state, record_id } => {
                    if self.cancelled() {
                        return Ok(RunOutcome::NotRun { reason: NotRunReason::Cancelled });
                    }
                    if Instant::now() >= deadline {
                        return Ok(self.yielded(&call_id, state, record_id, started.elapsed()));
                    }
                }
            }
            thread::sleep(RUN_POLL);
        }
    }

    fn wait(&self, seconds: u64, ignore_updates: bool, record: Option<u64>) -> Result<WaitView, String> {
        if let Some(id) = record
            && !self.session.has_finished_record(id)
            && !self.session.record_is_open(id)
        {
            return Err(format!("no command with id {id} is running or finished in this session"));
        }
        let used = self.wait_used.get();
        if used >= self.wait_budget {
            return Err(format!(
                "wait budget for this turn is used up ({} s of waiting); stop waiting and report what you have",
                used.as_secs()
            ));
        }
        let started = Instant::now();
        let view = self.wait_for(seconds, ignore_updates, record, started);
        self.wait_used.set(used + started.elapsed());
        Ok(view)
    }

    fn run_command_precheck(&self) -> Result<(), String> {
        // The same refusals run_command makes before it writes, first: a call that is doomed gets no approval card.
        if let Some(refusal) = self.not_at_prompt() {
            return Err(refusal);
        }
        if self.session.user_typed_since_prompt() {
            Err(user_text_refusal())
        } else {
            Ok(())
        }
    }

    fn integration_precheck(&self) -> Result<String, String> {
        if let Some(reason) = self.session.integration_refusal() {
            return Err(integration_refusal_text(&reason));
        }
        if screen_waits_for_input(&self.session.screen_snapshot().rows) {
            return Err(INTEGRATE_PASSWORD_REFUSAL.to_string());
        }
        if self.session.integration_starting() {
            return Err(STARTING_REFUSAL.to_string());
        }
        Ok(self.session.foreground().map(|fg| fg.command).unwrap_or_default())
    }

    fn integrate_shell(&self) -> IntegrateOutcome {
        self.session.integrate_shell()
    }

    fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
        Err("send_keys is disabled in slice 1".into())
    }
}

impl SessionTools<'_> {
    /// Waits until the top level is back at its prompt, for at most `PROMPT_SETTLE`. A finished command's D arrives before the
    /// prompt (A), which comes after the user's precmd hooks. Returns at once when the session exits. If the bound passes,
    /// the result is returned anyway; the next call is then refused as before.
    fn settle_at_prompt(&self) {
        let end = Instant::now() + PROMPT_SETTLE;
        loop {
            match self.session.state() {
                InputState::Prompt | InputState::Exited => return,
                _ if Instant::now() >= end => return,
                _ => thread::sleep(RUN_POLL),
            }
        }
    }

    /// Why `run_command` cannot run now, when the shell is not live or not at an idle prompt. None when it may run.
    /// `run_command` and `run_command_precheck` both use it, so the refusals match.
    fn not_at_prompt(&self) -> Option<String> {
        if self.session.integration_status() != IntegrationStatus::Live {
            return Some("shell integration is not live, so commands cannot be run".into());
        }
        let state = self.session.state();
        (state != InputState::Prompt).then(|| {
            // The agent's own command, when it is open: the one on top, else the innermost parked one.
            let agent = self.session.open_agent_records();
            let agent = agent.iter().find(|r| r.on_top).or(agent.last());
            not_at_prompt_text(state, self.session.foreground().as_ref(), agent)
        })
    }

    /// The outcome of a `run_command` whose wait ended with the command still going (or not yet started). `record_id` and
    /// `state` come from the same lock read as the poll that ended the wait.
    fn yielded(&self, call_id: &str, state: InputState, record_id: Option<u64>, waited: Duration) -> RunOutcome {
        let mut tail: Vec<String> = self.session.screen_snapshot().rows.into_iter().filter(|r| !r.trim().is_empty()).collect();
        let keep_from = tail.len().saturating_sub(SCREEN_TAIL_ROWS);
        let screen_tail = tail.split_off(keep_from);
        RunOutcome::Running {
            reason: YieldReason::Timeout,
            record_id,
            call_id: call_id.to_string(),
            state,
            foreground: self.session.foreground(),
            waited_ms: u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
            screen_tail,
            hint: running_hint(record_id),
        }
    }

    /// The wait itself. An idle prompt or an exited session returns at once. Otherwise the first of these ends it:
    /// the turn is cancelled, the session exits, a command finishes, the screen changes (unless `ignore_updates`, and
    /// then after its quiet period), or the time runs out. A command finishes the wait when it is the keyed one: the
    /// one `record` names, else the only agent command open at the start. Without a key, a command finishes it when the
    /// shell is back at its prompt with a newer record.
    fn wait_for(&self, seconds: u64, ignore_updates: bool, record: Option<u64>, started: Instant) -> WaitView {
        let deadline = started + Duration::from_secs(seconds);
        let start = self.session.activity();
        let key = record.or_else(|| {
            let open = self.session.open_agent_records();
            (open.len() == 1).then(|| open[0].id)
        });
        let (reason, finished) = match (key, start.state) {
            (Some(id), _) if self.session.has_finished_record(id) => (WaitReason::CommandFinished, Some(id)),
            (None, InputState::Prompt) => (WaitReason::Idle, None),
            (_, InputState::Exited) => (WaitReason::SessionExited, None),
            _ => {
                // The first and the latest time the screen was seen to change, since the wait began.
                let mut changed: Option<(Instant, Instant)> = None;
                let mut seen_gen = start.screen_gen;
                loop {
                    let now = Instant::now();
                    let act = self.session.activity();
                    if self.cancelled() {
                        break (WaitReason::Cancelled, None);
                    }
                    if act.state == InputState::Exited {
                        break (WaitReason::SessionExited, None);
                    }
                    let finished_key = match key {
                        Some(id) => self.session.has_finished_record(id).then_some(id),
                        None => (act.last_record_id > start.last_record_id && act.state == InputState::Prompt)
                            .then_some(act.last_record_id),
                    };
                    if let Some(id) = finished_key {
                        break (WaitReason::CommandFinished, Some(id));
                    }
                    if act.screen_gen != seen_gen {
                        seen_gen = act.screen_gen;
                        changed = Some(match changed {
                            Some((first, _)) => (first, now),
                            None => (now, now),
                        });
                    }
                    let settled = changed.is_some_and(|(first, last)| {
                        now.duration_since(last) >= WAIT_QUIET || now.duration_since(first) >= WAIT_BURST
                    });
                    if !ignore_updates && settled {
                        break (WaitReason::ScreenChanged, None);
                    }
                    if now >= deadline {
                        // A change seen before the deadline is reported, not lost to the timeout.
                        let reason = if !ignore_updates && changed.is_some() {
                            WaitReason::ScreenChanged
                        } else {
                            WaitReason::Timeout
                        };
                        break (reason, None);
                    }
                    thread::sleep(RUN_POLL);
                }
            }
        };
        if reason == WaitReason::CommandFinished {
            // A keyed command can finish while its shell is still drawing the prompt: settle, so the next run is not refused.
            self.settle_at_prompt();
        }
        let screen = self.get_screen();
        WaitView {
            reason,
            waited_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            state: screen.state,
            screen,
            record: finished.and_then(|id| self.session.get_record(id)),
        }
    }
}

/// The next step for a `run_command` that is still going (`RunOutcome::Running`). With an id the agent can wait for it; before
/// the shell has started the command there is no id yet.
pub fn running_hint(record_id: Option<u64>) -> String {
    match record_id {
        Some(id) => format!(
            "Still running. Call wait with id {id} to wait for it to finish, or get_screen with lines to read more of its output. \
             run_command is unavailable until it ends."
        ),
        None => "The shell has not started the command yet. Call wait to wait for it, or get_screen to see the screen. \
                 run_command is unavailable until the command ends."
            .to_string(),
    }
}

/// How long a command has been running, for the notes and refusals: seconds, then minutes, then hours.
pub fn age_text(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// The refusal for a `run_command` when the shell is not at an idle prompt. The text starts with the plain refusal
/// either way. When the agent's own command is open, the text names its id and says to wait for it. While a command runs
/// and the hint names a program, it says what runs and what the agent can do: read the screen, wait, and ask the user to
/// leave a program without shell integration.
pub fn not_at_prompt_text(state: InputState, foreground: Option<&Foreground>, agent: Option<&OpenAgentRecord>) -> String {
    if state == InputState::Unresponsive {
        return UNRESPONSIVE_REFUSAL.to_string();
    }
    let mut text = format!("cannot run a command now: the session is {state:?}, not at an idle prompt");
    if let Some(rec) = agent {
        text.push_str(&format!(
            ". Your command id {id} `{cmd}` is still running ({age}). Wait for it (wait with id {id}), then run the command again.",
            id = rec.id,
            cmd = rec.command,
            age = age_text(rec.age),
        ));
    }
    let Some(fg) = foreground.filter(|_| state == InputState::Running) else {
        return text;
    };
    match fg.kind {
        // The agent's own ordinary command is named above; it is not named twice.
        ForegroundKind::Other if agent.is_some_and(|rec| rec.on_top) => text,
        ForegroundKind::Other => format!(
            "{text}. The command `{}` is still running in the shell. Wait for it to finish (wait), then run the command again.",
            fg.command
        ),
        kind => format!(
            "{text}. The foreground program is `{}`: {}, without shell integration, so commands cannot run in it. \
             Read it with get_screen and wait for it to end. Ask the user to leave it, so commands can run again. {INTEGRATE_HINT}",
            fg.command,
            kind.describe()
        ),
    }
}

/// The text for a refused `request_integration`: the refusal and what the agent should do next.
pub fn integration_refusal_text(reason: &InjectRefusal) -> String {
    let next = match reason {
        InjectRefusal::NotRunning => {
            "Nothing is running in the shell: ask the user to open the remote shell (for example with ssh), then call request_integration again."
        }
        InjectRefusal::AltScreen => "Ask the user to close the full-screen program, then call request_integration again.",
        InjectRefusal::NoShellProgram => {
            "Only a shell, ssh, su or a container exec can be integrated. Work from get_screen and ask the user."
        }
        InjectRefusal::TooDeep => "Ask the user to leave the nested shell first.",
        InjectRefusal::Busy => "Another integration is in progress: wait a moment, then call request_integration again.",
    };
    format!("cannot integrate the shell now: {reason}. {next}")
}

/// What the model reads for a `request_integration` outcome. An integrated shell gives its text. Every other outcome is an
/// error that says what to do next.
pub fn integration_result(outcome: &IntegrateOutcome) -> Result<Value, String> {
    match outcome {
        IntegrateOutcome::Integrated { label, sh, user, host, .. } => {
            let who = match (user, host) {
                (Some(user), Some(host)) => format!(" as {user}@{host}"),
                _ => String::new(),
            };
            Ok(Value::String(format!("Integrated {label} ({sh}{who}). You can now use run_command in this shell.")))
        }
        IntegrateOutcome::UnsupportedShell => Err(
            "This shell is not supported (needs zsh or bash 4.4+); stay read-only: use get_screen and ask the user.".into(),
        ),
        IntegrateOutcome::Timeout => Err(
            "integration timed out: the shell did not answer the handshake. Ask the user to check that the shell is at its \
             prompt (not at a password or other prompt), then call request_integration again."
                .into(),
        ),
        IntegrateOutcome::Failed { hint } => Err(format!(
            "integration did not complete: {hint}. Ask the user to check that the shell is at its prompt, then call \
             request_integration again."
        )),
        IntegrateOutcome::Refused { reason } => Err(integration_refusal_text(reason)),
    }
}

/// The JSON schemas of every tool, in the form a model's tool-use API expects.
pub fn schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "list_commands",
            description: "List the most recent finished commands in the session, oldest first.",
            parameters: json!({
                "type": "object",
                "properties": {"n": {"type": "integer", "minimum": 0, "default": DEFAULT_LIST}},
            }),
        },
        ToolSchema {
            name: "get_command",
            description: "Get one finished command: its command line, exit code, cwd, and a page of its rendered output. Use offset and limit to page; truncated is true when output continues.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "minimum": 1},
                    "offset": {"type": "integer", "minimum": 0, "default": 0},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT},
                },
                "required": ["id"],
            }),
        },
        ToolSchema {
            name: "get_screen",
            description: "Get the visible screen: rows, cursor, whether the alternate screen is active, and the observer and shell-integration status. With lines, get the last N lines instead, including output that scrolled off the top (soft-wrapped rows joined, up to 12 KB). The alternate screen has no history, so it returns the visible rows and no_history.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "lines": {"type": "integer", "minimum": 1, "maximum": MAX_SCREEN_LINES, "description": "How many of the last logical lines to return."},
                },
            }),
        },
        ToolSchema {
            name: "get_cwd",
            description: "Get the shell's current host and directory, or null if not known yet.",
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSchema {
            name: "wait",
            description: "Wait for the shell instead of polling get_screen. Use it after run_command comes back with the command still running. Pass id (the record_id that run_command gave) to wait for that command: it returns with its record when the command finishes. Returns at once when a command finishes (with its record), and early when the screen changes unless ignore_updates is true; set ignore_updates when only the end of the command matters. Also returns on timeout, and at once when the shell is idle.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECONDS, "default": DEFAULT_WAIT_SECONDS},
                    "ignore_updates": {"type": "boolean", "default": false},
                    "id": {"type": "integer", "minimum": 1, "description": "The record_id of a command to wait for."},
                },
            }),
        },
        ToolSchema {
            name: "run_command",
            description: "Run a command in the shell on top: the user's own shell, or a shell the user opened inside another program (such as ssh) that the app integrated. Only works at an idle prompt with shell integration live. Waits up to wait_seconds (default 5, max 120) for the command to finish. If it is still running then, the result gives its record_id and the command keeps running: use wait with that id, or get_screen with lines, until it ends. run_command is unavailable until it ends. The user approves each call unless a saved rule or auto-approve allows it.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "minLength": 1},
                    "wait_seconds": {"type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECONDS, "default": DEFAULT_RUN_WAIT_SECONDS},
                },
                "required": ["command"],
            }),
        },
        ToolSchema {
            name: "request_integration",
            description: "Ask the user to integrate the shell in the foreground (ssh, a nested shell, su, or a container exec) so that run_command works there. Use it when get_screen shows such a shell without shell integration. The user confirms it in a dialog, unless the user chose to let the app do it. It is refused while the shell waits for a password. After it succeeds, run_command runs commands in that shell, under the approval rules of that shell.",
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSchema {
            name: "send_keys",
            description: "Send raw bytes to the terminal. Disabled in this version; always returns an error.",
            parameters: json!({
                "type": "object",
                "properties": {"bytes": {"type": "array", "items": {"type": "integer", "minimum": 0, "maximum": 255}}},
                "required": ["bytes"],
            }),
        },
    ]
}

/// True when the last non-blank row of a screen looks like a password or passphrase prompt. Case-insensitive.
pub fn screen_waits_for_input(rows: &[String]) -> bool {
    static PROMPT: OnceLock<Regex> = OnceLock::new();
    let prompt = PROMPT.get_or_init(|| {
        Regex::new(r"(?i)password.*:\s*$|\[sudo\] password|passphrase").expect("the prompt pattern compiles")
    });
    rows.iter().rev().find(|r| !r.trim().is_empty()).is_some_and(|row| prompt.is_match(row.trim_end()))
}

fn to_json<T: Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn usize_arg(args: &Value, key: &str, default: usize) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_u64().map(|n| n as usize).ok_or_else(|| format!("argument `{key}` must be a non-negative integer")),
    }
}

fn u64_arg(args: &Value, key: &str) -> Result<u64, String> {
    args.get(key).and_then(Value::as_u64).ok_or_else(|| format!("argument `{key}` must be a non-negative integer"))
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("argument `{key}` must be a string"))
}

fn bool_arg(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_bool().ok_or_else(|| format!("argument `{key}` must be true or false")),
    }
}

/// The `seconds` argument of `wait`, clamped to 1..=MAX_WAIT_SECONDS. A missing or non-integer value is the default.
/// The note says what was clamped, so the model learns how long it actually waited.
fn wait_seconds_arg(args: &Value) -> (u64, Option<String>) {
    seconds_arg(args, "seconds", DEFAULT_WAIT_SECONDS)
}

/// A seconds argument named `key`, clamped to 1..=MAX_WAIT_SECONDS. Missing or non-integer is `default`. The note names the
/// argument and what it was clamped to.
fn seconds_arg(args: &Value, key: &str, default: u64) -> (u64, Option<String>) {
    let requested = args.get(key).and_then(|v| v.as_i64().map(i128::from).or_else(|| v.as_u64().map(i128::from)));
    let Some(requested) = requested else {
        return (default, None);
    };
    let clamped = requested.clamp(1, i128::from(MAX_WAIT_SECONDS));
    let note = (clamped != requested)
        .then(|| format!("{key} {requested} is outside 1 to {MAX_WAIT_SECONDS}; waited {clamped} s instead"));
    (clamped as u64, note)
}

/// The `lines` argument of `get_screen`: None when absent (the visible screen, as before). Otherwise clamped to
/// 1..=MAX_SCREEN_LINES, with a note when it was. A non-integer is an error.
fn lines_arg(args: &Value) -> Result<Option<(usize, Option<String>)>, String> {
    let requested = match args.get("lines") {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v.as_i64().map(i128::from).or_else(|| v.as_u64().map(i128::from)),
    };
    let requested = requested.ok_or_else(|| "argument `lines` must be an integer".to_string())?;
    let clamped = requested.clamp(1, MAX_SCREEN_LINES as i128);
    let note = (clamped != requested)
        .then(|| format!("lines {requested} is outside 1 to {MAX_SCREEN_LINES}; showing {clamped} lines instead"));
    Ok(Some((clamped as usize, note)))
}

/// An optional non-negative integer argument: None when absent or null.
fn opt_u64_arg(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| format!("argument `{key}` must be a non-negative integer")),
    }
}

/// Largest index <= `i` that is a character boundary of `s`.
fn char_floor(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_seconds_are_clamped_and_the_clamp_is_noted() {
        assert_eq!(wait_seconds_arg(&json!({"seconds": 5})), (5, None));
        assert_eq!(wait_seconds_arg(&json!({"seconds": 120})), (120, None));
        assert_eq!(wait_seconds_arg(&json!({})), (DEFAULT_WAIT_SECONDS, None));
        assert_eq!(wait_seconds_arg(&json!({"seconds": "5"})), (DEFAULT_WAIT_SECONDS, None), "a string is not an integer");
        assert_eq!(wait_seconds_arg(&json!({"seconds": 2.5})), (DEFAULT_WAIT_SECONDS, None), "a fraction is not an integer");

        let (high, note) = wait_seconds_arg(&json!({"seconds": 500}));
        assert_eq!(high, MAX_WAIT_SECONDS);
        assert!(note.is_some_and(|n| n.contains("500")));
        let (huge, note) = wait_seconds_arg(&json!({"seconds": u64::MAX}));
        assert_eq!(huge, MAX_WAIT_SECONDS);
        assert!(note.is_some());
        let (zero, note) = wait_seconds_arg(&json!({"seconds": 0}));
        assert_eq!(zero, 1);
        assert!(note.is_some());
        let (negative, _) = wait_seconds_arg(&json!({"seconds": -4}));
        assert_eq!(negative, 1);
    }

    fn hint(kind: ForegroundKind, command: &str) -> Foreground {
        Foreground { kind, command: command.to_string() }
    }

    /// Without a hint, or outside Running, the refusal is the plain one, as it was before the hint existed.
    #[test]
    fn the_refusal_is_the_plain_text_without_a_running_hint() {
        let plain = "cannot run a command now: the session is Running, not at an idle prompt";
        assert_eq!(not_at_prompt_text(InputState::Running, None, None), plain);
        let ssh = hint(ForegroundKind::Ssh, "ssh ubuntu@lidia");
        assert_eq!(not_at_prompt_text(InputState::Alt, Some(&ssh), None), "cannot run a command now: the session is Alt, not at an idle prompt");
        assert_eq!(not_at_prompt_text(InputState::Submitted, Some(&ssh), None), "cannot run a command now: the session is Submitted, not at an idle prompt");
    }

    /// A program without integration: the text names it, says why commands cannot run, and says what the agent can do.
    /// The original wording is the prefix.
    #[test]
    fn the_refusal_names_a_program_without_integration_and_what_to_do() {
        let ssh = hint(ForegroundKind::Ssh, "ssh ubuntu@lidia");
        let text = not_at_prompt_text(InputState::Running, Some(&ssh), None);
        assert!(
            text.starts_with("cannot run a command now: the session is Running, not at an idle prompt"),
            "the original prefix is kept: {text}"
        );
        assert!(text.contains("`ssh ubuntu@lidia`"), "{text}");
        assert!(text.contains("a shell on another machine, without shell integration"), "{text}");
        assert!(text.contains("get_screen") && text.contains("wait"), "{text}");
        assert!(text.contains("Ask the user to leave it"), "{text}");
    }

    /// Any other command: it is still running, and the agent should wait for it.
    #[test]
    fn the_refusal_for_an_ordinary_running_command_says_it_is_still_running() {
        let sleep = hint(ForegroundKind::Other, "sleep 30");
        let text = not_at_prompt_text(InputState::Running, Some(&sleep), None);
        assert!(text.starts_with("cannot run a command now: the session is Running, not at an idle prompt"), "{text}");
        assert!(text.contains("`sleep 30` is still running"), "{text}");
        assert!(!text.contains("without shell integration"), "{text}");
    }

    /// The screen carries the hint only when there is one: the key is left out otherwise.
    #[test]
    fn the_screen_wire_has_the_foreground_only_when_present() {
        let screen = |foreground: Option<Foreground>| ScreenView {
            rows: vec!["ubuntu@lidia:~$".into()],
            cursor: (0, 0),
            alternate_screen: false,
            size: (24, 80),
            state: InputState::Running,
            observer_status: ObserverStatus::Ok,
            integration_status: IntegrationStatus::Live,
            foreground,
            location: None,
            history: None,
        };
        let without = serde_json::to_value(screen(None)).unwrap();
        assert!(without.get("foreground").is_none(), "{without}");
        let with = serde_json::to_value(screen(Some(hint(ForegroundKind::Ssh, "ssh ubuntu@lidia")))).unwrap();
        assert_eq!(with["foreground"], json!({"kind": "ssh", "command": "ssh ubuntu@lidia"}));
    }

    #[test]
    fn ignore_updates_must_be_a_boolean_and_defaults_to_false() {
        assert_eq!(bool_arg(&json!({}), "ignore_updates", false), Ok(false));
        assert_eq!(bool_arg(&json!({"ignore_updates": true}), "ignore_updates", false), Ok(true));
        assert!(bool_arg(&json!({"ignore_updates": "yes"}), "ignore_updates", false).is_err());
    }

    /// An Unresponsive nested shell refuses with its own text, which says what the agent can do instead.
    #[test]
    fn the_refusal_for_an_unresponsive_shell_says_so_and_what_to_do() {
        let text = not_at_prompt_text(InputState::Unresponsive, None, None);
        assert_eq!(text, UNRESPONSIVE_REFUSAL);
        assert!(text.starts_with("cannot run a command now: the shell is not answering"), "{text}");
        assert!(text.contains("it may be busy, or the connection dropped") && text.contains("use get_screen, or ask the user"), "{text}");
    }

    /// Only the last non-blank row is looked at, and only password-like prompts match. Case does not matter.
    #[test]
    fn a_password_prompt_on_the_last_row_is_detected() {
        let rows = |items: &[&str]| items.iter().map(|r| r.to_string()).collect::<Vec<String>>();
        assert!(screen_waits_for_input(&rows(&["ubuntu@lidia:~$ sudo ls", "[sudo] password for ubuntu:"])));
        assert!(screen_waits_for_input(&rows(&["Password: "])));
        assert!(screen_waits_for_input(&rows(&["ubuntu@lidia's PASSWORD:  ", "", ""])), "blank rows after it are skipped");
        assert!(screen_waits_for_input(&rows(&["Enter passphrase for key '/home/u/.ssh/id':"])));
        assert!(!screen_waits_for_input(&rows(&["Password: ", "ubuntu@lidia:~$"])), "the prompt is not the last row");
        assert!(!screen_waits_for_input(&rows(&["ubuntu@lidia:~$ echo password"])), "a shell prompt is not a password prompt");
        assert!(!screen_waits_for_input(&rows(&["ls", "ubuntu@lidia:~$"])));
        assert!(!screen_waits_for_input(&[]));
    }

    /// The top level of a nested shell is its label and Hello fields; the local shell has none.
    #[test]
    fn a_top_level_is_nested_only_with_a_location() {
        assert!(!TopLevel::default().is_nested());
        let remote = TopLevel { key: 1, location: Some("ssh h".into()), ..TopLevel::default() };
        assert!(remote.is_nested());
    }

    #[test]
    fn the_refusal_for_integration_names_the_reason_and_the_next_step() {
        let text = integration_refusal_text(&InjectRefusal::NotRunning);
        assert!(text.starts_with("cannot integrate the shell now: no command is running in this shell"), "{text}");
        assert!(text.contains("call request_integration again"), "{text}");
        for reason in [InjectRefusal::AltScreen, InjectRefusal::NoShellProgram, InjectRefusal::TooDeep, InjectRefusal::Busy] {
            let text = integration_refusal_text(&reason);
            assert!(text.starts_with("cannot integrate the shell now: "), "{text}");
            assert!(text.len() > 60, "a next step is given: {text}");
        }
    }

    #[test]
    fn integration_results_say_what_to_do_next() {
        let ok = integration_result(&IntegrateOutcome::Integrated {
            key: 1,
            label: "ssh ubuntu@lidia".into(),
            sh: "zsh".into(),
            user: None,
            host: None,
        });
        assert_eq!(ok, Ok(json!("Integrated ssh ubuntu@lidia (zsh). You can now use run_command in this shell.")));
        let unsupported = integration_result(&IntegrateOutcome::UnsupportedShell).unwrap_err();
        assert!(unsupported.contains("not supported") && unsupported.contains("stay read-only"), "{unsupported}");
        let timeout = integration_result(&IntegrateOutcome::Timeout).unwrap_err();
        assert!(timeout.contains("timed out") && timeout.contains("at its prompt"), "{timeout}");
        let failed = integration_result(&IntegrateOutcome::Failed { hint: "agentterm: Unsupported shell (promptvars off)".into() })
            .unwrap_err();
        assert!(failed.starts_with("integration did not complete: agentterm: Unsupported shell (promptvars off)."), "{failed}");
        let refused = integration_result(&IntegrateOutcome::Refused { reason: InjectRefusal::Busy }).unwrap_err();
        assert!(refused.starts_with("cannot integrate the shell now: another shell integration"), "{refused}");
    }

    #[test]
    fn request_integration_is_offered_with_no_arguments() {
        let schema = schemas().into_iter().find(|s| s.name == "request_integration").expect("offered");
        assert_eq!(schema.parameters["properties"], json!({}));
        assert!(schema.description.contains("dialog"), "{}", schema.description);
    }

    /// A shell or program without integration in the foreground ends the refusal with the next step. Other programs do not.
    #[test]
    fn the_refusal_offers_request_integration_only_for_a_shell_that_can_be_integrated() {
        let ssh = hint(ForegroundKind::Ssh, "ssh ubuntu@lidia");
        assert!(not_at_prompt_text(InputState::Running, Some(&ssh), None).ends_with(INTEGRATE_HINT));
        let sleep = hint(ForegroundKind::Other, "sleep 30");
        assert!(!not_at_prompt_text(InputState::Running, Some(&sleep), None).contains("request_integration"));
    }

    #[test]
    fn the_password_refusal_tells_the_agent_not_to_type() {
        assert!(INTEGRATE_PASSWORD_REFUSAL.contains("Do not type it"), "{INTEGRATE_PASSWORD_REFUSAL}");
        assert!(INTEGRATE_PASSWORD_REFUSAL.contains("call request_integration again"), "{INTEGRATE_PASSWORD_REFUSAL}");
    }

    #[test]
    fn run_command_wait_seconds_default_and_clamp() {
        assert_eq!(seconds_arg(&json!({}), "wait_seconds", DEFAULT_RUN_WAIT_SECONDS), (5, None));
        assert_eq!(seconds_arg(&json!({"wait_seconds": 1}), "wait_seconds", DEFAULT_RUN_WAIT_SECONDS), (1, None));
        let (high, note) = seconds_arg(&json!({"wait_seconds": 500}), "wait_seconds", DEFAULT_RUN_WAIT_SECONDS);
        assert_eq!(high, MAX_WAIT_SECONDS);
        assert!(note.is_some_and(|n| n.starts_with("wait_seconds 500 is outside 1 to 120")));
    }

    /// `lines` is absent (the visible screen), a clamped number with a note, or an error when it is not an integer.
    #[test]
    fn get_screen_lines_is_optional_clamped_and_must_be_an_integer() {
        assert_eq!(lines_arg(&json!({})), Ok(None));
        assert_eq!(lines_arg(&json!({"lines": null})), Ok(None));
        assert_eq!(lines_arg(&json!({"lines": 150})), Ok(Some((150, None))));
        let (high, note) = lines_arg(&json!({"lines": 900})).unwrap().unwrap();
        assert_eq!(high, MAX_SCREEN_LINES);
        assert!(note.is_some_and(|n| n.contains("900")));
        let (low, note) = lines_arg(&json!({"lines": 0})).unwrap().unwrap();
        assert_eq!(low, 1);
        assert!(note.is_some());
        assert!(lines_arg(&json!({"lines": "many"})).is_err());
    }

    #[test]
    fn the_running_hint_names_wait_and_get_screen_and_never_send_keys() {
        let with_id = running_hint(Some(7));
        assert!(with_id.contains("wait with id 7") && with_id.contains("get_screen with lines"), "{with_id}");
        assert!(with_id.contains("run_command is unavailable until it ends"), "{with_id}");
        assert!(!with_id.contains("send_keys"), "{with_id}");
        let without = running_hint(None);
        assert!(without.contains("has not started") && !without.contains("send_keys"), "{without}");
    }

    /// The refusal names the agent's open command and its id, and says to wait for it. A plain command the agent started is
    /// not named a second time from the foreground hint.
    #[test]
    fn the_refusal_names_the_agents_running_command_and_its_id() {
        let rec = OpenAgentRecord {
            id: 12,
            call_id: "run-3".into(),
            command: "sleep 30".into(),
            age: Duration::from_secs(190),
            on_top: true,
        };
        let sleep = hint(ForegroundKind::Other, "sleep 30");
        let text = not_at_prompt_text(InputState::Running, Some(&sleep), Some(&rec));
        assert!(text.starts_with("cannot run a command now: the session is Running, not at an idle prompt"), "{text}");
        assert!(text.contains("Your command id 12 `sleep 30` is still running (3m)"), "{text}");
        assert!(text.contains("wait with id 12"), "{text}");
        assert_eq!(text.matches("sleep 30").count(), 1, "named twice: {text}");
    }

    #[test]
    fn age_text_is_seconds_then_minutes_then_hours() {
        assert_eq!(age_text(Duration::from_secs(42)), "42s");
        assert_eq!(age_text(Duration::from_secs(180)), "3m");
        assert_eq!(age_text(Duration::from_secs(7300)), "2h");
    }

    /// The schemas offer the new arguments, and run_command no longer promises a 30 s wait.
    #[test]
    fn the_schemas_offer_wait_seconds_lines_and_the_wait_id() {
        let by_name = |n: &str| schemas().into_iter().find(|s| s.name == n).expect("offered");
        let run = by_name("run_command");
        assert!(run.parameters["properties"]["wait_seconds"].is_object(), "{}", run.parameters);
        assert!(!run.description.contains("30 s"), "{}", run.description);
        assert!(by_name("get_screen").parameters["properties"]["lines"].is_object());
        assert!(by_name("wait").parameters["properties"]["id"].is_object());
        assert!(!by_name("send_keys").description.is_empty());
    }
}
