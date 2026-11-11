//! Integration tests for `Session::integrate_shell` (B2b). A real zsh session runs the app's own handshake: a plain
//! `zsh -f -i` (or bash, or a container) is started as the foreground program, integrated as level 1, and then checked
//! through the session's records, state and submit path. The docker cases are #[ignore]: they need docker and the image
//! named in AT_TEST_IMAGE (default ubuntu:24.04).

mod common;

use std::process::Command;
use std::thread;
use std::time::Duration;

use watt_terminal_lib::core::foreground::ForegroundKind;
use watt_terminal_lib::core::observer::{InjectRefusal, InputState};
use watt_terminal_lib::core::record::{Record, RecordStatus, RecordSummary, Source};
use watt_terminal_lib::core::session::{IntegrateOutcome, Session, SubmitError};
use common::*;

const WAIT: Duration = Duration::from_secs(20);
const NESTED_ZSH: &str = "zsh -f -i";

/// Waits until `s` runs `kind` in the foreground. False on timeout.
fn wait_foreground(s: &Session, kind: ForegroundKind) -> bool {
    wait_until(WAIT, || (s.foreground().map(|f| f.kind) == Some(kind)).then_some(())).is_some()
}

/// Integrates the shell in the foreground and returns (key, sh). Any other outcome fails the test with its details.
fn integrate_ok(s: &Session) -> (u64, String) {
    match s.integrate_shell() {
        IntegrateOutcome::Integrated { key, sh, .. } => (key, sh),
        other => panic!("integration did not succeed: {other:?}; screen: {:?}", s.screen_snapshot().rows),
    }
}

/// The record of `command` at `level` that has finished, once it has finished (the caller checks the state separately).
fn finished(s: &Session, command: &str, level: u64) -> Record {
    let id = wait_until(WAIT, || {
        s.records().iter().find(|r| r.command == command && r.level == level && r.exit_code.is_some()).map(|r| r.id)
    })
    .unwrap_or_else(|| panic!("no finished record for {command:?} at level {level}; records={:?}", s.records()));
    s.get_record(id).expect("record by id")
}

/// True when the last non-blank row of the screen ends in a shell prompt sign (`$` or `#`).
fn at_container_prompt(s: &Session) -> bool {
    let rows = s.screen_snapshot().rows;
    rows.iter().rev().find(|r| !r.trim().is_empty()).is_some_and(|r| {
        let t = r.trim_end();
        t.ends_with('$') || t.ends_with('#')
    })
}

/// The version of the bash at `path` when it is older than 4.4 (the loader refuses it). None when it is newer or cannot run.
fn old_bash_at(path: &str) -> Option<(u32, u32)> {
    let out = Command::new(path).args(["-c", "echo ${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut parts = text.split_whitespace().map(|p| p.parse::<u32>().ok());
    let version = (parts.next()??, parts.next()??);
    (version.0 < 4 || (version.0 == 4 && version.1 < 4)).then_some(version)
}

/// zsh inside zsh: integrated as level 1, its commands are attributed to that level and its location, a `zsh -f -i`
/// parent record is kept, and `exit` pops back to the local shell.
#[test]
fn zsh_inside_zsh_is_integrated_and_its_commands_are_attributed() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, NESTED_ZSH).expect("submit the nested shell");
    assert!(wait_foreground(&s, ForegroundKind::Shell), "state {:?}", s.state());
    assert!(wait_program_up(&s, NESTED_ZSH), "the nested shell never drew its prompt: {:?}", s.screen_snapshot().rows);

    let (key, sh) = integrate_ok(&s);
    assert_eq!((key, sh.as_str()), (1, "zsh"));
    assert!(wait_state(&s, InputState::Prompt, WAIT), "the nested level never reached a prompt: {:?}", s.state());
    assert_eq!(s.levels().len(), 2);
    assert_eq!(s.top_key(), 1);
    assert_eq!(s.location().as_deref(), Some(NESTED_ZSH));

    s.submit(Source::Agent { call_id: "nested-hi".into() }, "echo hi").expect("agent submit at depth 2");
    let hi = finished(&s, "echo hi", 1);
    assert_eq!((hi.exit_code, hi.level), (Some(0), 1));
    assert_eq!(hi.location.as_deref(), Some(NESTED_ZSH));
    assert_eq!(hi.output, "hi");
    assert!(matches!(&hi.source, Source::Agent { call_id } if call_id == "nested-hi"), "{:?}", hi.source);
    assert!(wait_state(&s, InputState::Prompt, WAIT));

    s.submit(Source::User, "false").expect("user submit at depth 2");
    let fals = finished(&s, "false", 1);
    assert_eq!(fals.exit_code, Some(1));
    assert!(wait_state(&s, InputState::Prompt, WAIT));

    s.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (s.levels().len() == 1 && s.state() == InputState::Prompt).then_some(())).is_some(),
        "exit did not pop back: state {:?}, levels {:?}",
        s.state(),
        s.levels()
    );
    let exited = s.records().into_iter().find(|r| r.command == "exit").expect("the exit record");
    assert_eq!((exited.level, exited.status), (1, RecordStatus::Incomplete), "the popped level's record closes Incomplete");

    // The parent record is the command that started the nested shell. Its exit is the nested shell's: a bare `exit` returns
    // the status of the last command there, which was `false` (1). It is Approximate, not Complete: its output while the
    // nested shell was on top was not captured, so the B1 rule marks it (see the B2b report).
    let parent = finished(&s, NESTED_ZSH, 0);
    assert_eq!((parent.exit_code, parent.status), (Some(1), RecordStatus::Approximate));

    let local = run(&s, "echo local-again");
    assert_eq!((local.output.as_str(), local.level), ("local-again", 0));
}

/// A plain `sleep` is not a shell: refused as NoShellProgram. At a prompt there is no command at all: NotRunning.
#[test]
fn integrate_is_refused_at_a_prompt_and_for_a_program_that_is_not_a_shell() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    assert_eq!(s.integrate_shell(), IntegrateOutcome::Refused { reason: InjectRefusal::NotRunning });

    s.submit(Source::User, "sleep 2").expect("submit");
    assert!(wait_until(WAIT, || (s.state() == InputState::Running).then_some(())).is_some());
    assert_eq!(s.integrate_shell(), IntegrateOutcome::Refused { reason: InjectRefusal::NoShellProgram });
    let slept = finished(&s, "sleep 2", 0);
    assert_eq!(slept.exit_code, Some(0), "the refused call did not disturb the command");
    assert!(wait_state(&s, InputState::Prompt, WAIT));
}

/// A shell that has printed nothing since its command began is still starting, and the Ctrl-C an integration types would
/// stop it. Here the command sleeps before it execs zsh, so integrating at once is refused as still starting, and nothing
/// is typed: the command is still running afterwards. Once zsh has drawn its prompt, the same session integrates.
#[test]
fn integrate_waits_while_a_shell_is_still_starting() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = "sh -c 'sleep 2; exec zsh -f -i'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_foreground(&s, ForegroundKind::Shell), "state {:?}", s.state());
    match s.integrate_shell() {
        IntegrateOutcome::Failed { hint } => assert!(hint.contains("still starting"), "{hint}"),
        other => panic!("expected the still-starting refusal, got {other:?}"),
    }
    assert!(s.records().iter().all(|r| r.exit_code.is_none()), "the refused call stopped the command: {:?}", s.records());
    assert!(wait_program_up(&s, cmd), "the shell never drew its prompt: {:?}", s.screen_snapshot().rows);
    let (key, sh) = integrate_ok(&s);
    assert_eq!((key, sh.as_str()), (1, "zsh"));
}

/// bash 3.2 (macOS /bin/bash) is refused by the loader: UnsupportedShell, nothing installed, and the session still works
/// once the user leaves it. Skipped with a note where /bin/bash is 4.4 or newer (or cannot run).
#[test]
fn bash_32_is_refused_as_unsupported_and_the_session_goes_on() {
    let Some((major, minor)) = old_bash_at("/bin/bash") else {
        eprintln!("SKIPPED bash_32_is_refused_as_unsupported_and_the_session_goes_on: /bin/bash is 4.4 or newer, or cannot run");
        return;
    };
    eprintln!("note: /bin/bash is {major}.{minor}");
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "/bin/bash --noprofile --norc -i").expect("submit");
    assert!(wait_foreground(&s, ForegroundKind::Shell));
    assert!(wait_program_up(&s, "/bin/bash --noprofile --norc -i"), "bash never drew its prompt");

    assert_eq!(s.integrate_shell(), IntegrateOutcome::UnsupportedShell);
    assert_eq!(s.levels().len(), 1, "a refused shell adds no level");

    s.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (s.levels().len() == 1 && s.state() == InputState::Prompt).then_some(())).is_some(),
        "exit did not return to the local prompt: {:?}",
        s.state()
    );
    let r = run(&s, "echo still-here");
    assert_eq!(r.output, "still-here");
}

/// While a nested level is on top, a 133;A with a wrong nonce is forged: counted, and it changes nothing.
#[test]
fn a_forged_prompt_while_a_level_is_on_top_only_counts() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, NESTED_ZSH).expect("submit");
    assert!(wait_foreground(&s, ForegroundKind::Shell));
    assert!(wait_program_up(&s, NESTED_ZSH), "the nested shell never drew its prompt");
    integrate_ok(&s);
    assert!(wait_state(&s, InputState::Prompt, WAIT));
    let before = s.stats().forged_events;

    let zero = "0".repeat(32);
    let cmd = format!(r"printf '\033]133;A;n={zero}\033\\'");
    s.submit(Source::Agent { call_id: "forge".into() }, &cmd).expect("submit");
    let printed = finished(&s, &cmd, 1);
    assert_eq!(printed.exit_code, Some(0));
    assert_eq!(s.stats().forged_events, before + 1, "the wrong-nonce A was not counted as forged");
    assert_eq!((s.levels().len(), s.top_key()), (2, 1), "the forged event changed the stack");
    assert!(wait_state(&s, InputState::Prompt, WAIT));
    s.write_user(b"exit\r").expect("type exit");
    assert!(wait_until(WAIT, || (s.levels().len() == 1).then_some(())).is_some());
}

/// The docker flow: `docker run -it ... bash --noprofile --norc` as the foreground program, integrated as level 1.
/// `skel` sources /etc/skel/.bashrc first (ignoreboth: repeated commands are not recorded, so their E is empty).
fn docker_bash_flow(name_tag: &str, skel: bool) {
    let image = std::env::var("AT_TEST_IMAGE").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "ubuntu:24.04".into());
    let name = format!("at-b2b-{}-{name_tag}", std::process::id());
    let _cleanup = ContainerGuard(name.clone());
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let docker = format!("docker run -it --rm --name {name} {image} bash --noprofile --norc");
    s.submit(Source::User, &docker).expect("submit docker run");
    assert!(wait_foreground(&s, ForegroundKind::ContainerExec), "state {:?}", s.state());
    assert!(wait_until(Duration::from_secs(60), || at_container_prompt(&s).then_some(())).is_some(), "no prompt from {image}");

    if skel {
        s.write_user(b". /etc/skel/.bashrc\r").expect("type the skel rc");
        thread::sleep(Duration::from_secs(1));
        assert!(wait_until(WAIT, || at_container_prompt(&s).then_some(())).is_some(), "no prompt after the skel rc");
    }

    assert!(wait_program_up(&s, &docker), "the container shell never drew its prompt");
    let (key, sh) = integrate_ok(&s);
    assert_eq!((key, sh.as_str()), (1, "bash"), "{image}");
    assert!(wait_state(&s, InputState::Prompt, WAIT), "state {:?}", s.state());
    assert_eq!(s.location().as_deref(), Some(docker.as_str()));

    // A multi-line command is refused at a bash level, and nothing is written.
    assert_eq!(
        s.submit(Source::Agent { call_id: "multi".into() }, "echo one\necho two"),
        Err(SubmitError::MultiLineAtBash)
    );

    // Two identical commands. With ignoreboth the second is not recorded by bash, so its E is empty: the agent's own
    // line must still name both records.
    for call in ["same-1", "same-2"] {
        s.submit(Source::Agent { call_id: call.into() }, "echo same").expect("agent submit at the container prompt");
        let id = wait_until(WAIT, || {
            s.records()
                .iter()
                .find(|r| matches!(&r.source, Source::Agent { call_id: c } if c == call) && r.exit_code.is_some())
                .map(|r| r.id)
        })
        .unwrap_or_else(|| panic!("no finished record for {call}; records={:?}", s.records()));
        let rec = s.get_record(id).expect("record");
        assert_eq!((rec.command.as_str(), rec.level, rec.exit_code), ("echo same", key, Some(0)), "{call}");
        assert!(wait_state(&s, InputState::Prompt, WAIT));
    }
    let same: Vec<RecordSummary> = s.records().into_iter().filter(|r| r.command == "echo same").collect();
    assert_eq!(same.len(), 2, "{same:?}");

    // exit in the container pops back to the local shell.
    s.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (s.levels().len() == 1 && s.state() == InputState::Prompt).then_some(())).is_some(),
        "exit did not pop back: state {:?}, levels {:?}",
        s.state(),
        s.levels()
    );
    let local = run(&s, "echo local-again");
    assert_eq!((local.output.as_str(), local.level), ("local-again", 0));
}

#[test]
#[ignore = "needs docker and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn docker_bash_is_integrated_refuses_multiline_and_pops_back() {
    docker_bash_flow("plain", false);
}

#[test]
#[ignore = "needs docker and the image in AT_TEST_IMAGE (default ubuntu:24.04): sources /etc/skel/.bashrc"]
fn docker_bash_with_the_skel_bashrc_still_attributes_repeated_commands() {
    docker_bash_flow("skel", true);
}

/// Removes the container by its exact name, whatever the test did.
struct ContainerGuard(String);

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}
