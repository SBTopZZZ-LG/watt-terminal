//! Windows-only end-to-end test of the local session: a real PowerShell on ConPTY with the integration script
//! (core/shim/integration.ps1). It does not use common/mod.rs, which starts zsh. Every wait has a timeout.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use watt_terminal_lib::core::observer::{InputState, IntegrationStatus, ObserverStatus};
use watt_terminal_lib::core::pty::OutputSink;
use watt_terminal_lib::core::record::{Record, RecordStatus, Source};
use watt_terminal_lib::core::session::{Session, SpawnConfig, SubmitError, USER_TEXT_CODE};
use watt_terminal_lib::core::tools::{AgentTools, DEFAULT_RUN_WAIT, NotRunReason, RunOutcome, SessionTools};

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
    start_capturing(id, cols).0
}

/// Like `start_sized`, and also keeps every raw output byte the renderer received, for counting OSC events.
fn start_capturing(id: &str, cols: u16) -> (Arc<Session>, Arc<Mutex<Vec<u8>>>) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let histfile = history_path(id);
    let _ = std::fs::remove_file(&histfile);
    let cfg = SpawnConfig { rows: 30, cols, real_zdotdir: None, histfile: Some(histfile) };
    let session = Arc::new(Session::spawn(id, cfg, Box::new(ChanSink(tx))).expect("spawn the PowerShell session"));
    let log: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let log_thread = Arc::clone(&log);
    let weak: Weak<Session> = Arc::downgrade(&session);
    thread::spawn(move || {
        let mut seen: Vec<u8> = Vec::new();
        // Queries before this index are answered already.
        let mut answered = 0;
        for chunk in rx {
            log_thread.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk);
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
    (session, log)
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

/// Waits up to `limit` (not WAIT) for the observer to reach the prompt. Used where the repair must come without input.
fn wait_prompt_within(s: &Session, limit: Duration) {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if s.state() == InputState::Prompt {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "state never reached Prompt within {limit:?} without input; state={:?} records={:?} screen={:?}",
        s.state(),
        s.records(),
        s.screen_snapshot().rows
    );
}

/// Limit for the prompt-repair tests. A wedged shell must fail in seconds, not hold the suite for WAIT.
const REPAIR_WAIT: Duration = Duration::from_secs(6);

/// Waits up to `limit` for a finished record of `command` and returns its id.
fn finished_within(s: &Session, command: &str, limit: Duration) -> u64 {
    let end = Instant::now() + limit;
    loop {
        if let Some(id) = s.records().iter().find(|r| r.command == command && r.exit_code.is_some()).map(|r| r.id) {
            return id;
        }
        if Instant::now() >= end {
            panic!("no finished record for {command:?} within {limit:?}; state={:?} records={:?}", s.state(), s.records());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Submits `cmd` at a prompt that is reached within REPAIR_WAIT, then waits for its record and the next prompt.
fn run_within(s: &Session, cmd: &str) -> Record {
    wait_prompt_within(s, REPAIR_WAIT);
    s.submit(Source::User, cmd).expect("submit at an idle prompt");
    let id = finished_within(s, cmd, REPAIR_WAIT);
    wait_prompt_within(s, REPAIR_WAIT);
    s.get_record(id).expect("record by id")
}

/// How many times `needle` occurs in the captured raw output.
fn count_bytes(log: &Mutex<Vec<u8>>, needle: &[u8]) -> usize {
    let log = log.lock().unwrap_or_else(|e| e.into_inner());
    log.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Waits for a finished record of `command` and returns its id.
fn wait_finished(s: &Session, command: &str) -> u64 {
    wait_until(|| s.records().iter().find(|r| r.command == command && r.exit_code.is_some()).map(|r| r.id))
        .unwrap_or_else(|| panic!("no finished record for {command:?}; records={:?}", s.records()))
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
    // The redefined prompt sends no A. The idle hook (_WattIdle) wraps the prompt again without any input, so the
    // observer reaches the prompt on its own. Then a typed line still goes through the Enter handler normally.
    wait_prompt_within(&s, Duration::from_secs(3));
    assert_eq!(s.integration_status(), IntegrationStatus::Live);
    let throwaway = run(&s, "echo throwaway");
    assert_eq!(throwaway.exit_code, Some(0), "the throwaway line after the prompt was redefined");
    assert_eq!(s.stats().forged_events, 0);
    let r = run(&s, "echo b");
    assert_eq!(r.exit_code, Some(0), "after the prompt was redefined");
    assert_eq!(r.output, "b");
}

/// The one-line case from the QA report: the prompt is redefined and work follows on the same line. Nothing repairs it
/// except the idle hook, so the record must finish and the observer must reach the prompt without any further input.
#[test]
fn a_line_that_redefines_the_prompt_and_runs_work_finishes_without_more_input() {
    let _g = serial();
    let s = start("win-prompt-line");
    wait_prompt(&s);
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    run(&s, &format!("Set-Location -LiteralPath '{}'", dir.replace('\'', "''")));
    let before = s.records().len();
    let line = "function prompt { 'X> ' }; echo after-prompt";
    s.submit(Source::User, line).expect("submit at an idle prompt");
    let id = wait_finished(&s, line);
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.status, RecordStatus::Complete, "status; output={:?}", r.output);
    assert_eq!(r.exit_code, Some(0));
    assert!(r.output.contains("after-prompt"), "output={:?}", r.output);
    wait_prompt_within(&s, Duration::from_secs(3));
    assert_eq!(s.stats().forged_events, 0, "events were rejected for the nonce");
    let cwd = s.cwd().expect("cwd after the redefined prompt");
    assert_eq!(norm(&cwd.path), norm(&dir), "cwd {:?}", cwd.path);

    let second = run(&s, "echo second");
    assert_eq!(second.exit_code, Some(0), "the next line after the repair");
    assert_eq!(second.output, "second");
    let records = s.records();
    assert_eq!(records.len(), before + 2, "one record per command, no duplicates: {records:?}");
    assert_eq!(records.iter().filter(|r| r.command == line).count(), 1);
}

/// A redefined prompt with slow work after it: the idle hook does not run while a command runs, so the record must not
/// close before `late` is printed.
#[test]
fn a_redefined_prompt_followed_by_slow_work_does_not_finish_early() {
    let _g = serial();
    let s = start("win-prompt-slow");
    wait_prompt(&s);
    let line = "function prompt { 'X> ' }; Start-Sleep -Seconds 2; echo late";
    s.submit(Source::User, line).expect("submit at an idle prompt");
    thread::sleep(Duration::from_millis(800));
    assert_eq!(s.state(), InputState::Running, "the record closed during the sleep");
    assert!(
        !s.records().iter().any(|r| r.command == line && r.exit_code.is_some()),
        "the record finished during the sleep: {:?}",
        s.records()
    );
    let id = wait_finished(&s, line);
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.status, RecordStatus::Complete);
    // The replaced prompt text is printed before the idle hook writes D, so it ends up in the output ("late\nX>").
    assert!(r.output.starts_with("late"), "output={:?}", r.output);
    wait_prompt_within(&s, Duration::from_secs(3));
}

/// A line that redefines the prompt and changes directory: the cwd repair (OSC 7 and 633;Cwd) must come with the idle repair.
#[test]
fn a_redefined_prompt_line_that_changes_directory_updates_the_cwd() {
    let _g = serial();
    let s = start("win-prompt-cwd");
    wait_prompt(&s);
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    let line = format!("function prompt {{ 'X> ' }}; Set-Location -LiteralPath '{}'", dir.replace('\'', "''"));
    s.submit(Source::User, &line).expect("submit at an idle prompt");
    wait_finished(&s, &line);
    wait_prompt_within(&s, Duration::from_secs(3));
    let cwd = s.cwd().expect("cwd after the redefined prompt");
    assert!(Path::new(&cwd.path).is_absolute(), "cwd is not absolute: {:?}", cwd.path);
    assert_eq!(norm(&cwd.path), norm(&dir), "cwd {:?}", cwd.path);
}

/// A replacement prompt that is a copy of the wrapper's own source. The idle repair adopts the copy as the original prompt,
/// so the wrapper would call itself. The line must finish, the prompt must come back without input, and later lines must
/// record normally with one D per C.
#[test]
fn a_prompt_replaced_by_a_copy_of_the_wrapper_does_not_wedge() {
    let _g = serial();
    let (s, log) = start_capturing("win-prompt-copy", 120);
    wait_prompt(&s);
    let copy = "$d = (Get-Item function:prompt).ScriptBlock.ToString(); Set-Item function:prompt ([scriptblock]::Create($d))";
    s.submit(Source::User, copy).expect("submit at an idle prompt");
    let id = finished_within(&s, copy, REPAIR_WAIT);
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0), "the copy line; output={:?}", r.output);
    wait_prompt_within(&s, REPAIR_WAIT);

    let after = run_within(&s, "echo after-copy");
    assert_eq!(after.exit_code, Some(0), "echo after the copy; output={:?}", after.output);
    assert_eq!(after.output, "after-copy");
    let later = run_within(&s, "echo later");
    assert_eq!(later.exit_code, Some(0), "a later command; output={:?}", later.output);
    assert_eq!(later.output, "later");
    // The copy must not become the prompt text: the prompt line after the output still shows a prompt (ends in ">").
    let rows = s.screen_snapshot().rows;
    let last = rows.iter().rev().find(|r| !r.trim().is_empty()).cloned().unwrap_or_default();
    assert!(last.trim_end().ends_with('>'), "no prompt text after the copy was repaired; last row={last:?} screen={rows:?}");

    assert_eq!(s.stats().forged_events, 0, "events were rejected for the nonce");
    assert_eq!(s.records().iter().filter(|r| r.command == "echo after-copy").count(), 1, "records={:?}", s.records());
    let (c, d) = (count_bytes(&log, b"\x1b]133;C;"), count_bytes(&log, b"\x1b]133;D;"));
    assert_eq!((c, d), (3, 3), "C and D counts (copy, echo, later)");
    drop_within(s, Duration::from_secs(5));
}

/// A replacement prompt that calls the wrapper, the posh-git/oh-my-posh chaining pattern. The chain is adopted, so the
/// wrapper would call the chain and the chain would call the wrapper. The chained text must stay on the prompt line and
/// events must keep flowing for each later command.
#[test]
fn a_prompt_that_chains_the_wrapper_does_not_wedge() {
    let _g = serial();
    let (s, log) = start_capturing("win-prompt-chain", 120);
    wait_prompt(&s);
    let chain = "$old = (Get-Item function:prompt).ScriptBlock; function prompt { & $old; 'Y> ' }";
    s.submit(Source::User, chain).expect("submit at an idle prompt");
    let id = finished_within(&s, chain, REPAIR_WAIT);
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0), "the chain line; output={:?}", r.output);
    wait_prompt_within(&s, REPAIR_WAIT);

    let first = run_within(&s, "echo after-chain");
    assert_eq!(first.exit_code, Some(0), "echo after the chain; output={:?}", first.output);
    assert_eq!(first.output, "after-chain");
    let rows = s.screen_snapshot().rows;
    let last = rows.iter().rev().find(|r| !r.trim().is_empty()).cloned().unwrap_or_default();
    // The chained text is the whole prompt row: no leading space from the inner call. The snapshot trims trailing spaces.
    assert_eq!(last.trim_end(), "Y>", "the chained prompt row is not exactly the chained text; screen={rows:?}");

    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    run_within(&s, &format!("Set-Location -LiteralPath '{}'", dir.replace('\'', "''")));
    let cwd = s.cwd().expect("cwd after Set-Location");
    assert_eq!(norm(&cwd.path), norm(&dir), "cwd {:?}", cwd.path);
    let second = run_within(&s, "echo chain-2");
    assert_eq!(second.exit_code, Some(0), "output={:?}", second.output);
    assert_eq!(second.output, "chain-2");
    let third = run_within(&s, "echo chain-3");
    assert_eq!(third.exit_code, Some(0), "output={:?}", third.output);
    assert_eq!(third.output, "chain-3");

    assert_eq!(s.stats().forged_events, 0, "events were rejected for the nonce");
    assert_eq!(s.records().iter().filter(|r| r.command == "echo after-chain").count(), 1, "records={:?}", s.records());
    let (c, d) = (count_bytes(&log, b"\x1b]133;C;"), count_bytes(&log, b"\x1b]133;D;"));
    assert_eq!(c, d, "C and D counts differ");
    drop_within(s, Duration::from_secs(5));
}

/// With the prompt untouched, an idle shell after a finished command writes no OSC at all: the idle hook returns early.
#[test]
fn idle_prompt_after_a_command_writes_no_osc() {
    let _g = serial();
    let (s, log) = start_capturing("win-idle-quiet", 120);
    run(&s, "echo quiet");
    let mark = log.lock().unwrap_or_else(|e| e.into_inner()).len();
    thread::sleep(Duration::from_secs(3));
    let log = log.lock().unwrap_or_else(|e| e.into_inner());
    // Positive control: the capture sees the OSC events of the command, so an empty idle window means something.
    assert!(log[..mark].windows(2).any(|w| w == b"\x1b]"), "no OSC captured for the command");
    let idle = &log[mark..];
    let osc = idle.windows(2).filter(|w| *w == b"\x1b]").count();
    assert_eq!(osc, 0, "OSC bytes while idle: {:?}", String::from_utf8_lossy(idle));
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

/// The agent's own unclosed line: run_command returns WaitingForInput, then send_keys with Ctrl-C alone and no id cancels
/// it. The text the user typed into that continuation is cleared with it, so the next command runs.
#[test]
fn agent_send_keys_ctrl_c_cancels_its_unclosed_line() {
    let _g = serial();
    let s = start("win-agent-cancel");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let outcome = tools.run_command("echo \"abc", DEFAULT_RUN_WAIT, None).expect("run_command");
    assert!(matches!(outcome, RunOutcome::NotRun { reason: NotRunReason::WaitingForInput }), "got {outcome:?}");
    assert_eq!(s.state(), InputState::Continuation, "state={:?} screen={:?}", s.state(), s.screen_snapshot().rows);

    s.write_user(b"zqx").expect("the user types into the agent's continuation");
    let sent = tools.call("send_keys", &json!({"keys": ["Ctrl-C"]}), None).expect("send_keys Ctrl-C at the agent's continuation");
    assert_eq!(sent["outcome"], "sent", "{sent}");
    wait_prompt(&s);
    assert!(!s.records().iter().any(|r| r.command.contains("\"abc")), "the cancelled line became a record: {:?}", s.records());
    let ran = tools.run_command("echo ok", DEFAULT_RUN_WAIT, None).expect("run after the cancel");
    let RunOutcome::Finished { record } = ran else {
        panic!("expected a finished record");
    };
    assert_eq!(record.exit_code, Some(0), "output={:?}", record.output);
    assert_eq!(record.output, "ok");
}

/// A continuation the user typed is not the agent's line: send_keys Ctrl-C is refused there, and nothing is sent.
#[test]
fn agent_send_keys_ctrl_c_is_refused_at_a_continuation_the_user_typed() {
    let _g = serial();
    let s = start("win-agent-cancel-user");
    wait_prompt(&s);
    s.submit(Source::User, "echo \"abc").expect("submit at an idle prompt");
    wait_continuation(&s);
    let err = SessionTools::new(&s).call("send_keys", &json!({"keys": ["Ctrl-C"]}), None).expect_err("the user's continuation");
    assert!(err.contains("Continuation"), "{err}");
    assert_eq!(s.state(), InputState::Continuation, "the refused call changed the state");
    s.write_user(&[0x03]).expect("the user's ctrl-c");
    wait_prompt(&s);
    let after = run(&s, "echo after-user-cancel");
    assert_eq!(after.exit_code, Some(0));
}

/// The agent's command reads a line from the terminal, and the user answers it with Enter. The Enter ends the typed line
/// (the command read it), so the agent's next run_command is not refused by it.
#[test]
fn enter_answering_the_agents_command_does_not_refuse_the_next_one() {
    let _g = serial();
    let s = start("win-read-host");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let outcome = tools.run_command("Read-Host 'name'", DEFAULT_RUN_WAIT, None).expect("run_command");
    assert!(matches!(outcome, RunOutcome::Running { .. }), "got {outcome:?}");
    s.write_user(b"bob\r").expect("the user answers the prompt");
    wait_finished(&s, "Read-Host 'name'");
    wait_prompt(&s);
    let RunOutcome::Finished { record } = tools.run_command("echo next-ok", DEFAULT_RUN_WAIT, None).expect("not refused") else {
        panic!("expected a finished record");
    };
    assert_eq!(record.output, "next-ok");
}

/// The text on the cursor row. At a prompt, that is the prompt line.
fn prompt_line(s: &Session) -> String {
    let snap = s.screen_snapshot();
    snap.rows.get(snap.cursor.0 as usize).cloned().unwrap_or_default()
}

/// Text the user typed at the prompt (no Enter) is cleared by the agent's run_command: the command is submitted to a clean
/// line, its record holds only its own output, and the user's text is gone from the prompt line and was never run.
#[test]
fn text_typed_at_the_prompt_is_cleared_before_the_agents_command() {
    let _g = serial();
    let s = start("win-typed-cleared");
    wait_prompt(&s);
    s.write_user(b"echo user-typed").expect("the user types");
    wait_until(|| prompt_line(&s).contains("echo user-typed").then_some(())).expect("the typed text never reached the prompt line");
    assert!(s.user_typed_since_prompt(), "the typed text is not counted");

    let tools = SessionTools::new(&s);
    assert!(tools.run_command_precheck().is_ok(), "the precheck refused over the user's text");
    let RunOutcome::Finished { record } = tools.run_command("echo agent-after-clear", DEFAULT_RUN_WAIT, None).expect("runs after the clear")
    else {
        panic!("expected a finished record");
    };
    assert_eq!(record.command, "echo agent-after-clear", "the record holds more than the agent's line");
    assert!(matches!(record.status, RecordStatus::Complete), "status={:?}", record.status);
    assert_eq!(record.exit_code, Some(0), "output={:?}", record.output);
    assert_eq!(record.output, "agent-after-clear");
    wait_prompt(&s);
    assert!(!prompt_line(&s).contains("user-typed"), "the user's text is still on the prompt line: {:?}", prompt_line(&s));
    assert!(!s.user_typed_since_prompt(), "the typed-text flag is still set after the clear");
    assert!(!s.records().iter().any(|r| r.command.contains("user-typed")), "the user's text ran: {:?}", s.records());
}

/// The writer still refuses a submit over text typed after the clear: that is the backstop for text that arrives between
/// the clear and the submit. Nothing is written, and the text stays on the line.
#[test]
fn a_submit_over_text_typed_after_the_clear_is_refused_by_the_writer() {
    let _g = serial();
    let s = start("win-typed-backstop");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let RunOutcome::Finished { .. } = tools.run_command("echo first", DEFAULT_RUN_WAIT, None).expect("first run") else {
        panic!("expected a finished record");
    };
    wait_prompt(&s);
    s.write_user(b"typed-again").expect("the user types again");
    wait_until(|| prompt_line(&s).contains("typed-again").then_some(())).expect("the text never reached the prompt line");

    let err = s.submit(Source::User, "echo backstop-marker").expect_err("the writer submitted over the user's text");
    assert_eq!(err, SubmitError::UserTyped);
    assert!(err.to_string().starts_with(USER_TEXT_CODE), "error: {err}");
    assert!(!s.records().iter().any(|r| r.command.contains("backstop-marker")), "the refused command ran");
    assert!(prompt_line(&s).contains("typed-again"), "the refused submit changed the line: {:?}", prompt_line(&s));

    s.write_user(&[0x03]).expect("ctrl-c at the prompt");
    wait_until(|| (!prompt_line(&s).contains("typed-again")).then_some(())).expect("ctrl-c did not clear the line");
    wait_prompt(&s);
}

/// Waits (`limit`, not WAIT) for the observer to report the shell as Exited.
fn wait_exited_within(s: &Session, limit: Duration) {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if s.state() == InputState::Exited {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("state never reached Exited within {limit:?}; state={:?} screen={:?}", s.state(), s.screen_snapshot().rows);
}

/// Drops the session on another thread and fails if that takes longer than `limit`.
fn drop_within(s: Arc<Session>, limit: Duration) {
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        drop(s);
        let _ = done_tx.send(());
    });
    done_rx.recv_timeout(limit).expect("dropping the session hung");
}

/// The shell exits by itself (`exit` at the prompt). ConPTY never ends the reader while the master is open, so the session
/// must notice the exit itself: Exited within 3 s, the shell process gone, and the drop returns.
#[test]
fn a_shell_that_exits_by_itself_reports_exited() {
    let _g = serial();
    let s = start("win-exit");
    wait_prompt(&s);
    let pid = s.shell_pid().expect("shell pid");
    s.submit(Source::User, "exit").expect("submit exit at an idle prompt");
    wait_exited_within(&s, Duration::from_secs(3));
    assert!(wait_until(|| (!process_alive(pid)).then_some(())).is_some(), "shell pid {pid} is still running after exit");
    drop_within(s, Duration::from_secs(5));
}

/// Output printed before `exit` on the same line is not lost: the exit waits for the shell's last bytes.
#[test]
fn output_before_exit_on_the_same_line_is_kept() {
    let _g = serial();
    let s = start("win-exit-bye");
    wait_prompt(&s);
    s.submit(Source::User, "echo bye; exit").expect("submit at an idle prompt");
    wait_exited_within(&s, Duration::from_secs(3));
    let rows = s.screen_snapshot().rows;
    assert!(rows.iter().any(|r| r.trim() == "bye"), "the output line is not on screen; screen={rows:?}");
    drop_within(s, Duration::from_secs(5));
}

/// A command that runs for a while is Running the whole time, never Exited, and its output is recorded.
#[test]
fn a_slow_command_does_not_report_exited() {
    let _g = serial();
    let s = start("win-slow-live");
    wait_prompt(&s);
    let line = "Start-Sleep -Milliseconds 1500; echo slow-done";
    s.submit(Source::User, line).expect("submit at an idle prompt");
    thread::sleep(Duration::from_millis(800));
    assert_eq!(s.state(), InputState::Running, "not Running during the sleep; screen={:?}", s.screen_snapshot().rows);
    let id = wait_finished(&s, line);
    wait_prompt(&s);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(s.state(), InputState::Prompt, "the shell is still at its prompt after the command");
    let r = s.get_record(id).expect("record by id");
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output, "slow-done");
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

/// A line with a plain syntax error (not incomplete input). PowerShell must run it, print the parser error and draw the
/// next prompt, so the record closes with a non-zero exit and the next command runs. A line PSReadLine refuses to accept
/// never closes its record, so run_command would return Running here.
#[test]
fn a_line_with_a_parse_error_finishes_and_the_next_command_runs() {
    let _g = serial();
    let s = start("win-parse-error");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let outcome = tools.run_command("Write-Output )", DEFAULT_RUN_WAIT, None).expect("run_command");
    let RunOutcome::Finished { record } = outcome else {
        panic!("the parse-error line did not finish: {outcome:?}; state={:?} screen={:?}", s.state(), s.screen_snapshot().rows);
    };
    assert_ne!(record.exit_code, Some(0), "exit={:?} output={:?}", record.exit_code, record.output);
    assert!(record.output.contains("Unexpected token"), "output={:?}", record.output);
    wait_prompt_within(&s, REPAIR_WAIT);

    let RunOutcome::Finished { record: ok } = tools.run_command("echo ok", DEFAULT_RUN_WAIT, None).expect("run after the parse error")
    else {
        panic!("the command after the parse error did not finish; state={:?}", s.state());
    };
    assert_eq!((ok.exit_code, ok.output.as_str()), (Some(0), "ok"));
}

/// Clear-Host through the agent's run_command, after output on screen: the record keeps the output printed after the clear.
#[test]
fn agent_clear_host_keeps_the_output_after_it() {
    let _g = serial();
    let s = start("win-clear-host");
    wait_prompt(&s);
    let tools = SessionTools::new(&s);
    let RunOutcome::Finished { .. } = tools.run_command("1..40 | ForEach-Object { \"line-$_\" }", DEFAULT_RUN_WAIT, None).expect("run_command")
    else {
        panic!("the output lines did not finish");
    };
    let RunOutcome::Finished { record } = tools.run_command("Clear-Host; echo after-clear", DEFAULT_RUN_WAIT, None).expect("run_command")
    else {
        panic!("the Clear-Host command did not finish; state={:?}", s.state());
    };
    assert!(record.output.contains("after-clear"), "output={:?} status={:?}", record.output, record.status);
}
