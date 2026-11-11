//! Real-PTY tests for the agent tools, the MockAgent rules, and the approval gate. Harness: common/mod.rs.

mod common;

use std::fs;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use watt_terminal_lib::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate, DENIED, MockAgent, UserMessage, user_input};
use watt_terminal_lib::core::approval::RegexPolicy;
use watt_terminal_lib::core::chat::project::project;
use watt_terminal_lib::core::chat::redact::Redactor;
use watt_terminal_lib::core::llm::TurnCancel;
use watt_terminal_lib::core::foreground::ForegroundKind;
use watt_terminal_lib::core::observer::{EventSink, InputState, IntegrationStatus, ObserverStatus, SessionEvent};
use watt_terminal_lib::core::record::Source;
use watt_terminal_lib::core::session::{Session, USER_TEXT_CODE};
use watt_terminal_lib::core::settings::{ApprovalMode, ApprovalSettings};
use watt_terminal_lib::core::tools::{schemas, AgentTools, NotRunReason, RunOutcome, SessionTools, WaitReason, DEFAULT_RUN_WAIT};
use serde_json::{json, Value};

use common::*;

type Log = Arc<Mutex<Vec<AgentEvent>>>;

/// Runs one turn on this thread and returns every event it emitted.
fn turn(s: &Session, approvals: &ApprovalGate, text: &str) -> Vec<AgentEvent> {
    let log: Log = Arc::default();
    let sink = log.clone();
    let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
    let tools = SessionTools::new(s);
    let ctx = AgentCtx::new(&tools, approvals, &emit);
    agent::run_turn(&mut MockAgent, &UserMessage::text(text), &ctx);
    log.lock().unwrap().clone()
}

/// Runs one turn on another thread, for turns that wait for an approval. Returns the thread and the event log.
fn turn_in_thread(s: Arc<Session>, approvals: Arc<ApprovalGate>, text: &str) -> (thread::JoinHandle<()>, Log) {
    let log: Log = Arc::default();
    let sink = log.clone();
    let text = text.to_string();
    let h = thread::spawn(move || {
        let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
        let tools = SessionTools::new(&s);
        let ctx = AgentCtx::new(&tools, &approvals, &emit);
        agent::run_turn(&mut MockAgent, &UserMessage::text(&text), &ctx);
    });
    (h, log)
}

fn approval_ids(log: &Log) -> Vec<(String, String)> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalRequest { call_id, text, .. } => Some((call_id.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

/// Waits for the first approval request and returns (call_id, text). On timeout the panic carries what the turn and
/// the session know at that moment (see `dump_turn`).
fn wait_approval(s: &Session, log: &Log) -> (String, String) {
    match wait_until(Duration::from_secs(10), || approval_ids(log).first().cloned()) {
        Some(v) => v,
        None => panic!("an approval was requested\n{}", dump_turn(s, log)),
    }
}

/// Diagnostic dump for a stalled turn: the agent events so far (tool results with their error text), session state,
/// foreground, the typed-since-prompt flag, the newest records, and the last 15 screen rows.
fn dump_turn(s: &Session, log: &Log) -> String {
    let events = log.lock().unwrap().clone();
    let mut out = format!(
        "DUMP state={:?} foreground={:?} integration={:?} user_typed_since_prompt={}\n",
        s.state(),
        s.foreground(),
        s.integration_status(),
        s.user_typed_since_prompt()
    );
    let text_deltas = events.iter().filter(|e| matches!(e, AgentEvent::TextDelta { .. })).count();
    out.push_str(&format!("events ({} total, {text_deltas} TextDelta omitted):\n", events.len()));
    for e in events.iter().filter(|e| !matches!(e, AgentEvent::TextDelta { .. })) {
        out.push_str(&format!("  {e:?}\n"));
    }
    let records = s.records();
    out.push_str(&format!("records ({} total), newest first:\n", records.len()));
    for r in records.iter().rev().take(5) {
        out.push_str(&format!("  {r:?}\n"));
    }
    let snap = snapshot(s);
    let first = snap.rows.len().saturating_sub(15);
    out.push_str(&format!("screen cursor={:?} alt={} rows {first}..{}:\n", snap.cursor, snap.alternate_screen, snap.rows.len()));
    for (i, row) in snap.rows.iter().enumerate().skip(first) {
        out.push_str(&format!("  {i:2}|{row}\n"));
    }
    out
}

fn text_of(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_approval(events: &[AgentEvent]) -> bool {
    events.iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. }))
}

/// `ok` of the last `run_command` ToolResult.
fn run_ok(events: &[AgentEvent]) -> Option<bool> {
    events.iter().rev().find_map(|e| match e {
        AgentEvent::ToolResult { name, ok, .. } if name == "run_command" => Some(*ok),
        _ => None,
    })
}

fn run_command_ok(log: &Log) -> Option<bool> {
    run_ok(&log.lock().unwrap())
}

/// The `active` value of each agent_control event, in order.
fn control_events(log: &Log) -> Vec<bool> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::AgentControl { active } => Some(*active),
            _ => None,
        })
        .collect()
}

#[test]
fn list_and_get_command_page_and_json_calls() {
    let _g = serial();
    let s = start();
    run(&s, "echo one");
    run(&s, "echo two");
    let big = run(&s, "seq 1 200");
    let tools = SessionTools::new(&s);

    let last = tools.list_commands(2);
    assert_eq!(last.iter().map(|r| r.command.as_str()).collect::<Vec<_>>(), ["echo two", "seq 1 200"]);

    let full: String = (1..=200).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
    let p1 = tools.get_command(big.id, 0, 16).expect("page 1");
    assert_eq!(p1.output, &full[..16]);
    assert!(p1.truncated, "first page must say the output continues");
    assert_eq!(p1.total_bytes, full.len());
    let p2 = tools.get_command(big.id, 16, 100_000).expect("page 2");
    assert_eq!(p2.output, &full[16..]);
    assert!(!p2.truncated);
    assert_eq!(p2.cwd, p1.cwd);
    assert!(tools.get_command(999_999, 0, 10).is_err());

    // The same reads through the JSON call path an LLM would use.
    let v = tools.call("list_commands", &json!({"n": 1}), None).expect("list_commands");
    assert_eq!(v[0]["command"], "seq 1 200");
    let v = tools.call("get_command", &json!({"id": big.id, "limit": 16}), None).expect("get_command");
    assert_eq!(v["output"].as_str(), Some(&full[..16]));
    assert_eq!(v["truncated"], true);
    assert!(tools.call("get_command", &json!({}), None).is_err(), "id is required");
}

#[test]
fn get_screen_shows_output_and_alt_flag() {
    let _g = serial();
    let s = start();
    run(&s, "echo screen-marker");
    let tools = SessionTools::new(&s);
    let v = tools.get_screen();
    assert!(v.rows.iter().any(|r| r == "screen-marker"), "rows: {:?}", v.rows);
    assert!(!v.alternate_screen);
    assert_eq!(v.observer_status, ObserverStatus::Ok);
    assert_eq!(v.integration_status, IntegrationStatus::Live);

    let cmd = r"printf '\033[?1049h'; sleep 2; printf '\033[?1049l'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Alt, Duration::from_secs(5)), "state={:?}", s.state());
    assert!(tools.get_screen().alternate_screen, "alt flag not reported while the alt screen is up");
    run_finish(&s, cmd);
    assert!(!tools.get_screen().alternate_screen);
}

#[test]
fn get_cwd_follows_cd() {
    let _g = serial();
    let s = start();
    run(&s, "cd /tmp");
    let cwd = SessionTools::new(&s).get_cwd().expect("cwd known");
    assert_eq!(cwd.path, "/tmp");
    assert!(!cwd.host.is_empty());
}

#[test]
fn run_command_when_idle_returns_record_with_exit_code_and_output() {
    let _g = serial();
    let dir = unique_name("agent-cwd");
    fs::create_dir_all(&dir).expect("create dir");
    let s = start();
    run(&s, &format!("cd '{}'", dir.display()));
    let tools = SessionTools::new(&s);
    let outcome = tools.run_command("pwd", DEFAULT_RUN_WAIT, None).expect("run_command");
    let RunOutcome::Finished { record } = outcome else { panic!("expected a finished record, got {outcome:?}") };
    assert_eq!(record.exit_code, Some(0));
    assert_eq!(record.output, dir.display().to_string());
    assert!(matches!(&record.source, Source::Agent { call_id } if call_id.starts_with("run-")), "source={:?}", record.source);
    assert_eq!(tools.list_commands(1)[0].id, record.id, "the command is in the session history");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn run_command_is_refused_while_running_and_in_alt_screen() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 3").expect("submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(3)), "state={:?}", s.state());
    let tools = SessionTools::new(&s);
    let err = tools.run_command("echo refused-running", DEFAULT_RUN_WAIT, None).expect_err("refused while running");
    assert!(err.contains("Running"), "error: {err}");
    run_finish(&s, "sleep 3");

    let cmd = r"printf '\033[?1049h'; sleep 2; printf '\033[?1049l'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Alt, Duration::from_secs(5)), "state={:?}", s.state());
    let err = tools.run_command("echo refused-alt", DEFAULT_RUN_WAIT, None).expect_err("refused in alt screen");
    assert!(err.contains("Alt"), "error: {err}");
    run_finish(&s, cmd);
    assert!(!s.records().iter().any(|r| r.command.starts_with("echo refused")), "a refused command ran");
}

#[test]
fn run_command_returns_running_at_its_wait_and_the_session_stays_usable() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let outcome = tools.run_command("sleep 30", Duration::from_millis(500), None).expect("run_command");
    match outcome {
        // Usually Running; Submitted if the C event had not arrived yet when the timeout expired.
        RunOutcome::Running { state: InputState::Running | InputState::Submitted, .. } => {}
        other => panic!("expected a running command, got {other:?}"),
    }
    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
    let r = run(&s, "echo alive");
    assert_eq!(r.output, "alive");
    assert!(s.records().iter().any(|r| r.command == "sleep 30" && r.exit_code.is_some()), "the timed-out command never finished");
}

/// At an idle prompt there is no command of the agent's to type into: the refusal says to use run_command. The schemas list
/// every tool, send_keys among them.
#[test]
fn send_keys_is_refused_at_a_prompt_and_the_schemas_list_every_tool() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let err = tools.call("send_keys", &json!({"keys": ["Enter"]}), None).expect_err("nothing is running");
    assert!(err.contains("nothing is running; use run_command"), "error: {err}");
    let names: Vec<&str> = schemas().iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        ["list_commands", "get_command", "get_screen", "get_cwd", "wait", "run_command", "request_integration", "send_keys"]
    );
}

/// Submits `cmd` as the user and waits until the shell reports it running.
fn start_running(s: &Session, cmd: &str) {
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(s, InputState::Running, Duration::from_secs(10)), "state={:?}", s.state());
}

#[test]
fn wait_returns_at_once_when_a_command_finishes_and_carries_its_record() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    start_running(&s, "sleep 1");
    let started = Instant::now();
    let view = SessionTools::new(&s).wait(20, false, None).expect("wait");
    let took = started.elapsed();
    assert_eq!(view.reason, WaitReason::CommandFinished, "{view:?}");
    assert!(took < Duration::from_secs(10), "the wait took {took:?}");
    assert_eq!(view.state, InputState::Prompt);
    let record = view.record.expect("the finished command's record");
    assert_eq!(record.command, "sleep 1");
    assert_eq!(record.exit_code, Some(0));
}

#[test]
fn wait_returns_early_when_output_appears_while_the_command_still_runs() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = r"sleep 1; printf 'marker-out\n'; sleep 4";
    start_running(&s, cmd);
    let started = Instant::now();
    let view = SessionTools::new(&s).wait(20, false, None).expect("wait");
    assert_eq!(view.reason, WaitReason::ScreenChanged, "{view:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "the wait took {:?}", started.elapsed());
    assert_eq!(view.state, InputState::Running, "the command was still running");
    assert!(view.screen.rows.iter().any(|r| r == "marker-out"), "rows: {:?}", view.screen.rows);
    assert!(view.record.is_none());
    run_finish(&s, cmd);
}

/// With `ignore_updates` the output does not wake the wait: it ends when the command finishes, with the record.
#[test]
fn wait_with_ignore_updates_ignores_output_and_ends_at_the_command_finish() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = r"printf 'noise\n'; sleep 2; printf 'more\n'; sleep 2";
    start_running(&s, cmd);
    let view = SessionTools::new(&s).wait(20, true, None).expect("wait");
    assert_eq!(view.reason, WaitReason::CommandFinished, "{view:?}");
    assert!(view.waited_ms >= 2500, "the wait returned on output, after {} ms", view.waited_ms);
    let record = view.record.expect("the record");
    assert!(record.output.contains("more"), "output: {:?}", record.output);
    assert_eq!(record.exit_code, Some(0));
}

#[test]
fn wait_times_out_on_a_quiet_command_and_the_session_stays_usable() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    start_running(&s, "sleep 5");
    let view = SessionTools::new(&s).wait(1, false, None).expect("wait");
    assert_eq!(view.reason, WaitReason::Timeout, "{view:?}");
    assert!((900..3000).contains(&view.waited_ms), "waited {} ms", view.waited_ms);
    assert_eq!(view.state, InputState::Running, "the timeout must not touch the command");
    assert!(view.record.is_none());

    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
    let idle = SessionTools::new(&s).wait(10, false, None).expect("wait after the timeout");
    assert_eq!(idle.reason, WaitReason::Idle);
    let r = run(&s, "echo alive-after-wait");
    assert_eq!(r.output, "alive-after-wait");
}

#[test]
fn wait_at_an_idle_prompt_returns_at_once_with_or_without_ignore_updates() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    for ignore in [false, true] {
        let started = Instant::now();
        let view = SessionTools::new(&s).wait(10, ignore, None).expect("wait");
        assert_eq!(view.reason, WaitReason::Idle, "ignore_updates={ignore}");
        assert!(started.elapsed() < Duration::from_secs(1), "ignore_updates={ignore} took {:?}", started.elapsed());
        assert!(view.record.is_none());
    }
}

#[test]
fn cancel_ends_a_wait_within_a_second_and_leaves_the_command_alone() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    start_running(&s, "sleep 5");
    let cancel = Arc::new(TurnCancel::new());
    let (s1, c1) = (s.arc(), cancel.clone());
    let call = thread::spawn(move || SessionTools::new(&s1).with_cancel(c1).wait(60, false, None));
    thread::sleep(Duration::from_millis(300));
    let cancelled_at = Instant::now();
    cancel.cancel();
    let view = call.join().expect("wait thread").expect("wait");
    let took = cancelled_at.elapsed();
    assert_eq!(view.reason, WaitReason::Cancelled, "{view:?}");
    assert!(took < Duration::from_secs(1), "the cancelled wait took {took:?}");
    assert_eq!(s.state(), InputState::Running, "the cancel must not touch the running command");

    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
}

/// The JSON path: a missing `seconds` is the default, an out-of-range one is clamped with a note, and a bad type is an
/// error. The clamp is seen in a real wait: `seconds: 0` becomes 1 and says so.
#[test]
fn wait_arguments_are_clamped_and_the_clamp_is_noted() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools.call("wait", &json!({"seconds": 500, "ignore_updates": true}), None).expect("wait at the prompt");
    assert_eq!(v["reason"], "idle");
    assert!(v["note"].as_str().is_some_and(|n| n.contains("500")), "{v}");
    let v = tools.call("wait", &json!({}), None).expect("defaults");
    assert_eq!(v["reason"], "idle");
    assert!(v.get("note").is_none(), "no clamp, no note: {v}");
    assert!(tools.call("wait", &json!({"ignore_updates": "yes"}), None).is_err(), "a non-boolean is refused");

    start_running(&s, "sleep 3");
    let started = Instant::now();
    let v = tools.call("wait", &json!({"seconds": 0}), None).expect("clamped wait");
    assert_eq!(v["reason"], "timeout", "{v}");
    assert!(started.elapsed() < Duration::from_secs(3), "seconds 0 was not clamped up to 1");
    assert!(v["note"].as_str().is_some_and(|n| n.contains("waited 1 s")), "{v}");
    run_finish(&s, "sleep 3");
}

/// The wait budget is per turn: a wait starts only while the turn has budget left, and the one that starts is allowed to
/// finish. Here the budget is 500 ms, so the first 1 s wait is allowed and the second is refused without waiting.
#[test]
fn wait_budget_is_per_turn_and_refuses_a_wait_started_after_it_is_used_up() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    start_running(&s, "sleep 5");
    let tools = SessionTools::new(&s).with_wait_budget(Duration::from_millis(500));
    let first = tools.call("wait", &json!({"seconds": 1, "ignore_updates": true}), None).expect("the first wait runs");
    assert_eq!(first["reason"], "timeout", "{first}");
    let started = Instant::now();
    let err = tools.call("wait", &json!({"seconds": 5}), None).expect_err("the budget is used up");
    assert!(err.contains("wait budget for this turn is used up"), "{err}");
    assert!(started.elapsed() < Duration::from_millis(500), "a refused wait still waited");
    assert_eq!(s.state(), InputState::Running);

    // A new turn (a new SessionTools) has its own budget.
    let next = SessionTools::new(&s).wait(1, true, None).expect("a new turn waits again");
    assert_eq!(next.reason, WaitReason::Timeout);
    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
}

#[test]
fn user_submit_and_agent_run_command_race_exactly_one_proceeds() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let user_cmd = "sleep 1; echo USER";
    let barrier = Arc::new(Barrier::new(2));

    let (s1, b1) = (s.arc(), barrier.clone());
    let user = thread::spawn(move || {
        b1.wait();
        s1.submit(Source::User, "sleep 1; echo USER")
    });
    let (s2, b2) = (s.arc(), barrier.clone());
    let agent = thread::spawn(move || {
        b2.wait();
        SessionTools::new(&s2).run_command("echo AGENT", DEFAULT_RUN_WAIT, None)
    });
    let user_res = user.join().expect("user thread");
    let agent_res = agent.join().expect("agent thread");
    let user_won = user_res.is_ok();
    let agent_won = matches!(&agent_res, Ok(RunOutcome::Finished { .. }));
    assert!(user_won ^ agent_won, "expected exactly one to proceed: user={user_res:?} agent={agent_res:?}");

    let winner_output = if user_won {
        run_finish(&s, user_cmd).output
    } else {
        match agent_res.expect("agent result") {
            RunOutcome::Finished { record } => record.output,
            other => panic!("unexpected {other:?}"),
        }
    };
    wait_prompt(&s);
    assert_eq!(winner_output, if user_won { "USER" } else { "AGENT" });
    let commands: Vec<String> = s.records().into_iter().map(|r| r.command).collect();
    assert_eq!(commands.len(), 1, "the losing command ran too: {commands:?}");
}

#[test]
fn last_command_answer_states_exit_code_and_cwd() {
    let _g = serial();
    let s = start();
    run(&s, "cd /tmp");
    run(&s, "false");
    let approvals = ApprovalGate::new();
    for question in ["what was the last command", "why did it fail?"] {
        let events = turn(&s, &approvals, question);
        let text = text_of(&events).to_lowercase();
        assert!(text.contains("`false`"), "{question}: {text}");
        assert!(text.contains("exit code 1"), "{question}: {text}");
        assert!(text.contains("/tmp"), "{question}: {text}");
        let known = SessionTools::new(&s).get_cwd().expect("cwd known");
        assert!(!text.contains(&known.host.to_lowercase()), "{question}: {text}");
        assert!(!has_approval(&events));
        assert!(matches!(events.last(), Some(AgentEvent::Done)));
    }
}

#[test]
fn screen_and_directory_answers_use_the_session() {
    let _g = serial();
    let s = start();
    run(&s, "cd /tmp");
    run(&s, "echo screen-marker-42");
    let approvals = ApprovalGate::new();
    let screen = text_of(&turn(&s, &approvals, "what do you see"));
    assert!(screen.contains("screen-marker-42"), "{screen}");
    let screen = text_of(&turn(&s, &approvals, "show me the screen"));
    assert!(screen.contains("screen-marker-42"), "{screen}");
    let cwd = text_of(&turn(&s, &approvals, "where am I"));
    assert!(cwd.contains("/tmp"), "{cwd}");
    // The answer gives the path only. The host is in get_cwd's JSON but must not be in the answer.
    let known = SessionTools::new(&s).get_cwd().expect("cwd known");
    assert!(!known.host.is_empty());
    assert_eq!(cwd, format!("You are in {}.", known.path));
    assert!(!cwd.contains(&known.host), "{cwd}");
}

#[test]
fn unknown_message_gets_the_help_reply() {
    let _g = serial();
    let s = start();
    let events = turn(&s, &ApprovalGate::new(), "tell me a joke");
    assert!(text_of(&events).contains("I can"));
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::ToolCall { .. })), "help must not call tools");
}

#[test]
fn run_request_with_auto_approve_runs_without_asking() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = ApprovalGate::new();
    approvals.set_auto_approve(true, 0);
    let events = turn(&s, &approvals, "please run `echo auto-cmd`");
    let text = text_of(&events);
    assert!(!has_approval(&events), "auto-approve still asked");
    assert_eq!(run_ok(&events), Some(true), "agent reply: {text}");
    assert!(text.contains("exit code 0") && text.contains("auto-cmd"), "{text}");
    let r = s.records().into_iter().find(|r| r.command == "echo auto-cmd").expect("command ran");
    assert!(matches!(r.source, Source::Agent { .. }));
}

#[test]
fn denied_command_never_runs() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `echo denied-cmd`");
    let (id, shown) = wait_approval(&s, &log);
    assert_eq!(shown, "echo denied-cmd", "the approval card must show the command");
    assert!(!s.records().iter().any(|r| r.command == "echo denied-cmd"), "ran before approval");
    assert!(approvals.decide(&id, false));
    h.join().expect("turn");
    assert!(!s.records().iter().any(|r| r.command == "echo denied-cmd"), "denied command ran");
    assert_eq!(run_command_ok(&log), Some(false));
    let err = run_command_error(&log).expect("the denial is an error result");
    assert!(err.starts_with(DENIED), "a denial must carry the DENIED prefix: {err}");
    assert!(control_events(&log).is_empty(), "a denied command put the agent in control");
    run(&s, "echo still-here");
}

/// The agent is in control only from the approved start of run_command until its result, never while it waits for
/// the approval.
#[test]
fn agent_control_is_on_only_while_an_approved_command_runs() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `sleep 1; echo ctl-ok`");
    let (id, _) = wait_approval(&s, &log);
    assert!(control_events(&log).is_empty(), "in control while waiting for approval");
    assert!(approvals.decide(&id, true));
    h.join().expect("turn");
    assert_eq!(control_events(&log), vec![true, false]);
    assert_eq!(run_command_ok(&log), Some(true));
    let r = s.records().into_iter().find(|r| r.command == "sleep 1; echo ctl-ok").expect("approved command ran");
    assert_eq!(r.exit_code, Some(0));
}

#[test]
fn approved_command_runs_in_the_session() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `echo approved-cmd`");
    let (id, _) = wait_approval(&s, &log);
    assert!(approvals.decide(&id, true));
    h.join().expect("turn");
    let r = s.records().into_iter().find(|r| r.command == "echo approved-cmd").expect("approved command ran");
    assert_eq!(r.exit_code, Some(0));
    assert!(matches!(r.source, Source::Agent { .. }));
    assert_eq!(run_command_ok(&log), Some(true));
    assert!(!approvals.decide(&id, true), "a decided request cannot be decided again");
}

/// Waits for the record of `command` to finish, and returns its id.
fn finished_id(s: &Session, command: &str) -> u64 {
    wait_until(Duration::from_secs(10), || {
        s.records().into_iter().find(|r| r.command == command && r.exit_code.is_some()).map(|r| r.id)
    })
    .unwrap_or_else(|| panic!("`{command}` never finished"))
}

/// A cancel while run_command waits returns NotRun{Cancelled} within a poll, and sends nothing to stop the command: the
/// command finishes on its own.
#[test]
fn cancel_while_run_command_waits_returns_at_once_and_the_command_keeps_running() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cancel = Arc::new(TurnCancel::new());
    let (s1, c1) = (s.arc(), cancel.clone());
    let call = thread::spawn(move || SessionTools::new(&s1).with_cancel(c1).run_command("sleep 2; echo cancel-kept", DEFAULT_RUN_WAIT, None));
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(10)), "state={:?}", s.state());
    let cancelled_at = Instant::now();
    cancel.cancel();
    let outcome = call.join().expect("call thread").expect("run_command");
    let took = cancelled_at.elapsed();
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::Cancelled }), "got {outcome:?}");
    // What the model reads says the command may still be running, which is true: the line was sent before the cancel.
    let said = project("run_command", true, &serde_json::to_value(&outcome).expect("json"), &Redactor::new(Vec::new()));
    assert_eq!(said, "cancelled while waiting; the command may still be running", "{said}");
    assert!(took < Duration::from_secs(1), "the cancelled call took {took:?}");
    assert_eq!(s.state(), InputState::Running, "the cancel must not touch the running command");

    let id = finished_id(&s, "sleep 2; echo cancel-kept");
    let view = SessionTools::new(&s).get_command(id, 0, 100).expect("the command has a record");
    assert_eq!(view.output, "cancel-kept");
    assert_eq!(view.record.exit_code, Some(0), "the command was stopped instead of finishing");
}

/// A cancel that is already set when run_command is called sends nothing: the line never reaches the shell.
#[test]
fn cancel_before_run_command_sends_nothing() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cancel = Arc::new(TurnCancel::new());
    cancel.cancel();
    let outcome = SessionTools::new(&s).with_cancel(cancel).run_command("echo never-sent", DEFAULT_RUN_WAIT, None).expect("run_command");
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::CancelledBeforeSend }), "got {outcome:?}");
    // The line never reached the shell, so the text must not say the command may still be running.
    let said = project("run_command", true, &serde_json::to_value(&outcome).expect("json"), &Redactor::new(Vec::new()));
    assert!(said.contains("cancelled before the command was sent") && !said.contains("may still be running"), "{said}");
    assert_eq!(s.state(), InputState::Prompt, "the line reached the shell");
    run(&s, "echo after-cancel");
    assert!(!s.records().iter().any(|r| r.command.contains("never-sent")), "a cancelled call sent its command");
}

/// The cancel path the app uses (`agent::cancel_turn`, which Ctrl-C and cancel_agent both call) stops a turn that is
/// waiting in run_command: the turn ends at once with the cancelled sentence, and its command keeps running.
#[test]
fn cancel_turn_ends_a_turn_waiting_in_run_command_and_keeps_its_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = Arc::new(ApprovalGate::new());
    approvals.set_auto_approve(true, 0);
    let token = Arc::new(TurnCancel::new());
    let slot = Arc::new(Mutex::new(Some(token.clone())));
    let log: Log = Arc::default();
    let (s1, a1, sink) = (s.arc(), approvals.clone(), log.clone());
    let turn = thread::spawn(move || {
        let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
        let tools = SessionTools::new(&s1).with_cancel(token);
        let ctx = AgentCtx::new(&tools, &a1, &emit);
        agent::run_turn(&mut MockAgent, &UserMessage::text("please run `sleep 2; echo turn-kept`"), &ctx);
    });
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(10)), "state={:?}", s.state());
    let cancelled_at = Instant::now();
    agent::cancel_turn(&slot, &approvals);
    turn.join().expect("turn");
    assert!(cancelled_at.elapsed() < Duration::from_secs(1), "the cancelled turn took {:?}", cancelled_at.elapsed());
    let text = text_of(&log.lock().unwrap());
    assert!(text.contains("you cancelled it"), "{text}");
    let id = finished_id(&s, "sleep 2; echo turn-kept");
    assert_eq!(SessionTools::new(&s).get_command(id, 0, 100).expect("record").output, "turn-kept");
}

/// The decision on each ToolResult, through the real turn: read, auto, manual, denied and refused.
#[test]
fn tool_result_decisions_on_the_real_path() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let decision = |events: &[AgentEvent]| -> Vec<(String, &'static str, bool)> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolResult { name, decision, ok, .. } => Some((name.clone(), *decision, *ok)),
                _ => None,
            })
            .collect()
    };

    let read = turn(&s, &ApprovalGate::new(), "where am I");
    assert_eq!(decision(&read), [("get_cwd".to_string(), "read", true)]);

    let auto = ApprovalGate::new();
    auto.set_auto_approve(true, 0);
    let events = turn(&s, &auto, "please run `echo dec-auto`");
    assert_eq!(decision(&events), [("run_command".to_string(), "auto", true)]);

    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `echo dec-manual`");
    let (id, _) = wait_approval(&s, &log);
    assert!(approvals.decide(&id, true));
    h.join().expect("turn");
    assert_eq!(decision(&log.lock().unwrap()), [("run_command".to_string(), "manual", true)]);

    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `echo dec-denied`");
    let (id, _) = wait_approval(&s, &log);
    assert!(approvals.decide(&id, false));
    h.join().expect("turn");
    assert_eq!(decision(&log.lock().unwrap()), [("run_command".to_string(), "denied", false)]);

    s.write_user(b"echo typed-for-refusal").expect("typed");
    assert!(wait_until(Duration::from_secs(5), || screen_has(&s, "typed-for-refusal").then_some(())).is_some());
    let refused = turn(&s, &ApprovalGate::new(), "please run `echo dec-refused`");
    assert_eq!(decision(&refused), [("run_command".to_string(), "refused", false)]);
    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
}

#[test]
fn ctrl_c_cancels_a_pending_approval() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "run `echo cancelled-cmd`");
    wait_approval(&s, &log);
    user_input(&s, &approvals, "\u{3}").expect("ctrl-c");
    h.join().expect("turn");
    assert!(!s.records().iter().any(|r| r.command == "echo cancelled-cmd"), "cancelled command ran");
    assert_eq!(run_command_ok(&log), Some(false));
    let err = run_command_error(&log).expect("the cancellation is an error result");
    assert!(err.starts_with(DENIED), "a cancelled approval must carry the DENIED prefix: {err}");
}

#[test]
fn approval_timeout_counts_as_denial() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = ApprovalGate::with_timeout(Duration::from_millis(300));
    let events = turn(&s, &approvals, "run `echo timeout-cmd`");
    assert!(has_approval(&events));
    assert_eq!(run_ok(&events), Some(false));
    assert!(!s.records().iter().any(|r| r.command == "echo timeout-cmd"), "timed-out approval still ran the command");
}

#[test]
fn user_input_without_ctrl_c_keeps_approvals_pending() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "run `echo still-pending`");
    let (id, _) = wait_approval(&s, &log);
    user_input(&s, &approvals, "x").expect("typing");
    user_input(&s, &approvals, "\u{8}").expect("backspace");
    assert!(approvals.decide(&id, false));
    h.join().expect("turn");
    assert_eq!(run_command_ok(&log), Some(false));
}

/// Whether any screen row contains `text`.
fn screen_has(s: &Session, text: &str) -> bool {
    snapshot(s).rows.iter().any(|r| r.contains(text))
}

/// The row the cursor is on. zsh keeps an abandoned line (after Ctrl-C) above the new prompt, so only this row shows
/// whether the user's text is still on the prompt line.
fn prompt_line(s: &Session) -> String {
    let snap = snapshot(s);
    snap.rows.get(snap.cursor.0 as usize).cloned().unwrap_or_default()
}

/// The `error` of the last run_command ToolResult, if it failed.
fn run_command_error(log: &Log) -> Option<String> {
    log.lock().unwrap().iter().rev().find_map(|e| match e {
        AgentEvent::ToolResult { name, result, .. } if name == "run_command" => result["error"].as_str().map(str::to_string),
        _ => None,
    })
}

/// Text the user typed on the prompt (no CR) refuses run_command, and nothing reaches the PTY: the typed line stays as
/// it was and the agent's command never ran. Ctrl-C clears the line, and then the agent can run again.
#[test]
fn user_text_at_the_prompt_refuses_run_command_and_writes_nothing() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.write_user(b"echo typed-by-user").expect("typed");
    assert!(wait_until(Duration::from_secs(5), || screen_has(&s, "echo typed-by-user").then_some(())).is_some());

    let tools = SessionTools::new(&s);
    assert!(tools.run_command_precheck().is_err(), "the precheck let the command through");
    let err = tools.run_command("echo agent-marker", DEFAULT_RUN_WAIT, None).expect_err("refused while text is typed");
    assert!(err.starts_with(USER_TEXT_CODE), "error: {err}");
    assert!(!s.records().iter().any(|r| r.command.contains("agent-marker")), "the refused command ran");
    assert!(!screen_has(&s, "agent-marker"), "the refused command reached the screen");
    assert!(screen_has(&s, "echo typed-by-user"), "the user's line changed");

    s.write_user(&[0x03]).expect("ctrl-c");
    assert!(wait_until(Duration::from_secs(5), || (!prompt_line(&s).contains("typed-by-user")).then_some(())).is_some(), "ctrl-c did not clear the prompt line");
    wait_prompt(&s);
    let outcome = tools.run_command("echo after-ctrl-c", DEFAULT_RUN_WAIT, None).expect("runs after ctrl-c");
    let RunOutcome::Finished { record } = outcome else { panic!("expected a finished record, got {outcome:?}") };
    assert_eq!(record.output, "after-ctrl-c");
}

/// The MockAgent refuses before the approval card: nobody is asked to approve a command that would be refused.
#[test]
fn user_text_at_the_prompt_refuses_before_the_approval_card() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.write_user(b"echo typed-first").expect("typed");
    assert!(wait_until(Duration::from_secs(5), || screen_has(&s, "typed-first").then_some(())).is_some());

    // A card would wait for 300 ms and then count as a denial. Either way the refusal must come first.
    let approvals = ApprovalGate::with_timeout(Duration::from_millis(300));
    let events = turn(&s, &approvals, "please run `echo card-check`");
    assert!(!has_approval(&events), "an approval card was raised for a refused command");
    assert!(!events.iter().any(|e| matches!(e, AgentEvent::AgentControl { .. })), "the agent took control for a refused command");
    assert_eq!(run_ok(&events), Some(false));
    assert_eq!(
        text_of(&events),
        "I can't run that: you have text typed at the prompt. Clear it (Ctrl-C) and ask again."
    );
    assert!(!s.records().iter().any(|r| r.command == "echo card-check"), "the refused command ran");

    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
}

/// Text typed while the approval card waits is caught at injection: the approved command is refused, and nothing runs.
#[test]
fn user_text_typed_while_an_approval_waits_refuses_the_injection() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `echo late-cmd`");
    let (id, _) = wait_approval(&s, &log);
    s.write_user(b"echo typed-late").expect("typed while the card waits");
    assert!(wait_until(Duration::from_secs(5), || screen_has(&s, "typed-late").then_some(())).is_some());
    assert!(approvals.decide(&id, true));
    h.join().expect("turn");
    assert_eq!(run_command_ok(&log), Some(false));
    let err = run_command_error(&log).expect("the injection was refused with an error");
    assert!(err.starts_with(USER_TEXT_CODE), "error: {err}");
    assert!(!s.records().iter().any(|r| r.command == "echo late-cmd"), "the approved command ran over the user's text");
    assert!(!screen_has(&s, "late-cmd"), "the approved command reached the screen");

    s.write_user(&[0x03]).expect("ctrl-c");
    assert!(wait_until(Duration::from_secs(5), || (!prompt_line(&s).contains("typed-late")).then_some(())).is_some(), "ctrl-c did not clear the prompt line");
    wait_prompt(&s);
}

/// With nothing typed by the user, run_command works as before.
#[test]
fn run_command_works_when_the_user_has_typed_nothing() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    assert!(tools.run_command_precheck().is_ok());
    let RunOutcome::Finished { record } = tools.run_command("echo untouched", DEFAULT_RUN_WAIT, None).expect("run") else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "untouched");
}

/// A line the user submits leaves the prompt clean: once it has finished, the agent's command runs.
#[test]
fn a_line_the_user_submits_leaves_the_prompt_clean() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.write_user(b"echo user-line\r").expect("typed and submitted");
    run_finish(&s, "echo user-line");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let RunOutcome::Finished { record } = tools.run_command("echo agent-next", DEFAULT_RUN_WAIT, None).expect("run") else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "agent-next");
}

/// Keys typed while a command runs do not count toward the prompt check. Enter is used here, so the next prompt is
/// left empty and the agent's command runs.
#[test]
fn keys_typed_while_a_command_runs_do_not_block_the_next_agent_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 1").expect("submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(3)), "state={:?}", s.state());
    s.write_user(b"\r").expect("enter while the command runs");
    run_finish(&s, "sleep 1");
    wait_prompt(&s);
    let RunOutcome::Finished { record } =
        SessionTools::new(&s).run_command("echo next-ok", DEFAULT_RUN_WAIT, None).expect("not refused")
    else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "next-ok");
}


/// Ctrl-L redraws the line and keeps the user's text, so the refusal stays after it.
#[test]
fn ctrl_l_keeps_user_text_refusing_the_agent() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.write_user(b"echo kept").expect("typed");
    assert!(wait_until(Duration::from_secs(5), || screen_has(&s, "echo kept").then_some(())).is_some());
    s.write_user(b"\x0c").expect("ctrl-l");
    thread::sleep(Duration::from_millis(500));
    assert!(screen_has(&s, "echo kept"), "ctrl-l removed the user's text from the screen");
    assert!(SessionTools::new(&s).run_command_precheck().is_err(), "ctrl-l cleared the refusal");
    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
}

/// Text typed while a command runs is type-ahead: zsh draws it on the next prompt line. The agent's run_command is refused
/// there and writes nothing. Ctrl-C at the prompt clears the line, and then the agent's command runs.
#[test]
fn type_ahead_during_a_command_refuses_the_next_agent_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 1").expect("submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(3)), "state={:?}", s.state());
    s.write_user(b"zqx").expect("type-ahead while the command runs");
    run_finish(&s, "sleep 1");
    assert!(wait_until(Duration::from_secs(5), || prompt_line(&s).contains("zqx").then_some(())).is_some(), "the type-ahead never reached the prompt line");

    let tools = SessionTools::new(&s);
    assert!(tools.run_command_precheck().is_err(), "the precheck let the command through");
    let err = tools.run_command("echo type-ahead-marker", DEFAULT_RUN_WAIT, None).expect_err("refused over type-ahead");
    assert!(err.starts_with(USER_TEXT_CODE), "error: {err}");
    assert!(!s.records().iter().any(|r| r.command.contains("type-ahead-marker")), "the refused command ran");
    assert!(!screen_has(&s, "type-ahead-marker"), "the refused command reached the screen");

    s.write_user(&[0x03]).expect("ctrl-c at the prompt");
    assert!(wait_until(Duration::from_secs(5), || (!prompt_line(&s).contains("zqx")).then_some(())).is_some(), "ctrl-c did not clear the prompt line");
    wait_prompt(&s);
    let RunOutcome::Finished { record } = tools.run_command("echo after-type-ahead", DEFAULT_RUN_WAIT, None).expect("runs after ctrl-c") else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "after-type-ahead");
}

/// Ctrl-C while a command runs does not clear type-ahead, so the refusal holds until a Ctrl-C at the prompt.
#[test]
fn ctrl_c_during_a_command_keeps_the_type_ahead_refusal() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 3").expect("submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(3)), "state={:?}", s.state());
    s.write_user(b"zqx").expect("type-ahead");
    s.write_user(&[0x03]).expect("ctrl-c while the command runs");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    assert!(tools.run_command_precheck().is_err(), "ctrl-c during the command cleared the type-ahead");
    s.write_user(&[0x03]).expect("ctrl-c at the prompt");
    assert!(wait_until(Duration::from_secs(5), || tools.run_command_precheck().is_ok().then_some(())).is_some(), "ctrl-c at the prompt did not clear the refusal");
}

/// Keys typed in the alt screen go to the full-screen program, not to the prompt, so they do not refuse run_command.
#[test]
fn keys_typed_in_the_alt_screen_do_not_refuse_run_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = r"printf '\033[?1049h'; read line; printf '\033[?1049l'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Alt, Duration::from_secs(5)), "state={:?}", s.state());
    s.write_user(b"zqx\r").expect("typed in the alt screen");
    run_finish(&s, cmd);
    let tools = SessionTools::new(&s);
    assert!(tools.run_command_precheck().is_ok(), "alt-screen keys refused the agent");
    let RunOutcome::Finished { record } = tools.run_command("echo after-alt", DEFAULT_RUN_WAIT, None).expect("runs after the alt screen") else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "after-alt");
}

/// A line with an unclosed backtick makes zsh ask for more (PS2). run_command returns WaitingForInput after the
/// continuation wait, and does not send anything. Ctrl-C at the continuation prompt then cancels the line, and the
/// next command runs.
#[test]
fn unclosed_quote_waits_for_input_then_ctrl_c_recovers() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let began = std::time::Instant::now();
    let outcome = tools.run_command("echo `x", DEFAULT_RUN_WAIT, None).expect("run_command");
    let waited = began.elapsed();
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::WaitingForInput }), "got {outcome:?}");
    assert!(waited >= Duration::from_secs(2) && waited < Duration::from_secs(8), "returned after {waited:?}");
    assert_eq!(s.state(), InputState::Continuation, "the shell is not waiting");

    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
    assert!(!s.records().iter().any(|r| r.command.contains("echo `x")), "the unfinished line became a record");
    let RunOutcome::Finished { record } = tools.run_command("echo ok", DEFAULT_RUN_WAIT, None).expect("run after ctrl-c") else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "ok");
}

/// Through the agent's approval path (auto-approve on), the WaitingForInput outcome reaches the tool result as JSON.
#[test]
fn waiting_for_input_reaches_the_agent_as_a_not_run_result() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::with_input_wait(&s, Duration::from_millis(300));
    let approvals = ApprovalGate::new();
    approvals.set_auto_approve(true, 0);
    let log: Log = Arc::default();
    let sink = log.clone();
    let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
    let ctx = AgentCtx::new(&tools, &approvals, &emit);
    let v = ctx.call_tool("run_command", json!({"command": "echo `x"})).expect("the call itself succeeds");
    assert_eq!(v["outcome"], "not_run");
    assert_eq!(v["reason"], "waiting_for_input");
    assert_eq!(run_command_ok(&log), Some(true));
    assert!(text_of(&log.lock().unwrap()).is_empty(), "the tool call must not add chat text");

    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
}

/// Ctrl-C while the agent's line is still in the shell's continuation: the call returns Abandoned at once, well before
/// the continuation wait would have ended, and the session recovers.
#[test]
fn ctrl_c_in_continuation_abandons_the_call_promptly() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let (s1, t1) = (s.arc(), std::time::Instant::now());
    let call = thread::spawn(move || {
        let outcome = SessionTools::new(&s1).run_command("echo `x", DEFAULT_RUN_WAIT, None);
        (outcome, t1.elapsed())
    });
    assert!(wait_state(&s, InputState::Continuation, Duration::from_secs(10)), "state={:?}", s.state());
    s.write_user(&[0x03]).expect("ctrl-c");
    let (outcome, took) = call.join().expect("call thread");
    let outcome = outcome.expect("run_command");
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::Abandoned }), "got {outcome:?}");
    assert!(took < Duration::from_secs(2), "the abandoned call took {took:?}");
    wait_prompt(&s);
    let RunOutcome::Finished { record } = SessionTools::new(&s).run_command("echo after-abandon", DEFAULT_RUN_WAIT, None).expect("run")
    else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "after-abandon");
}

/// The user finishes the agent's unclosed line themselves. The agent's line never runs, so the call returns Abandoned
/// at once instead of waiting out the timeout.
#[test]
fn user_finishing_the_agent_line_abandons_the_call() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let (s1, t1) = (s.arc(), std::time::Instant::now());
    let call = thread::spawn(move || {
        let outcome = SessionTools::new(&s1).run_command("echo `x", DEFAULT_RUN_WAIT, None);
        (outcome, t1.elapsed())
    });
    assert!(wait_state(&s, InputState::Continuation, Duration::from_secs(10)), "state={:?}", s.state());
    s.write_user(b"`\r").expect("the user finishes the line");
    let (outcome, took) = call.join().expect("call thread");
    let outcome = outcome.expect("run_command");
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::Abandoned }), "got {outcome:?}");
    assert!(took < Duration::from_secs(2), "the abandoned call took {took:?}");
    wait_prompt(&s);
    assert!(s.records().iter().any(|r| r.source == Source::User), "the user's completed line has no record");
    assert!(!s.records().iter().any(|r| matches!(r.source, Source::Agent { .. })), "the abandoned line got a record");
}

/// A regex rule runs a matching command without a card, through the real shell, and the result says it was a rule. A
/// command the hard gate refuses still asks, even though a rule would match it.
#[test]
fn a_regex_rule_runs_an_allowed_command_without_a_card_and_a_gated_one_still_asks() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let approval = ApprovalSettings { mode: ApprovalMode::Regex, allow: vec![".*".into()], ..Default::default() };
    let policy = RegexPolicy::new(&approval).expect("the rules compile");
    let approvals = ApprovalGate::with_timeout(Duration::from_millis(200));
    let log: Log = Arc::default();
    let sink = log.clone();
    let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
    let tools = SessionTools::new(&s);
    let ctx = AgentCtx::new(&tools, &approvals, &emit).with_policy(&policy);

    let out = ctx.call_tool("run_command", json!({"command": "echo rule-ran"})).expect("the rule allows it");
    assert_eq!(out["record"]["output"], "rule-ran", "{out}");
    assert!(!has_approval(&log.lock().unwrap()), "an allowed command showed a card");

    // The pipe is refused by the hard gate. Nobody answers, so the card times out as a denial and nothing runs.
    let err = ctx.call_tool("run_command", json!({"command": "echo gated | wc -c"})).expect_err("gated, not answered");
    assert!(err.starts_with(DENIED), "{err}");
    let events = log.lock().unwrap();
    let reasons: Vec<(String, Option<&'static str>)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalRequest { name, reason, .. } => Some((name.clone(), *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(reasons, [("run_command".to_string(), Some("gate_chars"))]);
    let decisions: Vec<(&'static str, Option<&'static str>)> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult { name, decision, reason, .. } if name == "run_command" => Some((*decision, *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(decisions, [("auto", Some("rule")), ("denied", Some("gate_chars"))]);
    drop(events);
    assert!(!s.records().iter().any(|r| r.command.contains("gated")), "the gated command ran");
}

/// Every session event, in order, for the state-event checks.
#[derive(Default)]
struct Events(Mutex<Vec<SessionEvent>>);

impl EventSink for Events {
    fn event(&self, ev: SessionEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

/// A nested interactive zsh with no integration is the foreground program. The hint says so on the screen and on the state
/// event, `run_command` is refused before any approval card, and when the nested shell exits the hint goes away and the
/// agent can run commands again.
#[test]
fn a_nested_shell_is_the_foreground_hint_and_refuses_run_command_without_a_card() {
    let _g = serial();
    let s = start();
    let events = Arc::new(Events::default());
    s.set_event_sink(events.clone());
    wait_prompt(&s);

    let cmd = "zsh -f -i";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(5)), "state={:?}", s.state());

    let tools = SessionTools::new(&s);
    let screen = tools.get_screen();
    assert_eq!(screen.state, InputState::Running);
    let fg = screen.foreground.clone().expect("the nested shell is the foreground hint");
    assert_eq!(fg.kind, ForegroundKind::Shell, "{fg:?}");
    assert_eq!(fg.command, cmd);
    assert!(
        wait_until(Duration::from_secs(5), || {
            events.0.lock().unwrap().iter().any(|e| {
                matches!(e, SessionEvent::State { state: InputState::Running, foreground: Some(f) } if f.kind == ForegroundKind::Shell)
            })
            .then_some(())
        })
        .is_some(),
        "no running state event carried the hint: {:?}",
        events.0.lock().unwrap()
    );

    // The precheck refuses first, with the explanation, and no card is shown.
    let err = tools.run_command_precheck().expect_err("refused while a nested shell runs");
    assert!(err.starts_with("cannot run a command now: the session is Running, not at an idle prompt"), "{err}");
    assert!(err.contains("`zsh -f -i`") && err.contains("without shell integration"), "{err}");
    let err = tools.run_command("echo nested-refused", DEFAULT_RUN_WAIT, None).expect_err("refused in the nested shell");
    assert!(err.starts_with("cannot run a command now"), "{err}");

    let approvals = ApprovalGate::with_timeout(Duration::from_millis(300));
    let turn_events = turn(&s, &approvals, "please run `echo nested-card-check`");
    assert!(!has_approval(&turn_events), "an approval card was raised for a refused command");
    assert!(!turn_events.iter().any(|e| matches!(e, AgentEvent::AgentControl { .. })), "the agent took control");
    assert_eq!(run_ok(&turn_events), Some(false));
    let said = text_of(&turn_events);
    assert!(said.contains("cannot run a command now") && said.contains("`zsh -f -i`"), "{said}");
    assert!(!s.records().iter().any(|r| r.command.contains("nested-card-check")), "the refused command ran");

    // The user types into the nested shell: the command runs there, and the agent stays refused while it is nested.
    s.write_user(b"echo hi\r").expect("typed into the nested shell");
    assert!(
        wait_until(Duration::from_secs(5), || snapshot(&s).rows.iter().any(|r| r.trim() == "hi").then_some(())).is_some(),
        "the nested shell did not run echo hi"
    );
    assert!(tools.run_command_precheck().is_err(), "the agent ran while the nested shell was still up");

    // The nested shell exits (typed at the nested prompt). The hint goes away, and the state event says so.
    s.write_user(b"exit\r").expect("exit the nested shell");
    wait_prompt(&s);
    assert!(tools.get_screen().foreground.is_none(), "the hint outlived the nested shell");
    assert!(
        wait_until(Duration::from_secs(5), || {
            let evs = events.0.lock().unwrap();
            let with = evs.iter().rposition(|e| matches!(e, SessionEvent::State { foreground: Some(_), .. }));
            let without = evs.iter().rposition(|e| matches!(e, SessionEvent::State { state: InputState::Prompt, foreground: None }));
            matches!((with, without), (Some(w), Some(p)) if p > w).then_some(())
        })
        .is_some(),
        "no prompt state event without the hint after it: {:?}",
        events.0.lock().unwrap()
    );

    // The keys typed into the nested shell were its input, not the prompt line (see Core::note_user_bytes): the agent's
    // command runs straight away, with no Ctrl-C at the prompt.
    assert!(tools.run_command_precheck().is_ok(), "the nested shell's typed text refused the agent after it exited");
    let RunOutcome::Finished { record } =
        tools.run_command("echo after-nested", DEFAULT_RUN_WAIT, None).expect("runs right after the nested shell exits")
    else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "after-nested");
}

/// An ordinary command still running is also the hint (kind other): run_command is refused without a card, and the
/// refusal says the command is still running. When it finishes on its own the hint is gone and the agent can run.
#[test]
fn an_ordinary_running_command_is_the_hint_and_refuses_without_a_card() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 2").expect("submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(3)), "state={:?}", s.state());

    let tools = SessionTools::new(&s);
    let fg = tools.get_screen().foreground.expect("the running command is the hint");
    assert_eq!(fg.kind, ForegroundKind::Other);
    assert_eq!(fg.command, "sleep 2");
    let err = tools.run_command("echo during-sleep", DEFAULT_RUN_WAIT, None).expect_err("refused while sleep runs");
    assert!(err.contains("`sleep 2` is still running"), "{err}");
    assert!(!err.contains("without shell integration"), "{err}");

    let approvals = ApprovalGate::with_timeout(Duration::from_millis(300));
    let turn_events = turn(&s, &approvals, "please run `echo during-sleep-card`");
    assert!(!has_approval(&turn_events), "a card was raised for a refused command");

    run_finish(&s, "sleep 2");
    wait_prompt(&s);
    assert!(tools.get_screen().foreground.is_none(), "the hint outlived the command");
    let RunOutcome::Finished { record } =
        tools.run_command("echo after-sleep", DEFAULT_RUN_WAIT, None).expect("runs after the command finished")
    else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "after-sleep");
}

/// In the alternate screen the shell is not at a prompt, so the precheck refuses before the card, with the plain refusal (no
/// hint: a full-screen program has no foreground hint).
#[test]
fn run_command_in_the_alt_screen_is_refused_before_the_card() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = r"printf '\033[?1049h'; sleep 2; printf '\033[?1049l'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Alt, Duration::from_secs(5)), "state={:?}", s.state());
    let tools = SessionTools::new(&s);
    assert_eq!(tools.get_screen().foreground, None, "no hint in the alt screen");
    let err = tools.run_command_precheck().expect_err("the precheck refuses in the alt screen");
    assert_eq!(err, "cannot run a command now: the session is Alt, not at an idle prompt");

    let approvals = ApprovalGate::with_timeout(Duration::from_millis(300));
    let events = turn(&s, &approvals, "please run `echo alt-card-check`");
    assert!(!has_approval(&events), "an approval card was raised in the alt screen");
    assert_eq!(run_ok(&events), Some(false));
    run_finish(&s, cmd);
}


/// A user precmd hook that takes 0.4 s runs before the shell's prompt mark (A). A run_command that returned at the command's
/// D used to be followed by a refusal ("not at an idle prompt") in that gap. run_command now waits for the prompt, so two
/// calls back to back both run. The test waits for nothing itself.
#[test]
fn back_to_back_run_commands_both_run_with_a_slow_precmd_hook() {
    let _g = serial();
    let h = start_with_zshrc("precmd() { sleep 0.4 }\n");
    wait_prompt(&h);
    let tools = SessionTools::new(&h);
    let first = tools.run_command("echo first-slow", DEFAULT_RUN_WAIT, None).expect("the first run_command");
    let second = tools.run_command("echo second-slow", DEFAULT_RUN_WAIT, None).expect("the second run_command");
    assert!(matches!(first, RunOutcome::Finished { .. }), "{first:?}");
    assert!(matches!(second, RunOutcome::Finished { .. }), "{second:?}");
}

/// A command that outlives its wait is returned `running`, with its record id, the call id, the screen tail and the next step.
/// `wait` with that id then returns the finished record, and `get_command` reads it afterwards.
#[test]
fn a_run_that_outlives_its_wait_is_returned_running_and_wait_by_id_finishes_it() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools
        .call("run_command", &json!({"command": "sleep 3; echo yield-done", "wait_seconds": 1}), None)
        .expect("run_command");
    assert_eq!(v["outcome"], "running", "{v}");
    assert_eq!(v["reason"], "timeout", "{v}");
    assert_eq!(v["state"], "running", "{v}");
    let id = v["record_id"].as_u64().expect("the record id");
    assert!(v["call_id"].as_str().is_some_and(|c| c.starts_with("run-")), "{v}");
    assert!(v["waited_ms"].as_u64().is_some_and(|ms| (900..2500).contains(&ms)), "waited {v}");
    let tail: Vec<&str> = v["screen_tail"].as_array().expect("tail").iter().filter_map(|r| r.as_str()).collect();
    assert!(tail.iter().any(|r| r.contains("yield-done")), "the command line is on the tail: {tail:?}");
    let hint = v["hint"].as_str().expect("hint");
    assert!(hint.contains(&format!("wait with id {id}")), "{hint}");
    assert!(hint.contains("get_screen with lines") && hint.contains("run_command is unavailable until it ends"), "{hint}");
    assert!(!hint.contains("send_keys"), "{hint}");
    assert!(!v.to_string().contains("send_keys"), "{v}");

    let w = tools.call("wait", &json!({"id": id, "seconds": 20}), None).expect("wait by id");
    assert_eq!(w["reason"], "command_finished", "{w}");
    assert_eq!(w["record"]["id"], id);
    assert_eq!(w["record"]["exit_code"], 0);
    assert_eq!(w["record"]["output"], "yield-done");
    assert_eq!(w["state"], "prompt", "the wait returns once the shell is back at its prompt");
    assert!(tools.open_agent_records().is_empty(), "the record is closed");

    let c = tools.get_command(id, 0, 1000).expect("get_command after the finish");
    assert_eq!(c.output, "yield-done");
    assert_eq!(c.record.exit_code, Some(0));
    assert!(matches!(&c.record.source, Source::Agent { call_id } if Some(call_id.as_str()) == v["call_id"].as_str()));
}

/// A quick command finishes within the default wait. A `wait_seconds` past the cap is clamped, and the result says so.
#[test]
fn a_quick_command_finishes_within_the_default_wait_and_a_big_wait_seconds_is_clamped() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools.call("run_command", &json!({"command": "echo quick-one"}), None).expect("run_command");
    assert_eq!(v["outcome"], "finished", "{v}");
    assert_eq!(v["record"]["output"], "quick-one");
    assert!(v.get("note").is_none(), "no clamp, no note: {v}");
    let v = tools
        .call("run_command", &json!({"command": "echo clamped-one", "wait_seconds": 500}), None)
        .expect("clamped run_command");
    assert_eq!(v["outcome"], "finished", "{v}");
    assert!(v["note"].as_str().is_some_and(|n| n.contains("500") && n.contains("120")), "{v}");
}

/// `get_screen` with `lines` reaches back past the visible rows. Without it the answer is the visible screen, as before.
/// The history is recorded for the agent's own commands only, so the command runs through run_command.
#[test]
fn get_screen_lines_returns_the_scrolled_history_and_the_default_is_unchanged() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools.call("run_command", &json!({"command": "seq 1 200", "wait_seconds": 20}), None).expect("run_command");
    assert_eq!(v["outcome"], "finished", "{v}");
    wait_prompt(&s);

    let view = tools.get_screen_lines(150);
    assert_eq!(view.rows.len(), 150, "{:?}", &view.rows[..3.min(view.rows.len())]);
    // The prompt rows that scrolled off while the command's prompt was drawn are history too, so the window is a few
    // lines later than the 150 numbers alone: the newest line is there, and the oldest numbers are not.
    assert!(view.rows.iter().any(|r| r == "200") && view.rows.iter().any(|r| r == "100"), "the history is there");
    assert!(!view.rows.iter().any(|r| r == "40"), "150 lines cannot reach back that far");
    let h = view.history.clone().expect("history is set with lines");
    assert_eq!((h.lines, h.truncated, h.no_history), (150, false, false));

    let v = tools.call("get_screen", &json!({"lines": 150}), None).expect("get_screen with lines");
    assert_eq!(v["history"]["lines"], 150, "{v}");
    assert_eq!(v["rows"].as_array().map(Vec::len), Some(150));
    let text = project("get_screen", true, &v, &Redactor::new(Vec::new()));
    assert!(text.lines().next().is_some_and(|h| h.contains(" lines=150")), "{text}");

    let plain = tools.call("get_screen", &json!({}), None).expect("get_screen");
    assert!(plain.get("history").is_none(), "the default has no history key: {plain}");
    assert!(plain["rows"].as_array().is_some_and(|r| r.len() <= 30), "the visible screen only");
}

/// A command that yielded keeps its history: after more output, `get_screen` with `lines` still reads the early lines, and
/// `wait` by its id finishes it.
#[test]
fn a_yielded_commands_history_is_readable_after_more_output() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools
        .call("run_command", &json!({"command": "seq 1 300; sleep 2; seq 301 330", "wait_seconds": 1}), None)
        .expect("run_command");
    assert_eq!(v["outcome"], "running", "{v}");
    let id = v["record_id"].as_u64().expect("the record id");
    let w = tools.call("wait", &json!({"id": id, "seconds": 20, "ignore_updates": true}), None).expect("wait by id");
    assert_eq!(w["reason"], "command_finished", "{w}");
    let view = tools.call("get_screen", &json!({"lines": 200}), None).expect("get_screen with lines");
    let rows: Vec<&str> = view["rows"].as_array().expect("rows").iter().filter_map(Value::as_str).collect();
    // 200 logical lines reach back past the 30 visible rows and the prompt rows that scrolled off after the command.
    assert!(rows.contains(&"140") && rows.contains(&"330"), "the last 200 lines are not all there: {rows:?}");
}

/// The agent's own command is the open record while it runs, through the alternate screen, where the foreground hint is gone.
/// The refusal names its id, and `wait` by that id finishes it.
#[test]
fn the_agents_open_command_is_visible_through_the_alternate_screen_and_refusals_name_it() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let cmd = r"printf '\033[?1049h'; sleep 3; printf '\033[?1049l'; echo alt-done";
    let v = tools.call("run_command", &json!({"command": cmd, "wait_seconds": 1}), None).expect("run_command");
    assert_eq!(v["outcome"], "running", "{v}");
    let id = v["record_id"].as_u64().expect("the record id");
    assert!(wait_state(&s, InputState::Alt, Duration::from_secs(5)), "state={:?}", s.state());

    assert_eq!(tools.get_screen().foreground, None, "no hint in the alt screen");
    let open = tools.open_agent_records();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!((open[0].id, open[0].on_top), (id, true));
    assert_eq!(open[0].command, cmd);

    let err = tools.run_command_precheck().expect_err("refused in the alt screen");
    assert!(err.contains(&format!("Your command id {id} ")) && err.contains("Wait for it"), "{err}");
    let err = tools.run_command("echo refused-named", DEFAULT_RUN_WAIT, None).expect_err("refused while the agent's command runs");
    assert!(err.contains(&format!("command id {id}")), "{err}");

    let lines = tools.get_screen_lines(10);
    assert!(lines.history.as_ref().is_some_and(|h| h.no_history), "the alternate screen has no history");

    let w = tools.call("wait", &json!({"id": id, "seconds": 20, "ignore_updates": true}), None).expect("wait by id");
    assert_eq!(w["reason"], "command_finished", "{w}");
    assert!(tools.open_agent_records().is_empty());
    // A command that used the alternate screen is finished without its on-screen rows, so its stored output is not
    // checked here. What is true after it exits: the record is finished and marked as used_alt_screen, and the
    // visible screen shows what the program printed.
    let done = tools.get_command(id, 0, 1000).expect("finished");
    assert!(done.record.used_alt_screen, "{done:?}");
    assert!(tools.get_screen().rows.iter().any(|r| r == "alt-done"), "rows: {:?}", tools.get_screen().rows);
}

/// `wait` without an id is keyed to the one agent command open at the start, so it returns when that command finishes.
#[test]
fn a_wait_without_an_id_is_keyed_to_the_only_open_agent_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools
        .call("run_command", &json!({"command": "sleep 2; echo keyed-done", "wait_seconds": 1}), None)
        .expect("run_command");
    assert_eq!(v["outcome"], "running", "{v}");
    let id = v["record_id"].as_u64().expect("the record id");
    let w = tools.call("wait", &json!({"seconds": 20, "ignore_updates": true}), None).expect("wait");
    assert_eq!(w["reason"], "command_finished", "{w}");
    assert_eq!(w["record"]["id"], id, "{w}");
    assert_eq!(w["record"]["output"], "keyed-done");
}

/// An id that is neither running nor finished is refused, and the refusal says so.
#[test]
fn wait_refuses_an_id_that_is_not_running_or_finished() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let err = SessionTools::new(&s).call("wait", &json!({"id": 999_999}), None).expect_err("no such command");
    assert!(err.contains("no command with id 999999"), "{err}");
}

/// Calls run_command the way the agent does (`call`, with wait_seconds), and returns the result with the time the call took.
fn timed_run(tools: &SessionTools, command: &str, wait_seconds: u64) -> (Value, Duration) {
    let started = Instant::now();
    let v = tools.call("run_command", &json!({"command": command, "wait_seconds": wait_seconds}), None).expect("run_command");
    (v, started.elapsed())
}

/// A zsh started with no command (`zsh -f -i`) is a login with no shell integration. Its prompt turns bracketed paste on, so
/// run_command returns at once with reason remote_shell, well before its 30 s wait. The hint names the command and does not
/// suggest typing into it.
#[test]
fn run_command_into_a_zsh_without_integration_returns_remote_shell_early() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let (v, took) = timed_run(&tools, "zsh -f -i", 30);
    println!("remote_shell early return, zsh -f -i: {} ms (waited_ms {})", took.as_millis(), v["waited_ms"]);
    assert_eq!(v["outcome"], "running", "{v}");
    assert_eq!(v["reason"], "remote_shell", "{v}");
    assert!(took < Duration::from_secs(4), "returned after {took:?}, not early");
    assert!(v["record_id"].as_u64().is_some(), "{v}");
    let hint = v["hint"].as_str().expect("hint");
    assert!(
        hint.starts_with("`zsh -f -i` opened a shell that has no shell integration, so commands cannot run in it yet.")
            && hint.contains("request_integration") && hint.contains("Do not type commands into it."),
        "{hint}"
    );
    assert_eq!(v["foreground"]["kind"], "shell", "{v}");
    assert!(v["screen_tail"].as_array().is_some_and(|t| !t.is_empty()), "the prompt row is in the tail: {v}");
    assert!(tools.run_command_precheck().is_err(), "a command is refused in the nested shell");
}

/// `sh -c` is not a login: it runs a command, not a shell. Its command finishes inside the wait, and nothing returns early.
#[test]
fn a_shell_given_a_command_is_not_returned_early() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let (v, took) = timed_run(&tools, "sh -c 'sleep 2'", 5);
    println!("sh -c 'sleep 2' with wait 5: {} ms, outcome {}", took.as_millis(), v["outcome"]);
    assert_eq!(v["outcome"], "finished", "{v}");
    assert!(took >= Duration::from_millis(1900), "returned after {took:?}, before the command finished");
}

/// A password-like prompt on the last row, quiet for 300 ms, returns at once with reason needs_user. The do-not-type note
/// comes with it, and the agent is told to ask the user, then wait.
#[test]
fn a_password_prompt_returns_needs_user_at_once() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let (v, took) = timed_run(&tools, "printf 'Password: '; sleep 6", 30);
    println!("needs_user early return: {} ms (waited_ms {})", took.as_millis(), v["waited_ms"]);
    assert_eq!(v["outcome"], "running", "{v}");
    assert_eq!(v["reason"], "needs_user", "{v}");
    assert!(took < Duration::from_millis(2500), "returned after {took:?}, not at once");
    assert!(took >= Duration::from_millis(250), "returned after {took:?}, before the screen was quiet");
    assert!(v["screen_tail"].as_array().is_some_and(|t| t.iter().any(|r| r == "Password:")), "{v}");
    let hint = v["hint"].as_str().expect("hint");
    assert!(hint.contains("Do not type it") && hint.contains("Ask the user to handle it, then wait with id"), "{hint}");
    let note = v["note"].as_str().expect("the do-not-type note");
    assert!(note.contains("Do not type it"), "{v}");
}

/// The foreground ssh that runs a program on the remote host (`ssh host sleep 3`) has no shell to integrate. The precheck
/// refuses request_integration with the remote-program text, and run_command does not return early for it.
/// A shell function named ssh stands in for it here, so no network is needed: the command text is what the hint reads.
#[test]
fn request_integration_is_refused_while_an_ssh_runs_a_remote_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "ssh() { sleep 3; }").expect("define the stand-in");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let (v, _took) = timed_run(&tools, "ssh 127.0.0.1 sleep 3", 1);
    assert_eq!(v["outcome"], "running", "{v}");
    assert_eq!(v["reason"], "timeout", "a command on a remote host is not a login, so no early return: {v}");
    assert_eq!(v["foreground"]["kind"], "ssh", "{v}");

    let err = tools.integration_precheck().expect_err("refused while the ssh runs a program");
    assert_eq!(
        err,
        "cannot integrate the shell now: that command runs a program on the remote host, not a shell; wait for it with wait or stop it.",
        "{err}"
    );
    assert_eq!(err, watt_terminal_lib::core::tools::REMOTE_PROGRAM_REFUSAL);

    let w = tools.call("wait", &json!({"id": v["record_id"], "seconds": 20, "ignore_updates": true}), None).expect("wait");
    assert_eq!(w["reason"], "command_finished", "{w}");
}

/// `send_keys` types into the command the agent started: the echo and cat's copy show the text, and Ctrl-D ends cat, so the
/// call reports the finished command.
#[test]
fn send_keys_types_into_a_running_command_and_ctrl_d_finishes_it() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools.call("run_command", &json!({"command": "cat", "wait_seconds": 1}), None).expect("run_command");
    assert_eq!(v["outcome"], "running", "{v}");
    let id = v["record_id"].as_u64().expect("the record id");
    let sent = tools.call("send_keys", &json!({"text": "hello", "keys": ["Enter"], "id": id}), None).expect("send_keys");
    assert_eq!(sent["outcome"], "sent", "{sent}");
    assert_eq!(sent["bytes"], 6, "the text, then Enter: {sent}");
    let shown = wait_until(Duration::from_secs(5), || {
        (snapshot(&s).rows.iter().filter(|r| r.trim() == "hello").count() >= 2).then_some(())
    });
    assert!(shown.is_some(), "the echo and cat's copy both show: {:?}", snapshot(&s).rows);
    let done = tools.call("send_keys", &json!({"keys": ["Ctrl-D"]}), None).expect("ctrl-d");
    assert_eq!(done["outcome"], "finished", "{done}");
    assert_eq!(done["record"]["id"], id, "{done}");
    assert_eq!(done["record"]["output"].as_str().unwrap_or("").matches("hello").count(), 2, "{done}");
}

/// Ctrl-C interrupts the agent's own `sleep`: the record finishes with exit 130.
#[test]
fn send_keys_ctrl_c_interrupts_the_agents_own_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools.call("run_command", &json!({"command": "sleep 30", "wait_seconds": 1}), None).expect("run_command");
    assert_eq!(v["outcome"], "running", "{v}");
    let id = v["record_id"].as_u64().expect("the record id");
    tools.call("send_keys", &json!({"keys": ["Ctrl-C"], "id": id}), None).expect("ctrl-c");
    let w = tools.call("wait", &json!({"id": id, "seconds": 10, "ignore_updates": true}), None).expect("wait");
    assert_eq!(w["reason"], "command_finished", "{w}");
    assert_eq!(w["record"]["exit_code"], 130, "{w}");
}

/// An id that is not the command on top is refused, and nothing is typed.
#[test]
fn send_keys_refuses_an_id_that_is_not_the_command_on_top() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools.call("run_command", &json!({"command": "sleep 30", "wait_seconds": 1}), None).expect("run_command");
    let id = v["record_id"].as_u64().expect("the record id");
    let err = tools.call("send_keys", &json!({"keys": ["Ctrl-C"], "id": id + 1000}), None).expect_err("wrong id");
    assert!(err.contains(&format!("the running command is id {id}")), "error: {err}");
    tools.call("send_keys", &json!({"keys": ["Ctrl-C"]}), None).expect("the right command takes the interrupt");
    tools.call("wait", &json!({"id": id, "seconds": 10, "ignore_updates": true}), None).expect("wait");
}

/// A program asking for a password is never typed into, even by the agent's own command.
#[test]
fn send_keys_refuses_a_password_prompt() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let v = tools
        .call("run_command", &json!({"command": "printf 'Password: '; sleep 6", "wait_seconds": 1}), None)
        .expect("run_command");
    let id = v["record_id"].as_u64().expect("the record id");
    let err = tools.call("send_keys", &json!({"text": "hunter2", "keys": ["Enter"]}), None).expect_err("a password prompt");
    assert!(err.contains("asking for a password or secret"), "error: {err}");
    assert!(!snapshot(&s).rows.iter().any(|r| r.contains("hunter2")), "nothing was typed: {:?}", snapshot(&s).rows);
    tools.call("wait", &json!({"id": id, "seconds": 20, "ignore_updates": true}), None).expect("wait for the sleep to end");
}

/// Through the approval gate on a real shell: the first write shows a card, the next one runs without it, and Ctrl-D needs
/// no card at all.
#[test]
fn the_approval_gate_asks_for_the_first_send_keys_and_then_latches_the_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(5));
    let log: Log = Arc::default();
    let sink = log.clone();
    let gate_ref = &gate;
    let emit = move |ev: AgentEvent| {
        if let AgentEvent::ApprovalRequest { call_id, .. } = &ev {
            assert!(gate_ref.decide(call_id, true), "the card was waiting");
        }
        sink.lock().unwrap().push(ev);
    };
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let v = ctx.call_tool("run_command", json!({"command": "cat", "wait_seconds": 1})).expect("approved and started");
    assert_eq!(v["outcome"], "running", "{v}");
    ctx.call_tool("send_keys", json!({"text": "one", "keys": ["Enter"]})).expect("the first write asks, then goes");
    ctx.call_tool("send_keys", json!({"text": "two", "keys": ["Enter"]})).expect("the latched command takes it");
    let done = ctx.call_tool("send_keys", json!({"keys": ["Ctrl-D"]})).expect("ctrl-d");
    assert!(matches!(done["outcome"].as_str(), Some("finished" | "sent")), "{done}");
    let cards: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalRequest { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(cards, ["run_command", "send_keys"], "one card per tool that needed one");
    assert_eq!(
        reasons_of(&log),
        [
            ("run_command".to_string(), "manual", Some("manual_mode")),
            ("send_keys".to_string(), "manual", Some("send_keys")),
            ("send_keys".to_string(), "auto", Some("latched")),
            ("send_keys".to_string(), "auto", Some("interrupt")),
        ]
    );
}

/// `(name, decision, reason)` of every ToolResult, in order.
fn reasons_of(log: &Log) -> Vec<(String, &'static str, Option<&'static str>)> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolResult { name, decision, reason, .. } => Some((name.clone(), *decision, *reason)),
            _ => None,
        })
        .collect()
}

