//! Shared harness for the real-PTY tests. Each session runs `zsh -l` through the shim with the user's real
//! startup files. zsh with oh-my-zsh's `share_history` appends every command to HISTFILE, so each test session
//! gets a temporary real ZDOTDIR whose wrapper files source the user's own $HOME files and then point HISTFILE
//! into a temp directory. The user's real history file is never written by these tests.
//! Sessions are killed with SIGKILL on drop. Tests run one at a time (a global lock) so timing
//! assertions are not disturbed by each other.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant};

use agent_term_lib::core::observer::{InputState, ScreenSnapshot};
use agent_term_lib::core::pty::OutputSink;
use agent_term_lib::core::record::{Record, Source};
use agent_term_lib::core::session::{Session, SpawnConfig};

static SERIAL: Mutex<()> = Mutex::new(());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

type Hook = Box<dyn FnMut(&[u8]) + Send>;

pub fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct ChanSink(pub mpsc::Sender<Vec<u8>>);

impl OutputSink for ChanSink {
    fn output(&mut self, bytes: &[u8]) {
        let _ = self.0.send(bytes.to_vec());
    }
}

pub fn unique_name(tag: &str) -> PathBuf {
    let n = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("agent-term-test-{}-{n}-{tag}", std::process::id()))
}

/// Temporary real ZDOTDIR: wrapper files that source the user's own $HOME files, then point HISTFILE at a temp file.
pub fn make_real_home() -> PathBuf {
    let dir = unique_name("zdot");
    fs::create_dir_all(&dir).expect("create real zdotdir");
    let source = |name: &str| format!("[[ -r \"$HOME/{name}\" ]] && source \"$HOME/{name}\"\n");
    fs::write(dir.join(".zshenv"), source(".zshenv")).expect("write");
    fs::write(dir.join(".zprofile"), source(".zprofile")).expect("write");
    fs::write(dir.join(".zlogin"), source(".zlogin")).expect("write");
    let mut rc = source(".zshrc");
    rc.push_str(&format!("HISTFILE='{}'\n", dir.join(".zsh_history").display()));
    fs::write(dir.join(".zshrc"), rc).expect("write");
    dir
}

/// A running session plus its temporary real ZDOTDIR. Dropping it kills the shell, then removes the directory.
pub struct Harness {
    session: Option<Arc<Session>>,
    pub home: PathBuf,
}

impl Harness {
    pub fn arc(&self) -> Arc<Session> {
        self.session.clone().expect("session")
    }
}

impl Deref for Harness {
    type Target = Session;
    fn deref(&self) -> &Session {
        self.session.as_ref().expect("session")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.session.take());
        let _ = fs::remove_dir_all(&self.home);
    }
}

/// Starts a session. A renderer thread receives every output chunk, runs `hook` on it, and acks it.
/// The renderer holds only a Weak reference, so dropping the Harness tears the session down.
pub fn start_with_hook(hook: Option<Hook>) -> Harness {
    start_inner(hook, None)
}

/// Like `start`, but the shell's HISTFILE is set to `histfile` by the shim (the AGENT_TERM_HISTFILE path).
pub fn start_with_histfile(histfile: &Path) -> Harness {
    start_inner(None, Some(histfile.to_path_buf()))
}

fn start_inner(hook: Option<Hook>, histfile: Option<PathBuf>) -> Harness {
    let home = make_real_home();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let id = format!("t{}", NEXT_ID.fetch_add(1, Ordering::SeqCst));
    let cfg = SpawnConfig { rows: 30, cols: 120, real_zdotdir: Some(home.clone()), histfile };
    let session = Arc::new(Session::spawn(&id, cfg, Box::new(ChanSink(tx))).expect("spawn session"));
    let weak: Weak<Session> = Arc::downgrade(&session);
    thread::spawn(move || {
        let mut hook = hook;
        for chunk in rx {
            if let Some(h) = hook.as_mut() {
                h(&chunk);
            }
            if let Some(s) = weak.upgrade() {
                s.ack(chunk.len());
            }
        }
    });
    Harness { session: Some(session), home }
}

pub fn start() -> Harness {
    start_with_hook(None)
}

pub fn wait_until<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + timeout;
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

pub fn wait_state(s: &Session, want: InputState, timeout: Duration) -> bool {
    wait_until(timeout, || (s.state() == want).then_some(())).is_some()
}

pub fn wait_prompt(s: &Session) {
    assert!(wait_state(s, InputState::Prompt, Duration::from_secs(20)), "never reached prompt; state={:?}", s.state());
}

/// Submits `cmd` at a prompt, waits for its D, then for the next prompt. Returns the completed record.
pub fn run(s: &Session, cmd: &str) -> Record {
    wait_prompt(s);
    s.submit(Source::User, cmd).expect("submit at prompt");
    let id = wait_until(Duration::from_secs(20), || {
        s.records().iter().find(|r| r.command == cmd && r.exit_code.is_some()).map(|r| r.id)
    })
    .unwrap_or_else(|| panic!("no finished record for {cmd:?}; records={:?}", s.records()));
    wait_prompt(s);
    s.get_record(id).expect("record by id")
}

/// Waits for an already-submitted command to finish, then for the prompt.
pub fn run_finish(s: &Session, cmd: &str) -> Record {
    let id = wait_until(Duration::from_secs(20), || {
        s.records().iter().find(|r| r.command == cmd && r.exit_code.is_some()).map(|r| r.id)
    })
    .unwrap_or_else(|| panic!("command did not finish: {cmd:?}"));
    wait_prompt(s);
    s.get_record(id).expect("record")
}

pub fn snapshot(s: &Session) -> ScreenSnapshot {
    s.screen_snapshot()
}
