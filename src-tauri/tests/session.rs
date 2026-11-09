//! Real-PTY tests for the session core. Each test runs `zsh -l` through the shim with a temporary real
//! ZDOTDIR (see common/mod.rs), so the user's history file is never touched.

mod common;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_term_lib::core::observer::{InputState, IntegrationStatus, ObserverStatus};
use agent_term_lib::core::record::{Cwd, RecordStatus, Source};
use agent_term_lib::core::session::{Session, SpawnConfig, SubmitError};
use agent_term_lib::core::shim;

use common::*;

#[test]
fn hello_and_first_prompt_within_5s() {
    let _g = serial();
    let s = start();
    let t = Instant::now();
    assert!(wait_state(&s, InputState::Prompt, Duration::from_secs(5)), "state after 5s: {:?}", s.state());
    let took = t.elapsed();
    assert!(took < Duration::from_secs(5));
    assert_eq!(s.integration_status(), IntegrationStatus::Live);
    assert_eq!(s.observer_status(), ObserverStatus::Ok);
    eprintln!("boot to first prompt: {took:?}, forged_events={}", s.stats().forged_events);
}

#[test]
fn echo_hi_has_exit_0_and_exact_output() {
    let _g = serial();
    let s = start();
    let r = run(&s, "echo hi");
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output, "hi");
    assert_eq!(r.source, Source::User);
    assert_eq!(r.status, RecordStatus::Complete);
    assert!(!r.used_alt_screen);
    assert!(r.cwd.is_some(), "cwd recorded at C");
}

#[test]
fn false_has_exit_1() {
    let _g = serial();
    let s = start();
    let r = run(&s, "false");
    assert_eq!(r.exit_code, Some(1));
    assert_eq!(r.pipestatus, vec![1]);
    assert_eq!(r.output, "");
}

#[test]
fn pipeline_then_second_command_on_one_line() {
    let _g = serial();
    let s = start();
    let r = run(&s, "echo a | false; echo b");
    assert_eq!(r.command, "echo a | false; echo b");
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output, "b");
}

#[test]
fn empty_enter_creates_no_record() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.write_user(b"\r").expect("write");
    thread::sleep(Duration::from_secs(2));
    let r = run(&s, "echo after-empty-enter");
    assert_eq!(r.output, "after-empty-enter");
    assert_eq!(s.records().len(), 1, "records: {:?}", s.records());
}

#[test]
fn multiline_paste_is_one_record() {
    let _g = serial();
    let s = start();
    let text = "echo line-one\necho line-two";
    let r = run(&s, text);
    assert_eq!(r.command, text);
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output, "line-one\nline-two");
    assert_eq!(s.records().len(), 1);
}

#[test]
fn cd_updates_cwd_host_qualified() {
    let _g = serial();
    let s = start();
    run(&s, "cd /tmp");
    let cwd: Cwd = s.cwd().expect("cwd known");
    assert_eq!(cwd.path, "/tmp");
    assert!(!cwd.host.is_empty());
}

#[test]
fn forged_untagged_d_is_rejected_and_does_not_finish_the_record() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = r"printf '\033]133;D;0\007'; sleep 1; echo forged-done";
    s.submit(Source::User, cmd).expect("submit");
    let forged = wait_until(Duration::from_secs(5), || (s.stats().forged_events >= 1).then_some(()));
    assert!(forged.is_some(), "untagged 133;D was not counted as forged");
    assert_eq!(s.state(), InputState::Running, "forged D must not end the running command");
    assert!(
        !s.records().iter().any(|r| r.command == cmd && r.exit_code.is_some()),
        "record finished by a forged event"
    );
    let r = run_finish(&s, cmd);
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.output, "forged-done");
}
#[test]
fn submit_is_refused_while_a_command_runs() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 3").expect("first submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(3)), "state={:?}", s.state());
    let second = s.submit(Source::Agent { call_id: "c1".into() }, "echo should-not-run");
    assert_eq!(second, Err(SubmitError::NotAtPrompt(InputState::Running)));
    let r = run_finish(&s, "sleep 3");
    assert_eq!(r.exit_code, Some(0));
    assert!(!s.records().iter().any(|r| r.command == "echo should-not-run"));
}

#[test]
fn concurrent_submits_one_wins() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let n = 4;
    let barrier = Arc::new(Barrier::new(n));
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let (s, b) = (s.arc(), barrier.clone());
            thread::spawn(move || {
                let cmd = format!("sleep 2; echo T{i}");
                b.wait();
                (cmd.clone(), s.submit(Source::User, &cmd))
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().expect("thread")).collect();
    let wins: Vec<_> = results.iter().filter(|(_, r)| r.is_ok()).collect();
    assert_eq!(wins.len(), 1, "expected exactly one accepted submit, got {results:?}");
    for (_, r) in results.iter().filter(|(_, r)| r.is_err()) {
        assert!(matches!(r, Err(SubmitError::NotAtPrompt(_))), "unexpected error {r:?}");
    }
    let winner = &wins[0].0;
    let r = run_finish(&s, winner);
    let expected = winner.rsplit("echo ").next().expect("echo suffix");
    assert_eq!(r.output, expected);
    assert_eq!(s.records().len(), 1);
}

#[test]
fn ctrl_c_interrupts_a_running_command() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.submit(Source::User, "sleep 30").expect("submit");
    assert!(wait_state(&s, InputState::Running, Duration::from_secs(3)));
    s.write_user(&[0x03]).expect("ctrl-c");
    let r = run_finish(&s, "sleep 30");
    assert_ne!(r.exit_code, Some(0));
}

#[test]
fn ctrl_l_at_prompt_clears_and_keeps_the_prompt_state() {
    let _g = serial();
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = seen.clone();
    let s = start_with_hook(Some(Box::new(move |c: &[u8]| sink.lock().unwrap().extend_from_slice(c))));
    wait_prompt(&s);
    run(&s, "echo before-clear");
    let records_before = s.records().len();
    let forged_before = s.stats().forged_events;
    let mark = seen.lock().unwrap().len();
    // A lone form feed (^L) at the prompt: zsh runs its clear-screen widget and redraws the prompt.
    s.write_user(b"\x0c").expect("ctrl-l");
    let cleared = wait_until(Duration::from_secs(5), || {
        seen.lock().unwrap()[mark..].windows(4).any(|w| w == b"\x1b[2J").then_some(())
    });
    assert!(cleared.is_some(), "no ESC[2J after ^L; output={:?}", String::from_utf8_lossy(&seen.lock().unwrap()[mark..]));
    // The redrawn prompt emits A and B again. That must not open a record, change the state, or count as forged.
    let redrawn = wait_until(Duration::from_secs(5), || {
        seen.lock().unwrap()[mark..].windows(7).any(|w| w == b"\x1b]133;A").then_some(())
    });
    assert!(redrawn.is_some(), "prompt was not redrawn after ^L");
    thread::sleep(Duration::from_millis(500));
    assert_eq!(s.state(), InputState::Prompt);
    assert_eq!(s.integration_status(), IntegrationStatus::Live);
    assert_eq!(s.observer_status(), ObserverStatus::Ok);
    assert_eq!(s.records().len(), records_before, "records: {:?}", s.records());
    assert_eq!(s.stats().forged_events, forged_before);
    let r = run(&s, "echo after-clear");
    assert_eq!(r.output, "after-clear");
}

#[test]
fn alt_screen_flag_and_used_alt_screen() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = r"printf '\033[?1049h'; sleep 1; printf '\033[?1049l'";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Alt, Duration::from_secs(3)), "state={:?}", s.state());
    assert!(snapshot(&s).alternate_screen, "snapshot should show alt screen");
    let r = run_finish(&s, cmd);
    assert!(r.used_alt_screen);
    assert!(!snapshot(&s).alternate_screen);
}

#[test]
fn seq_1_200_is_exact() {
    let _g = serial();
    let s = start();
    let r = run(&s, "seq 1 200");
    let want: Vec<String> = (1..=200).map(|i| i.to_string()).collect();
    assert_eq!(r.output, want.join("\n"));
}

#[test]
fn progress_bar_shows_final_state_only() {
    let _g = serial();
    let s = start();
    let r = run(&s, r"printf 'p1\rp2\rp3\n'");
    assert_eq!(r.output, "p3");
}

#[test]
fn long_unbroken_line_is_one_row() {
    let _g = serial();
    let s = start();
    let r = run(&s, "head -c 300 /dev/zero | tr '\\0' a; echo");
    assert_eq!(r.output, "a".repeat(300));
}

#[test]
fn long_unbroken_line_joins_across_scrolling() {
    let _g = serial();
    let s = start();
    run(&s, "seq 1 40");
    let r = run(&s, "head -c 3000 /dev/zero | tr '\\0' a; echo");
    assert_eq!(r.output, "a".repeat(3000), "joined output must not contain row breaks");
}

#[test]
fn resize_reaches_pty_and_screen() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    s.resize(40, 100).expect("resize");
    // Resize is applied by the observer thread, in order with queued bytes, so wait for it.
    assert!(wait_until(Duration::from_secs(3), || (snapshot(&s).size == (40, 100)).then_some(())).is_some(), "observer did not apply resize");
    let r = run(&s, "stty size");
    assert_eq!(r.output, "40 100");
}

#[test]
fn history_file_is_not_in_the_shim_dir_and_real_history_untouched() {
    let _g = serial();
    let home = std::env::var("HOME").expect("HOME");
    let real_hist = PathBuf::from(&home).join(".zsh_history");
    let before = meta_of(&real_hist);
    let s = start();
    let shim_dir = s.shim_dir().to_path_buf();
    let expected = s.home.join(".zsh_history").display().to_string();
    let r = run(&s, "print -r -- $HISTFILE");
    assert_eq!(r.output, expected);
    assert!(!Path::new(&r.output).starts_with(&shim_dir));
    assert!(!shim_dir.join(".zsh_history").exists());
    drop(s);
    assert_eq!(meta_of(&real_hist), before, "real ~/.zsh_history changed during the test");
}

/// The shim's own repair: /etc/zshrc on macOS sets HISTFILE inside the shim dir. With an empty real
/// ZDOTDIR (no user config, so nothing writes history), the shim must point HISTFILE at the real dir.
#[test]
fn histfile_repair_points_at_real_dir_when_etc_zshrc_sets_the_shim_path() {
    let _g = serial();
    let real = unique_name("empty-real");
    fs::create_dir_all(&real).expect("create");
    let shim_dir = shim::dir_for("repair-test");
    shim::write_to(&shim_dir).expect("write shim");
    let out = Command::new("zsh")
        .args(["-l", "-i", "-c", "print -r -- $HISTFILE"])
        .env_clear()
        .env("HOME", std::env::var("HOME").expect("HOME"))
        .env("PATH", std::env::var("PATH").expect("PATH"))
        .env("TERM", "xterm-256color")
        .env("ZDOTDIR", &shim_dir)
        .env("AGENTTERM_REAL_ZDOTDIR", &real)
        .env("AGENTTERM_NONCE", "00112233445566778899aabbccddeeff")
        .stdin(Stdio::null())
        .output()
        .expect("run zsh");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // Hello is printed first (ends with ST, ESC \), then the print output.
    let histfile = stdout.rsplit("\x1b\\").next().unwrap_or_default().trim().to_string();
    let _ = fs::remove_dir_all(&shim_dir);
    let _ = fs::remove_dir_all(&real);
    assert_eq!(histfile, real.join(".zsh_history").display().to_string(), "stdout={stdout:?}");
}

#[test]
fn user_dotfiles_are_not_modified() {
    let _g = serial();
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    let files = [".zshrc", ".zshenv", ".zprofile", ".zlogin"];
    let before: Vec<_> = files.iter().map(|f| meta_of(&home.join(f))).collect();
    let s = start();
    run(&s, "echo dotfiles");
    drop(s);
    let after: Vec<_> = files.iter().map(|f| meta_of(&home.join(f))).collect();
    assert_eq!(before, after);
}

#[test]
fn drop_removes_shim_dir_and_kills_shell() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let pid = s.shell_pid().expect("pid");
    let dir = s.shim_dir().to_path_buf();
    assert!(dir.join(".zshrc").exists());
    drop(s);
    assert!(!dir.exists(), "shim dir left behind");
    let alive = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|st| st.success())
        .unwrap_or(false);
    assert!(!alive, "shell pid {pid} still alive after drop");
}

/// The shell dying while a command is open (here the command kills its own shell) gives that command no D. The
/// record must still be closed, as incomplete, when the PTY reaches EOF.
#[test]
fn shell_exit_closes_the_open_record_as_incomplete() {
    let _g = serial();
    let s = start();
    wait_prompt(&s);
    let cmd = "kill -9 $$";
    s.submit(Source::User, cmd).expect("submit");
    assert!(wait_state(&s, InputState::Exited, Duration::from_secs(10)), "state={:?}", s.state());
    let records = s.records();
    let r = records.iter().find(|r| r.command == cmd).unwrap_or_else(|| panic!("no record for {cmd:?}: {records:?}"));
    assert_eq!(r.status, RecordStatus::Incomplete);
    assert_eq!(r.exit_code, None);
}

/// Regression for the pump's closed-gate path (pty.rs). Dropping a session while output is still in flight
/// must not hang. With `break` on a closed gate, the reader stopped draining the master while Session::drop
/// waited for the shell, and POC 4 saw about half of the 2 MiB-buffered cycles hang.
#[test]
fn drop_while_output_is_in_flight_never_hangs() {
    let _g = serial();
    // Cycles 0-4: submit and drop at once, renderer acking. Cycles 5-9: the renderer stops acking, so 2 MiB
    // piles up and the gate is closed (paused) when the drop happens.
    for cycle in 0..10 {
        drop_while_in_flight(cycle, cycle >= 5);
    }
}

fn drop_while_in_flight(cycle: usize, stalled: bool) {
    let tag = format!("drop-inflight-{cycle}");
    let home = make_real_home();
    let shim_dir = shim::dir_for(&tag);
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let cfg = SpawnConfig { rows: 30, cols: 120, real_zdotdir: Some(home.clone()), histfile: None };
    let s = Arc::new(Session::spawn(&tag, cfg, Box::new(ChanSink(tx))).expect("spawn session"));

    // Renderer: counts bytes and acks them unless stalled. It holds a Weak, so the test owns the Session.
    let received = Arc::new(AtomicUsize::new(0));
    let stall = Arc::new(AtomicBool::new(false));
    {
        let (weak, received, stall) = (Arc::downgrade(&s), received.clone(), stall.clone());
        thread::spawn(move || {
            for chunk in rx {
                received.fetch_add(chunk.len(), Ordering::SeqCst);
                if !stall.load(Ordering::SeqCst)
                    && let Some(s) = weak.upgrade()
                {
                    s.ack(chunk.len());
                }
            }
        });
    }

    wait_prompt(&s);
    let pid = s.shell_pid();
    stall.store(stalled, Ordering::SeqCst);
    s.submit(Source::User, "yes | head -c 5000000").expect("submit");
    if stalled {
        let got = wait_until(Duration::from_secs(20), || (received.load(Ordering::SeqCst) >= 2 << 20).then_some(()));
        assert!(got.is_some(), "cycle {cycle}: 2 MiB of output never arrived");
        thread::sleep(Duration::from_millis(300));
    }

    // The drop runs on a helper thread so a hang shows up as a timeout instead of blocking the test runner.
    let (done_tx, done_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        drop(s);
        let _ = done_tx.send(());
    });
    if done_rx.recv_timeout(Duration::from_secs(10)).is_err() {
        // Leave nothing running: kill the stuck shell's children, then the shell, by pid (never by name).
        if let Some(pid) = pid {
            let pid = pid.to_string();
            let _ = Command::new("pkill").args(["-KILL", "-P", &pid]).status();
            let _ = Command::new("kill").args(["-KILL", &pid]).status();
        }
        let _ = done_rx.recv_timeout(Duration::from_secs(5));
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&shim_dir);
        panic!("cycle {cycle} (stalled={stalled}): Session::drop did not return within 10 s; shell pid {pid:?} was killed");
    }
    let _ = fs::remove_dir_all(&home);
    assert!(!shim_dir.exists(), "cycle {cycle}: shim dir left behind");
}

/// AGENT_TERM_HISTFILE (dev only): the shell's HISTFILE is the requested path, and nothing reaches the real history.
#[test]
fn histfile_override_is_applied_and_real_history_untouched() {
    let _g = serial();
    let real_hist = PathBuf::from(std::env::var("HOME").expect("HOME")).join(".zsh_history");
    let before = meta_of(&real_hist);
    let want = unique_name("histfile-override");
    let marker = format!("override-marker-{}", std::process::id());
    let s = start_with_histfile(&want);
    let r = run(&s, "print -r -- $HISTFILE");
    assert_eq!(r.output, want.display().to_string());
    run(&s, &format!("echo {marker}"));
    // fc -W writes the in-memory history to HISTFILE, so the override file must hold the marker.
    run(&s, "fc -W");
    drop(s);
    let written = fs::read_to_string(&want).unwrap_or_default();
    let _ = fs::remove_file(&want);
    assert!(written.contains(&marker), "override file does not contain the marker; file={written:?}");
    assert_eq!(meta_of(&real_hist), before, "real ~/.zsh_history changed during the test");
    let real_text = fs::read_to_string(&real_hist).unwrap_or_default();
    assert!(!real_text.contains(&marker), "marker reached the real ~/.zsh_history");
}

/// Verifies the renderer's byte stream: BEGIN marker, then exactly `total` pattern bytes, then END.
struct Verify {
    pre: Vec<u8>,
    found: bool,
    off: usize,
    mism: usize,
    total: usize,
    post: Vec<u8>,
}

const BEGIN: &[u8] = b"BEGIN!";
const END: &[u8] = b"END!";

fn pattern_byte(i: usize) -> u8 {
    b'a' + (i % 26) as u8
}

impl Verify {
    fn new(total: usize) -> Self {
        Verify { pre: Vec::new(), found: false, off: 0, mism: 0, total, post: Vec::new() }
    }

    fn feed(&mut self, c: &[u8]) {
        if !self.found {
            self.pre.extend_from_slice(c);
            if let Some(p) = self.pre.windows(BEGIN.len()).position(|w| w == BEGIN) {
                self.found = true;
                let rest = self.pre.split_off(p + BEGIN.len());
                self.pre.clear();
                self.data(&rest);
            } else if self.pre.len() > 1 << 20 {
                let keep = self.pre.len() - BEGIN.len();
                self.pre.drain(..keep);
            }
            return;
        }
        self.data(c);
    }

    fn data(&mut self, d: &[u8]) {
        for &b in d {
            if self.off < self.total {
                if b != pattern_byte(self.off) {
                    self.mism += 1;
                }
                self.off += 1;
            } else if self.post.len() < 64 {
                self.post.push(b);
            }
        }
    }

    fn complete(&self) -> bool {
        self.found && self.off == self.total && self.mism == 0 && self.post.starts_with(END)
    }
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn overflow_desyncs_renderer_stays_exact_and_recovers_at_next_prompt() {
    let _g = serial();
    const TOTAL: usize = 100 * 1024 * 1024;
    let path = std::env::temp_dir().join(format!("agent-term-test-{}-100mb.txt", std::process::id()));
    let _cleanup = TempFile(path.clone());
    {
        let mut w = BufWriter::new(File::create(&path).expect("create"));
        let mut block = vec![0u8; 1 << 20];
        let mut off = 0;
        while off < TOTAL {
            let n = block.len().min(TOTAL - off);
            for (i, b) in block[..n].iter_mut().enumerate() {
                *b = pattern_byte(off + i);
            }
            w.write_all(&block[..n]).expect("write");
            off += n;
        }
        w.flush().expect("flush");
    }

    let verify = Arc::new(Mutex::new(Verify::new(TOTAL)));
    let v2 = verify.clone();
    let s = start_with_hook(Some(Box::new(move |c: &[u8]| v2.lock().unwrap().feed(c))));
    wait_prompt(&s);
    // The trailing `sleep 3` keeps the shell silent for 3 s after END. The observer works through its
    // backlog, hits the dropped gap, and stays Desynced until the prompt that follows the sleep. That
    // window is long enough to observe reliably.
    let cmd = format!("printf 'BEG''IN!'; cat '{}'; printf 'EN''D!'; sleep 3", path.display());
    s.submit(Source::User, &cmd).expect("submit cat");

    let started = Instant::now();
    loop {
        let done = verify.lock().unwrap().post.starts_with(END);
        if done {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(600), "cat did not finish: off={}", verify.lock().unwrap().off);
        thread::sleep(Duration::from_millis(20));
    }
    let saw_desynced = wait_until(Duration::from_secs(30), || (s.observer_status() == ObserverStatus::Desynced).then_some(())).is_some();
    let stats = s.stats();
    eprintln!("overflow run: stats={stats:?} saw_desynced={saw_desynced} elapsed={:?}", started.elapsed());
    assert!(stats.overflow_events > 0, "observer never overflowed; the path was not exercised");
    assert!(saw_desynced, "status never showed Desynced after the overflow");

    // The sleep ends, the prompt is drawn, and the observer re-syncs at that prompt.
    let recovered = wait_until(Duration::from_secs(60), || {
        (s.observer_status() == ObserverStatus::Ok && s.state() == InputState::Prompt).then_some(())
    });
    assert!(recovered.is_some(), "status did not return to ok at the next prompt: {:?} / {:?}", s.observer_status(), s.state());

    let v = verify.lock().unwrap();
    assert!(v.found, "BEGIN marker not seen");
    assert_eq!(v.off, TOTAL, "renderer received {} of {} payload bytes", v.off, TOTAL);
    assert_eq!(v.mism, 0, "renderer bytes out of order or corrupted");
    assert!(v.complete(), "END marker not seen after payload");
}

fn meta_of(p: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()))
}
