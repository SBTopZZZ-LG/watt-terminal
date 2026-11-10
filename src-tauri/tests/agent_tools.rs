//! Real-PTY tests for the agent tools, the MockAgent rules, and the approval gate. Harness: common/mod.rs.

mod common;

use std::fs;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_term_lib::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate, DENIED, MockAgent, user_input};
use agent_term_lib::core::llm::TurnCancel;
use agent_term_lib::core::observer::{InputState, IntegrationStatus, ObserverStatus};
use agent_term_lib::core::record::Source;
use agent_term_lib::core::session::{Session, USER_TEXT_CODE};
use agent_term_lib::core::tools::{schemas, AgentTools, NotRunReason, RunOutcome, SessionTools, DEFAULT_RUN_TIMEOUT};
use serde_json::json;

use common::*;

type Log = Arc<Mutex<Vec<AgentEvent>>>;

/// Runs one turn on this thread and returns every event it emitted.
fn turn(s: &Session, approvals: &ApprovalGate, text: &str) -> Vec<AgentEvent> {
    let log: Log = Arc::default();
    let sink = log.clone();
    let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
    let tools = SessionTools::new(s);
    let ctx = AgentCtx::new(&tools, approvals, &emit);
    agent::run_turn(&mut MockAgent, text, &ctx);
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
        agent::run_turn(&mut MockAgent, &text, &ctx);
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

/// Waits for the first approval request and returns (call_id, text).
fn wait_approval(log: &Log) -> (String, String) {
    wait_until(Duration::from_secs(10), || approval_ids(log).first().cloned()).expect("an approval was requested")
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
    let v = tools.call("list_commands", &json!({"n": 1})).expect("list_commands");
    assert_eq!(v[0]["command"], "seq 1 200");
    let v = tools.call("get_command", &json!({"id": big.id, "limit": 16})).expect("get_command");
    assert_eq!(v["output"].as_str(), Some(&full[..16]));
    assert_eq!(v["truncated"], true);
    assert!(tools.call("get_command", &json!({})).is_err(), "id is required");
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
    let outcome = tools.run_command("pwd", DEFAULT_RUN_TIMEOUT).expect("run_command");
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
    let err = tools.run_command("echo refused-running", DEFAULT_RUN_TIMEOUT).expect_err("refused while running");
    assert!(err.contains("Running"), "error: {err}");
    run_finish(&s, "sleep 3");

    let cmd = r"printf '\033[?1049h'; sleep 2; printf '\033[?1049l'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Alt, Duration::from_secs(5)), "state={:?}", s.state());
    let err = tools.run_command("echo refused-alt", DEFAULT_RUN_TIMEOUT).expect_err("refused in alt screen");
    assert!(err.contains("Alt"), "error: {err}");
    run_finish(&s, cmd);
    assert!(!s.records().iter().any(|r| r.command.starts_with("echo refused")), "a refused command ran");
}

#[test]
fn run_command_times_out_cleanly_and_session_stays_usable() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let outcome = tools.run_command("sleep 30", Duration::from_millis(500)).expect("run_command");
    match outcome {
        // Usually Running; Submitted if the C event had not arrived yet when the timeout expired.
        RunOutcome::TimedOut { state: InputState::Running | InputState::Submitted } => {}
        other => panic!("expected a timeout, got {other:?}"),
    }
    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
    let r = run(&s, "echo alive");
    assert_eq!(r.output, "alive");
    assert!(s.records().iter().any(|r| r.command == "sleep 30" && r.exit_code.is_some()), "the timed-out command never finished");
}

#[test]
fn send_keys_is_disabled_and_schemas_list_every_tool() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let err = tools.send_keys(b"ls\r").expect_err("send_keys must fail in slice 1");
    assert!(err.contains("disabled in slice 1"), "error: {err}");
    let err = tools.call("send_keys", &json!({"bytes": [108, 115, 13]})).expect_err("send_keys via JSON");
    assert!(err.contains("disabled in slice 1"), "error: {err}");
    let names: Vec<&str> = schemas().iter().map(|t| t.name).collect();
    assert_eq!(names, ["list_commands", "get_command", "get_screen", "get_cwd", "run_command", "send_keys"]);
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
        SessionTools::new(&s2).run_command("echo AGENT", DEFAULT_RUN_TIMEOUT)
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
    approvals.set_auto_approve(true);
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
    let (id, shown) = wait_approval(&log);
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
    let (id, _) = wait_approval(&log);
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
    let (id, _) = wait_approval(&log);
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
    let call = thread::spawn(move || SessionTools::new(&s1).with_cancel(c1).run_command("sleep 2; echo cancel-kept", DEFAULT_RUN_TIMEOUT));
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(10)), "state={:?}", s.state());
    let cancelled_at = Instant::now();
    cancel.cancel();
    let outcome = call.join().expect("call thread").expect("run_command");
    let took = cancelled_at.elapsed();
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::Cancelled }), "got {outcome:?}");
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
    let outcome = SessionTools::new(&s).with_cancel(cancel).run_command("echo never-sent", DEFAULT_RUN_TIMEOUT).expect("run_command");
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::Cancelled }), "got {outcome:?}");
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
    approvals.set_auto_approve(true);
    let token = Arc::new(TurnCancel::new());
    let slot = Arc::new(Mutex::new(Some(token.clone())));
    let log: Log = Arc::default();
    let (s1, a1, sink) = (s.arc(), approvals.clone(), log.clone());
    let turn = thread::spawn(move || {
        let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
        let tools = SessionTools::new(&s1).with_cancel(token);
        let ctx = AgentCtx::new(&tools, &a1, &emit);
        agent::run_turn(&mut MockAgent, "please run `sleep 2; echo turn-kept`", &ctx);
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
    auto.set_auto_approve(true);
    let events = turn(&s, &auto, "please run `echo dec-auto`");
    assert_eq!(decision(&events), [("run_command".to_string(), "auto", true)]);

    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `echo dec-manual`");
    let (id, _) = wait_approval(&log);
    assert!(approvals.decide(&id, true));
    h.join().expect("turn");
    assert_eq!(decision(&log.lock().unwrap()), [("run_command".to_string(), "manual", true)]);

    let approvals = Arc::new(ApprovalGate::new());
    let (h, log) = turn_in_thread(s.arc(), approvals.clone(), "please run `echo dec-denied`");
    let (id, _) = wait_approval(&log);
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
    wait_approval(&log);
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
    let (id, _) = wait_approval(&log);
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
    let err = tools.run_command("echo agent-marker", DEFAULT_RUN_TIMEOUT).expect_err("refused while text is typed");
    assert!(err.starts_with(USER_TEXT_CODE), "error: {err}");
    assert!(!s.records().iter().any(|r| r.command.contains("agent-marker")), "the refused command ran");
    assert!(!screen_has(&s, "agent-marker"), "the refused command reached the screen");
    assert!(screen_has(&s, "echo typed-by-user"), "the user's line changed");

    s.write_user(&[0x03]).expect("ctrl-c");
    assert!(wait_until(Duration::from_secs(5), || (!prompt_line(&s).contains("typed-by-user")).then_some(())).is_some(), "ctrl-c did not clear the prompt line");
    wait_prompt(&s);
    let outcome = tools.run_command("echo after-ctrl-c", DEFAULT_RUN_TIMEOUT).expect("runs after ctrl-c");
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
    let (id, _) = wait_approval(&log);
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
    let RunOutcome::Finished { record } = tools.run_command("echo untouched", DEFAULT_RUN_TIMEOUT).expect("run") else {
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
    let RunOutcome::Finished { record } = tools.run_command("echo agent-next", DEFAULT_RUN_TIMEOUT).expect("run") else {
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
        SessionTools::new(&s).run_command("echo next-ok", DEFAULT_RUN_TIMEOUT).expect("not refused")
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
    let err = tools.run_command("echo type-ahead-marker", DEFAULT_RUN_TIMEOUT).expect_err("refused over type-ahead");
    assert!(err.starts_with(USER_TEXT_CODE), "error: {err}");
    assert!(!s.records().iter().any(|r| r.command.contains("type-ahead-marker")), "the refused command ran");
    assert!(!screen_has(&s, "type-ahead-marker"), "the refused command reached the screen");

    s.write_user(&[0x03]).expect("ctrl-c at the prompt");
    assert!(wait_until(Duration::from_secs(5), || (!prompt_line(&s).contains("zqx")).then_some(())).is_some(), "ctrl-c did not clear the prompt line");
    wait_prompt(&s);
    let RunOutcome::Finished { record } = tools.run_command("echo after-type-ahead", DEFAULT_RUN_TIMEOUT).expect("runs after ctrl-c") else {
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
    let RunOutcome::Finished { record } = tools.run_command("echo after-alt", DEFAULT_RUN_TIMEOUT).expect("runs after the alt screen") else {
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
    let outcome = tools.run_command("echo `x", DEFAULT_RUN_TIMEOUT).expect("run_command");
    let waited = began.elapsed();
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::WaitingForInput }), "got {outcome:?}");
    assert!(waited >= Duration::from_secs(2) && waited < Duration::from_secs(8), "returned after {waited:?}");
    assert_eq!(s.state(), InputState::Continuation, "the shell is not waiting");

    s.write_user(&[0x03]).expect("ctrl-c");
    wait_prompt(&s);
    assert!(!s.records().iter().any(|r| r.command.contains("echo `x")), "the unfinished line became a record");
    let RunOutcome::Finished { record } = tools.run_command("echo ok", DEFAULT_RUN_TIMEOUT).expect("run after ctrl-c") else {
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
    approvals.set_auto_approve(true);
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
        let outcome = SessionTools::new(&s1).run_command("echo `x", DEFAULT_RUN_TIMEOUT);
        (outcome, t1.elapsed())
    });
    assert!(wait_state(&s, InputState::Continuation, Duration::from_secs(10)), "state={:?}", s.state());
    s.write_user(&[0x03]).expect("ctrl-c");
    let (outcome, took) = call.join().expect("call thread");
    let outcome = outcome.expect("run_command");
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::Abandoned }), "got {outcome:?}");
    assert!(took < Duration::from_secs(2), "the abandoned call took {took:?}");
    wait_prompt(&s);
    let RunOutcome::Finished { record } = SessionTools::new(&s).run_command("echo after-abandon", DEFAULT_RUN_TIMEOUT).expect("run")
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
        let outcome = SessionTools::new(&s1).run_command("echo `x", DEFAULT_RUN_TIMEOUT);
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
