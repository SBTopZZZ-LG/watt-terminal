//! The agent's `request_integration` tool over a real zsh session. A plain `zsh -f -i` is the foreground program (a shell
//! without integration). The agent asks through `AgentCtx`, with the tools of a real `SessionTools`, and the answer comes
//! from the consent dialog's side: an emit callback that answers the ask, as the UI would. Nothing here needs docker.

mod common;

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use agent_term_lib::core::agent::{AgentCtx, AgentEvent, ApprovalGate, DENIED, TurnRules};
use agent_term_lib::core::foreground::ForegroundKind;
use agent_term_lib::core::observer::InputState;
use agent_term_lib::core::record::Source;
use agent_term_lib::core::session::Session;
use agent_term_lib::core::tools::{INTEGRATE_PASSWORD_REFUSAL, SessionTools};
use common::*;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);
const NESTED_ZSH: &str = "zsh -f -i";

/// Waits until `s` runs `kind` in the foreground. False on timeout.
fn wait_foreground(s: &Session, kind: ForegroundKind) -> bool {
    wait_until(WAIT, || (s.foreground().map(|f| f.kind) == Some(kind)).then_some(())).is_some()
}

/// A session at a prompt with `zsh -f -i` running in the foreground, not yet integrated.
fn nested_zsh_session() -> Harness {
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, NESTED_ZSH).expect("submit the nested shell");
    assert!(wait_foreground(&s, ForegroundKind::Shell), "state {:?}", s.state());
    assert!(wait_program_up(&s, NESTED_ZSH), "the nested shell never drew its prompt");
    s
}

/// Leaves the nested shell the user opened, and waits for the local prompt again.
fn exit_nested(s: &Session) {
    s.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (s.levels().len() == 1 && s.state() == InputState::Prompt).then_some(())).is_some(),
        "exit did not pop back: state {:?}, levels {:?}",
        s.state(),
        s.levels()
    );
}

/// The phases of the integration events, in order.
fn phases(events: &Mutex<Vec<AgentEvent>>) -> Vec<&'static str> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Integration { phase, .. } => Some(*phase),
            _ => None,
        })
        .collect()
}

/// With auto, the agent integrates the nested shell at once. No dialog, and the result says it can run commands there.
#[test]
fn auto_integrates_the_nested_shell_without_asking() {
    let _g = serial();
    let s = nested_zsh_session();
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(5));
    let events = Mutex::new(Vec::new());
    let emit = |ev: AgentEvent| events.lock().unwrap().push(ev);
    let rules = TurnRules { integrate_without_asking: true, ..TurnRules::default() };
    let ctx = AgentCtx::new(&tools, &gate, &emit).with_rules(rules);

    let result = ctx.call_tool("request_integration", json!({})).expect("integrated");
    let text = result.as_str().unwrap_or_default();
    assert!(text.starts_with("Integrated zsh -f -i (zsh"), "{text}");
    assert!(text.ends_with("You can now use run_command in this shell."), "{text}");
    assert_eq!(phases(&events), ["running", "done"], "no dialog");
    assert_eq!((s.levels().len(), s.top_key()), (2, 1));
    assert!(wait_state(&s, InputState::Prompt, WAIT), "state {:?}", s.state());

    s.submit(Source::Agent { call_id: "nested-hi".into() }, "echo hi").expect("agent submit at depth 2");
    assert!(wait_until(WAIT, || {
        s.records().iter().find(|r| r.command == "echo hi" && r.level == 1 && r.exit_code.is_some()).map(|_| ())
    })
    .is_some());
    assert!(wait_state(&s, InputState::Prompt, WAIT));
    exit_nested(&s);
}

/// The default: the user is asked. Accepting in the dialog integrates the shell, and the events are asking, running, done.
#[test]
fn an_accepted_question_integrates_the_nested_shell() {
    let _g = serial();
    let s = nested_zsh_session();
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(10));
    let events = Mutex::new(Vec::new());
    let emit = |ev: AgentEvent| {
        if let AgentEvent::Integration { call_id, phase: "asking", label, .. } = &ev {
            assert_eq!(label.as_deref(), Some(NESTED_ZSH));
            assert!(gate.integrations().decide(call_id, true), "the dialog's answer is waiting");
        }
        events.lock().unwrap().push(ev);
    };
    let ctx = AgentCtx::new(&tools, &gate, &emit);

    let result = ctx.call_tool("request_integration", json!({})).expect("integrated");
    assert!(result.as_str().unwrap_or_default().starts_with("Integrated zsh -f -i (zsh"), "{result}");
    assert_eq!(phases(&events), ["asking", "running", "done"]);
    assert_eq!(s.levels().len(), 2);
    assert!(wait_state(&s, InputState::Prompt, WAIT));
    exit_nested(&s);
}

/// A declined question is refused: no level is pushed, the shell stays as it was, and the error says what to do instead.
/// It is not a DENIED error, so the turn goes on.
#[test]
fn a_declined_question_leaves_the_shell_alone() {
    let _g = serial();
    let s = nested_zsh_session();
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(10));
    let events = Mutex::new(Vec::new());
    let emit = |ev: AgentEvent| {
        if let AgentEvent::Integration { call_id, phase: "asking", .. } = &ev {
            assert!(gate.integrations().decide(call_id, false));
        }
        events.lock().unwrap().push(ev);
    };
    let ctx = AgentCtx::new(&tools, &gate, &emit);

    let err = ctx.call_tool("request_integration", json!({})).expect_err("declined");
    assert!(err.starts_with("declined: "), "{err}");
    assert!(!err.starts_with(DENIED), "a decline must not stop the turn: {err}");
    assert_eq!(phases(&events), ["asking", "done"]);
    assert_eq!(s.levels().len(), 1, "a declined shell adds no level");
    assert_eq!(s.foreground().map(|f| f.kind), Some(ForegroundKind::Shell), "the shell is still in the foreground");
    exit_nested(&s);
}

/// A question nobody answers is declined when it times out, and that does not stop the turn either.
#[test]
fn an_unanswered_question_is_declined_after_its_timeout() {
    let _g = serial();
    let s = nested_zsh_session();
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_millis(300));
    let emit = |_: AgentEvent| {};
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let err = ctx.call_tool("request_integration", json!({})).expect_err("timed out");
    assert!(err.starts_with("declined: "), "{err}");
    assert!(!err.starts_with(DENIED), "{err}");
    assert_eq!(s.levels().len(), 1);
    exit_nested(&s);
}

/// At a plain prompt there is nothing to integrate: refused before any question, with the text for it.
#[test]
fn at_a_prompt_the_request_is_refused_without_a_question() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(5));
    let events = Mutex::new(Vec::new());
    let emit = |ev: AgentEvent| events.lock().unwrap().push(ev);
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let err = ctx.call_tool("request_integration", json!({})).expect_err("refused");
    assert!(err.starts_with("cannot integrate the shell now: no command is running in this shell"), "{err}");
    assert!(phases(&events).is_empty(), "no dialog for a refusal");
}

/// While the shell is still starting (it has printed nothing since its command began) the request is refused before any
/// question: the text says to wait or look at the screen first, and no dialog opens. (The integration itself after the
/// shell is up is tested in integrate_session.rs.)
#[test]
fn a_request_is_refused_while_the_shell_is_still_starting() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = "sh -c 'sleep 2; exec zsh -f -i'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_foreground(&s, ForegroundKind::Shell), "state {:?}", s.state());
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(5));
    let events = Mutex::new(Vec::new());
    let emit = |ev: AgentEvent| events.lock().unwrap().push(ev);
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let err = ctx.call_tool("request_integration", json!({})).expect_err("refused while the shell starts");
    assert!(err.starts_with("cannot integrate the shell yet: it is still starting"), "{err}");
    assert!(err.contains("Wait a few seconds") && err.contains("get_screen"), "{err}");
    assert!(phases(&events).is_empty(), "no dialog for a refusal");
    assert!(wait_program_up(&s, cmd), "the shell never drew its prompt: {:?}", s.screen_snapshot().rows);
}

/// A program that is not a shell is refused, and the command it runs is not disturbed.
#[test]
fn a_program_that_is_not_a_shell_is_refused() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 2").expect("submit");
    assert!(wait_until(WAIT, || (s.state() == InputState::Running).then_some(())).is_some());
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(5));
    let emit = |_: AgentEvent| {};
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let err = ctx.call_tool("request_integration", json!({})).expect_err("refused");
    assert!(err.contains("the running program is not a shell, ssh, su or a container exec"), "{err}");
    assert!(wait_state(&s, InputState::Prompt, WAIT), "the sleep finished on its own");
}

/// A shell that waits for a password is refused before the loader can be typed into it. The agent is told not to type.
#[test]
fn a_password_prompt_refuses_before_anything_is_typed() {
    let _g = serial();
    let s = nested_zsh_session();
    s.write_user(b"printf 'Password: '; read -r secret\r").expect("type a prompt that waits");
    assert!(
        wait_until(WAIT, || {
            let rows = s.screen_snapshot().rows;
            let last = rows.iter().rev().find(|r| !r.trim().is_empty())?;
            last.trim_end().ends_with("Password:").then_some(())
        })
        .is_some(),
        "the prompt did not show: {:?}",
        s.screen_snapshot().rows
    );

    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(5));
    let events = Mutex::new(Vec::new());
    let emit = |ev: AgentEvent| events.lock().unwrap().push(ev);
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let err = ctx.call_tool("request_integration", json!({})).expect_err("refused");
    assert_eq!(err, INTEGRATE_PASSWORD_REFUSAL);
    assert!(phases(&events).is_empty(), "no dialog while a password is asked");
    assert_eq!(s.levels().len(), 1, "nothing was typed into the prompt");

    s.write_user(b"\x03").expect("Ctrl-C the read");
    thread::sleep(Duration::from_millis(300));
    exit_nested(&s);
}

/// The result of a granted request is the value the model reads; it is a plain string, not a JSON object.
#[test]
fn the_granted_result_is_text() {
    let _g = serial();
    let s = nested_zsh_session();
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(10));
    let emit = |ev: AgentEvent| {
        if let AgentEvent::Integration { call_id, phase: "asking", .. } = &ev {
            assert!(gate.integrations().decide(call_id, true));
        }
    };
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let result: Value = ctx.call_tool("request_integration", json!({})).expect("integrated");
    assert!(result.is_string(), "{result}");
    assert!(wait_state(&s, InputState::Prompt, WAIT));
    exit_nested(&s);
}
