//! Real-shell tests for the injection handshake (B2a). A shell runs in a pty under a temporary HOME with no rc files.
//! The test types the loader line, waits for Ready, writes the blob and END_MARKER, waits for Hello, runs commands,
//! and parses everything the shell printed with the scanner. The blob's nonce is added as level 1, so events name
//! their level. The bash cases skip with a note when no bash >= 4.4 is installed. The docker case is #[ignore].

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_term_lib::core::inject::{blob_lines, loader_line, new_token, END_MARKER};
use agent_term_lib::core::scanner::{Event, Scanner};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

const PROMPT_WAIT: Duration = Duration::from_secs(10);
const READY_WAIT: Duration = Duration::from_secs(5);
const HELLO_WAIT: Duration = Duration::from_secs(5);
const COMMAND_WAIT: Duration = Duration::from_secs(5);
const REFUSAL_WAIT: Duration = Duration::from_secs(3);

/// A shell in a pty. A reader thread collects everything it prints.
struct Shell {
    _master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    out: Arc<Mutex<Vec<u8>>>,
    /// The nonce the blob installs. The scanner knows it as level 1.
    nonce: String,
    /// The scanner's level 0 nonce. Nothing in the shell uses it, so it only has to differ from `nonce`.
    local: String,
}

impl Shell {
    fn spawn(cmd: CommandBuilder) -> Shell {
        let pair = native_pty_system()
            .openpty(PtySize { rows: 40, cols: 200, pixel_width: 0, pixel_height: 0 })
            .expect("openpty");
        let child = pair.slave.spawn_command(cmd).expect("spawn the shell");
        // The parent must drop the slave, or EOF is never seen on the master.
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("pty reader");
        let writer = pair.master.take_writer().expect("pty writer");
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&out);
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Shell {
            _master: pair.master,
            writer,
            child,
            out,
            nonce: new_token().expect("nonce"),
            local: new_token().expect("nonce"),
        }
    }

    fn send(&mut self, text: &str) {
        self.writer.write_all(text.as_bytes()).expect("write to the pty");
        self.writer.flush().expect("flush the pty");
    }

    fn raw(&self) -> Vec<u8> {
        self.out.lock().unwrap().clone()
    }

    fn dump(&self) -> String {
        format!("{:?}", String::from_utf8_lossy(&self.raw()))
    }

    /// Every event the scanner finds in the output so far, with the level of the nonce that tagged it.
    fn events(&self) -> Vec<(Event, Option<u64>)> {
        let mut sc = Scanner::new(&self.local);
        sc.add_nonce(1, &self.nonce);
        sc.feed(&self.raw()).into_iter().map(|f| (f.event, f.level)).collect()
    }

    /// Polls the output until `done` holds, or `within` passes. Returns whether it held.
    fn wait_output(&self, within: Duration, done: impl Fn(&[u8]) -> bool) -> bool {
        let start = Instant::now();
        loop {
            if done(&self.raw()) {
                return true;
            }
            if start.elapsed() > within {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Polls the scanned events until `done` holds, or `within` passes. Returns whether it held.
    fn wait_events(&self, within: Duration, done: impl Fn(&[(Event, Option<u64>)]) -> bool) -> bool {
        let start = Instant::now();
        loop {
            if done(&self.events()) {
                return true;
            }
            if start.elapsed() > within {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The handshake the app runs: Ctrl-C, loader, Ready, blob, END_MARKER, Hello. Panics with the output on failure.
    fn integrate(&mut self, sh: &str) {
        assert!(self.wait_output(PROMPT_WAIT, |b| !b.is_empty()), "no prompt from {sh}");
        self.send("\x03");
        let token = new_token().expect("token");
        self.send(&format!("{}\r", loader_line(&token)));
        let want_token = token.clone();
        let want_sh = sh.to_string();
        assert!(
            self.wait_events(READY_WAIT, move |evs| evs.iter().any(|(e, _)| {
                matches!(e, Event::Ready { token: t, sh: s } if *t == want_token && *s == want_sh)
            })),
            "no Ready for {sh}; output: {}",
            self.dump()
        );
        for line in blob_lines(&self.nonce, sh).expect("blob") {
            self.send(&format!("{line}\r"));
        }
        self.send(&format!("{END_MARKER}\r"));
        let want_sh = sh.to_string();
        assert!(
            self.wait_events(HELLO_WAIT, move |evs| evs.iter().any(|(e, l)| {
                matches!(e, Event::Hello { sh: Some(s), .. } if *s == want_sh) && *l == Some(1)
            })),
            "no Hello from {sh}; output: {}",
            self.dump()
        );
    }

    /// Types one command and waits until its exit is reported and the next prompt has started.
    fn run(&mut self, command: &str) {
        let before = ends(&self.events());
        self.send(&format!("{command}\r"));
        assert!(
            self.wait_events(COMMAND_WAIT, |evs| ends(evs) > before && level_sequence(evs).last().map(String::as_str) == Some("A")),
            "{command}: no CommandEnd and prompt; output: {}",
            self.dump()
        );
    }

    /// True when the tty has ECHO on, read with `stty -a` run in the shell.
    fn echo_is_on(&mut self) -> bool {
        let mark = self.raw().len();
        self.run("stty -a");
        let text = String::from_utf8_lossy(&strip_osc(&self.raw()[mark..])).into_owned();
        let words: Vec<&str> = text.split_whitespace().collect();
        words.contains(&"echo") && !words.contains(&"-echo")
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A temporary HOME that is removed on drop. Declare it before the shell so the shell is dropped first.
struct TempHome(PathBuf);

impl TempHome {
    fn new(label: &str) -> TempHome {
        let dir = std::env::temp_dir().join(format!("at-b2a-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp home");
        TempHome(dir)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A local shell with no rc files, HOME and cwd at `home`, and a private history file.
fn local_command(program: &str, args: &[&str], home: &Path) -> CommandBuilder {
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.cwd(home);
    cmd.env("HOME", home);
    cmd.env("TERM", "xterm-256color");
    cmd.env("HISTFILE", home.join(".history"));
    cmd.env_remove("ZDOTDIR");
    cmd.env_remove("AGENTTERM_NONCE");
    cmd.env_remove("ENV");
    cmd.env_remove("BASH_ENV");
    cmd
}

/// Number of CommandEnd events from the blob's shell (level 1).
fn ends(evs: &[(Event, Option<u64>)]) -> usize {
    evs.iter().filter(|(e, l)| matches!(e, Event::CommandEnd { .. }) && *l == Some(1)).count()
}

/// The level 1 events that matter here, as text: `E <command>`, `C`, `D <exit>`, `A`. Repeated `A` (prompt redraws) is
/// collapsed.
fn level_sequence(evs: &[(Event, Option<u64>)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (event, level) in evs {
        if *level != Some(1) {
            continue;
        }
        let item = match event {
            Event::CommandLine(text) => format!("E {text}"),
            Event::CommandStart => "C".to_string(),
            Event::CommandEnd { exit, .. } => format!("D {exit}"),
            Event::PromptStart => "A".to_string(),
            _ => continue,
        };
        if item == "A" && out.last().map(String::as_str) == Some("A") {
            continue;
        }
        out.push(item);
    }
    out
}

/// The output with every OSC sequence removed (ESC ] ... BEL, or ESC \). What is left is what a person would see as text.
fn strip_osc(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b']') {
            i += 2;
            while i < bytes.len() {
                if bytes[i] == 0x07 {
                    i += 1;
                    break;
                }
                if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// The level 1 sequence for the three commands `check_flow` types, from the first one on.
fn expected_three() -> Vec<String> {
    let lines = ["E echo hi", "C", "D 0", "A", "E false", "C", "D 1", "A", r#"E echo "a;b""#, "C", "D 0", "A"];
    lines.iter().map(|s| s.to_string()).collect()
}

/// Runs the three commands and checks the events, the leaks, the cwd and the tty.
fn check_flow(sh: &mut Shell, home: Option<&Path>) {
    sh.run("echo hi");
    sh.run("false");
    sh.run(r#"echo "a;b""#);

    let seq = level_sequence(&sh.events());
    let start = seq.iter().position(|s| s == "E echo hi").expect("the first command was reported");
    assert_eq!(seq[start..].to_vec(), expected_three(), "level 1 events");

    let cwds: Vec<String> = sh
        .events()
        .into_iter()
        .filter_map(|(e, l)| match e {
            Event::LevelCwd { path } if l == Some(1) => Some(path),
            _ => None,
        })
        .collect();
    let last_cwd = cwds.last().expect("a tagged Cwd for the level");
    if let Some(home) = home {
        let want = fs::canonicalize(home).expect("canonical home");
        let got = fs::canonicalize(last_cwd).expect("the reported cwd exists");
        assert_eq!(got, want, "the level's cwd is the shell's directory");
    } else {
        assert!(last_cwd.starts_with('/'), "an absolute cwd, got {last_cwd:?}");
    }

    let visible = String::from_utf8_lossy(&strip_osc(&sh.raw())).into_owned();
    assert!(!visible.contains(&sh.nonce), "the nonce is not printed outside OSC events");
    assert!(!visible.contains("_agentterm_inject_nonce"), "the blob is not echoed to the screen");

    assert!(sh.echo_is_on(), "the tty echo is back on after the blob (stty -a)");
}

#[test]
fn zsh_integrates_and_reports_each_command() {
    let home = TempHome::new("zsh");
    let mut sh = Shell::spawn(local_command("zsh", &["-f", "-i"], &home.0));
    sh.integrate("zsh");
    check_flow(&mut sh, Some(&home.0));
}

/// The first bash on this machine that is >= 4.4, as a program name or path. None when there is none.
fn modern_bash() -> Option<String> {
    ["/opt/homebrew/bin/bash", "/usr/local/bin/bash", "bash"].into_iter().find(|bin| {
        bash_version(bin).is_some_and(|(major, minor)| major > 4 || (major == 4 && minor >= 4))
    }).map(str::to_string)
}

/// `(major, minor)` of a bash binary, or None when it cannot be run.
fn bash_version(bin: &str) -> Option<(u32, u32)> {
    let out = Command::new(bin).args(["-c", "echo ${BASH_VERSINFO[0]} ${BASH_VERSINFO[1]}"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut parts = text.split_whitespace().map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??))
}

/// A second loader + blob in an integrated zsh is refused as already integrated: the message, and no second Hello.
#[test]
fn zsh_second_integration_is_refused_without_a_second_hello() {
    let home = TempHome::new("zsh-twice");
    let mut sh = Shell::spawn(local_command("zsh", &["-f", "-i"], &home.0));
    sh.integrate("zsh");
    let token = new_token().expect("token");
    sh.send(&format!("{}\r", loader_line(&token)));
    let want = token.clone();
    assert!(
        sh.wait_events(READY_WAIT, move |evs| evs.iter().any(|(e, _)| matches!(e, Event::Ready { token: t, .. } if *t == want))),
        "the second loader reaches Ready; output: {}",
        sh.dump()
    );
    for line in blob_lines(&sh.nonce, "zsh").expect("blob") {
        sh.send(&format!("{line}\r"));
    }
    sh.send(&format!("{END_MARKER}\r"));
    assert!(
        sh.wait_output(HELLO_WAIT, |b| String::from_utf8_lossy(b).contains("agentterm: already integrated")),
        "the second blob is refused; output: {}",
        sh.dump()
    );
    let hellos = sh.events().iter().filter(|(e, l)| matches!(e, Event::Hello { .. }) && *l == Some(1)).count();
    assert_eq!(hellos, 1, "no second Hello from an already integrated shell");
}

#[test]
fn bash_integrates_and_reports_each_command() {
    let Some(bin) = modern_bash() else {
        eprintln!("SKIPPED bash_integrates_and_reports_each_command: no bash >= 4.4 found (tried /opt/homebrew/bin/bash, /usr/local/bin/bash, bash on PATH)");
        return;
    };
    let home = TempHome::new("bash");
    let mut sh = Shell::spawn(local_command(&bin, &["--noprofile", "--norc", "-i"], &home.0));
    sh.integrate("bash");
    check_flow(&mut sh, Some(&home.0));
}

/// Ubuntu 24.04 in docker, bash with no profile. Needs docker and the image. Run with `cargo test -- --ignored docker`.
#[test]
#[ignore = "needs docker and ubuntu:24.04"]
fn docker_ubuntu_bash_integrates_and_reports_each_command() {
    let name = format!("at-b2a-{}", std::process::id());
    let _cleanup = ContainerGuard(name.clone());
    let mut cmd = CommandBuilder::new("docker");
    cmd.args(["run", "-it", "--rm", "--name", name.as_str(), "ubuntu:24.04", "bash", "--noprofile", "--norc"]);
    let mut sh = Shell::spawn(cmd);
    sh.integrate("bash");
    check_flow(&mut sh, None);
}

/// Removes the container by its exact name, whatever the test did.
struct ContainerGuard(String);

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
    }
}

/// bash 3.2 (macOS /bin/bash) and `bash --posix` are refused by the loader: the Unsupported line, and no Ready. A bash
/// that is >= 4.4 outside posix mode is not in this test. The 3.2 case runs only where /bin/bash is older than 4.4.
#[test]
fn old_bash_and_posix_mode_are_refused_without_ready() {
    let home = TempHome::new("refuse");
    let mut cases: Vec<Vec<&str>> = vec![vec!["--posix"]];
    match bash_version("/bin/bash") {
        Some((major, minor)) if major < 4 || (major == 4 && minor < 4) => cases.push(vec![]),
        other => eprintln!("NOTE: /bin/bash is {other:?}; the bash 3.2 case is skipped, the posix case still runs"),
    }
    for extra in cases {
        let mut args = extra.clone();
        args.extend(["--noprofile", "--norc", "-i"]);
        let mut sh = Shell::spawn(local_command("/bin/bash", &args, &home.0));
        assert!(sh.wait_output(PROMPT_WAIT, |b| !b.is_empty()), "no prompt from /bin/bash {extra:?}");
        sh.send("\x03");
        sh.send(&format!("{}\r", loader_line(&new_token().expect("token"))));
        assert!(
            sh.wait_output(REFUSAL_WAIT, |b| String::from_utf8_lossy(b).contains("Unsupported shell")),
            "/bin/bash {extra:?} was not refused; output: {}",
            sh.dump()
        );
        assert!(
            !sh.events().iter().any(|(e, _)| matches!(e, Event::Ready { .. } | Event::Hello { .. })),
            "/bin/bash {extra:?}: a refused shell must not say Ready or Hello"
        );
    }
}
