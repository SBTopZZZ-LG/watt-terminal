//! Windows-only end-to-end test of nested PowerShell integration. A PowerShell session on ConPTY (core/shim/integration.ps1)
//! starts another pwsh as the foreground program. The app integrates that pwsh as level 1 through `integrate_shell`, the
//! entry the Integrate chip and request_integration use, and commands run in it. The nested pwsh is started with
//! -HistorySaveStyle SaveNothing, so it writes no PSReadLine history; the outer session's history goes to WATT_HISTFILE.
//! Every wait has a timeout. Does not use common/mod.rs, which starts zsh.

#![cfg(windows)]

use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use watt_terminal_lib::core::foreground::ForegroundKind;
use watt_terminal_lib::core::observer::InputState;
use watt_terminal_lib::core::pty::OutputSink;
use watt_terminal_lib::core::record::{Record, Source};
use watt_terminal_lib::core::session::{IntegrateOutcome, Session, SpawnConfig};

static SERIAL: Mutex<()> = Mutex::new(());
const WAIT: Duration = Duration::from_secs(30);
/// Cursor position query. Each pwsh sends it at startup and waits for the answer.
const DSR: &[u8] = b"\x1b[6n";
/// The nested shell's start line. -NoExit keeps it interactive after -Command; SaveNothing keeps its history out of files.
const NESTED: &str = "pwsh -NoLogo -NoProfile -NoExit -Command Set-PSReadLineOption -HistorySaveStyle SaveNothing";
/// The same start line for Windows PowerShell 5.1.
const NESTED_51: &str = "powershell -NoLogo -NoProfile -NoExit -Command Set-PSReadLineOption -HistorySaveStyle SaveNothing";

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

struct ChanSink(mpsc::Sender<Vec<u8>>);

impl OutputSink for ChanSink {
    fn output(&mut self, bytes: &[u8]) {
        let _ = self.0.send(bytes.to_vec());
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The history file of the session `id` (WATT_HISTFILE for the outer shell). Never the user's own history file.
fn history_path(id: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("watt-windows-integrate-{id}-{}.history", std::process::id()))
}

/// Starts a session the way the app does. A renderer thread takes the output, acks it, and answers each cursor position
/// query the way xterm.js does (a terminal reply goes through `write_user` as it is). Queries from nested shells are
/// answered too, since they arrive on the same output.
fn start(id: &str) -> Arc<Session> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let histfile = history_path(id);
    let _ = std::fs::remove_file(&histfile);
    let cfg = SpawnConfig { rows: 30, cols: 120, real_zdotdir: None, histfile: Some(histfile) };
    let session = Arc::new(Session::spawn(id, cfg, Box::new(ChanSink(tx))).expect("spawn the PowerShell session"));
    let weak: Weak<Session> = Arc::downgrade(&session);
    thread::spawn(move || {
        let mut seen: Vec<u8> = Vec::new();
        let mut answered = 0;
        for chunk in rx {
            seen.extend_from_slice(&chunk);
            while let Some(i) = find(&seen[answered..], DSR) {
                answered += i + DSR.len();
                if let Some(s) = weak.upgrade() {
                    let _ = s.write_user(b"\x1b[1;1R");
                }
            }
            if let Some(s) = weak.upgrade() {
                s.ack(chunk.len());
            }
        }
    });
    session
}

fn wait_until<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + WAIT;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= end {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_state(s: &Session, want: InputState) {
    let ok = wait_until(|| (s.state() == want).then_some(())).is_some();
    assert!(ok, "state never reached {want:?} within {WAIT:?}; state={:?} screen={:?}", s.state(), s.screen_snapshot().rows);
}

fn wait_prompt(s: &Session) {
    wait_state(s, InputState::Prompt);
}

/// Submits `cmd` at an idle prompt, waits for its record to finish and for the next prompt. Returns the finished record.
fn run(s: &Session, cmd: &str) -> Record {
    wait_prompt(s);
    s.submit(Source::User, cmd).expect("submit at an idle prompt");
    let id = wait_until(|| s.records().iter().find(|r| r.command == cmd && r.exit_code.is_some()).map(|r| r.id))
        .unwrap_or_else(|| panic!("no finished record for {cmd:?}; records={:?}", s.records()));
    wait_prompt(s);
    s.get_record(id).expect("record by id")
}

/// The finished record of `command` at `level`. The caller checks the state separately.
fn finished(s: &Session, command: &str, level: u64) -> Record {
    let id = wait_until(|| {
        s.records().iter().find(|r| r.command == command && r.level == level && r.exit_code.is_some()).map(|r| r.id)
    })
    .unwrap_or_else(|| panic!("no finished record for {command:?} at level {level}; records={:?}", s.records()));
    s.get_record(id).expect("record by id")
}

/// True when the last non-blank row of the screen is a PowerShell prompt (`PS ...>`).
fn at_ps_prompt(s: &Session) -> bool {
    let rows = s.screen_snapshot().rows;
    rows.iter().rev().find(|r| !r.trim().is_empty()).is_some_and(|r| {
        let t = r.trim_end();
        t.starts_with("PS ") && t.ends_with('>')
    })
}

/// The nested pwsh, integrated from the outer session: its commands are attributed to level 1, a native exit code and the
/// output come through, `exit` pops back to the outer level, and the outer level still runs commands afterwards.
#[test]
fn nested_powershell_is_integrated_and_runs_commands_then_exits_back() {
    let _g = serial();
    let s = start("win-nested");
    wait_prompt(&s);
    s.submit(Source::User, NESTED).expect("submit the nested shell");

    let shown = wait_until(|| (s.foreground().map(|f| f.kind) == Some(ForegroundKind::Shell)).then_some(()));
    assert!(shown.is_some(), "the nested shell is not a foreground Shell: {:?} state={:?}", s.foreground(), s.state());
    assert!(wait_until(|| at_ps_prompt(&s).then_some(())).is_some(), "the nested shell never drew its prompt: {:?}", s.screen_snapshot().rows);
    assert!(wait_until(|| (!s.integration_starting()).then_some(())).is_some(), "the nested shell is still starting");

    let (key, sh) = match s.integrate_shell() {
        IntegrateOutcome::Integrated { key, sh, .. } => (key, sh),
        other => panic!("integration did not succeed: {other:?}; screen: {:?}", s.screen_snapshot().rows),
    };
    assert_eq!((key, sh.as_str()), (1, "pwsh"));
    wait_prompt(&s);
    assert_eq!(s.levels().len(), 2, "levels={:?}", s.levels());
    assert_eq!(s.top_key(), 1);
    assert_eq!(s.location().as_deref(), Some(NESTED));

    s.submit(Source::Agent { call_id: "nested-hi".into() }, "echo hi").expect("agent submit at depth 2");
    let hi = finished(&s, "echo hi", 1);
    assert_eq!((hi.exit_code, hi.level), (Some(0), 1), "records={:?}", s.records());
    assert_eq!(hi.output, "hi", "output={:?}", hi.output);
    assert_eq!(hi.location.as_deref(), Some(NESTED));
    assert!(matches!(&hi.source, Source::Agent { call_id } if call_id == "nested-hi"), "{:?}", hi.source);
    wait_prompt(&s);

    s.submit(Source::User, "cmd /c exit 3").expect("user submit at depth 2");
    let native = finished(&s, "cmd /c exit 3", 1);
    assert_eq!(native.exit_code, Some(3), "native exit code at depth 2");
    wait_prompt(&s);

    s.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(|| (s.levels().len() == 1 && s.state() == InputState::Prompt).then_some(())).is_some(),
        "exit did not pop back: state {:?}, levels {:?}, screen {:?}",
        s.state(),
        s.levels(),
        s.screen_snapshot().rows
    );

    let local = run(&s, "echo outer");
    assert_eq!((local.exit_code, local.level), (Some(0), 0), "records={:?}", s.records());
    assert_eq!(local.output, "outer");
    assert_eq!(s.stats().forged_events, 0, "events were rejected for the nonce");
    drop(s);
    let _ = std::fs::remove_file(history_path("win-nested"));
}

/// A nested Windows PowerShell 5.1 gets `&&`, which is a parse error there. The line closes its record with a non-zero exit,
/// and the next command at that level runs.
#[test]
fn ps51_and_operator_line_finishes_and_the_next_command_runs() {
    let _g = serial();
    let s = start("win-ps51-and");
    wait_prompt(&s);
    s.submit(Source::User, NESTED_51).expect("submit the 5.1 shell");
    assert!(wait_until(|| at_ps_prompt(&s).then_some(())).is_some(), "5.1 never drew its prompt: {:?}", s.screen_snapshot().rows);
    assert!(wait_until(|| (!s.integration_starting()).then_some(())).is_some(), "5.1 still starting");
    assert!(matches!(s.integrate_shell(), IntegrateOutcome::Integrated { key: 1, .. }), "5.1 not integrated");
    wait_prompt(&s);

    s.submit(Source::Agent { call_id: "ps51-and".into() }, "echo a && echo b").expect("agent submit at depth 2");
    let and = finished(&s, "echo a && echo b", 1);
    assert!(and.exit_code.is_some_and(|c| c != 0), "exit={:?} output={:?}", and.exit_code, and.output);
    assert!(and.output.contains("&&"), "output={:?}", and.output);
    wait_prompt(&s);

    s.submit(Source::User, "echo ok").expect("user submit at depth 2");
    let ok = finished(&s, "echo ok", 1);
    assert_eq!((ok.exit_code, ok.output.as_str()), (Some(0), "ok"));
    wait_prompt(&s);
    drop(s);
    let _ = std::fs::remove_file(history_path("win-ps51-and"));
}
