//! Windows-only end-to-end test of the local session: a real PowerShell on ConPTY with the integration script
//! (core/shim/integration.ps1). It does not use common/mod.rs, which starts zsh. Every wait has a timeout.

#![cfg(windows)]

use std::path::Path;
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

/// Starts a session the way the app does. A renderer thread takes the output and acks it, and answers each cursor
/// position query the way xterm.js does: the reply goes back through `write_user`, which writes terminal replies as they are.
fn start(id: &str) -> Arc<Session> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let cfg = SpawnConfig { rows: 30, cols: 120, real_zdotdir: None, histfile: None };
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

fn wait_prompt(s: &Session) {
    let ok = wait_until(|| (s.state() == InputState::Prompt).then_some(())).is_some();
    assert!(ok, "no idle prompt within {WAIT:?}; state={:?} screen={:?}", s.state(), s.screen_snapshot().rows);
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
