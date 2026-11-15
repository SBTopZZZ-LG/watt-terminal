//! Windows-only end-to-end test of the local session: a real PowerShell on ConPTY with the integration script
//! (core/shim/integration.ps1). It does not use common/mod.rs, which starts zsh. Every wait has a timeout.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use watt_terminal_lib::core::observer::{InputState, IntegrationStatus, ObserverStatus};
use watt_terminal_lib::core::pty::OutputSink;
use watt_terminal_lib::core::record::{Record, RecordStatus, Source};
use watt_terminal_lib::core::session::{Session, SpawnConfig, SubmitError};

static SERIAL: Mutex<()> = Mutex::new(());
const WAIT: Duration = Duration::from_secs(30);
/// Cursor position query. pwsh sends it at startup and waits for the answer.
const DSR: &[u8] = b"\x1b[6n";

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

/// The history file of the session `id`. Every test session gets one (WATT_HISTFILE), so no test writes to the user's
/// PSReadLine history.
fn history_path(id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("watt-windows-session-{id}-{}.history", std::process::id()))
}

/// Starts a session the way the app does. A renderer thread takes the output and acks it, and answers each cursor
/// position query the way xterm.js does: the reply goes back through `write_user`, which writes terminal replies as they are.
fn start(id: &str) -> Arc<Session> {
    start_sized(id, 120)
}

fn start_sized(id: &str, cols: u16) -> Arc<Session> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let histfile = history_path(id);
    let _ = std::fs::remove_file(&histfile);
    let cfg = SpawnConfig { rows: 30, cols, real_zdotdir: None, histfile: Some(histfile) };
    let session = Arc::new(Session::spawn(id, cfg, Box::new(ChanSink(tx))).expect("spawn the PowerShell session"));
    let weak: Weak<Session> = Arc::downgrade(&session);
    thread::spawn(move || {
        let mut seen: Vec<u8> = Vec::new();
        // Queries before this index are answered already.
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

/// Waits for a continuation prompt and gives PSReadLine time to read input again. The 133;P is sent from the Enter
/// handler, before ValidateAndAcceptLine returns, so the observer can say Continuation while the handler is still
/// running. Keys written then can be lost. The row check waits for the continuation prompt on screen, the sleep
/// lets the handler return.
fn wait_continuation(s: &Session) {
    wait_state(s, InputState::Continuation);
    let ok = wait_until(|| s.screen_snapshot().rows.iter().any(|r| r.trim_end() == ">").then_some(())).is_some();
    assert!(ok, "no continuation row on screen within {WAIT:?}; screen={:?}", s.screen_snapshot().rows);
    thread::sleep(Duration::from_millis(200));
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

/// Forward slashes, no trailing slash, lower case: the form to compare two Windows paths in.
fn norm(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase()
}

/// True while a process with this pid is listed by tasklist.
fn process_alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("run tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

#[test]
fn powershell_session_runs_commands_reports_cwd_and_terminates() {
    let _g = serial();
    let s = start("win-e2e");
    wait_prompt(&s);
    assert_eq!(s.integration_status(), IntegrationStatus::Live, "Hello was not seen");
    assert_eq!(s.observer_status(), ObserverStatus::Ok);
    assert_eq!(s.stats().forged_events, 0, "events were rejected for the nonce");
    let pid = s.shell_pid().expect("shell pid");

    // PowerShell runs each line of a multi-line submit as its own command, so it is refused like bash, and nothing is written.
    assert_eq!(s.submit(Source::User, "echo one\necho two"), Err(SubmitError::MultiLineAtBash));
    assert_eq!(s.state(), InputState::Prompt, "a refused submit leaves the prompt as it was");

    let echo = run(&s, "echo hello");
    assert_eq!(echo.exit_code, Some(0));
    assert_eq!(echo.status, RecordStatus::Complete);
    assert!(echo.output.contains("hello"), "output={:?}", echo.output);

    let native = run(&s, "cmd /c exit 3");
    assert_eq!(native.exit_code, Some(3), "native exit code; pipestatus={:?}", native.pipestatus);

    // The cwd comes from OSC 7 (`/C:/...` on the wire). It must be an absolute Windows path that names the directory.
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    run(&s, &format!("Set-Location -LiteralPath '{}'", dir.replace('\'', "''")));
    let cwd = s.cwd().expect("cwd after Set-Location");
    assert!(Path::new(&cwd.path).is_absolute(), "cwd is not absolute: {:?}", cwd.path);
    assert_eq!(norm(&cwd.path), norm(&dir), "cwd {:?}", cwd.path);

    // Dropping the session must end the shell and return. Run the drop on another thread so a hang fails the test.
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        drop(s);
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(WAIT).expect("dropping the session hung");
    assert!(wait_until(|| (!process_alive(pid)).then_some(())).is_some(), "shell pid {pid} is still running after drop");
}

#[test]
fn backtick_continuation_is_one_record() {
    let _g = serial();
    let s = start("win-backtick");
    wait_prompt(&s);
    let before = s.records().len();
    s.submit(Source::User, "echo a `").expect("submit at an idle prompt");
    wait_continuation(&s);
    assert_eq!(s.records().len(), before, "the incomplete line made a record: {:?}", s.records());
    s.write_user(b"b\r").expect("write the second line");
    let id = wait_until(|| s.records().iter().find(|r| r.command.starts_with("echo a `") && r.exit_code.is_some()).map(|r| r.id))
        .unwrap_or_else(|| panic!("no finished record; records={:?}", s.records()));
    wait_prompt(&s);
    assert_eq!(s.records().len(), before + 1, "records={:?}", s.records());
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.status, RecordStatus::Complete);
    assert!(r.command.ends_with('b'), "command={:?}", r.command);
    // echo gets two arguments and writes one object per argument, so the output is one word per line.
    assert_eq!(r.output, "a\nb", "the command's own rows are not output");
}

#[test]
fn record_output_is_exactly_the_command_output() {
    let _g = serial();
    let s = start("win-output");
    let echo = run(&s, "echo hi");
    assert_eq!(echo.output, "hi", "output={:?}", echo.output);
    let range = run(&s, "1..3");
    assert_eq!(range.output, "1\n2\n3", "output={:?}", range.output);
}

/// Waits until a screen row contains `text`, then a short settle so PSReadLine has rendered the keys it read.
fn wait_screen_contains(s: &Session, text: &str) {
    let ok = wait_until(|| s.screen_snapshot().rows.iter().any(|r| r.contains(text)).then_some(())).is_some();
    assert!(ok, "{text:?} never reached the screen; screen={:?}", s.screen_snapshot().rows);
    thread::sleep(Duration::from_millis(200));
}

#[test]
fn enter_with_the_cursor_moved_to_the_start_of_a_wrapped_command_records_only_its_output() {
    let _g = serial();
    let s = start_sized("win-wrap", 80);
    wait_prompt(&s);
    // The comment pads the command past 80 columns so it wraps. The output is one short word.
    let cmd = format!("Write-Output 'ok' #{}", "x".repeat(100));
    s.write_user(cmd.as_bytes()).expect("type the command");
    s.write_user(b"\x1b[H").expect("Home");
    s.write_user(b"\r").expect("Enter");
    let id = wait_until(|| s.records().iter().find(|r| r.command == cmd && r.exit_code.is_some()).map(|r| r.id))
        .unwrap_or_else(|| panic!("no finished record; records={:?}", s.records()));
    wait_prompt(&s);
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output, "ok", "the command's rows are in the output: {:?}", r.output);
}

#[test]
fn enter_with_the_cursor_moved_up_in_a_multi_line_command_records_only_its_output() {
    let _g = serial();
    let s = start_sized("win-multi", 120);
    wait_prompt(&s);
    s.write_user(b"if ($true) {\r").expect("write the first line");
    wait_continuation(&s);
    s.write_user(b"Write-Output 'ok'\r").expect("write the second line");
    wait_screen_contains(&s, "Write-Output 'ok'");
    s.write_user(b"}").expect("write the closing brace");
    wait_screen_contains(&s, "}");
    s.write_user(b"\x1b[A").expect("Up: the cursor moves off the last row");
    thread::sleep(Duration::from_millis(200));
    s.write_user(b"\r").expect("Enter");
    let id = wait_until(|| s.records().iter().find(|r| r.command.starts_with("if ($true) {") && r.exit_code.is_some()).map(|r| r.id))
        .unwrap_or_else(|| panic!("no finished record; records={:?}", s.records()));
    wait_prompt(&s);
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0));
    assert!(r.command.ends_with('}'), "command={:?}", r.command);
    assert_eq!(r.output, "ok", "the command's rows are in the output: {:?}", r.output);
}

#[test]
fn enter_survives_a_runtime_vi_edit_mode_switch() {
    let _g = serial();
    let s = start("win-vi");
    run(&s, "Set-PSReadLineOption -EditMode Vi");
    run(&s, "echo throwaway");
    let r = run(&s, "echo a");
    assert_eq!(r.exit_code, Some(0), "after the Vi switch");
    assert_eq!(r.output, "a");
}

#[test]
fn enter_rebound_at_runtime_records_later_lines() {
    let _g = serial();
    let s = start("win-rebind");
    run(&s, "Set-PSReadLineKeyHandler -Key Enter -Function AcceptLine");
    let r = run(&s, "echo after-rebind");
    assert_eq!(r.exit_code, Some(0), "after the rebind");
    assert_eq!(r.output, "after-rebind");
}

#[test]
fn a_prompt_redefined_after_integration_is_wrapped_again() {
    let _g = serial();
    let s = start("win-prompt");
    wait_prompt(&s);
    s.submit(Source::User, "function prompt { 'X> ' }").expect("submit at an idle prompt");
    // The redefined prompt sends no A, so the observer is still Running. The next line goes through the Enter handler,
    // which wraps the prompt again.
    wait_state(&s, InputState::Running);
    s.write_user(b"echo throwaway\r").expect("write the throwaway line");
    wait_until(|| s.records().iter().find(|r| r.command == "echo throwaway" && r.exit_code.is_some()).map(|_| ()))
        .unwrap_or_else(|| panic!("the throwaway line did not finish; records={:?}", s.records()));
    wait_prompt(&s);
    assert_eq!(s.integration_status(), IntegrationStatus::Live);
    let r = run(&s, "echo b");
    assert_eq!(r.exit_code, Some(0), "after the prompt was redefined");
    assert_eq!(r.output, "b");
}

#[test]
fn no_op_lines_after_a_failed_command_exit_zero() {
    let _g = serial();
    let s = start("win-noop");
    let native = run(&s, "cmd /c exit 3");
    assert_eq!(native.exit_code, Some(3), "native exit code");
    let comment = run(&s, "# x");
    assert_eq!(comment.exit_code, Some(0), "comment after a failed command");
    assert_eq!(comment.status, RecordStatus::Complete);
    run(&s, "cmd /c exit 3");
    let func = run(&s, "function WattNoOpProbe { }");
    assert_eq!(func.exit_code, Some(0), "function definition after a failed command");
}

#[test]
fn open_block_is_a_continuation_until_it_is_closed() {
    let _g = serial();
    let s = start("win-block");
    wait_prompt(&s);
    let before = s.records().len();
    s.submit(Source::User, "if ($true) {").expect("submit at an idle prompt");
    wait_continuation(&s);
    assert_eq!(s.records().len(), before, "the open block made a record: {:?}", s.records());
    s.write_user(b"}\r").expect("write the closing brace");
    let id = wait_until(|| s.records().iter().find(|r| r.command.starts_with("if ($true) {") && r.exit_code.is_some()).map(|r| r.id))
        .unwrap_or_else(|| panic!("no finished record; records={:?}", s.records()));
    wait_prompt(&s);
    assert_eq!(s.records().len(), before + 1, "records={:?}", s.records());
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0));
    assert!(r.command.ends_with('}'), "command={:?}", r.command);
}

#[test]
fn ctrl_c_at_the_continuation_prompt_returns_to_the_prompt() {
    let _g = serial();
    let s = start("win-ctrlc");
    wait_prompt(&s);
    let before = s.records().len();
    s.submit(Source::User, "if ($true) {").expect("submit at an idle prompt");
    wait_continuation(&s);
    s.write_user(b"\x03").expect("send Ctrl-C");
    wait_prompt(&s);
    assert_eq!(s.records().len(), before, "the cancelled block made a record: {:?}", s.records());
    let after = run(&s, "echo after-ctrl-c");
    assert_eq!(after.exit_code, Some(0));
}

#[test]
fn watt_histfile_receives_the_session_history() {
    let _g = serial();
    let s = start("win-hist");
    let marker = "echo watt-histfile-probe";
    run(&s, marker);
    let path = history_path("win-hist");
    let found = wait_until(|| std::fs::read_to_string(&path).ok().filter(|t| t.contains(marker)).map(|_| ())).is_some();
    let _ = std::fs::remove_file(&path);
    assert!(found, "{} does not contain {marker:?}", path.display());
}
