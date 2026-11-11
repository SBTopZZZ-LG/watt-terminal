//! The agent's tools: plain Rust methods over a session, plus JSON schemas so a model can call them later.
//!
//! Read tools are always available. `run_command` is the only tool that writes to the shell, and it is
//! refused unless the session is at an idle prompt with shell integration live. `send_keys` is a stub.
//! Approval is not checked here; `agent::AgentCtx::call_tool` asks for it before calling these methods.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::core::foreground::{Foreground, ForegroundKind};
use crate::core::llm::TurnCancel;
use crate::core::observer::{CallStatus, InputState, IntegrationStatus, ObserverStatus};
use crate::core::record::{Cwd, Record, RecordSummary, Source};
use crate::core::session::{user_text_refusal, Session};

/// How long `run_command` waits for the command to finish before it returns `TimedOut`.
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(30);
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
    /// The turn was cancelled while the call waited. Nothing is sent to the shell to stop it: if the line was already
    /// sent, it keeps running, and the user can Ctrl-C it in the terminal.
    Cancelled,
}

// `Finished` carries a whole Record. Boxing it would change the public RunOutcome type, so it stays inline.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    /// The command finished within the timeout.
    Finished { record: Record },
    /// The command had not finished when the timeout expired. It keeps running; the shell is not touched.
    /// `state` is the shell state at that moment: only `Running` means the command is still running.
    TimedOut { state: InputState },
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
    fn get_cwd(&self) -> Option<Cwd>;
    /// The foreground program hint, while a command runs (core/foreground.rs). A hint only, never a safety decision.
    /// Defaults to None, so a fake without a shell needs no change.
    fn foreground(&self) -> Option<Foreground> {
        None
    }
    fn run_command(&self, command: &str, timeout: Duration) -> Result<RunOutcome, String>;
    /// Waits up to `seconds` (already clamped to 1..=MAX_WAIT_SECONDS) for the shell to do something worth reporting. See
    /// `WaitReason`. Counts against the turn's wait budget, not the command limit.
    fn wait(&self, seconds: u64, ignore_updates: bool) -> Result<WaitView, String>;
    /// Whether `run_command` would already be refused for text the user typed on the prompt. Checked before the approval
    /// card, so the user is not asked to approve a command that will be refused. Writes nothing.
    fn run_command_precheck(&self) -> Result<(), String> {
        Ok(())
    }
    fn send_keys(&self, bytes: &[u8]) -> Result<(), String>;

    /// Calls a tool by schema name. Optional arguments take their defaults. Does not ask for approval.
    fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "list_commands" => to_json(self.list_commands(usize_arg(args, "n", DEFAULT_LIST)?)),
            "get_command" => {
                let id = u64_arg(args, "id")?;
                let offset = usize_arg(args, "offset", 0)?;
                let limit = usize_arg(args, "limit", DEFAULT_LIMIT)?.min(MAX_LIMIT);
                to_json(self.get_command(id, offset, limit)?)
            }
            "get_screen" => to_json(self.get_screen()),
            "get_cwd" => to_json(self.get_cwd()),
            "wait" => {
                let (seconds, note) = wait_seconds_arg(args);
                let ignore_updates = bool_arg(args, "ignore_updates", false)?;
                let mut value = to_json(self.wait(seconds, ignore_updates)?)?;
                if let Some(note) = note {
                    value["note"] = json!(note);
                }
                Ok(value)
            }
            "run_command" => {
                let command = str_arg(args, "command")?;
                to_json(self.run_command(command, DEFAULT_RUN_TIMEOUT)?)
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
        }
    }

    fn get_cwd(&self) -> Option<Cwd> {
        self.session.cwd()
    }

    fn foreground(&self) -> Option<Foreground> {
        self.session.foreground()
    }

    fn run_command(&self, command: &str, timeout: Duration) -> Result<RunOutcome, String> {
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
            return Ok(RunOutcome::NotRun { reason: NotRunReason::Cancelled });
        }
        let call_id = format!("run-{}", NEXT_RUN.fetch_add(1, Ordering::Relaxed));
        // submit() re-checks the prompt, and the user's text, atomically, so a racing user submit or keystroke wins or
        // loses cleanly.
        self.session
            .submit(Source::Agent { call_id: call_id.clone() }, command)
            .map_err(|e| e.to_string())?;

        let deadline = Instant::now() + timeout;
        loop {
            // One lock read per poll: the outcome and the shell state come from the same moment.
            match self.session.call_status(&call_id, self.input_wait) {
                CallStatus::Finished(record) => return Ok(RunOutcome::Finished { record: *record }),
                CallStatus::Abandoned => return Ok(RunOutcome::NotRun { reason: NotRunReason::Abandoned }),
                CallStatus::WaitingForInput => return Ok(RunOutcome::NotRun { reason: NotRunReason::WaitingForInput }),
                CallStatus::Pending(state) => {
                    if self.cancelled() {
                        return Ok(RunOutcome::NotRun { reason: NotRunReason::Cancelled });
                    }
                    if Instant::now() >= deadline {
                        return Ok(RunOutcome::TimedOut { state });
                    }
                }
            }
            thread::sleep(RUN_POLL);
        }
    }

    fn wait(&self, seconds: u64, ignore_updates: bool) -> Result<WaitView, String> {
        let used = self.wait_used.get();
        if used >= self.wait_budget {
            return Err(format!(
                "wait budget for this turn is used up ({} s of waiting); stop waiting and report what you have",
                used.as_secs()
            ));
        }
        let started = Instant::now();
        let view = self.wait_for(seconds, ignore_updates, started);
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

    fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
        Err("send_keys is disabled in slice 1".into())
    }
}

impl SessionTools<'_> {
    /// Why `run_command` cannot run now, when the shell is not live or not at an idle prompt. None when it may run.
    /// `run_command` and `run_command_precheck` both use it, so the refusals match.
    fn not_at_prompt(&self) -> Option<String> {
        if self.session.integration_status() != IntegrationStatus::Live {
            return Some("shell integration is not live, so commands cannot be run".into());
        }
        let state = self.session.state();
        (state != InputState::Prompt).then(|| not_at_prompt_text(state, self.session.foreground().as_ref()))
    }

    /// The wait itself. An idle prompt or an exited session returns at once. Otherwise the first of these ends it:
    /// the turn is cancelled, the session exits, a command finishes (the shell back at its prompt with a newer record),
    /// the screen changes (unless `ignore_updates`, and then after its quiet period), or the time runs out.
    fn wait_for(&self, seconds: u64, ignore_updates: bool, started: Instant) -> WaitView {
        let deadline = started + Duration::from_secs(seconds);
        let start = self.session.activity();
        let (reason, finished) = match start.state {
            InputState::Prompt => (WaitReason::Idle, None),
            InputState::Exited => (WaitReason::SessionExited, None),
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
                    if act.last_record_id > start.last_record_id && act.state == InputState::Prompt {
                        break (WaitReason::CommandFinished, Some(act.last_record_id));
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

/// The refusal for a `run_command` when the shell is not at an idle prompt. The text starts with the plain refusal
/// either way. While a command runs and the hint names a program, the text says what runs and what the agent can do:
/// read the screen, wait, and ask the user to leave a program without shell integration.
pub fn not_at_prompt_text(state: InputState, foreground: Option<&Foreground>) -> String {
    let base = format!("cannot run a command now: the session is {state:?}, not at an idle prompt");
    let Some(fg) = foreground.filter(|_| state == InputState::Running) else {
        return base;
    };
    match fg.kind {
        ForegroundKind::Other => format!(
            "{base}. The command `{}` is still running in the shell. Wait for it to finish (wait), then run the command again.",
            fg.command
        ),
        kind => format!(
            "{base}. The foreground program is `{}`: {}, without shell integration, so commands cannot run in it. \
             Read it with get_screen and wait for it to end. Ask the user to leave it, so commands can run again.",
            fg.command,
            kind.describe()
        ),
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
            description: "Get the visible screen: rows, cursor, whether the alternate screen is active, and the observer and shell-integration status.",
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSchema {
            name: "get_cwd",
            description: "Get the shell's current host and directory, or null if not known yet.",
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSchema {
            name: "wait",
            description: "Wait for the shell instead of polling get_screen. Use it after run_command comes back with the command still running. Returns at once when a command finishes (with its record), and early when the screen changes unless ignore_updates is true; set ignore_updates when only the end of the command matters. Also returns on timeout, and at once when the shell is idle.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer", "minimum": 1, "maximum": MAX_WAIT_SECONDS, "default": DEFAULT_WAIT_SECONDS},
                    "ignore_updates": {"type": "boolean", "default": false},
                },
            }),
        },
        ToolSchema {
            name: "run_command",
            description: "Run a command in the user's own shell session. Only works at an idle prompt with shell integration live. Waits up to 30 s for the command to finish. The user must approve each call unless auto-approve is on.",
            parameters: json!({
                "type": "object",
                "properties": {"command": {"type": "string", "minLength": 1}},
                "required": ["command"],
            }),
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
    let requested = args.get("seconds").and_then(|v| v.as_i64().map(i128::from).or_else(|| v.as_u64().map(i128::from)));
    let Some(requested) = requested else {
        return (DEFAULT_WAIT_SECONDS, None);
    };
    let clamped = requested.clamp(1, i128::from(MAX_WAIT_SECONDS));
    let note = (clamped != requested)
        .then(|| format!("seconds {requested} is outside 1 to {MAX_WAIT_SECONDS}; waited {clamped} s instead"));
    (clamped as u64, note)
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
        assert_eq!(not_at_prompt_text(InputState::Running, None), plain);
        let ssh = hint(ForegroundKind::Ssh, "ssh ubuntu@lidia");
        assert_eq!(not_at_prompt_text(InputState::Alt, Some(&ssh)), "cannot run a command now: the session is Alt, not at an idle prompt");
        assert_eq!(not_at_prompt_text(InputState::Submitted, Some(&ssh)), "cannot run a command now: the session is Submitted, not at an idle prompt");
    }

    /// A program without integration: the text names it, says why commands cannot run, and says what the agent can do.
    /// The original wording is the prefix.
    #[test]
    fn the_refusal_names_a_program_without_integration_and_what_to_do() {
        let ssh = hint(ForegroundKind::Ssh, "ssh ubuntu@lidia");
        let text = not_at_prompt_text(InputState::Running, Some(&ssh));
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
        let text = not_at_prompt_text(InputState::Running, Some(&sleep));
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
}
