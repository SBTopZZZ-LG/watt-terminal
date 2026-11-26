//! The agent's `request_integration` tool over a real zsh session. A plain `zsh -f -i` is the foreground program (a shell
//! without integration). The agent asks through `AgentCtx`, with the tools of a real `SessionTools`, and the answer comes
//! from the consent dialog's side: an emit callback that answers the ask, as the UI would. Nothing here needs docker.

// Every test runs a zsh session through common/mod.rs: no zsh on Windows.
#![cfg(unix)]

mod common;

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use watt_terminal_lib::core::agent::{AgentCtx, AgentEvent, ApprovalGate, DENIED, TurnRules};
use watt_terminal_lib::core::foreground::ForegroundKind;
use watt_terminal_lib::core::observer::InputState;
use watt_terminal_lib::core::record::Source;
use watt_terminal_lib::core::session::Session;
use watt_terminal_lib::core::tools::{AgentTools, DEFAULT_RUN_WAIT, INTEGRATE_PASSWORD_REFUSAL, RunOutcome, SessionTools};
use common::*;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);
const NESTED_ZSH: &str = "zsh -f -i";

/// Waits until `s` runs `kind` in the foreground. False on timeout.
fn wait_foreground(s: &Session, kind: ForegroundKind) -> bool {
    wait_until(WAIT, || (s.foreground().map(|f| f.kind) == Some(kind)).then_some(())).is_some()
}

/// Waits until the screen of `s` has been still for `quiet`. False on timeout (WAIT).
fn wait_screen_quiet(s: &Session, quiet: Duration) -> bool {
    let end = Instant::now() + WAIT;
    let mut seen = s.activity().screen_gen;
    let mut still_since = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(20));
        let generation = s.activity().screen_gen;
        if generation != seen {
            seen = generation;
            still_since = Instant::now();
        }
        if still_since.elapsed() >= quiet {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
    }
}

/// A session at a prompt with `zsh -f -i` running in the foreground, not yet integrated.
fn nested_zsh_session() -> Harness {
    let s = start();
    open_nested_zsh(&s);
    s
}

/// Starts `zsh -f -i` at the local prompt of `s`, and waits until it has drawn its prompt.
fn open_nested_zsh(s: &Session) {
    wait_prompt(s);
    s.submit(Source::User, NESTED_ZSH).expect("submit the nested shell");
    assert!(wait_foreground(s, ForegroundKind::Shell), "state {:?}", s.state());
    assert!(wait_program_up(s, NESTED_ZSH), "the nested shell never drew its prompt");
}

/// Linux procfs: the `stat` and `wchan` of `pid`, and its direct children (pid and comm from `stat`).
fn proc_lines(pid: u32) -> String {
    let read = |f: &str| std::fs::read_to_string(format!("/proc/{pid}/{f}")).unwrap_or_default();
    let stat = read("stat");
    let mut out = format!("  pid={pid} stat={:?} wchan={:?}\n", stat.trim(), read("wchan").trim());
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in dir.flatten() {
        let Ok(child) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(cstat) = std::fs::read_to_string(format!("/proc/{child}/stat")) else { continue };
        // Fields after the last ')' are: state ppid ...; comm may itself contain spaces or ')'.
        let Some(after) = cstat.rfind(')').map(|i| &cstat[i + 1..]) else { continue };
        let mut fields = after.split_whitespace();
        let _state = fields.next();
        if fields.next().and_then(|p| p.parse::<u32>().ok()) == Some(pid) {
            let wchan = std::fs::read_to_string(format!("/proc/{child}/wchan")).unwrap_or_default();
            out.push_str(&format!("  child pid={child} stat={:?} wchan={:?}\n", cstat.trim(), wchan.trim()));
        }
    }
    out
}

/// The `run_command` Err path of the nested-prompt test: panics with what the harness sees, so a flake names its cause. It first
/// waits up to 10 s more for the prompt (state Prompt and a prompt count above `before`), and reports whether it came late.
fn run_command_failure(s: &Session, raw: &Mutex<Vec<u8>>, before: u64, call_at: Instant, err: &str) -> ! {
    let err_at = Instant::now();
    let late = wait_until(Duration::from_secs(10), || {
        (s.state() == InputState::Prompt && s.prompt_count() > before).then(Instant::now)
    });
    let late = match late {
        Some(at) => format!(
            "prompt ARRIVED late: {}ms after the Err, {}ms after the call",
            at.duration_since(err_at).as_millis(),
            at.duration_since(call_at).as_millis()
        ),
        None => format!("prompt did NOT arrive within 10s after the Err (prompt_count now {}, before the call {before})", s.prompt_count()),
    };
    let bytes = raw.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let tail = &bytes[bytes.len().saturating_sub(4096)..];
    let snap = s.screen_snapshot();
    let procs = s.shell_pid().map(proc_lines).unwrap_or_else(|| "  (no shell pid)\n".into());
    panic!(
        "run_command failed: {err}
late prompt: {late}
state={:?} prompt_count before-call={before} now={} user_writes={} typed_since_prompt={}
levels={:?}
foreground={:?}
records={:?}
shell_pid={:?} procfs (local shell and its children):
{procs}screen size={:?} cursor={:?} alternate={}
rows:
{}
raw bytes total={} tail(4096)={:?}",
        s.state(),
        s.prompt_count(),
        s.user_writes(),
        s.user_typed_since_prompt(),
        s.levels(),
        s.foreground(),
        s.records(),
        s.shell_pid(),
        snap.size,
        snap.cursor,
        snap.alternate_screen,
        snap.rows.iter().enumerate().map(|(i, r)| format!("  {i:2}|{r}")).collect::<Vec<_>>().join("\n"),
        bytes.len(),
        String::from_utf8_lossy(tail),
    );
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
    let key = s.top_key();
    assert_ne!(key, 0, "the nested level has its own key");
    assert_eq!(s.levels().len(), 2);
    assert!(wait_state(&s, InputState::Prompt, WAIT), "state {:?}", s.state());

    s.submit(Source::Agent { call_id: "nested-hi".into() }, "echo hi").expect("agent submit at depth 2");
    assert!(wait_until(WAIT, || {
        s.records().iter().find(|r| r.command == "echo hi" && r.level == key && r.exit_code.is_some()).map(|_| ())
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

/// A question nobody answers says so when it times out (not a decline), and that does not stop the turn either.
#[test]
fn an_unanswered_question_says_no_answer_after_its_timeout() {
    let _g = serial();
    let s = nested_zsh_session();
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_millis(300));
    let emit = |_: AgentEvent| {};
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let err = ctx.call_tool("request_integration", json!({})).expect_err("timed out");
    assert!(err.starts_with("no answer: "), "{err}");
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

/// Text the user typed at the integrated nested prompt (no Enter) is cleared by run_command: its Ctrl-C makes a fresh prompt
/// at the nested level, which run_command waits for. The command then runs in the nested shell, and the text never runs.
#[test]
fn typed_text_at_a_nested_prompt_is_cleared_before_run_command() {
    let _g = serial();
    let (s, raw) = start_recording();
    open_nested_zsh(&s);
    let tools = SessionTools::new(&s);
    let gate = ApprovalGate::with_timeout(Duration::from_secs(5));
    let emit = |_: AgentEvent| {};
    let rules = TurnRules { integrate_without_asking: true, ..TurnRules::default() };
    let ctx = AgentCtx::new(&tools, &gate, &emit).with_rules(rules);
    ctx.call_tool("request_integration", json!({})).expect("integrated");
    assert!(wait_state(&s, InputState::Prompt, WAIT), "state {:?}", s.state());

    // Type only once the nested shell has settled. Bytes written right after its prompt start can be lost while zsh switches
    // its line editor on, so the Ctrl-C that run_command sends after them is not acted on. Then wait for the echo.
    assert!(wait_screen_quiet(&s, PROGRAM_QUIET), "the nested prompt never settled: {:?}", s.screen_snapshot().rows);
    s.write_user(b"echo user-typed").expect("the user types at the nested prompt");
    assert!(wait_until(WAIT, || s.user_typed_since_prompt().then_some(())).is_some(), "the typed text is not counted");
    assert!(
        wait_until(Duration::from_secs(5), || s.screen_snapshot().rows.iter().any(|r| r.contains("echo user-typed")).then_some(())).is_some(),
        "the typed text was not echoed at the nested prompt: {:?}",
        s.screen_snapshot().rows
    );

    let before = s.prompt_count();
    let call_at = Instant::now();
    let outcome = match tools.run_command("echo nested-after-clear", DEFAULT_RUN_WAIT, None) {
        Ok(outcome) => outcome,
        Err(err) => run_command_failure(&s, &raw, before, call_at, &err),
    };
    let RunOutcome::Finished { record } = outcome else {
        panic!("the command did not finish: state {:?}", s.state());
    };
    assert_eq!(record.exit_code, Some(0), "output={:?}", record.output);
    assert_eq!(record.output, "nested-after-clear", "the record holds more than the agent's line");
    assert!(!s.records().iter().any(|r| r.command.contains("user-typed")), "the user's text ran: {:?}", s.records());
    assert!(wait_state(&s, InputState::Prompt, WAIT));
    exit_nested(&s);
}
