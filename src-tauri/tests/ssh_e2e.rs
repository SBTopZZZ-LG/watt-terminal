//! End to end over real ssh (B5). A local `zsh -f`-style shell (the common harness) runs `ssh` into an Ubuntu container
//! that runs openssh-server. The app integrates the remote shell as level 1 with its own handshake, and then the agent
//! path (submit, approvals, cwd) and the observer's recovery paths are checked against that real remote shell.
//!
//! The tests are #[ignore]: they need docker (colima works), network for apt on the first run, and the image named in
//! AT_TEST_IMAGE (default ubuntu:24.04). The first run builds a local image `at-b5-sshd:<image>` with openssh-server,
//! zsh and procps; later runs reuse it. Each test starts its own container `at-b5-<pid>` on 127.0.0.1 with a free
//! port and a throwaway ed25519 key in a temp directory. Containers are removed by exact name.
//!
//! Run: cargo test --test ssh_e2e -- --ignored --test-threads=1 --nocapture

mod common;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use watt_terminal_lib::core::agent::{AgentCtx, AgentEvent, ApprovalGate, DENIED, TurnRules};
use watt_terminal_lib::core::approval::RegexPolicy;
use watt_terminal_lib::core::foreground::ForegroundKind;
use watt_terminal_lib::core::observer::{InputState, LevelKind};
use watt_terminal_lib::core::record::{Record, RecordStatus, RecordSummary, Source};
use watt_terminal_lib::core::session::{IntegrateOutcome, Session, SubmitError};
use watt_terminal_lib::core::settings::{ApprovalMode, ApprovalSettings};
use watt_terminal_lib::core::tools::{AgentTools, SessionTools};
use common::*;
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(30);
const SSH_WAIT: Duration = Duration::from_secs(60);
const CARD_WAIT: Duration = Duration::from_secs(10);

/// Installed once into the base image. Runs inside the container as root.
const PREP_SCRIPT: &str = "set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends openssh-server procps zsh
mkdir -p /run/sshd
ssh-keygen -A
useradd -m -s /bin/bash tester
useradd -m -s /usr/bin/zsh tester2
touch /home/tester2/.zshrc
chown tester2:tester2 /home/tester2/.zshrc";

/// The image for these tests: AT_TEST_IMAGE, or ubuntu:24.04.
fn image_ref() -> String {
    std::env::var("AT_TEST_IMAGE").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "ubuntu:24.04".into())
}

/// The local tag of the prepared image: `at-b5-sshd:<image with / and : turned into ->`.
fn base_image_tag(image: &str) -> String {
    let safe: String = image
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    format!("at-b5-sshd:{safe}")
}

fn docker(args: &[&str]) -> Output {
    Command::new("docker").args(args).output().unwrap_or_else(|e| panic!("cannot run docker: {e}"))
}

fn docker_ok(args: &[&str]) -> String {
    let out = docker(args);
    assert!(out.status.success(), "docker {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Runs docker with `input` on stdin (used to install the throwaway public key without writing a file in the container).
fn docker_stdin(args: &[&str], input: &str) {
    let mut child = Command::new("docker")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn docker");
    child.stdin.take().expect("stdin").write_all(input.as_bytes()).expect("write the key");
    let out = child.wait_with_output().expect("docker exec");
    assert!(out.status.success(), "docker {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// A container name that is removed by exact name when the guard drops. Any container of that name is removed first.
struct Named(String);

impl Named {
    fn new(name: &str) -> Named {
        let _ = docker(&["rm", "-f", name]);
        Named(name.to_string())
    }
}

impl Drop for Named {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

/// Builds the prepared image once (apt needs network). Returns its tag. An existing tag is reused as it is.
fn ensure_base_image(image: &str) -> String {
    let tag = base_image_tag(image);
    if docker(&["image", "inspect", &tag]).status.success() {
        return tag;
    }
    let name = format!("at-b5-{}", std::process::id());
    let _prep = Named::new(&name);
    docker_ok(&["run", "-d", "--name", &name, image, "sleep", "infinity"]);
    docker_ok(&["exec", &name, "sh", "-c", PREP_SCRIPT]);
    docker_ok(&["commit", &name, &tag]);
    tag
}

/// Installs `pubkey` as the only authorized key of `user`.
fn install_key(container: &str, user: &str, pubkey: &str) {
    let script = format!(
        "set -e; d=/home/{user}/.ssh; install -d -m 700 -o {user} -g {user} $d; cat > $d/authorized_keys; \
         chown {user}:{user} $d/authorized_keys; chmod 600 $d/authorized_keys"
    );
    docker_stdin(&["exec", "-i", container, "sh", "-c", &script], pubkey);
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").expect("bind a free port").local_addr().expect("local addr").port()
}

/// Waits until sshd on `port` sends its banner. A bare TCP accept is not enough: docker's proxy accepts before sshd is up.
fn wait_for_sshd(port: u16) {
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(mut sock) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = sock.set_read_timeout(Some(Duration::from_secs(3)));
            let mut banner = [0u8; 4];
            if sock.read_exact(&mut banner).is_ok() && &banner == b"SSH-" {
                return;
            }
        }
        assert!(Instant::now() < end, "sshd did not answer on 127.0.0.1:{port}");
        thread::sleep(Duration::from_millis(200));
    }
}

/// A running container with sshd on 127.0.0.1, and a throwaway key. Dropping it removes the container and the key.
struct Remote {
    container: Named,
    port: u16,
    key_dir: PathBuf,
    key: PathBuf,
}

impl Remote {
    fn start() -> Remote {
        let tag = ensure_base_image(&image_ref());
        let name = format!("at-b5-{}", std::process::id());
        let key_dir = unique_name("b5-key");
        // Built before anything fallible, so a failed start still removes the key directory and the container.
        let remote = Remote {
            container: Named::new(&name),
            port: free_port(),
            key: key_dir.join("id_ed25519"),
            key_dir,
        };
        fs::create_dir_all(&remote.key_dir).expect("create the key directory");
        fs::set_permissions(&remote.key_dir, fs::Permissions::from_mode(0o700)).expect("key directory mode");
        let out = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "at-b5-test", "-f"])
            .arg(&remote.key)
            .output()
            .expect("run ssh-keygen");
        assert!(out.status.success(), "ssh-keygen: {}", String::from_utf8_lossy(&out.stderr));
        fs::set_permissions(&remote.key, fs::Permissions::from_mode(0o600)).expect("key mode");
        let pubkey = fs::read_to_string(remote.key_dir.join("id_ed25519.pub")).expect("read the public key");

        let port_map = format!("127.0.0.1:{}:22", remote.port);
        docker_ok(&["run", "-d", "--name", &remote.container.0, "-p", &port_map, &tag, "/usr/sbin/sshd", "-D", "-e"]);
        install_key(&remote.container.0, "tester", &pubkey);
        install_key(&remote.container.0, "tester2", &pubkey);
        wait_for_sshd(remote.port);
        remote
    }

    /// The ssh command the user types. Host keys are not recorded (the container's keys are throwaway).
    fn ssh_command(&self, user: &str) -> String {
        format!(
            "ssh -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -p {} {user}@127.0.0.1",
            self.key.display(),
            self.port
        )
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.key_dir);
    }
}

/// The last three non-blank screen rows, owned (for messages and observations).
fn last_rows(s: &Session) -> Vec<String> {
    let rows = s.screen_snapshot().rows;
    rows.iter().rev().filter(|r| !r.trim().is_empty()).take(3).cloned().collect()
}

/// The label a nested level keeps for its command: the whole command, since it is far below LABEL_CAP (observer.rs).
fn expected_label(command: &str) -> String {
    command.to_string()
}

/// True when the last non-blank row ends in a prompt sign (`$`, `#` or zsh's `%`).
fn at_remote_prompt(s: &Session) -> bool {
    let rows = s.screen_snapshot().rows;
    rows.iter().rev().find(|r| !r.trim().is_empty()).is_some_and(|r| {
        let t = r.trim_end();
        t.ends_with('$') || t.ends_with('#') || t.ends_with('%')
    })
}

type Log = Arc<std::sync::Mutex<Vec<AgentEvent>>>;

/// Waits until the foreground program is `kind`. False on timeout.
fn wait_foreground(s: &Session, kind: ForegroundKind) -> bool {
    wait_until(WAIT, || (s.foreground().map(|f| f.kind) == Some(kind)).then_some(())).is_some()
}

/// Types the ssh command at the local prompt and waits for the remote prompt on the screen. Returns the command text.
fn connect(h: &Harness, remote: &Remote, user: &str) -> String {
    wait_prompt(h);
    let cmd = remote.ssh_command(user);
    h.submit(Source::User, &cmd).expect("submit ssh");
    assert!(wait_foreground(h, ForegroundKind::Ssh), "ssh did not start: state {:?}", h.state());
    let prompt = wait_until(SSH_WAIT, || at_remote_prompt(h).then_some(())).is_some();
    assert!(prompt, "no remote prompt: foreground {:?}, screen {:?}", h.foreground(), h.screen_snapshot().rows);
    // Integration types Ctrl-C first: the remote prompt must also have been still for a moment.
    assert!(wait_program_up(h, &cmd), "the remote prompt did not settle: screen {:?}", h.screen_snapshot().rows);
    cmd
}

/// Integrates the remote shell and returns (key, sh). Any other outcome fails the test with its details.
fn integrate_remote(h: &Harness, user: &str) -> (u64, String) {
    match h.integrate_shell() {
        IntegrateOutcome::Integrated { key, sh, user: got, .. } => {
            assert_eq!(got.as_deref(), Some(user), "the remote user");
            (key, sh)
        }
        other => panic!("integration did not succeed: {other:?}; screen: {:?}", h.screen_snapshot().rows),
    }
}

/// True when the top level is `key` and it is at its prompt.
fn at_prompt(h: &Harness, key: u64) -> bool {
    h.state() == InputState::Prompt && h.top_key() == key
}

fn wait_at_prompt(h: &Harness, key: u64) {
    assert!(
        wait_until(WAIT, || at_prompt(h, key).then_some(())).is_some(),
        "not at the prompt of level {key}: state {:?}, top {}, levels {:?}",
        h.state(),
        h.top_key(),
        h.levels()
    );
}

/// The first record that matches `what` and has finished. Waits for it.
fn finished_by(h: &Harness, what: impl Fn(&RecordSummary) -> bool) -> Record {
    let id = wait_until(WAIT, || h.records().iter().find(|r| what(r) && r.exit_code.is_some()).map(|r| r.id))
        .unwrap_or_else(|| panic!("no finished record; records={:?}", h.records()));
    h.get_record(id).expect("record by id")
}

/// A command the agent submits at `level`, through the same writer path as run_command. Waits for it and the prompt.
fn agent_cmd(h: &Harness, call: &str, cmd: &str, level: u64) -> Record {
    h.submit(Source::Agent { call_id: call.into() }, cmd).unwrap_or_else(|e| panic!("submit {cmd:?}: {e}"));
    let rec = finished_by(h, |r| matches!(&r.source, Source::Agent { call_id } if call_id == call));
    assert_eq!(rec.level, level, "{cmd:?} ran at the wrong level");
    wait_at_prompt(h, level);
    rec
}

/// A command the user types at `level`. Waits for it and the prompt.
fn typed_cmd(h: &Harness, cmd: &str, level: u64) -> Record {
    h.submit(Source::User, cmd).unwrap_or_else(|e| panic!("submit {cmd:?}: {e}"));
    let rec = finished_by(h, |r| r.command == cmd && r.level == level);
    wait_at_prompt(h, level);
    rec
}

/// Makes one run_command call the way the agent does (through the gate and `policy`), on its own thread, so the test can
/// answer the card. The thread's result is the call's result. The log holds the events it emitted.
fn agent_run(
    s: Arc<Session>,
    gate: Arc<ApprovalGate>,
    policy: RegexPolicy,
    command: &str,
) -> (thread::JoinHandle<Result<Value, String>>, Log) {
    let log: Log = Arc::default();
    let sink = log.clone();
    let command = command.to_string();
    let handle = thread::spawn(move || {
        let emit = move |ev: AgentEvent| sink.lock().unwrap().push(ev);
        let tools = SessionTools::new(&s);
        let ctx = AgentCtx::new(&tools, &gate, &emit).with_policy(&policy);
        ctx.call_tool("run_command", json!({"command": command}))
    });
    (handle, log)
}

/// The first approval card in `log`: its call id, reason code, level and location. Waits for it.
fn wait_card(log: &Log) -> (String, Option<&'static str>, u64, Option<String>) {
    wait_until(WAIT, || {
        log.lock().unwrap().iter().find_map(|e| match e {
            AgentEvent::ApprovalRequest { call_id, reason, level, location, .. } => {
                Some((call_id.clone(), *reason, *level, location.clone()))
            }
            _ => None,
        })
    })
    .expect("an approval card")
}

/// `(decision, ok)` of the run_command result in `log`, once the call has returned.
fn run_decision(log: &Log) -> Option<(&'static str, bool)> {
    log.lock().unwrap().iter().find_map(|e| match e {
        AgentEvent::ToolResult { name, decision, ok, .. } if name == "run_command" => Some((*decision, *ok)),
        _ => None,
    })
}

/// A policy whose allow list is `allow` (anchored, as the settings are). Nothing else is saved.
fn rules(allow: &[&str]) -> RegexPolicy {
    let approval = ApprovalSettings {
        mode: ApprovalMode::Regex,
        allow: allow.iter().map(|s| s.to_string()).collect(),
        deny: Vec::new(),
        ..Default::default()
    };
    RegexPolicy::new(&approval).expect("the rules compile")
}

/// The level-1 nonce: the one accepted nonce that is not the local shell's.
fn nested_nonce(h: &Harness) -> String {
    let local = h.nonce().to_string();
    let others: Vec<String> = h.nonces().into_iter().filter(|n| *n != local).collect();
    assert_eq!(others.len(), 1, "one nested nonce is accepted while level 1 is on top: {others:?}");
    others[0].clone()
}

/// The whole flow over ssh into bash: integration, agent commands with their output, exit codes, cwd, the approval rules
/// on this level, a user exit back to the local shell, and approve-all resuming there.
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn ssh_into_bash_integrates_and_the_agent_works_there() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();
    let s = h.arc();
    // Approve-all is switched on in the local shell, before the connection.
    let gate_all = Arc::new(ApprovalGate::with_timeout(CARD_WAIT));
    gate_all.set_auto_approve(true, h.top_key());

    let ssh = connect(&h, &remote, "tester");
    let label = expected_label(&ssh);
    let (key, sh) = integrate_remote(&h, "tester");
    assert_eq!((key, sh.as_str()), (1, "bash"));
    wait_at_prompt(&h, 1);
    let levels = h.levels();
    assert_eq!(levels.len(), 2, "{levels:?}");
    assert_eq!(
        (levels[1].kind, levels[1].label.as_str(), levels[1].sh.as_deref(), levels[1].user.as_deref()),
        (LevelKind::Remote, label.as_str(), Some("bash"), Some("tester")),
        "{levels:?}"
    );
    assert_eq!(h.location().as_deref(), Some(label.as_str()));

    // Commands the agent runs in the remote shell: output, exit code, level and location of each record.
    let hi = agent_cmd(&h, "b5-hi", "echo hi", 1);
    assert_eq!((hi.exit_code, hi.output.as_str()), (Some(0), "hi"));
    assert_eq!(hi.location.as_deref(), Some(label.as_str()));
    assert!(matches!(&hi.source, Source::Agent { call_id } if call_id == "b5-hi"), "{:?}", hi.source);

    let fals = typed_cmd(&h, "false", 1);
    assert_eq!(fals.exit_code, Some(1));

    let pwd = agent_cmd(&h, "b5-pwd", "pwd", 1);
    assert_eq!((pwd.exit_code, pwd.output.as_str()), (Some(0), "/home/tester"));

    let cd = agent_cmd(&h, "b5-cd", "cd /tmp && pwd", 1);
    assert_eq!((cd.exit_code, cd.output.as_str()), (Some(0), "/tmp"));
    assert!(
        wait_until(WAIT, || h.cwd().filter(|c| c.path == "/tmp").map(|_| ())).is_some(),
        "the remote cwd never reached the session: {:?}",
        h.cwd()
    );
    let via_tools = SessionTools::new(&h).get_cwd().expect("a cwd through the tools");
    assert_eq!(via_tools.path, "/tmp", "get_cwd follows the remote level");

    // Two identical commands. ignoreboth in the skel .bashrc keeps the second out of history, so its own text must still
    // name it.
    for call in ["b5-same-1", "b5-same-2"] {
        let rec = agent_cmd(&h, call, "echo same", 1);
        assert_eq!((rec.command.as_str(), rec.exit_code, rec.output.as_str()), ("echo same", Some(0), "same"), "{call}");
    }

    let quoted = agent_cmd(&h, "b5-quoted", r#"echo "a;b"; echo 'c d'"#, 1);
    assert_eq!(quoted.output.lines().collect::<Vec<_>>(), ["a;b", "c d"], "{quoted:?}");

    assert_eq!(h.submit(Source::Agent { call_id: "b5-multi".into() }, "echo one\necho two"), Err(SubmitError::MultiLineAtBash));

    // Approvals on this level. An allow rule that matches locally asks here, with the nested reason, and the card names the
    // remote shell. Approving it runs the command in the remote shell.
    let gate_rule = Arc::new(ApprovalGate::with_timeout(CARD_WAIT));
    let (call, log) = agent_run(s.clone(), gate_rule.clone(), rules(&["echo .*"]), "echo nested-rule");
    let (call_id, reason, level, location) = wait_card(&log);
    assert_eq!((reason, level, location.as_deref()), (Some("nested_level"), 1, Some(label.as_str())));
    assert!(gate_rule.decide(&call_id, true), "the card was still pending");
    let result = call.join().expect("the turn thread");
    assert!(result.is_ok(), "the approved command failed: {result:?}");
    assert_eq!(run_decision(&log), Some(("manual", true)));
    let approved = finished_by(&h, |r| r.command == "echo nested-rule" && r.level == 1);
    assert_eq!(approved.output, "nested-rule");
    wait_at_prompt(&h, 1);

    // Approve-all switched on in the local shell does not run anything here: the card asks with the suspended reason.
    let (denied, denied_log) = agent_run(s.clone(), gate_all.clone(), rules(&[]), "echo nested-all");
    let (call_id, reason, level, _) = wait_card(&denied_log);
    assert_eq!((reason, level), (Some("approve_all_suspended"), 1));
    assert!(gate_all.decide(&call_id, false), "the card was still pending");
    let err = denied.join().expect("the turn thread").expect_err("refused");
    assert!(err.starts_with(DENIED), "{err}");
    assert!(!h.records().iter().any(|r| r.command == "echo nested-all"), "a denied command ran");

    // The user types exit. The local shell is on top again, the remote command is Incomplete, and the ssh record finishes.
    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(())).is_some(),
        "exit did not return to the local prompt: state {:?}, levels {:?}",
        h.state(),
        h.levels()
    );
    let exited = h.records().into_iter().find(|r| r.command == "exit").expect("the exit record");
    assert_eq!((exited.level, exited.status), (1, RecordStatus::Incomplete));
    let parent = finished_by(&h, |r| r.command == ssh && r.level == 0);
    assert!(matches!(parent.status, RecordStatus::Complete | RecordStatus::Approximate), "{:?}", parent.status);
    assert!(parent.exit_code.is_some());

    // Approve-all resumes on the local shell, and a local command runs.
    let (again, again_log) = agent_run(s.clone(), gate_all.clone(), rules(&[]), "echo local-again");
    assert_eq!(again.join().expect("the turn thread").map(|_| ()), Ok(()));
    assert_eq!(run_decision(&again_log), Some(("auto", true)), "approve-all resumes on the local shell");
    let local = run(&h, "echo local-again-typed");
    assert_eq!((local.output.as_str(), local.level), ("local-again-typed", 0));
}

/// The phases of the integration events in `log`, in order.
fn integration_phases(log: &Log) -> Vec<&'static str> {
    log.lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Integration { phase, .. } => Some(*phase),
            _ => None,
        })
        .collect()
}

/// The agent asks for the integration over a real sshd. First the user is asked and accepts in the dialog. Then the ssh
/// session is left and reopened, the setting is auto, and the shell is integrated with no dialog. A command runs in each.
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn the_agent_requests_integration_over_ssh_asks_then_runs_without_asking() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();

    connect(&h, &remote, "tester");
    let asked: Log = Arc::default();
    let gate = ApprovalGate::with_timeout(WAIT);
    let emit = |ev: AgentEvent| {
        if let AgentEvent::Integration { call_id, phase: "asking", .. } = &ev {
            assert!(gate.integrations().decide(call_id, true), "the dialog's answer is waiting");
        }
        asked.lock().unwrap().push(ev);
    };
    let tools = SessionTools::new(&h);
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let text = ctx.call_tool("request_integration", json!({})).expect("integrated after the dialog");
    let text = text.as_str().unwrap_or_default().to_string();
    assert!(text.starts_with("Integrated ssh ") && text.contains("(bash as tester@"), "{text}");
    assert_eq!(integration_phases(&asked), ["asking", "running", "done"]);
    let key = h.top_key();
    assert_eq!(h.levels().len(), 2, "{:?}", h.levels());
    wait_at_prompt(&h, key);
    let hi = agent_cmd(&h, "b5r-hi", "echo asked", key);
    assert_eq!((hi.exit_code, hi.output.as_str()), (Some(0), "asked"));

    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(())).is_some(),
        "exit did not return to the local prompt: state {:?}, levels {:?}",
        h.state(),
        h.levels()
    );

    connect(&h, &remote, "tester");
    let auto: Log = Arc::default();
    let emit_auto = |ev: AgentEvent| auto.lock().unwrap().push(ev);
    let gate_auto = ApprovalGate::with_timeout(WAIT);
    let tools = SessionTools::new(&h);
    let rules = TurnRules { integrate_without_asking: true, ..TurnRules::default() };
    let ctx = AgentCtx::new(&tools, &gate_auto, &emit_auto).with_rules(rules);
    let text = ctx.call_tool("request_integration", json!({})).expect("integrated without asking");
    let text = text.as_str().unwrap_or_default().to_string();
    assert!(text.starts_with("Integrated ssh ") && text.contains("(bash as tester@"), "{text}");
    assert_eq!(integration_phases(&auto), ["running", "done"], "no dialog under auto");
    let key = h.top_key();
    wait_at_prompt(&h, key);
    let pwd = agent_cmd(&h, "b5r-pwd", "pwd", key);
    assert_eq!((pwd.exit_code, pwd.output.as_str()), (Some(0), "/home/tester"));

    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(())).is_some(),
        "the second exit did not return to the local prompt"
    );
}

/// The remote sshd session is killed while the remote shell is integrated. Its bash goes with it, ssh exits, and the local
/// shell prompts. The observer must pop the dead level within 5 s (no stuck Running) and the local shell must work.
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn a_dropped_connection_pops_the_remote_level_and_the_local_shell_works() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();
    let ssh = connect(&h, &remote, "tester");
    let (key, _) = integrate_remote(&h, "tester");
    assert_eq!(key, 1);
    wait_at_prompt(&h, 1);

    // The sshd session process of user tester (not the main sshd) is killed with SIGKILL.
    let out = docker(&["exec", &remote.container.0, "pkill", "-KILL", "-x", "-u", "tester", "sshd"]);
    assert!(out.status.success(), "pkill in the container: {}", String::from_utf8_lossy(&out.stderr));

    let popped = wait_until(Duration::from_secs(5), || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(()));
    let tail = last_rows(&h);
    eprintln!("OBSERVED screen after the drop (last rows): {tail:?}");
    assert!(popped.is_some(), "the dead remote level was not popped within 5 s: state {:?}, levels {:?}", h.state(), h.levels());
    let dead = finished_by(&h, |r| r.command == ssh && r.level == 0);
    eprintln!("OBSERVED ssh record after the drop: exit {:?}, status {:?}", dead.exit_code, dead.status);

    let local = run(&h, "echo local-after-drop");
    assert_eq!((local.output.as_str(), local.level), ("local-after-drop", 0));
}

/// `exec bash --norc` replaces the integrated remote bash with one that has no hooks, so it never prints a prompt marker.
/// The observer must not claim a prompt it did not see. Integrate is then tried again and its outcome is printed: the
/// outcome is observed behaviour, not a result the spec fixes.
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn exec_into_an_unhooked_shell_is_not_reported_as_a_prompt() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();
    let _ssh = connect(&h, &remote, "tester");
    let (key, _) = integrate_remote(&h, "tester");
    assert_eq!(key, 1);
    wait_at_prompt(&h, 1);

    h.submit(Source::User, "exec bash --norc").expect("submit exec");
    thread::sleep(Duration::from_secs(5));
    let state = h.state();
    let tail = last_rows(&h);
    eprintln!(
        "OBSERVED after exec bash --norc (5 s): state {state:?}, levels {:?}, foreground {:?}, last rows {tail:?}",
        h.levels(),
        h.foreground()
    );
    assert!(
        matches!(state, InputState::Running | InputState::Unresponsive),
        "the observer claims {state:?} with no prompt from the new shell"
    );

    let outcome = h.integrate_shell();
    eprintln!("OBSERVED integrate_shell after exec bash --norc: {outcome:?}; levels {:?}; state {:?}", h.levels(), h.state());

    // Whatever the outcome, the user must be able to get back to the local prompt by typing exit. Each exit is observed.
    for step in 1..=3 {
        if h.levels().len() == 1 && at_prompt(&h, 0) {
            break;
        }
        h.write_user(b"exit\r").expect("type exit");
        thread::sleep(Duration::from_secs(2));
        let levels: Vec<_> = h.levels().iter().map(|l| (l.key, l.state)).collect();
        eprintln!("OBSERVED after exit #{step}: levels {levels:?}, state {:?}", h.state());
    }
    assert!(
        wait_until(WAIT, || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(())).is_some(),
        "the local prompt did not come back: state {:?}, levels {:?}",
        h.state(),
        h.levels()
    );
    let local = run(&h, "echo local-after-exec");
    assert_eq!((local.output.as_str(), local.level), ("local-after-exec", 0));
}

/// Forged and retired prompt markers from the remote side. An unknown nonce is forged: counted, and nothing changes. The
/// nonce of the popped remote level is stale, not forged: after the user leaves the remote shell, a late marker with it
/// is not counted as forged, is counted as stale, and the local stack does not change.
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn forged_and_retired_prompt_markers_from_the_remote_side_change_nothing() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();
    let _ssh = connect(&h, &remote, "tester");
    let (key, _) = integrate_remote(&h, "tester");
    assert_eq!(key, 1);
    wait_at_prompt(&h, 1);
    let remote_nonce = nested_nonce(&h);
    let forged_before = h.stats().forged_events;

    let unknown = "f".repeat(32);
    let cmd = format!(r"printf '\033]133;A;n={unknown}\033\\'");
    let printed = agent_cmd(&h, "b5-forge", &cmd, 1);
    assert_eq!(printed.exit_code, Some(0));
    assert_eq!(h.stats().forged_events, forged_before + 1, "the unknown-nonce A was not counted as forged");
    assert_eq!((h.levels().len(), h.top_key()), (2, 1), "the forged marker changed the stack");
    wait_at_prompt(&h, 1);

    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(())).is_some(),
        "exit did not return to the local prompt: state {:?}, levels {:?}",
        h.state(),
        h.levels()
    );
    let forged_after_pop = h.stats().forged_events;
    let stale_after_pop = h.stats().stale_events;
    let late = format!(r"printf '\033]133;A;n={remote_nonce}\033\\'");
    let rec = typed_cmd(&h, &late, 0);
    assert_eq!(rec.exit_code, Some(0));
    assert_eq!(h.stats().forged_events, forged_after_pop, "a retired nonce was counted as forged");
    assert_eq!(h.stats().stale_events, stale_after_pop + 1, "the popped level's late marker was not counted as stale");
    assert_eq!((h.levels().len(), h.top_key()), (1, 0), "a late marker changed the stack");
    assert!(at_prompt(&h, 0), "state {:?}", h.state());
}

/// The same flow with a zsh login shell in the container (user tester2, with an empty ~/.zshrc so no new-user wizard).
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn ssh_into_zsh_integrates_and_the_agent_works_there() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();
    let ssh = connect(&h, &remote, "tester2");
    let label = expected_label(&ssh);
    let (key, sh) = integrate_remote(&h, "tester2");
    assert_eq!((key, sh.as_str()), (1, "zsh"));
    wait_at_prompt(&h, 1);

    let hi = agent_cmd(&h, "b5-zsh-hi", "echo hi", 1);
    assert_eq!((hi.exit_code, hi.output.as_str(), hi.location.as_deref()), (Some(0), "hi", Some(label.as_str())));
    let fals = typed_cmd(&h, "false", 1);
    assert_eq!(fals.exit_code, Some(1));

    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(())).is_some(),
        "exit did not return to the local prompt: state {:?}, levels {:?}",
        h.state(),
        h.levels()
    );
    let local = run(&h, "echo local-after-zsh");
    assert_eq!((local.output.as_str(), local.level), ("local-after-zsh", 0));
}

/// The remote bash's version as (major, minor), read inside the container.
fn remote_bash(container: &str) -> (u32, u32) {
    let text = docker_ok(&["exec", container, "bash", "-c", "echo ${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"]);
    let (major, minor) = text.split_once('.').expect("the remote bash version is major.minor");
    (major.parse().expect("major"), minor.parse().expect("minor"))
}

/// The agent runs `ssh` into the container with run_command, as a plain command. Bash 5.1 and later turn bracketed paste on
/// at their prompt, so the call returns early with reason remote_shell, at that prompt. Bash 4.4 does not, so it yields at
/// its wait (8 s here) with the timeout reason. The expectation follows the remote bash's version. Under auto-integration,
/// request_integration then integrates the remote shell, and run_command works in it.
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn ssh_run_command_returns_early_at_the_remote_prompt_and_integration_follows() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();
    wait_prompt(&h);
    let bash = remote_bash(&remote.container.0);
    let tools = SessionTools::new(&h);
    let started = Instant::now();
    let v = tools
        .call("run_command", &json!({"command": remote.ssh_command("tester"), "wait_seconds": 8}), None)
        .expect("run_command");
    let took = started.elapsed();
    println!(
        "ssh run_command, remote bash {}.{}: reason {}, returned after {} ms (waited_ms {})",
        bash.0, bash.1, v["reason"], took.as_millis(), v["waited_ms"]
    );
    assert_eq!(v["outcome"], "running", "{v}");
    if bash < (5, 1) {
        assert_eq!(v["reason"], "timeout", "bash {}.{} turns bracketed paste off: no early return: {v}", bash.0, bash.1);
        assert!(took >= Duration::from_secs(7), "returned after {took:?}, before the wait ended");
        return;
    }
    assert_eq!(v["reason"], "remote_shell", "{v}");
    assert!(took < Duration::from_secs(8), "returned after {took:?}, not early");
    assert_eq!(v["foreground"]["kind"], "ssh", "{v}");

    let gate = ApprovalGate::with_timeout(WAIT);
    let emit = |_ev: AgentEvent| {};
    let rules = TurnRules { integrate_without_asking: true, ..TurnRules::default() };
    let ctx = AgentCtx::new(&tools, &gate, &emit).with_rules(rules);
    let text = ctx.call_tool("request_integration", json!({})).expect("integrated without asking");
    let text = text.as_str().unwrap_or_default().to_string();
    assert!(text.starts_with("Integrated ssh ") && text.contains("(bash as tester@"), "{text}");
    let key = h.top_key();
    wait_at_prompt(&h, key);

    let pwd = tools.call("run_command", &json!({"command": "pwd", "wait_seconds": 10}), None).expect("run_command in the remote shell");
    assert_eq!(pwd["outcome"], "finished", "{pwd}");
    assert_eq!(pwd["record"]["output"], "/home/tester", "{pwd}");
    assert_eq!(pwd["record"]["level"], key, "{pwd}");

    h.write_user(b"exit\r").expect("type exit");
    assert!(
        wait_until(WAIT, || (h.levels().len() == 1 && at_prompt(&h, 0)).then_some(())).is_some(),
        "exit did not return to the local prompt: levels {:?}",
        h.levels()
    );
}

/// Un-integrated ssh: run_command returns `remote_shell` at the remote prompt. send_keys then types into that shell. Each Enter
/// asks, since the shell has no integration, and approve-all in the local shell does not cover it without follow_local.
/// request_integration still works after that.
#[test]
#[ignore = "needs docker, network for apt on the first run, and the image in AT_TEST_IMAGE (default ubuntu:24.04)"]
fn send_keys_types_into_an_unintegrated_remote_shell_with_approval_and_integration_still_works() {
    let _g = serial();
    let remote = Remote::start();
    let h = start();
    wait_prompt(&h);
    let bash = remote_bash(&remote.container.0);
    let tools = SessionTools::new(&h);
    let v = tools
        .call("run_command", &json!({"command": remote.ssh_command("tester"), "wait_seconds": 8}), None)
        .expect("run_command");
    assert_eq!(v["outcome"], "running", "{v}");
    if bash < (5, 1) {
        println!("bash {}.{} turns bracketed paste off: no early return, so this test has nothing to check", bash.0, bash.1);
        return;
    }
    assert_eq!(v["reason"], "remote_shell", "{v}");
    let id = v["record_id"].as_u64().expect("the record id");

    let gate = Arc::new(ApprovalGate::with_timeout(WAIT));
    let log: Log = Arc::default();
    let (sink, answers) = (log.clone(), gate.clone());
    let emit = move |ev: AgentEvent| {
        if let AgentEvent::ApprovalRequest { call_id, .. } = &ev {
            assert!(answers.decide(call_id, true), "the card was waiting");
        }
        sink.lock().unwrap().push(ev);
    };
    let ctx = AgentCtx::new(&tools, &gate, &emit);
    let sent = ctx
        .call_tool("send_keys", json!({"text": "echo hi", "keys": ["Enter"], "id": id}))
        .expect("the Enter asks, then is typed");
    assert_eq!(sent["outcome"], "sent", "{sent}");
    assert!(
        wait_until(WAIT, || h.screen_snapshot().rows.iter().any(|r| r.trim() == "hi").then_some(())).is_some(),
        "the remote shell printed hi: {:?}",
        h.screen_snapshot().rows
    );

    gate.set_auto_approve(true, h.top_key());
    ctx.call_tool("send_keys", json!({"text": "echo again", "keys": ["Enter"]})).expect("asks again under approve-all");
    let reasons: Vec<&'static str> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalRequest { name, reason, .. } if name == "send_keys" => *reason,
            _ => None,
        })
        .collect();
    assert_eq!(reasons, ["unintegrated_shell", "unintegrated_shell"], "every Enter in the un-integrated shell asked");

    let rules = TurnRules { integrate_without_asking: true, ..TurnRules::default() };
    let ctx = AgentCtx::new(&tools, &gate, &emit).with_rules(rules);
    let text = ctx.call_tool("request_integration", json!({})).expect("integrated after the sends");
    let text = text.as_str().unwrap_or_default().to_string();
    assert!(text.starts_with("Integrated ssh ") && text.contains("(bash as tester@"), "{text}");
}
