//! Agent calls in a nested shell (B3): the approval card is bound to the shell it was made for, an allow rule asks there,
//! approve-all is tied to the level it was switched on in, and the nested shell's own cwd reaches the tools. The nested
//! shell is a real `zsh -f -i`, integrated as level 1 by the app's own handshake (see tests/integrate_session.rs).
//! Harness: common/mod.rs. The unit tests of the same rules are in core/agent.rs and core/approval.rs.

mod common;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use watt_terminal_lib::core::agent::{AgentCtx, AgentEvent, ApprovalGate, DENIED};
use watt_terminal_lib::core::approval::RegexPolicy;
use watt_terminal_lib::core::foreground::ForegroundKind;
use watt_terminal_lib::core::observer::InputState;
use watt_terminal_lib::core::record::Source;
use watt_terminal_lib::core::session::{IntegrateOutcome, Session};
use watt_terminal_lib::core::settings::{ApprovalMode, ApprovalSettings};
use watt_terminal_lib::core::tools::{AgentTools, SessionTools};
use serde_json::{json, Value};
use common::*;

const WAIT: Duration = Duration::from_secs(20);
const NESTED_ZSH: &str = "zsh -f -i";

type Log = Arc<Mutex<Vec<AgentEvent>>>;

/// Waits until the foreground program is `kind`. False on timeout.
fn wait_foreground(s: &Session, kind: ForegroundKind) -> bool {
    wait_until(WAIT, || (s.foreground().map(|f| f.kind) == Some(kind)).then_some(())).is_some()
}

/// Starts `zsh -f -i` as the foreground program and integrates it as level 1. Returns once level 1 is on top at its prompt.
fn nested_zsh(s: &Session) {
    wait_prompt(s);
    s.submit(Source::User, NESTED_ZSH).expect("submit the nested shell");
    assert!(wait_foreground(s, ForegroundKind::Shell), "state {:?}", s.state());
    assert!(wait_program_up(s, NESTED_ZSH), "the nested shell never drew its prompt");
    match s.integrate_shell() {
        IntegrateOutcome::Integrated { key: 1, .. } => {}
        other => panic!("integration did not succeed: {other:?}; screen: {:?}", s.screen_snapshot().rows),
    }
    assert!(
        wait_until(WAIT, || (s.state() == InputState::Prompt && s.top_key() == 1).then_some(())).is_some(),
        "level 1 never reached its prompt: state {:?}",
        s.state()
    );
}

/// Makes one run_command call the way the agent does (through the gate and `policy`), on its own thread, so the test can
/// answer the card. The thread's result is the call's result. The log holds the events it emitted.
fn agent_run(
    s: Arc<Session>,
    gate: Arc<ApprovalGate>,
    policy: RegexPolicy,
    command: &str,
) -> (thread::JoinHandle<Result<Value, String>>, Log) {
    let log: Log = Arc::default();
    let sink = log.clone();
    let command = command.to_string();
    let handle = thread::spawn(move || {
        let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
        let tools = SessionTools::new(&s);
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        ctx.call_tool("run_command", json!({"command": command}))
    });
    (handle, log)
}

/// The first approval card in `log`: its call id, reason code, level and location. Waits for it.
fn wait_card(log: &Log) -> (String, Option<&'static str>, u64, Option<String>) {
    wait_until(WAIT, || {
        log.lock().unwrap().iter().find_map(|e| match e {
            AgentEvent::ApprovalRequest { call_id, reason, level, location, .. } => {
                Some((call_id.clone(), *reason, *level, location.clone()))
            }
            _ => None,
        })
    })
    .expect("an approval card")
}

/// `(decision, ok)` of the run_command result in `log`, once the call has returned.
fn run_decision(log: &Log) -> Option<(&'static str, bool)> {
    log.lock().unwrap().iter().find_map(|e| match e {
        AgentEvent::ToolResult { name, decision, ok, .. } if name == "run_command" => Some((*decision, *ok)),
        _ => None,
    })
}

/// A policy whose allow list is `allow` (anchored, as the settings are). Nothing else is saved.
fn rules(allow: &[&str]) -> RegexPolicy {
    let approval = ApprovalSettings {
        mode: ApprovalMode::Regex,
        allow: allow.iter().map(|s| s.to_string()).collect(),
        deny: Vec::new(),
        ..Default::default()
    };
    RegexPolicy::new(&approval).expect("the rules compile")
}

/// The card is for the nested shell it was made in. The user leaves that shell while the card waits. The approval then
/// reaches the writer, which refuses it with LocationChanged: nothing is typed into the local shell, and no record of the
/// command is made.
#[test]
fn an_approval_is_refused_once_the_nested_shell_it_was_made_for_has_gone() {
    let _g = serial();
    let h = start();
    nested_zsh(&h);
    let s = h.arc();
    let gate = Arc::new(ApprovalGate::new());

    let (call, log) = agent_run(s.clone(), gate.clone(), rules(&[]), "echo left-behind");
    let (call_id, reason, level, location) = wait_card(&log);
    assert_eq!((reason, level, location.as_deref()), (Some("no_rule"), 1, Some(NESTED_ZSH)));

    // The user types exit at the nested prompt. The local shell is on top again before the approval is given.
    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.top_key() == 0 && h.state() == InputState::Prompt).then_some(())).is_some(),
        "the nested shell did not exit: state {:?}, levels {:?}",
        h.state(),
        h.levels()
    );
    assert!(gate.decide(&call_id, true), "the card was still pending");

    let err = call.join().expect("the turn thread").expect_err("refused: the shell changed");
    assert!(err.contains("the shell you were working in has changed"), "{err}");
    assert!(!err.starts_with(DENIED), "a refusal after an approval is not a denial: {err}");
    assert_eq!(run_decision(&log), Some(("manual", false)), "the user approved, so the decision stays manual");
    assert!(!h.records().iter().any(|r| r.command == "echo left-behind"), "the command ran");
    assert!(!h.screen_snapshot().rows.iter().any(|r| r.contains("left-behind")), "the command was typed");
}

/// A saved allow rule runs a command on the local shell without a card. In the nested shell the same rule asks, with the
/// nested reason, and the card names the nested shell. A refusal there runs nothing.
#[test]
fn an_allow_rule_asks_in_the_nested_shell_and_runs_locally_without_a_card() {
    let _g = serial();
    let h = start();
    wait_prompt(&h);
    let s = h.arc();
    let gate = Arc::new(ApprovalGate::with_timeout(Duration::from_secs(5)));

    let (local, local_log) = agent_run(s.clone(), gate.clone(), rules(&["echo .*"]), "echo local-rule");
    assert_eq!(local.join().expect("thread").map(|_| ()), Ok(()));
    assert_eq!(run_decision(&local_log), Some(("auto", true)));
    assert!(!local_log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));

    nested_zsh(&h);
    let (nested, nested_log) = agent_run(s.clone(), gate.clone(), rules(&["echo .*"]), "echo nested-rule");
    let (call_id, reason, level, _) = wait_card(&nested_log);
    assert_eq!((reason, level), (Some("nested_level"), 1));
    assert!(gate.decide(&call_id, false), "the card was still pending");
    let err = nested.join().expect("thread").expect_err("denied");
    assert!(err.starts_with(DENIED), "{err}");
    assert!(!h.records().iter().any(|r| r.command == "echo nested-rule"), "the denied command ran");
}

/// The nested shell's own cwd (its 633;Cwd) is what the tools report while it is on top.
#[test]
fn the_cwd_while_a_nested_shell_is_on_top_is_its_own() {
    let _g = serial();
    let h = start();
    nested_zsh(&h);
    h.submit(Source::User, "cd /usr").expect("cd in the nested shell");
    assert!(
        wait_until(WAIT, || h.cwd().filter(|c| c.path == "/usr").map(|_| ())).is_some(),
        "the nested shell's cwd never reached the session: {:?}",
        h.cwd()
    );
    let via_tools = SessionTools::new(&h).get_cwd().expect("a cwd through the tools");
    assert_eq!(via_tools.path, "/usr");
}

/// Approve-all switched on in the local shell does not run a command in the nested shell. The card asks with the
/// suspended reason. Back on the local shell, approve-all applies again.
#[test]
fn approve_all_from_the_local_shell_is_suspended_in_the_nested_shell() {
    let _g = serial();
    let h = start();
    wait_prompt(&h);
    let s = h.arc();
    let gate = Arc::new(ApprovalGate::with_timeout(Duration::from_secs(5)));
    gate.set_auto_approve(true, h.top_key());

    let (local, local_log) = agent_run(s.clone(), gate.clone(), rules(&[]), "echo local-all");
    assert_eq!(local.join().expect("thread").map(|_| ()), Ok(()));
    assert_eq!(run_decision(&local_log), Some(("auto", true)));

    nested_zsh(&h);
    let (nested, nested_log) = agent_run(s.clone(), gate.clone(), rules(&[]), "echo nested-all");
    let (call_id, reason, level, _) = wait_card(&nested_log);
    assert_eq!((reason, level), (Some("approve_all_suspended"), 1));
    assert!(gate.decide(&call_id, false), "the card was still pending");
    assert!(nested.join().expect("thread").is_err());

    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.top_key() == 0 && h.state() == InputState::Prompt).then_some(())).is_some(),
        "the nested shell did not exit"
    );
    let (again, again_log) = agent_run(s.clone(), gate.clone(), rules(&[]), "echo local-again");
    assert_eq!(again.join().expect("thread").map(|_| ()), Ok(()));
    assert_eq!(run_decision(&again_log), Some(("auto", true)), "approve-all resumes on the local shell");
}
