//! The terminals of one agent turn. A chat may use several. Each is a `Terminal` (the app's tab; the app implements the
//! trait), and the tools of a turn are `ChatTerminals`: it picks the terminal a call is for, and reads the rest from the
//! default one. The core knows a terminal only through this trait, so everything here is testable with fakes.
//!
//! The terminal set is read on every call, so a terminal assigned or unassigned during a turn is seen by the next call.
//! A view of a terminal owns its `Arc`, so a call in flight keeps working when the terminal leaves the set.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::core::foreground::Foreground;
use crate::core::keys::KeyPlan;
use crate::core::observer::{InputState, IntegrationStatus, ObserverStatus, OpenAgentRecord};
use crate::core::record::{Cwd, RecordSummary};
use crate::core::session::IntegrateOutcome;
use crate::core::tools::{
    AgentTools, CommandView, RunOutcome, ScreenView, SendKeysOutcome, SendTarget, SessionFacts, TopLevel, TurnBudget, WaitView,
};

/// The tools that act on one terminal. `ChatTerminals::scoped` routes a call to one of them, by its `terminal` argument or
/// the record it names.
pub const TERMINAL_TOOLS: [&str; 8] =
    ["list_commands", "get_command", "get_screen", "get_cwd", "wait", "run_command", "request_integration", "send_keys"];

/// What a terminal tool says when the chat has no terminal. Nothing runs.
pub const NO_TERMINAL: &str = "no terminal is assigned to this chat; ask the user to assign one";

/// Said for `run_command` and `wait` when they reach the chat's own tools rather than a terminal's view. Only a terminal's view
/// may act on a shell, so this never runs anything.
const NOT_ROUTED: &str = "this call did not name a terminal; it was not run";

/// One terminal of a chat: a shell the agent may act in. The app implements it for its tabs.
pub trait Terminal: Send + Sync {
    /// The id the model names in `terminal`, and that `list_terminals` gives.
    fn id(&self) -> String;
    /// The title `list_terminals` gives, or None when there is none.
    fn title(&self) -> Option<String>;
    /// The tools over this terminal, for one call. `budget` is the turn's, shared by every terminal of the turn.
    fn view(self: Arc<Self>, budget: Arc<TurnBudget>) -> Box<dyn AgentTools>;
}

/// Reads the terminal set of the chat, in order: the first terminal is the default.
type Provider = Box<dyn Fn() -> Vec<Arc<dyn Terminal>> + Send + Sync>;

/// The tools of one agent turn over the chat's terminals (see the module docs).
pub struct ChatTerminals {
    provider: Provider,
    /// The turn's wait and `send_keys` budget, shared by the views of every terminal.
    budget: Arc<TurnBudget>,
}

impl ChatTerminals {
    /// `terminals` gives the chat's terminals now, oldest first. `wait_budget` is the turn's `wait` limit.
    pub fn new(terminals: impl Fn() -> Vec<Arc<dyn Terminal>> + Send + Sync + 'static, wait_budget: Duration) -> Self {
        ChatTerminals { provider: Box::new(terminals), budget: Arc::new(TurnBudget::new(wait_budget)) }
    }

    /// The ids of the chat's terminals now, the default first.
    pub fn ids(&self) -> Vec<String> {
        self.current().iter().map(|t| t.id()).collect()
    }

    fn current(&self) -> Vec<Arc<dyn Terminal>> {
        (self.provider)()
    }

    /// The view of the default terminal. With no terminal, a view that says so.
    fn default_view(&self) -> Box<dyn AgentTools> {
        match self.current().into_iter().next() {
            Some(terminal) => terminal.view(self.budget.clone()),
            None => Box::new(NoTerminal),
        }
    }

    /// `list_terminals`: one object per terminal, in order. `state`, `cwd` and `location` come from the terminal's view.
    fn list(&self) -> Value {
        let terminals = self.current();
        Value::Array(
            terminals
                .iter()
                .enumerate()
                .map(|(i, terminal)| {
                    let view = terminal.clone().view(self.budget.clone());
                    let facts = view.session_facts();
                    json!({
                        "id": terminal.id(),
                        "title": terminal.title(),
                        "state": facts.as_ref().map(|f| f.state),
                        "cwd": facts.as_ref().and_then(|f| f.cwd.clone()),
                        "location": view.top_level().location,
                        "default": i == 0,
                    })
                })
                .collect(),
        )
    }
}

/// The `terminal` argument, if given. Not a string is an error.
fn explicit_terminal(args: &Value) -> Result<Option<&str>, String> {
    match args.get("terminal") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().map(Some).ok_or_else(|| "argument `terminal` must be a string".to_string()),
    }
}

/// The terminal that owns record `id`, open or finished, if any. Record ids are process-wide, so at most one owns it.
fn owner(terminals: &[Arc<dyn Terminal>], budget: &Arc<TurnBudget>, id: u64) -> Option<Arc<dyn Terminal>> {
    terminals
        .iter()
        .find(|terminal| {
            let view = Arc::clone(terminal).view(budget.clone());
            view.finished_summary(id).is_some() || view.open_record_ids().contains(&id)
        })
        .cloned()
}

impl AgentTools for ChatTerminals {
    /// Routes a terminal tool to one terminal: the `terminal` argument wins, else the terminal that owns the record `id`
    /// names, else the default. A chat with no terminal refuses the call, and so does an unknown `terminal`.
    fn scoped(&self, name: &str, args: &Value) -> Result<Option<Box<dyn AgentTools + '_>>, String> {
        if !TERMINAL_TOOLS.contains(&name) {
            return Ok(None);
        }
        let terminals = self.current();
        let Some(first) = terminals.first() else {
            return Err(NO_TERMINAL.into());
        };
        let chosen = match explicit_terminal(args)? {
            Some(id) => terminals
                .iter()
                .find(|t| t.id() == id)
                .cloned()
                .ok_or_else(|| format!("no terminal '{id}' in this chat; call list_terminals"))?,
            None => match args.get("id").and_then(Value::as_u64).and_then(|id| owner(&terminals, &self.budget, id)) {
                Some(terminal) => terminal,
                None => first.clone(),
            },
        };
        // With two or more terminals the result names its terminal (see `TerminalView::call`). One terminal: unchanged.
        let tag = (terminals.len() >= 2).then(|| chosen.id());
        Ok(Some(Box::new(TerminalView { tools: chosen.view(self.budget.clone()), tag })))
    }

    fn call(&self, name: &str, args: &Value, expected_level: Option<u64>) -> Result<Value, String> {
        if name == "list_terminals" {
            return Ok(self.list());
        }
        match self.scoped(name, args)? {
            Some(view) => view.call(name, args, expected_level),
            None => Err(format!("unknown tool `{name}`")),
        }
    }

    fn list_commands(&self, n: usize) -> Vec<RecordSummary> {
        self.default_view().list_commands(n)
    }

    fn get_command(&self, id: u64, offset: usize, limit: usize) -> Result<CommandView, String> {
        self.default_view().get_command(id, offset, limit)
    }

    fn get_screen(&self) -> ScreenView {
        self.default_view().get_screen()
    }

    fn get_cwd(&self) -> Option<Cwd> {
        self.default_view().get_cwd()
    }

    /// Never runs: a shell action goes through `call`, which names its terminal first.
    fn run_command(&self, _command: &str, _wait: Duration, _expected_level: Option<u64>) -> Result<RunOutcome, String> {
        Err(NOT_ROUTED.into())
    }

    /// Never waits: a wait goes through `call`, which names its terminal first.
    fn wait(&self, _seconds: u64, _ignore_updates: bool, _record: Option<u64>) -> Result<WaitView, String> {
        Err(NOT_ROUTED.into())
    }

    fn open_agent_records(&self) -> Vec<OpenAgentRecord> {
        self.default_view().open_agent_records()
    }

    /// The open commands of every terminal: the user's and the agent's, on all of them.
    fn open_record_ids(&self) -> Vec<u64> {
        self.current().into_iter().flat_map(|t| t.view(self.budget.clone()).open_record_ids()).collect()
    }

    fn finished_summary(&self, id: u64) -> Option<RecordSummary> {
        self.current().into_iter().find_map(|t| t.view(self.budget.clone()).finished_summary(id))
    }

    fn foreground(&self) -> Option<Foreground> {
        self.default_view().foreground()
    }

    fn foreground_age(&self) -> Option<Duration> {
        self.default_view().foreground_age()
    }

    fn top_level(&self) -> TopLevel {
        self.default_view().top_level()
    }

    fn session_facts(&self) -> Option<SessionFacts> {
        self.default_view().session_facts()
    }
}

/// The tools of a chat with no terminal. It has no shell: the terminal tools never reach it (`scoped` refuses them first),
/// and the aggregate reads give what a chat without a shell gives.
struct NoTerminal;

impl AgentTools for NoTerminal {
    fn list_commands(&self, _n: usize) -> Vec<RecordSummary> {
        Vec::new()
    }

    fn get_command(&self, _id: u64, _offset: usize, _limit: usize) -> Result<CommandView, String> {
        Err(NO_TERMINAL.into())
    }

    /// An empty screen in the exited state: there is no shell to show.
    fn get_screen(&self) -> ScreenView {
        ScreenView {
            rows: Vec::new(),
            cursor: (0, 0),
            alternate_screen: false,
            size: (0, 0),
            state: InputState::Exited,
            observer_status: ObserverStatus::Ok,
            integration_status: IntegrationStatus::Raw,
            foreground: None,
            location: None,
            history: None,
        }
    }

    fn get_cwd(&self) -> Option<Cwd> {
        None
    }

    fn run_command(&self, _command: &str, _wait: Duration, _expected_level: Option<u64>) -> Result<RunOutcome, String> {
        Err(NO_TERMINAL.into())
    }

    fn wait(&self, _seconds: u64, _ignore_updates: bool, _record: Option<u64>) -> Result<WaitView, String> {
        Err(NO_TERMINAL.into())
    }
}

/// The tools over one terminal for one call. It delegates everything to the terminal's own tools. When the chat has two or
/// more terminals, `call` names the terminal in the result: an object gets a `terminal` field; anything else (`list_commands`
/// returns a list, `get_cwd` may return null) is wrapped as `{"terminal": id, "result": value}`. Core/chat/project reads both.
/// Errors are not tagged: they are text.
struct TerminalView {
    tools: Box<dyn AgentTools>,
    tag: Option<String>,
}

impl AgentTools for TerminalView {
    fn list_commands(&self, n: usize) -> Vec<RecordSummary> {
        self.tools.list_commands(n)
    }

    fn get_command(&self, id: u64, offset: usize, limit: usize) -> Result<CommandView, String> {
        self.tools.get_command(id, offset, limit)
    }

    fn get_screen(&self) -> ScreenView {
        self.tools.get_screen()
    }

    fn get_screen_lines(&self, lines: usize) -> ScreenView {
        self.tools.get_screen_lines(lines)
    }

    fn open_agent_records(&self) -> Vec<OpenAgentRecord> {
        self.tools.open_agent_records()
    }

    fn open_record_ids(&self) -> Vec<u64> {
        self.tools.open_record_ids()
    }

    fn finished_summary(&self, id: u64) -> Option<RecordSummary> {
        self.tools.finished_summary(id)
    }

    fn get_cwd(&self) -> Option<Cwd> {
        self.tools.get_cwd()
    }

    fn foreground(&self) -> Option<Foreground> {
        self.tools.foreground()
    }

    fn foreground_age(&self) -> Option<Duration> {
        self.tools.foreground_age()
    }

    fn top_level(&self) -> TopLevel {
        self.tools.top_level()
    }

    fn run_command(&self, command: &str, wait: Duration, expected_level: Option<u64>) -> Result<RunOutcome, String> {
        self.tools.run_command(command, wait, expected_level)
    }

    fn wait(&self, seconds: u64, ignore_updates: bool, record: Option<u64>) -> Result<WaitView, String> {
        self.tools.wait(seconds, ignore_updates, record)
    }

    fn run_command_precheck(&self) -> Result<(), String> {
        self.tools.run_command_precheck()
    }

    fn take_typed_cleared(&self) -> bool {
        self.tools.take_typed_cleared()
    }

    fn session_facts(&self) -> Option<SessionFacts> {
        self.tools.session_facts()
    }

    fn integration_precheck(&self) -> Result<String, String> {
        self.tools.integration_precheck()
    }

    fn integrate_shell(&self) -> IntegrateOutcome {
        self.tools.integrate_shell()
    }

    fn send_keys_precheck(&self, plan: &KeyPlan) -> Result<SendTarget, String> {
        self.tools.send_keys_precheck(plan)
    }

    fn user_writes(&self) -> u64 {
        self.tools.user_writes()
    }

    fn send_keys(&self, plan: &KeyPlan, record: Option<u64>, expected_level: Option<u64>) -> Result<SendKeysOutcome, String> {
        self.tools.send_keys(plan, record, expected_level)
    }

    fn call(&self, name: &str, args: &Value, expected_level: Option<u64>) -> Result<Value, String> {
        let value = self.tools.call(name, args, expected_level)?;
        Ok(match &self.tag {
            None => value,
            Some(id) => match value {
                Value::Object(mut fields) => {
                    fields.insert("terminal".into(), json!(id));
                    Value::Object(fields)
                }
                other => json!({"terminal": id, "result": other}),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::core::agent::{AgentCtx, AgentEvent, ApprovalGate};
    use crate::core::tools::SEND_KEYS_PER_TURN;

    /// What the fake views log: (terminal id, tool name, the level the call was bound to).
    type Log = Arc<Mutex<Vec<(String, String, Option<u64>)>>>;

    /// A fake terminal: its top level, whether its precheck refuses, the records it owns, and what a wait costs.
    struct Fake {
        id: &'static str,
        title: Option<&'static str>,
        top: TopLevel,
        precheck: Mutex<Option<String>>,
        records: Vec<u64>,
        wait_cost: Duration,
        log: Log,
    }

    impl Fake {
        fn new(id: &'static str, top: TopLevel, log: &Log) -> Arc<Fake> {
            Arc::new(Fake { id, title: None, top, precheck: Mutex::new(None), records: Vec::new(), wait_cost: Duration::ZERO, log: log.clone() })
        }
    }

    impl Terminal for Fake {
        fn id(&self) -> String {
            self.id.to_string()
        }

        fn title(&self) -> Option<String> {
            self.title.map(str::to_string)
        }

        fn view(self: Arc<Self>, budget: Arc<TurnBudget>) -> Box<dyn AgentTools> {
            Box::new(FakeView { term: self, budget })
        }
    }

    struct FakeView {
        term: Arc<Fake>,
        budget: Arc<TurnBudget>,
    }

    impl AgentTools for FakeView {
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

        fn run_command(&self, _command: &str, _wait: Duration, _expected_level: Option<u64>) -> Result<RunOutcome, String> {
            Err("not used".into())
        }

        fn wait(&self, _seconds: u64, _ignore_updates: bool, _record: Option<u64>) -> Result<WaitView, String> {
            Err("not used".into())
        }

        fn run_command_precheck(&self) -> Result<(), String> {
            self.term.precheck.lock().unwrap().clone().map_or(Ok(()), Err)
        }

        fn top_level(&self) -> TopLevel {
            self.term.top.clone()
        }

        fn open_record_ids(&self) -> Vec<u64> {
            self.term.records.clone()
        }

        fn session_facts(&self) -> Option<SessionFacts> {
            Some(SessionFacts {
                state: InputState::Prompt,
                integration: IntegrationStatus::Live,
                observer: ObserverStatus::Ok,
                size: (24, 80),
                cwd: None,
            })
        }

        fn call(&self, name: &str, _args: &Value, level: Option<u64>) -> Result<Value, String> {
            self.term.log.lock().unwrap().push((self.term.id.to_string(), name.to_string(), level));
            match name {
                "wait" => {
                    self.budget.check_wait()?;
                    self.budget.spend_wait(self.term.wait_cost);
                    Ok(json!({"ok": true}))
                }
                "send_keys" => {
                    self.budget.check_keys()?;
                    self.budget.count_keys();
                    Ok(json!({"ok": true}))
                }
                "list_commands" => Ok(json!([{"id": 1}])),
                _ => Ok(json!({"ok": true})),
            }
        }
    }

    /// A nested shell on top, as a terminal reports it.
    fn nested(key: u64, label: &str) -> TopLevel {
        TopLevel { key, location: Some(label.to_string()), ..TopLevel::default() }
    }

    /// The chat over a fixed set of terminals (the default first).
    fn chat(terminals: Vec<Arc<dyn Terminal>>, wait_budget: Duration) -> ChatTerminals {
        ChatTerminals::new(move || terminals.clone(), wait_budget)
    }

    fn ids_of(list: &[Arc<Fake>]) -> Vec<Arc<dyn Terminal>> {
        list.iter().map(|f| f.clone() as Arc<dyn Terminal>).collect()
    }

    fn logged(log: &Log) -> Vec<(String, String, Option<u64>)> {
        log.lock().unwrap().clone()
    }

    /// The approval cards of a turn: (level, location) of each.
    fn cards(events: &Mutex<Vec<AgentEvent>>) -> Vec<(u64, Option<String>)> {
        events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ApprovalRequest { level, location, .. } => Some((*level, location.clone())),
                _ => None,
            })
            .collect()
    }

    /// `(name, decision, ok)` of every ToolResult, in order.
    fn decisions(events: &Mutex<Vec<AgentEvent>>) -> Vec<(String, &'static str, bool)> {
        events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolResult { name, decision, ok, .. } => Some((name.clone(), *decision, *ok)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_gate_inspects_the_terminal_the_call_names() {
        let log: Log = Arc::default();
        let t1 = Fake::new("t1", TopLevel::default(), &log);
        let t2 = Fake::new("t2", nested(7, "ssh h"), &log);
        *t2.precheck.lock().unwrap() = Some("t2 is not at a prompt".into());
        let tools = chat(ids_of(&[t1.clone(), t2.clone()]), Duration::from_secs(10));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let events = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, .. } = &ev {
                assert!(gate.decide(call_id, true));
            }
            events.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);

        // t2's precheck refuses before any card: the gate looked at t2, not at t1.
        assert!(ctx.call_tool("run_command", json!({"command": "pwd", "terminal": "t2"})).is_err());
        assert!(cards(&events).is_empty(), "no card for a refused call");
        assert!(logged(&log).is_empty());

        // Once t2's precheck passes, the card shows t2's level and location, and the call runs on t2 bound to that level.
        *t2.precheck.lock().unwrap() = None;
        assert!(ctx.call_tool("run_command", json!({"command": "pwd", "terminal": "t2"})).is_ok());
        assert_eq!(cards(&events), [(7, Some("ssh h".to_string()))]);
        assert_eq!(logged(&log), [("t2".to_string(), "run_command".to_string(), Some(7))]);

        // Without the argument the call goes to the default, t1, and its card names t1's level.
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(cards(&events), [(7, Some("ssh h".to_string())), (0, None)]);
        assert_eq!(logged(&log)[1], ("t1".to_string(), "run_command".to_string(), Some(0)));
        assert_eq!(
            decisions(&events),
            [
                ("run_command".to_string(), "refused", false),
                ("run_command".to_string(), "manual", true),
                ("run_command".to_string(), "manual", true),
            ]
        );
    }

    #[test]
    fn an_unknown_terminal_is_refused_before_any_card_and_nothing_runs() {
        let log: Log = Arc::default();
        let t1 = Fake::new("t1", TopLevel::default(), &log);
        let tools = chat(ids_of(&[t1]), Duration::from_secs(10));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let events = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| events.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);

        let err = ctx.call_tool("run_command", json!({"command": "pwd", "terminal": "t9"})).unwrap_err();
        assert_eq!(err, "no terminal 't9' in this chat; call list_terminals");
        let bad_type = ctx.call_tool("run_command", json!({"command": "pwd", "terminal": 3})).unwrap_err();
        assert_eq!(bad_type, "argument `terminal` must be a string");
        assert!(cards(&events).is_empty());
        assert!(logged(&log).is_empty());
        assert_eq!(decisions(&events), [("run_command".to_string(), "refused", false), ("run_command".to_string(), "refused", false)]);
    }

    #[test]
    fn a_terminal_removed_mid_turn_is_refused_at_the_next_call() {
        let log: Log = Arc::default();
        let t1 = Fake::new("t1", TopLevel::default(), &log);
        let t2 = Fake::new("t2", TopLevel::default(), &log);
        let set: Arc<Mutex<Vec<Arc<dyn Terminal>>>> = Arc::new(Mutex::new(ids_of(&[t1, t2])));
        let provider = set.clone();
        let tools = ChatTerminals::new(move || provider.lock().unwrap().clone(), Duration::from_secs(10));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let events = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| events.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);

        assert!(ctx.read_tool("get_cwd", json!({"terminal": "t2"})).is_ok());
        set.lock().unwrap().retain(|t| t.id() == "t1");
        let err = ctx.read_tool("get_cwd", json!({"terminal": "t2"})).unwrap_err();
        assert_eq!(err, "no terminal 't2' in this chat; call list_terminals");
        // The default is still there.
        assert!(ctx.read_tool("get_cwd", json!({})).is_ok());
        assert_eq!(logged(&log).len(), 2);
    }

    #[test]
    fn with_no_terminal_the_terminal_tools_refuse_and_the_rest_still_work() {
        let tools = ChatTerminals::new(Vec::new, Duration::from_secs(10));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let events = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| events.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);

        assert_eq!(ctx.call_tool("run_command", json!({"command": "pwd"})).unwrap_err(), NO_TERMINAL);
        assert_eq!(ctx.read_tool("get_screen", json!({})).unwrap_err(), NO_TERMINAL);
        assert!(cards(&events).is_empty(), "no card when there is no terminal");
        assert_eq!(ctx.read_tool("list_terminals", json!({})).unwrap(), json!([]));
        assert!(ctx.top_level().location.is_none());
        assert!(ctx.session_facts().is_none());
        // A tool that is not a terminal's runs as before.
        let todo = ctx.local_tool("todo_write", &json!({}), |_| Ok(json!("ok")));
        assert_eq!(todo.unwrap(), json!("ok"));
    }

    #[test]
    fn a_record_id_routes_to_the_terminal_that_owns_it() {
        let log: Log = Arc::default();
        let t1 = Fake::new("t1", TopLevel::default(), &log);
        let t2 = Arc::new(Fake {
            id: "t2",
            title: None,
            top: TopLevel::default(),
            precheck: Mutex::new(None),
            records: vec![7],
            wait_cost: Duration::ZERO,
            log: log.clone(),
        });
        let tools = chat(ids_of(&[t1, t2]), Duration::from_secs(10));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);

        assert!(ctx.read_tool("get_command", json!({"id": 7})).is_ok());
        assert!(ctx.read_tool("wait", json!({"id": 7})).is_ok());
        assert!(ctx.read_tool("get_command", json!({"id": 8})).is_ok(), "no owner: the default");
        assert!(ctx.read_tool("get_command", json!({"id": 7, "terminal": "t1"})).is_ok(), "the argument wins");
        assert_eq!(
            logged(&log).iter().map(|(t, n, _)| (t.as_str(), n.as_str())).collect::<Vec<_>>(),
            [("t2", "get_command"), ("t2", "wait"), ("t1", "get_command"), ("t1", "get_command")]
        );
    }

    #[test]
    fn the_wait_budget_is_per_turn_across_terminals() {
        let log: Log = Arc::default();
        let mut t1 = Fake::new("t1", TopLevel::default(), &log);
        let mut t2 = Fake::new("t2", TopLevel::default(), &log);
        // Each wait costs 6 s of a 10 s turn budget. Two different terminals spend it together.
        Arc::get_mut(&mut t1).unwrap().wait_cost = Duration::from_secs(6);
        Arc::get_mut(&mut t2).unwrap().wait_cost = Duration::from_secs(6);
        let tools = chat(ids_of(&[t1, t2]), Duration::from_secs(10));
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);

        assert!(ctx.read_tool("wait", json!({"terminal": "t1"})).is_ok());
        assert!(ctx.read_tool("wait", json!({"terminal": "t2"})).is_ok());
        let spent = ctx.read_tool("wait", json!({"terminal": "t1"})).unwrap_err();
        assert!(spent.starts_with("wait budget for this turn is used up (12 s"), "{spent}");
    }

    #[test]
    fn the_send_keys_limit_is_per_turn_across_terminals() {
        let log: Log = Arc::default();
        let t1 = Fake::new("t1", TopLevel::default(), &log);
        let t2 = Fake::new("t2", TopLevel::default(), &log);
        let tools = chat(ids_of(&[t1, t2]), Duration::from_secs(10));
        for i in 0..SEND_KEYS_PER_TURN {
            let id = if i % 2 == 0 { "t1" } else { "t2" };
            let view = tools.scoped("send_keys", &json!({"terminal": id})).unwrap().expect("a terminal call");
            assert!(view.call("send_keys", &json!({"terminal": id}), None).is_ok(), "write {i}");
        }
        let view = tools.scoped("send_keys", &json!({"terminal": "t1"})).unwrap().expect("a terminal call");
        let refused = view.call("send_keys", &json!({"terminal": "t1"}), None).unwrap_err();
        assert!(refused.starts_with("the send_keys limit for this turn is reached"), "{refused}");
    }

    #[test]
    fn list_terminals_gives_each_terminal_in_order_with_its_facts() {
        let log: Log = Arc::default();
        let t1 = Arc::new(Fake {
            id: "t1",
            title: Some("shell"),
            top: TopLevel::default(),
            precheck: Mutex::new(None),
            records: Vec::new(),
            wait_cost: Duration::ZERO,
            log: log.clone(),
        });
        let t2 = Fake::new("t2", nested(7, "ssh h"), &log);
        let tools = chat(ids_of(&[t1, t2]), Duration::from_secs(10));
        let state = serde_json::to_value(InputState::Prompt).unwrap();

        assert_eq!(
            tools.call("list_terminals", &json!({}), None).unwrap(),
            json!([
                {"id": "t1", "title": "shell", "state": state, "cwd": null, "location": null, "default": true},
                {"id": "t2", "title": null, "state": state, "cwd": null, "location": "ssh h", "default": false},
            ])
        );
        assert_eq!(tools.ids(), ["t1", "t2"]);
    }

    #[test]
    fn one_terminal_leaves_results_unchanged_and_two_name_the_terminal() {
        let log: Log = Arc::default();
        let t1 = Fake::new("t1", TopLevel::default(), &log);
        let one = chat(ids_of(std::slice::from_ref(&t1)), Duration::from_secs(10));
        assert_eq!(one.call("get_cwd", &json!({}), None).unwrap(), json!({"ok": true}));
        assert_eq!(one.call("list_commands", &json!({}), None).unwrap(), json!([{"id": 1}]));

        let t2 = Fake::new("t2", TopLevel::default(), &log);
        let two = chat(ids_of(&[t1, t2]), Duration::from_secs(10));
        assert_eq!(two.call("get_cwd", &json!({}), None).unwrap(), json!({"ok": true, "terminal": "t1"}));
        assert_eq!(
            two.call("list_commands", &json!({"terminal": "t2"}), None).unwrap(),
            json!({"terminal": "t2", "result": [{"id": 1}]})
        );
    }

    #[test]
    fn a_non_terminal_tool_is_not_routed() {
        let tools = chat(ids_of(&[Fake::new("t1", TopLevel::default(), &Arc::default())]), Duration::from_secs(10));
        assert!(tools.scoped("read_file", &json!({"terminal": "t9"})).unwrap().is_none());
        assert!(tools.scoped("list_terminals", &json!({})).unwrap().is_none());
        assert_eq!(tools.call("read_file", &json!({}), None).unwrap_err(), "unknown tool `read_file`");
    }
}
