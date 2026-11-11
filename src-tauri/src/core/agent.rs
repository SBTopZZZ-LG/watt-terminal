//! The agent: turns one user message into tool calls and events, and gates the tools that act.
//!
//! SECURITY: command output and screen contents are untrusted input to a model. Text printed in the
//! terminal (by a program, a file, or a web page) can try to steer a real LLM (prompt injection), and both
//! can contain secrets. So `run_command` asks for approval every time by default, `send_keys` always asks,
//! and auto-approve starts off. Auto-approve only skips the approval step; it never lets a command run
//! while the shell is busy.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::core::approval::{ApprovalPolicy, ManualPolicy, Reason, Verdict};
use crate::core::chat::todo::Todo;
use crate::core::foreground::Foreground;
use crate::core::llm::TurnCancel;
use crate::core::observer::OpenAgentRecord;
use crate::core::record::RecordSummary;
use crate::core::session::{Session, USER_TEXT_CODE};
use crate::core::tools::{AgentTools, TopLevel, integration_result};

pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Prefix of the error for a call the user did not approve: denied, cancelled, or timed out. Callers match on it to tell
/// a denial from a tool error. The text after it is for people.
pub const DENIED: &str = "not_approved";

/// Whether approve-all applies to the shell on top right now (see `ApprovalGate::auto_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoState {
    /// Approve-all is off.
    Off,
    /// On, and switched on in the shell that is on top now.
    On,
    /// On, but switched on in another shell level. The calls here ask.
    Suspended,
}

/// Calls waiting for an answer, keyed by call id. A call that is not answered in time counts as a denial.
struct Pending {
    requests: Mutex<HashMap<String, Sender<bool>>>,
    timeout: Duration,
}

impl Pending {
    fn new(timeout: Duration) -> Self {
        Pending { requests: Mutex::new(HashMap::new()), timeout }
    }

    /// Registers `call_id`, runs `announce` so the UI can show the request, then waits. Only an explicit answer of true
    /// returns true. Denial, cancel and timeout return false.
    fn ask(&self, call_id: &str, announce: impl FnOnce()) -> bool {
        let (tx, rx) = mpsc::channel();
        self.lock().insert(call_id.to_string(), tx);
        announce();
        let answer = rx.recv_timeout(self.timeout).unwrap_or(false);
        self.lock().remove(call_id);
        answer
    }

    /// Answers a pending request. Returns false if `call_id` is not pending (already answered or timed out).
    fn decide(&self, call_id: &str, answer: bool) -> bool {
        let tx = self.lock().remove(call_id);
        tx.is_some_and(|tx| tx.send(answer).is_ok())
    }

    /// Answers every pending request with false.
    fn cancel_all(&self) {
        for (_, tx) in self.lock().drain() {
            let _ = tx.send(false);
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Sender<bool>>> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Pending approvals for `run_command` and `send_keys`, keyed by call id. Also holds the `request_integration` asks.
pub struct ApprovalGate {
    pending: Pending,
    /// The `request_integration` asks. Cancelled together with the approvals (Ctrl-C, cancel).
    integrations: IntegrationGate,
    /// Approve-all: None when off. When on, the key of the shell level it was switched on in. It applies only while that
    /// level is on top (see `auto_state`), so it never carries into a nested shell or back out of one unseen.
    auto: Mutex<Option<u64>>,
}

impl Default for ApprovalGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalGate {
    pub fn new() -> Self {
        Self::with_timeout(APPROVAL_TIMEOUT)
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        ApprovalGate {
            pending: Pending::new(timeout),
            integrations: IntegrationGate::with_timeout(timeout),
            auto: Mutex::new(None),
        }
    }

    /// Switches approve-all on or off. `level` is the key of the shell on top when it is switched on. The caller takes it
    /// from the session. Ignored when `on` is false.
    pub fn set_auto_approve(&self, on: bool, level: u64) {
        *self.auto_lock() = on.then_some(level);
    }

    /// True while approve-all is on, in whatever shell level it was switched on.
    pub fn auto_approve(&self) -> bool {
        self.auto_lock().is_some()
    }

    /// Approve-all as it applies to the shell `level` (the one on top now).
    pub fn auto_state(&self, level: u64) -> AutoState {
        match *self.auto_lock() {
            None => AutoState::Off,
            Some(on_level) if on_level == level => AutoState::On,
            Some(_) => AutoState::Suspended,
        }
    }

    /// Approve-all as it applies to the shell `level`, when nested shells follow the local rules (`across_levels`): then it
    /// is on whenever it is switched on, in whichever shell that was. Otherwise the same as `auto_state`.
    pub fn auto_state_across(&self, level: u64, across_levels: bool) -> AutoState {
        if across_levels && self.auto_approve() { AutoState::On } else { self.auto_state(level) }
    }

    fn auto_lock(&self) -> MutexGuard<'_, Option<u64>> {
        self.auto.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers `call_id`, runs `announce` so the UI can show the request, then waits. Only an explicit approval returns
    /// true. Denial, cancel and timeout return false.
    pub fn ask(&self, call_id: &str, announce: impl FnOnce()) -> bool {
        self.pending.ask(call_id, announce)
    }

    /// Answers a pending approval. Returns false if `call_id` is not pending (already answered or timed out).
    pub fn decide(&self, call_id: &str, approve: bool) -> bool {
        self.pending.decide(call_id, approve)
    }

    /// The gate for `request_integration` asks.
    pub fn integrations(&self) -> &IntegrationGate {
        &self.integrations
    }

    /// Denies every pending request, approvals and integration asks alike. Called when the user presses Ctrl-C.
    pub fn cancel_all(&self) {
        self.pending.cancel_all();
        self.integrations.cancel_all();
    }
}

/// Pending `request_integration` asks, keyed by call id. Answered by `answer_integration`. A timed-out or cancelled ask is
/// declined, as an approval would be.
pub struct IntegrationGate {
    pending: Pending,
}

impl IntegrationGate {
    pub fn with_timeout(timeout: Duration) -> Self {
        IntegrationGate { pending: Pending::new(timeout) }
    }

    /// Registers `call_id`, runs `announce` (the dialog), then waits. Only an explicit accept returns true.
    pub fn ask(&self, call_id: &str, announce: impl FnOnce()) -> bool {
        self.pending.ask(call_id, announce)
    }

    /// Answers a pending ask. Returns false if `call_id` is not pending.
    pub fn decide(&self, call_id: &str, accept: bool) -> bool {
        self.pending.decide(call_id, accept)
    }

    pub fn cancel_all(&self) {
        self.pending.cancel_all();
    }
}

/// Sends user keystrokes to the PTY. Ctrl-C (0x03) also denies every pending approval.
pub fn user_input(session: &Session, approvals: &ApprovalGate, text: &str) -> io::Result<()> {
    if text.contains('\u{3}') {
        approvals.cancel_all();
    }
    session.write_user(text.as_bytes())
}

/// Cancels the turn running on this session, if there is one, and denies every pending approval. Ctrl-C and the
/// cancel command both call it. Idempotent, and harmless when no turn is running. A cancelled `run_command` returns
/// at once; a command it already sent keeps running in the shell.
pub fn cancel_turn(current: &Mutex<Option<Arc<TurnCancel>>>, approvals: &ApprovalGate) {
    let token = current.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(token) = token {
        token.cancel();
    }
    approvals.cancel_all();
}

/// Runs a closure when dropped, including while a panic unwinds. Used so a turn cannot leave flags set.
pub struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    pub fn new(f: F) -> Self {
        OnDrop(Some(f))
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Text { text: String },
    ToolCall { call_id: String, name: String, args: Value },
    /// `decision` says how the call was allowed or stopped: "read" (a read tool, or `todo_write`: no approval), "manual"
    /// (the user approved), "auto" (run without a card), "denied" (not approved: denied, cancelled or timed out),
    /// or "refused" (refused before it ran, without asking, or by the tool when it was run without a card).
    /// `reason` is why the call asked or ran without asking (core/approval codes). Absent for read tools, `todo_write`
    /// and calls refused before the policy.
    ToolResult {
        call_id: String,
        name: String,
        ok: bool,
        decision: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'static str>,
        result: Value,
    },
    /// A call waits for the user. `name` is the tool name, so the card can say what is being asked. `reason` is why it
    /// asks (a core/approval code). `level` is the key of the shell the card is for (0 is the local shell), and `location`
    /// is that shell's label when it is a nested one. A command approved here runs only while that shell is on top.
    ApprovalRequest {
        call_id: String,
        name: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'static str>,
        level: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        location: Option<String>,
    },
    /// A `request_integration` call. `phase` is `asking` (the user's answer is awaited, see `answer_integration`), `running`
    /// (the handshake is typing into the shell) or `done` (`outcome` is what the handshake gave, or `declined`). `label` is the
    /// command that holds the shell, on `asking` and `running`. The wire type is `integration_request`: `integration` is the
    /// session's shell-integration status event.
    #[serde(rename = "integration_request")]
    Integration {
        call_id: String,
        phase: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        outcome: Option<Value>,
    },
    /// Streamed assistant text. `Text` is still used for whole messages.
    TextDelta { text: String },
    /// The model is working on a reply (true) or has produced its first output (false).
    Thinking { active: bool },
    /// The agent is in control of the shell: an approved `run_command` or `send_keys` is executing. Sent true before
    /// the call and false after it. A denied call sends nothing. A call the tool refuses (for example, the shell is
    /// busy) is bracketed too, but the bracket is immediate.
    AgentControl { active: bool },
    /// The conversation's todo list was replaced by a successful `todo_write`. `items` is the whole list.
    TodoUpdate { items: Vec<Todo> },
    Done,
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

fn new_call_id() -> String {
    format!("call-{}", NEXT_CALL.fetch_add(1, Ordering::Relaxed))
}

/// Tools that never change the shell. `read_tool` accepts only these. Must match `READ_NAMES` in core/chat/agent_loop.rs.
const READ_TOOLS: [&str; 5] = ["list_commands", "get_command", "get_screen", "get_cwd", "wait"];

/// What a turn reads from the saved settings when it decides. lib.rs builds one per user message. The defaults are the
/// cautious ones: a nested shell is approved as before (its allow rules ask), and `request_integration` asks the user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnRules {
    /// Approval nested = same as local: allow rules and approve-all apply in nested shells too.
    pub follow_local_in_nested: bool,
    /// Integration requests = auto: `request_integration` integrates the shell without a dialog.
    pub integrate_without_asking: bool,
}

/// What an agent may use during one turn: the approval gate, the approval policy, the turn's rules, and a way to report
/// events. The tools are private: the only route to them is `call_tool` (approval-gated) or `read_tool` (read-only names).
pub struct AgentCtx<'a> {
    tools: &'a dyn AgentTools,
    approvals: &'a ApprovalGate,
    policy: &'a dyn ApprovalPolicy,
    rules: TurnRules,
    emit: &'a dyn Fn(AgentEvent),
}

/// The policy when none is given: every `run_command` asks. Safe by default.
static MANUAL: ManualPolicy = ManualPolicy;

impl<'a> AgentCtx<'a> {
    pub fn new(tools: &'a dyn AgentTools, approvals: &'a ApprovalGate, emit: &'a dyn Fn(AgentEvent)) -> Self {
        AgentCtx { tools, approvals, policy: &MANUAL, rules: TurnRules::default(), emit }
    }

    /// Uses `policy` for the calls that are not approve-all (core/approval). lib.rs builds one per user message.
    pub fn with_policy(mut self, policy: &'a dyn ApprovalPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Uses `rules` for the nested-shell and integration decisions of this turn (see `TurnRules`).
    pub fn with_rules(mut self, rules: TurnRules) -> Self {
        self.rules = rules;
        self
    }

    pub fn emit(&self, ev: AgentEvent) {
        (self.emit)(ev);
    }

    pub fn say(&self, text: impl Into<String>) {
        self.emit(AgentEvent::Text { text: text.into() });
    }

    /// The foreground program hint at this moment (see `AgentTools::foreground`). The agent loop reads it per model call.
    pub fn foreground(&self) -> Option<Foreground> {
        self.tools.foreground()
    }

    /// The shell the agent acts in at this moment (see `AgentTools::top_level`). The agent loop reads it per model call.
    pub fn top_level(&self) -> TopLevel {
        self.tools.top_level()
    }

    /// The agent's own open commands at this moment (see `AgentTools::open_agent_records`). Read per model call.
    pub fn open_agent_records(&self) -> Vec<OpenAgentRecord> {
        self.tools.open_agent_records()
    }

    /// A finished command's summary, by id (see `AgentTools::finished_summary`).
    pub fn finished_summary(&self, id: u64) -> Option<RecordSummary> {
        self.tools.finished_summary(id)
    }

    /// Calls a read-only tool (`list_commands`, `get_command`, `get_screen`, `get_cwd`, `wait`) and reports the call
    /// and its result. Never asks for approval. Any other name is refused without running.
    pub fn read_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        if !READ_TOOLS.contains(&name) {
            return Err(format!("`{name}` is not a read-only tool; it must go through call_tool"));
        }
        self.run(name, args, false)
    }

    /// Calls a tool and reports the call and its result. `run_command` and `send_keys` wait for approval
    /// first (`run_command` skips it when auto-approve is on). A denied call returns an error and does nothing.
    pub fn call_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        self.run(name, args, true)
    }

    /// Runs a tool the agent loop handles itself (`todo_write`): no approval, no shell, no backend. It reports the call
    /// and its result like any tool, with the "read" decision, since no approval was asked.
    pub fn local_tool(
        &self,
        name: &str,
        args: &Value,
        run: impl FnOnce(&Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let call_id = new_call_id();
        self.emit(AgentEvent::ToolCall { call_id: call_id.clone(), name: name.to_string(), args: args.clone() });
        let result = run(args);
        let (ok, value) = match &result {
            Ok(v) => (true, v.clone()),
            Err(e) => (false, json!({"error": e})),
        };
        self.emit(AgentEvent::ToolResult {
            call_id,
            name: name.to_string(),
            ok,
            decision: "read",
            reason: None,
            result: value,
        });
        result
    }

    fn run(&self, name: &str, args: Value, gated: bool) -> Result<Value, String> {
        let call_id = new_call_id();
        self.emit(AgentEvent::ToolCall { call_id: call_id.clone(), name: name.to_string(), args: args.clone() });
        let (decision, reason, result) = self.gate_and_execute(&call_id, name, &args, gated);
        let (ok, value) = match &result {
            Ok(v) => (true, v.clone()),
            Err(e) => (false, json!({"error": e})),
        };
        self.emit(AgentEvent::ToolResult { call_id, name: name.to_string(), ok, decision, reason, result: value });
        result
    }

    /// Applies the gate to one call and runs it if allowed. Returns the decision label, the reason (see
    /// `AgentEvent::ToolResult`) and the result.
    fn gate_and_execute(
        &self,
        call_id: &str,
        name: &str,
        args: &Value,
        gated: bool,
    ) -> (&'static str, Option<&'static str>, Result<Value, String>) {
        if !gated || READ_TOOLS.contains(&name) {
            return ("read", None, self.execute(name, args, false, None));
        }
        if name == "request_integration" {
            return self.gate_integration(call_id);
        }
        // A run_command the user's typed text would block is refused before the approval card: the user is not asked to
        // approve it. The tool checks again when it injects, in case the user types while the card waits.
        if name == "run_command"
            && let Err(e) = self.tools.run_command_precheck()
        {
            return ("refused", None, Err(e));
        }
        // The shell on top now. The card and the submit are both bound to its key.
        let top = self.tools.top_level();
        // Approve-all skips the policy for run_command. send_keys always goes to the policy, which always asks.
        let verdict = match name {
            "run_command" => self.run_command_verdict(&top, args),
            "send_keys" => self.policy.decide(name, args, self.policy_nested(&top)),
            // Nothing else is gated: it runs as it always did, without a card.
            _ => return self.run_ungated(name, args),
        };
        let level = (name == "run_command").then_some(top.key);
        match verdict {
            Verdict::Auto(reason) => {
                // The gate skipped the card. A tool that refuses here did not run and the user was never asked.
                let result = self.execute(name, args, true, level);
                (if result.is_ok() { "auto" } else { "refused" }, Some(reason.code()), result)
            }
            Verdict::Ask(reason) => {
                let text = args.get("command").and_then(Value::as_str).map_or_else(|| args.to_string(), str::to_string);
                let approved = self.approvals.ask(call_id, || {
                    self.emit(AgentEvent::ApprovalRequest {
                        call_id: call_id.to_string(),
                        name: name.to_string(),
                        text: text.clone(),
                        reason: Some(reason.code()),
                        level: top.key,
                        location: top.location.clone(),
                    })
                });
                if !approved {
                    let err = format!("{DENIED}: the user did not approve this call (denied, cancelled, or timed out)");
                    return ("denied", Some(reason.code()), Err(err));
                }
                // The user approved, so the decision stays "manual" even if the tool then refuses.
                ("manual", Some(reason.code()), self.execute(name, args, true, level))
            }
        }
    }

    /// The verdict for a run_command. Approve-all decides it while it applies to the shell on top, and asks (suspended)
    /// while it was switched on in another shell. Otherwise the policy decides, and a nested shell is told so.
    fn run_command_verdict(&self, top: &TopLevel, args: &Value) -> Verdict {
        match self.approvals.auto_state_across(top.key, self.rules.follow_local_in_nested) {
            AutoState::On => Verdict::Auto(Reason::ApproveAll),
            AutoState::Suspended => Verdict::Ask(Reason::ApproveAllSuspended),
            AutoState::Off => self.policy.decide("run_command", args, self.policy_nested(top)),
        }
    }

    /// Whether the policy treats the shell on top as nested: a nested level, unless nested shells follow the local rules.
    fn policy_nested(&self, top: &TopLevel) -> bool {
        top.is_nested() && !self.rules.follow_local_in_nested
    }

    /// `request_integration`: integrates the shell in the foreground as a new level. It never uses the command card, and
    /// approve-all does not apply to it. With `integration.agent_requests` = auto it runs at once. Otherwise the user is asked
    /// (`asking`) and the call waits for `answer_integration`. A decline, a cancel or a timeout ends the call with an error
    /// that is not a denial (no DENIED prefix), so the turn goes on: the user is away, and the agent can still work.
    fn gate_integration(&self, call_id: &str) -> (&'static str, Option<&'static str>, Result<Value, String>) {
        let label = match self.tools.integration_precheck() {
            Ok(label) => label,
            Err(refusal) => return ("refused", None, Err(refusal)),
        };
        if self.rules.integrate_without_asking {
            let result = self.run_integration(call_id, &label);
            return (if result.is_ok() { "auto" } else { "refused" }, None, result);
        }
        let accepted = self.approvals.integrations().ask(call_id, || {
            self.emit(AgentEvent::Integration {
                call_id: call_id.to_string(),
                phase: "asking",
                label: Some(label.clone()),
                outcome: None,
            })
        });
        if !accepted {
            self.emit(AgentEvent::Integration {
                call_id: call_id.to_string(),
                phase: "done",
                label: None,
                outcome: Some(json!({"outcome": "declined"})),
            });
            let err = "declined: the user did not integrate this shell (declined or no answer within 2 minutes). Do not ask \
                       again in this turn. You can still read it with get_screen, or ask the user in your reply."
                .to_string();
            return ("denied", None, Err(err));
        }
        // The user accepted, so the decision stays "manual" even if the handshake then fails.
        ("manual", None, self.run_integration(call_id, &label))
    }

    /// The handshake for `request_integration`, bracketed by its `running` and `done` events. The result is what the model
    /// reads: the text of an integrated shell, or the error that says what to do next.
    fn run_integration(&self, call_id: &str, label: &str) -> Result<Value, String> {
        self.emit(AgentEvent::Integration {
            call_id: call_id.to_string(),
            phase: "running",
            label: Some(label.to_string()),
            outcome: None,
        });
        let outcome = self.tools.integrate_shell();
        self.emit(AgentEvent::Integration {
            call_id: call_id.to_string(),
            phase: "done",
            label: None,
            outcome: serde_json::to_value(&outcome).ok(),
        });
        integration_result(&outcome)
    }

    /// A gated name the gate does not know: no card and no policy. Runs as it did before the policy existed.
    fn run_ungated(&self, name: &str, args: &Value) -> (&'static str, Option<&'static str>, Result<Value, String>) {
        let result = self.execute(name, args, true, None);
        (if result.is_ok() { "auto" } else { "refused" }, None, result)
    }

    /// Calls the tool. For the calls that act on the shell, the agent is in control from before the call until after it.
    /// `level` is the shell the call was approved for (see `AgentTools::run_command`).
    fn execute(&self, name: &str, args: &Value, gated: bool, level: Option<u64>) -> Result<Value, String> {
        let controls_shell = gated && matches!(name, "run_command" | "send_keys");
        if controls_shell {
            self.emit(AgentEvent::AgentControl { active: true });
        }
        let result = self.tools.call(name, args, level);
        if controls_shell {
            self.emit(AgentEvent::AgentControl { active: false });
        }
        result
    }
}

pub trait Agent {
    /// Handles one user message. Replies and tool activity go out through `ctx`.
    fn respond(&mut self, user_text: &str, ctx: &AgentCtx<'_>);
}

/// Runs one turn and always ends it with `Done`.
pub fn run_turn(agent: &mut dyn Agent, user_text: &str, ctx: &AgentCtx<'_>) {
    agent.respond(user_text, ctx);
    ctx.emit(AgentEvent::Done);
}

/// Deterministic keyword rules over the tools. No network, no model.
pub struct MockAgent;

const HELP: &str = "I can tell you the last command and how it ended (\"what was the last command\"), \
describe the screen (\"what do you see\"), tell you the directory (\"where am I\"), or run a command \
once you approve it (\"run `pwd`\").";

impl Agent for MockAgent {
    fn respond(&mut self, user_text: &str, ctx: &AgentCtx<'_>) {
        let text = user_text.trim();
        // ASCII lowercasing keeps byte offsets equal to those in `text`.
        let lower = text.to_ascii_lowercase();
        if let Some(cmd) = run_request(text, &lower) {
            return run_and_report(ctx, &cmd);
        }
        if ["last command", "what happened", "why did it fail"].iter().any(|k| lower.contains(k)) {
            return last_command(ctx);
        }
        if lower.contains("screen") || lower.contains("what do you see") {
            return screen(ctx);
        }
        if lower.contains("where am i") || lower.contains("cwd") {
            return where_am_i(ctx);
        }
        ctx.say(HELP);
    }
}

/// "please run `cmd`" anywhere in the message, or "run cmd" at the start.
fn run_request(text: &str, lower: &str) -> Option<String> {
    const PLEASE: &str = "please run `";
    if let Some(start) = lower.find(PLEASE) {
        let rest = &text[start + PLEASE.len()..];
        return Some(rest.split('`').next().unwrap_or_default().trim().to_string());
    }
    if lower.starts_with("run ") {
        return Some(text[4..].trim().trim_matches('`').trim().to_string());
    }
    None
}

fn run_and_report(ctx: &AgentCtx<'_>, cmd: &str) {
    if cmd.is_empty() {
        ctx.say("Tell me what to run, for example: run `pwd`.");
        return;
    }
    match ctx.call_tool("run_command", json!({"command": cmd})) {
        Ok(v) => ctx.say(outcome_text(cmd, &v)),
        Err(e) if e.starts_with(USER_TEXT_CODE) => {
            ctx.say("I can't run that: you have text typed at the prompt. Clear it (Ctrl-C) and ask again.")
        }
        Err(e) if e.starts_with(DENIED) => {
            ctx.say(format!("I did not run `{cmd}`: it was not approved (denied, cancelled, or timed out)."))
        }
        Err(e) => ctx.say(format!("I did not run `{cmd}`: {e}.")),
    }
}

/// The chat text for a `run_command` result that came back (not an error). Only a shell in the Running state is
/// described as still running.
fn outcome_text(cmd: &str, v: &Value) -> String {
    match v["outcome"].as_str() {
        Some("finished") => {
            let r = &v["record"];
            format!(
                "Ran `{cmd}`: exit code {}. Output: {}.",
                code_text(&r["exit_code"]),
                output_text(r["output"].as_str().unwrap_or_default())
            )
        }
        Some("running") if !v["record_id"].is_null() => format!(
            "`{cmd}` is still running after the wait. It keeps running in the terminal; ask for the last command once it finishes."
        ),
        Some("running") => format!(
            "`{cmd}` has not started after the wait (shell state: {}). Ask for the last command to see whether it ran.",
            v["state"].as_str().unwrap_or("unknown")
        ),
        Some("not_run") if v["reason"] == "abandoned" => format!(
            "`{cmd}` did not run: the shell went back to the prompt without executing it (the input was cancelled or incomplete)."
        ),
        Some("not_run") if v["reason"] == "cancelled" => format!(
            "I stopped waiting for `{cmd}` because you cancelled it. If it had already been sent to the shell, it keeps running there; Ctrl-C in the terminal stops it."
        ),
        Some("not_run") => format!(
            "`{cmd}` did not run yet: the shell is waiting for the rest of a multi-line command (an unclosed quote or backslash). Press Ctrl-C in the terminal to cancel it, then ask again."
        ),
        _ => format!("I got an unexpected result for `{cmd}`."),
    }
}

fn last_command(ctx: &AgentCtx<'_>) {
    let last = ctx
        .read_tool("list_commands", json!({"n": 1}))
        .ok()
        .and_then(|v| v.as_array().and_then(|a| a.last().cloned()));
    let Some(id) = last.and_then(|c| c["id"].as_u64()) else {
        ctx.say("No command has finished in this session yet.");
        return;
    };
    let view = match ctx.read_tool("get_command", json!({"id": id, "limit": 2000})) {
        Ok(v) => v,
        Err(e) => return ctx.say(format!("I could not read the last command: {e}.")),
    };
    let observer_ok = ctx.read_tool("get_screen", json!({})).ok().is_some_and(|s| s["observer_status"] == "ok");
    let cwd = match &view["cwd"] {
        Value::Null => "unknown".to_string(),
        c => c["path"].as_str().unwrap_or("?").to_string(),
    };
    ctx.say(format!(
        "Last command: `{}` (source: {}). Exit code {}. Directory: {}. Output: {}. Observer status: {}.",
        view["command"].as_str().unwrap_or_default(),
        view["source"]["kind"].as_str().unwrap_or("unknown"),
        code_text(&view["exit_code"]),
        cwd,
        output_text(view["output"].as_str().unwrap_or_default()),
        if observer_ok { "ok" } else { "desynced, so the output may be incomplete" },
    ));
}

fn screen(ctx: &AgentCtx<'_>) {
    match ctx.read_tool("get_screen", json!({})) {
        Ok(v) => {
            let rows: Vec<&str> = v["rows"]
                .as_array()
                .map(|r| r.iter().filter_map(Value::as_str).filter(|r| !r.trim().is_empty()).collect())
                .unwrap_or_default();
            let alt = if v["alternate_screen"].as_bool() == Some(true) { " (alternate screen)" } else { "" };
            ctx.say(format!("The screen{alt} shows:\n{}", rows.join("\n")));
        }
        Err(e) => ctx.say(format!("I could not read the screen: {e}.")),
    }
}

fn where_am_i(ctx: &AgentCtx<'_>) {
    match ctx.read_tool("get_cwd", json!({})) {
        Ok(Value::Null) => ctx.say("I don't know the working directory yet."),
        Ok(c) => ctx.say(format!("You are in {}.", c["path"].as_str().unwrap_or("?"))),
        Err(e) => ctx.say(format!("I could not read the directory: {e}.")),
    }
}

fn code_text(v: &Value) -> String {
    v.as_i64().map_or_else(|| "unknown".to_string(), |c| c.to_string())
}

fn output_text(out: &str) -> String {
    match out.lines().collect::<Vec<_>>().as_slice() {
        [] => "(none)".to_string(),
        [only] => format!("\"{only}\""),
        [first, .., last] => format!("first line \"{first}\", last line \"{last}\""),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::thread;

    use super::*;
    use crate::core::approval::{MAX_AUTO_PER_TURN, RegexPolicy};
    use crate::core::foreground::ForegroundKind;
    use crate::core::observer::{InputState, IntegrationStatus, ObserverStatus};
    use crate::core::record::{Cwd, RecordSummary};
    use crate::core::session::IntegrateOutcome;
    use crate::core::settings::{ApprovalMode, ApprovalSettings};
    use crate::core::tools::{CommandView, RunOutcome, ScreenView, TopLevel, WaitView};

    /// Records every tool call the agent manages to reach. Nothing here touches a shell.
    #[derive(Default)]
    struct Recorder {
        reached: Mutex<Vec<String>>,
        /// The `expected_level` each `run_command` call reached the tools with.
        bound_levels: Mutex<Vec<Option<u64>>>,
        /// What `run_command_precheck` returns as an error. None means the precheck passes.
        precheck: Option<String>,
        /// What `foreground` reports. None means nothing runs in the foreground.
        foreground: Option<Foreground>,
        /// The shell on top. The default is the local shell; a test can change it between calls.
        top: Mutex<TopLevel>,
        /// When set, every `call` fails with this error after it is recorded.
        call_error: Option<String>,
        /// What `integration_precheck` refuses with. None means it passes (the shell is `ssh h`).
        integration_refusal: Option<String>,
        /// What `integrate_shell` returns. None means an integrated shell.
        integrate_outcome: Mutex<Option<IntegrateOutcome>>,
        /// How many times the handshake was run.
        integrate_calls: Mutex<usize>,
    }

    impl Recorder {
        fn reached(&self) -> Vec<String> {
            self.reached.lock().unwrap().clone()
        }

        fn set_top(&self, top: TopLevel) {
            *self.top.lock().unwrap() = top;
        }
    }

    /// A nested shell on top, as the session reports it: its key and the command that started it.
    fn nested(key: u64, label: &str) -> TopLevel {
        TopLevel { key, location: Some(label.to_string()), ..TopLevel::default() }
    }

    impl AgentTools for Recorder {
        fn list_commands(&self, _n: usize) -> Vec<RecordSummary> {
            Vec::new()
        }
        fn get_command(&self, _id: u64, _offset: usize, _limit: usize) -> Result<CommandView, String> {
            Err("not used".into())
        }
        fn get_screen(&self) -> ScreenView {
            ScreenView {
                rows: Vec::new(),
                cursor: (0, 0),
                alternate_screen: false,
                size: (24, 80),
                state: InputState::Prompt,
                observer_status: ObserverStatus::Ok,
                integration_status: IntegrationStatus::Live,
                foreground: None,
                location: None,
                history: None,
            }
        }
        fn get_cwd(&self) -> Option<Cwd> {
            None
        }
        fn run_command(&self, _command: &str, _wait: Duration, _expected: Option<u64>) -> Result<RunOutcome, String> {
            Err("not used".into())
        }
        fn wait(&self, _seconds: u64, _ignore_updates: bool, _record: Option<u64>) -> Result<WaitView, String> {
            Err("not used".into())
        }
        fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
            Err("not used".into())
        }
        fn run_command_precheck(&self) -> Result<(), String> {
            self.precheck.clone().map_or(Ok(()), Err)
        }
        fn foreground(&self) -> Option<Foreground> {
            self.foreground.clone()
        }
        fn top_level(&self) -> TopLevel {
            self.top.lock().unwrap().clone()
        }
        fn integration_precheck(&self) -> Result<String, String> {
            self.integration_refusal.clone().map_or(Ok("ssh h".to_string()), Err)
        }
        fn integrate_shell(&self) -> IntegrateOutcome {
            *self.integrate_calls.lock().unwrap() += 1;
            self.integrate_outcome.lock().unwrap().clone().unwrap_or_else(integrated)
        }
        fn call(&self, name: &str, _args: &Value, expected_level: Option<u64>) -> Result<Value, String> {
            self.reached.lock().unwrap().push(name.to_string());
            if name == "run_command" {
                self.bound_levels.lock().unwrap().push(expected_level);
            }
            match &self.call_error {
                Some(e) => Err(e.clone()),
                None => Ok(json!({"ok": true})),
            }
        }
    }

    /// `(name, decision, ok)` of every ToolResult, in order.
    fn decisions(log: &Mutex<Vec<AgentEvent>>) -> Vec<(String, &'static str, bool)> {
        log.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolResult { name, decision, ok, .. } => Some((name.clone(), *decision, *ok)),
                _ => None,
            })
            .collect()
    }

    fn named(name: &str, decision: &'static str, ok: bool) -> (String, &'static str, bool) {
        (name.to_string(), decision, ok)
    }

    #[test]
    fn serialized_events_use_the_wire_names() {
        let wire = |e: AgentEvent| serde_json::to_value(&e).unwrap();
        assert_eq!(
            wire(AgentEvent::ApprovalRequest {
                call_id: "c1".into(),
                name: "run_command".into(),
                text: "pwd".into(),
                reason: None,
                level: 0,
                location: None,
            }),
            json!({"type": "approval_request", "call_id": "c1", "name": "run_command", "text": "pwd", "level": 0})
        );
        // A nested card names its shell's label.
        assert_eq!(
            wire(AgentEvent::ApprovalRequest {
                call_id: "c1".into(),
                name: "run_command".into(),
                text: "pwd".into(),
                reason: Some("nested_level"),
                level: 2,
                location: Some("ssh ubuntu@lidia".into()),
            }),
            json!({"type": "approval_request", "call_id": "c1", "name": "run_command", "text": "pwd", "reason": "nested_level", "level": 2, "location": "ssh ubuntu@lidia"})
        );
        assert_eq!(
            wire(AgentEvent::ToolResult {
                call_id: "c1".into(),
                name: "run_command".into(),
                ok: false,
                decision: "denied",
                reason: None,
                result: json!({"error": "x"}),
            }),
            json!({"type": "tool_result", "call_id": "c1", "name": "run_command", "ok": false, "decision": "denied", "result": {"error": "x"}})
        );
        // A reason is sent as its code, next to the decision.
        assert_eq!(
            wire(AgentEvent::ApprovalRequest {
                call_id: "c1".into(),
                name: "run_command".into(),
                text: "ls | wc".into(),
                reason: Some("gate_chars"),
                level: 0,
                location: None,
            }),
            json!({"type": "approval_request", "call_id": "c1", "name": "run_command", "text": "ls | wc", "reason": "gate_chars", "level": 0})
        );
        assert_eq!(
            wire(AgentEvent::ToolResult {
                call_id: "c1".into(),
                name: "run_command".into(),
                ok: true,
                decision: "auto",
                reason: Some("rule"),
                result: json!({"ok": true}),
            }),
            json!({"type": "tool_result", "call_id": "c1", "name": "run_command", "ok": true, "decision": "auto", "reason": "rule", "result": {"ok": true}})
        );
        assert_eq!(wire(AgentEvent::TextDelta { text: "Hel".into() }), json!({"type": "text_delta", "text": "Hel"}));
        assert_eq!(wire(AgentEvent::Thinking { active: true }), json!({"type": "thinking", "active": true}));
        assert_eq!(wire(AgentEvent::Done), json!({"type": "done"}));
        assert_eq!(
            wire(AgentEvent::TodoUpdate {
                items: vec![Todo { content: "fix it".into(), status: crate::core::chat::todo::TodoStatus::InProgress }],
            }),
            json!({"type": "todo_update", "items": [{"content": "fix it", "status": "in_progress"}]})
        );
    }

    #[test]
    fn decision_is_read_for_read_tools() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("get_screen", json!({})).is_ok());
        assert!(ctx.call_tool("get_cwd", json!({})).is_ok());
        assert_eq!(decisions(&log), [named("get_screen", "read", true), named("get_cwd", "read", true)]);
    }

    /// `wait` is a read tool: it goes through `read_tool` with the "read" decision, and never shows an approval card.
    #[test]
    fn wait_is_a_read_tool_and_never_asks_for_approval() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("wait", json!({"seconds": 1})).is_ok());
        assert_eq!(decisions(&log), [named("wait", "read", true)]);
        assert_eq!(tools.reached(), ["wait"]);
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
    }

    #[test]
    fn decision_is_manual_when_the_user_approves() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        // The card's emit runs after the call is registered, so the answer is already waiting when `ask` reads it.
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, name, .. } = &ev {
                assert_eq!(name, "run_command", "the card names the tool");
                assert!(gate.decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(decisions(&log), [named("run_command", "manual", true)]);
        assert_eq!(tools.reached(), ["run_command"]);
    }

    #[test]
    fn decision_is_auto_when_auto_approve_skips_the_card() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(decisions(&log), [named("run_command", "auto", true)]);
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
    }

    #[test]
    fn decision_is_denied_when_nobody_approves_and_the_error_says_so() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("run_command", json!({"command": "pwd"})).expect_err("timed out");
        assert!(err.starts_with(DENIED), "{err}");
        assert_eq!(decisions(&log), [named("run_command", "denied", false)]);
        assert!(tools.reached().is_empty());
    }

    #[test]
    fn decision_is_refused_when_the_precheck_refuses_without_a_card() {
        let tools = Recorder { precheck: Some("prompt_has_user_text: typed".into()), ..Default::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("run_command", json!({"command": "pwd"})).expect_err("refused");
        assert!(!err.starts_with(DENIED), "a precheck refusal is not a denial: {err}");
        assert_eq!(decisions(&log), [named("run_command", "refused", false)]);
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
    }

    #[test]
    fn decision_is_refused_when_auto_approved_tool_refuses_and_manual_when_approved_tool_refuses() {
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);

        // Auto-approve skipped the card, then the tool refused: the user never saw it, so it is "refused".
        let tools = Recorder { call_error: Some("shell is busy".into()), ..Default::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("run_command", json!({"command": "pwd"})).expect_err("tool error");
        assert!(!err.starts_with(DENIED), "{err}");
        assert_eq!(decisions(&log), [named("run_command", "refused", false)]);

        // The user approved, then the tool refused: the approval happened, so it stays "manual".
        log.lock().unwrap().clear();
        let tools = Recorder { call_error: Some("shell is busy".into()), ..Default::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, .. } = &ev {
                assert!(gate.decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert_eq!(decisions(&log), [named("run_command", "manual", false)]);
    }

    #[test]
    fn cancel_turn_cancels_the_token_and_denies_pending_approvals() {
        let slot: Mutex<Option<Arc<TurnCancel>>> = Mutex::new(None);
        let gate = ApprovalGate::new();
        // No turn running: harmless, and repeatable.
        cancel_turn(&slot, &gate);
        cancel_turn(&slot, &gate);

        let token = Arc::new(TurnCancel::new());
        *slot.lock().unwrap() = Some(token.clone());
        let approved = thread::scope(|s| {
            let (tx, rx) = mpsc::channel();
            let gate_ref = &gate;
            // The waiter reports once its request is registered, so the cancel finds it pending.
            let waiter = s.spawn(move || gate_ref.ask("pending-call", move || tx.send(()).unwrap()));
            rx.recv().unwrap();
            cancel_turn(&slot, &gate);
            waiter.join().unwrap()
        });
        assert!(!approved, "a cancelled approval must count as a denial");
        assert!(token.is_cancelled());
        cancel_turn(&slot, &gate);
        assert!(token.is_cancelled());
        assert!(!gate.decide("pending-call", true), "the cancelled request is no longer pending");
    }

    #[test]
    fn read_tool_refuses_write_tools_without_reaching_the_backend() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("run_command", json!({"command": "pwd"})).is_err());
        assert!(ctx.read_tool("send_keys", json!({"bytes": [13]})).is_err());
        assert!(tools.reached().is_empty(), "a write tool reached the backend: {:?}", tools.reached());
        assert!(ctx.read_tool("get_screen", json!({})).is_ok());
        assert_eq!(tools.reached(), vec!["get_screen".to_string()]);
    }

    #[test]
    fn call_tool_never_reaches_the_backend_without_approval() {
        let tools = Recorder::default();
        // No one answers, so the request times out and counts as a denial.
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("send_keys", json!({"bytes": [13]})).is_err());
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert!(tools.reached().is_empty(), "an unapproved call reached the backend: {:?}", tools.reached());
    }

    #[test]
    fn call_tool_skips_approval_for_read_tools() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("get_cwd", json!({})).is_ok());
        assert_eq!(tools.reached(), vec!["get_cwd".to_string()]);
    }

    /// One label per event, so the order of the calls and the control bracket can be checked in one go.
    fn labels(log: &Mutex<Vec<AgentEvent>>) -> Vec<String> {
        log.lock()
            .unwrap()
            .iter()
            .map(|e| match e {
                AgentEvent::ToolCall { name, .. } => format!("call {name}"),
                AgentEvent::ToolResult { name, .. } => format!("result {name}"),
                AgentEvent::ApprovalRequest { .. } => "approval".to_string(),
                AgentEvent::AgentControl { active } => format!("control {active}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn agent_control_sits_between_an_approved_shell_call_and_its_result() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(labels(&log), ["call run_command", "control true", "control false", "result run_command"]);
    }

    #[test]
    fn agent_control_is_never_sent_for_read_tools_or_unapproved_calls() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("get_screen", json!({})).is_ok());
        assert!(ctx.call_tool("get_cwd", json!({})).is_ok());
        // Nobody answers: the approval times out and the call never reaches the tools.
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert!(labels(&log).iter().all(|l| !l.starts_with("control")), "{:?}", labels(&log));
        assert_eq!(tools.reached(), vec!["get_screen".to_string(), "get_cwd".to_string()]);
    }

    /// `(name, decision, reason)` of every ToolResult, in order.
    fn reasons(log: &Mutex<Vec<AgentEvent>>) -> Vec<(String, &'static str, Option<&'static str>)> {
        log.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolResult { name, decision, reason, .. } => Some((name.clone(), *decision, *reason)),
                _ => None,
            })
            .collect()
    }

    fn regex_policy(allow: &[&str]) -> RegexPolicy {
        let approval = ApprovalSettings {
            mode: ApprovalMode::Regex,
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: Vec::new(),
            ..Default::default()
        };
        RegexPolicy::new(&approval).expect("the rules compile")
    }

    #[test]
    fn a_regex_rule_runs_a_call_without_a_card_and_the_result_says_why() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let policy = regex_policy(&["echo .*"]);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        assert!(ctx.call_tool("run_command", json!({"command": "echo hi"})).is_ok());
        assert!(ctx.read_tool("get_screen", json!({})).is_ok());
        assert_eq!(
            reasons(&log),
            [("run_command".to_string(), "auto", Some("rule")), ("get_screen".to_string(), "read", None)]
        );
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
        assert_eq!(tools.reached(), ["run_command", "get_screen"]);
    }

    #[test]
    fn a_gated_command_asks_with_the_gate_reason_and_is_manual_when_approved() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        // The rule would match, but the hard gate refuses the pipe first.
        let policy = regex_policy(&[".*"]);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, reason, .. } = &ev {
                assert_eq!(*reason, Some("gate_chars"));
                assert!(gate.decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        assert!(ctx.call_tool("run_command", json!({"command": "ls | wc"})).is_ok());
        assert_eq!(reasons(&log), [("run_command".to_string(), "manual", Some("gate_chars"))]);
        assert_eq!(tools.reached(), ["run_command"]);
    }

    #[test]
    fn a_call_nobody_answers_is_denied_with_its_reason_and_never_reaches_the_tools() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let policy = regex_policy(&["echo .*"]);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        let err = ctx.call_tool("run_command", json!({"command": "git status"})).expect_err("timed out");
        assert!(err.starts_with(DENIED), "{err}");
        assert_eq!(reasons(&log), [("run_command".to_string(), "denied", Some("no_rule"))]);
        assert!(tools.reached().is_empty());
    }

    #[test]
    fn approve_all_skips_the_policy_and_says_approve_all_but_send_keys_still_asks() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        // No rules: the policy would ask for everything, and the gate would refuse the pipe.
        let policy = regex_policy(&[]);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        assert!(ctx.call_tool("run_command", json!({"command": "ls | wc"})).is_ok());
        assert!(ctx.call_tool("send_keys", json!({"bytes": [13]})).is_err());
        assert_eq!(
            reasons(&log),
            [("run_command".to_string(), "auto", Some("approve_all")), ("send_keys".to_string(), "denied", Some("send_keys"))]
        );
        let asked_for_run_command = log
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequest { name, .. } if name == "run_command"));
        assert!(!asked_for_run_command, "approve-all showed a card for run_command");
        assert_eq!(tools.reached(), ["run_command"]);
    }

    #[test]
    fn the_ninth_automatic_call_in_a_turn_asks_with_auto_cap() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let policy = regex_policy(&["echo .*"]);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, reason, .. } = &ev {
                assert_eq!(*reason, Some("auto_cap"));
                assert!(gate.decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        for n in 0..MAX_AUTO_PER_TURN {
            assert!(ctx.call_tool("run_command", json!({"command": format!("echo {n}")})).is_ok());
        }
        assert!(ctx.call_tool("run_command", json!({"command": "echo over"})).is_ok());
        let got = reasons(&log);
        assert_eq!(got.len(), MAX_AUTO_PER_TURN + 1);
        assert!(got[..MAX_AUTO_PER_TURN].iter().all(|(_, d, r)| *d == "auto" && *r == Some("rule")), "{got:?}");
        assert_eq!(got[MAX_AUTO_PER_TURN], ("run_command".to_string(), "manual", Some("auto_cap")));
        assert_eq!(tools.reached().len(), MAX_AUTO_PER_TURN + 1);
    }

    /// "still running" only for a command that has started (it has a record id). Otherwise the state is named; not-run
    /// outcomes say why.
    #[test]
    fn run_command_outcomes_are_described_truthfully() {
        let text = |v: Value| outcome_text("sleep 30", &v);
        assert_eq!(
            text(json!({"outcome": "running", "record_id": 4, "state": "running"})),
            "`sleep 30` is still running after the wait. It keeps running in the terminal; ask for the last command once it finishes."
        );
        let submitted = text(json!({"outcome": "running", "record_id": null, "state": "submitted"}));
        assert!(!submitted.contains("still running"), "{submitted}");
        assert!(submitted.contains("shell state: submitted"), "{submitted}");
        let abandoned = text(json!({"outcome": "not_run", "reason": "abandoned"}));
        assert!(abandoned.contains("did not run") && abandoned.contains("went back to the prompt"), "{abandoned}");
        let waiting = text(json!({"outcome": "not_run", "reason": "waiting_for_input"}));
        assert!(waiting.contains("waiting for the rest") && waiting.contains("Press Ctrl-C"), "{waiting}");
        assert!(!waiting.contains("still running"), "{waiting}");
        let cancelled = text(json!({"outcome": "not_run", "reason": "cancelled"}));
        assert!(cancelled.contains("you cancelled it") && cancelled.contains("keeps running"), "{cancelled}");
        assert!(!cancelled.contains("waiting for the rest"), "{cancelled}");
    }

    /// The context reads the hint from its tools each time it is asked: the agent loop asks once per model call.
    #[test]
    fn the_context_reports_the_tools_foreground_hint() {
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let plain = Recorder::default();
        assert_eq!(AgentCtx::new(&plain, &gate, &emit).foreground(), None);
        let ssh = Recorder {
            foreground: Some(Foreground { kind: ForegroundKind::Ssh, command: "ssh h".into() }),
            ..Default::default()
        };
        let ctx = AgentCtx::new(&ssh, &gate, &emit);
        assert_eq!(ctx.foreground().map(|f| f.command), Some("ssh h".to_string()));
        assert_eq!(ctx.foreground().map(|f| f.kind), Some(ForegroundKind::Ssh));
    }

    #[test]
    fn on_drop_runs_when_the_body_panics() {
        let ran = std::cell::Cell::new(false);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = OnDrop::new(|| ran.set(true));
            panic!("turn crashed");
        }));
        assert!(result.is_err());
        assert!(ran.get(), "the guard did not run while the panic unwound");
    }

    /// The card is for the shell on top: its key, and its label when it is a nested shell. The approved command carries the
    /// same key to the tools, so the submit can be bound to it.
    #[test]
    fn the_card_and_the_approved_call_carry_the_level_they_were_made_for() {
        let tools = Recorder::default();
        tools.set_top(nested(3, "ssh ubuntu@lidia"));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let cards = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, level, location, .. } = &ev {
                cards.lock().unwrap().push((*level, location.clone()));
                assert!(gate.decide(call_id, true));
            }
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(*cards.lock().unwrap(), [(3, Some("ssh ubuntu@lidia".to_string()))]);
        assert_eq!(*tools.bound_levels.lock().unwrap(), [Some(3)]);
    }

    /// The local shell's card has key 0 and no location, and the call is bound to key 0.
    #[test]
    fn a_local_call_is_bound_to_the_local_level() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(*tools.bound_levels.lock().unwrap(), [Some(0)]);
    }

    /// Approve-all switched on at the local level does not run commands in a nested shell: they ask, with the suspended
    /// reason. Back on the local shell it applies again.
    #[test]
    fn approve_all_is_suspended_in_a_nested_shell_and_resumes_after_the_pop() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);

        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        tools.set_top(nested(1, "ssh h"));
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err(), "nobody answers the card");
        tools.set_top(TopLevel::default());
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());

        assert_eq!(
            reasons(&log),
            [
                ("run_command".to_string(), "auto", Some("approve_all")),
                ("run_command".to_string(), "denied", Some("approve_all_suspended")),
                ("run_command".to_string(), "auto", Some("approve_all")),
            ]
        );
        assert_eq!(*tools.bound_levels.lock().unwrap(), [Some(0), Some(0)], "the suspended call never reached the tools");
    }

    /// Approve-all switched on inside a nested shell does not carry into a later nested shell with another key.
    #[test]
    fn approve_all_switched_on_in_a_nested_shell_does_not_carry_to_the_next_one() {
        let tools = Recorder::default();
        tools.set_top(nested(1, "ssh a"));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 1);
        assert_eq!(gate.auto_state(1), AutoState::On);
        assert_eq!(gate.auto_state(2), AutoState::Suspended, "a new level with a new key");
        assert_eq!(gate.auto_state(0), AutoState::Suspended);

        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        tools.set_top(nested(2, "ssh b"));
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert_eq!(
            reasons(&log),
            [
                ("run_command".to_string(), "auto", Some("approve_all")),
                ("run_command".to_string(), "denied", Some("approve_all_suspended")),
            ]
        );
        gate.set_auto_approve(false, 0);
        assert_eq!(gate.auto_state(1), AutoState::Off);
    }

    /// A regex allow rule runs a command on the local shell, and asks with `nested_level` in a nested one. The rule's
    /// verdict is what the card says, and nothing runs when nobody approves.
    #[test]
    fn a_regex_rule_asks_in_a_nested_shell_with_the_nested_reason() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let policy = regex_policy(&["echo .*"]);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        assert!(ctx.call_tool("run_command", json!({"command": "echo local"})).is_ok());

        tools.set_top(nested(1, "ssh h"));
        let err = ctx.call_tool("run_command", json!({"command": "echo remote"})).expect_err("asks");
        assert!(err.starts_with(DENIED), "{err}");
        assert_eq!(
            reasons(&log),
            [
                ("run_command".to_string(), "auto", Some("rule")),
                ("run_command".to_string(), "denied", Some("nested_level")),
            ]
        );
        assert_eq!(tools.reached(), ["run_command"], "the nested call never reached the tools");
    }

    /// The card names the nested reason, and approving it runs the call bound to that level.
    #[test]
    fn an_approved_nested_call_is_bound_to_its_level() {
        let tools = Recorder::default();
        tools.set_top(nested(4, "ssh h"));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let policy = regex_policy(&["echo .*"]);
        let reasons_shown = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, reason, .. } = &ev {
                reasons_shown.lock().unwrap().push(*reason);
                assert!(gate.decide(call_id, true));
            }
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        assert!(ctx.call_tool("run_command", json!({"command": "echo hi"})).is_ok());
        assert_eq!(*reasons_shown.lock().unwrap(), [Some("nested_level")]);
        assert_eq!(*tools.bound_levels.lock().unwrap(), [Some(4)]);
    }

    /// A deny rule still asks first, and in a nested shell its reason is the deny rule's, not the nested one.
    #[test]
    fn a_deny_rule_keeps_its_reason_in_a_nested_shell() {
        let tools = Recorder::default();
        tools.set_top(nested(1, "ssh h"));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let approval = ApprovalSettings {
            mode: ApprovalMode::Regex,
            allow: vec![".*".into()],
            deny: vec!["cargo .*".into()],
            ..Default::default()
        };
        let policy = RegexPolicy::new(&approval).expect("the rules compile");
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        assert!(ctx.call_tool("run_command", json!({"command": "cargo test"})).is_err());
        assert_eq!(reasons(&log), [("run_command".to_string(), "denied", Some("deny_rule"))]);
    }

    // ---------- request_integration and the turn rules ----------

    fn integrated() -> IntegrateOutcome {
        IntegrateOutcome::Integrated {
            key: 1,
            label: "ssh h".into(),
            sh: "bash".into(),
            user: Some("tester".into()),
            host: Some("127.0.0.1".into()),
        }
    }

    /// The phases of the integration events, in order, with whether each carries a label and its outcome.
    fn integration_events(log: &Mutex<Vec<AgentEvent>>) -> Vec<(&'static str, bool, Option<Value>)> {
        log.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Integration { phase, label, outcome, .. } => Some((*phase, label.is_some(), outcome.clone())),
                _ => None,
            })
            .collect()
    }

    fn phases(log: &Mutex<Vec<AgentEvent>>) -> Vec<&'static str> {
        integration_events(log).into_iter().map(|e| e.0).collect()
    }

    /// With auto, the shell is integrated at once: no card, no question, and the result is the integrated text.
    #[test]
    fn request_integration_with_auto_runs_without_asking() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let rules = TurnRules { integrate_without_asking: true, ..TurnRules::default() };
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_rules(rules);
        let result = ctx.call_tool("request_integration", json!({})).expect("integrated");
        assert_eq!(result, json!("Integrated ssh h (bash as tester@127.0.0.1). You can now use run_command in this shell."));
        assert_eq!(*tools.integrate_calls.lock().unwrap(), 1);
        assert_eq!(phases(&log), ["running", "done"], "no asking event");
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
        assert_eq!(decisions(&log), [named("request_integration", "auto", true)]);
    }

    /// Without auto the user is asked first. The answer is given in the emit callback: the ask is registered before its event
    /// goes out, so an answer given there is already waiting.
    #[test]
    fn request_integration_asks_first_and_runs_when_accepted() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::Integration { call_id, phase: "asking", label, .. } = &ev {
                assert_eq!(label.as_deref(), Some("ssh h"));
                assert!(gate.integrations().decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("request_integration", json!({})).is_ok());
        assert_eq!(phases(&log), ["asking", "running", "done"]);
        assert_eq!(*tools.integrate_calls.lock().unwrap(), 1);
        assert_eq!(decisions(&log), [named("request_integration", "manual", true)]);
    }

    /// A declined ask is refused: the handshake never runs, the decision is "denied", and the error says what the agent can
    /// do instead. It is not a DENIED error, so the turn does not stop (see the agent loop's tests).
    #[test]
    fn a_declined_integration_never_integrates_and_is_not_a_denial_error() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::Integration { call_id, phase: "asking", .. } = &ev {
                assert!(gate.integrations().decide(call_id, false));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("request_integration", json!({})).expect_err("declined");
        assert!(!err.starts_with(DENIED), "a declined integration must not stop the turn: {err}");
        assert!(err.starts_with("declined: the user did not integrate this shell"), "{err}");
        assert!(err.contains("Do not ask again in this turn") && err.contains("get_screen"), "{err}");
        assert_eq!(*tools.integrate_calls.lock().unwrap(), 0);
        assert_eq!(phases(&log), ["asking", "done"]);
        assert_eq!(integration_events(&log)[1].2, Some(json!({"outcome": "declined"})));
        assert_eq!(decisions(&log), [named("request_integration", "denied", false)]);
    }

    /// A cancel while the dialog is open declines it, and so does an ask nobody answers in time. Neither stops the turn.
    #[test]
    fn a_cancelled_or_unanswered_integration_ask_is_declined() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if matches!(ev, AgentEvent::Integration { phase: "asking", .. }) {
                gate.cancel_all();
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("request_integration", json!({})).expect_err("cancelled");
        assert!(err.starts_with("declined: "), "{err}");
        assert!(!err.starts_with(DENIED), "{err}");
        assert_eq!(phases(&log), ["asking", "done"]);

        let quiet = ApprovalGate::with_timeout(Duration::from_millis(30));
        let silent = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &quiet, &silent);
        let err = ctx.call_tool("request_integration", json!({})).expect_err("timed out");
        assert!(err.starts_with("declined: "), "{err}");
        assert!(!err.starts_with(DENIED), "{err}");
        assert_eq!(*tools.integrate_calls.lock().unwrap(), 0);
    }

    /// Approve-all is for commands: it does not answer an integration ask.
    #[test]
    fn approve_all_does_not_integrate_a_shell() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::Integration { call_id, phase: "asking", .. } = &ev {
                assert!(gate.integrations().decide(call_id, false));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("request_integration", json!({})).is_err());
        assert_eq!(phases(&log), ["asking", "done"], "approve-all still asks");
        assert_eq!(*tools.integrate_calls.lock().unwrap(), 0);
    }

    /// A refused precheck is an error with no card and no question, and the handshake does not run.
    #[test]
    fn a_refused_integration_never_asks_and_says_why() {
        let tools = Recorder { integration_refusal: Some("cannot integrate the shell now: nothing".into()), ..Recorder::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("request_integration", json!({})).expect_err("refused");
        assert_eq!(err, "cannot integrate the shell now: nothing");
        assert!(phases(&log).is_empty());
        assert_eq!(*tools.integrate_calls.lock().unwrap(), 0);
        assert_eq!(decisions(&log), [named("request_integration", "refused", false)]);
    }

    /// Accepted, then the handshake fails: the user did approve, so the decision stays manual, and the result is an error that
    /// is not a denial.
    #[test]
    fn a_failed_handshake_after_acceptance_stays_manual_and_is_an_error() {
        let tools = Recorder { integrate_outcome: Mutex::new(Some(IntegrateOutcome::Timeout)), ..Recorder::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::Integration { call_id, phase: "asking", .. } = &ev {
                assert!(gate.integrations().decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("request_integration", json!({})).expect_err("timed out");
        assert!(!err.starts_with(DENIED), "{err}");
        assert!(err.contains("timed out") && err.contains("request_integration again"), "{err}");
        assert_eq!(decisions(&log), [named("request_integration", "manual", false)]);
    }

    /// The result of an integration the user accepted: the outcome and the events carry the label on the asking and running
    /// phases only.
    #[test]
    fn the_integration_events_carry_the_label_and_the_outcome() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::Integration { call_id, phase: "asking", .. } = &ev {
                assert!(gate.integrations().decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("request_integration", json!({})).is_ok());
        let events = integration_events(&log);
        assert_eq!(events[0], ("asking", true, None));
        assert_eq!(events[1], ("running", true, None));
        assert_eq!(events[2].0, "done");
        assert!(!events[2].1, "done carries no label");
        assert_eq!(events[2].2.as_ref().and_then(|o| o.get("outcome")), Some(&json!("integrated")));
    }

    /// The wire form: the type is `integration_request` (not the session's `integration` status event), and the optional
    /// fields are left out when absent.
    #[test]
    fn the_integration_event_serializes_with_its_phase_and_optional_fields() {
        let asking = AgentEvent::Integration {
            call_id: "call-9".into(),
            phase: "asking",
            label: Some("ssh h".into()),
            outcome: None,
        };
        assert_eq!(
            serde_json::to_value(&asking).unwrap(),
            json!({"type": "integration_request", "call_id": "call-9", "phase": "asking", "label": "ssh h"})
        );
        let done = AgentEvent::Integration { call_id: "call-9".into(), phase: "done", label: None, outcome: Some(json!({"outcome": "declined"})) };
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            json!({"type": "integration_request", "call_id": "call-9", "phase": "done", "outcome": {"outcome": "declined"}})
        );
    }

    /// With approval nested = same as local, an allow rule runs a command in a nested shell without a card, as it does on the
    /// local shell. The reason is the rule.
    #[test]
    fn an_allow_rule_runs_in_a_nested_shell_when_nested_follows_local() {
        let tools = Recorder::default();
        tools.set_top(nested(1, "ssh h"));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let policy = regex_policy(&["echo .*"]);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let rules = TurnRules { follow_local_in_nested: true, ..TurnRules::default() };
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy).with_rules(rules);
        assert!(ctx.call_tool("run_command", json!({"command": "echo remote"})).is_ok());
        assert_eq!(reasons(&log), [("run_command".to_string(), "auto", Some("rule"))]);
        assert_eq!(tools.reached(), ["run_command"]);
    }

    /// Following the local rules does not loosen the floor: the hard gate and the deny rules still refuse in a nested shell.
    #[test]
    fn the_hard_gate_and_deny_rules_still_apply_when_nested_follows_local() {
        let tools = Recorder::default();
        tools.set_top(nested(1, "ssh h"));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let policy = RegexPolicy::new(&ApprovalSettings {
            mode: ApprovalMode::Regex,
            allow: vec![".*".into()],
            deny: vec!["cargo .*".into()],
            ..ApprovalSettings::default()
        })
        .expect("rules compile");
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let rules = TurnRules { follow_local_in_nested: true, ..TurnRules::default() };
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy).with_rules(rules);
        assert!(ctx.call_tool("run_command", json!({"command": "sudo ls"})).is_err());
        assert!(ctx.call_tool("run_command", json!({"command": "cargo test"})).is_err());
        assert_eq!(
            reasons(&log),
            [
                ("run_command".to_string(), "denied", Some("gate_denylist")),
                ("run_command".to_string(), "denied", Some("deny_rule")),
            ]
        );
        assert!(tools.reached().is_empty(), "nothing reached the tools");
    }

    /// Approve-all switched on in the local shell applies in a nested shell when nested follows local. Without that setting it
    /// is suspended there, as before.
    #[test]
    fn approve_all_carries_into_a_nested_shell_when_nested_follows_local() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true, 0);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        tools.set_top(nested(1, "ssh h"));

        let follow = AgentCtx::new(&tools, &gate, &emit).with_rules(TurnRules { follow_local_in_nested: true, ..TurnRules::default() });
        assert!(follow.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(reasons(&log), [("run_command".to_string(), "auto", Some("approve_all"))]);

        let asking = AgentCtx::new(&tools, &gate, &emit);
        assert!(asking.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert_eq!(reasons(&log)[1], ("run_command".to_string(), "denied", Some("approve_all_suspended")));
        assert_eq!(tools.reached(), ["run_command"]);
    }
}
