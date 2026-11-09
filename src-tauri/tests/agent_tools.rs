//! Real-PTY tests for the agent tools, the MockAgent rules, and the approval gate. Harness: common/mod.rs.

mod common;

use std::fs;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use agent_term_lib::core::agent::{self, AgentCtx, AgentEvent, ApprovalGate, MockAgent, user_input};
use agent_term_lib::core::observer::{InputState, IntegrationStatus, ObserverStatus};
use agent_term_lib::core::record::Source;
use agent_term_lib::core::session::Session;
use agent_term_lib::core::tools::{schemas, AgentTools, RunOutcome, SessionTools, DEFAULT_RUN_TIMEOUT};
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
    let events = log.lock().unwrap().clone();
    events
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
            AgentEvent::ApprovalRequest { call_id, text } => Some((call_id.clone(), text.clone())),
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
    run(&s, "echo still-here");
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
